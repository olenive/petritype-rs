//! Deep concurrent firing: how the concurrent driver reaches the nets inside
//! a net, launches their firings beside the outer net's, and lands each one
//! where it was launched (DESIGN §13, since 0.4.0).
//!
//! The driver keeps, for each firing it hands to a worker, the *way* down
//! to the net that launched it: one [`Level`] per net on the way, each the
//! [`PathStep`] a failure's path names it by and the net's own id, which
//! tells apart two nets of one name in one token. A completion lands by its
//! way, and what each net on the way logged is lifted into the net around
//! it on the way back up, as a deep step lifts it.
//!
//! The nets on such a way are reached here directly, as a deep step reaches
//! them, and not through [`Net::nested_scope`] or [`Net::subnet_scope`]:
//! their firings are their own moves, which the rules on closed nets do not
//! stand in the way of (DESIGN §6.9).
//!
//! A token whose nets have a firing of the driver in flight, at any depth,
//! is *busy* ([`Busy`]) until every one of them has landed: a take or a
//! mutating edge passes it over, and a take-all or a drain, which takes
//! every token of its place, waits for it, so that the way stays good and
//! the nets in flight are never handed to a body. A take-all or a drain
//! that waits so holds its place ([`Holds`]): the nets there launch nothing
//! until it has had its turn.

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

use crate::edge::EdgeKind;
use crate::error::PathStep;
use crate::event::Event;
use crate::ids::{NetId, PlaceId, TokenId};
use crate::nested::{At, Member, Stepping};
use crate::net::{Net, slot_of};
use crate::place::PlaceSlot;

/// One level of the way down to a net inside another: the step a path
/// names it by, and the net's own id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Level {
    /// The place, and for a token's net the token and the net's name.
    pub(crate) step: PathStep,
    /// The id of the net this step leads to.
    pub(crate) id: NetId,
}

/// The way down from the net a driver runs to one of the nets inside it,
/// outer to inner; empty for the net itself.
pub(crate) type Way = Vec<Level>;

/// A way taken apart into the two lists a failure carries:
/// [`FiringFailed::path`](crate::FiringFailed::path) and
/// [`FiringFailed::nested_ids`](crate::FiringFailed::nested_ids).
pub(crate) fn split(way: &[Level]) -> (Vec<PathStep>, Vec<NetId>) {
    way.iter()
        .map(|level| (level.step.clone(), level.id))
        .unzip()
}

/// The nets inside which the concurrent driver has firings in flight, by
/// id, each with how many, at its depth or below: what makes a token that
/// carries one of them *busy*.
///
/// An edge that takes one token or lends one to change passes a busy token
/// over, as if it were not in its place, and a transition that could only
/// use busy tokens is not fireable for now. A take-all or a drain
/// ([`EdgeKind::takes_every`]) takes every token of its place, so its
/// transition is not fireable while any of them is busy
/// ([`any_in`](Busy::any_in)), and holds the place still meanwhile
/// ([`Holds`]). A read edge and a guard see a busy token as
/// it stands, the firing in flight and all. Only the concurrent driver
/// chooses with one; every other choice is made with none, and so as it
/// always was.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Busy<'a> {
    inside: &'a HashMap<NetId, usize>,
}

impl<'a> Busy<'a> {
    /// The tokens whose nets `inside` counts are busy.
    pub(crate) fn new(inside: &'a HashMap<NetId, usize>) -> Busy<'a> {
        Busy { inside }
    }

    /// `busy`, for an edge of `kind`: `None`, so that every token may be
    /// chosen, when there is none, when nothing is in flight inside, or
    /// when the edge only reads.
    pub(crate) fn for_edge(busy: Option<Busy<'a>>, kind: EdgeKind) -> Option<Busy<'a>> {
        busy.filter(|busy| !busy.inside.is_empty() && kind.takes_or_changes())
    }

    /// Whether the token at `at` of `slot` is busy: one of its nets has a
    /// firing of the driver in flight, at any depth.
    pub(crate) fn holds(self, slot: &PlaceSlot, at: usize) -> bool {
        let (Some(nested), Some((_, value))) = (slot.meta.nested, slot.tokens.get(at)) else {
            return false;
        };
        let value: &dyn Any = &**value;
        (nested.nets)(value)
            .iter()
            .any(|(_, net)| self.inside.contains_key(&net.id))
    }

    /// Whether `busy`, already given for its edge, holds any token of
    /// `slot`: what holds a take-all or a drain back.
    pub(crate) fn any_in(busy: Option<Busy<'_>>, slot: &PlaceSlot) -> bool {
        busy.is_some_and(|busy| (0..slot.tokens.len()).any(|at| busy.holds(slot, at)))
    }

    /// The first position in `slot` that `busy`, already given for its
    /// edge, lets the edge choose: the front, or the oldest token that is
    /// not busy. `None` when there is none.
    pub(crate) fn first_free(busy: Option<Busy<'_>>, slot: &PlaceSlot) -> Option<usize> {
        match busy {
            None => (!slot.tokens.is_empty()).then_some(0),
            Some(busy) => (0..slot.tokens.len()).find(|&at| !busy.holds(slot, at)),
        }
    }
}

/// A place whose nets the concurrent driver holds still: the id of the net
/// whose round steps the nets there, and the place's index in that net.
pub(crate) type Held = (NetId, usize);

/// The places the concurrent driver holds still for a take-all or a drain
/// that waits for their busy tokens (DESIGN §13, since 0.4.0), each with the
/// id of the net whose transition waits.
///
/// A transition that would be fireable but for the busy tokens of a place
/// it takes every token of ([`Net::waiting_with`]) holds every such place
/// of its own: the driver launches nothing new inside the nets there, at
/// any depth, while what is in flight lands. The net sets what it holds at the
/// end of each of its own sweeps, from what waits then, so a hold lasts
/// until the transition has had its turn, offered to the net's choice once
/// every token there is at rest, or has stopped being fireable otherwise.
/// Lasting so long, it outlives the landings that leave its place at rest,
/// so that the nets there cannot launch again before the transition's
/// turn.
#[derive(Debug, Default)]
pub(crate) struct Holds {
    /// The net whose transitions wait, and one place they hold.
    held: Vec<(NetId, Held)>,
    /// How many entries there may be before the next
    /// [`prune`](Holds::prune) drops those of nets no longer in the tree.
    prune_at: usize,
}

impl Holds {
    /// Fewer entries than this are never pruned.
    const PRUNE_FROM: usize = 32;

    /// What the transitions of the net `net` hold now, in place of what
    /// they held.
    pub(crate) fn set(&mut self, net: NetId, places: Vec<Held>) {
        if self.held.is_empty() && places.is_empty() {
            return;
        }
        self.held.retain(|&(by, _)| by != net);
        self.held
            .extend(places.into_iter().map(|place| (net, place)));
    }

    /// Whether the nets of the place at `place` of the net `net` are held.
    pub(crate) fn holds(&self, net: NetId, place: usize) -> bool {
        self.held.iter().any(|&(_, held)| held == (net, place))
    }

    /// Drops what the nets no longer in the tree of `top` held, once there
    /// are more entries than twice those the last prune kept, and more than
    /// [`PRUNE_FROM`](Holds::PRUNE_FROM): a net that left the tree before
    /// its next sweep, its token taken or moved away, sets nothing again.
    pub(crate) fn prune(&mut self, top: &Net) {
        if self.held.len() <= self.prune_at.max(Holds::PRUNE_FROM) {
            return;
        }
        let mut present = HashSet::from([top.id]);
        top.net_ids(&mut present);
        self.held.retain(|(by, _)| present.contains(by));
        self.prune_at = 2 * self.held.len();
    }
}

/// One net's round, as a pass of the concurrent driver takes it: the
/// members of its inner-first places, then its ring, from where the cursor
/// says, the net's own turn among them. The outer-first members are asked
/// for afresh once the ring has gone ([`Net::outer_first`]), since the net's
/// own turn may have moved the tokens they ride in.
pub(crate) struct Round {
    /// The nets in the inner-first places, in walk order.
    pub(crate) inner_first: Vec<Member>,
    /// The ring: the take-turns members after the cursor, the net's own
    /// turn, then the rest.
    pub(crate) ring: Vec<Turn>,
}

/// One turn of a [`Round`]'s ring.
pub(crate) enum Turn {
    /// The net's own: a launch sweep of its transitions.
    Own,
    /// A take-turns member's: a round of that net.
    Member(Member),
}

impl Net {
    /// This net's [`Round`] as it stands, the ring in the order a deep step
    /// takes it ([`Stepping`]).
    pub(crate) fn round(&self) -> Round {
        let members = self.members();
        let ring: Vec<Member> = members
            .iter()
            .copied()
            .filter(|member| member.stepping == Stepping::TakeTurns)
            .collect();
        let start = self.ring_start(&ring);
        let mut turns: Vec<Turn> = ring[start..].iter().copied().map(Turn::Member).collect();
        turns.push(Turn::Own);
        turns.extend(ring[..start].iter().copied().map(Turn::Member));
        Round {
            inner_first: members
                .into_iter()
                .filter(|member| member.stepping == Stepping::InnerFirst)
                .collect(),
            ring: turns,
        }
    }

    /// The members of this net's outer-first places, in walk order, as they
    /// stand.
    pub(crate) fn outer_first(&self) -> Vec<Member> {
        self.members()
            .into_iter()
            .filter(|member| member.stepping == Stepping::OuterFirst)
            .collect()
    }

    /// Whether a transition of this net is in flight, whoever launched it.
    pub(crate) fn has_in_flight(&self) -> bool {
        self.transitions.iter().any(|slot| slot.body.is_none())
    }

    /// The places of nets this net's transitions hold still now ([`Holds`]):
    /// every place of nets that a transition waiting for busy tokens
    /// ([`waiting_with`](Net::waiting_with)) takes every token of, named by
    /// the net whose round steps the nets there, this one or, for a window,
    /// its subnet, with the port.
    pub(crate) fn holding(&self, busy: Busy<'_>) -> Vec<Held> {
        let mut held = Vec::new();
        for transition in self.waiting_with(busy) {
            for &(place, kind) in &self.transitions[transition.0 as usize].inputs {
                let of_nets =
                    slot_of(&self.places, place).is_some_and(|slot| slot.meta.nested.is_some());
                if !kind.takes_every() || !of_nets {
                    continue;
                }
                let at = match self.places[place.0 as usize].meta.window {
                    None => (self.id, place.0 as usize),
                    Some(window) => match self.places[window.subnet.0 as usize].subnet.as_deref() {
                        Some(subnet) => (subnet.net.id, window.port.0 as usize),
                        None => continue,
                    },
                };
                if !held.contains(&at) {
                    held.push(at);
                }
            }
        }
        held
    }

    /// Adds to `into` the id of every net inside this one, at any depth,
    /// those in passive places included.
    pub(crate) fn net_ids(&self, into: &mut HashSet<NetId>) {
        for slot in &self.places {
            // A window's tokens are its subnet's, and found there.
            if slot.meta.window.is_some() {
                continue;
            }
            if let Some(subnet) = &slot.subnet {
                into.insert(subnet.net.id);
                subnet.net.net_ids(into);
                continue;
            }
            let Some(nested) = slot.meta.nested else {
                continue;
            };
            for (_, value) in &slot.tokens {
                let value: &dyn Any = &**value;
                for (_, inner) in (nested.nets)(value) {
                    into.insert(inner.id);
                    inner.net_ids(into);
                }
            }
        }
    }

    /// Calls `f` with the net a member names, mutably, and the level that
    /// leads to it; then lifts what that net logged into this net's log, as
    /// a deep step lifts a member's firing (DESIGN §6.6, rule 1), and, if it
    /// logged anything, indexes its token again. On a panic of `f` too, and
    /// then the panic goes on.
    ///
    /// The member is found by its token's id and its own id, wherever the
    /// token stands now: `None`, with `f` not called, when the token or the
    /// net is no longer there.
    pub(crate) fn within_member<R>(
        &mut self,
        member: &Member,
        f: impl FnOnce(&mut Net, Level) -> R,
    ) -> Option<R> {
        let place = PlaceId(member.place as u32);
        let At::Token { token, .. } = member.at else {
            let subnet = &mut self.places[member.place].subnet.as_deref_mut()?.net;
            if subnet.id != member.id {
                return None;
            }
            let level = Level {
                step: PathStep::Subnet { place },
                id: member.id,
            };
            let result = catch_unwind(AssertUnwindSafe(|| f(&mut *subnet, level)));
            let events: Vec<Event> = subnet.take_events().collect();
            self.lift_subnet(place, member.id, events);
            return Some(result.unwrap_or_else(|panic| resume_unwind(panic)));
        };
        let slot = &mut self.places[member.place];
        let nested = slot.meta.nested?;
        let at = slot.position(token)?;
        let (name, result, events) = {
            let value = &mut slot.tokens[at].1;
            let (name, inner) = (nested.nets_mut)(&mut **value)
                .into_iter()
                .find(|(_, inner)| inner.id == member.id)?;
            let name = name.to_owned();
            let level = Level {
                step: PathStep::Token {
                    place,
                    token,
                    net: name.clone(),
                },
                id: member.id,
            };
            let result = catch_unwind(AssertUnwindSafe(|| f(&mut *inner, level)));
            let events: Vec<Event> = inner.take_events().collect();
            (name, result, events)
        };
        self.lift_token(place, token, at, &name, member.id, events);
        Some(result.unwrap_or_else(|panic| resume_unwind(panic)))
    }

    /// Calls `f` with the net at the end of `way`, mutably, and `way`
    /// itself; then, level by level on the way back up, lifts what each net
    /// logged into the net around it and indexes the token it rides in
    /// again, as [`within_member`](Net::within_member) does for one level.
    /// On a panic of `f` too, and then the panic goes on.
    ///
    /// Each level is found by its place, its token's id and its net's id.
    /// `Err(f)`, with `f` not called and nothing lifted, when the way no
    /// longer leads to that net.
    pub(crate) fn within_way<R, F>(&mut self, way: &[Level], f: F) -> Result<R, F>
    where
        F: FnOnce(&mut Net, &[Level]) -> R,
    {
        self.down(way, way, f)
    }

    /// [`within_way`](Net::within_way), with `rest` the part of `way` still
    /// to go from this net.
    fn down<R, F>(&mut self, way: &[Level], rest: &[Level], f: F) -> Result<R, F>
    where
        F: FnOnce(&mut Net, &[Level]) -> R,
    {
        let Some((level, rest)) = rest.split_first() else {
            return Ok(f(self, way));
        };
        match level.step {
            PathStep::Subnet { place } => {
                let Some(subnet) = self
                    .places
                    .get_mut(place.0 as usize)
                    .and_then(|slot| slot.subnet.as_deref_mut())
                    .map(|subnet| &mut subnet.net)
                    .filter(|subnet| subnet.id == level.id)
                else {
                    return Err(f);
                };
                let result = catch_unwind(AssertUnwindSafe(|| subnet.down(way, rest, f)));
                if matches!(result, Ok(Err(_))) {
                    return result.unwrap_or_else(|panic| resume_unwind(panic));
                }
                let events: Vec<Event> = subnet.take_events().collect();
                self.lift_subnet(place, level.id, events);
                result.unwrap_or_else(|panic| resume_unwind(panic))
            }
            PathStep::Token { place, token, .. } => {
                let Some(slot) = self.places.get_mut(place.0 as usize) else {
                    return Err(f);
                };
                let (Some(nested), Some(at)) = (slot.meta.nested, slot.position(token)) else {
                    return Err(f);
                };
                let (name, result, events) = {
                    let value = &mut slot.tokens[at].1;
                    let Some((name, inner)) = (nested.nets_mut)(&mut **value)
                        .into_iter()
                        .find(|(_, inner)| inner.id == level.id)
                    else {
                        return Err(f);
                    };
                    let name = name.to_owned();
                    let result = catch_unwind(AssertUnwindSafe(|| inner.down(way, rest, f)));
                    if matches!(result, Ok(Err(_))) {
                        return result.unwrap_or_else(|panic| resume_unwind(panic));
                    }
                    let events: Vec<Event> = inner.take_events().collect();
                    (name, result, events)
                };
                self.lift_token(place, token, at, &name, level.id, events);
                result.unwrap_or_else(|panic| resume_unwind(panic))
            }
        }
    }

    /// The way down to the net whose id is `id`, at any depth, passive
    /// places included: where a completion lands whose way no longer leads
    /// to its net, because the program moved the token it rides in between
    /// two ticks. `None` when the net is in no place of the tree any more.
    pub(crate) fn find_way(&self, id: NetId) -> Option<Way> {
        let mut way = Vec::new();
        self.find_into(id, &mut way).then_some(way)
    }

    /// [`find_way`](Net::find_way), from this net, `way` being the way to
    /// it: true, with `way` leading to the net, when it is found.
    fn find_into(&self, id: NetId, way: &mut Way) -> bool {
        for (index, slot) in self.places.iter().enumerate() {
            // A window's tokens are its subnet's, and found there.
            if slot.meta.window.is_some() {
                continue;
            }
            let place = PlaceId(index as u32);
            if let Some(subnet) = &slot.subnet {
                way.push(Level {
                    step: PathStep::Subnet { place },
                    id: subnet.net.id,
                });
                if subnet.net.id == id || subnet.net.find_into(id, way) {
                    return true;
                }
                way.pop();
                continue;
            }
            let Some(nested) = slot.meta.nested else {
                continue;
            };
            for (token, value) in &slot.tokens {
                let value: &dyn Any = &**value;
                for (name, inner) in (nested.nets)(value) {
                    way.push(Level {
                        step: PathStep::Token {
                            place,
                            token: *token,
                            net: name.to_owned(),
                        },
                        id: inner.id,
                    });
                    if inner.id == id || inner.find_into(id, way) {
                        return true;
                    }
                    way.pop();
                }
            }
        }
        false
    }

    /// What a subnet logged, lifted into this net's log as
    /// [`Event::Subnet`].
    fn lift_subnet(&mut self, place: PlaceId, nested_id: NetId, events: Vec<Event>) {
        for event in events {
            self.emit(Event::Subnet {
                place,
                nested_id,
                event: Box::new(event),
            });
        }
    }

    /// What one of a token's nets logged, lifted into this net's log as
    /// [`Event::Nested`] under the place and the id the token has, and then,
    /// if there was anything, the token indexed again: a key may read the
    /// nets it carries (DESIGN §8.3). The events go first, so that a key
    /// extractor that panics loses none of them.
    fn lift_token(
        &mut self,
        place: PlaceId,
        token: TokenId,
        at: usize,
        name: &str,
        nested_id: NetId,
        events: Vec<Event>,
    ) {
        if events.is_empty() {
            return;
        }
        for event in events {
            self.emit(Event::Nested {
                place,
                token,
                net: name.to_owned(),
                nested_id,
                event: Box::new(event),
            });
        }
        let slot = &self.places[place.0 as usize];
        if slot.meta.keys.is_some() {
            self.index
                .forget(&slot.meta, place, token, &*slot.tokens[at].1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NetBuilder;

    #[test]
    fn a_prune_drops_only_what_nets_gone_from_the_tree_held() {
        let mut b = NetBuilder::new();
        b.place::<u32>("Lap");
        let inner = b.build().expect("an inner net");
        let inner_id = inner.id;
        let mut b = NetBuilder::new();
        let yard = b.place_nested::<Net>("Yard");
        b.seed(yard, [inner]);
        let net = b.build().expect("a yard");

        let mut holds = Holds::default();
        holds.set(net.id, vec![(net.id, 0)]);
        holds.set(inner_id, vec![(inner_id, 0)]);
        holds.prune(&net);
        assert_eq!(holds.held.len(), 2, "too few to prune");
        for _ in 0..Holds::PRUNE_FROM {
            holds.set(NetId::mint(), vec![(net.id, 1)]);
        }
        assert!(holds.holds(net.id, 1));
        holds.prune(&net);
        assert!(holds.holds(net.id, 0) && holds.holds(inner_id, 0));
        assert!(!holds.holds(net.id, 1), "held by nets gone from the tree");

        holds.set(net.id, Vec::new());
        assert!(!holds.holds(net.id, 0), "a net that holds nothing now");
        assert!(holds.holds(inner_id, 0));
    }
}
