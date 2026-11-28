//! Notifications: what the runner tells of a change, unasked.

use std::collections::BTreeMap;
use std::fmt;

use petritype::KeyValue;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{PathStep, PlaceName, to_line};

/// A notification line: `{"method": ..., "params": {...}}`.
///
/// Every notification carries `marking_version` (after the change, if it made
/// one) and `running` (PROTOCOL.md §7). The state notifications are the ones
/// that raise `marking_version` by one each; see
/// [`is_state`](Notification::is_state).
///
/// A port place is in the markings of two nets, its subnet and the net
/// around it, so a state notification that changes one raises both nets'
/// versions and carries the other net's new value besides its own
/// (PROTOCOL.md §12.7): `subnet_marking_versions` when the net around the
/// subnet tells it, `outer_marking_version` when the subnet does.
/// [`across`](Notification::across) sets them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Notification {
    /// A firing reported in one piece.
    Fired {
        /// The transition that fired.
        transition: String,
        /// `[place, id]` of each token taken.
        consumed: Vec<(PlaceName, String)>,
        /// Each token produced: `[place, id, label]`, and its keys when its
        /// place declares some ([`Produced`]).
        produced: Vec<Produced>,
        /// How long the body ran.
        duration_ms: f64,
        /// See the type's documentation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subnet_marking_versions: Option<BTreeMap<String, u64>>,
        /// See the type's documentation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outer_marking_version: Option<u64>,
        /// See the type's documentation.
        marking_version: u64,
        /// See the type's documentation.
        running: bool,
    },
    /// The first half of a firing reported in two pieces: inputs taken,
    /// body started.
    Launched {
        /// The transition launched.
        transition: String,
        /// `[place, id]` of each token taken.
        consumed: Vec<(PlaceName, String)>,
        /// See the type's documentation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subnet_marking_versions: Option<BTreeMap<String, u64>>,
        /// See the type's documentation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outer_marking_version: Option<u64>,
        /// See the type's documentation.
        marking_version: u64,
        /// See the type's documentation.
        running: bool,
    },
    /// The second half: body returned, outputs placed.
    Distributed {
        /// The transition that landed.
        transition: String,
        /// Each token produced: `[place, id, label]`, and its keys when its
        /// place declares some ([`Produced`]).
        produced: Vec<Produced>,
        /// How long the body was airborne.
        duration_ms: f64,
        /// See the type's documentation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subnet_marking_versions: Option<BTreeMap<String, u64>>,
        /// See the type's documentation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outer_marking_version: Option<u64>,
        /// See the type's documentation.
        marking_version: u64,
        /// See the type's documentation.
        running: bool,
    },
    /// A body failed.
    Failed {
        /// The transition whose body failed.
        transition: String,
        /// The error's text.
        error: String,
        /// Whether the consumed tokens went back.
        restored: bool,
        /// `[place, id]` of each token the firing took.
        consumed: Vec<(PlaceName, String)>,
        /// See the type's documentation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subnet_marking_versions: Option<BTreeMap<String, u64>>,
        /// See the type's documentation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outer_marking_version: Option<u64>,
        /// See the type's documentation.
        marking_version: u64,
        /// See the type's documentation.
        running: bool,
    },
    /// A token arrived from outside; or, under the connection's filter, a
    /// token came into its view (PROTOCOL.md §13.4).
    Added {
        /// Where: the place, or the subnet place of a port (§12.7).
        place: String,
        /// The port of the subnet place `place`, for a port place.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<String>,
        /// Its id.
        id: String,
        /// Its label.
        label: String,
        /// Its colour, `#rrggbb`, when its place has a style that gives
        /// it one (§5.3).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        color: Option<String>,
        /// Its keys, when its place declares some (§13.1).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        keys: Option<Keys>,
        /// See the type's documentation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subnet_marking_versions: Option<BTreeMap<String, u64>>,
        /// See the type's documentation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outer_marking_version: Option<u64>,
        /// See the type's documentation.
        marking_version: u64,
        /// See the type's documentation.
        running: bool,
    },
    /// A token was removed from outside; or, under the connection's
    /// filter, a token went out of its view (PROTOCOL.md §13.4).
    Removed {
        /// Where from: the place, or the subnet place of a port (§12.7).
        place: String,
        /// The port of the subnet place `place`, for a port place.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<String>,
        /// Its id.
        id: String,
        /// See the type's documentation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subnet_marking_versions: Option<BTreeMap<String, u64>>,
        /// See the type's documentation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outer_marking_version: Option<u64>,
        /// See the type's documentation.
        marking_version: u64,
        /// See the type's documentation.
        running: bool,
    },
    /// A token changed in place.
    Mutated {
        /// Where: the place, or the subnet place of a port (§12.7).
        place: String,
        /// The port of the subnet place `place`, for a port place.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<String>,
        /// Its id.
        id: String,
        /// Its new label.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        /// Its keys after the change, when its place declares some (§13.1).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        keys: Option<Keys>,
        /// See the type's documentation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subnet_marking_versions: Option<BTreeMap<String, u64>>,
        /// See the type's documentation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outer_marking_version: Option<u64>,
        /// See the type's documentation.
        marking_version: u64,
        /// See the type's documentation.
        running: bool,
    },
    /// Stands in for an `added`, `removed` or `mutated` about a token the
    /// connection's filter hides (PROTOCOL.md §13.4): a state notification,
    /// so the version moves as the hidden one would have, and nothing of the
    /// token is told but where it is.
    Hidden {
        /// What happened to the hidden token.
        kind: HiddenKind,
        /// The place it happened in, or the subnet place of a port (§12.7).
        place: String,
        /// The port of the subnet place `place`, for a port place.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<String>,
        /// See the type's documentation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subnet_marking_versions: Option<BTreeMap<String, u64>>,
        /// See the type's documentation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outer_marking_version: Option<u64>,
        /// See the type's documentation.
        marking_version: u64,
        /// See the type's documentation.
        running: bool,
    },
    /// A transition's switch changed.
    Toggled {
        /// The transition.
        transition: String,
        /// Its switch now.
        enabled: bool,
        /// See the type's documentation.
        marking_version: u64,
        /// See the type's documentation.
        running: bool,
    },
    /// `set_param` took effect.
    ParamChanged {
        /// The parameter's wire name.
        param: String,
        /// See the type's documentation.
        marking_version: u64,
        /// See the type's documentation.
        running: bool,
    },
    /// The marking was replaced wholesale: refetch state.
    Reloaded {
        /// The new generation.
        generation: u64,
        /// `"load"`, `"reset"` or `"resync"`.
        reason: String,
        /// See the type's documentation.
        marking_version: u64,
        /// See the type's documentation.
        running: bool,
    },
    /// The fireable set after a batch of changes, sent only when it changed.
    Fireable {
        /// Names, in declaration order.
        transitions: Vec<String>,
        /// See the type's documentation.
        marking_version: u64,
        /// See the type's documentation.
        running: bool,
    },
    /// A `"24/7"` net found nothing fireable; once per idle spell.
    Idle {
        /// When the spell began.
        idle_since: String,
        /// See the type's documentation.
        marking_version: u64,
        /// See the type's documentation.
        running: bool,
    },
    /// The net stopped running on its own.
    ExecutionStopped {
        /// `"no_fireable_transitions"` or `"transition_failed"`.
        reason: String,
        /// `"panic"` or `"error"`, for a failure.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error_type: Option<String>,
        /// The failure's text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        /// See the type's documentation.
        marking_version: u64,
        /// See the type's documentation.
        running: bool,
    },
    /// Periodic while loaded: the body that runs and when it started, both
    /// `null` when no body runs, and the inner net it runs in when it runs
    /// in one.
    Heartbeat {
        /// When the running body started.
        step_started_at: Option<String>,
        /// The transition whose body is running, by its name in its own net.
        executing_transition: Option<String>,
        /// The path of the inner net the body runs in (PROTOCOL.md §12.1),
        /// as a `step` result names the net of its firing; absent for a body
        /// of the loaded net and when no body runs.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<Vec<PathStep>>,
        /// That inner net's nested id, present and absent with `path`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        nested_id: Option<String>,
        /// See the type's documentation.
        marking_version: u64,
        /// See the type's documentation.
        running: bool,
    },
    /// Progress of a `load`.
    LoadProgress {
        /// The stage, `"build"`.
        step: String,
        /// Free text.
        message: String,
        /// See the type's documentation.
        marking_version: u64,
        /// See the type's documentation.
        running: bool,
    },
    /// An event inside an inner net (PROTOCOL.md §12.4): the notification
    /// that net would send on its own, wrapped once whatever the depth.
    ///
    /// `event` carries the inner net's own `marking_version`; the wrapper
    /// carries the loaded net's, which an inner event moves only when it
    /// changes a port place of one of the loaded net's subnet places
    /// (§12.7), and then the event says so with `outer_marking_version`.
    /// The wrapper is not a state notification of its own.
    Nested {
        /// The inner net's full path from the loaded net, as the marking
        /// stood when the event was lifted (§12.1).
        path: Vec<PathStep>,
        /// The inner net's nested id.
        nested_id: String,
        /// The inner notification, `{method, params}`.
        event: Box<Notification>,
        /// The loaded net's version, after the event.
        marking_version: u64,
        /// See the type's documentation.
        running: bool,
    },
}

/// What a [`Notification::Hidden`] stands in for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HiddenKind {
    /// An `added`.
    Added,
    /// A `removed`.
    Removed,
    /// A `mutated`.
    Mutated,
}

/// One entry of a `produced` list (PROTOCOL.md §7.1): `[place, id, label]`,
/// `[place, id, label, color]` for a token with a colour, or, for a token
/// whose place declares keys, `[place, id, label, color, keys]`.
///
/// The fourth element is the token's colour, `#rrggbb`, which a token has
/// when its place has a style that gives it one. With keys it is written
/// even without a colour, as `null`, to keep the keys in fifth place. This
/// is the one place the protocol writes `null` for an absent optional.
///
/// The first element is a place's name, or `{"place", "port"}` for a port
/// place of one of the net's subnet places (§12.7, [`PlaceName`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Produced {
    /// The place the token went to.
    pub place: PlaceName,
    /// Its id.
    pub id: String,
    /// Its label.
    pub label: String,
    /// Its colour, when it has one.
    pub color: Option<String>,
    /// Its keys, when its place declares some.
    pub keys: Option<Keys>,
}

impl Serialize for Produced {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let len = match (&self.color, &self.keys) {
            (_, Some(_)) => 5,
            (Some(_), None) => 4,
            (None, None) => 3,
        };
        let mut seq = serializer.serialize_seq(Some(len))?;
        seq.serialize_element(&self.place)?;
        seq.serialize_element(&self.id)?;
        seq.serialize_element(&self.label)?;
        if len > 3 {
            seq.serialize_element(&self.color)?;
        }
        if let Some(keys) = &self.keys {
            seq.serialize_element(keys)?;
        }
        seq.end()
    }
}

impl<'de> Deserialize<'de> for Produced {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Entry;

        impl<'de> Visitor<'de> for Entry {
            type Value = Produced;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("[place, id, label, colour?, keys?]")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Produced, A::Error> {
                let missing =
                    |what: &str| de::Error::custom(format!("a produced entry needs its {what}"));
                let place = seq
                    .next_element::<PlaceName>()?
                    .ok_or_else(|| missing("place"))?;
                let id = seq.next_element::<String>()?.ok_or_else(|| missing("id"))?;
                let label = seq
                    .next_element::<String>()?
                    .ok_or_else(|| missing("label"))?;
                let color: Option<String> = seq.next_element::<Option<String>>()?.flatten();
                let keys: Option<Keys> = seq.next_element()?;
                if seq.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::custom(
                        "a produced entry has at most five elements",
                    ));
                }
                Ok(Produced {
                    place,
                    id,
                    label,
                    color,
                    keys,
                })
            }
        }

        deserializer.deserialize_seq(Entry)
    }
}

/// A token's keys on the wire (PROTOCOL.md §13.1): `{name: value}`, written
/// in the order its place declares them, each value a string, an integer or
/// a boolean.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Keys(pub Vec<(String, KeyValue)>);

impl Keys {
    /// The value of the key `name`, if the token has that key.
    pub fn get(&self, name: &str) -> Option<&KeyValue> {
        self.0.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }
}

impl Serialize for Keys {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (name, value) in &self.0 {
            map.serialize_entry(name, value)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Keys {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Object;

        impl<'de> Visitor<'de> for Object {
            type Value = Keys;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an object of strings, integers and booleans")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Keys, A::Error> {
                let mut keys = Vec::new();
                while let Some(entry) = map.next_entry::<String, KeyValue>()? {
                    keys.push(entry);
                }
                Ok(Keys(keys))
            }
        }

        deserializer.deserialize_map(Object)
    }
}

impl Notification {
    /// Whether this is a state notification (PROTOCOL.md §7.1), the kind that
    /// raises `marking_version` by exactly one.
    pub fn is_state(&self) -> bool {
        matches!(
            self,
            Notification::Fired { .. }
                | Notification::Launched { .. }
                | Notification::Distributed { .. }
                | Notification::Failed { .. }
                | Notification::Added { .. }
                | Notification::Removed { .. }
                | Notification::Mutated { .. }
                | Notification::Hidden { .. }
                | Notification::Toggled { .. }
                | Notification::ParamChanged { .. }
        )
    }

    /// The notification as one line of JSON, without the newline.
    ///
    /// # Errors
    ///
    /// What `serde_json` says when the notification cannot be written
    /// (since 2026-11-28; until then this panicked). The runner leaves such
    /// a line out and says so on stderr, as the Python runner does.
    pub fn to_line(&self) -> Result<String, serde_json::Error> {
        to_line(self)
    }

    /// The notification's `method`, as on the wire: `"fired"`, `"added"`
    /// and so on.
    pub fn method(&self) -> &'static str {
        match self {
            Notification::Fired { .. } => "fired",
            Notification::Launched { .. } => "launched",
            Notification::Distributed { .. } => "distributed",
            Notification::Failed { .. } => "failed",
            Notification::Added { .. } => "added",
            Notification::Removed { .. } => "removed",
            Notification::Mutated { .. } => "mutated",
            Notification::Hidden { .. } => "hidden",
            Notification::Toggled { .. } => "toggled",
            Notification::ParamChanged { .. } => "param_changed",
            Notification::Reloaded { .. } => "reloaded",
            Notification::Fireable { .. } => "fireable",
            Notification::Idle { .. } => "idle",
            Notification::ExecutionStopped { .. } => "execution_stopped",
            Notification::Heartbeat { .. } => "heartbeat",
            Notification::LoadProgress { .. } => "load_progress",
            Notification::Nested { .. } => "nested",
        }
    }

    /// Sets the versions of the nets on the far side of the port places
    /// this notification changed (PROTOCOL.md §12.7): `subnets`, each
    /// subnet place's subnet's new version, when the net around them tells
    /// it, and `outer`, the new version of the net around this one, when
    /// this net is a subnet and tells it. A notification that is not a
    /// state notification about tokens carries neither, and is left as it
    /// is.
    pub fn across(&mut self, subnets: Option<BTreeMap<String, u64>>, outer: Option<u64>) {
        match self {
            Notification::Fired {
                subnet_marking_versions,
                outer_marking_version,
                ..
            }
            | Notification::Launched {
                subnet_marking_versions,
                outer_marking_version,
                ..
            }
            | Notification::Distributed {
                subnet_marking_versions,
                outer_marking_version,
                ..
            }
            | Notification::Failed {
                subnet_marking_versions,
                outer_marking_version,
                ..
            }
            | Notification::Added {
                subnet_marking_versions,
                outer_marking_version,
                ..
            }
            | Notification::Removed {
                subnet_marking_versions,
                outer_marking_version,
                ..
            }
            | Notification::Mutated {
                subnet_marking_versions,
                outer_marking_version,
                ..
            }
            | Notification::Hidden {
                subnet_marking_versions,
                outer_marking_version,
                ..
            } => {
                *subnet_marking_versions = subnets;
                *outer_marking_version = outer;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    use crate::protocol::address::cabin;
    use crate::protocol::{Token, to_value};

    #[test]
    fn a_nested_notification_wraps_the_inner_one_and_moves_no_outer_version() {
        let n = Notification::Nested {
            path: vec![cabin()],
            nested_id: "3".into(),
            event: Box::new(Notification::Added {
                place: "Door".into(),
                port: None,
                id: "1".into(),
                label: "\"Ada\"".into(),
                color: None,
                keys: None,
                subnet_marking_versions: None,
                outer_marking_version: None,
                marking_version: 4,
                running: false,
            }),
            marking_version: 42,
            running: false,
        };
        assert!(!n.is_state(), "the outer version does not move");
        assert_eq!(
            to_value(&n),
            json!({"method": "nested", "params": {
                "path": [{"place": "Depot", "token": "17", "net": "cabin"}], "nested_id": "3",
                "event": {"method": "added", "params": {
                    "place": "Door", "id": "1", "label": "\"Ada\"",
                    "marking_version": 4, "running": false}},
                "marking_version": 42, "running": false}})
        );
        let back: Notification = serde_json::from_value(to_value(&n)).unwrap();
        assert_eq!(back, n);
    }

    fn bird() -> Keys {
        Keys(vec![
            ("species".into(), KeyValue::from("Robin")),
            ("legs".into(), KeyValue::Int(2)),
            ("flies".into(), KeyValue::Bool(true)),
        ])
    }

    #[test]
    fn keys_are_an_object_in_declaration_order() {
        let keys = bird();
        assert_eq!(
            serde_json::to_string(&keys).unwrap(),
            r#"{"species":"Robin","legs":2,"flies":true}"#,
            "declaration order, not sorted"
        );
        let back: Keys =
            serde_json::from_str(r#"{"species":"Robin","legs":2,"flies":true}"#).unwrap();
        assert_eq!(back, keys);
        assert_eq!(keys.get("legs"), Some(&KeyValue::Int(2)));
        assert!(serde_json::from_value::<Keys>(json!({"weight": 1.5})).is_err());
        let token = Token {
            id: "1".into(),
            label: "Robin".into(),
            color: None,
            keys: Some(keys),
            nets: Vec::new(),
        };
        assert_eq!(
            to_value(&token),
            json!({"id": "1", "label": "Robin", "keys": {"species": "Robin", "legs": 2, "flies": true}})
        );
    }

    #[test]
    fn a_produced_entry_has_three_elements_four_with_a_colour_five_with_keys() {
        let plain = Produced {
            place: PlaceName::Place("Out".into()),
            id: "4".into(),
            label: "4".into(),
            color: None,
            keys: None,
        };
        assert_eq!(to_value(&plain), json!(["Out", "4", "4"]));
        let coloured = Produced {
            color: Some("#d94f4f".into()),
            ..plain.clone()
        };
        assert_eq!(to_value(&coloured), json!(["Out", "4", "4", "#d94f4f"]));
        let keyed = Produced {
            keys: Some(bird()),
            ..plain.clone()
        };
        assert_eq!(
            to_value(&keyed),
            json!(["Out", "4", "4", null, {"species": "Robin", "legs": 2, "flies": true}]),
            "a token with keys and no colour has null fourth, so the keys stay fifth"
        );
        let both = Produced {
            color: Some("#d94f4f".into()),
            ..keyed.clone()
        };
        assert_eq!(
            to_value(&both),
            json!(["Out", "4", "4", "#d94f4f", {"species": "Robin", "legs": 2, "flies": true}])
        );
        // Read from text, as a line arrives: a `json!` value sorts its keys.
        for (line, entry) in [
            (r#"["Out","4","4"]"#, &plain),
            (
                r#"["Out","4","4",null,{"species":"Robin","legs":2,"flies":true}]"#,
                &keyed,
            ),
            (r##"["Out","4","4","#d94f4f"]"##, &coloured),
            (
                r##"["Out","4","4","#d94f4f",{"species":"Robin","legs":2,"flies":true}]"##,
                &both,
            ),
        ] {
            let back: Produced = serde_json::from_str(line).unwrap();
            assert_eq!(&back, entry, "{line}");
        }
        assert!(serde_json::from_value::<Produced>(json!(["Out", "4"])).is_err());
        assert!(serde_json::from_value::<Produced>(json!(["Out", "4", "4", null, {}, 6])).is_err());
    }

    #[test]
    fn a_colour_is_written_where_a_token_has_one() {
        let token = Token {
            id: "1".into(),
            label: "Red".into(),
            color: Some("#ff6347".into()),
            keys: None,
            nets: Vec::new(),
        };
        assert_eq!(
            to_value(&token),
            json!({"id": "1", "label": "Red", "color": "#ff6347"})
        );
        let added = Notification::Added {
            place: "Light".into(),
            port: None,
            id: "1".into(),
            label: "Red".into(),
            color: Some("#ff6347".into()),
            keys: None,
            subnet_marking_versions: None,
            outer_marking_version: None,
            marking_version: 2,
            running: false,
        };
        assert_eq!(
            to_value(&added),
            json!({"method": "added", "params": {"place": "Light", "id": "1", "label": "Red",
                   "color": "#ff6347", "marking_version": 2, "running": false}})
        );
        let back: Notification = serde_json::from_value(to_value(&added)).unwrap();
        assert_eq!(back, added);
    }

    #[test]
    fn hidden_is_a_state_notification_naming_only_the_place() {
        let n = Notification::Hidden {
            kind: HiddenKind::Mutated,
            place: "Savanna".into(),
            port: None,
            subnet_marking_versions: None,
            outer_marking_version: None,
            marking_version: 9,
            running: false,
        };
        assert!(n.is_state(), "it moves the version as the hidden one would");
        assert_eq!(
            to_value(&n),
            json!({"method": "hidden", "params": {"kind": "mutated", "place": "Savanna",
                   "marking_version": 9, "running": false}})
        );
    }

    #[test]
    fn a_change_of_a_port_carries_the_version_across_it() {
        let mut n = Notification::Removed {
            place: "Level 3".into(),
            port: Some("Lobby".into()),
            id: "4".into(),
            subnet_marking_versions: None,
            outer_marking_version: None,
            marking_version: 43,
            running: false,
        };
        n.across(Some(BTreeMap::from([("Level 3".to_owned(), 8)])), None);
        assert_eq!(
            to_value(&n),
            json!({"method": "removed", "params": {"place": "Level 3", "port": "Lobby", "id": "4",
                   "subnet_marking_versions": {"Level 3": 8},
                   "marking_version": 43, "running": false}})
        );
        let back: Notification = serde_json::from_value(to_value(&n)).unwrap();
        assert_eq!(back, n);
        let mut fireable = Notification::Fireable {
            transitions: Vec::new(),
            marking_version: 1,
            running: false,
        };
        let before = fireable.clone();
        fireable.across(None, Some(9));
        assert_eq!(fireable, before, "not a change of tokens");
    }
}
