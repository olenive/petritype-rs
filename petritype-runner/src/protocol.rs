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
//! heartbeat's two fields are `null` by design).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The protocol version this crate speaks.
pub const PROTOCOL_VERSION: u32 = 1;

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

// ----- notifications ---------------------------------------------------------

/// A notification line: `{"method": ..., "params": {...}}`.
///
/// Every notification carries `marking_version` (after the change, if it made
/// one) and `running` (PROTOCOL.md §7). The state notifications are the ones
/// that raise `marking_version` by one each; see
/// [`is_state`](Notification::is_state).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Notification {
    /// A firing reported in one piece.
    Fired {
        /// The transition that fired.
        transition: String,
        /// `[place, id]` of each token taken.
        consumed: Vec<(String, String)>,
        /// `[place, id, label]` of each token produced.
        produced: Vec<(String, String, String)>,
        /// How long the body ran.
        duration_ms: f64,
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
        consumed: Vec<(String, String)>,
        /// See the type's documentation.
        marking_version: u64,
        /// See the type's documentation.
        running: bool,
    },
    /// The second half: body returned, outputs placed.
    Distributed {
        /// The transition that landed.
        transition: String,
        /// `[place, id, label]` of each token produced.
        produced: Vec<(String, String, String)>,
        /// How long the body was airborne.
        duration_ms: f64,
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
        consumed: Vec<(String, String)>,
        /// See the type's documentation.
        marking_version: u64,
        /// See the type's documentation.
        running: bool,
    },
    /// A token arrived from outside.
    Added {
        /// Where.
        place: String,
        /// Its id.
        id: String,
        /// Its label.
        label: String,
        /// See the type's documentation.
        marking_version: u64,
        /// See the type's documentation.
        running: bool,
    },
    /// A token was removed from outside.
    Removed {
        /// Where from.
        place: String,
        /// Its id.
        id: String,
        /// See the type's documentation.
        marking_version: u64,
        /// See the type's documentation.
        running: bool,
    },
    /// A token changed in place.
    Mutated {
        /// Where.
        place: String,
        /// Its id.
        id: String,
        /// Its new label.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
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
    /// Periodic while loaded; both fields `null` when no body runs.
    Heartbeat {
        /// When the running body started.
        step_started_at: Option<String>,
        /// The transition whose body is running.
        executing_transition: Option<String>,
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
                | Notification::Toggled { .. }
                | Notification::ParamChanged { .. }
        )
    }

    /// The notification as one line of JSON, without the newline.
    pub fn to_line(&self) -> String {
        to_line(self)
    }
}

// ----- describe --------------------------------------------------------------

/// The answer to `describe` (PROTOCOL.md §6.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Describe {
    /// Always 1 here.
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
    /// Always 1 here.
    pub protocol_version: u32,
    /// `"rust"`.
    pub runtime: String,
    /// The loaded net's registered name.
    pub net: String,
    /// See PROTOCOL.md §4.
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
}

/// A place in [`Topology`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    /// Unique within the net.
    pub name: String,
    /// The Rust type name of its tokens, for display.
    pub type_name: String,
    /// Whether its tokens have a JSON form.
    pub serializable: bool,
}

/// A transition in [`Topology`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transition {
    /// Unique within the net.
    pub name: String,
    /// Rust's introspection: parameter type names and `detachable`.
    pub detail: TransitionDetail,
}

/// The Rust runtime's `detail` object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitionDetail {
    /// The type names of the transition's `Param<T>` inputs.
    pub params: Vec<String>,
    /// False for a transition with a mutate edge, whose body runs inline.
    pub detachable: bool,
}

/// An edge in [`Topology`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    /// The place end.
    pub place: String,
    /// The transition end.
    pub transition: String,
    /// `"in"` (place to transition) or `"out"`.
    pub direction: Direction,
    /// How an input edge reads its place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<EdgeKind>,
    /// The output slot, for a transition with more than one output.
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
    pub places: BTreeMap<String, PlaceMarking>,
}

/// One place's tokens: the true count and the oldest ones, capped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaceMarking {
    /// How many tokens the place holds.
    pub count: usize,
    /// The oldest, up to the limit.
    pub tokens: Vec<Token>,
}

/// A token on the wire: its id and a label of at most 1000 characters (plus
/// `"..."` when cut).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Token {
    /// The decimal id.
    pub id: String,
    /// The token's `Debug`, cut.
    pub label: String,
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
    /// The place asked for.
    pub place: String,
    /// The version the page reflects.
    pub marking_version: u64,
    /// The true count.
    pub count: usize,
    /// Where the page starts.
    pub offset: usize,
    /// The page size used, after clamping.
    pub limit: usize,
    /// The page.
    pub tokens: Vec<Token>,
    /// Whether tokens follow this page.
    pub has_more: bool,
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
    /// `[place, id]` taken.
    pub consumed: Vec<(String, String)>,
    /// `[place, id]` produced.
    pub produced: Vec<(String, String)>,
    /// The failure's text, for `"failed"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Whether the inputs went back, for `"failed"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restored: Option<bool>,
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
            ErrorCode::Internal,
        ] {
            assert_eq!(to_value(code), json!(code.as_str()));
        }
    }
}
