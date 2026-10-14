//! The core's ids and read API turned into the protocol's names and shapes,
//! and a request's filter turned into the core's.

use std::collections::HashMap;
use std::fmt;

use petritype::{Clause, EdgeInfo, Filter, Net, PlaceId, TokenId, Topology, TransitionId};
use serde_json::Value;

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
/// Empty by default: every id then prints as its number.
#[derive(Debug, Default)]
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

    /// `produced` entries, labelled from the live net, each with its keys
    /// where its place declares some (PROTOCOL.md §7.1).
    pub(crate) fn produced(
        &self,
        net: &Net,
        tokens: &[(PlaceId, TokenId)],
    ) -> Vec<protocol::Produced> {
        tokens
            .iter()
            .map(|&(place, token)| protocol::Produced {
                place: self.place(place),
                id: token.to_string(),
                label: label(net, place, token),
                keys: keys(net, place, token),
            })
            .collect()
    }
}

/// A token's keys as the wire carries them (PROTOCOL.md §13.1): `Some` when
/// its place declares keys, in declaration order; `None` for a place that
/// declares none, or a token that is gone.
pub(crate) fn keys(net: &Net, place: PlaceId, token: TokenId) -> Option<protocol::Keys> {
    net.keys(place, token)
        .filter(|keys| !keys.is_empty())
        .map(|keys| {
            protocol::Keys(
                keys.iter()
                    .map(|(name, value)| (name.to_owned(), value.clone()))
                    .collect(),
            )
        })
}

/// A filter of the core's, places by id, as the wire writes it, places by
/// name (§13.2): what a keyed edge carries in the topology (§13.5), a
/// `{key, param}` clause as declared.
pub(crate) fn filter_to_wire(filter: &Filter, names: &Names) -> protocol::Filter {
    protocol::Filter {
        clauses: filter
            .clauses
            .iter()
            .filter_map(|clause| match clause {
                Clause::In { key, values } => Some(protocol::Clause::In {
                    key: key.clone(),
                    values: values.clone(),
                }),
                Clause::IsParam { key, param } => Some(protocol::Clause::IsParam {
                    key: key.clone(),
                    param: param.clone(),
                }),
                // A kind of clause this runner does not know yet: left out,
                // as an unknown kind of edge is.
                _ => None,
            })
            .collect(),
        places: filter
            .places
            .as_ref()
            .map(|places| places.iter().map(|&place| names.place(place)).collect()),
        prefix: filter.prefix.clone(),
    }
}

/// Why a request's filter could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FilterError {
    /// Not a filter's shape (PROTOCOL.md §13.2): `invalid_params`.
    Shape(String),
    /// `places` names a place the net does not have: `unknown_place`.
    UnknownPlace(String),
}

impl fmt::Display for FilterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FilterError::Shape(message) => f.write_str(message),
            FilterError::UnknownPlace(name) => write!(f, "no place named {name:?}"),
        }
    }
}

/// A request's filter as the core's, for `net`: the shape read strictly
/// ([`protocol::Filter::parse`]) and each place name turned into the id of
/// that place of `net`.
pub(crate) fn filter(value: &Value, net: &Net) -> Result<Filter, FilterError> {
    let wire = protocol::Filter::parse(value).map_err(FilterError::Shape)?;
    filter_from_wire(&wire, net)
}

/// A wire filter's places, by name, turned into `net`'s ids.
pub(crate) fn filter_from_wire(wire: &protocol::Filter, net: &Net) -> Result<Filter, FilterError> {
    let places = wire
        .places
        .as_ref()
        .map(|names| place_ids(names, net))
        .transpose()?;
    Ok(Filter {
        clauses: wire
            .clauses
            .iter()
            .map(|clause| match clause {
                protocol::Clause::In { key, values } => Clause::In {
                    key: key.clone(),
                    values: values.clone(),
                },
                protocol::Clause::IsParam { key, param } => Clause::IsParam {
                    key: key.clone(),
                    param: param.clone(),
                },
            })
            .collect(),
        places,
        prefix: wire.prefix.clone(),
    })
}

/// Place names turned into `net`'s ids; the first that names no place is
/// [`FilterError::UnknownPlace`].
pub(crate) fn place_ids(names: &[String], net: &Net) -> Result<Vec<PlaceId>, FilterError> {
    names
        .iter()
        .map(|name| {
            net.place_named(name)
                .ok_or_else(|| FilterError::UnknownPlace(name.clone()))
        })
        .collect()
}

/// The topology keyed by names (PROTOCOL.md §5.1): places with `nested` and
/// `passive` when true (§12.2), edges with a direction, `kind` on inputs,
/// `slot` only for a transition with more than one output that returns
/// them by position, `detail: {params, detachable}`, and no `hints`.
///
/// A routed or distributing body has no slots to speak of: its outputs find
/// their places by type or by name, and the declared order is the author's
/// only. Python petritype writes `slot` from a return edge's `return_index`,
/// which an output distribution function's edges do not have, so the two
/// runtimes draw `transit`'s `Arrive` alike.
pub(crate) fn topology(topology: &Topology, names: &Names) -> protocol::Topology {
    let slotted: HashMap<TransitionId, bool> = topology
        .transitions
        .iter()
        .map(|t| (t.id, t.positional && t.outputs.len() > 1))
        .collect();
    protocol::Topology {
        places: topology
            .places
            .iter()
            .map(|p| protocol::Place {
                name: p.name.clone(),
                type_name: p.type_name.to_owned(),
                serializable: p.serializable,
                nested: p.nested,
                passive: p.passive,
                key_names: p.key_names.iter().map(|name| (*name).to_owned()).collect(),
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
            .filter_map(|edge| match edge {
                EdgeInfo::Input {
                    place,
                    transition,
                    kind,
                    filter,
                    ..
                } => Some(protocol::Edge {
                    place: names.place(*place),
                    transition: names.transition(*transition),
                    direction: protocol::Direction::In,
                    kind: edge_kind(*kind),
                    // A keyed edge's filter as declared (§13.5), its places
                    // by name.
                    filter: filter.as_ref().map(|filter| filter_to_wire(filter, names)),
                    slot: None,
                }),
                EdgeInfo::Output {
                    transition,
                    place,
                    slot,
                } => Some(protocol::Edge {
                    place: names.place(*place),
                    transition: names.transition(*transition),
                    direction: protocol::Direction::Out,
                    kind: None,
                    filter: None,
                    slot: slotted
                        .get(transition)
                        .copied()
                        .unwrap_or(false)
                        .then_some(*slot),
                }),
                // A kind of edge this runner does not know yet: left out, as
                // a reader of the protocol would skip it.
                _ => None,
            })
            .collect(),
    }
}

/// The wire's name for an input edge's kind, or `None` for a kind this
/// runner does not know, which goes out without `kind` and is drawn as a
/// plain edge (PROTOCOL.md §5.1).
fn edge_kind(kind: petritype::EdgeKind) -> Option<protocol::EdgeKind> {
    match kind {
        petritype::EdgeKind::Take => Some(protocol::EdgeKind::Take),
        petritype::EdgeKind::TakeAll => Some(protocol::EdgeKind::TakeAll),
        petritype::EdgeKind::Snapshot => Some(protocol::EdgeKind::Snapshot),
        petritype::EdgeKind::Mutate => Some(protocol::EdgeKind::Mutate),
        petritype::EdgeKind::TakeWhere => Some(protocol::EdgeKind::TakeWhere),
        petritype::EdgeKind::MutateWhere => Some(protocol::EdgeKind::MutateWhere),
        petritype::EdgeKind::Drain => Some(protocol::EdgeKind::Drain),
        petritype::EdgeKind::Survey => Some(protocol::EdgeKind::Survey),
        _ => None,
    }
}

/// Up to `limit` tokens of `place` from `offset`, oldest first, as wire
/// tokens, each with its keys and the nets it holds. `path` is the wire path
/// of `net` itself, which the paths in `nets` extend; `place_name` is the
/// place's.
pub(crate) fn tokens(
    net: &Net,
    path: &str,
    place: PlaceId,
    place_name: &str,
    offset: usize,
    limit: usize,
) -> Vec<protocol::Token> {
    net.tokens(place)
        .skip(offset)
        .take(limit)
        .map(|(token, _)| entry(net, path, place, place_name, token))
        .collect()
}

/// One token entry (PROTOCOL.md §5.3): its id, label, keys and nets.
pub(crate) fn entry(
    net: &Net,
    path: &str,
    place: PlaceId,
    place_name: &str,
    token: TokenId,
) -> protocol::Token {
    protocol::Token {
        id: token.to_string(),
        label: label(net, place, token),
        keys: keys(net, place, token),
        nets: nets(net, path, place, place_name, token),
    }
}

/// The `nets` of a token entry (PROTOCOL.md §12.2): each net the token
/// holds, in the token's order, with its nested id, its path as of now, its
/// template and how many tokens it holds. Empty for a token of a place that
/// is not nested.
pub(crate) fn nets(
    net: &Net,
    path: &str,
    place: PlaceId,
    place_name: &str,
    token: TokenId,
) -> Vec<protocol::InnerNet> {
    net.nested(place, token)
        .unwrap_or_default()
        .into_iter()
        .map(|(name, inner)| protocol::InnerNet {
            name: name.to_owned(),
            nested_id: inner.id().to_string(),
            path: level(path, place_name, token, name),
            template: inner.template().map(str::to_owned),
            count: inner.places().map(|(_, info)| info.count).sum(),
        })
        .collect()
}

/// `path` one level further down (PROTOCOL.md §12.1): the place, the token
/// with `#` in front, and the net's name within the token, which is empty
/// for a token that is a net, so that such a path ends in `/`.
pub(crate) fn level(path: &str, place_name: &str, token: TokenId, net: &str) -> String {
    format!("{path}/{place_name}/#{token}/{net}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_level_is_place_token_and_net_and_a_bare_net_ends_in_a_slash() {
        let token: TokenId = "17".parse().unwrap();
        assert_eq!(
            level("City", "Depot", token, "cabin"),
            "City/Depot/#17/cabin"
        );
        assert_eq!(level("City", "Yard", token, ""), "City/Yard/#17/");
        assert_eq!(
            level("City/Depot/#17/cabin", "Door", token, "luggage"),
            "City/Depot/#17/cabin/Door/#17/luggage"
        );
    }

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
