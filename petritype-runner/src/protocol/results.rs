//! The answers to `describe` and to the reads other than `get_state`.

use petritype::KeyValue;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{InnerNet, PathStep, PlaceName, Token};

/// The answer to `describe` (PROTOCOL.md §6.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Describe {
    /// Always 2 here ([`PROTOCOL_VERSION`](super::PROTOCOL_VERSION)).
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
