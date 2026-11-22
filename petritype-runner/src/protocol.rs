//! The wire shapes of the net protocol, as serde types.
//!
//! Everything here is keyed by names, with token ids as strings, exactly as
//! [PROTOCOL.md](https://github.com/olenive/petritype/blob/main/PROTOCOL.md)
//! writes it. Nothing in this module knows about a [`Net`](petritype::Net):
//! turning the core's ids and events into these shapes is the owner's job,
//! and keeping the shapes out of the core is deliberate (DESIGN §19.2).
//!
//! Optional fields are omitted, never `null`, unless the protocol says
//! otherwise (`progress.last_fired`, `progress.last_progress_at` and the
//! heartbeat's `step_started_at` and `executing_transition` are `null` by
//! design, and so is the colour of a `produced` entry that carries keys,
//! [`Produced`]).
//!
//! A key's value is the core's [`KeyValue`]: a string, an integer or a
//! boolean, written as the bare JSON value (PROTOCOL.md §13.1).
//!
//! The address of an inner net, its `path`, is a list of [`PathStep`]s, one
//! per level from the loaded net down (PROTOCOL.md §12.1), never text: no
//! character in a name has a special meaning.

use std::collections::BTreeMap;
use std::fmt;

use petritype::KeyValue;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

/// The protocol version this crate speaks. A reader of version 2 does not
/// read version 1 (PROTOCOL.md §10).
pub const PROTOCOL_VERSION: u32 = 2;

/// What every Rust runner reports as its `runtime`.
pub const RUNTIME: &str = "rust";

// ----- framing ---------------------------------------------------------------

/// One request line: `{"id": string, "method": string, "params": object}`.
///
/// `params` is kept as the raw value: a missing or `null` one reads as `{}`,
/// and a value that is not an object is the method's to reject with
/// `invalid_params`, so that the error carries the request's id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// Chosen by the host, echoed in the response.
    pub id: String,
    /// The method name, for example `"get_state"`.
    pub method: String,
    /// The method's parameters; `{}` when the line had none.
    #[serde(default = "empty_object")]
    pub params: Value,
}

fn empty_object() -> Value {
    Value::Object(Map::new())
}

/// A line that could not be read as a request: the `id` if one could be read
/// (else `""`) and what was wrong, for an `invalid_params` response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Malformed {
    /// The line's `id`, or `""` when it had none that was a string.
    pub id: String,
    /// What was wrong with the line.
    pub message: String,
}

impl Request {
    /// Parses one line of stdin.
    ///
    /// # Errors
    ///
    /// [`Malformed`] for a line that is not a JSON object, or whose `id` or
    /// `method` is missing or not a string.
    pub fn parse(line: &str) -> Result<Request, Malformed> {
        let value: Value = serde_json::from_str(line).map_err(|error| Malformed {
            id: String::new(),
            message: format!("not a JSON line: {error}"),
        })?;
        let Value::Object(mut object) = value else {
            return Err(Malformed {
                id: String::new(),
                message: "a request must be a JSON object".into(),
            });
        };
        let id = match object.remove("id") {
            Some(Value::String(id)) => id,
            _ => {
                return Err(Malformed {
                    id: String::new(),
                    message: "a request needs a string \"id\"".into(),
                });
            }
        };
        let method = match object.remove("method") {
            Some(Value::String(method)) => method,
            _ => {
                return Err(Malformed {
                    id,
                    message: "a request needs a string \"method\"".into(),
                });
            }
        };
        let params = match object.remove("params") {
            None | Some(Value::Null) => empty_object(),
            Some(params) => params,
        };
        Ok(Request { id, method, params })
    }
}

/// A response line: `{"id", "result"}` or `{"id", "error"}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    /// The request's id.
    pub id: String,
    /// The answer.
    #[serde(flatten)]
    pub outcome: Outcome,
}

/// The two kinds of answer a request gets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The method's result object.
    Result(Value),
    /// Why it failed.
    Error(ErrorBody),
}

/// `{"message", "code"}` of an error response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    /// Free text for a human.
    pub message: String,
    /// One of the closed set, for a program.
    pub code: ErrorCode,
}

/// The closed set of error codes (PROTOCOL.md §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// No such method.
    UnknownMethod,
    /// Params missing or of the wrong shape.
    InvalidParams,
    /// The method needs a loaded net.
    NotLoaded,
    /// `load` named a net this process does not carry.
    UnknownNet,
    /// The factory failed, panicked, or the net did not build.
    LoadFailed,
    /// A place name that does not exist.
    UnknownPlace,
    /// A transition name that does not exist.
    UnknownTransition,
    /// A token id that does not exist now.
    UnknownToken,
    /// The place has no JSON form.
    NotSerializable,
    /// The JSON did not decode to the place's type.
    InvalidToken,
    /// This runtime cannot do this.
    Unsupported,
    /// `press` named a control the net does not declare.
    UnknownControl,
    /// Anything else.
    Internal,
}

impl ErrorCode {
    /// The code as it appears on the wire, for example `"unknown_place"`.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::UnknownMethod => "unknown_method",
            ErrorCode::InvalidParams => "invalid_params",
            ErrorCode::NotLoaded => "not_loaded",
            ErrorCode::UnknownNet => "unknown_net",
            ErrorCode::LoadFailed => "load_failed",
            ErrorCode::UnknownPlace => "unknown_place",
            ErrorCode::UnknownTransition => "unknown_transition",
            ErrorCode::UnknownToken => "unknown_token",
            ErrorCode::NotSerializable => "not_serializable",
            ErrorCode::InvalidToken => "invalid_token",
            ErrorCode::Unsupported => "unsupported",
            ErrorCode::UnknownControl => "unknown_control",
            ErrorCode::Internal => "internal",
        }
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Response {
    /// A success line.
    pub fn result(id: impl Into<String>, result: impl Serialize) -> Response {
        Response {
            id: id.into(),
            outcome: Outcome::Result(to_value(result)),
        }
    }

    /// An error line.
    pub fn error(id: impl Into<String>, code: ErrorCode, message: impl Into<String>) -> Response {
        Response {
            id: id.into(),
            outcome: Outcome::Error(ErrorBody {
                message: message.into(),
                code,
            }),
        }
    }

    /// The response as one line of JSON, without the newline.
    pub fn to_line(&self) -> String {
        to_line(self)
    }
}

/// Serialises a protocol value. Every type in this module serialises
/// infallibly (string keys, no non-finite floats leave the runner), so a
/// failure is a bug and is reported as one rather than threaded through.
pub(crate) fn to_value(value: impl Serialize) -> Value {
    serde_json::to_value(value).expect("protocol shapes always serialise")
}

pub(crate) fn to_line(value: impl Serialize) -> String {
    serde_json::to_string(&value).expect("protocol shapes always serialise")
}

// ----- addresses -------------------------------------------------------------

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

// ----- notifications ---------------------------------------------------------

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
    pub fn to_line(&self) -> String {
        to_line(self)
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

// ----- describe --------------------------------------------------------------

/// The answer to `describe` (PROTOCOL.md §6.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Describe {
    /// Always 2 here ([`PROTOCOL_VERSION`]).
    pub protocol_version: u32,
    /// `"rust"`.
    pub runtime: String,
    /// The runner crate's version.
    pub runtime_version: String,
    /// The nets this binary carries.
    pub nets: Vec<NetDescription>,
}

/// One net in [`Describe`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetDescription {
    /// What `load` names it by.
    pub name: String,
    /// What it is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// `"manual"`, `"24/7"`, `"batch"` or `"cron"`.
    pub mode: String,
    /// The cron expression, for `"cron"` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<String>,
    /// The load parameters it takes.
    pub params: Vec<ParamDescription>,
}

/// One load parameter in [`NetDescription`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParamDescription {
    /// The key in `load {params}`.
    pub name: String,
    /// For display, for example `"u32"`.
    pub type_name: String,
    /// Whether `load` must supply it.
    pub required: bool,
    /// Used when `load` does not supply it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
}

// ----- state -----------------------------------------------------------------

/// The answer to `get_state` (PROTOCOL.md §5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// Always 2 here ([`PROTOCOL_VERSION`]).
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

/// The net's shape keyed by names (PROTOCOL.md §5.1). No `hints`: nothing in
/// the core produces positions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Topology {
    /// Every place, in id order.
    pub places: Vec<Place>,
    /// Every transition, in declaration order.
    pub transitions: Vec<Transition>,
    /// Every edge, transition by transition, inputs then outputs.
    pub edges: Vec<Edge>,
    /// The net's own groups, in declared order (§5.1); written only when
    /// there are some. An inner net's topology carries its own groups and
    /// none of the net around it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,
    /// The controls the net declares, in declaration order (§5.1); written
    /// only when there are some.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controls: Vec<Control>,
}

/// A control in [`Topology`]: a widget a viewer renders, which `press`
/// names (§5.1, §6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Control {
    /// Unique within the net; what `press` names.
    pub name: String,
    /// What the viewer shows.
    pub label: String,
    /// `"button"`, `"toggle"`, `"slider"`, `"select"` or `"text"`: the
    /// widget, and the value a press carries.
    pub kind: String,
    /// The place or transition the control acts on, by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    /// `"inject"`, `"fire"` or `"enable"`: what a press does, for the
    /// viewer's lighting rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// The kind's settings; omitted when it has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<ControlConfig>,
}

/// A [`Control`]'s `config`: what its kind needs, each field written only
/// when it has one.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ControlConfig {
    /// The initial value, for the kinds that have one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    /// A select's choices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
    /// A slider's smallest value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<f64>,
    /// A slider's largest value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<f64>,
    /// A slider's increment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    /// A text box's hint while empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
}

impl ControlConfig {
    /// Whether nothing is set, in which case a control has no `config`.
    pub fn is_empty(&self) -> bool {
        *self == ControlConfig::default()
    }
}

/// A place in [`Topology`].
///
/// A subnet place (PROTOCOL.md §12.7) is `{name, type_name, subnet: true,
/// ports, stepping, group?}`: its `type_name` the template that built its
/// subnet, or `""`, and `ports` its subnet's port places in the subnet's
/// declaration order, each as the subnet's own topology has it but without
/// `port` and `group`. It is not serializable, holding no tokens of its
/// own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    /// Unique within the net.
    pub name: String,
    /// The Rust type name of its tokens, for display; for a subnet place,
    /// its subnet's template, or `""`.
    pub type_name: String,
    /// Whether its tokens have a JSON form. Not written for a subnet place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serializable: Option<bool>,
    /// Whether its tokens may hold nets of their own (§12.2); written only
    /// when true.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub nested: bool,
    /// Whether it is a subnet place (§12.7); written only when true.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub subnet: bool,
    /// A subnet place's ports (§12.7); written for a subnet place alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<Place>>,
    /// Whether the net around this net reaches the place, in a subnet's
    /// own topology (§5.1, §12.7); written only when true, and never in the
    /// topology of a net that is no subnet.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub port: bool,
    /// The rule by which deep stepping fires the nets the place holds
    /// (§5.1, §12.3): written exactly for a place that holds nets, the
    /// default rule included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stepping: Option<Stepping>,
    /// The keys the place derives from its tokens, in declaration order
    /// (§13.1); written only when there are some.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub key_names: Vec<String>,
    /// How the place itself is drawn (§5.1); written only when it has a
    /// style.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<PlaceStyle>,
    /// The name of the group the place is in, one of [`Topology::groups`]
    /// (§5.1); written only for a place in a group, and never for a port
    /// listed in its subnet place's `ports`, whose group is the subnet's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

/// A [`Place`]'s `stepping` (PROTOCOL.md §12.3): when the nets in the place
/// fire, measured against the net around them, by the protocol's four
/// names. The core's rule is `petritype::Stepping`, and the runner writes it
/// as this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stepping {
    /// `"inner_first"`, the default: the nets fire until they are at rest
    /// before the net around them fires.
    InnerFirst,
    /// `"take_turns"`: the net around them and each net here fire in turn.
    TakeTurns,
    /// `"outer_first"`: the nets fire only when the net around them and its
    /// turn-takers have nothing to fire.
    OuterFirst,
    /// `"passive"`: deep stepping never enters the place; the program or a
    /// body steps its nets.
    Passive,
}

/// A [`Place`]'s `style`: `{"fill": "token"}`, a rule the viewer
/// evaluates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaceStyle {
    /// How the place is filled.
    pub fill: Fill,
}

/// How a [`PlaceStyle`] fills its place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fill {
    /// With the colour of the one token it holds, while it holds exactly
    /// one that has a colour.
    Token,
}

/// A transition in [`Topology`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transition {
    /// Unique within the net.
    pub name: String,
    /// Rust's introspection: parameter type names and `detachable`.
    pub detail: TransitionDetail,
    /// The name of the group the transition is in, one of
    /// [`Topology::groups`] (§5.1); written only for a transition in a
    /// group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

/// The Rust runtime's `detail` object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitionDetail {
    /// The type names of the transition's `Param<T>` inputs.
    pub params: Vec<String>,
    /// False for a transition with a mutate edge, whose body runs inline.
    pub detachable: bool,
}

/// An edge in [`Topology`]. An edge on a port place of one of the net's
/// subnet places names the subnet place as `place` and the port as `port`
/// (§12.7); a reader keys an edge by its place, its port if any, its
/// transition and its direction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    /// The place end, or the subnet place of a port.
    pub place: String,
    /// The port of the subnet place `place`, for an edge on a port place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    /// The transition end.
    pub transition: String,
    /// `"in"` (place to transition) or `"out"`.
    pub direction: Direction,
    /// How an input edge reads its place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<EdgeKind>,
    /// The filter a keyed edge (`take_where`, `mutate_where`) chooses its
    /// token with, as declared (§13.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<Filter>,
    /// The output slot, for a transition with more than one output that
    /// returns them by position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<usize>,
}

/// Which way an [`Edge`] points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Place to transition.
    In,
    /// Transition to place.
    Out,
}

/// How an input [`Edge`] reads its place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// Consumes the front token.
    Take,
    /// Consumes every token.
    TakeAll,
    /// Reads a clone of the front token.
    Snapshot,
    /// Borrows the front token and may change it.
    Mutate,
    /// Consumes the oldest token its filter accepts (§13.5).
    TakeWhere,
    /// Borrows the oldest token its filter accepts and may change it.
    MutateWhere,
    /// Consumes every token, zero or more: an empty place does not hold the
    /// transition back (§5.1, slice 22).
    Drain,
    /// Reads a clone of every token, zero or more: an empty place does not
    /// hold the transition back (§5.1, slice 22).
    Survey,
}

// ----- filters ---------------------------------------------------------------

/// A filter on the wire (PROTOCOL.md §13.2): every clause must pass (AND
/// across clauses), and the token must lie in a place `places` names, if
/// given, whose name begins with `prefix`, if given. Places by name, a port
/// place of one of the net's subnet places by `{"place", "port"}`
/// ([`PlaceName`]); `prefix` is tested against a port's own name.
///
/// Read off a request with [`Filter::parse`], which is as strict as the
/// Python runner; the derived `Deserialize` is the lenient reading of a
/// line this runner wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filter {
    /// Every one must pass.
    #[serde(default)]
    pub clauses: Vec<Clause>,
    /// The places to look in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub places: Option<Vec<PlaceName>>,
    /// The start of the name of every place to look in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
}

/// One clause of a [`Filter`]: `{key, values}` or `{key, param}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Clause {
    /// The token's `key` is one of `values` (OR within the clause),
    /// compared by JSON type and value.
    In {
        /// The key's name.
        key: String,
        /// The values accepted.
        values: Vec<KeyValue>,
    },
    /// The token's `key` equals the current value of the net parameter
    /// `param` (keyed edges, §13.5). `IsParam`, as the core's, and not
    /// `Param`, which is the core's parameter input.
    IsParam {
        /// The key's name.
        key: String,
        /// The net parameter's wire name.
        param: String,
    },
}

impl Filter {
    /// Reads a request's filter (PROTOCOL.md §13.2), as strictly as the
    /// Python runner reads one: an object with no fields but `clauses`,
    /// `places` and `prefix`; `clauses` a list (or `null`) of objects with a
    /// string `key` and either a non-empty list `values` of strings,
    /// integers and booleans or a string `param`, never both; `places` a
    /// list of place names and `{"place", "port"}` objects
    /// ([`PlaceName::parse`]); `prefix` a string. Whether the places exist
    /// is the net's to say.
    ///
    /// # Errors
    ///
    /// What is wrong with the shape, for an `invalid_params` answer.
    pub fn parse(value: &Value) -> Result<Filter, String> {
        let Value::Object(object) = value else {
            return Err("a filter is an object".into());
        };
        if let Some(unknown) = object
            .keys()
            .find(|key| !["clauses", "places", "prefix"].contains(&key.as_str()))
        {
            return Err(format!("a filter has no {unknown:?}"));
        }
        let clauses = match object.get("clauses") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(clauses)) => clauses
                .iter()
                .map(Clause::parse)
                .collect::<Result<_, _>>()?,
            Some(_) => return Err("a filter's \"clauses\" is a list".into()),
        };
        let places = match object.get("places") {
            None | Some(Value::Null) => None,
            Some(Value::Array(places)) => Some(
                places
                    .iter()
                    .map(PlaceName::parse)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|why| format!("in \"places\", {why}"))?,
            ),
            Some(_) => return Err("\"places\" is a list of places".into()),
        };
        let prefix = match object.get("prefix") {
            None | Some(Value::Null) => None,
            Some(Value::String(prefix)) => Some(prefix.clone()),
            Some(_) => return Err("\"prefix\" is a string".into()),
        };
        Ok(Filter {
            clauses,
            places,
            prefix,
        })
    }
}

impl Clause {
    fn parse(value: &Value) -> Result<Clause, String> {
        const SHAPE: &str = "a clause is {\"key\", \"values\"} or {\"key\", \"param\"}";
        let Some(Value::String(key)) = value.get("key") else {
            return Err(SHAPE.into());
        };
        let key = key.clone();
        match (value.get("values"), value.get("param")) {
            (None, Some(Value::String(param))) => Ok(Clause::IsParam {
                key,
                param: param.clone(),
            }),
            (None, None) => Err(SHAPE.into()),
            (Some(_), Some(_)) | (None, Some(_)) => {
                Err("a clause has \"values\" or a string \"param\"".into())
            }
            (Some(values), None) => {
                let values = values
                    .as_array()
                    .filter(|values| !values.is_empty())
                    .and_then(|values| values.iter().map(key_value).collect::<Option<Vec<_>>>())
                    .ok_or(
                        "a clause's \"values\" is a non-empty list of strings, integers or booleans",
                    )?;
                Ok(Clause::In { key, values })
            }
        }
    }
}

/// A JSON value as a key value: a string, an integer that fits an `i64`, or
/// a boolean (PROTOCOL.md §13.1); `None` for anything else.
pub(crate) fn key_value(value: &Value) -> Option<KeyValue> {
    match value {
        Value::String(value) => Some(KeyValue::Str(value.clone())),
        Value::Number(number) => number.as_i64().map(KeyValue::Int),
        Value::Bool(value) => Some(KeyValue::Bool(*value)),
        _ => None,
    }
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
    /// The default selector's score; omitted if computing it panicked.
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

/// A token on the wire: its id, a label of at most 1000 characters (plus
/// `"..."` when cut), its colour if it has one, its keys if its place
/// declares some, and the nets it holds, if any.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Token {
    /// The decimal id.
    pub id: String,
    /// The token's `Debug`, cut.
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

// ----- other results ---------------------------------------------------------

/// The answer to `get_place_tokens`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaceTokens {
    /// The place asked for, or the subnet place of the port asked for.
    pub place: String,
    /// The port asked for, beside its subnet place (§12.7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    /// The version the page reflects.
    pub marking_version: u64,
    /// The true count.
    pub count: usize,
    /// How many tokens the connection's filter lets through, while one is
    /// set; the page and `has_more` are then of those (§13.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matching: Option<usize>,
    /// Where the page starts.
    pub offset: usize,
    /// The page size used, after clamping.
    pub limit: usize,
    /// The page.
    pub tokens: Vec<Token>,
    /// Whether tokens follow this page.
    pub has_more: bool,
    /// The inner net's path, for a page of an inner net.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<Vec<PathStep>>,
    /// The inner net's nested id, for a page of an inner net.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nested_id: Option<String>,
}

/// The answer to `get_token`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenValue {
    /// The id asked for.
    pub id: String,
    /// The place holding it.
    pub place: String,
    /// The place's type name.
    pub type_name: String,
    /// The value, as JSON or as text.
    #[serde(flatten)]
    pub value: TokenBody,
    /// The inner net's path, for a token of an inner net.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<Vec<PathStep>>,
    /// The inner net's nested id, for a token of an inner net.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nested_id: Option<String>,
    /// The nets the token holds, if any (§12.2).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nets: Vec<InnerNet>,
}

/// A token's full value: JSON on a serializable place, `Debug` text
/// otherwise.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TokenBody {
    /// `kind: "json"`.
    Json {
        /// The token as JSON.
        value: Value,
    },
    /// `kind: "text"`.
    Text {
        /// The token's `Debug`, untruncated.
        text: String,
    },
}

/// The answer to `query` (PROTOCOL.md §13.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryResult {
    /// The version the answer reflects.
    pub marking_version: u64,
    /// How many tokens matched.
    pub count: usize,
    /// The first `token_limit` of them, in place order then token order.
    pub matches: Vec<QueryMatch>,
}

/// One token a `query` found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryMatch {
    /// Where it is: its place, or the subnet place of its port.
    pub place: String,
    /// Its port, for a match in a port place (§12.7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    /// The token entry, keys included.
    pub token: Token,
    /// Its JSON value, where its place is serializable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json: Option<Value>,
}

/// The answer to `distinct` (PROTOCOL.md §13.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DistinctResult {
    /// The version the answer reflects.
    pub marking_version: u64,
    /// The key asked about.
    pub key: String,
    /// Its values, in order of first appearance.
    pub values: Vec<KeyValue>,
}

/// One entry of `get_log` (PROTOCOL.md §8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    /// From 1, one per landed firing.
    pub seq: u64,
    /// The transition.
    pub transition: String,
    /// When the body started.
    pub started_at: String,
    /// How long until it landed.
    pub duration_ms: f64,
    /// `"fired"` or `"failed"`.
    pub outcome: String,
    /// `[place, id]` taken, a port by `{"place", "port"}` (§12.7).
    pub consumed: Vec<(PlaceName, String)>,
    /// `[place, id]` produced, a port by `{"place", "port"}`.
    pub produced: Vec<(PlaceName, String)>,
    /// The failure's text, for `"failed"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Whether the inputs went back, for `"failed"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restored: Option<bool>,
    /// For a firing inside an inner net: its path when it fired (§8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<Vec<PathStep>>,
    /// For a firing inside an inner net: its nested id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nested_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_request_line_parses_and_params_default_to_an_object() {
        let r = Request::parse(r#"{"id":"7","method":"step"}"#).unwrap();
        assert_eq!((r.id.as_str(), r.method.as_str()), ("7", "step"));
        assert_eq!(r.params, json!({}));
        let r = Request::parse(r#"{"id":"7","method":"step","params":null}"#).unwrap();
        assert_eq!(r.params, json!({}));
        let r = Request::parse(r#"{"id":"7","method":"step","params":[1]}"#).unwrap();
        assert_eq!(r.params, json!([1]), "kept for the method to reject");
    }

    #[test]
    fn a_malformed_line_keeps_the_id_it_could_read() {
        assert_eq!(Request::parse("nonsense").unwrap_err().id, "");
        assert_eq!(Request::parse("[1]").unwrap_err().id, "");
        assert_eq!(
            Request::parse(r#"{"id":3,"method":"x"}"#).unwrap_err().id,
            ""
        );
        assert_eq!(Request::parse(r#"{"id":"9"}"#).unwrap_err().id, "9");
    }

    #[test]
    fn responses_and_notifications_have_the_wire_shape() {
        let ok = Response::result("1", json!({"a": 1}));
        assert_eq!(to_value(&ok), json!({"id": "1", "result": {"a": 1}}));
        let err = Response::error("2", ErrorCode::UnknownPlace, "no");
        assert_eq!(
            to_value(&err),
            json!({"id": "2", "error": {"message": "no", "code": "unknown_place"}})
        );
        let n = Notification::Toggled {
            transition: "T".into(),
            enabled: false,
            marking_version: 3,
            running: false,
        };
        assert!(n.is_state());
        assert_eq!(
            to_value(&n),
            json!({"method": "toggled", "params": {
                "transition": "T", "enabled": false, "marking_version": 3, "running": false}})
        );
        let back: Notification = serde_json::from_value(to_value(&n)).unwrap();
        assert_eq!(back, n);
    }

    fn cabin() -> PathStep {
        PathStep::Token {
            place: "Depot".into(),
            token: "17".into(),
            net: "cabin".into(),
        }
    }

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
    fn a_place_style_and_the_controls_have_the_wire_shape() {
        let place = Place {
            name: "Light".into(),
            type_name: "Colour".into(),
            serializable: Some(true),
            nested: false,
            subnet: false,
            ports: None,
            port: false,
            stepping: None,
            key_names: Vec::new(),
            style: Some(PlaceStyle { fill: Fill::Token }),
            group: None,
        };
        assert_eq!(
            to_value(&place),
            json!({"name": "Light", "type_name": "Colour", "serializable": true,
                   "style": {"fill": "token"}})
        );
        let change = Control {
            name: "change".into(),
            label: "Change".into(),
            kind: "button".into(),
            node: None,
            action: None,
            config: None,
        };
        assert_eq!(
            to_value(&change),
            json!({"name": "change", "label": "Change", "kind": "button"}),
            "node, action and config only when there are some"
        );
        let car = Control {
            node: Some("Waiting".into()),
            action: Some("inject".into()),
            kind: "text".into(),
            config: Some(ControlConfig {
                placeholder: Some("name".into()),
                ..ControlConfig::default()
            }),
            ..change.clone()
        };
        assert_eq!(
            to_value(&car),
            json!({"name": "change", "label": "Change", "kind": "text", "node": "Waiting",
                   "action": "inject", "config": {"placeholder": "name"}})
        );
        let topology = Topology {
            places: vec![place],
            transitions: Vec::new(),
            edges: Vec::new(),
            groups: Vec::new(),
            controls: Vec::new(),
        };
        assert!(
            to_value(&topology).get("controls").is_none(),
            "a net without controls writes none"
        );
        let back: Topology = serde_json::from_value(to_value(Topology {
            controls: vec![car.clone()],
            ..topology
        }))
        .unwrap();
        assert_eq!(back.controls, [car]);
        assert!(ControlConfig::default().is_empty());
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
    fn a_group_is_written_only_when_set() {
        let sorted = Place {
            name: "Sorted".into(),
            type_name: "String".into(),
            serializable: Some(true),
            nested: false,
            subnet: false,
            ports: None,
            port: false,
            stepping: None,
            key_names: Vec::new(),
            style: None,
            group: Some("Sorting".into()),
        };
        let sort = Transition {
            name: "Sort".into(),
            detail: TransitionDetail {
                params: Vec::new(),
                detachable: true,
            },
            group: Some("Sorting".into()),
        };
        let topology = Topology {
            places: vec![sorted.clone()],
            transitions: vec![sort.clone()],
            edges: Vec::new(),
            groups: vec!["Sorting".into(), "Packing".into()],
            controls: Vec::new(),
        };
        let value = to_value(&topology);
        assert_eq!(
            value,
            json!({"places": [{"name": "Sorted", "type_name": "String", "serializable": true,
                               "group": "Sorting"}],
                   "transitions": [{"name": "Sort", "detail": {"params": [], "detachable": true},
                                    "group": "Sorting"}],
                   "edges": [], "groups": ["Sorting", "Packing"]})
        );
        let back: Topology = serde_json::from_value(value).unwrap();
        assert_eq!(back, topology);

        let plain = Topology {
            places: vec![Place {
                group: None,
                ..sorted
            }],
            transitions: vec![Transition {
                group: None,
                ..sort
            }],
            groups: Vec::new(),
            ..topology
        };
        let value = to_value(&plain);
        assert!(value.get("groups").is_none(), "a net with no groups");
        assert!(value["places"][0].get("group").is_none());
        assert!(value["transitions"][0].get("group").is_none());
        let back: Topology = serde_json::from_value(value).unwrap();
        assert_eq!(back, plain, "a reader takes a topology without them");
    }

    #[test]
    fn a_filter_reads_strictly_and_writes_what_it_has() {
        let filter = Filter::parse(&json!({
            "clauses": [{"key": "class", "values": ["Bird", 2, true]},
                        {"key": "class", "param": "season"}],
            "places": ["Forest"], "prefix": "F"}))
        .unwrap();
        assert_eq!(
            filter.clauses,
            [
                Clause::In {
                    key: "class".into(),
                    values: vec!["Bird".into(), KeyValue::Int(2), KeyValue::Bool(true)]
                },
                Clause::IsParam {
                    key: "class".into(),
                    param: "season".into()
                }
            ]
        );
        assert_eq!(
            to_value(&filter),
            json!({"clauses": [{"key": "class", "values": ["Bird", 2, true]},
                               {"key": "class", "param": "season"}],
                   "places": ["Forest"], "prefix": "F"})
        );
        assert_eq!(
            Filter::parse(&json!({})).unwrap(),
            Filter::default(),
            "everything is optional"
        );
        assert_eq!(
            Filter::parse(&json!({"clauses": null, "places": null, "prefix": null})).unwrap(),
            Filter::default()
        );
        assert_eq!(to_value(Filter::default()), json!({"clauses": []}));
        for bad in [
            json!(null),
            json!({"clauses": [{"key": "class"}]}),
            json!({"clauses": [{"key": "class", "values": [null]}]}),
            json!({"clauses": [{"key": "class", "values": "Bird"}]}),
            json!({"clauses": [{"key": "class", "param": 3}]}),
            json!({"clauses": [{"key": 3, "values": [1]}]}),
            json!({"places": [1]}),
            json!({"limit": 3}),
        ] {
            assert!(Filter::parse(&bad).is_err(), "{bad}");
        }
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

    #[test]
    fn every_error_code_spells_as_its_serde_name() {
        for code in [
            ErrorCode::UnknownMethod,
            ErrorCode::InvalidParams,
            ErrorCode::NotLoaded,
            ErrorCode::UnknownNet,
            ErrorCode::LoadFailed,
            ErrorCode::UnknownPlace,
            ErrorCode::UnknownTransition,
            ErrorCode::UnknownToken,
            ErrorCode::NotSerializable,
            ErrorCode::InvalidToken,
            ErrorCode::Unsupported,
            ErrorCode::UnknownControl,
            ErrorCode::Internal,
        ] {
            assert_eq!(to_value(code), json!(code.as_str()));
        }
    }
}
