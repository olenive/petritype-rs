//! The wire shapes of the net protocol, as serde types.
//!
//! Everything here is keyed by names, with token ids as strings, exactly as
//! [PROTOCOL.md](https://github.com/olenive/petritype/blob/main/PROTOCOL.md)
//! writes it. Nothing in this module knows about a [`Net`](petritype::Net):
//! turning the core's ids and events into these shapes is the owner's job.
//! The shapes live here and not in the core so that a wire format the core
//! does not own cannot freeze the core's types.
//!
//! Optional fields are omitted, never `null`, unless the protocol says
//! otherwise (`progress.last_fired`, `progress.last_progress_at` and the
//! heartbeat's `step_started_at` and `executing_transition` are `null` by
//! design, and so is the colour of a `produced` entry that carries keys,
//! [`Produced`]).
//!
//! A key's value is the core's [`KeyValue`](petritype::KeyValue): a string,
//! an integer or a boolean, written as the bare JSON value (PROTOCOL.md
//! §13.1).
//!
//! The address of an inner net, its `path`, is a list of [`PathStep`]s, one
//! per level from the loaded net down (PROTOCOL.md §12.1), never text: no
//! character in a name has a special meaning.

mod address;
mod filter;
mod framing;
mod notifications;
mod results;
mod state;
mod topology;

pub use address::{PathStep, PlaceName, parse_path};
pub use filter::{Clause, Filter, InputKey};
pub use framing::{ErrorBody, ErrorCode, Malformed, Outcome, Request, Response};
pub use notifications::{HiddenKind, Keys, Notification, Produced};
pub use results::{
    Describe, DistinctResult, LogEntry, NetDescription, ParamDescription, PlaceTokens, QueryMatch,
    QueryResult, TokenBody, TokenValue,
};
pub use state::{
    InnerNet, Marking, PlaceEntry, PlaceMarking, Progress, State, SubnetEntry, SubnetMarking,
    Token, TransitionState,
};
pub use topology::{
    Control, ControlConfig, Direction, Edge, EdgeKind, Fill, Place, PlaceStyle, Stepping, Topology,
    Transition, TransitionDetail,
};

#[cfg(test)]
pub(crate) use framing::to_value;
pub(crate) use framing::{HEARTBEAT_WRITES, to_line};

/// The protocol version this crate speaks. A reader of version 2 does not
/// read version 1 (PROTOCOL.md §10).
pub const PROTOCOL_VERSION: u32 = 2;

/// What every Rust runner reports as its `runtime`.
pub const RUNTIME: &str = "rust";
