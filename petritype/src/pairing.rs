//! Bound edges: a keyed edge that chooses its token by a key of the token
//! another input of the same firing gives (DESIGN §8.3, since 2026-11-23).
//!
//! A [`Clause::IsInput`](crate::Clause::IsInput) in the filter of a
//! [`TakeWhereEdge`](crate::TakeWhereEdge) or a
//! [`MutateWhereEdge`](crate::MutateWhereEdge) names another input place of
//! the transition, the edge's *source*, and a key of it: "the unit this order
//! names". The edge is *bound* to that source, and the firing chooses the two
//! tokens as a pair.
//!
//! # Which pair
//!
//! The oldest source token that has a match, with the oldest match for it.
//! The source's candidates are its place's tokens in queue order, or, for a
//! source that is keyed itself, the tokens its own filter accepts, in queue
//! order. For each candidate, every edge bound to the source asks its place
//! for the first token its filter accepts, each input clause reading the
//! candidate's key: one lookup in the key index per bound edge
//! ([`first_resolved`]), never a scan of the bound place. The first candidate
//! that every bound edge has a match for is the pair; a candidate without one
//! stays where it is and does not hold back those behind it.
//!
//! Fireability asks this of every source of the transition (several sources
//! are independent: each reads its own place, and so does each bound edge),
//! and launch asks it again of the same marking before anything moves, so it
//! chooses the pair fireability found.
//!
//! # What a build checks
//!
//! [`bindings`] reads a transition's filters once, at
//! [`build`](crate::NetBuilder::build), and refuses what has no one pair:
//! a source that is not an input of the transition, one that gives every
//! token rather than one, one that is itself bound (a chain, or the edge's own
//! place, the shortest cycle), a filter naming two sources, and a key that
//! either place does not declare. Two edges of one transition never read one
//! place ([`DuplicateInputPlace`](crate::BuildError::DuplicateInputPlace)),
//! so a place names one edge.

use crate::edge::EdgeKind;
use crate::error::BuildError;
use crate::ids::{PlaceId, TokenId};
use crate::index::{KeyIndex, all_resolved, first_resolved};
use crate::keys::{Filter, Resolved, keys_or_none};
use crate::net::slot_of;
use crate::param::ParamSlot;
use crate::place::PlaceSlot;
use crate::transition::TransitionSlot;

/// One source of a transition and the edges bound to it, each by its index
/// among the transition's input edges ([`TransitionSlot::inputs`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Binding {
    /// The edge whose token the others choose by.
    pub(crate) source: usize,
    /// The edges bound to it, in declaration order.
    pub(crate) bound: Vec<usize>,
}

/// The tokens one binding chose, as `(edge, position, token)`: the source's
/// first, then each bound edge's in the binding's order. The position is the
/// token's place in its queue (its port's, for a window), good until the
/// edge's turn in stage one, since every other edge reads another place.
pub(crate) type Chosen = Vec<(usize, usize, TokenId)>;

/// Whether a source of this kind gives one token, which is what a key can be
/// read from: a take, a snapshot, a mutation or a read, keyed or not.
fn gives_one(kind: EdgeKind) -> bool {
    matches!(
        kind,
        EdgeKind::Take
            | EdgeKind::TakeWhere
            | EdgeKind::Snapshot
            | EdgeKind::Mutate
            | EdgeKind::MutateWhere
            | EdgeKind::Read
            | EdgeKind::ReadWhere
    )
}

/// Whether a filter holds a [`Clause::IsInput`](crate::Clause::IsInput):
/// whether its edge is bound.
fn is_bound(filter: Option<&Filter>) -> bool {
    filter.is_some_and(|filter| filter.inputs().next().is_some())
}

/// Whether the place declares a key of this name.
fn declares(places: &[PlaceSlot], place: PlaceId, key: &str) -> bool {
    places[place.0 as usize]
        .meta
        .keys
        .as_ref()
        .is_some_and(|keys| keys.names.contains(&key))
}

/// The bindings of one transition, checked: one per source, in the order of
/// the first edge bound to each. Empty for a transition with no input
/// clause, which is almost every transition.
///
/// # Errors
///
/// For the first bound edge, in edge order, that names two sources
/// ([`BuildError::BoundToTwoSources`]), a place the transition does not
/// read ([`BuildError::BoundSourceNotAnInput`]), a source that gives every
/// token ([`BuildError::BoundSourceGivesMany`]), a source that is itself
/// bound or its own place ([`BuildError::BoundSourceIsBound`]), or a key
/// its place or its source's does not declare
/// ([`BuildError::BoundKeyNotDeclared`]).
pub(crate) fn bindings(
    transition: &TransitionSlot,
    places: &[PlaceSlot],
) -> Result<Vec<Binding>, BuildError> {
    let name = |place: PlaceId| places[place.0 as usize].meta.name.clone();
    let mut bindings: Vec<Binding> = Vec::new();
    for (edge, filter) in transition.filters.iter().enumerate() {
        let Some(filter) = filter else {
            continue;
        };
        let place = transition.inputs[edge].0;
        let mut source: Option<PlaceId> = None;
        for (_, input) in filter.inputs() {
            match source {
                Some(first) if first != input.place => {
                    return Err(BuildError::BoundToTwoSources {
                        transition: transition.name.clone(),
                        place: name(place),
                        first: name(first),
                        second: name(input.place),
                    });
                }
                _ => source = Some(input.place),
            }
        }
        let Some(source) = source else {
            continue;
        };
        let Some(from) = transition
            .inputs
            .iter()
            .position(|(read, _)| *read == source)
        else {
            // The id may be of another net's place, past this net's.
            return Err(BuildError::BoundSourceNotAnInput {
                transition: transition.name.clone(),
                place: name(place),
                input: places
                    .get(source.0 as usize)
                    .map_or_else(|| format!("{source:?}"), |slot| slot.meta.name.clone()),
            });
        };
        if !gives_one(transition.inputs[from].1) {
            return Err(BuildError::BoundSourceGivesMany {
                transition: transition.name.clone(),
                place: name(place),
                input: name(source),
            });
        }
        if from == edge || is_bound(transition.filters[from].as_ref()) {
            return Err(BuildError::BoundSourceIsBound {
                transition: transition.name.clone(),
                place: name(place),
                input: name(source),
            });
        }
        for (key, input) in filter.inputs() {
            for (owner, key) in [(place, key), (source, input.key.as_str())] {
                if !declares(places, owner, key) {
                    return Err(BuildError::BoundKeyNotDeclared {
                        transition: transition.name.clone(),
                        place: name(place),
                        key: key.to_owned(),
                        owner: name(owner),
                    });
                }
            }
        }
        match bindings.iter_mut().find(|binding| binding.source == from) {
            Some(binding) => binding.bound.push(edge),
            None => bindings.push(Binding {
                source: from,
                bound: vec![edge],
            }),
        }
    }
    Ok(bindings)
}

/// The pair one binding fires with now, as [`Chosen`], or `None` when no
/// source token has a match in every edge bound to it. See the module docs
/// for which pair.
///
/// The source's own filter and each bound edge's are resolved against the
/// net's parameters once; each candidate then costs one extraction of its
/// keys and one index lookup per bound edge.
pub(crate) fn pair(
    places: &[PlaceSlot],
    params: &[ParamSlot],
    index: &KeyIndex,
    transition: &TransitionSlot,
    binding: &Binding,
) -> Option<Chosen> {
    let (source, _) = transition.inputs[binding.source];
    let everything = Filter::new();
    let own = transition.filters[binding.source]
        .as_ref()
        .unwrap_or(&everything)
        .resolve_in(params);
    let mut bound: Vec<(usize, PlaceId, Resolved<'_>)> = binding
        .bound
        .iter()
        .map(|&edge| {
            let filter = transition.filters[edge]
                .as_ref()
                .expect("a bound edge is keyed, so it carries a filter");
            (edge, transition.inputs[edge].0, filter.resolve_in(params))
        })
        .collect();
    // The slot the source's tokens are in: its own, or its port's.
    let slot = slot_of(places, source)?;
    for (at, token) in all_resolved(places, index, source, &own) {
        let keys = keys_or_none(slot, &*slot.tokens[at].1);
        let mut chosen: Chosen = Vec::with_capacity(1 + bound.len());
        chosen.push((binding.source, at, token));
        let matched = bound.iter_mut().all(|(edge, place, resolved)| {
            resolved.bind(&keys);
            first_resolved(places, index, *place, resolved)
                .map(|(at, token)| chosen.push((*edge, at, token)))
                .is_some()
        });
        if matched {
            return Some(chosen);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::Resolved;
    use crate::{Filter, MutateWhereEdge, Net, NetBuilder, TakeEdge, TakeWhereEdge};

    #[derive(Debug)]
    struct Broken;

    impl std::fmt::Display for Broken {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("broken")
        }
    }

    impl std::error::Error for Broken {}

    /// The first token of `place` in queue order that `resolved` accepts,
    /// by extracting every token's keys: what the index is checked against.
    fn first_scan(
        places: &[PlaceSlot],
        place: PlaceId,
        resolved: &Resolved<'_>,
    ) -> Option<(usize, TokenId)> {
        let slot = slot_of(places, place)?;
        if !resolved.clauses.is_empty() && slot.meta.keys.is_none() {
            return None;
        }
        slot.tokens
            .iter()
            .position(|(_, value)| resolved.accepts(&keys_or_none(slot, &**value)))
            .map(|at| (at, slot.tokens[at].0))
    }

    /// [`pair`] by reading every place front to back.
    fn pair_scan(net: &Net, transition: &TransitionSlot, binding: &Binding) -> Option<Chosen> {
        let (source, _) = transition.inputs[binding.source];
        let everything = Filter::new();
        let own = transition.filters[binding.source]
            .as_ref()
            .unwrap_or(&everything)
            .resolve_in(&net.params);
        let slot = slot_of(&net.places, source)?;
        for (at, (token, value)) in slot.tokens.iter().enumerate() {
            let keys = keys_or_none(slot, &**value);
            if !own.accepts(&keys) {
                continue;
            }
            let mut chosen = vec![(binding.source, at, *token)];
            let all = binding.bound.iter().all(|&edge| {
                let mut resolved = transition.filters[edge]
                    .as_ref()
                    .unwrap()
                    .resolve_in(&net.params);
                resolved.bind(&keys);
                first_scan(&net.places, transition.inputs[edge].0, &resolved)
                    .map(|(at, token)| chosen.push((edge, at, token)))
                    .is_some()
            });
            if all {
                return Some(chosen);
            }
        }
        None
    }

    /// The indexed pairing and the scanning one agree for every binding of
    /// every transition.
    fn check(net: &Net) {
        for transition in &net.transitions {
            for binding in &transition.bindings {
                assert_eq!(
                    pair(&net.places, &net.params, &net.index, transition, binding),
                    pair_scan(net, transition, binding),
                    "{}",
                    transition.name
                );
            }
        }
    }

    /// Orders name a unit; units are found by name. The pairing from the
    /// index agrees with a scan as tokens come, go, are taken from the
    /// middle, and come back out of id order.
    #[test]
    fn the_indexed_pairing_agrees_with_a_scan() {
        let mut b = NetBuilder::new();
        let orders = b.place::<(String, String)>("Orders");
        let units = b.place::<(String, u32)>("Units");
        b.keys(
            orders,
            ["unit", "item"],
            |(unit, item): &(String, String)| [unit.as_str().into(), item.as_str().into()],
        );
        b.keys(units, ["name"], |(name, _): &(String, u32)| {
            [name.as_str().into()]
        });
        let order = |unit: &str, item: &str| (unit.to_owned(), item.to_owned());
        b.seed(
            orders,
            [
                order("Cleo", "Lamp"),
                order("Ben", "Book"),
                order("Ada", "Kite"),
                order("Ben", "Kite"),
            ],
        );
        b.seed(units, [("Ada".to_owned(), 0), ("Ben".to_owned(), 0)]);
        let named = Filter::new().key_is_input("name", orders, "unit");
        b.transition(
            "Kites",
            (
                TakeWhereEdge(orders, Filter::new().key_in("item", ["Kite"])),
                MutateWhereEdge(units, named.clone()),
            ),
            (),
            |_: (String, String), unit: &mut (String, u32)| unit.1 += 1,
        );
        for name in ["Fail", "Fail Too"] {
            b.transition(
                name,
                (TakeEdge(orders), TakeWhereEdge(units, named.clone())),
                (),
                |_: (String, String), _: (String, u32)| -> Result<(), Broken> { Err(Broken) },
            )
            .restore_tokens_on_failure();
        }
        let mut net = b.build().unwrap();
        let all: Vec<_> = net.transitions().map(|(id, _)| id).collect();
        for transition in &all {
            net.disable(*transition).unwrap();
        }
        let fire = |net: &mut Net, name: &str| {
            let transition = net.transition_named(name).unwrap();
            net.enable(transition).unwrap();
            let fired = net.fire(transition);
            net.disable(transition).unwrap();
            fired
        };
        check(&net);

        fire(&mut net, "Kites").unwrap();
        check(&net);
        net.put(units, ("Cleo".to_owned(), 0));
        check(&net);
        // Two restoring firings, distributed in the order they launched
        // behind a token put in meanwhile, put the places out of id order.
        let ids = |net: &Net, place: PlaceId| -> Vec<TokenId> {
            net.tokens(place).map(|(id, _)| id).collect()
        };
        for name in ["Fail", "Fail Too"] {
            let transition = net.transition_named(name).unwrap();
            net.enable(transition).unwrap();
        }
        let first = net.launch(net.transition_named("Fail").unwrap()).unwrap();
        let second = net
            .launch(net.transition_named("Fail Too").unwrap())
            .unwrap();
        check(&net);
        net.put(orders, order("Ada", "Kite"));
        net.put(units, ("Ben".to_owned(), 7));
        check(&net);
        net.distribute(first.run()).unwrap_err();
        check(&net);
        net.distribute(second.run()).unwrap_err();
        check(&net);
        for name in ["Fail", "Fail Too"] {
            let transition = net.transition_named(name).unwrap();
            net.disable(transition).unwrap();
        }
        assert!(
            !ids(&net, orders.id()).is_sorted(),
            "orders out of id order"
        );
        assert!(!ids(&net, units.id()).is_sorted(), "units out of id order");
        fire(&mut net, "Kites").unwrap();
        check(&net);
        fire(&mut net, "Kites").unwrap();
        check(&net);
        let (front, _) = net.tokens(units.id()).next().unwrap();
        net.remove(units.id(), front).unwrap();
        check(&net);
        net.index_agrees().unwrap();
    }
}
