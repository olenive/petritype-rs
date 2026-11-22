//! Serve a petritype net over the net protocol on stdin and stdout.
//!
//! The protocol is newline-delimited JSON, specified once for every runtime
//! and viewer in
//! [PROTOCOL.md](https://github.com/olenive/petritype/blob/main/PROTOCOL.md)
//! (version 2). A host (a server's worker, the Bevy viewer)
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
//! with no arguments serves the protocol. `--net <name>`, with either,
//! carries the one net registered as `<name>` and no other, so that
//! `describe` lists it alone: a host replays a shared transcript of
//! PROTOCOL.md §11, which may pin `describe` in full, against the binary
//! started that way. While serving, a `println!` in a body goes to stderr,
//! never into the protocol stream (Unix).
//!
//! A program that builds a net and fires it from its own code (a game, a
//! simulation) can be watched too: it gives the net to a [`HostServer`],
//! which listens on a TCP port, and a viewer attaches to read it. Nothing
//! is loaded or driven over that connection; see [`Host`].
//!
//! # Layers
//!
//! - [`protocol`]: the wire shapes as serde types, keyed by names, with
//!   string token ids. Nothing there knows about a [`Net`](petritype::Net).
//! - [`Owner`]: the state machine. It turns an [`Input`] into the
//!   [`Outgoing`] lines and firings it causes, creates no threads, takes
//!   the time of day only from the `now` it is handed, times a body it runs
//!   inline with a stopwatch of its own, and writes nothing, so a host or a
//!   test can embed it without a process.
//! - [`main`]: the stdout guard, a reader, a writer, a heartbeat, a thread
//!   per detachable body, and the loop around the owner.
//! - [`Host`]: the state machine around a net the program owns, and
//!   [`HostServer`], the TCP listener in front of it.
//!
//! # How a Rust net looks on the wire
//!
//! - A firing is reported as `launched` then `distributed`, because every
//!   detachable body runs off the owner's thread. Two kinds of body run
//!   inline, on the owner's thread, inside the request or tick that fires
//!   them, and each such firing is reported in one piece: a transition with
//!   a mutate edge runs inside `launch`, and is reported as its `mutated`
//!   lines then one `fired`; a firing inside an inner net runs inside the
//!   deep step, and is reported as one `nested` around its `fired`. Their
//!   `duration_ms`, in the notification and in the log entry, is the time
//!   the body took, measured round the call that ran it, and a body that
//!   failed has that time in its log entry. A `press` runs its control
//!   inline too, and a press that fires one transition of the loaded net
//!   gives that firing the time the press took. A firing a body makes
//!   inside the nets of the tokens it holds is part of that body's time,
//!   and carries 0. The process answers no request until an inline body
//!   ends; the heartbeat still goes out, and names the body. Every
//!   `heartbeat` names the body that runs, if one does, by its transition's
//!   name in its own net (`executing_transition`) and when it started
//!   (`step_started_at`), and a body inside an inner net by that net's
//!   `path` and `nested_id` too, as the firing's `step` result names it; the
//!   two are absent for a body of the loaded net and when no body runs
//!   ([`StatusCell`]). A firing that a body makes inside the nets of its own
//!   tokens is named by no heartbeat, which names the body that made it,
//!   and a press's firings by none at all.
//! - `get_state`, `get_place_tokens`, `get_token`, `get_log` and `stop` are
//!   answered while a detachable body runs; other requests wait for it to
//!   land, in arrival order. A request that arrives while a body runs
//!   inline is read when that body lands, before a running net fires
//!   again, so a `stop` sent then is answered after that one body and the
//!   net fires nothing more first. `start` is answered when the first body
//!   of the run has landed, if that body runs inline, and a process whose
//!   input closes during an inline body ends when the body lands.
//! - Token labels are the token's `Debug`, cut at 1000 characters; a token
//!   on a place marked `serializable` also has a JSON form, which is what
//!   `inject` and `get_token`'s `kind: "json"` need.
//! - `set_param` sets a net-wide parameter registered with
//!   `NetBuilder::serializable_param`; its `transition` is ignored.
//! - A token that carries nets of its own (`petritype::Nested`, a place
//!   declared with `place_nested`) lists them in `nets` wherever a token is
//!   given in full, and the reads and `inject` take a `path`, a list of
//!   steps from the loaded net down
//!   (`[{"place": "Depot", "token": "0", "net": "cabin"}]`,
//!   [`protocol::PathStep`]), or a `nested_id` to act on one of them
//!   (PROTOCOL.md §12). What happens inside arrives as one `nested`
//!   notification around the inner net's own, with that net's own
//!   `marking_version`, and the wrapper carries the outer net's, which
//!   moves when the inner event changes a port place of one of the outer
//!   net's subnets (below). `step` and a running net fire one transition
//!   in the order the nested places' stepping rules give
//!   (`petritype::Stepping`, which the topology writes as each such place's
//!   `stepping`): the outer net's choice, launched, or one inner firing,
//!   inline. Progress is per net: an inner firing is counted in the
//!   progress of the net it was in, and no other.
//! - A subnet place (`NetBuilder::subnet_place`) holds a net of its own,
//!   its subnet, addressed by the step `{"place": "Level 1"}` and served as
//!   one more inner net (PROTOCOL.md §12.7). The net around it names a port
//!   of the subnet by the pair: `port` beside `place` in an object, and
//!   `{"place", "port"}` ([`protocol::PlaceName`]) where a place is named
//!   by position. A port's tokens carry the subnet's ids, are projected by
//!   the subnet's filter, and are told once, by the net that changed them;
//!   that notification carries the other net's new version too, as
//!   `subnet_marking_versions` or `outer_marking_version`.
//! - A net's groups (`NetBuilder::group`), a label on parts of one net that
//!   only drawings read, are in its topology: `groups` lists them in
//!   declared order, and each place, subnet place and transition in one
//!   carries its `group` (PROTOCOL.md §5.1). Each inner net's topology
//!   carries its own groups, and a port listed in a subnet place's `ports`
//!   carries none, its group being the subnet's. No notification, marking
//!   entry or log entry names a group.
//! - A place that declares keys (`NetBuilder::keys`) lists them as
//!   `key_names`, and its tokens carry theirs as `keys` wherever a token is
//!   given, `produced` entries included (PROTOCOL.md §13). `query` asks for
//!   the tokens a filter keeps and `distinct` for a key's values, from the
//!   net's key index; `set_filter` projects one net, the outer one or an
//!   inner one, through a filter, so that reads list only what it keeps,
//!   with `matching` beside `count`, and a notification about a token it
//!   hides arrives as `hidden {kind, place}`.

#![warn(missing_docs, missing_debug_implementations, unreachable_pub)]
#![forbid(unsafe_code)]

mod errors;
mod host;
mod io;
mod owner;
mod projection;
pub mod protocol;
mod registry;
mod time;
mod translate;

pub use host::{Host, HostServer};
pub use io::main;
pub use owner::{InnerAddress, Input, Outgoing, Owner, StatusCell};
pub use registry::{Factory, Hosting, LoadError, NetEntry, ParamSpec, Params};
