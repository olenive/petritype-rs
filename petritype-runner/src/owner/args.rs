//! Reading a request's parameters.

use serde_json::{Map, Value};

use crate::errors::CoreError;
use crate::protocol::ErrorCode;
use crate::translate::FilterError;

use super::RpcError;

/// A request's parameters, or why they are not usable.
pub(super) type Args = Map<String, Value>;

/// `get_state`'s and `get_place_tokens`'s default page.
const DEFAULT_TOKEN_LIMIT: u64 = 200;
/// The largest page either will send.
const MAX_TOKEN_LIMIT: u64 = 1000;

/// A request's optional `port`: `None` when absent or `null`, and
/// `invalid_params` when it is not a string.
pub(super) fn port_arg(args: &Args) -> Result<Option<&str>, RpcError> {
    match optional(args, "port") {
        None => Ok(None),
        Some(Value::String(port)) => Ok(Some(port)),
        Some(_) => Err(RpcError::new(
            ErrorCode::InvalidParams,
            "\"port\" must be a string",
        )),
    }
}

/// A request's filter that could not be used, as the error it answers.
pub(super) fn filter_error(error: FilterError) -> RpcError {
    RpcError::core(CoreError::Filter(&error))
}

pub(super) fn not_place_names() -> RpcError {
    RpcError::new(
        ErrorCode::InvalidParams,
        "\"places\" is a list of place names and {\"place\", \"port\"} objects",
    )
}

pub(super) fn string_arg<'a>(args: &'a Args, key: &str) -> Result<&'a str, RpcError> {
    match args.get(key) {
        Some(Value::String(value)) => Ok(value),
        _ => Err(RpcError::new(
            ErrorCode::InvalidParams,
            format!("needs a string {key:?}"),
        )),
    }
}

/// An optional field of a request: `None` when it is absent *or* `null`,
/// which counts as absent everywhere (PROTOCOL.md §2, §13.3). Every optional
/// field is read through here; a required one (`place`, `token`, `value`)
/// is read directly, and a `null` there is a value.
pub(super) fn optional<'a>(args: &'a Args, key: &str) -> Option<&'a Value> {
    args.get(key).filter(|value| !value.is_null())
}

pub(super) fn u64_arg(args: &Args, key: &str) -> Result<Option<u64>, RpcError> {
    match optional(args, key) {
        None => Ok(None),
        Some(value) => value.as_u64().map(Some).ok_or_else(|| {
            RpcError::new(
                ErrorCode::InvalidParams,
                format!("{key:?} must be a non-negative integer"),
            )
        }),
    }
}

pub(super) fn usize_arg(args: &Args, key: &str) -> Result<Option<usize>, RpcError> {
    Ok(u64_arg(args, key)?.map(|n| usize::try_from(n).unwrap_or(usize::MAX)))
}

/// A page size: 200 by default, clamped to 1..=1000.
pub(super) fn page_limit(args: &Args, key: &str) -> Result<usize, RpcError> {
    Ok(u64_arg(args, key)?
        .unwrap_or(DEFAULT_TOKEN_LIMIT)
        .clamp(1, MAX_TOKEN_LIMIT) as usize)
}
