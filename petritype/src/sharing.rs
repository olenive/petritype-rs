//! Edges that share a place: two or more input edges of one transition on
//! one place, each taking a token of its own (DESIGN §4.4, since 0.4.0).
//!
//! # Which edges may
//!
//! The edges that take one token: [`TakeEdge`](crate::TakeEdge) and
//! [`TakeWhereEdge`](crate::TakeWhereEdge), in any mix. Two oxygen radicals
//! taken from one place by two take edges make one molecule, and the
//! transition is fireable only while the place holds two; from a ballroom,
//! one keyed edge takes a lady and another a gentleman, fireable only while
//! the ballroom holds one of each, and the two never take the same token.
//! There is no count on a single edge: an edge is what gives each token its
//! argument.
//!
//! Every other kind beside another edge on its place is refused when the
//! net is built ([`check`], [`BuildError::EdgeCannotSharePlace`]): a
//! mutating or a read edge lends the body a token where it lies, a snapshot
//! or a survey copies one or all, and a take-all or a drain takes every
//! token, each a token another edge of the same firing may take. A bound
//! edge and its source ([`Clause::IsInput`](crate::Clause::IsInput)) choose
//! their tokens as a pair, and are refused on a shared place as well
//! ([`BuildError::BoundPlaceShared`], from
//! [`bindings`](crate::pairing::bindings)).
//!
//! # Which tokens
//!
//! The edges on one place choose in declaration order, each the first token
//! in queue order that no edge before it chose: a take edge the front one
//! left, a take-where edge the first left that its filter accepts. The
//! choice is greedy, as Python petritype's planning is: an edge never gives
//! its token up for a later edge. So when the first of two filters takes
//! the only token the second also accepts, the transition is not fireable,
//! though the other way round it would have been; declare the edge with the
//! narrower filter first.
//!
//! Fireability asks [`share`], and stage one asks it again of the same
//! marking before anything moves, so it takes the tokens fireability found.

use std::collections::HashMap;

use crate::deep::Busy;
use crate::error::BuildError;
use crate::ids::{PlaceId, TokenId};
use crate::index::{KeyIndex, all_resolved, first_resolved};
use crate::net::slot_of;
use crate::param::ParamSlot;
use crate::place::PlaceSlot;
use crate::transition::TransitionSlot;

/// The tokens the shared edges chose, as `(edge, position, token)`, in edge
/// order: the same shape as a pairing's
/// [`Chosen`](crate::pairing::Chosen), so that stage one reads both alike.
/// The position is the token's place in its queue (its port's, for a
/// window) as the marking stands before anything moves.
pub(crate) type Shared = Vec<(usize, usize, TokenId)>;

/// The input edges of a transition whose place another of its input edges
/// reads, in edge order: what [`TransitionSlot::shared`] holds. Empty when
/// every edge reads a place of its own.
pub(crate) fn shared_edges(inputs: &[(PlaceId, crate::edge::EdgeKind)]) -> Vec<usize> {
    let mut edges_on: HashMap<PlaceId, usize> = HashMap::with_capacity(inputs.len());
    for (place, _) in inputs {
        *edges_on.entry(*place).or_default() += 1;
    }
    inputs
        .iter()
        .enumerate()
        .filter(|(_, (place, _))| edges_on[place] > 1)
        .map(|(edge, _)| edge)
        .collect()
}

/// Refuses a transition that reads one place through two or more edges of
/// which one cannot share it: [`BuildError::EdgeCannotSharePlace`] for the
/// first place in edge order that a second edge reads, naming the first
/// kind there that is neither a take nor a take-where.
pub(crate) fn check(transition: &TransitionSlot, places: &[PlaceSlot]) -> Result<(), BuildError> {
    for &edge in &transition.shared {
        let (place, _) = transition.inputs[edge];
        if let Some(&(_, kind)) = transition
            .inputs
            .iter()
            .find(|(other, kind)| *other == place && !kind.takes_one())
        {
            return Err(BuildError::EdgeCannotSharePlace {
                transition: transition.name.clone(),
                place: places[place.0 as usize].meta.name.clone(),
                kind,
            });
        }
    }
    Ok(())
}

/// The token each shared edge of the transition takes now, by the greedy
/// rule of the module docs, or `None` when one of them has none left to
/// take. Empty for a transition with no shared edge.
///
/// A take-where edge asks the key index for its first match, and only when
/// an earlier edge chose that one reads on through every match in queue
/// order; a take edge counts from the front past the positions chosen.
///
/// With `busy`, the concurrent driver's, every edge also passes over the
/// busy tokens ([`Busy::for_edge`]), as it passes over a token an edge
/// before it chose.
pub(crate) fn share(
    places: &[PlaceSlot],
    params: &[ParamSlot],
    index: &KeyIndex,
    transition: &TransitionSlot,
    busy: Option<Busy<'_>>,
) -> Option<Shared> {
    let mut chosen: Shared = Vec::with_capacity(transition.shared.len());
    for &edge in &transition.shared {
        let (place, kind) = transition.inputs[edge];
        let slot = slot_of(places, place)?;
        let passed = Busy::for_edge(busy, kind);
        let taken = |at: usize| {
            passed.is_some_and(|busy| busy.holds(slot, at))
                || chosen
                    .iter()
                    .any(|&(other, there, _)| there == at && transition.inputs[other].0 == place)
        };
        let found = match &transition.filters[edge] {
            None => (0..slot.tokens.len())
                .find(|&at| !taken(at))
                .map(|at| (at, slot.tokens[at].0)),
            Some(filter) => {
                let resolved = filter.resolve_in(params);
                match first_resolved(places, index, place, &resolved) {
                    Some((at, token)) if !taken(at) => Some((at, token)),
                    Some(_) => {
                        all_resolved(places, index, place, &resolved).find(|&(at, _)| !taken(at))
                    }
                    None => None,
                }
            }
        };
        let (at, token) = found?;
        chosen.push((edge, at, token));
    }
    Some(chosen)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::EdgeKind;
    use crate::{Filter, KeyValue, Net, NetBuilder, PlaceRef, TakeWhereEdge};

    #[derive(Debug, Clone, PartialEq)]
    struct Dancer {
        name: &'static str,
        role: &'static str,
    }

    fn dancer(name: &'static str, role: &'static str) -> Dancer {
        Dancer { name, role }
    }

    fn role(role: &str) -> Filter {
        Filter::new().key_in("role", [role])
    }

    /// A ballroom of dancers keyed by role, and a transition `Pair` that
    /// takes through `edges`.
    fn ballroom(
        seed: &[Dancer],
        edges: impl FnOnce(PlaceRef<Dancer>) -> (TakeWhereEdge<Dancer>, TakeWhereEdge<Dancer>),
    ) -> (Net, PlaceRef<Dancer>) {
        let mut b = NetBuilder::new();
        let hall = b.place::<Dancer>("Ballroom");
        let floor = b.place::<(Dancer, Dancer)>("Floor");
        b.keys(hall, ["role"], |d: &Dancer| [KeyValue::from(d.role)]);
        b.seed(hall, seed.iter().cloned());
        b.transition("Pair", edges(hall), (floor,), |a: Dancer, c: Dancer| {
            ((a, c),)
        });
        (b.build().unwrap(), hall)
    }

    #[test]
    fn the_shared_edges_are_those_on_a_place_another_edge_reads() {
        let (a, c, d) = (PlaceId(0), PlaceId(1), PlaceId(2));
        let take = EdgeKind::Take;
        assert!(shared_edges(&[(a, take), (c, take)]).is_empty());
        assert_eq!(
            shared_edges(&[(a, take), (c, take), (a, take), (d, take), (c, take)]),
            [0, 1, 2, 4]
        );
    }

    /// Each edge chooses past the tokens the edges before it chose, through
    /// the index or by reading on, and the index stays right as the tokens
    /// leave from the middle and come back to the front.
    #[test]
    fn a_shared_choice_skips_what_an_earlier_edge_chose_and_the_index_follows() {
        let any = Filter::new();
        let seed = [
            dancer("Ada", "lady"),
            dancer("Ben", "gentleman"),
            dancer("Cleo", "lady"),
            dancer("Dan", "gentleman"),
        ];
        let (mut net, hall) = ballroom(&seed, |hall| {
            (
                TakeWhereEdge(hall, role("lady")),
                TakeWhereEdge(hall, any.clone()),
            )
        });
        let transition = &net.transitions[0];
        let ids: Vec<TokenId> = net.tokens(hall.id()).map(|(id, _)| id).collect();
        assert_eq!(
            share(&net.places, &net.params, &net.index, transition, None),
            Some(vec![(0, 0, ids[0]), (1, 1, ids[1])]),
            "the first lady, then the first dancer she leaves"
        );
        net.step().unwrap();
        net.index_agrees().unwrap();
        let transition = &net.transitions[0];
        assert_eq!(
            share(&net.places, &net.params, &net.index, transition, None),
            Some(vec![(0, 0, ids[2]), (1, 1, ids[3])])
        );
        net.step().unwrap();
        net.index_agrees().unwrap();
        assert_eq!(net.count(hall.id()), 0);
        let transition = &net.transitions[0];
        assert_eq!(
            share(&net.places, &net.params, &net.index, transition, None),
            None
        );
    }
}
