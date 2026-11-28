//! Stage one of a firing, [`launch`](Net::launch): the body taken out of
//! its slot and the inputs out of their places, all of them or none.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::Arc;

use crate::closed::{Hold, LAUNCH, Mark, Marked, mark, unmark};
use crate::deep::Busy;
use crate::edge::EdgeKind;
use crate::erased::{ErasedBody, ErasedInput};
use crate::error::NetError;
use crate::event::Event;
use crate::ids::{PlaceId, TokenId, TransitionId};
use crate::index::{KeyIndex, first_free_match};
use crate::keys::KeyValue;
use crate::nested::lift_from;
use crate::net::{Net, slot_of};
use crate::pairing::pair;
use crate::param::{ParamSlot, param_slot};
use crate::place::{ErasedToken, PlaceMeta, PlaceSlot, Window};
use crate::sharing::share;
use crate::transition::{ArgSource, TransitionSlot};

use super::{Firing, Report, Stage, call_body};

/// Why an input place is never empty once stage one starts, for an edge
/// that [requires presence](EdgeKind::requires_presence): being fireable is
/// what was checked. A drain or a survey edge never asks.
const NON_EMPTY: &str = "a fireable transition's input places all hold a token";

/// Why a keyed edge finds its token once stage one starts, and a bound edge
/// its pair: fireability asked the same question of the same marking, and
/// the other edges of the transition read other places.
const MATCHING: &str = "a fireable transition's keyed edges each have a token to choose";

/// Why stage one finds the body at home when it packs it: it is taken out
/// of the slot once, and packed once, as the last thing stage one does.
const PACKED_ONCE: &str = "stage one packs the body once, as the last thing it does";

/// What stage one hands [`Net::launch_now`]: how far the firing is, what it
/// consumed and lent, the clones it took to restore, and its hold.
type StageOne = (
    Stage,
    Vec<(PlaceId, TokenId)>,
    Vec<(PlaceId, TokenId)>,
    Option<Vec<(PlaceId, TokenId, ErasedToken)>>,
    Option<Arc<Hold>>,
);

impl Net {
    // ----- the three stages -----------------------------------------------

    /// Stage one: consumes this transition's inputs and takes its body out of
    /// the net.
    ///
    /// The tokens leave their places *now*, which is the whole point: a peer
    /// transition that wanted one of them is no longer
    /// [fireable](Net::fireable), so consuming in the launch sweep is what
    /// resolves conflicts, with no locks and no in-flight registry. The
    /// transition is [in flight](Net::is_in_flight) until the
    /// [`Completed`](crate::Completed) comes back to [`distribute`](Net::distribute), and
    /// cannot be launched again meanwhile.
    ///
    /// Emits [`Event::Launched`] naming the tokens that were taken.
    ///
    /// A transition that is not *detachable* (one with a mutating or a read
    /// edge, such as a [`MutateEdge`](crate::MutateEdge) or a
    /// [`ReadEdge`](crate::ReadEdge), whose body borrows a token where it
    /// lies; [`TransitionInfo::detachable`](crate::TransitionInfo::detachable))
    /// has its body run inside this call, and the `Firing` comes back with
    /// [`is_pending`](Firing::is_pending) false.
    ///
    /// ```
    /// # use petritype::{NetBuilder, TakeEdge};
    /// # let mut b = NetBuilder::new();
    /// # let inbox = b.place::<u32>("Inbox");
    /// # b.port(inbox);
    /// # let out = b.place::<u32>("Out");
    /// let double = b
    ///     .transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,))
    ///     .id();
    /// let mut net = b.build()?;
    /// net.put(inbox, 21);
    ///
    /// let firing = net.launch(double)?;
    /// assert_eq!(net.count(inbox.id()), 0, "consumed at launch");
    /// assert_eq!(net.in_flight(), [double]);
    /// assert!(!net.is_fireable(double), "in flight, so not fireable");
    ///
    /// net.distribute(firing.run())?;
    /// assert!(net.in_flight().is_empty());
    /// assert_eq!(net.fired_count(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// [`NetError::NoSuchTransition`] for an unknown id, and
    /// [`NetError::NotFireable`] for a transition that cannot fire right now,
    /// including one that is already in flight. [`NetError::HeldByBody`]
    /// when a body holds this net, as for [`fire`](Net::fire). The net is untouched in each case.
    ///
    /// # Panics
    ///
    /// Not for a panicking body, inline firing included: the panic is caught
    /// here, and the `Firing` comes back carrying the failure exactly as it
    /// would carry one a body returned. [`distribute`](Net::distribute) is
    /// where every failure is reported, whichever stage produced it.
    ///
    /// A panic of the net's own code that stage one runs, a key extractor,
    /// a token's or a parameter's `Clone` or
    /// [`Nested::nets_mut`](crate::Nested::nets_mut), goes on to the
    /// caller. It takes none of the firing's tokens (since 0.4.0): every
    /// one of those calls is made before the first token moves, so each is
    /// where it was, with its id, and the body is back in its slot. The one
    /// exception is a body that a mutating edge lends a token, which runs
    /// inside `launch`: should the extractor that reindexes that token, or
    /// the `nets_mut` that unmarks its nets, panic once the body has run,
    /// what the body was handed is its own, and is lost.
    #[must_use = "a Firing that is dropped instead of distributed strands its transition in \
                  flight for good: the body never comes home"]
    pub fn launch(&mut self, transition: TransitionId) -> Result<Firing, NetError> {
        self.admit_knob(LAUNCH)?;
        self.launch_for_driver(transition, None)
    }

    /// [`launch`](Net::launch) without the check: the concurrent driver's,
    /// which a body may run on a net it holds, as it may step one. The
    /// driver launches what [`fireable_with`](Net::fireable_with) names and
    /// makes no choice by name, and its edges that take or change a token
    /// mind the tokens `busy` holds as fireability does.
    pub(crate) fn launch_for_driver(
        &mut self,
        transition: TransitionId,
        busy: Option<Busy<'_>>,
    ) -> Result<Firing, NetError> {
        let slot = self
            .transitions
            .get(transition.0 as usize)
            .ok_or(NetError::NoSuchTransition { transition })?;
        if !self.slot_is_fireable_with(slot, busy) {
            return Err(NetError::NotFireable {
                transition,
                name: slot.name.clone(),
            });
        }
        Ok(self.launch_now(transition, Report::Staged, busy))
    }

    // ----- the stages, written once ---------------------------------------

    /// Stage one, with fireability already established.
    ///
    /// `report` is how this firing will describe itself: one
    /// [`Fired`](Event::Fired) at the end for the sequential path, or
    /// [`Launched`](Event::Launched) here and
    /// [`Distributed`](Event::Distributed) at stage three for the staged one.
    /// It travels with the firing, so the two paths differ in nothing but
    /// what they log.
    ///
    /// Stage one runs code of the net's own: a key extractor, a `Clone` of
    /// a token or a parameter, [`Nested::nets_mut`](crate::Nested::nets_mut).
    /// It takes all its tokens or none: every one of those calls is made
    /// before the first token moves (since 0.4.0; until then a token taken
    /// before the panic went with the unwind), so should one panic, every
    /// token is where it was, with its id, and the body goes back to its
    /// slot (since 2026-11-28; until then the transition was left in
    /// flight for good) before the panic goes on. The transition is
    /// fireable again when its places say so. What the inner nets of the
    /// tokens it was to take had logged is read out of them by then, and
    /// told here, under the place and the id each still has, as
    /// [`Net::lift`] tells it of a token at rest.
    ///
    /// An inline body's own work is past that point: once a body that a
    /// mutating edge lends a token has run, the key extractor that reindexes
    /// the token and the `nets_mut` that unmarks its nets run after the
    /// inputs were handed over, and a panic of either still loses what the
    /// firing took, which the body owns by then.
    ///
    /// `busy` is the concurrent driver's: a take or a mutating edge passes
    /// over the tokens it holds, as fireability did, which let a take-all or
    /// a drain go only with none of its place's tokens busy.
    pub(super) fn launch_now(
        &mut self,
        transition: TransitionId,
        report: Report,
        busy: Option<Busy<'_>>,
    ) -> Firing {
        let index = transition.0 as usize;
        // Out of its slot until stage three: the body may not borrow the
        // transition list while it runs, and an empty slot is what "in
        // flight" means. Held here until the firing packs it, so that a
        // panic before then can put it back.
        let mut home = Some(
            self.transitions[index]
                .body
                .take()
                .expect("a fireable transition has its body in its slot"),
        );
        // What the inner nets of the tokens it takes had logged, lifted
        // under the place and id each token had (DESIGN §6.6, rule 3). Kept
        // out here, so that a panic after the lifting still tells it.
        let mut lifted = Vec::new();
        let staged = catch_unwind(AssertUnwindSafe(|| {
            self.stage_one(index, &mut home, &mut lifted, busy)
        }));
        let (state, consumed, mutated, restore, hold) = match staged {
            Ok(staged) => staged,
            Err(payload) => {
                self.transitions[index].body = home.take();
                self.emit_all(lifted);
                resume_unwind(payload)
            }
        };

        // Before the firing says anything of its own: these are the last
        // words of tokens that may never be seen again.
        self.emit_all(lifted);
        if report == Report::Staged {
            self.emit(Event::Launched {
                transition,
                consumed: consumed.clone(),
            });
        }
        Firing {
            net: self.id,
            transition,
            consumed,
            mutated,
            restore,
            report,
            state,
            hold,
        }
    }

    /// Stage one's work for [`launch_now`](Net::launch_now): the inputs
    /// taken, and the body, taken out of `home` as the firing packs it,
    /// with the arguments or, for an inline firing, the outcome it has
    /// already produced. Nothing of the net's own code runs after the body
    /// is packed.
    ///
    /// In three passes, so that it takes all its tokens or none: [`plan`]
    /// reads the net, running every key extractor and `Clone` stage one
    /// needs, and changes nothing; [`lift_and_mark`] and [`mark_lent`] run
    /// `nets_mut` on the tokens where they lie, which moves none of them;
    /// and [`take`] moves the tokens, running none of the net's own code.
    fn stage_one(
        &mut self,
        index: usize,
        home: &mut Option<ErasedBody>,
        lifted: &mut Vec<Event>,
        busy: Option<Busy<'_>>,
    ) -> StageOne {
        let mut consumed = Vec::new();
        let mut mutated = Vec::new();
        // Beside `mutated`: for each token a mutating edge will lend the
        // body, the argument it fills and where it stands in its place.
        let mut lent = Vec::new();
        // The same for each token a read edge lends the body to look at,
        // which is told of in no event (DESIGN §8.3, read edges).
        let mut looked = Vec::new();
        // `Some`, empty, for a transition that restores its inputs: the
        // clones are taken by `plan`, below, before the body sees a thing.
        // `None` for every other transition, which is also what says that
        // there is nothing to put back.
        let mut restore = self.transitions[index]
            .restore_tokens_on_failure
            .then(Vec::new);
        // The mark of this firing, made when it first lends or hands its
        // body a token's net, and held until the firing ends (DESIGN §6.9).
        // A panic before the firing is packed drops it, and every net it
        // marked is held by nothing again.
        let mut hold = None;
        let state = {
            // Field by field, so that the input places can be emptied while
            // the edge list and the parameter values are read out of the
            // net beside them.
            let Net {
                places,
                transitions,
                params,
                index: key_index,
                ..
            } = &mut *self;
            let transition = &transitions[index];
            let planned = plan(
                places,
                params,
                key_index,
                transition,
                &mut mutated,
                &mut lent,
                &mut looked,
                restore.as_mut(),
                busy,
            );
            // The keys of the tokens the body is about to change, while
            // they are still the old ones: the index finds a token's
            // entries by them (DESIGN §8.3). None for a detachable
            // transition, which lends nothing.
            let before = keys_before(places, &mutated, &lent);
            lift_and_mark(places, transition, &planned, lifted, &mut hold);
            // The nets of the tokens the body is lent are held by it for as
            // long as it runs, and no longer (DESIGN §6.9).
            let marks = mark_lent(places, &lent, &mut hold, &transition.reaches);
            // Every piece of the net's own code has run: from here on,
            // nothing but the body can panic before the firing is packed.
            let args = take(places, key_index, planned, &mut consumed);
            if transition.detachable {
                Stage::Airborne {
                    body: home.take().expect(PACKED_ONCE),
                    args,
                }
            } else {
                // Stage two, here and now: a mutating or a read edge hands the
                // body a borrow into a place, which may neither travel with the
                // `Firing` nor still be alive when stage three writes.
                let args = borrow_mutated(places, &lent, &looked, args);
                let outcome = call_body(home.as_mut().expect(PACKED_ONCE), args, hold.as_deref());
                unmark_lent(places, &lent, marks);
                // Whatever the body did, done or half done before a panic,
                // is in the place now, and the index follows it there. The
                // body could change the token, never the place around it,
                // so each token is still where it was lent.
                for (((place, token), lent), before) in mutated.iter().zip(&lent).zip(before) {
                    let (slot, key_index, at) = resolve_mut(places, key_index, *place);
                    let (_, value) = &slot.tokens[lent.at];
                    key_index.changed(&slot.meta, at, *token, before, &**value);
                }
                Stage::Ran {
                    body: home.take().expect(PACKED_ONCE),
                    outcome,
                }
            }
        };
        (state, consumed, mutated, restore, hold)
    }
}

/// A token a mutating edge lends the body, beside its entry in `mutated`, or
/// that a read edge lends it to look at: which argument it fills and where
/// it stands in its place.
///
/// The position is taken when the edge chooses its token (the front for a
/// [`MutateEdge`](crate::MutateEdge), the first its filter accepts for a
/// [`MutateWhereEdge`](crate::MutateWhereEdge)) and stays good until stage
/// one is over: the other edges of the transition read other places, and the
/// body can change the token but never the queue around it.
#[derive(Debug, Clone, Copy)]
struct Lent {
    /// The argument's position in the body's parameter list. Not the
    /// edge's: a parameter may stand between two edges.
    arg: usize,
    /// The edge's position in the transition's input edges, whose
    /// `reaches` a mark of the token's nets reads.
    edge: usize,
    /// The token's position in its place's queue.
    at: usize,
    /// The index of this net's slot that holds the token: its place's own,
    /// or, for a window, its subnet place's.
    holder: usize,
    /// For a window, the port's index in that slot's subnet.
    port: Option<usize>,
    /// For a [`ReadAllEdge`](crate::ReadAllEdge), every token of the place
    /// rather than the one at `at`.
    all: bool,
}

/// One argument of a firing, as [`plan`] decided it before anything moved:
/// the token each consuming edge takes, where it stands, and its keys as
/// they are, or an argument that is ready already.
enum Planned {
    /// An argument in hand: a parameter's clone, a snapshot's, a survey's.
    Ready(ErasedInput<'static>),
    /// A take: the token `token`, at `at` in `place`, with its keys.
    Take {
        edge: usize,
        place: PlaceId,
        at: usize,
        token: TokenId,
        keys: Option<Vec<KeyValue>>,
    },
    /// A take-all or a drain: every token of `place`, at `at`, each with
    /// its keys, in queue order.
    TakeAll {
        edge: usize,
        place: PlaceId,
        at: Vec<usize>,
        keys: Vec<Option<Vec<KeyValue>>>,
    },
    /// A token a mutating or a read edge lends where it lies, filled in by
    /// [`borrow_mutated`].
    Lent,
}

/// Stage one's first pass: one argument per input, in declaration order,
/// decided while nothing moves. It reads the net and changes nothing, and
/// runs every key extractor and `Clone` stage one needs (the keys of every
/// token a consuming edge will take, the clones a restoring transition
/// keeps, a snapshot's, a survey's, a parameter's), so that a panic of any
/// of them leaves every token where it was. [`take`] then moves them.
///
/// Every argument it produces is **owned** (`ErasedInput<'static>`), which
/// is what lets a [`Firing`] be `Send` and outlive the call that made it. A
/// [`MutateEdge`](crate::MutateEdge) or
/// [`MutateWhereEdge`](crate::MutateWhereEdge) has no owned argument to give,
/// since its token stays in its place, so it is [`Planned::Lent`] and is
/// recorded in `mutated`, with where the token stands in `lent`;
/// [`borrow_mutated`] fills those positions in, for the inline path that is
/// the only one allowed to hold such a borrow.
///
/// A keyed edge chooses its token with the key index ([`first_match`]): the
/// first in queue order its filter accepts, which is wherever it is. A
/// [`TakeWhereEdge`](crate::TakeWhereEdge) takes it out of the middle of the
/// place, as a [`TakeEdge`](crate::TakeEdge) takes the front, and every
/// other token stays where it is. A bound edge and its source
/// ([`Clause::IsInput`](crate::Clause::IsInput)) take the pair
/// [`pair`] finds, and the takes that share a place the distinct tokens
/// [`share`] finds. Every edge chooses here, before any edge has moved a
/// thing, as fireability chose, and every position is the token's in the
/// marking as it stands: [`take`] counts past the tokens taken before it
/// from the same place (DESIGN §8.3).
///
/// A [`DrainEdge`](crate::DrainEdge) takes every token as a
/// [`TakeAllEdge`](crate::TakeAllEdge) does, which for an empty place is
/// nothing: an empty argument, nothing in `consumed` or `restore`. A
/// [`SurveyEdge`](crate::SurveyEdge) clones every token as a
/// [`SnapshotEdge`](crate::SnapshotEdge) clones the front one, and moves
/// nothing.
///
/// `busy` is the concurrent driver's (DESIGN §13): a take or a mutating
/// edge passes over the tokens it holds, choosing the first that is not
/// busy, as fireability chose. A take-all or a drain takes every token of
/// its place, since fireability let its transition go only with none of
/// them busy.
///
/// `restore` is `Some` for a transition declared with
/// [`restore_tokens_on_failure`](crate::TransitionBuilder::restore_tokens_on_failure),
/// and every token a consuming edge takes is cloned into it, with its id, in
/// the order it will be taken. The clone is taken *here*, before the body is
/// given anything, because that is the last moment the token is certainly
/// intact: from the moment it is taken the body owns it.
///
/// A [`Param<T>`](crate::Param) argument is a clone of the net's current
/// value, taken here too and for the same reason (DESIGN §10): the body may
/// run for a long time somewhere else, and a
/// [`set_param`](Net::set_param) meanwhile must not change what it is working
/// with. A parameter consumes nothing, so it is in neither `consumed` nor
/// `restore` and puts nothing in an event.
///
/// The transition's `args` are the tuple order (one entry per argument,
/// saying which input edge or which parameter fills it), because its
/// `inputs` are the edges alone.
#[allow(clippy::too_many_arguments)]
fn plan(
    places: &[PlaceSlot],
    params: &[ParamSlot],
    index: &KeyIndex,
    transition: &TransitionSlot,
    mutated: &mut Vec<(PlaceId, TokenId)>,
    lent: &mut Vec<Lent>,
    looked: &mut Vec<Lent>,
    mut restore: Option<&mut Vec<(PlaceId, TokenId, ErasedToken)>>,
    busy: Option<Busy<'_>>,
) -> Vec<Planned> {
    let mut planned = Vec::with_capacity(transition.args.len());

    // The pairs of the bound edges and their sources, and the tokens of the
    // takes that share a place, each edge's position and token, chosen
    // before anything moves, as fireability chose them.
    let mut paired: Vec<(usize, usize, TokenId)> = transition
        .bindings
        .iter()
        .flat_map(|binding| pair(places, params, index, transition, binding, busy).expect(MATCHING))
        .collect();
    if !transition.shared.is_empty() {
        paired.extend(share(places, params, index, transition, busy).expect(MATCHING));
    }

    for source in &transition.args {
        let edge = match *source {
            ArgSource::Edge(edge) => edge,
            ArgSource::Param { type_id, .. } => {
                let slot = param_slot(params, type_id)
                    .expect("build refuses a net whose declared parameter has no value");
                planned.push(Planned::Ready(ErasedInput::Token(slot.clone_value())));
                continue;
            }
        };
        let (place, kind) = transition.inputs[edge];
        // The busy tokens this edge passes over, if it takes or changes one.
        let busy = Busy::for_edge(busy, kind);
        // The token a keyed edge, a bound edge, a source or a take on a
        // shared place chooses, with where it stands.
        let pairs = paired.iter().find(|(chosen, _, _)| *chosen == edge);
        let chooses = kind.is_keyed() || pairs.is_some();
        let chosen = || {
            if let Some(&(_, at, token)) = pairs {
                return (at, token);
            }
            let filter = transition.filters[edge]
                .as_ref()
                .expect("a keyed edge carries its filter");
            first_free_match(places, params, index, place, filter, busy).expect(MATCHING)
        };
        // The holder's slot: a window's is its port's.
        let slot = || slot_of(places, place).expect(NON_EMPTY);
        // The front token, or the first that is not busy.
        let front = || Busy::first_free(busy, slot()).expect(NON_EMPTY);
        match kind {
            EdgeKind::Take | EdgeKind::TakeWhere => {
                let slot = slot();
                let at = if chooses { chosen().0 } else { front() };
                let (token, value) = slot.tokens.get(at).expect(NON_EMPTY);
                debug_assert!(
                    pairs.is_none_or(|&(_, _, chosen)| chosen == *token),
                    "a paired token is where it was chosen"
                );
                let keys = KeyIndex::before(&slot.meta, &**value);
                if let Some(restore) = restore.as_deref_mut() {
                    restore.push((place, *token, restore_clone(&slot.meta, value)));
                }
                planned.push(Planned::Take {
                    edge,
                    place,
                    at,
                    token: *token,
                    keys,
                });
            }
            // A drain is a take-all that its place cannot hold back, so an
            // empty place gives it an empty batch through the same code.
            EdgeKind::TakeAll | EdgeKind::Drain => {
                let slot = slot();
                debug_assert!(
                    !Busy::any_in(busy, slot),
                    "a take-all or a drain waits for its place's busy tokens"
                );
                let at: Vec<usize> = (0..slot.tokens.len()).collect();
                let mut keys = Vec::with_capacity(at.len());
                for &at in &at {
                    let (token, value) = &slot.tokens[at];
                    keys.push(KeyIndex::before(&slot.meta, &**value));
                    if let Some(restore) = restore.as_deref_mut() {
                        restore.push((place, *token, restore_clone(&slot.meta, value)));
                    }
                }
                planned.push(Planned::TakeAll {
                    edge,
                    place,
                    at,
                    keys,
                });
            }
            EdgeKind::Snapshot => {
                let slot = slot();
                let clone = slot
                    .meta
                    .clone
                    .expect("build rejects a snapshot of a place that cannot clone its tokens");
                // The front token, or a source's, wherever it stands.
                let at = if chooses { chosen().0 } else { 0 };
                let (_, value) = slot.tokens.get(at).expect(NON_EMPTY);
                // The clone is taken now, so the body sees a value that
                // cannot change under it even once firings run elsewhere.
                planned.push(Planned::Ready(ErasedInput::Token(clone(&**value))));
            }
            EdgeKind::Survey => {
                let slot = slot();
                let clone = slot
                    .meta
                    .clone
                    .expect("build rejects a survey of a place that cannot clone its tokens");
                // Every token, oldest first, cloned now for the same reason
                // as a snapshot's; none at all from an empty place.
                let values = slot
                    .tokens
                    .iter()
                    .map(|(_, value)| clone(&**value))
                    .collect();
                planned.push(Planned::Ready(ErasedInput::Tokens(values)));
            }
            EdgeKind::Mutate | EdgeKind::MutateWhere => {
                let (at, token) = if chooses {
                    chosen()
                } else {
                    let at = front();
                    (at, slot().tokens[at].0)
                };
                mutated.push((place, token));
                let (holder, port) = holder_of(places, place);
                lent.push(Lent {
                    arg: planned.len(),
                    edge,
                    at,
                    holder,
                    port,
                    all: false,
                });
                planned.push(Planned::Lent);
            }
            // A read edge moves nothing and is told of in no event: it is
            // recorded in `looked`, and `borrow_mutated` lends the token, or
            // every token for a read-all, once the other places are done.
            EdgeKind::Read | EdgeKind::ReadWhere | EdgeKind::ReadAll => {
                let all = kind == EdgeKind::ReadAll;
                let at = if chooses && !all { chosen().0 } else { 0 };
                let (holder, port) = holder_of(places, place);
                looked.push(Lent {
                    arg: planned.len(),
                    edge,
                    at,
                    holder,
                    port,
                    all,
                });
                planned.push(Planned::Lent);
            }
        }
    }

    planned
}

/// The index of this net's slot that holds a place's tokens, and for a
/// window the port's index in that slot's subnet: where a [`Lent`] token
/// is found again.
fn holder_of(places: &[PlaceSlot], place: PlaceId) -> (usize, Option<usize>) {
    match places[place.0 as usize].meta.window {
        None => (place.0 as usize, None),
        Some(window) => (window.subnet.0 as usize, Some(window.port.0 as usize)),
    }
}

/// Stage one's second pass, for the tokens the consuming edges will take
/// from a [nested](crate::NetBuilder::place_nested) place: each one's inner
/// nets' logs emptied into `lifted`, under the place and the id it has
/// there, and its nets marked as held by this firing through its edge, all
/// while it lies in its place. [`Nested::nets_mut`](crate::Nested::nets_mut)
/// runs here, the last of the net's own code before [`take`] moves a token,
/// so a panic of it leaves every token where it was, and the events already
/// lifted are told all the same (DESIGN §6.6, rule 3).
fn lift_and_mark(
    places: &mut [PlaceSlot],
    transition: &TransitionSlot,
    planned: &[Planned],
    lifted: &mut Vec<Event>,
    hold: &mut Option<Arc<Hold>>,
) {
    for step in planned {
        let (edge, place, taken) = match step {
            Planned::Take {
                edge, place, at, ..
            } => (*edge, *place, std::slice::from_ref(at)),
            Planned::TakeAll {
                edge, place, at, ..
            } => (*edge, *place, &at[..]),
            Planned::Ready(_) | Planned::Lent => continue,
        };
        let slot = slot_mut(places, place);
        let Some(nested) = slot.meta.nested else {
            continue;
        };
        for &at in taken {
            let (token, value) = slot.tokens.get_mut(at).expect(NON_EMPTY);
            lifted.extend(lift_from(nested, value, place, *token));
            // The body owns the token's nets once it is taken, and holds
            // them by this edge until the firing ends.
            mark(
                nested,
                value,
                &Mark::by_edge(hold, &transition.reaches, edge),
            );
        }
    }
}

/// The slot that holds a place's tokens, mutably: its own, or a window's
/// port's in the subnet. The mutable [`slot_of`].
fn slot_mut(places: &mut [PlaceSlot], place: PlaceId) -> &mut PlaceSlot {
    let at = place.0 as usize;
    match places[at].meta.window {
        None => &mut places[at],
        Some(window) => resolve_port(places, window).0,
    }
}

/// Stage one's last pass: the tokens [`plan`] chose, taken out of their
/// places in edge order, each leaving the key index with the keys `plan`
/// read, so that none of the net's own code runs here and every token is
/// taken once the first is. The arguments come out in declaration order. A
/// position `plan` gave is the token's before anything moved, so a take
/// counts past the tokens taken ahead of it from the same place.
///
/// A token taken from a nested place had its inner nets' logs lifted by
/// [`lift_and_mark`]: the body owns it from here on, and may never put it
/// back (DESIGN §6.6, rule 3).
fn take(
    places: &mut [PlaceSlot],
    index: &mut KeyIndex,
    planned: Vec<Planned>,
    consumed: &mut Vec<(PlaceId, TokenId)>,
) -> Vec<ErasedInput<'static>> {
    // Where each take found its token, for the takes after it: a token
    // taken from in front of another, out of the same place, moves that
    // one forward by one. Only takes share a place, so a take-all never
    // meets a place a take has touched.
    let mut removed: Vec<(PlaceId, usize)> = Vec::new();
    planned
        .into_iter()
        .map(|step| match step {
            Planned::Ready(arg) => arg,
            Planned::Take {
                place,
                at,
                token,
                keys,
                ..
            } => {
                let ahead = removed
                    .iter()
                    .filter(|&&(from, there)| from == place && there < at)
                    .count();
                removed.push((place, at));
                let (slot, index, here) = resolve_mut(places, index, place);
                let (taken, value) = slot.tokens.remove(at - ahead).expect(NON_EMPTY);
                debug_assert_eq!(taken, token, "a token is where it was planned");
                index.left(&slot.meta, here, token, keys);
                if slot.tokens.is_empty() {
                    index.emptied(here);
                }
                consumed.push((place, token));
                ErasedInput::Token(value)
            }
            Planned::TakeAll {
                place, at, keys, ..
            } => {
                let (slot, index, here) = resolve_mut(places, index, place);
                debug_assert_eq!(at.len(), slot.tokens.len(), "every token is planned");
                let mut values = Vec::with_capacity(at.len());
                for ((token, value), keys) in slot.tokens.drain(..).zip(keys) {
                    index.left(&slot.meta, here, token, keys);
                    consumed.push((place, token));
                    values.push(value);
                }
                if slot.tokens.is_empty() {
                    index.emptied(here);
                }
                ErasedInput::Tokens(values)
            }
            // Overwritten by `borrow_mutated`, which is the only caller
            // allowed to hold the borrow; a body never sees this.
            Planned::Lent => ErasedInput::Taken,
        })
        .collect()
}

/// The key values of each token a mutating edge is about to hand its body,
/// in edge order, while they are still the old ones; `None` for a token of a
/// place without keys. See [`KeyIndex::changed`].
fn keys_before(
    places: &[PlaceSlot],
    mutated: &[(PlaceId, TokenId)],
    lent: &[Lent],
) -> Vec<Option<Vec<KeyValue>>> {
    mutated
        .iter()
        .zip(lent)
        .map(|((place, _), lent)| {
            let slot = slot_of(places, *place).expect(NON_EMPTY);
            let (_, value) = &slot.tokens[lent.at];
            KeyIndex::before(&slot.meta, &**value)
        })
        .collect()
}

/// The slot that holds a place's tokens, the key index that holds their
/// entries, and the place's id there: the place's own and this net's index,
/// or, for a window, its port's slot, index and id in the subnet.
///
/// The mutable half of [`slot_of`], over the places and the index alone, as
/// [`consume`] needs them.
#[inline]
fn resolve_mut<'a>(
    places: &'a mut [PlaceSlot],
    index: &'a mut KeyIndex,
    place: PlaceId,
) -> (&'a mut PlaceSlot, &'a mut KeyIndex, PlaceId) {
    let at = place.0 as usize;
    match places[at].meta.window {
        None => (&mut places[at], index, place),
        Some(window) => resolve_port(places, window),
    }
}

/// [`resolve_mut`]'s other branch, for a window, kept out of line.
#[cold]
fn resolve_port(
    places: &mut [PlaceSlot],
    window: Window,
) -> (&mut PlaceSlot, &mut KeyIndex, PlaceId) {
    let subnet = &mut places[window.subnet.0 as usize]
        .subnet
        .as_deref_mut()
        .expect("a window's subnet place holds its subnet")
        .net;
    (
        &mut subnet.places[window.port.0 as usize],
        &mut subnet.index,
        window.port,
    )
}

/// The clone a restoring transition takes of one token it consumes, with that
/// place's cloner.
///
/// [`build`](crate::NetBuilder::build) refuses a transition that restores and
/// consumes a place without a cloner
/// ([`RestoreOfNoClonePlace`](crate::BuildError::RestoreOfNoClonePlace)), so
/// by the time a firing runs the question has been answered.
fn restore_clone(meta: &PlaceMeta, value: &ErasedToken) -> ErasedToken {
    let clone = meta.clone.expect(
        "build rejects restoring a transition that consumes a place which cannot clone its tokens",
    );
    clone(&**value)
}

/// Marks the nets of each token a mutating edge lends the body as held by
/// this firing through that edge, whose ports `reaches` keeps, and gives
/// back what each was marked with before, for [`unmark_lent`]: `None` for a
/// token that carries no nets.
fn mark_lent(
    places: &mut [PlaceSlot],
    lent: &[Lent],
    hold: &mut Option<Arc<Hold>>,
    reaches: &Arc<[Vec<String>]>,
) -> Vec<Option<Marked>> {
    lent.iter()
        .map(|lent| {
            let slot = lent_slot(places, lent);
            let nested = slot.meta.nested?;
            let (_, value) = slot.tokens.get_mut(lent.at).expect(NON_EMPTY);
            Some(mark(
                nested,
                value,
                &Mark::by_edge(hold, reaches, lent.edge),
            ))
        })
        .collect()
}

/// Puts back what [`mark_lent`] found, once the body has returned: a
/// mutating edge's token never left its place, so it is where it was lent.
fn unmark_lent(places: &mut [PlaceSlot], lent: &[Lent], marks: Vec<Option<Marked>>) {
    for (lent, before) in lent.iter().zip(marks) {
        let Some(before) = before else {
            continue;
        };
        let slot = lent_slot(places, lent);
        let nested = slot.meta.nested.expect("a marked token carries nets");
        let (_, value) = slot.tokens.get_mut(lent.at).expect(NON_EMPTY);
        unmark(nested, value, before);
    }
}

/// The slot a lent token lies in: its place's own, or, for a window, its
/// port's in the subnet.
fn lent_slot<'a>(places: &'a mut [PlaceSlot], lent: &Lent) -> &'a mut PlaceSlot {
    let slot = &mut places[lent.holder];
    match lent.port {
        None => slot,
        Some(port) => {
            &mut slot
                .subnet
                .as_deref_mut()
                .expect("a window's subnet place holds its subnet")
                .net
                .places[port]
        }
    }
}

/// Fills in the arguments of the mutating edges and the read edges: the token
/// each one chose, borrowed where it lies (the front one for a
/// [`MutateEdge`](crate::MutateEdge) or a [`ReadEdge`](crate::ReadEdge), the
/// one its filter accepted for a [`MutateWhereEdge`](crate::MutateWhereEdge)
/// or a [`ReadWhereEdge`](crate::ReadWhereEdge), or every token for a
/// [`ReadAllEdge`](crate::ReadAllEdge)), mutably for a mutating edge and
/// shared for a read edge.
///
/// The second half of stage one, and inline firing's alone. A `&mut` into a
/// place may not be taken while [`consume`] is still draining the other
/// places, and may not outlive the call to the body, so it is taken here,
/// between the two, by the one path that calls the body inside the net.
///
/// `args` comes in owned and goes out borrowing `places` for `'a`: an
/// `ErasedInput<'static>` is an `ErasedInput<'a>` for any shorter `'a`, which
/// is what the annotation below says.
fn borrow_mutated<'a>(
    places: &'a mut [PlaceSlot],
    lent: &[Lent],
    looked: &[Lent],
    args: Vec<ErasedInput<'static>>,
) -> Vec<ErasedInput<'a>> {
    let mut args: Vec<ErasedInput<'a>> = args;
    if lent.is_empty() && looked.is_empty() {
        return args;
    }

    // `slice::get_disjoint_mut` wants its indices as a fixed-size array and a
    // transition's lending edges are only counted at run time, so the same
    // job is done with `iter_mut`: its items borrow the slice rather than the
    // iterator, so every borrow this pass takes may be alive at once, a
    // subnet's ports included, through the subnet's own `iter_mut`. That
    // they are disjoint is `BuildError::EdgeCannotSharePlace`'s doing: a
    // place a mutating or a read edge reads is read by no other input edge
    // of its transition, and a port has one window. A read edge's slot is
    // taken mutably like the others and then
    // only read through, so its tokens are lent shared, and nothing can
    // change them while the body holds them.
    for (index, slot) in places.iter_mut().enumerate() {
        let PlaceSlot { tokens, subnet, .. } = slot;
        if let Some(subnet) = subnet.as_deref_mut() {
            for (port, slot) in subnet.net.places.iter_mut().enumerate() {
                lend(
                    &mut args,
                    &mut slot.tokens,
                    (index, Some(port)),
                    lent,
                    looked,
                );
            }
            continue;
        }
        lend(&mut args, tokens, (index, None), lent, looked);
    }

    args
}

/// Fills in the argument of the one lending edge, if any, that reads this
/// slot (`at`: its holder's index and, for a port, the port's): a `&mut`
/// for a mutating edge, a shared borrow of one token or of every token for
/// a read edge.
fn lend<'a>(
    args: &mut [ErasedInput<'a>],
    tokens: &'a mut VecDeque<(TokenId, ErasedToken)>,
    at: (usize, Option<usize>),
    lent: &[Lent],
    looked: &[Lent],
) {
    let here = |lent: &&Lent| (lent.holder, lent.port) == at;
    if let Some(lent) = lent.iter().find(here) {
        let (_, value) = tokens.get_mut(lent.at).expect(NON_EMPTY);
        args[lent.arg] = ErasedInput::Borrowed(value);
        return;
    }
    let Some(look) = looked.iter().find(here) else {
        return;
    };
    let tokens: &'a VecDeque<(TokenId, ErasedToken)> = tokens;
    args[look.arg] = if look.all {
        ErasedInput::LentAll(tokens.iter().map(|(_, value)| value).collect())
    } else {
        ErasedInput::Lent(&tokens.get(look.at).expect(NON_EMPTY).1)
    };
}
