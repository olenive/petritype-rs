//! Serve a petritype net over the net protocol on stdin and stdout.
//!
//! The protocol is newline-delimited JSON, specified once for every runtime
//! and viewer in
//! [PROTOCOL.md](https://github.com/olenive/petritype/blob/main/PROTOCOL.md)
//! (version 1). A host — petritype-server's worker, the Bevy visualiser —
//! spawns a net process, asks it to `describe` and `load` a net, and then
//! drives and watches it with `get_state`, `step`, `start`, `inject` and the
//! rest, reading the notifications the process writes as the net changes.
//!
//! A net's transition bodies are the author's code, so the net process is
//! always the author's own binary. It lists the nets it carries and hands
//! over to [`main`]:
//!
//! ```no_run
//! use petritype::{Net, NetBuilder, TakeEdge};
//! use petritype_runner::{Hosting, LoadError, NetEntry, ParamSpec, Params};
//!
//! /// A factory takes the load parameters as JSON and returns a built net.
//! fn build(params: &Params) -> Result<Net, LoadError> {
//!     let trucks = params["trucks"]
//!         .as_u64()
//!         .ok_or_else(|| LoadError::InvalidParams("trucks must be a number".into()))?;
//!     let mut b = NetBuilder::new();
//!     let inbox = b.place::<u64>("Inbox");
//!     let out = b.place::<u64>("Out");
//!     b.seed(inbox, 0..trucks);
//!     b.transition("Double", (TakeEdge(inbox),), (out,), |n: u64| (n * 2,));
//!     Ok(b.build()?)
//! }
//!
//! fn main() -> std::process::ExitCode {
//!     petritype_runner::main([
//!         NetEntry::new("doubler", build)
//!             .description("Doubles numbers")
//!             .param(ParamSpec::new("trucks", "u64").default(2)),
//!         NetEntry::new("doubler-24-7", build)
//!             .param(ParamSpec::new("trucks", "u64").default(0))
//!             .hosting(Hosting::Continuous),
//!     ])
//! }
//! ```
//!
//! `binary --describe` prints what it carries as one JSON line; `binary`
//! with no arguments serves the protocol. While serving, a `println!` in a
//! body goes to stderr, never into the protocol stream (Unix).
//!
//! # Layers
//!
//! - [`protocol`]: the wire shapes as serde types, keyed by names, with
//!   string token ids. Nothing there knows about a [`Net`](petritype::Net).
//! - [`Owner`]: the state machine. It turns an [`Input`] into the
//!   [`Outgoing`] lines and firings it causes, creates no threads, reads no
//!   clock but the one it is handed, and writes nothing — so a host or a
//!   test can embed it without a process.
//! - [`main`]: the stdout guard, a reader, a writer, a heartbeat, a thread
//!   per detachable body, and the loop around the owner.
//!
//! # How a Rust net looks on the wire
//!
//! - A firing is reported as `launched` then `distributed`, because every
//!   detachable body runs off the owner's thread. A transition with a
//!   mutate edge runs inline, inside `launch`, and is reported as one
//!   `fired`; while it runs, the owner answers nothing (the heartbeat still
//!   goes out).
//! - `get_state`, `get_place_tokens`, `get_token`, `get_log` and `stop` are
//!   answered while a detachable body runs; other requests wait for it to
//!   land, in arrival order.
//! - Token labels are the token's `Debug`, cut at 1000 characters; a token
//!   on a place marked `serializable` also has a JSON form, which is what
//!   `inject` and `get_token`'s `kind: "json"` need.
//! - `set_param` sets a net-wide parameter registered with
//!   `NetBuilder::serializable_param`; its `transition` is ignored.

#![warn(missing_docs, missing_debug_implementations, unreachable_pub)]
#![forbid(unsafe_code)]

mod errors;
mod io;
mod owner;
pub mod protocol;
mod registry;
mod time;
mod translate;

pub use io::main;
pub use owner::{Input, Outgoing, Owner, StatusCell};
pub use registry::{Factory, Hosting, LoadError, NetEntry, ParamSpec, Params};
