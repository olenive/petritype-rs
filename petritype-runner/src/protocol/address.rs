//! Addresses: the path to an inner net, and a place named by its name or
//! by the pair of a subnet place and a port.

use serde::de;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// One step of a `path` (PROTOCOL.md §12.1): one level of nesting, naming a
/// place of the net the steps before it reach.
///
/// On the wire a step is an object: `{"place", "token", "net"}` for a net
/// carried by a token, `{"place"}` alone for a subnet. Names are compared
/// exactly as given, whatever characters they hold.
///
/// Read off a request with [`parse_path`], which says which step was wrong;
/// `Deserialize` reads one step by the same rules, [`PathStep::parse`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(untagged)]
pub enum PathStep {
    /// A net carried by a token: `{"place": "Depot", "token": "17", "net":
    /// "cabin"}`.
    Token {
        /// The place that holds the token.
        place: String,
        /// The token's id, a string (PROTOCOL.md §2).
        token: String,
        /// The net's name within the token; `""` for a token that is itself
        /// a net.
        net: String,
    },
    /// A subnet: `{"place": "Level 3"}`, its subnet place.
    Subnet {
        /// The subnet place.
        place: String,
    },
}

impl PathStep {
    /// The place this step names, in the net the steps before it reach.
    pub fn place(&self) -> &str {
        match self {
            PathStep::Token { place, .. } | PathStep::Subnet { place } => place,
        }
    }

    /// Reads one step (PROTOCOL.md §12.1): an object with a string `place`
    /// and either both of `token` and `net`, each a string, or neither. A
    /// `null` is a value of the wrong type, not an absent field. Other keys
    /// are ignored (§2).
    ///
    /// # Errors
    ///
    /// What is wrong with the step, for an `invalid_params` answer.
    pub fn parse(value: &Value) -> Result<PathStep, String> {
        let Value::Object(step) = value else {
            return Err(format!("is an object, not {}", kind(value)));
        };
        let text = |key: &str| match step.get(key) {
            None => Ok(None),
            Some(Value::String(text)) => Ok(Some(text.clone())),
            Some(other) => Err(format!(
                "has a {key:?} that is {}, not a string",
                kind(other)
            )),
        };
        let place = text("place")?.ok_or("has no \"place\"")?;
        match (text("token")?, text("net")?) {
            (Some(token), Some(net)) => Ok(PathStep::Token { place, token, net }),
            (None, None) => Ok(PathStep::Subnet { place }),
            (Some(_), None) => {
                Err("has \"token\" without \"net\": a token's net is named by both".into())
            }
            (None, Some(_)) => {
                Err("has \"net\" without \"token\": a token's net is named by both".into())
            }
        }
    }
}

impl<'de> Deserialize<'de> for PathStep {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        PathStep::parse(&value).map_err(|why| de::Error::custom(format!("a path step {why}")))
    }
}

/// Reads a request's `path` (PROTOCOL.md §12.1): a list of steps, each read
/// by [`PathStep::parse`]. The empty list is the loaded net. Whether the
/// steps name a live net is the runner's question, asked after this one.
///
/// # Errors
///
/// What is wrong with the shape, naming the step by its index from 0, for
/// an `invalid_params` answer.
pub fn parse_path(value: &Value) -> Result<Vec<PathStep>, String> {
    let Value::Array(steps) = value else {
        return Err(format!(
            "\"path\" is a list of steps, one per level from the loaded net down, not {}",
            kind(value)
        ));
    };
    steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            PathStep::parse(step).map_err(|why| format!("step {index} of the path {why}"))
        })
        .collect()
}

/// A place as an object or a position names it (PROTOCOL.md §7.1, §8,
/// §12.7, §13.2): a place of the net's own by its name, or a port place of
/// one of the net's subnet places by the pair of the subnet place and the
/// port, never by a combined name.
///
/// On the wire a place of the net's own is its name, `"Door"`, and a port is
/// the object `{"place": "Level 3", "port": "Lobby"}`: in the first element
/// of a `consumed` or `produced` entry, of a log entry's pairs, and of a
/// filter's `places`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PlaceName {
    /// A place of the net's own, by its name.
    Place(String),
    /// A port place of one of the net's subnet places.
    Port {
        /// The subnet place.
        place: String,
        /// The port, by its own name in the subnet.
        port: String,
    },
}

impl PlaceName {
    /// The place's `place` field and its `port` field, as an object that
    /// names a place writes them (`added`, `removed`, `mutated`, `hidden`,
    /// an edge, a page of tokens, a query's match): the subnet place and the
    /// port for a port, the name and no port for a place of the net's own.
    pub fn split(self) -> (String, Option<String>) {
        match self {
            PlaceName::Place(name) => (name, None),
            PlaceName::Port { place, port } => (place, Some(port)),
        }
    }

    /// Reads one entry of a filter's or a `distinct`'s `places` (PROTOCOL.md
    /// §13.2): a place name, or an object with a string `place` and a string
    /// `port`. Other keys of the object are ignored (§2).
    ///
    /// # Errors
    ///
    /// What is wrong with the entry, for an `invalid_params` answer.
    pub fn parse(value: &Value) -> Result<PlaceName, String> {
        const SHAPE: &str =
            "a place is named by its name or, for a port, by {\"place\", \"port\"}, both strings";
        match value {
            Value::String(name) => Ok(PlaceName::Place(name.clone())),
            Value::Object(object) => match (object.get("place"), object.get("port")) {
                (Some(Value::String(place)), Some(Value::String(port))) => Ok(PlaceName::Port {
                    place: place.clone(),
                    port: port.clone(),
                }),
                _ => Err(SHAPE.into()),
            },
            _ => Err(SHAPE.into()),
        }
    }
}

/// What kind of JSON value `value` is, for a message.
fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

/// The step into the net `cabin` of the token 17 in `Depot`, for the tests
/// of the wire shapes that hold a path.
#[cfg(test)]
pub(super) fn cabin() -> PathStep {
    PathStep::Token {
        place: "Depot".into(),
        token: "17".into(),
        net: "cabin".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    use crate::protocol::{Filter, Produced, to_value};

    #[test]
    fn a_path_is_a_list_of_steps_of_two_forms() {
        let path = json!([
            {"place": "Depot", "token": "17", "net": "cabin"},
            {"place": "Level 3"},
            {"place": "Yard/North", "token": "4", "net": "", "colour": "ignored"},
        ]);
        let steps = parse_path(&path).unwrap();
        assert_eq!(
            steps,
            [
                cabin(),
                PathStep::Subnet {
                    place: "Level 3".into()
                },
                PathStep::Token {
                    place: "Yard/North".into(),
                    token: "4".into(),
                    net: String::new()
                },
            ],
            "a name may hold any character, and another key in a step is ignored"
        );
        assert_eq!(steps[1].place(), "Level 3");
        assert_eq!(
            to_value(&steps),
            json!([{"place": "Depot", "token": "17", "net": "cabin"}, {"place": "Level 3"},
                   {"place": "Yard/North", "token": "4", "net": ""}])
        );
        let back: Vec<PathStep> = serde_json::from_value(to_value(&steps)).unwrap();
        assert_eq!(back, steps);
        assert_eq!(parse_path(&json!([])).unwrap(), [], "the loaded net");
    }

    #[test]
    fn a_path_of_the_wrong_shape_says_which_step_is_wrong() {
        for (path, says) in [
            (json!("transit/Depot/#17/cabin"), "is a list of steps"),
            (json!({"place": "Depot"}), "not an object"),
            (json!(null), "not null"),
            (
                json!(["Depot"]),
                "step 0 of the path is an object, not a string",
            ),
            (
                json!([{"token": "1", "net": ""}]),
                "step 0 of the path has no \"place\"",
            ),
            (
                json!([{"place": "Depot", "token": "1", "net": "cabin"}, {"place": "Door", "token": "3"}]),
                "step 1 of the path has \"token\" without \"net\"",
            ),
            (
                json!([{"place": "Depot", "net": "cabin"}]),
                "has \"net\" without \"token\"",
            ),
            (
                json!([{"place": "Depot", "token": null, "net": "cabin"}]),
                "has a \"token\" that is null",
            ),
            (
                json!([{"place": "Depot", "token": 17, "net": "cabin"}]),
                "has a \"token\" that is a number",
            ),
            (json!([{"place": null}]), "has a \"place\" that is null"),
        ] {
            let error = parse_path(&path).unwrap_err();
            assert!(error.contains(says), "{path}: {error}");
        }
        assert!(
            serde_json::from_value::<PathStep>(json!({"place": "Depot", "token": "1"})).is_err()
        );
    }

    #[test]
    fn a_port_is_named_by_the_pair_and_a_place_of_the_net_s_own_by_its_name() {
        let lobby = PlaceName::Port {
            place: "Level 3".into(),
            port: "Lobby".into(),
        };
        assert_eq!(
            to_value(&lobby),
            json!({"place": "Level 3", "port": "Lobby"})
        );
        assert_eq!(to_value(PlaceName::Place("Door".into())), json!("Door"));
        let entry = Produced {
            place: lobby.clone(),
            id: "4".into(),
            label: "Ada".into(),
            color: None,
            keys: None,
        };
        assert_eq!(
            to_value(&entry),
            json!([{"place": "Level 3", "port": "Lobby"}, "4", "Ada"])
        );
        let back: Produced = serde_json::from_value(to_value(&entry)).unwrap();
        assert_eq!(back, entry);
        assert_eq!(
            lobby.clone().split(),
            ("Level 3".to_owned(), Some("Lobby".to_owned()))
        );
        for (value, parsed) in [
            (json!("Door"), Some(PlaceName::Place("Door".into()))),
            (
                json!({"place": "Level 3", "port": "Lobby", "note": "ignored"}),
                Some(lobby),
            ),
            (json!({"place": "Level 3"}), None),
            (json!({"place": "Level 3", "port": 7}), None),
            (json!(3), None),
        ] {
            assert_eq!(PlaceName::parse(&value).ok(), parsed, "{value}");
        }
        assert!(Filter::parse(&json!({"places": [{"place": "Level 3"}]})).is_err());
    }
}
