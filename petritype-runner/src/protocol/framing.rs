//! Framing: the request and response lines, and the error codes.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

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

/// The error codes of PROTOCOL.md §3.
///
/// Read tolerantly (since 2026-11-23): a code this version does not know is
/// kept as [`Other`](ErrorCode::Other), its name as written, and the rest of
/// the answer is read, so that a client meets a later runner's new code as a
/// failed request with that code (§3). A client built on 0.2.x fails to read
/// such an answer at all. No longer `Copy`, since `Other` holds its name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    /// `inject` named a place of a closed net that is not one of its ports
    /// (§3, §6.4, since 2026-11-23): the net does not declare itself open.
    /// Every net is closed unless it does, since 2026-11-28 in this runner;
    /// until then only a net that marked a port was. Also, since
    /// 2026-11-28, any place, a port included, of a net inside a closed
    /// net, addressed by `path` or by `nested_id`. An older client sees a
    /// code it does not know, which §3 tells it to treat as a failed
    /// request.
    ClosedNet,
    /// Anything else.
    Internal,
    /// A code this version does not know, by the name it was written with,
    /// written back unchanged. A runner never sends one of its own.
    #[serde(untagged)]
    Other(String),
}

impl ErrorCode {
    /// The code as it appears on the wire, for example `"unknown_place"`.
    pub fn as_str(&self) -> &str {
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
            ErrorCode::ClosedNet => "closed_net",
            ErrorCode::Internal => "internal",
            ErrorCode::Other(code) => code,
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
    ///
    /// # Errors
    ///
    /// What `serde_json` says when `result` cannot be written as JSON: its
    /// own `Serialize` refused, or it is a map whose keys are not strings
    /// (since 2026-11-28; until then this panicked). The runner answers the
    /// request with an `internal` error instead and says on stderr what was
    /// lost, as the Python runner does.
    pub fn result(
        id: impl Into<String>,
        result: impl Serialize,
    ) -> Result<Response, serde_json::Error> {
        Ok(Response {
            id: id.into(),
            outcome: Outcome::Result(serde_json::to_value(result)?),
        })
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
    ///
    /// # Errors
    ///
    /// What `serde_json` says when the response cannot be written (since
    /// 2026-11-28; until then this panicked). A response holds JSON values
    /// and strings only, which always write, so this is for completeness.
    pub fn to_line(&self) -> Result<String, serde_json::Error> {
        to_line(self)
    }
}

/// Why a heartbeat line is always written: it holds strings, integers and
/// booleans only, which JSON always writes.
pub(crate) const HEARTBEAT_WRITES: &str =
    "a heartbeat holds strings, integers and booleans only, which JSON always writes";

/// A protocol value as JSON, or what `serde_json` said: a value whose own
/// `Serialize` refused, or a map whose keys are not strings. A `NaN` or an
/// infinity is written as `null`; the owner keeps them off its lines.
#[cfg(test)]
pub(crate) fn to_value(value: impl Serialize) -> Value {
    serde_json::to_value(value).expect("the test's shapes serialise")
}

/// A protocol value as one line of JSON, without the newline, or what
/// `serde_json` said, as for [`Response::result`].
pub(crate) fn to_line(value: impl Serialize) -> Result<String, serde_json::Error> {
    serde_json::to_string(&value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    use crate::protocol::Notification;

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
        let ok = Response::result("1", json!({"a": 1})).unwrap();
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
            ErrorCode::UnknownControl,
            ErrorCode::ClosedNet,
            ErrorCode::Internal,
        ] {
            assert_eq!(to_value(&code), json!(code.as_str()));
        }
    }

    /// An answer carrying a code this version does not know is read whole:
    /// the code is kept by its name and written back unchanged, and a known
    /// code still reads as itself.
    #[test]
    fn an_unknown_error_code_is_kept_and_the_answer_read() {
        let line = r#"{"id":"7","error":{"code":"teapot_full","message":"no room"}}"#;
        let answer: Response = serde_json::from_str(line).unwrap();
        assert_eq!(answer.id, "7");
        let Outcome::Error(error) = &answer.outcome else {
            panic!("an error answer: {answer:?}");
        };
        assert_eq!(error.code, ErrorCode::Other("teapot_full".into()));
        assert_eq!(error.code.as_str(), "teapot_full");
        assert_eq!(error.message, "no room");
        assert_eq!(
            to_value(&answer),
            json!({"id": "7", "error": {"code": "teapot_full", "message": "no room"}})
        );
        let known: ErrorCode = serde_json::from_value(json!("closed_net")).unwrap();
        assert_eq!(known, ErrorCode::ClosedNet);
    }
}
