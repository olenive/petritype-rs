//! The answer to `get_state`: the net's shape, its transitions, its
//! marking and its progress.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{Keys, PathStep, Topology};

/// The answer to `get_state` (PROTOCOL.md §5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// Always 2 here ([`PROTOCOL_VERSION`](super::PROTOCOL_VERSION)).
    pub protocol_version: u32,
    /// `"rust"`.
    pub runtime: String,
    /// The loaded net's registered name; for an inner net, its name within
    /// the token that holds it (`""` for a bare net), and for a subnet, the
    /// name of its subnet place.
    pub net: String,
    /// See PROTOCOL.md §4; always 1 for an inner net.
    pub generation: u64,
    /// See PROTOCOL.md §4.
    pub marking_version: u64,
    /// See PROTOCOL.md §4.
    pub running: bool,
    /// The shape, by names.
    pub topology: Topology,
    /// Each transition's state, by name.
    pub transitions: BTreeMap<String, TransitionState>,
    /// The tokens.
    pub marking: Marking,
    /// Firing statistics kept by the runner.
    pub progress: Progress,
    /// The inner net's path, for the state of an inner net (§12.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<Vec<PathStep>>,
    /// The inner net's nested id, for the state of an inner net.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nested_id: Option<String>,
    /// The template that built the inner net, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
}

/// One transition's state (PROTOCOL.md §5.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransitionState {
    /// The on/off switch.
    pub enabled: bool,
    /// Whether it could fire now.
    pub fireable: bool,
    /// Whether its body is running.
    pub in_flight: bool,
    /// The default selector's score; omitted when computing it panicked,
    /// and since 2026-11-28 when it is `NaN`, which is an error, or an
    /// infinity, which JSON has no number for (until then a `NaN` was sent
    /// as `0.0` and an infinity as `null`), as the Python runner does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<f64>,
}

/// The tokens of every place (PROTOCOL.md §5.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Marking {
    /// By place name.
    pub places: BTreeMap<String, PlaceEntry>,
}

/// One place's entry in a [`Marking`]: its tokens, or, for a subnet place,
/// which holds none of its own, its subnet and its ports (PROTOCOL.md
/// §12.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PlaceEntry {
    /// A place's tokens.
    Tokens(PlaceMarking),
    /// A subnet place's `{subnet, ports}`.
    Subnet(SubnetMarking),
}

/// A subnet place's entry in a [`Marking`] (PROTOCOL.md §12.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubnetMarking {
    /// Its subnet, as a token's `nets[]` entry names a net, without `name`.
    pub subnet: SubnetEntry,
    /// Each port's tokens, by the port's name, as a place gives them.
    pub ports: BTreeMap<String, PlaceMarking>,
}

/// The subnet of a subnet place, as its [`SubnetMarking`] names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubnetEntry {
    /// Its nested id: the same from `load` or `reset` until the next.
    pub nested_id: String,
    /// Its path: the path of the net that holds the subnet place, and the
    /// step `{"place"}`.
    pub path: Vec<PathStep>,
    /// The template that built it, when one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// How many tokens its own places hold, its ports included; those of a
    /// subnet of its own are counted in that subnet's entry. A hint.
    pub count: usize,
}

/// One place's tokens: the true count and the oldest ones, capped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaceMarking {
    /// How many tokens the place holds.
    pub count: usize,
    /// How many of them the connection's filter lets through, while one is
    /// set (PROTOCOL.md §13.4); `tokens` then holds only those.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matching: Option<usize>,
    /// The oldest, up to the limit.
    pub tokens: Vec<Token>,
}

/// A token on the wire: its id, a label of at most 1000 characters, the
/// `"..."` that ends a label that was cut counted among them (PROTOCOL.md
/// §5.3), its colour if it has one, its keys if its place declares some,
/// and the nets it holds, if any.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Token {
    /// The decimal id.
    pub id: String,
    /// The token's `Debug`, cut to 1000 characters with `"..."` as the
    /// last three when it is longer.
    pub label: String,
    /// Its colour, `#rrggbb`, when its place has a style that gives it one
    /// (§5.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Its keys, when its place declares some (§13.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keys: Option<Keys>,
    /// The nets the token holds (§12.2), in the token's own order; omitted
    /// for a token that holds none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nets: Vec<InnerNet>,
}

/// One net a token holds, as a token entry lists it (`nets[]`, §12.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InnerNet {
    /// The net's name within the token; `""` for a token that is a net.
    pub name: String,
    /// Its nested id: stable while the token moves.
    pub nested_id: String,
    /// Its path as the marking stands now: the path of the net the token is
    /// in, and one step more.
    pub path: Vec<PathStep>,
    /// The template that built it, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// How many tokens it holds, over all its places: a hint for a badge,
    /// read with the outer state.
    pub count: usize,
}

/// Firing statistics (PROTOCOL.md §5.4).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    /// Completed firings since load or reset.
    pub step_count: u64,
    /// The last transition to complete a firing, or `null`.
    pub last_fired: Option<String>,
    /// Completed firings per transition name.
    pub fired_counts: BTreeMap<String, u64>,
    /// When the last firing completed, or `null`.
    pub last_progress_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    use crate::protocol::address::cabin;
    use crate::protocol::{Place, Stepping, to_value};

    #[test]
    fn the_nested_fields_are_omitted_where_nothing_is_nested() {
        let token = Token {
            id: "1".into(),
            label: "x".into(),
            color: None,
            keys: None,
            nets: Vec::new(),
        };
        assert_eq!(to_value(&token), json!({"id": "1", "label": "x"}));
        let place = Place {
            name: "P".into(),
            type_name: "u32".into(),
            serializable: Some(false),
            nested: false,
            subnet: false,
            ports: None,
            port: false,
            stepping: None,
            key_names: Vec::new(),
            style: None,
            group: None,
        };
        assert_eq!(
            to_value(&place),
            json!({"name": "P", "type_name": "u32", "serializable": false})
        );
        for (stepping, name) in [
            (Stepping::InnerFirst, "inner_first"),
            (Stepping::TakeTurns, "take_turns"),
            (Stepping::OuterFirst, "outer_first"),
            (Stepping::Passive, "passive"),
        ] {
            let nested = Place {
                nested: true,
                stepping: Some(stepping),
                ..place.clone()
            };
            assert_eq!(
                to_value(&nested),
                json!({"name": "P", "type_name": "u32", "serializable": false,
                       "nested": true, "stepping": name})
            );
            let back: Place = serde_json::from_value(to_value(&nested)).unwrap();
            assert_eq!(back, nested);
        }
        let held = Token {
            nets: vec![InnerNet {
                name: "cabin".into(),
                nested_id: "3".into(),
                path: vec![cabin()],
                template: None,
                count: 0,
            }],
            ..token
        };
        assert_eq!(
            to_value(&held)["nets"],
            json!([{"name": "cabin", "nested_id": "3",
                    "path": [{"place": "Depot", "token": "17", "net": "cabin"}], "count": 0}])
        );
    }

    #[test]
    fn a_subnet_place_is_written_with_its_ports_and_its_entry_with_its_subnet() {
        let lobby = Place {
            name: "Lobby".into(),
            type_name: "Hero".into(),
            serializable: Some(true),
            nested: false,
            subnet: false,
            ports: None,
            port: false,
            stepping: None,
            key_names: Vec::new(),
            style: None,
            group: None,
        };
        let level = Place {
            name: "Level 3".into(),
            type_name: "Level".into(),
            serializable: None,
            subnet: true,
            ports: Some(vec![lobby.clone()]),
            stepping: Some(Stepping::InnerFirst),
            ..lobby.clone()
        };
        assert_eq!(
            to_value(&level),
            json!({"name": "Level 3", "type_name": "Level", "subnet": true,
                   "ports": [{"name": "Lobby", "type_name": "Hero", "serializable": true}],
                   "stepping": "inner_first"})
        );
        let inside = Place {
            port: true,
            ..lobby
        };
        assert_eq!(to_value(&inside)["port"], json!(true));
        let entry = PlaceEntry::Subnet(SubnetMarking {
            subnet: SubnetEntry {
                nested_id: "5".into(),
                path: vec![PathStep::Subnet {
                    place: "Level 3".into(),
                }],
                template: Some("Level".into()),
                count: 2,
            },
            ports: BTreeMap::from([(
                "Lobby".to_owned(),
                PlaceMarking {
                    count: 1,
                    matching: None,
                    tokens: Vec::new(),
                },
            )]),
        });
        let value = to_value(&entry);
        assert_eq!(
            value,
            json!({"subnet": {"nested_id": "5", "path": [{"place": "Level 3"}], "template": "Level",
                              "count": 2},
                   "ports": {"Lobby": {"count": 1, "tokens": []}}})
        );
        let back: PlaceEntry = serde_json::from_value(value).unwrap();
        assert_eq!(back, entry);
        let place: PlaceEntry = serde_json::from_value(json!({"count": 0, "tokens": []})).unwrap();
        assert!(matches!(place, PlaceEntry::Tokens(_)));
    }

    #[test]
    fn under_a_filter_a_place_says_how_many_match() {
        let place = PlaceMarking {
            count: 2,
            matching: None,
            tokens: Vec::new(),
        };
        assert_eq!(to_value(&place), json!({"count": 2, "tokens": []}));
        let place = PlaceMarking {
            matching: Some(1),
            ..place
        };
        assert_eq!(
            to_value(&place),
            json!({"count": 2, "matching": 1, "tokens": []})
        );
    }
}
