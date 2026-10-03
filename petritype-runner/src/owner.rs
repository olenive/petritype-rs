//! The state machine at the heart of the runner (DESIGN §19.3.2–§19.3.6).
//!
//! [`Owner`] owns the registry and the loaded net and turns each [`Input`]
//! into the [`Outgoing`] lines and firings it causes. It creates no threads,
//! reads no clock but the `now` it is handed, and writes nothing, so every
//! behaviour of the protocol can be tested by feeding it inputs and reading
//! what comes out.

use std::collections::{BTreeMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use petritype::{
    BodyPanicked, Completed, Event, Firing, FiringFailed, JsonCommand, Net, NetError, PlaceId,
    TokenId, Topology, TransitionId,
};
use serde_json::{Map, Value, json};

use crate::errors::{CoreError, code_of};
use crate::protocol::{
    self, Describe, ErrorCode, LogEntry, Notification, PlaceTokens, Request, Response, State,
    TokenBody, TokenValue,
};
use crate::registry::{Hosting, NetEntry, Params};
use crate::time::iso8601;
use crate::translate::{self, Names};

/// What the heartbeat thread reads: the transition whose body is running and
/// when it started, or `None` when no body runs.
pub type StatusCell = Arc<Mutex<Option<(String, SystemTime)>>>;

/// How many entries `get_log` keeps (PROTOCOL.md §8).
const LOG_CAPACITY: usize = 1000;
/// `get_state`'s and `get_place_tokens`'s default page.
const DEFAULT_TOKEN_LIMIT: u64 = 200;
/// The largest page either will send.
const MAX_TOKEN_LIMIT: u64 = 1000;

/// The requests answered at once while a body is airborne; every other one
/// waits in arrival order until it lands (DESIGN §19.3.5). `shutdown` is here
/// because it abandons the body (§19.3.6), and `describe` because it reads
/// nothing of the net.
const ANSWERED_WHILE_AIRBORNE: &[&str] = &[
    "get_state",
    "get_place_tokens",
    "get_token",
    "get_log",
    "stop",
    "shutdown",
    "describe",
];

/// Every method of protocol version 1.
const METHODS: &[&str] = &[
    "describe",
    "load",
    "get_state",
    "get_place_tokens",
    "get_token",
    "step",
    "start",
    "stop",
    "reset",
    "inject",
    "enable",
    "disable",
    "set_param",
    "get_log",
    "shutdown",
];

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

/// What the heartbeat thread needs besides the status cell, mirrored after
/// every [`Owner::handle`].
#[derive(Debug, Default)]
pub(crate) struct Gauges {
    pub(crate) marking_version: AtomicU64,
    pub(crate) running: AtomicBool,
    pub(crate) loaded: AtomicBool,
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
}

/// One generation of one net, and what the runner keeps about it
/// (DESIGN §19.3.3).
struct Loaded {
    entry: usize,
    params: Params,
    net: Net,
    topology: Topology,
    names: Names,
    running: bool,
    in_flight: Option<InFlight>,
    deferred: VecDeque<Request>,
    last_fireable: Vec<String>,
    log: VecDeque<LogEntry>,
    log_seq: u64,
    progress: ProgressKept,
    idle: bool,
    /// The wire name of the parameter the `set_param` being served sets.
    pending_param: Option<String>,
}

/// The one firing that is airborne, or running inline.
struct InFlight {
    name: String,
    started_at: SystemTime,
    /// The `step` request waiting for this firing to land, if one launched it.
    step_id: Option<String>,
    /// What the launch took, read off its `Launched` event: a detachable
    /// firing's launch is drained before it lands.
    consumed: Vec<(PlaceId, TokenId)>,
    /// The body runs inside `launch`, so `launched` and `distributed` are
    /// reported as one `fired`.
    inline: bool,
}

#[derive(Default)]
struct ProgressKept {
    last_fired: Option<String>,
    fired_counts: BTreeMap<String, u64>,
    last_progress_at: Option<SystemTime>,
}

impl std::fmt::Debug for Owner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Owner")
            .field("nets", &self.nets)
            .field("generation", &self.generation)
            .field("marking_version", &self.marking_version)
            .field(
                "loaded",
                &self.loaded.as_ref().map(|l| self.nets[l.entry].name),
            )
            .finish_non_exhaustive()
    }
}

/// A request's parameters, or why they are not usable.
type Args = Map<String, Value>;

impl Owner {
    /// An owner serving `nets`, with nothing loaded.
    ///
    /// `status` is the heartbeat thread's view: the owner sets it to the
    /// transition and the `now` of each launch, before the body can start,
    /// and clears it when the firing lands. `heartbeat` is the period the
    /// caller should run its heartbeat and ticker at; see
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

    /// Whether a run fired an inline (mutate-edge) body in the last
    /// [`handle`](Owner::handle) and is free to fire again at once.
    ///
    /// An inline body lands inside `handle`, so no [`Input::Completed`] will
    /// come to prompt the next firing. A caller that sees `true` should feed
    /// an [`Input::Tick`] behind whatever input is already queued, rather
    /// than wait for the ticker; queueing it, instead of looping inside
    /// `handle`, is what lets a `stop` that arrived meanwhile be heard.
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
        self.gauges
            .marking_version
            .store(self.marking_version, Ordering::Relaxed);
        self.gauges.running.store(
            self.loaded.as_ref().is_some_and(|l| l.running),
            Ordering::Relaxed,
        );
        self.gauges
            .loaded
            .store(self.loaded.is_some(), Ordering::Relaxed);
        out
    }

    // ----- dispatch ----------------------------------------------------------

    fn request(&mut self, request: Request, now: SystemTime, out: &mut Vec<Outgoing>) {
        if let Some(loaded) = &mut self.loaded
            && loaded.in_flight.is_some()
            && !ANSWERED_WHILE_AIRBORNE.contains(&request.method.as_str())
        {
            loaded.deferred.push_back(request);
            return;
        }
        if self.serve(request, now, out) {
            return;
        }
        self.drive(now, out);
    }

    fn completed(&mut self, completed: Completed, now: SystemTime, out: &mut Vec<Outgoing>) {
        let Some(loaded) = &mut self.loaded else {
            // Nothing launched it: a load replaces the net only when nothing
            // is airborne. Dropping it strands nothing that still exists.
            return;
        };
        if loaded.in_flight.is_none() {
            return;
        }
        self.land(completed, now, out);
        if self.serve_deferred(now, out) {
            return;
        }
        self.drive(now, out);
    }

    /// Serves the requests that waited for a landing, in arrival order, until
    /// one of them launches a body again. True if one of them was `shutdown`.
    fn serve_deferred(&mut self, now: SystemTime, out: &mut Vec<Outgoing>) -> bool {
        let Some(loaded) = &mut self.loaded else {
            return false;
        };
        let mut queue = std::mem::take(&mut loaded.deferred);
        while let Some(request) = queue.pop_front() {
            if self.serve(request, now, out) {
                return true;
            }
            if let Some(loaded) = &mut self.loaded
                && loaded.in_flight.is_some()
            {
                // A deferred `step` launched again: the rest wait for it.
                queue.append(&mut loaded.deferred);
                loaded.deferred = queue;
                return false;
            }
        }
        false
    }

    /// Answers one request. True if the caller must stop: `shutdown`.
    fn serve(&mut self, request: Request, now: SystemTime, out: &mut Vec<Outgoing>) -> bool {
        let Request { id, method, params } = request;
        if !METHODS.contains(&method.as_str()) {
            respond_error(
                out,
                &id,
                ErrorCode::UnknownMethod,
                format!("no such method: {method:?}"),
            );
            return false;
        }
        let Value::Object(args) = params else {
            respond_error(
                out,
                &id,
                ErrorCode::InvalidParams,
                "params must be an object",
            );
            return false;
        };
        match method.as_str() {
            "describe" => respond(out, &id, self.describe()),
            "shutdown" => {
                respond(out, &id, json!({"status": "ok"}));
                out.push(Outgoing::Exit);
                return true;
            }
            "load" => self.load(&id, &args, out),
            _ if self.loaded.is_none() => respond_error(
                out,
                &id,
                ErrorCode::NotLoaded,
                format!("{method} needs a loaded net"),
            ),
            "reset" => self.reset(&id, out),
            _ => {
                if let Err((code, message)) = self.serve_loaded(&id, &method, &args, now, out) {
                    respond_error(out, &id, code, message);
                }
            }
        }
        false
    }

    fn serve_loaded(
        &mut self,
        id: &str,
        method: &str,
        args: &Args,
        now: SystemTime,
        out: &mut Vec<Outgoing>,
    ) -> Result<(), (ErrorCode, String)> {
        match method {
            "get_state" => {
                let limit = page_limit(args, "token_limit")?;
                let state = self.state(limit);
                respond(out, id, state);
            }
            "get_place_tokens" => {
                let place = string_arg(args, "place")?;
                let offset = usize_arg(args, "offset")?.unwrap_or(0);
                let limit = page_limit(args, "limit")?;
                let loaded = self.loaded_ref();
                let place_id = loaded.net.place_named(place).ok_or_else(|| {
                    (ErrorCode::UnknownPlace, format!("no place named {place:?}"))
                })?;
                let count = loaded.net.count(place_id);
                let tokens = translate::tokens(&loaded.net, place_id, offset, limit);
                let page = PlaceTokens {
                    place: place.to_owned(),
                    marking_version: self.marking_version,
                    count,
                    offset,
                    limit,
                    has_more: offset.saturating_add(tokens.len()) < count,
                    tokens,
                };
                respond(out, id, page);
            }
            "get_token" => {
                let token = string_arg(args, "id")?;
                let value = self.token(token)?;
                respond(out, id, value);
            }
            "get_log" => {
                let after = u64_arg(args, "after")?.unwrap_or(0);
                let limit = u64_arg(args, "limit")?
                    .unwrap_or(LOG_CAPACITY as u64)
                    .clamp(1, LOG_CAPACITY as u64) as usize;
                let entries: Vec<&LogEntry> = self
                    .loaded_ref()
                    .log
                    .iter()
                    .filter(|entry| entry.seq > after)
                    .take(limit)
                    .collect();
                respond(out, id, json!({ "entries": entries }));
            }
            "step" => self.step(id, now, out)?,
            "start" => {
                let loaded = self.loaded_mut();
                loaded.running = true;
                loaded.idle = false;
                respond(
                    out,
                    id,
                    json!({"status": "started", "marking_version": self.marking_version, "running": true}),
                );
            }
            "stop" => {
                self.loaded_mut().running = false;
                respond(
                    out,
                    id,
                    json!({"status": "stopped", "marking_version": self.marking_version, "running": false}),
                );
            }
            "inject" => {
                let place = string_arg(args, "place")?.to_owned();
                let token = args.get("token").cloned().ok_or_else(|| {
                    (
                        ErrorCode::InvalidParams,
                        "inject needs a \"token\"".to_owned(),
                    )
                })?;
                self.apply(JsonCommand::Extend {
                    place,
                    tokens: vec![token],
                })?;
                let events = self.flush_events(now, None, true, out);
                let added = events.iter().find_map(|event| match event {
                    Event::Added { token, .. } => Some(token.to_string()),
                    _ => None,
                });
                let added = added.ok_or_else(|| {
                    (
                        ErrorCode::Internal,
                        "the token was added without an event".to_owned(),
                    )
                })?;
                respond(
                    out,
                    id,
                    json!({"status": "injected", "id": added, "marking_version": self.marking_version}),
                );
            }
            "enable" | "disable" => {
                let transition = string_arg(args, "transition")?.to_owned();
                self.apply(if method == "enable" {
                    JsonCommand::Enable { transition }
                } else {
                    JsonCommand::Disable { transition }
                })?;
                self.flush_events(now, None, true, out);
                respond(out, id, json!({"marking_version": self.marking_version}));
            }
            "set_param" => {
                let param = string_arg(args, "param")?.to_owned();
                let value = args.get("value").cloned().ok_or_else(|| {
                    (
                        ErrorCode::InvalidParams,
                        "set_param needs a \"value\"".to_owned(),
                    )
                })?;
                self.loaded_mut().pending_param = Some(param.clone());
                let applied = self.apply(JsonCommand::SetParam { param, value });
                if applied.is_ok() {
                    self.flush_events(now, None, true, out);
                }
                self.loaded_mut().pending_param = None;
                applied?;
                respond(out, id, json!({"marking_version": self.marking_version}));
            }
            _ => {
                return Err((
                    ErrorCode::Internal,
                    format!("{method} has no handler in this runner"),
                ));
            }
        }
        Ok(())
    }

    fn apply(&mut self, command: JsonCommand) -> Result<(), (ErrorCode, String)> {
        self.loaded_mut()
            .net
            .apply_json(command)
            .map_err(|error| (code_of(CoreError::Command(&error)), error.to_string()))
    }

    fn loaded_ref(&self) -> &Loaded {
        self.loaded.as_ref().expect("checked by serve")
    }

    fn loaded_mut(&mut self) -> &mut Loaded {
        self.loaded.as_mut().expect("checked by serve")
    }

    // ----- load and reset ----------------------------------------------------

    fn load(&mut self, id: &str, args: &Args, out: &mut Vec<Outgoing>) {
        if !args.contains_key("net")
            && ["module", "function", "factory_params"]
                .iter()
                .any(|key| args.contains_key(*key))
        {
            respond_error(
                out,
                id,
                ErrorCode::InvalidParams,
                "the {module, function} form of load is Python's; a Rust runner loads {net, params}",
            );
            return;
        }
        let name = match args.get("net") {
            Some(Value::String(name)) => name.clone(),
            _ => {
                respond_error(
                    out,
                    id,
                    ErrorCode::InvalidParams,
                    "load needs a string \"net\"",
                );
                return;
            }
        };
        let Some(entry) = self.nets.iter().position(|e| e.name == name) else {
            respond_error(
                out,
                id,
                ErrorCode::UnknownNet,
                format!("no net named {name:?}"),
            );
            return;
        };
        let given = match args.get("params") {
            None | Some(Value::Null) => Params::new(),
            Some(Value::Object(params)) => params.clone(),
            Some(_) => {
                respond_error(
                    out,
                    id,
                    ErrorCode::InvalidParams,
                    "\"params\" must be an object",
                );
                return;
            }
        };
        let params = match self.nets[entry].complete(given) {
            Ok(params) => params,
            Err(message) => {
                respond_error(out, id, ErrorCode::InvalidParams, message);
                return;
            }
        };
        let running = self.loaded.as_ref().is_some_and(|l| l.running);
        push(
            out,
            Notification::LoadProgress {
                step: "build".into(),
                message: format!("building {name}"),
                marking_version: self.marking_version,
                running,
            },
        );
        if self.build(id, entry, params, "load", out) {
            respond(
                out,
                id,
                json!({"status": "loaded", "generation": self.generation, "marking_version": self.marking_version}),
            );
        }
    }

    fn reset(&mut self, id: &str, out: &mut Vec<Outgoing>) {
        let loaded = self.loaded_ref();
        let (entry, params) = (loaded.entry, loaded.params.clone());
        if self.build(id, entry, params, "reset", out) {
            respond(
                out,
                id,
                json!({"status": "reset", "generation": self.generation, "marking_version": self.marking_version}),
            );
        }
    }

    /// Runs the factory and, if it gives a net, installs it as the next
    /// generation and writes `reloaded`. On failure writes the error
    /// response, leaves the previous net as it was, and returns false.
    fn build(
        &mut self,
        id: &str,
        entry: usize,
        params: Params,
        reason: &str,
        out: &mut Vec<Outgoing>,
    ) -> bool {
        let factory = &self.nets[entry].factory;
        let built = catch_unwind(AssertUnwindSafe(|| factory(&params)));
        let mut net = match built {
            Ok(Ok(net)) => net,
            Ok(Err(error)) => {
                respond_error(out, id, code_of(CoreError::Load(&error)), error.to_string());
                return false;
            }
            Err(payload) => {
                respond_error(
                    out,
                    id,
                    ErrorCode::LoadFailed,
                    format!("the factory panicked: {}", panic_message(payload.as_ref())),
                );
                return false;
            }
        };
        // A new generation starts from its built marking; whatever the
        // factory did to it on the way is part of that.
        net.drain_events().for_each(drop);
        let topology = net.topology();
        let names = Names::new(&topology);
        let last_fireable = net
            .fireable()
            .into_iter()
            .map(|t| names.transition(t))
            .collect();
        self.generation += 1;
        self.loaded = Some(Loaded {
            entry,
            params,
            net,
            topology,
            names,
            running: false,
            in_flight: None,
            deferred: VecDeque::new(),
            last_fireable,
            log: VecDeque::new(),
            log_seq: 0,
            progress: ProgressKept::default(),
            idle: false,
            pending_param: None,
        });
        *lock(&self.status) = None;
        push(
            out,
            Notification::Reloaded {
                generation: self.generation,
                reason: reason.to_owned(),
                marking_version: self.marking_version,
                running: false,
            },
        );
        true
    }

    // ----- reads -------------------------------------------------------------

    fn state(&self, token_limit: usize) -> State {
        let loaded = self.loaded_ref();
        let net = &loaded.net;
        let transitions = loaded
            .topology
            .transitions
            .iter()
            .map(|t| {
                let priority = catch_unwind(AssertUnwindSafe(|| net.priority_of(t.id))).ok();
                (
                    t.name.clone(),
                    protocol::TransitionState {
                        enabled: net.is_enabled(t.id),
                        fireable: net.is_fireable(t.id),
                        in_flight: net.is_in_flight(t.id),
                        priority,
                    },
                )
            })
            .collect();
        let places = loaded
            .topology
            .places
            .iter()
            .map(|p| {
                (
                    p.name.clone(),
                    protocol::PlaceMarking {
                        count: net.count(p.id),
                        tokens: translate::tokens(net, p.id, 0, token_limit),
                    },
                )
            })
            .collect();
        State {
            protocol_version: protocol::PROTOCOL_VERSION,
            runtime: protocol::RUNTIME.to_owned(),
            net: self.nets[loaded.entry].name.to_owned(),
            generation: self.generation,
            marking_version: self.marking_version,
            running: loaded.running,
            topology: translate::topology(&loaded.topology, &loaded.names),
            transitions,
            marking: protocol::Marking { places },
            progress: protocol::Progress {
                step_count: net.fired_count(),
                last_fired: loaded.progress.last_fired.clone(),
                fired_counts: loaded.progress.fired_counts.clone(),
                last_progress_at: loaded.progress.last_progress_at.map(iso8601),
            },
        }
    }

    fn token(&self, wanted: &str) -> Result<TokenValue, (ErrorCode, String)> {
        let loaded = self.loaded_ref();
        let unknown = || (ErrorCode::UnknownToken, format!("no live token {wanted:?}"));
        let number: u64 = wanted.parse().map_err(|_| unknown())?;
        let token: TokenId = serde_json::from_value(Value::from(number)).map_err(|_| unknown())?;
        let net = &loaded.net;
        let place = loaded
            .topology
            .places
            .iter()
            .find(|p| net.tokens(p.id).any(|(t, _)| t == token))
            .ok_or_else(unknown)?;
        let value = match net.json(place.id, token) {
            Some(value) => TokenBody::Json { value },
            None => TokenBody::Text {
                text: net.debug(place.id, token).unwrap_or_default(),
            },
        };
        Ok(TokenValue {
            id: wanted.to_owned(),
            place: place.name.clone(),
            type_name: place.type_name.to_owned(),
            value,
        })
    }

    // ----- the firing cycle --------------------------------------------------

    fn step(
        &mut self,
        id: &str,
        now: SystemTime,
        out: &mut Vec<Outgoing>,
    ) -> Result<(), (ErrorCode, String)> {
        match self.loaded_ref().net.select() {
            None => {
                let running = self.loaded_ref().running;
                respond(
                    out,
                    id,
                    json!({"fired": false, "marking_version": self.marking_version, "running": running}),
                );
                Ok(())
            }
            Some(transition) => self.launch(transition, Some(id.to_owned()), now, out),
        }
    }

    /// Launches `transition`. A detachable body goes out as
    /// [`Outgoing::Launch`]; an inline one has already run, and lands here.
    fn launch(
        &mut self,
        transition: TransitionId,
        step_id: Option<String>,
        now: SystemTime,
        out: &mut Vec<Outgoing>,
    ) -> Result<(), (ErrorCode, String)> {
        let loaded = self.loaded_mut();
        let name = loaded.names.transition(transition);
        *lock(&self.status) = Some((name.clone(), now));
        let loaded = self.loaded_mut();
        let firing = match loaded.net.launch(transition) {
            Ok(firing) => firing,
            Err(error) => {
                *lock(&self.status) = None;
                return Err((code_of(CoreError::Net(&error)), error.to_string()));
            }
        };
        loaded.idle = false;
        let inline = !firing.is_pending();
        loaded.in_flight = Some(InFlight {
            name,
            started_at: now,
            step_id,
            consumed: Vec::new(),
            inline,
        });
        if inline {
            // `launch` ran the body; `run` only unwraps the outcome.
            self.land(firing.run(), now, out);
        } else {
            // The fireable set is compared when the firing lands, not now:
            // the launch's own change (this transition in flight) is
            // transient, and a `fireable` between `launched` and
            // `distributed` would split the pair a viewer folds into `fired`.
            let events = self.flush_events(now, None, false, out);
            let consumed = events.into_iter().find_map(|event| match event {
                Event::Launched { consumed, .. } => Some(consumed),
                _ => None,
            });
            if let Some(flight) = &mut self.loaded_mut().in_flight {
                flight.consumed = consumed.unwrap_or_default();
            }
            out.push(Outgoing::Launch(firing));
        }
        Ok(())
    }

    /// Distributes a firing that has run, and reports everything its landing
    /// means: the events, the log entry, progress, the fireable set, a stop
    /// on failure, and the `step` that was waiting for it.
    fn land(&mut self, completed: Completed, now: SystemTime, out: &mut Vec<Outgoing>) {
        let loaded = self.loaded_mut();
        let flight = loaded
            .in_flight
            .take()
            .expect("land is only called with a firing in flight");
        let result = loaded.net.distribute(completed);
        *lock(&self.status) = None;
        let events = self.flush_events(now, Some(&flight), true, out);

        let consumed = events.iter().find_map(|event| match event {
            Event::Launched { consumed, .. } | Event::Failed { consumed, .. } => {
                Some(consumed.clone())
            }
            _ => None,
        });
        let produced = events.iter().find_map(|event| match event {
            Event::Distributed { produced, .. } => Some(produced.clone()),
            _ => None,
        });
        let failure = match &result {
            Ok(()) => None,
            Err(NetError::FiringFailed(failed)) => Some(Failure::of(failed)),
            Err(other) => Some(Failure {
                error_type: None,
                message: other.to_string(),
                restored: false,
            }),
        };

        let duration_ms = duration_ms(flight.started_at, now);
        let loaded = self.loaded_mut();
        loaded.log_seq += 1;
        let consumed = consumed.unwrap_or(flight.consumed);
        let entry = LogEntry {
            seq: loaded.log_seq,
            transition: flight.name.clone(),
            started_at: iso8601(flight.started_at),
            duration_ms,
            outcome: if failure.is_some() { "failed" } else { "fired" }.to_owned(),
            consumed: loaded.names.pairs(&consumed),
            produced: loaded.names.pairs(&produced.unwrap_or_default()),
            error: failure.as_ref().map(|f| f.message.clone()),
            restored: failure.as_ref().map(|f| f.restored),
        };
        if loaded.log.len() == LOG_CAPACITY {
            loaded.log.pop_front();
        }
        loaded.log.push_back(entry);
        if failure.is_none() {
            loaded.progress.last_fired = Some(flight.name.clone());
            *loaded
                .progress
                .fired_counts
                .entry(flight.name.clone())
                .or_default() += 1;
            loaded.progress.last_progress_at = Some(now);
        }

        if let Some(failure) = &failure
            && loaded.running
        {
            loaded.running = false;
            push(
                out,
                Notification::ExecutionStopped {
                    reason: "transition_failed".into(),
                    error_type: failure.error_type.map(str::to_owned),
                    message: Some(failure.message.clone()),
                    marking_version: self.marking_version,
                    running: false,
                },
            );
        }

        if let Some(step_id) = flight.step_id {
            match result {
                Ok(()) | Err(NetError::FiringFailed(_)) => {
                    let running = self.loaded_ref().running;
                    respond(
                        out,
                        &step_id,
                        json!({
                            "fired": true,
                            "transition": flight.name,
                            "marking_version": self.marking_version,
                            "running": running,
                        }),
                    );
                }
                Err(other) => respond_error(out, &step_id, ErrorCode::Internal, other.to_string()),
            }
        }
    }

    /// Keeps a running net going: launches the next firing if it is free, or
    /// handles quiescence — `idle` once per spell for a `"24/7"` net,
    /// `execution_stopped` for any other.
    fn drive(&mut self, now: SystemTime, out: &mut Vec<Outgoing>) {
        let Some(loaded) = &self.loaded else {
            return;
        };
        if !loaded.running || loaded.in_flight.is_some() {
            return;
        }
        let continuous = self.nets[loaded.entry].hosting == Hosting::Continuous;
        match loaded.net.select() {
            Some(transition) => {
                if let Err((code, message)) = self.launch(transition, None, now, out) {
                    // `select` chose it, so `launch` cannot refuse it; if it
                    // somehow does, stop rather than spin.
                    self.loaded_mut().running = false;
                    push(
                        out,
                        Notification::ExecutionStopped {
                            reason: "transition_failed".into(),
                            error_type: Some("error".into()),
                            message: Some(format!("{code}: {message}")),
                            marking_version: self.marking_version,
                            running: false,
                        },
                    );
                    return;
                }
                let loaded = self.loaded_ref();
                self.poke = loaded.running && loaded.in_flight.is_none();
            }
            None if continuous => {
                if !loaded.idle {
                    self.loaded_mut().idle = true;
                    push(
                        out,
                        Notification::Idle {
                            idle_since: iso8601(now),
                            marking_version: self.marking_version,
                            running: true,
                        },
                    );
                }
            }
            None => {
                self.loaded_mut().running = false;
                push(
                    out,
                    Notification::ExecutionStopped {
                        reason: "no_fireable_transitions".into(),
                        error_type: None,
                        message: None,
                        marking_version: self.marking_version,
                        running: false,
                    },
                );
            }
        }
    }

    // ----- events ------------------------------------------------------------

    /// Drains the net's log, writes one notification per event (each state
    /// notification raising `marking_version` by one), and then, if asked
    /// and anything happened, the `fireable` set if it changed. Returns the
    /// events for the caller to read ids off.
    fn flush_events(
        &mut self,
        now: SystemTime,
        flight: Option<&InFlight>,
        compare_fireable: bool,
        out: &mut Vec<Outgoing>,
    ) -> Vec<Event> {
        let loaded = self.loaded.as_mut().expect("events need a net");
        let events: Vec<Event> = loaded.net.drain_events().collect();
        let running = loaded.running;
        let names = &loaded.names;
        let net = &loaded.net;
        let inline = flight.is_some_and(|f| f.inline);
        let duration = flight.map_or(0.0, |f| duration_ms(f.started_at, now));
        let mut inline_consumed: Vec<(PlaceId, TokenId)> = Vec::new();
        for event in &events {
            let version = self.marking_version + 1;
            let notification = match event {
                Event::Fired {
                    transition,
                    consumed,
                    produced,
                } => Notification::Fired {
                    transition: names.transition(*transition),
                    consumed: names.pairs(consumed),
                    produced: names.produced(net, produced),
                    duration_ms: duration,
                    marking_version: version,
                    running,
                },
                Event::Launched { consumed, .. } if inline => {
                    // Reported with the landing, as one `fired`.
                    inline_consumed.clone_from(consumed);
                    continue;
                }
                Event::Launched {
                    transition,
                    consumed,
                } => Notification::Launched {
                    transition: names.transition(*transition),
                    consumed: names.pairs(consumed),
                    marking_version: version,
                    running,
                },
                Event::Distributed {
                    transition,
                    produced,
                } if inline => Notification::Fired {
                    transition: names.transition(*transition),
                    consumed: names.pairs(&inline_consumed),
                    produced: names.produced(net, produced),
                    duration_ms: duration,
                    marking_version: version,
                    running,
                },
                Event::Distributed {
                    transition,
                    produced,
                } => Notification::Distributed {
                    transition: names.transition(*transition),
                    produced: names.produced(net, produced),
                    duration_ms: duration,
                    marking_version: version,
                    running,
                },
                Event::Failed {
                    transition,
                    consumed,
                    restored,
                    error,
                } => Notification::Failed {
                    transition: names.transition(*transition),
                    error: error.clone(),
                    restored: *restored,
                    consumed: names.pairs(consumed),
                    marking_version: version,
                    running,
                },
                Event::Added { place, token } => Notification::Added {
                    place: names.place(*place),
                    id: token.to_string(),
                    label: translate::label(net, *place, *token),
                    marking_version: version,
                    running,
                },
                Event::Removed { place, token } => Notification::Removed {
                    place: names.place(*place),
                    id: token.to_string(),
                    marking_version: version,
                    running,
                },
                Event::Mutated { place, token } => Notification::Mutated {
                    place: names.place(*place),
                    id: token.to_string(),
                    label: net.debug(*place, *token).map(translate::cut),
                    marking_version: version,
                    running,
                },
                Event::Toggled {
                    transition,
                    enabled,
                } => Notification::Toggled {
                    transition: names.transition(*transition),
                    enabled: *enabled,
                    marking_version: version,
                    running,
                },
                Event::ParamChanged { type_name } => Notification::ParamChanged {
                    param: loaded
                        .pending_param
                        .clone()
                        .unwrap_or_else(|| (*type_name).to_owned()),
                    marking_version: version,
                    running,
                },
                // An event this runner does not know: say the marking moved
                // in a way it cannot describe.
                _ => Notification::Reloaded {
                    generation: self.generation,
                    reason: "resync".into(),
                    marking_version: self.marking_version,
                    running,
                },
            };
            if notification.is_state() {
                self.marking_version = version;
            }
            push(out, notification);
        }
        if compare_fireable && !events.is_empty() {
            let fireable: Vec<String> = net
                .fireable()
                .into_iter()
                .map(|t| names.transition(t))
                .collect();
            if fireable != loaded.last_fireable {
                loaded.last_fireable.clone_from(&fireable);
                push(
                    out,
                    Notification::Fireable {
                        transitions: fireable,
                        marking_version: self.marking_version,
                        running,
                    },
                );
            }
        }
        events
    }
}

/// How a landing failed, for `execution_stopped` and the log.
struct Failure {
    error_type: Option<&'static str>,
    message: String,
    restored: bool,
}

impl Failure {
    fn of(failed: &FiringFailed) -> Failure {
        let panicked = failed.error.downcast_ref::<BodyPanicked>().is_some();
        Failure {
            error_type: Some(if panicked { "panic" } else { "error" }),
            message: failed.error.to_string(),
            restored: failed.restored,
        }
    }
}

// ----- helpers ---------------------------------------------------------------

fn push(out: &mut Vec<Outgoing>, notification: Notification) {
    out.push(Outgoing::Line(notification.to_line()));
}

fn respond(out: &mut Vec<Outgoing>, id: &str, result: impl serde::Serialize) {
    out.push(Outgoing::Line(Response::result(id, result).to_line()));
}

fn respond_error(out: &mut Vec<Outgoing>, id: &str, code: ErrorCode, message: impl Into<String>) {
    out.push(Outgoing::Line(Response::error(id, code, message).to_line()));
}

fn lock<T>(cell: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // The cell holds plain data; a panic elsewhere cannot leave it torn.
    cell.lock().unwrap_or_else(PoisonError::into_inner)
}

fn duration_ms(from: SystemTime, to: SystemTime) -> f64 {
    to.duration_since(from).unwrap_or_default().as_secs_f64() * 1000.0
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

fn string_arg<'a>(args: &'a Args, key: &str) -> Result<&'a str, (ErrorCode, String)> {
    match args.get(key) {
        Some(Value::String(value)) => Ok(value),
        _ => Err((ErrorCode::InvalidParams, format!("needs a string {key:?}"))),
    }
}

fn u64_arg(args: &Args, key: &str) -> Result<Option<u64>, (ErrorCode, String)> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value.as_u64().map(Some).ok_or_else(|| {
            (
                ErrorCode::InvalidParams,
                format!("{key:?} must be a non-negative integer"),
            )
        }),
    }
}

fn usize_arg(args: &Args, key: &str) -> Result<Option<usize>, (ErrorCode, String)> {
    Ok(u64_arg(args, key)?.map(|n| usize::try_from(n).unwrap_or(usize::MAX)))
}

/// A page size: 200 by default, clamped to 1..=1000.
fn page_limit(args: &Args, key: &str) -> Result<usize, (ErrorCode, String)> {
    Ok(u64_arg(args, key)?
        .unwrap_or(DEFAULT_TOKEN_LIMIT)
        .clamp(1, MAX_TOKEN_LIMIT) as usize)
}
