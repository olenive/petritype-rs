//! The core's ids and read API turned into the protocol's names and shapes.

use std::collections::HashMap;

use petritype::{EdgeInfo, Net, PlaceId, TokenId, Topology, TransitionId};

use crate::protocol;

/// The protocol's cap on a label, in characters (PROTOCOL.md §5.3).
pub(crate) const LABEL_LIMIT: usize = 1000;

/// `text` cut so that it fits [`LABEL_LIMIT`] characters *including* the
/// `"..."` appended when anything was cut (PROTOCOL.md §5.3: the cap counts
/// the marker). On a character boundary. Whatever the net's own
/// `label_limit`: the wire's cap is the protocol's, not the author's.
pub(crate) fn cut(text: String) -> String {
    const MARKER: &str = "...";
    if text.chars().count() <= LABEL_LIMIT {
        return text;
    }
    let keep = LABEL_LIMIT - MARKER.chars().count();
    let end = text.char_indices().nth(keep).map_or(text.len(), |(i, _)| i);
    let mut text = text;
    text.truncate(end);
    text.push_str(MARKER);
    text
}

/// A token's wire label: its `Debug`, cut. Empty for a token that is gone.
pub(crate) fn label(net: &Net, place: PlaceId, token: TokenId) -> String {
    net.debug(place, token).map(cut).unwrap_or_default()
}

/// The name tables of one generation, taken from its [`Topology`] once.
#[derive(Debug)]
pub(crate) struct Names {
    places: HashMap<PlaceId, String>,
    transitions: HashMap<TransitionId, String>,
}

impl Names {
    pub(crate) fn new(topology: &Topology) -> Names {
        Names {
            places: topology
                .places
                .iter()
                .map(|p| (p.id, p.name.clone()))
                .collect(),
            transitions: topology
                .transitions
                .iter()
                .map(|t| (t.id, t.name.clone()))
                .collect(),
        }
    }

    pub(crate) fn place(&self, place: PlaceId) -> String {
        self.places
            .get(&place)
            .cloned()
            .unwrap_or_else(|| place.to_string())
    }

    pub(crate) fn transition(&self, transition: TransitionId) -> String {
        self.transitions
            .get(&transition)
            .cloned()
            .unwrap_or_else(|| transition.to_string())
    }

    /// `[place, id]` pairs.
    pub(crate) fn pairs(&self, tokens: &[(PlaceId, TokenId)]) -> Vec<(String, String)> {
        tokens
            .iter()
            .map(|&(place, token)| (self.place(place), token.to_string()))
            .collect()
    }

    /// `[place, id, label]` triples, labelled from the live net.
    pub(crate) fn produced(
        &self,
        net: &Net,
        tokens: &[(PlaceId, TokenId)],
    ) -> Vec<(String, String, String)> {
        tokens
            .iter()
            .map(|&(place, token)| {
                (
                    self.place(place),
                    token.to_string(),
                    label(net, place, token),
                )
            })
            .collect()
    }
}

/// The topology keyed by names (PROTOCOL.md §5.1): edges with a direction,
/// `kind` on inputs, `slot` only for a transition with more than one output,
/// `detail: {params, detachable}`, and no `hints`.
pub(crate) fn topology(topology: &Topology, names: &Names) -> protocol::Topology {
    let outputs: HashMap<TransitionId, usize> = topology
        .transitions
        .iter()
        .map(|t| (t.id, t.outputs.len()))
        .collect();
    protocol::Topology {
        places: topology
            .places
            .iter()
            .map(|p| protocol::Place {
                name: p.name.clone(),
                type_name: p.type_name.to_owned(),
                serializable: p.serializable,
            })
            .collect(),
        transitions: topology
            .transitions
            .iter()
            .map(|t| protocol::Transition {
                name: t.name.clone(),
                detail: protocol::TransitionDetail {
                    params: t.params.iter().map(|p| (*p).to_owned()).collect(),
                    detachable: t.detachable,
                },
            })
            .collect(),
        edges: topology
            .edges
            .iter()
            .filter_map(|edge| match *edge {
                EdgeInfo::Input {
                    place,
                    transition,
                    kind,
                } => Some(protocol::Edge {
                    place: names.place(place),
                    transition: names.transition(transition),
                    direction: protocol::Direction::In,
                    kind: Some(edge_kind(kind)),
                    slot: None,
                }),
                EdgeInfo::Output {
                    transition,
                    place,
                    slot,
                } => Some(protocol::Edge {
                    place: names.place(place),
                    transition: names.transition(transition),
                    direction: protocol::Direction::Out,
                    kind: None,
                    slot: (outputs.get(&transition).copied().unwrap_or(0) > 1).then_some(slot),
                }),
                // A kind of edge this runner does not know yet: left out, as
                // a reader of the protocol would skip it.
                _ => None,
            })
            .collect(),
    }
}

fn edge_kind(kind: petritype::EdgeKind) -> protocol::EdgeKind {
    match kind {
        petritype::EdgeKind::Take => protocol::EdgeKind::Take,
        petritype::EdgeKind::TakeAll => protocol::EdgeKind::TakeAll,
        petritype::EdgeKind::Snapshot => protocol::EdgeKind::Snapshot,
        petritype::EdgeKind::Mutate => protocol::EdgeKind::Mutate,
    }
}

/// Up to `limit` tokens of `place` from `offset`, oldest first, as wire
/// tokens.
pub(crate) fn tokens(
    net: &Net,
    place: PlaceId,
    offset: usize,
    limit: usize,
) -> Vec<protocol::Token> {
    net.tokens(place)
        .skip(offset)
        .take(limit)
        .map(|(token, _)| protocol::Token {
            id: token.to_string(),
            label: label(net, place, token),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_label_is_untouched() {
        assert_eq!(cut("abc".into()), "abc");
        let exact = "x".repeat(LABEL_LIMIT);
        assert_eq!(cut(exact.clone()), exact);
    }

    #[test]
    fn a_long_label_is_cut_to_1000_characters_including_the_ellipsis() {
        let long = "y".repeat(LABEL_LIMIT + 1);
        let short = cut(long);
        assert_eq!(short.chars().count(), LABEL_LIMIT);
        assert!(short.ends_with("y..."));
        // Exactly at the cap nothing is cut.
        let exact = cut("y".repeat(LABEL_LIMIT));
        assert_eq!(exact.chars().count(), LABEL_LIMIT);
        assert!(!exact.ends_with("..."));
    }

    #[test]
    fn the_cut_is_on_a_character_boundary() {
        // Two bytes per character: a byte-based cut would split one.
        let long = "é".repeat(LABEL_LIMIT + 5);
        let cut = cut(long);
        assert_eq!(cut.chars().count(), LABEL_LIMIT);
        assert_eq!(cut.chars().filter(|c| *c == 'é').count(), LABEL_LIMIT - 3);
    }
}
