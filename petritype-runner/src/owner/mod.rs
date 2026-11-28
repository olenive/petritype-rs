//! The state machine at the heart of the runner (DESIGN §19.3.2–§19.3.6,
//! §19.3.11 for nested nets, §19.3.12 for keys, queries and the
//! connection's projection, and §19.3.17 for subnet places).
//!
//! [`Owner`] owns the registry and the loaded net and turns each [`Input`]
//! into the [`Outgoing`] lines and firings it causes. It creates no threads,
//! takes the time of day only from the `now` it is handed, and writes
//! nothing, so every behaviour of the protocol can be tested by feeding it
//! inputs and reading what comes out. The one clock it reads itself is a
//! stopwatch ([`Instant`](std::time::Instant)) round a core call that runs a
//! body inline, on the owner's thread, whose time is that firing's
//! `duration_ms`.

mod address;
mod args;
mod commands;
mod dispatch;
mod events;
mod firing;
mod hosted;
mod lifted;
mod reads;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use petritype::{Completed, Firing, Net, NetId, PlaceId, TokenId, Topology, TransitionId};

use crate::errors::{CoreError, code_of};
use crate::projection::Projection;
use crate::protocol::{
    self, Describe, ErrorCode, LogEntry, Notification, PathStep, Request, Response,
};
use crate::registry::{NetEntry, Params};
use crate::translate::Names;

use events::fireable_names;
use hosted::HostFlight;

/// What the heartbeat thread reads: the transition whose body is running,
/// when it started, and, for a body that runs inside an inner net, where
/// that net is; `None` when no body runs.
///
/// The owner writes it ([`Owner::new`] says when) and a heartbeat reads it:
/// [`main`](crate::main)'s writes the name as `executing_transition`, the
/// time as `step_started_at`, and the [`InnerAddress`], when there is one,
/// as `path` and `nested_id`. The third element is `None` for a body of the
/// loaded net.
pub type StatusCell = Arc<Mutex<Option<(String, SystemTime, Option<InnerAddress>)>>>;

/// Where an inner net is, as the wire names it (PROTOCOL.md §12.1): its
/// `path` from the loaded net and its `nested_id`, the two fields that a
/// `step` result and a log entry carry for a firing inside an inner net.
/// The [`StatusCell`] holds one while a body runs in an inner net.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct InnerAddress {
    /// The net's path, one step per level from the loaded net down.
    pub path: Vec<PathStep>,
    /// The net's nested id, as the wire writes it.
    pub nested_id: String,
}

/// How many entries `get_log` keeps (PROTOCOL.md §8).
const LOG_CAPACITY: usize = 1000;

/// One thing that happened to the runner.
#[derive(Debug)]
pub enum Input {
    /// A request line, already parsed.
    Request(Request),
    /// A body that ran on some thread, coming home: the result of
    /// [`Firing::run`] on a firing this owner handed out as
    /// [`Outgoing::Launch`].
    Completed(Completed),
    /// The ticker: a running net that is free looks for its next firing, and
    /// a `"24/7"` net that is idle checks again.
    Tick,
}

/// One thing the caller of [`Owner::handle`] must do, in order.
#[derive(Debug)]
pub enum Outgoing {
    /// A notification or a response, serialised, without its newline. Lines
    /// must be written in the order they come.
    Line(String),
    /// A detachable firing to run on some thread; its [`Firing::run`] result
    /// comes back as [`Input::Completed`].
    Launch(Firing),
    /// `shutdown` was answered: stop reading and exit once the lines before
    /// this one are written.
    Exit,
}

/// What the heartbeat thread reads besides the status cell: whether the
/// net runs, mirrored after every [`Owner::handle`] and also just before a
/// body runs inline. The marking version and whether a net is loaded are
/// not here: [`main`](crate::main) hands them to the writer with each
/// batch of lines, so that a heartbeat carries the version of the lines
/// written before it.
#[derive(Debug, Default)]
pub(crate) struct Gauges {
    pub(crate) running: AtomicBool,
}

/// The runner's state machine: the registry, the loaded net and everything
/// the protocol keeps about it.
///
/// Feed it [`Input`]s with [`handle`](Owner::handle) and do what the
/// [`Outgoing`]s it returns say. [`main`](crate::main) does exactly that with
/// stdin, stdout and threads; a test or an embedding host can do it with none
/// of them.
pub struct Owner {
    nets: Vec<NetEntry>,
    status: StatusCell,
    gauges: Arc<Gauges>,
    heartbeat: Duration,
    /// Loads and resets so far: 0 until the first `load`.
    generation: u64,
    /// Never decreases, and continues across `load` and `reset`.
    marking_version: u64,
    loaded: Option<Loaded>,
    /// Set when a run fired an inline body during the last `handle` and could
    /// fire again at once.
    poke: bool,
    /// The loaded net is the host program's ([`Owner::hosted`]): the wire
    /// reads it and never loads, fires or changes it.
    hosted: bool,
    /// The staged firings a host has launched and not yet landed, oldest
    /// first: what each took and when, for its log entry.
    host_flights: Vec<HostFlight>,
    /// Someone reads the lines that tell of a hosted net's changes. While
    /// nobody does, [`host_changes`](Owner::host_changes) builds no line
    /// and keeps the rest. Always true for a net the wire loads.
    listening: bool,
}

/// One generation of one net, and what the runner keeps about it
/// (DESIGN §19.3.3).
struct Loaded {
    entry: usize,
    /// The entry's name: what `get_state` calls the net.
    name: &'static str,
    params: Params,
    net: Net,
    topology: Topology,
    names: Names,
    running: bool,
    in_flight: Option<InFlight>,
    deferred: VecDeque<Request>,
    last_fireable: Vec<String>,
    log: VecDeque<Logged>,
    log_seq: u64,
    progress: ProgressKept,
    idle: bool,
    /// The wire name of the parameter the `set_param` being served sets.
    pending_param: Option<String>,
    /// One record per live inner net, by nested id (PROTOCOL.md §12.3).
    inner: HashMap<NetId, Inner>,
    /// Inner nets that have had events since their fireable set was last
    /// compared or taken afresh, in the order they first had them.
    touched: Vec<NetId>,
    /// The connection's projection of the outer net (PROTOCOL.md §13.4),
    /// while `set_filter` has set one. A generation's own: `load` and
    /// `reset` clear it.
    projection: Option<Projection>,
    /// The loaded net's subnet places (PROTOCOL.md §12.7).
    seats: Vec<Seat>,
    /// The loaded net's places that hold nets ([`holding`]): empty for a
    /// flat net, which never holds an inner net.
    holding: Vec<PlaceId>,
}

/// One subnet place of a net, as the owner keeps it (PROTOCOL.md §12.7):
/// the place, its subnet's nested id, and the places the net has for the
/// subnet's ports, its *windows* (DESIGN §6.7), each with the port's own id
/// in the subnet. A subnet lives as long as its net, so a net's seats never
/// change.
#[derive(Debug, Clone)]
struct Seat {
    place: PlaceId,
    id: NetId,
    windows: Vec<(PlaceId, PlaceId)>,
}

/// The subnet places of `net`, in declaration order.
fn seats_of(net: &Net) -> Vec<Seat> {
    net.places()
        .filter(|(_, info)| info.subnet)
        .filter_map(|(place, _)| {
            let subnet = net.subnet(place)?;
            let windows = net
                .places()
                .filter(|(_, info)| info.port_of == Some(place))
                .filter_map(|(window, info)| Some((window, subnet.place_named(info.name)?)))
                .collect();
            Some(Seat {
                place,
                id: subnet.id(),
                windows,
            })
        })
        .collect()
}

/// The net around a subnet: the net that holds its subnet place, which
/// sees the subnet's ports too, so that a change of one moves its version
/// as well (PROTOCOL.md §12.7). It never changes: a subnet lives in its
/// net for that net's whole life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Around {
    /// The loaded net.
    Loaded,
    /// An inner net, by its nested id.
    Inner(NetId),
}

/// What the runner keeps about one inner net (PROTOCOL.md §12.3,
/// DESIGN §19.3.11): what it keeps about the outer net, per net.
///
/// Made when the net is first seen (at `load` and `reset` by walking the
/// marking, at an `inject` for the nets inside the token it brings, when a
/// request names it, or when one of its events arrives)
/// and dropped at the first landing (or inner firing) after which its
/// nested id no longer resolves: the token that held it was consumed and
/// not put back. A subnet's record lives as long as the net around it
/// (PROTOCOL.md §12.7): for the loaded net's subnets, from `load` or
/// `reset` until the next.
struct Inner {
    /// Its own version: 0 when first seen, one more per state notification
    /// of this net. Never the outer net's.
    marking_version: u64,
    /// Its shape, which never changes once built (§12.6). `None` only for a
    /// net first heard of while its token was in a body's hands, until it is
    /// seen.
    topology: Option<Topology>,
    /// Its name tables, empty while `topology` is `None`.
    names: Names,
    progress: ProgressKept,
    last_fireable: Vec<String>,
    /// The template that built it, when one did.
    template: Option<String>,
    /// The path it was last found at or reported under.
    last_path: Vec<PathStep>,
    /// The connection's projection of this net, while `set_filter` named it
    /// and set one (PROTOCOL.md §13.4); it goes with the record.
    projection: Option<Projection>,
    /// Its subnet places, empty while `topology` is `None`.
    seats: Vec<Seat>,
    /// Its places that hold nets ([`holding`]), empty while `topology` is
    /// `None`.
    holding: Vec<PlaceId>,
    /// For a subnet, the net around it; `None` for a net a token carries.
    around: Option<Around>,
}

impl Inner {
    /// A record for `net`, found at `path`, a subnet of the net `around`
    /// says or a token's net for `None`. `silent` records its fireable set
    /// as already known, as a `load` does for the outer net; otherwise the
    /// first non-empty set is announced.
    fn new(net: Option<&Net>, path: Vec<PathStep>, silent: bool, around: Option<Around>) -> Inner {
        let mut inner = Inner {
            marking_version: 0,
            topology: None,
            names: Names::default(),
            progress: ProgressKept::default(),
            last_fireable: Vec::new(),
            template: None,
            last_path: path,
            projection: None,
            seats: Vec::new(),
            holding: Vec::new(),
            around,
        };
        if let Some(net) = net {
            inner.see(net);
            if silent {
                inner.last_fireable = fireable_names(net, &inner.names);
            }
        }
        inner
    }

    /// Takes the shape of a net heard of before it was seen.
    fn see(&mut self, net: &Net) {
        if self.topology.is_none() {
            let topology = net.topology();
            self.names = Names::new(&topology);
            self.holding = holding(&topology);
            self.topology = Some(topology);
            self.template = net.template().map(str::to_owned);
            self.seats = seats_of(net);
        }
    }
}

/// The places of a net that hold nets, by its topology: its nested places,
/// its subnet places, and the places it has for ports that hold nets. Only
/// a change at one of these can bring an inner net into the marking or
/// take one out of it.
fn holding(topology: &Topology) -> Vec<PlaceId> {
    topology
        .places
        .iter()
        .filter(|place| place.nested || place.subnet)
        .map(|place| place.id)
        .collect()
}

/// One log entry and, off the wire, the nested ids of the nets it happened
/// in, outer to inner: empty for a firing of the outer net. `get_log` with a
/// `path` or `nested_id` keeps the entries whose chain holds that net.
struct Logged {
    entry: LogEntry,
    chain: Vec<NetId>,
}

/// The one firing that is airborne, or running inline.
struct InFlight {
    transition: TransitionId,
    name: String,
    started_at: SystemTime,
    /// The `step` request waiting for this firing to land, if one launched it.
    step_id: Option<String>,
    /// What the launch took, read off its `Launched` event: a detachable
    /// firing's launch is drained before it lands.
    consumed: Vec<(PlaceId, TokenId)>,
    /// For a body that ran inside `launch`, how long that call took: its
    /// `launched` and `distributed` are reported as one `fired`, which
    /// carries this time. `None` for a detachable body, which runs
    /// elsewhere and is timed from `started_at` to its landing.
    inline: Option<Duration>,
}

impl InFlight {
    /// How long the body took, for a landing at `now`.
    fn duration_ms(&self, now: SystemTime) -> f64 {
        self.inline
            .map_or_else(|| duration_ms(self.started_at, now), millis)
    }
}

/// One net's progress (PROTOCOL.md §5.4) besides its `step_count`, which is
/// the core's `fired_count`: what the net's own transitions did, by their
/// own names. A firing in an inner net is that net's progress alone.
#[derive(Default)]
struct ProgressKept {
    last_fired: Option<String>,
    fired_counts: BTreeMap<String, u64>,
    last_progress_at: Option<SystemTime>,
}

impl ProgressKept {
    fn fired(&mut self, transition: &str, now: SystemTime) {
        self.last_fired = Some(transition.to_owned());
        *self.fired_counts.entry(transition.to_owned()).or_default() += 1;
        self.last_progress_at = Some(now);
    }
}

impl std::fmt::Debug for Owner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Owner")
            .field("nets", &self.nets)
            .field("generation", &self.generation)
            .field("marking_version", &self.marking_version)
            .field("loaded", &self.loaded.as_ref().map(|l| l.name))
            .field("listening", &self.listening)
            .finish_non_exhaustive()
    }
}

impl Owner {
    /// An owner serving `nets`, with nothing loaded.
    ///
    /// `status` is the heartbeat thread's view: the owner sets it to the
    /// transition and the `now` of each launch of the loaded net's own,
    /// before the body can start, and clears it when the firing lands. For a
    /// firing inside an inner net, which a deep step makes inline, it sets
    /// it to the transition, the `now` of the step and the net's
    /// [`InnerAddress`] just before the body starts, and clears it when the
    /// step returns, the body having fired or failed. A step or a launch
    /// that the net's own code panics in (a selector, a priority, a guard)
    /// clears it too, and is answered as an error, the run stopping, while
    /// the owner goes on serving. `heartbeat` is the period the caller
    /// should run its heartbeat and ticker at; see
    /// [`heartbeat`](Owner::heartbeat).
    pub fn new(nets: Vec<NetEntry>, status: StatusCell, heartbeat: Duration) -> Self {
        Owner {
            nets,
            status,
            gauges: Arc::default(),
            heartbeat,
            generation: 0,
            marking_version: 0,
            loaded: None,
            poke: false,
            hosted: false,
            host_flights: Vec::new(),
            listening: true,
        }
    }

    /// The heartbeat and tick period this owner was made with.
    pub fn heartbeat(&self) -> Duration {
        self.heartbeat
    }

    /// The `describe` answer: the protocol version, `"rust"`, this crate's
    /// version, and the nets. Needs no loaded net.
    pub fn describe(&self) -> Describe {
        Describe {
            protocol_version: protocol::PROTOCOL_VERSION,
            runtime: protocol::RUNTIME.to_owned(),
            runtime_version: env!("CARGO_PKG_VERSION").to_owned(),
            nets: self.nets.iter().map(NetEntry::describe).collect(),
        }
    }

    /// Whether a run fired an inline (mutate-edge) body, or a firing inside
    /// an inner net, in the last [`handle`](Owner::handle) and is free to
    /// fire again at once.
    ///
    /// An inline body lands inside `handle`, so no [`Input::Completed`] will
    /// come to prompt the next firing. A caller that sees `true` should feed
    /// an [`Input::Tick`] once every input already waiting has been handled,
    /// rather than wait for the ticker. Feeding it after them, instead of
    /// looping inside `handle`, is what lets a `stop` that arrived meanwhile
    /// be heard before the net fires again; [`main`](crate::main) holds the
    /// ticker's ticks back in the same way, taking one only when nothing
    /// else waits.
    pub fn wants_tick(&self) -> bool {
        self.poke
            && self
                .loaded
                .as_ref()
                .is_some_and(|l| l.running && l.in_flight.is_none())
    }

    pub(crate) fn gauges(&self) -> Arc<Gauges> {
        Arc::clone(&self.gauges)
    }

    /// Handles one input at time `now` and returns what to do, in order.
    pub fn handle(&mut self, input: Input, now: SystemTime) -> Vec<Outgoing> {
        let mut out = Vec::new();
        self.poke = false;
        match input {
            Input::Request(request) => self.request(request, now, &mut out),
            Input::Completed(completed) => self.completed(completed, now, &mut out),
            Input::Tick => self.drive(now, &mut out),
        }
        self.mirror_gauges();
        out
    }

    /// Copies whether the net runs into its gauge, just before a body runs
    /// inline: the heartbeat thread reads it while the body runs, and the
    /// first body of a run runs in the `handle` that started it, before
    /// [`mirror_gauges`](Owner::mirror_gauges) would copy it. A heartbeat's
    /// version stays that of the lines written so far, which the writer
    /// keeps.
    fn mirror_running(&self) {
        self.gauges.running.store(
            self.loaded.as_ref().is_some_and(|l| l.running),
            Ordering::Relaxed,
        );
    }

    /// Copies what the heartbeat thread reads into the gauges.
    fn mirror_gauges(&self) {
        self.mirror_running();
    }

    /// Whether a net is loaded: a heartbeat is written only then.
    pub(crate) fn is_loaded(&self) -> bool {
        self.loaded.is_some()
    }

    fn loaded_ref(&self) -> &Loaded {
        self.loaded.as_ref().expect("checked by serve")
    }

    fn loaded_mut(&mut self) -> &mut Loaded {
        self.loaded.as_mut().expect("checked by serve")
    }
}

impl Loaded {
    fn push_log(&mut self, entry: LogEntry, chain: Vec<NetId>) {
        if self.log.len() == LOG_CAPACITY {
            self.log.pop_front();
        }
        self.log.push_back(Logged { entry, chain });
    }
}

/// An error answer to a request (PROTOCOL.md §3): one of the protocol's
/// codes, and a message for a human. A handler that fails returns one, and
/// [`respond_error`] writes it.
#[derive(Debug)]
struct RpcError {
    code: ErrorCode,
    message: String,
}

impl RpcError {
    /// The answer with this code and message.
    fn new(code: ErrorCode, message: impl Into<String>) -> RpcError {
        RpcError {
            code,
            message: message.into(),
        }
    }

    /// The answer to an error of the core's or a factory's: the code
    /// [`code_of`] gives it, and the error's `Display` as the message.
    fn core(error: CoreError<'_>) -> RpcError {
        let message = match error {
            CoreError::Command(error) => error.to_string(),
            CoreError::Net(error) => error.to_string(),
            CoreError::Load(error) => error.to_string(),
            CoreError::Filter(error) => error.to_string(),
            CoreError::Press(error) => error.to_string(),
        };
        RpcError::new(code_of(error), message)
    }
}

// ----- helpers ---------------------------------------------------------------

/// A notification's line. One that cannot be written as JSON costs that
/// line and not the owner: it is left out and said on stderr, as the
/// Python runner does, and a host that counts versions meets the gap as
/// any gap (PROTOCOL.md §4).
fn push(out: &mut Vec<Outgoing>, notification: Notification) {
    match notification.to_line() {
        Ok(line) => out.push(Outgoing::Line(line)),
        Err(error) => eprintln!(
            "petritype-runner: a {:?} notification is not JSON ({error}); left out",
            notification.method()
        ),
    }
}

/// The answer to request `id`. One that cannot be written as JSON is
/// answered with an `internal` error instead, said on stderr, as the
/// Python runner does.
fn respond(out: &mut Vec<Outgoing>, id: &str, result: impl serde::Serialize) {
    match Response::result(id, result).and_then(|response| response.to_line()) {
        Ok(line) => out.push(Outgoing::Line(line)),
        Err(error) => {
            eprintln!(
                "petritype-runner: the answer to request {id:?} is not JSON ({error}); answered \
                 with an internal error"
            );
            respond_error(
                out,
                id,
                RpcError::new(
                    ErrorCode::Internal,
                    format!("the answer could not be written as JSON: {error}"),
                ),
            );
        }
    }
}

fn respond_error(out: &mut Vec<Outgoing>, id: &str, error: RpcError) {
    match Response::error(id, error.code, error.message).to_line() {
        Ok(line) => out.push(Outgoing::Line(line)),
        Err(error) => {
            eprintln!(
                "petritype-runner: the error answer to request {id:?} is not JSON ({error}); left out"
            );
        }
    }
}

/// A path as JSON, for a message.
fn path_text(path: &[protocol::PathStep]) -> String {
    protocol::to_line(path).unwrap_or_else(|_| format!("{path:?}"))
}

fn lock<T>(cell: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // The cell holds plain data; a panic elsewhere cannot leave it torn.
    cell.lock().unwrap_or_else(PoisonError::into_inner)
}

fn duration_ms(from: SystemTime, to: SystemTime) -> f64 {
    millis(to.duration_since(from).unwrap_or_default())
}

/// A span as the wire's `duration_ms`: milliseconds, fractions kept.
fn millis(span: Duration) -> f64 {
    span.as_secs_f64() * 1000.0
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "panic payload of unknown type".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    /// A result whose `Serialize` refuses.
    struct Unwritable;

    impl serde::Serialize for Unwritable {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("cannot be written"))
        }
    }

    #[test]
    fn an_answer_that_is_not_json_is_an_internal_error_and_the_owner_goes_on() {
        let mut out = Vec::new();
        respond(&mut out, "7", Unwritable);
        respond(&mut out, "8", json!({"status": "ok"}));
        let lines: Vec<Value> = out
            .iter()
            .map(|outgoing| match outgoing {
                Outgoing::Line(line) => serde_json::from_str(line).expect("a JSON line"),
                _ => panic!("lines only"),
            })
            .collect();
        assert_eq!(
            lines,
            [
                json!({"id": "7", "error": {"code": "internal",
                       "message": "the answer could not be written as JSON: cannot be written"}}),
                json!({"id": "8", "result": {"status": "ok"}}),
            ]
        );
        assert!(protocol::Response::result("9", Unwritable).is_err());
    }
}
