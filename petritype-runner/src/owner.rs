//! The state machine at the heart of the runner (DESIGN §19.3.2–§19.3.6,
//! §19.3.11 for nested nets, and §19.3.12 for keys, queries and the
//! connection's projection).
//!
//! [`Owner`] owns the registry and the loaded net and turns each [`Input`]
//! into the [`Outgoing`] lines and firings it causes. It creates no threads,
//! reads no clock but the `now` it is handed, and writes nothing, so every
//! behaviour of the protocol can be tested by feeding it inputs and reading
//! what comes out.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use petritype::{
    BodyPanicked, Completed, Event, Filter, Firing, FiringFailed, JsonCommand, Net, NetError,
    NetId, PlaceId, TokenId, Topology, TransitionId,
};
use serde_json::{Map, Value, json};

use crate::errors::{CoreError, code_of};
use crate::projection::{Projection, Seen};
use crate::protocol::{
    self, Describe, DistinctResult, ErrorCode, HiddenKind, LogEntry, Notification, PlaceTokens,
    QueryMatch, QueryResult, Request, Response, State, TokenBody, TokenValue,
};
use crate::registry::{Hosting, NetEntry, Params};
use crate::time::iso8601;
use crate::translate::{self, FilterError, Names};

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
/// nothing of the net. `query` and `distinct` are reads (PROTOCOL.md §13.3);
/// `set_filter` is not, and waits, so that a firing is projected through one
/// filter from its `launched` to its landing.
const ANSWERED_WHILE_AIRBORNE: &[&str] = &[
    "get_state",
    "get_place_tokens",
    "get_token",
    "get_log",
    "query",
    "distinct",
    "stop",
    "shutdown",
    "describe",
];

/// The reads that a `path` or a `nested_id` may point at an inner net
/// (PROTOCOL.md §12.3). `inject` and `set_filter` take one too, but are not
/// reads: one changes the net it names, the other the connection's view of
/// it, and both wait for an airborne body to land.
const ADDRESSED_READS: &[&str] = &[
    "get_state",
    "get_place_tokens",
    "get_token",
    "get_log",
    "query",
    "distinct",
];

/// The methods that act on the outer net alone (PROTOCOL.md §12.3): a `path`
/// or a `nested_id` on one of them is `invalid_params`. An inner net's
/// switches and parameters are its template's, or an outer body's to set.
const OUTER_ONLY: &[&str] = &[
    "step",
    "start",
    "stop",
    "reset",
    "enable",
    "disable",
    "set_param",
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
    "query",
    "distinct",
    "set_filter",
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
    /// The entry's name: what `get_state` calls the net, and the first
    /// segment of every path (PROTOCOL.md §12.1).
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
    /// compared, in the order they first had them.
    touched: Vec<NetId>,
    /// The connection's projection of the outer net (PROTOCOL.md §13.4),
    /// while `set_filter` has set one. A generation's own: `load` and
    /// `reset` clear it.
    projection: Option<Projection>,
}

/// What the runner keeps about one inner net (PROTOCOL.md §12.3,
/// DESIGN §19.3.11): what it keeps about the outer net, per net.
///
/// Made when the net is first seen — at `load` and `reset` by walking the
/// marking, when a request names it, or when one of its events arrives —
/// and dropped at the first landing (or inner firing) after which its
/// nested id no longer resolves: the token that held it was consumed and
/// not put back.
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
    last_path: String,
    /// The connection's projection of this net, while `set_filter` named it
    /// and set one (PROTOCOL.md §13.4); it goes with the record.
    projection: Option<Projection>,
}

impl Inner {
    /// A record for `net`, found at `path`. `silent` records its fireable
    /// set as already known, as a `load` does for the outer net; otherwise
    /// the first non-empty set is announced.
    fn new(net: Option<&Net>, path: String, silent: bool) -> Inner {
        let mut inner = Inner {
            marking_version: 0,
            topology: None,
            names: Names::default(),
            progress: ProgressKept::default(),
            last_fireable: Vec::new(),
            template: None,
            last_path: path,
            projection: None,
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
            self.topology = Some(topology);
            self.template = net.template().map(str::to_owned);
        }
    }
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
    /// The inner firings counted here besides the net's own, which its
    /// `fired_count` holds: the outer net's count of the firings the runner
    /// made inside its tokens.
    inside: u64,
}

impl ProgressKept {
    fn fired(&mut self, transition: &str, now: SystemTime) {
        self.last_fired = Some(transition.to_owned());
        *self.fired_counts.entry(transition.to_owned()).or_default() += 1;
        self.last_progress_at = Some(now);
    }

    /// An inner firing, counted by the outer net under `key`: its path
    /// without the outer net's name, then the transition
    /// (`Depot/#17/cabin/Sit`), as the Python runner counts it.
    fn fired_inside(&mut self, key: &str, now: SystemTime) {
        self.inside += 1;
        self.fired(key, now);
    }
}

impl std::fmt::Debug for Owner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Owner")
            .field("nets", &self.nets)
            .field("generation", &self.generation)
            .field("marking_version", &self.marking_version)
            .field("loaded", &self.loaded.as_ref().map(|l| l.name))
            .finish_non_exhaustive()
    }
}

/// A request's parameters, or why they are not usable.
type Args = Map<String, Value>;

// ----- addressing inner nets (PROTOCOL.md §12.1) ------------------------------

/// One step down from a net to one of its inner nets: the nested place, the
/// token in it, and which of the token's nets, by its position in
/// `Nested::nets` (names need not be unique; positions are).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Level {
    place: PlaceId,
    token: TokenId,
    index: usize,
}

/// An inner net found in the live marking: how to get there, its path as the
/// marking stands, its nested id and its name within its token.
#[derive(Debug, Clone)]
struct Found {
    levels: Vec<Level>,
    path: String,
    nested_id: NetId,
    name: String,
}

/// The net a request names: the outer one, or an inner one.
enum Target {
    Outer,
    Inner(Found),
}

/// What [`walk`] calls with each inner net: the levels down to it, its path,
/// its name within its token, and the net. `Some` stops the walk.
type Visit<'v, 'a, R> = dyn FnMut(&[Level], &str, &str, &'a Net) -> Option<R> + 'v;

/// `[place, token]` pairs of one net, as its events carry them.
type Pairs = Vec<(PlaceId, TokenId)>;

/// Calls `visit` with every inner net below `net`, at every depth and in
/// passive places too, in the order `step_deep` walks them (nested places in
/// place order, tokens oldest first, a token's nets in order, a net's own
/// inner nets straight after it), until `visit` answers `Some`.
fn walk<'a, R>(
    net: &'a Net,
    levels: &mut Vec<Level>,
    path: &str,
    visit: &mut Visit<'_, 'a, R>,
) -> Option<R> {
    for (place, info) in net.places() {
        if !info.nested {
            continue;
        }
        for (token, _) in net.tokens(place) {
            let Some(nets) = net.nested(place, token) else {
                continue;
            };
            for (index, (name, inner)) in nets.into_iter().enumerate() {
                levels.push(Level {
                    place,
                    token,
                    index,
                });
                let here = translate::level(path, info.name, token, name);
                let found =
                    visit(levels, &here, name, inner).or_else(|| walk(inner, levels, &here, visit));
                levels.pop();
                if found.is_some() {
                    return found;
                }
            }
        }
    }
    None
}

/// The first inner net below `outer`, at any depth, that `wanted` picks.
fn find<'a>(
    outer: &'a Net,
    outer_name: &str,
    wanted: impl Fn(&Net) -> bool,
) -> Option<(Found, &'a Net)> {
    walk(
        outer,
        &mut Vec::new(),
        outer_name,
        &mut |levels, path, name, net| {
            wanted(net).then(|| {
                let found = Found {
                    levels: levels.to_vec(),
                    path: path.to_owned(),
                    nested_id: net.id(),
                    name: name.to_owned(),
                };
                (found, net)
            })
        },
    )
}

/// The inner net a wire path names, or `None` for a path that is the loaded
/// net's name alone, which names the outer net. The error says which
/// segment failed.
fn resolve_path<'a>(
    outer: &'a Net,
    outer_name: &str,
    path: &str,
) -> Result<Option<(Found, &'a Net)>, String> {
    let mut segments = path.split('/');
    if segments.next() != Some(outer_name) {
        return Err(format!(
            "path {path:?} does not start with the loaded net's name {outer_name:?}"
        ));
    }
    let rest: Vec<&str> = segments.collect();
    if rest.is_empty() {
        return Ok(None);
    }
    let mut net = outer;
    let mut levels = Vec::new();
    let mut here = outer_name.to_owned();
    let mut name = "";
    for level in rest.chunks(3) {
        let &[place_name, token_segment, net_name] = level else {
            return Err(format!(
                "path {path:?} stops after {:?}: each level is a place, a #token and a net",
                level.last().copied().unwrap_or_default()
            ));
        };
        let place = net
            .place_named(place_name)
            .ok_or_else(|| format!("path {path:?}: {here:?} has no place {place_name:?}"))?;
        let token = token_segment
            .strip_prefix('#')
            .and_then(|id| id.parse::<TokenId>().ok())
            .ok_or_else(|| {
                format!("path {path:?}: {token_segment:?} is not a token, which is written #id")
            })?;
        let nets = net.nested(place, token).ok_or_else(|| {
            if net.tokens(place).any(|(t, _)| t == token) {
                format!("path {path:?}: token #{token} in {place_name:?} holds no nets")
            } else {
                format!("path {path:?}: there is no token #{token} in {place_name:?}")
            }
        })?;
        let index = nets
            .iter()
            .position(|(n, _)| *n == net_name)
            .ok_or_else(|| {
                format!("path {path:?}: token #{token} in {place_name:?} holds no net {net_name:?}")
            })?;
        here = translate::level(&here, place_name, token, net_name);
        levels.push(Level {
            place,
            token,
            index,
        });
        net = nets[index].1;
        name = net_name;
    }
    let found = Found {
        levels,
        path: here,
        nested_id: net.id(),
        name: name.to_owned(),
    };
    Ok(Some((found, net)))
}

/// The net `levels` lead to from `net`, as the marking stands.
fn descend<'a>(net: &'a Net, levels: &[Level]) -> Option<&'a Net> {
    levels.iter().try_fold(net, |here, level| {
        here.nested(level.place, level.token)?
            .into_iter()
            .nth(level.index)
            .map(|(_, inner)| inner)
    })
}

/// Calls `f` with the net `levels` lead to, mutably, through
/// `Net::nested_scope` at every level, so that what `f` did there is lifted
/// level by level into the outer log as each scope closes. `None` if the
/// levels lead nowhere.
fn descend_mut<R>(net: &mut Net, levels: &[Level], f: impl FnOnce(&mut Net) -> R) -> Option<R> {
    let Some((level, rest)) = levels.split_first() else {
        return Some(f(net));
    };
    net.nested_scope(level.place, level.token, |nets| {
        let (_, inner) = nets.get_mut(level.index)?;
        descend_mut(inner, rest, f)
    })
    .flatten()
}

/// An event lifted out of an inner net, unwrapped down to the net it
/// happened in: one wrapper on the wire whatever the depth (§12.4).
struct Lifted<'e> {
    /// Per level, outer to inner: the place and token in the net above as
    /// they were when the event was lifted, the net's name in the token,
    /// and its nested id.
    levels: Vec<(PlaceId, TokenId, &'e str, NetId)>,
    /// What the innermost net logged.
    event: &'e Event,
}

impl<'e> Lifted<'e> {
    /// `None` for an event of the outer net itself.
    fn of(mut event: &'e Event) -> Option<Lifted<'e>> {
        let mut levels = Vec::new();
        while let Event::Nested {
            place,
            token,
            net,
            nested_id,
            event: inner,
        } = event
        {
            levels.push((*place, *token, net.as_str(), *nested_id));
            event = inner;
        }
        (!levels.is_empty()).then_some(Lifted { levels, event })
    }

    /// The nested id of the net the event happened in.
    fn nested_id(&self) -> NetId {
        self.levels.last().expect("a lifted event has a level").3
    }

    fn chain(&self) -> Vec<NetId> {
        self.levels.iter().map(|level| level.3).collect()
    }

    /// The net it happened in, if its token is where the event says, at
    /// every level: not while a body holds it, nor once it is gone.
    fn live<'a>(&self, outer: &'a Net) -> Option<&'a Net> {
        self.levels
            .iter()
            .try_fold(outer, |here, &(place, token, _, id)| {
                here.nested(place, token)?
                    .into_iter()
                    .find(|(_, inner)| inner.id() == id)
                    .map(|(_, inner)| inner)
            })
    }
}

/// A firing inside an inner net, as a flush reported it: what a `step`
/// that made it answers with.
#[derive(Debug, Clone)]
struct InnerFiring {
    transition: String,
    path: String,
    nested_id: String,
}

/// What one flush wrote: the events, for the caller to read ids off, and
/// the inner firings among them.
struct Flushed {
    events: Vec<Event>,
    inner_firings: Vec<InnerFiring>,
}

/// The order the wire tells a batch of events in (PROTOCOL.md §12.4): what a
/// firing's body did inside the tokens it holds goes out after its
/// `launched` and before its own `mutated`s and its `distributed` (or
/// `fired`, or `failed`).
///
/// The core lifts a mutated token's inner events just before that token's
/// `Mutated` (DESIGN §6.6, rule 2) and a produced token's just after the
/// firing's `Distributed` or `Fired`, under its new place and id (rule 4).
/// Here every `Nested` of the landing comes first — those lifted for mutated
/// tokens, then those for produced ones, each in the order logged — then the
/// `Mutated`s, then the event that ends the firing. The core's own log
/// keeps its rules; only the wire is reordered.
fn wire_order(events: &[Event]) -> Vec<&Event> {
    let mut ordered: Vec<&Event> = Vec::with_capacity(events.len());
    let mut index = 0;
    while index < events.len() {
        let event = &events[index];
        index += 1;
        let produced: &[(PlaceId, TokenId)] = match event {
            Event::Fired { produced, .. } | Event::Distributed { produced, .. } => produced,
            Event::Failed { .. } => &[],
            _ => {
                ordered.push(event);
                continue;
            }
        };
        // The landing so far: the run of `Nested` and `Mutated` written
        // since the firing's `Launched` (or since the batch began).
        let start = ordered
            .iter()
            .rposition(|e| !matches!(e, Event::Nested { .. } | Event::Mutated { .. }))
            .map_or(0, |at| at + 1);
        let landing: Vec<&Event> = ordered.drain(start..).collect();
        let mut arrivals = Vec::new();
        while let Some(next @ Event::Nested { place, token, .. }) = events.get(index)
            && produced.contains(&(*place, *token))
        {
            arrivals.push(next);
            index += 1;
        }
        ordered.extend(
            landing
                .iter()
                .copied()
                .filter(|e| matches!(e, Event::Nested { .. })),
        );
        ordered.extend(arrivals);
        ordered.extend(
            landing
                .iter()
                .copied()
                .filter(|e| matches!(e, Event::Mutated { .. })),
        );
        ordered.push(event);
    }
    ordered
}

/// A net's fireable set by name, in declaration order.
fn fireable_names(net: &Net, names: &Names) -> Vec<String> {
    net.fireable()
        .into_iter()
        .map(|t| names.transition(t))
        .collect()
}

/// `produced` entries, labelled from `net` when it can be read, with their
/// keys where their place declares some, and with `""` and no keys when it
/// cannot (an inner net whose token a body holds).
fn produced(
    names: &Names,
    net: Option<&Net>,
    tokens: &[(PlaceId, TokenId)],
) -> Vec<protocol::Produced> {
    match net {
        Some(net) => names.produced(net, tokens),
        None => tokens
            .iter()
            .map(|&(place, token)| protocol::Produced {
                place: names.place(place),
                id: token.to_string(),
                label: String::new(),
                keys: None,
            })
            .collect(),
    }
}

/// A token event — `Added`, `Removed` or `Mutated` — as the notification the
/// connection is told (PROTOCOL.md §7.1, §13.4), through `projection` when
/// the net has one: a token the filter hides is told of as `hidden`, and a
/// mutation that carries a token across the filter's boundary as an `added`
/// or a `removed`, in place of its `mutated`. `net` is the net it happened
/// in, `None` when it cannot be read. `None` for any other event.
fn token_line(
    names: &Names,
    net: Option<&Net>,
    projection: Option<&mut Projection>,
    event: &Event,
    marking_version: u64,
    running: bool,
) -> Option<Notification> {
    let keys = |place, token| net.and_then(|net| translate::keys(net, place, token));
    let hidden = |kind, place| Notification::Hidden {
        kind,
        place: names.place(place),
        marking_version,
        running,
    };
    let added = |place, token: TokenId| Notification::Added {
        place: names.place(place),
        id: token.to_string(),
        label: net.map_or_else(String::new, |net| translate::label(net, place, token)),
        keys: keys(place, token),
        marking_version,
        running,
    };
    let removed = |place, token: TokenId| Notification::Removed {
        place: names.place(place),
        id: token.to_string(),
        marking_version,
        running,
    };
    Some(match *event {
        Event::Added { place, token } => {
            if projection.is_none_or(|p| p.arrived(net, place, token)) {
                added(place, token)
            } else {
                hidden(HiddenKind::Added, place)
            }
        }
        Event::Removed { place, token } => {
            if projection.is_none_or(|p| p.left(place, token)) {
                removed(place, token)
            } else {
                hidden(HiddenKind::Removed, place)
            }
        }
        Event::Mutated { place, token } => {
            match projection.map_or(Seen::Mutated, |p| p.changed(net, place, token)) {
                Seen::Mutated => Notification::Mutated {
                    place: names.place(place),
                    id: token.to_string(),
                    label: net
                        .and_then(|net| net.debug(place, token))
                        .map(translate::cut),
                    keys: keys(place, token),
                    marking_version,
                    running,
                },
                Seen::Added => added(place, token),
                Seen::Removed => removed(place, token),
                Seen::Hidden => hidden(HiddenKind::Mutated, place),
            }
        }
        _ => return None,
    })
}

/// The lines that tell a connection which tokens went out of its view and
/// which came into it although none of them moved (PROTOCOL.md §13.4): a
/// `removed` for each of `gone`, then an `added` for each of `came`, each a
/// state notification stamped from `version` upward.
fn view_lines(
    names: &Names,
    net: &Net,
    gone: &[(PlaceId, TokenId)],
    came: &[(PlaceId, TokenId)],
    version: &mut u64,
    running: bool,
) -> Vec<Notification> {
    let mut lines = Vec::with_capacity(gone.len() + came.len());
    for &(place, token) in gone {
        *version += 1;
        lines.push(Notification::Removed {
            place: names.place(place),
            id: token.to_string(),
            marking_version: *version,
            running,
        });
    }
    for &(place, token) in came {
        *version += 1;
        lines.push(Notification::Added {
            place: names.place(place),
            id: token.to_string(),
            label: translate::label(net, place, token),
            keys: translate::keys(net, place, token),
            marking_version: *version,
            running,
        });
    }
    lines
}

/// The tokens one firing event names, as the connection is told of them:
/// what `consumed` it took (moved out of view, kept for a restore) and what
/// `produced` it placed, through `projection` when the net has one, every
/// one of them when it has none. `landed` says the firing ended with this
/// event.
fn firing_tokens(
    projection: Option<&mut Projection>,
    net: Option<&Net>,
    consumed: &[(PlaceId, TokenId)],
    produced: &[(PlaceId, TokenId)],
    landed: bool,
) -> (Pairs, Pairs) {
    match projection {
        None => (consumed.to_vec(), produced.to_vec()),
        Some(projection) => {
            let consumed = projection.took(consumed);
            if landed {
                projection.landed();
            }
            (consumed, projection.placed(net, produced))
        }
    }
}

impl Loaded {
    /// A record for every inner net the marking holds now, its fireable set
    /// taken silently: what `load` and `reset` start from.
    fn know_all(&mut self) {
        let mut seen = Vec::new();
        walk(
            &self.net,
            &mut Vec::new(),
            self.name,
            &mut |_, path, _, net| {
                seen.push((net.id(), Inner::new(Some(net), path.to_owned(), true)));
                None::<()>
            },
        );
        self.inner.extend(seen);
    }

    /// The net a request's `path` or `nested_id` names (PROTOCOL.md §12.1):
    /// both is `invalid_params`, neither the outer net, and one that names
    /// no live inner net `unknown_net`.
    fn resolve(&self, args: &Args) -> Result<Target, (ErrorCode, String)> {
        let path = args.get("path").filter(|v| !v.is_null());
        let nested_id = args.get("nested_id").filter(|v| !v.is_null());
        match (path, nested_id) {
            (None, None) => Ok(Target::Outer),
            (Some(_), Some(_)) => Err((
                ErrorCode::InvalidParams,
                "give a path or a nested_id, not both".to_owned(),
            )),
            (Some(Value::String(path)), None) => resolve_path(&self.net, self.name, path)
                .map(|found| found.map_or(Target::Outer, |(found, _)| Target::Inner(found)))
                .map_err(|message| (ErrorCode::UnknownNet, message)),
            (None, Some(Value::String(wanted))) => {
                find(&self.net, self.name, |net| net.id().to_string() == *wanted)
                    .map(|(found, _)| Target::Inner(found))
                    .ok_or_else(|| {
                        (
                            ErrorCode::UnknownNet,
                            format!("no live inner net has the nested id {wanted:?}"),
                        )
                    })
            }
            _ => Err((
                ErrorCode::InvalidParams,
                "\"path\" and \"nested_id\" must be strings".to_owned(),
            )),
        }
    }

    /// Makes sure the net `found` names has a record, its fireable set
    /// taken silently: whoever named it reads it in the same answer.
    fn know(&mut self, found: &Found) {
        let net = descend(&self.net, &found.levels);
        match self.inner.get_mut(&found.nested_id) {
            Some(inner) => {
                if let Some(net) = net {
                    inner.see(net);
                }
                inner.last_path.clone_from(&found.path);
            }
            None => {
                self.inner
                    .insert(found.nested_id, Inner::new(net, found.path.clone(), true));
            }
        }
    }

    /// Drops the records of inner nets whose nested ids no longer resolve:
    /// a firing consumed their token and did not put it back. Called when a
    /// firing lands and after an inner firing, never while a body holds a
    /// token, so that a vehicle in transit keeps its record and version.
    fn prune(&mut self) {
        if self.inner.is_empty() {
            return;
        }
        let mut live = HashSet::new();
        walk(
            &self.net,
            &mut Vec::new(),
            self.name,
            &mut |_, _, _, net| {
                live.insert(net.id());
                None::<()>
            },
        );
        self.inner.retain(|id, _| live.contains(id));
        self.touched.retain(|id| live.contains(id));
    }

    fn push_log(&mut self, entry: LogEntry, chain: Vec<NetId>) {
        if self.log.len() == LOG_CAPACITY {
            self.log.pop_front();
        }
        self.log.push_back(Logged { entry, chain });
    }

    /// The wire path of a lifted event: the loaded net's name, then per
    /// level the place's name in the net above, the token and the net's
    /// name. Makes a record for each net on the way that has none.
    fn lifted_path(&mut self, lifted: &Lifted<'_>) -> String {
        let mut path = self.name.to_owned();
        for (depth, &(place, token, name, id)) in lifted.levels.iter().enumerate() {
            let place_name = match depth {
                0 => self.names.place(place),
                _ => {
                    let above = lifted.levels[depth - 1].3;
                    self.inner
                        .get(&above)
                        .map_or_else(|| place.to_string(), |inner| inner.names.place(place))
                }
            };
            path = translate::level(&path, &place_name, token, name);
            let live = Lifted {
                levels: lifted.levels[..=depth].to_vec(),
                event: lifted.event,
            }
            .live(&self.net);
            match self.inner.get_mut(&id) {
                Some(inner) => {
                    if let Some(net) = live {
                        inner.see(net);
                    }
                    inner.last_path.clone_from(&path);
                }
                None => {
                    self.inner.insert(id, Inner::new(live, path.clone(), false));
                }
            }
        }
        path
    }

    /// One event lifted out of an inner net as the `nested` line the wire
    /// carries (PROTOCOL.md §12.4): the notification the inner net would
    /// send on its own, with its own names and version, inside one wrapper
    /// with the full path and the outer net's version, unchanged. An inner
    /// firing also gets its log entry and its net's progress here.
    fn lift(
        &mut self,
        event: &Event,
        outer_version: u64,
        now: SystemTime,
        launched: &mut HashMap<NetId, Pairs>,
        firings: &mut Vec<InnerFiring>,
    ) -> Notification {
        let lifted = Lifted::of(event).expect("lift is given a Nested event");
        let path = self.lifted_path(&lifted);
        let nested_id = lifted.nested_id();
        let running = self.running;
        if !self.touched.contains(&nested_id) {
            self.touched.push(nested_id);
        }
        let live = lifted.live(&self.net);
        let inner = self
            .inner
            .get_mut(&nested_id)
            .expect("lifted_path made the record");
        let names = &inner.names;
        // The inner net's own projection, if `set_filter` named it: its
        // events are told through it, whatever the outer net's is.
        let mut projection = inner.projection.as_mut();
        let version = inner.marking_version + 1;
        // What the inner net's log entry for a firing says, if this is one.
        let mut firing: Option<(String, Pairs, Pairs)> = None;
        let mut failure: Option<(String, bool)> = None;
        let notification = match lifted.event {
            // An inner firing is inline in the core (`step_deep`), so it is
            // one piece on the wire too.
            Event::Fired {
                transition,
                consumed,
                produced: made,
            } => {
                firing = Some((
                    names.transition(*transition),
                    consumed.clone(),
                    made.clone(),
                ));
                let (consumed, made) =
                    firing_tokens(projection.as_deref_mut(), live, consumed, made, true);
                Notification::Fired {
                    transition: names.transition(*transition),
                    consumed: names.pairs(&consumed),
                    produced: produced(names, live, &made),
                    duration_ms: 0.0,
                    marking_version: version,
                    running,
                }
            }
            Event::Launched {
                transition,
                consumed,
            } => {
                launched.insert(nested_id, consumed.clone());
                let (consumed, _) =
                    firing_tokens(projection.as_deref_mut(), live, consumed, &[], false);
                Notification::Launched {
                    transition: names.transition(*transition),
                    consumed: names.pairs(&consumed),
                    marking_version: version,
                    running,
                }
            }
            Event::Distributed {
                transition,
                produced: made,
            } => {
                let consumed = launched.remove(&nested_id).unwrap_or_default();
                firing = Some((names.transition(*transition), consumed, made.clone()));
                let (_, made) = firing_tokens(projection.as_deref_mut(), live, &[], made, true);
                Notification::Distributed {
                    transition: names.transition(*transition),
                    produced: produced(names, live, &made),
                    duration_ms: 0.0,
                    marking_version: version,
                    running,
                }
            }
            Event::Failed {
                transition,
                consumed,
                restored,
                error,
            } => {
                firing = Some((names.transition(*transition), consumed.clone(), Vec::new()));
                failure = Some((error.clone(), *restored));
                let told = match projection.as_deref_mut() {
                    Some(projection) => projection.failed(consumed, *restored),
                    None => consumed.clone(),
                };
                Notification::Failed {
                    transition: names.transition(*transition),
                    error: error.clone(),
                    restored: *restored,
                    consumed: names.pairs(&told),
                    marking_version: version,
                    running,
                }
            }
            // Under rule 3 (lifted at launch, the token in the body's hands)
            // there is no net to label from, nor to read keys from.
            event @ (Event::Added { .. } | Event::Removed { .. } | Event::Mutated { .. }) => {
                token_line(names, live, projection, event, version, running).expect("a token event")
            }
            Event::Toggled {
                transition,
                enabled,
            } => Notification::Toggled {
                transition: names.transition(*transition),
                enabled: *enabled,
                marking_version: version,
                running,
            },
            // A body that set an inner parameter: its wire name, from the
            // inner net, which only the outer net's own `set_param` names.
            Event::ParamChanged { type_name } => Notification::ParamChanged {
                param: live
                    .and_then(|net| net.param_name(type_name))
                    .unwrap_or(type_name)
                    .to_owned(),
                marking_version: version,
                running,
            },
            // An `import_marking` inside (a body loading a saved cabin): the
            // inner net's marking was replaced wholesale. An inner net is
            // always in its first generation.
            Event::Resynced => Notification::Reloaded {
                generation: 1,
                reason: "resync".into(),
                marking_version: inner.marking_version,
                running,
            },
            // An event this runner does not know: the same, it moved in a
            // way the runner cannot describe.
            _ => Notification::Reloaded {
                generation: 1,
                reason: "resync".into(),
                marking_version: inner.marking_version,
                running,
            },
        };
        if notification.is_state() {
            inner.marking_version = version;
        }
        if let Some((transition, consumed, made)) = firing {
            if failure.is_none() {
                inner.progress.fired(&transition, now);
            }
            let entry = LogEntry {
                seq: 0,
                transition: transition.clone(),
                started_at: iso8601(now),
                duration_ms: 0.0,
                outcome: if failure.is_some() { "failed" } else { "fired" }.to_owned(),
                consumed: inner.names.pairs(&consumed),
                produced: inner.names.pairs(&made),
                error: failure.as_ref().map(|(error, _)| error.clone()),
                restored: failure.as_ref().map(|&(_, restored)| restored),
                path: Some(path.clone()),
                nested_id: Some(nested_id.to_string()),
            };
            self.log_seq += 1;
            let entry = LogEntry {
                seq: self.log_seq,
                ..entry
            };
            self.push_log(entry, lifted.chain());
            firings.push(InnerFiring {
                transition,
                path: path.clone(),
                nested_id: nested_id.to_string(),
            });
        }
        Notification::Nested {
            path,
            nested_id: nested_id.to_string(),
            event: Box::new(notification),
            marking_version: outer_version,
            running,
        }
    }

    /// The `added`s and `removed`s of the tokens that came into a
    /// projection's view, or went out of it, with no token event of their
    /// own (PROTOCOL.md §13.4), after a batch's events and before its
    /// `fireable`s, as the Python runner tells them: after a change of a net
    /// parameter a `{key, param}` clause names, every token is compared;
    /// after inner events, the tokens they were lifted through, whose keys
    /// may read the nets they hold. The outer net's first, then each inner
    /// net's, `nested`; each line a state notification of its own net.
    fn reproject(
        &mut self,
        outer_version: &mut u64,
        params_changed: &[Option<NetId>],
        lifted_through: &[(Option<NetId>, PlaceId, TokenId)],
        out: &mut Vec<Outgoing>,
    ) {
        if params_changed.is_empty() && lifted_through.is_empty() {
            return;
        }
        let running = self.running;
        let mut nets: Vec<Option<NetId>> = vec![None];
        for which in params_changed
            .iter()
            .copied()
            .chain(lifted_through.iter().map(|&(which, ..)| which))
        {
            if !nets.contains(&which) {
                nets.push(which);
            }
        }
        for which in nets {
            let only: Pairs = lifted_through
                .iter()
                .filter(|&&(net, ..)| net == which)
                .map(|&(_, place, token)| (place, token))
                .collect();
            let asks_all = params_changed.contains(&which);
            match which {
                None => {
                    let Some(projection) = self.projection.as_mut() else {
                        continue;
                    };
                    let full = asks_all && projection.reads_params();
                    if !full && only.is_empty() {
                        continue;
                    }
                    let (gone, came) =
                        projection.reconcile(&self.net, (!full).then_some(only.as_slice()));
                    for line in
                        view_lines(&self.names, &self.net, &gone, &came, outer_version, running)
                    {
                        push(out, line);
                    }
                }
                Some(id) => {
                    let Some(inner) = self.inner.get_mut(&id) else {
                        continue;
                    };
                    let Some(projection) = inner.projection.as_mut() else {
                        continue;
                    };
                    let full = asks_all && projection.reads_params();
                    if !full && only.is_empty() {
                        continue;
                    }
                    let Some((found, net)) = find(&self.net, self.name, |net| net.id() == id)
                    else {
                        continue;
                    };
                    let (gone, came) =
                        projection.reconcile(net, (!full).then_some(only.as_slice()));
                    let lines = view_lines(
                        &inner.names,
                        net,
                        &gone,
                        &came,
                        &mut inner.marking_version,
                        running,
                    );
                    for line in lines {
                        push(
                            out,
                            Notification::Nested {
                                path: found.path.clone(),
                                nested_id: id.to_string(),
                                event: Box::new(line),
                                marking_version: *outer_version,
                                running,
                            },
                        );
                    }
                }
            }
        }
    }

    /// The `fireable` of each inner net that had events since it was last
    /// compared, `nested`, when it differs from the last one sent: after
    /// the outer net's (PROTOCOL.md §12.4). Not a state notification, so no
    /// version moves. A net whose token is gone, or in a body's hands, is
    /// skipped.
    fn compare_inner_fireable(&mut self, outer_version: u64, out: &mut Vec<Outgoing>) {
        let running = self.running;
        for id in std::mem::take(&mut self.touched) {
            let Some((found, net)) = find(&self.net, self.name, |net| net.id() == id) else {
                continue;
            };
            let Some(inner) = self.inner.get_mut(&id) else {
                continue;
            };
            inner.see(net);
            inner.last_path.clone_from(&found.path);
            let fireable = fireable_names(net, &inner.names);
            if fireable == inner.last_fireable {
                continue;
            }
            inner.last_fireable.clone_from(&fireable);
            push(
                out,
                Notification::Nested {
                    path: found.path,
                    nested_id: id.to_string(),
                    event: Box::new(Notification::Fireable {
                        transitions: fireable,
                        marking_version: inner.marking_version,
                        running,
                    }),
                    marking_version: outer_version,
                    running,
                },
            );
        }
    }
}

/// One net as a read sees it: the outer net, or an inner net with its
/// record.
struct View<'a> {
    net: &'a Net,
    topology: &'a Topology,
    names: &'a Names,
    progress: &'a ProgressKept,
    marking_version: u64,
    /// The net's wire path: the loaded net's name for the outer one, which
    /// the paths of its tokens' `nets` extend.
    path: String,
    /// What a result about an inner net carries besides (§12.3).
    inner: Option<InnerView>,
    /// The connection's projection of this net, if it set one (§13.4).
    projection: Option<&'a Projection>,
}

struct InnerView {
    nested_id: String,
    /// Its name within the token: what its `get_state` calls it.
    name: String,
    template: Option<String>,
}

impl View<'_> {
    /// The `path` and `nested_id` a result about an inner net carries; both
    /// `None` for the outer net.
    fn address(&self) -> (Option<String>, Option<String>) {
        match &self.inner {
            Some(inner) => (Some(self.path.clone()), Some(inner.nested_id.clone())),
            None => (None, None),
        }
    }
}

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

    /// Whether a run fired an inline (mutate-edge) body, or a firing inside
    /// an inner net, in the last [`handle`](Owner::handle) and is free to
    /// fire again at once.
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
            && (!ANSWERED_WHILE_AIRBORNE.contains(&request.method.as_str())
                || held_by_the_body(loaded, &request))
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
            _ if OUTER_ONLY.contains(&method.as_str())
                && (args.contains_key("path") || args.contains_key("nested_id")) =>
            {
                respond_error(
                    out,
                    &id,
                    ErrorCode::InvalidParams,
                    format!(
                        "{method} acts on the outer net only (PROTOCOL.md §12.3): \
                         it takes no path or nested_id"
                    ),
                );
            }
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
                let target = self.target(args)?;
                let state = self.state(&target, limit)?;
                respond(out, id, state);
            }
            "get_place_tokens" => {
                let place = string_arg(args, "place")?;
                let offset = usize_arg(args, "offset")?.unwrap_or(0);
                let limit = page_limit(args, "limit")?;
                let target = self.target(args)?;
                let view = self.view(&target)?;
                let place_id = view.net.place_named(place).ok_or_else(|| {
                    (ErrorCode::UnknownPlace, format!("no place named {place:?}"))
                })?;
                let count = view.net.count(place_id);
                // Under the connection's filter the page is of the tokens it
                // keeps, and `has_more` is relative to them (§13.4).
                let (matching, tokens) = match view.projection {
                    None => (
                        None,
                        translate::tokens(view.net, &view.path, place_id, place, offset, limit),
                    ),
                    Some(projection) => {
                        let kept = projection.kept_in(view.net, place_id);
                        let tokens = kept
                            .iter()
                            .skip(offset)
                            .take(limit)
                            .map(|&token| {
                                translate::entry(view.net, &view.path, place_id, place, token)
                            })
                            .collect();
                        (Some(kept.len()), tokens)
                    }
                };
                let (path, nested_id) = view.address();
                let page = PlaceTokens {
                    place: place.to_owned(),
                    marking_version: view.marking_version,
                    count,
                    matching,
                    offset,
                    limit,
                    has_more: offset.saturating_add(tokens.len()) < matching.unwrap_or(count),
                    tokens,
                    path,
                    nested_id,
                };
                respond(out, id, page);
            }
            "query" => self.query(id, args, out)?,
            "distinct" => {
                let key = string_arg(args, "key")?.to_owned();
                let target = self.target(args)?;
                let view = self.view(&target)?;
                let places = match args.get("places") {
                    None | Some(Value::Null) => None,
                    Some(Value::Array(names)) => {
                        let names: Vec<String> = names
                            .iter()
                            .map(|name| name.as_str().map(str::to_owned))
                            .collect::<Option<_>>()
                            .ok_or_else(not_place_names)?;
                        Some(translate::place_ids(&names, view.net).map_err(filter_error)?)
                    }
                    Some(_) => return Err(not_place_names()),
                };
                let values = view.net.distinct_values(&key, places.as_deref());
                respond(
                    out,
                    id,
                    DistinctResult {
                        marking_version: view.marking_version,
                        key,
                        values,
                    },
                );
            }
            "set_filter" => {
                let target = self.target(args)?;
                let projection = match args.get("filter").filter(|value| !value.is_null()) {
                    None => None,
                    Some(value) => {
                        let view = self.view(&target)?;
                        let filter = translate::filter(value, view.net).map_err(filter_error)?;
                        Some(Projection::new(filter, view.net))
                    }
                };
                let version = match &target {
                    Target::Outer => {
                        self.loaded_mut().projection = projection;
                        self.marking_version
                    }
                    Target::Inner(found) => {
                        let inner = self
                            .loaded_mut()
                            .inner
                            .get_mut(&found.nested_id)
                            .ok_or_else(|| {
                                (
                                    ErrorCode::UnknownNet,
                                    format!("no live inner net at {:?}", found.path),
                                )
                            })?;
                        inner.projection = projection;
                        inner.marking_version
                    }
                };
                respond(out, id, json!({ "marking_version": version }));
            }
            "get_token" => {
                let token = string_arg(args, "id")?;
                let target = self.target(args)?;
                let value = self.token(&target, token)?;
                respond(out, id, value);
            }
            "get_log" => {
                let after = u64_arg(args, "after")?.unwrap_or(0);
                let limit = u64_arg(args, "limit")?
                    .unwrap_or(LOG_CAPACITY as u64)
                    .clamp(1, LOG_CAPACITY as u64) as usize;
                // One ring for the process (PROTOCOL.md §8): an inner net's
                // entries, and its own inner nets', are those whose chain
                // holds it.
                let within = match self.target(args)? {
                    Target::Outer => None,
                    Target::Inner(found) => Some(found.nested_id),
                };
                let entries: Vec<&LogEntry> = self
                    .loaded_ref()
                    .log
                    .iter()
                    .filter(|logged| logged.entry.seq > after)
                    .filter(|logged| within.is_none_or(|id| logged.chain.contains(&id)))
                    .take(limit)
                    .map(|logged| &logged.entry)
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
            "inject" => self.inject(id, args, now, out)?,
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

    /// `inject {place, token, path?, nested_id?}`: the token decoded into
    /// the named net's place and appended (PROTOCOL.md §6.4, §12.3). Into an
    /// inner net it goes through `nested_scope` at every level, so its
    /// `added` comes out `nested`, and the answer carries that net's version.
    fn inject(
        &mut self,
        id: &str,
        args: &Args,
        now: SystemTime,
        out: &mut Vec<Outgoing>,
    ) -> Result<(), (ErrorCode, String)> {
        let place = string_arg(args, "place")?.to_owned();
        let token = args.get("token").cloned().ok_or_else(|| {
            (
                ErrorCode::InvalidParams,
                "inject needs a \"token\"".to_owned(),
            )
        })?;
        let command = JsonCommand::Extend {
            place,
            tokens: vec![token],
        };
        let no_event = || {
            (
                ErrorCode::Internal,
                "the token was added without an event".to_owned(),
            )
        };
        match self.target(args)? {
            Target::Outer => {
                self.apply(command)?;
                let flushed = self.flush_events(now, None, true, out);
                let added = flushed
                    .events
                    .iter()
                    .find_map(|event| match event {
                        Event::Added { token, .. } => Some(token.to_string()),
                        _ => None,
                    })
                    .ok_or_else(no_event)?;
                respond(
                    out,
                    id,
                    json!({"status": "injected", "id": added, "marking_version": self.marking_version}),
                );
            }
            Target::Inner(found) => {
                let applied = descend_mut(&mut self.loaded_mut().net, &found.levels, |net| {
                    net.apply_json(command)
                })
                .ok_or_else(|| {
                    (
                        ErrorCode::UnknownNet,
                        format!("no live inner net at {:?}", found.path),
                    )
                })?;
                applied
                    .map_err(|error| (code_of(CoreError::Command(&error)), error.to_string()))?;
                let flushed = self.flush_events(now, None, true, out);
                let added = flushed
                    .events
                    .iter()
                    .find_map(|event| {
                        let lifted = Lifted::of(event)?;
                        match lifted.event {
                            Event::Added { token, .. } if lifted.nested_id() == found.nested_id => {
                                Some(token.to_string())
                            }
                            _ => None,
                        }
                    })
                    .ok_or_else(no_event)?;
                let version = self
                    .loaded_ref()
                    .inner
                    .get(&found.nested_id)
                    .map_or(0, |inner| inner.marking_version);
                respond(
                    out,
                    id,
                    json!({"status": "injected", "id": added, "marking_version": version}),
                );
            }
        }
        Ok(())
    }

    /// `query {filter?, token_limit?, path?, nested_id?}` (PROTOCOL.md
    /// §13.3): the tokens `filter` keeps — or, without one, the connection's
    /// filter, or else every token — in place order then token order, from
    /// the key index; `count` all of them, and the first `token_limit` in
    /// full, each with its JSON where its place is serializable.
    fn query(
        &mut self,
        id: &str,
        args: &Args,
        out: &mut Vec<Outgoing>,
    ) -> Result<(), (ErrorCode, String)> {
        let limit = page_limit(args, "token_limit")?;
        let target = self.target(args)?;
        let view = self.view(&target)?;
        let net = view.net;
        let given = match args.get("filter").filter(|value| !value.is_null()) {
            Some(value) => Some(translate::filter(value, net).map_err(filter_error)?),
            None => None,
        };
        let every = Filter::new();
        let filter = given
            .as_ref()
            .or(view.projection.map(|projection| &projection.filter))
            .unwrap_or(&every);
        let found = net
            .query_ids(filter)
            .map_err(|error| (code_of(CoreError::Net(&error)), error.to_string()))?;
        let matches = found
            .iter()
            .take(limit)
            .map(|&(place, token)| {
                let name = view.names.place(place);
                QueryMatch {
                    token: translate::entry(net, &view.path, place, &name, token),
                    json: net.json(place, token),
                    place: name,
                }
            })
            .collect();
        respond(
            out,
            id,
            QueryResult {
                marking_version: view.marking_version,
                count: found.len(),
                matches,
            },
        );
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

    /// The net a request names, with a record made for an inner one.
    fn target(&mut self, args: &Args) -> Result<Target, (ErrorCode, String)> {
        let loaded = self.loaded_mut();
        let target = loaded.resolve(args)?;
        if let Target::Inner(found) = &target {
            loaded.know(found);
        }
        Ok(target)
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
    ///
    /// Every inner net goes with the old marking (PROTOCOL.md §12.6): the
    /// new generation's are walked and recorded afresh, at version 0.
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
        let last_fireable = fireable_names(&net, &names);
        self.generation += 1;
        let mut loaded = Loaded {
            entry,
            name: self.nets[entry].name,
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
            inner: HashMap::new(),
            touched: Vec::new(),
            projection: None,
        };
        loaded.know_all();
        self.loaded = Some(loaded);
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

    /// The net `target` names, as a read sees it.
    fn view(&self, target: &Target) -> Result<View<'_>, (ErrorCode, String)> {
        let loaded = self.loaded_ref();
        let Target::Inner(found) = target else {
            return Ok(View {
                net: &loaded.net,
                topology: &loaded.topology,
                names: &loaded.names,
                progress: &loaded.progress,
                marking_version: self.marking_version,
                path: loaded.name.to_owned(),
                inner: None,
                projection: loaded.projection.as_ref(),
            });
        };
        let gone = || {
            (
                ErrorCode::UnknownNet,
                format!("no live inner net at {:?}", found.path),
            )
        };
        let net = descend(&loaded.net, &found.levels).ok_or_else(gone)?;
        let inner = loaded.inner.get(&found.nested_id).ok_or_else(gone)?;
        let topology = inner.topology.as_ref().ok_or_else(gone)?;
        Ok(View {
            net,
            topology,
            names: &inner.names,
            progress: &inner.progress,
            marking_version: inner.marking_version,
            path: found.path.clone(),
            inner: Some(InnerView {
                nested_id: found.nested_id.to_string(),
                name: found.name.clone(),
                template: inner.template.clone(),
            }),
            projection: inner.projection.as_ref(),
        })
    }

    fn state(&self, target: &Target, token_limit: usize) -> Result<State, (ErrorCode, String)> {
        let view = self.view(target)?;
        let net = view.net;
        let transitions = view
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
        // Under the connection's filter, each place lists the tokens it
        // keeps and says how many there are (§13.4), from the key index.
        let kept: Option<HashMap<PlaceId, Vec<TokenId>>> = view.projection.map(|projection| {
            let mut kept: HashMap<PlaceId, Vec<TokenId>> = HashMap::new();
            for (place, token) in projection.kept(net) {
                kept.entry(place).or_default().push(token);
            }
            kept
        });
        let places = view
            .topology
            .places
            .iter()
            .map(|p| {
                let (matching, tokens) = match &kept {
                    None => (
                        None,
                        translate::tokens(net, &view.path, p.id, &p.name, 0, token_limit),
                    ),
                    Some(kept) => {
                        let ids = kept.get(&p.id).map_or(&[][..], Vec::as_slice);
                        let tokens = ids
                            .iter()
                            .take(token_limit)
                            .map(|&token| translate::entry(net, &view.path, p.id, &p.name, token))
                            .collect();
                        (Some(ids.len()), tokens)
                    }
                };
                (
                    p.name.clone(),
                    protocol::PlaceMarking {
                        count: net.count(p.id),
                        matching,
                        tokens,
                    },
                )
            })
            .collect();
        let (path, nested_id) = view.address();
        let loaded = self.loaded_ref();
        Ok(State {
            protocol_version: protocol::PROTOCOL_VERSION,
            runtime: protocol::RUNTIME.to_owned(),
            net: view
                .inner
                .as_ref()
                .map_or_else(|| loaded.name.to_owned(), |inner| inner.name.clone()),
            // An inner net is in its first generation for as long as it
            // exists (§12.3).
            generation: if view.inner.is_some() {
                1
            } else {
                self.generation
            },
            marking_version: view.marking_version,
            running: loaded.running,
            topology: translate::topology(view.topology, view.names),
            transitions,
            marking: protocol::Marking { places },
            progress: protocol::Progress {
                // The outer net's count includes the firings the runner made
                // inside its tokens (§12.3), as the Python runner's does.
                step_count: net.fired_count() + view.progress.inside,
                last_fired: view.progress.last_fired.clone(),
                fired_counts: view.progress.fired_counts.clone(),
                last_progress_at: view.progress.last_progress_at.map(iso8601),
            },
            path,
            nested_id,
            template: view.inner.as_ref().and_then(|inner| inner.template.clone()),
        })
    }

    fn token(&self, target: &Target, wanted: &str) -> Result<TokenValue, (ErrorCode, String)> {
        let view = self.view(target)?;
        let unknown = || (ErrorCode::UnknownToken, format!("no live token {wanted:?}"));
        let token: TokenId = wanted.parse().map_err(|_| unknown())?;
        let net = view.net;
        let place = view
            .topology
            .places
            .iter()
            .find(|p| net.tokens(p.id).any(|(t, _)| t == token))
            .ok_or_else(unknown)?;
        // A token the connection's filter hides is not there for it
        // (§13.4), and is answered as one that is not there at all.
        if view
            .projection
            .is_some_and(|projection| !projection.keeps(Some(net), place.id, token))
        {
            return Err(unknown());
        }
        let value = match net.json(place.id, token) {
            Some(value) => TokenBody::Json { value },
            None => TokenBody::Text {
                text: net.debug(place.id, token).unwrap_or_default(),
            },
        };
        let (path, nested_id) = view.address();
        Ok(TokenValue {
            id: wanted.to_owned(),
            place: place.name.clone(),
            type_name: place.type_name.to_owned(),
            value,
            path,
            nested_id,
            nets: translate::nets(net, &view.path, place.id, &place.name, token),
        })
    }

    // ----- the firing cycle --------------------------------------------------

    /// One firing (PROTOCOL.md §6.3, §12.3): the outer net's own choice, or,
    /// when it has none, the first inner firing in `step_deep`'s order.
    fn step(
        &mut self,
        id: &str,
        now: SystemTime,
        out: &mut Vec<Outgoing>,
    ) -> Result<(), (ErrorCode, String)> {
        if let Some(transition) = self.loaded_ref().net.select() {
            return self.launch(transition, Some(id.to_owned()), now, out);
        }
        if !self.fire_inside(Some(id), now, out) {
            let running = self.loaded_ref().running;
            respond(
                out,
                id,
                json!({"fired": false, "marking_version": self.marking_version, "running": running}),
            );
        }
        Ok(())
    }

    /// One firing inside an inner net, inline, through `Net::step_inside`:
    /// the outer selector has already been asked, so it is not asked again.
    /// Reports it, `nested`, answers the `step` that asked for it if one
    /// did, and stops a run on a failure. False if no inner net had
    /// anything to fire.
    fn fire_inside(
        &mut self,
        step_id: Option<&str>,
        now: SystemTime,
        out: &mut Vec<Outgoing>,
    ) -> bool {
        let loaded = self.loaded_mut();
        let failure = match loaded.net.step_inside() {
            Ok(None) => return false,
            Ok(Some(_)) => None,
            Err(failed) => Some(Failure::of(&failed)),
        };
        loaded.idle = false;
        let flushed = self.flush_events(now, None, true, out);
        let loaded = self.loaded_mut();
        // The outer net counts the firing too, under its path without the
        // outer net's name, then the transition (`Depot/#17/cabin/Sit`), as
        // the Python runner does; the inner net's own progress counted it
        // under its own name when the flush lifted it.
        if failure.is_none()
            && let Some(firing) = flushed.inner_firings.last()
        {
            let below = firing
                .path
                .strip_prefix(loaded.name)
                .and_then(|rest| rest.strip_prefix('/'))
                .unwrap_or(&firing.path);
            loaded
                .progress
                .fired_inside(&format!("{below}/{}", firing.transition), now);
        }
        loaded.prune();
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
        let Some(step_id) = step_id else {
            return true;
        };
        // The firing's own line said where it was: the last inner firing
        // the flush reported.
        let Some(firing) = flushed.inner_firings.last() else {
            respond_error(
                out,
                step_id,
                ErrorCode::Internal,
                "an inner firing left no event",
            );
            return true;
        };
        let mut answer = json!({
            "fired": true,
            "transition": firing.transition,
            "path": firing.path,
            "nested_id": firing.nested_id,
            "marking_version": self.marking_version,
            "running": self.loaded_ref().running,
        });
        if failure.is_some() {
            answer["failed"] = json!(true);
        }
        respond(out, step_id, answer);
        true
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
            let flushed = self.flush_events(now, None, false, out);
            let consumed = flushed.events.into_iter().find_map(|event| match event {
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
        let events = self.flush_events(now, Some(&flight), true, out).events;

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
            path: None,
            nested_id: None,
        };
        loaded.push_log(entry, Vec::new());
        if failure.is_none() {
            loaded.progress.fired(&flight.name, now);
        }
        // A token this firing consumed and did not put back took its inner
        // nets with it (§12.6).
        loaded.prune();

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
            let failed = matches!(result, Err(NetError::FiringFailed(_)));
            match result {
                Ok(()) | Err(NetError::FiringFailed(_)) => {
                    let running = self.loaded_ref().running;
                    let mut answer = json!({
                        "fired": true,
                        "transition": flight.name,
                        "marking_version": self.marking_version,
                        "running": running,
                    });
                    // PROTOCOL.md §6.3: a step whose body failed says so.
                    if failed {
                        answer["failed"] = json!(true);
                    }
                    respond(out, &step_id, answer);
                }
                Err(other) => respond_error(out, &step_id, ErrorCode::Internal, other.to_string()),
            }
        }
    }

    /// Keeps a running net going: launches the next firing if it is free,
    /// or makes the next inner firing when the outer net has none (§12.3),
    /// or handles quiescence — nothing fireable anywhere — with `idle` once
    /// per spell for a `"24/7"` net and `execution_stopped` for any other.
    fn drive(&mut self, now: SystemTime, out: &mut Vec<Outgoing>) {
        let Some(loaded) = &self.loaded else {
            return;
        };
        if !loaded.running || loaded.in_flight.is_some() {
            return;
        }
        let continuous = self.nets[loaded.entry].hosting == Hosting::Continuous;
        if let Some(transition) = loaded.net.select() {
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
            return;
        }
        // Inline, like a mutate-edge body: one per `handle`, and a tick
        // asked for so that a `stop` in between is heard.
        if self.fire_inside(None, now, out) {
            let loaded = self.loaded_ref();
            self.poke = loaded.running && loaded.in_flight.is_none();
            return;
        }
        if continuous {
            if !self.loaded_ref().idle {
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
            return;
        }
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

    // ----- events ------------------------------------------------------------

    /// Drains the net's log and writes one notification per event, in the
    /// wire's order ([`wire_order`]): each state notification of the outer
    /// net raising its `marking_version` by one, each inner event `nested`
    /// and raising its own net's. Then, if asked and anything happened, the
    /// outer `fireable` set if it changed, and each touched inner net's
    /// after it. Returns the events for the caller to read ids off, and the
    /// inner firings it reported.
    fn flush_events(
        &mut self,
        now: SystemTime,
        flight: Option<&InFlight>,
        compare_fireable: bool,
        out: &mut Vec<Outgoing>,
    ) -> Flushed {
        let generation = self.generation;
        let marking_version = &mut self.marking_version;
        let loaded = self.loaded.as_mut().expect("events need a net");
        let events: Vec<Event> = loaded.net.drain_events().collect();
        let running = loaded.running;
        let inline = flight.is_some_and(|f| f.inline);
        let duration = flight.map_or(0.0, |f| duration_ms(f.started_at, now));
        let mut inline_consumed: Vec<(PlaceId, TokenId)> = Vec::new();
        let mut inner_launched = HashMap::new();
        let mut inner_firings = Vec::new();
        // What may bring tokens into a projection's view, or take them out
        // of it, with no token event (PROTOCOL.md §13.4): a parameter that
        // changed, in the outer net (`None`) or an inner one, and the tokens
        // inner events were lifted through, by the net that holds them.
        let mut params_changed: Vec<Option<NetId>> = Vec::new();
        let mut lifted_through: Vec<(Option<NetId>, PlaceId, TokenId)> = Vec::new();
        for event in wire_order(&events) {
            if let Some(lifted) = Lifted::of(event) {
                let mut above = None;
                for &(place, token, _, id) in &lifted.levels {
                    lifted_through.push((above, place, token));
                    above = Some(id);
                }
                if matches!(lifted.event, Event::ParamChanged { .. }) {
                    params_changed.push(Some(lifted.nested_id()));
                }
            }
            if matches!(event, Event::Nested { .. }) {
                let line = loaded.lift(
                    event,
                    *marking_version,
                    now,
                    &mut inner_launched,
                    &mut inner_firings,
                );
                push(out, line);
                continue;
            }
            let names = &loaded.names;
            let net = &loaded.net;
            // The connection's projection of the outer net, if it set one:
            // every token a line names goes through it (§13.4).
            let projection = loaded.projection.as_mut();
            let version = *marking_version + 1;
            let notification = match event {
                Event::Fired {
                    transition,
                    consumed,
                    produced,
                } => {
                    let (consumed, produced) =
                        firing_tokens(projection, Some(net), consumed, produced, true);
                    Notification::Fired {
                        transition: names.transition(*transition),
                        consumed: names.pairs(&consumed),
                        produced: names.produced(net, &produced),
                        duration_ms: duration,
                        marking_version: version,
                        running,
                    }
                }
                Event::Launched { consumed, .. } if inline => {
                    // Reported with the landing, as one `fired`.
                    inline_consumed = firing_tokens(projection, Some(net), consumed, &[], false).0;
                    continue;
                }
                Event::Launched {
                    transition,
                    consumed,
                } => {
                    let (consumed, _) = firing_tokens(projection, Some(net), consumed, &[], false);
                    Notification::Launched {
                        transition: names.transition(*transition),
                        consumed: names.pairs(&consumed),
                        marking_version: version,
                        running,
                    }
                }
                Event::Distributed {
                    transition,
                    produced,
                } if inline => {
                    let (_, produced) = firing_tokens(projection, Some(net), &[], produced, true);
                    Notification::Fired {
                        transition: names.transition(*transition),
                        consumed: names.pairs(&inline_consumed),
                        produced: names.produced(net, &produced),
                        duration_ms: duration,
                        marking_version: version,
                        running,
                    }
                }
                Event::Distributed {
                    transition,
                    produced,
                } => {
                    let (_, produced) = firing_tokens(projection, Some(net), &[], produced, true);
                    Notification::Distributed {
                        transition: names.transition(*transition),
                        produced: names.produced(net, &produced),
                        duration_ms: duration,
                        marking_version: version,
                        running,
                    }
                }
                Event::Failed {
                    transition,
                    consumed,
                    restored,
                    error,
                } => {
                    let told = match projection {
                        Some(projection) => projection.failed(consumed, *restored),
                        None => consumed.clone(),
                    };
                    Notification::Failed {
                        transition: names.transition(*transition),
                        error: error.clone(),
                        restored: *restored,
                        consumed: names.pairs(&told),
                        marking_version: version,
                        running,
                    }
                }
                event @ (Event::Added { .. } | Event::Removed { .. } | Event::Mutated { .. }) => {
                    token_line(names, Some(net), projection, event, version, running)
                        .expect("a token event")
                }
                Event::Toggled {
                    transition,
                    enabled,
                } => Notification::Toggled {
                    transition: names.transition(*transition),
                    enabled: *enabled,
                    marking_version: version,
                    running,
                },
                // The name the `set_param` being served used; for a change
                // nobody asked for over the wire, the wire name the net
                // registered, or the type name if it registered none.
                Event::ParamChanged { type_name } => {
                    params_changed.push(None);
                    Notification::ParamChanged {
                        param: loaded
                            .pending_param
                            .clone()
                            .or_else(|| net.param_name(type_name).map(str::to_owned))
                            .unwrap_or_else(|| (*type_name).to_owned()),
                        marking_version: version,
                        running,
                    }
                }
                // An `import_marking`: the marking was replaced wholesale and
                // is not told as events (DESIGN §8.1), so a reader refetches.
                Event::Resynced => Notification::Reloaded {
                    generation,
                    reason: "resync".into(),
                    marking_version: *marking_version,
                    running,
                },
                // An event this runner does not know: say the marking moved
                // in a way it cannot describe.
                _ => Notification::Reloaded {
                    generation,
                    reason: "resync".into(),
                    marking_version: *marking_version,
                    running,
                },
            };
            if notification.is_state() {
                *marking_version = version;
            }
            push(out, notification);
        }
        loaded.reproject(marking_version, &params_changed, &lifted_through, out);
        if compare_fireable && !events.is_empty() {
            let fireable = fireable_names(&loaded.net, &loaded.names);
            if fireable != loaded.last_fireable {
                loaded.last_fireable.clone_from(&fireable);
                push(
                    out,
                    Notification::Fireable {
                        transitions: fireable,
                        marking_version: *marking_version,
                        running,
                    },
                );
            }
            loaded.compare_inner_fireable(*marking_version, out);
        }
        Flushed {
            events,
            inner_firings,
        }
    }
}

/// Whether a read must wait for the airborne body to land: it names an inner
/// net that does not resolve now, which may be inside the token the body
/// holds (PROTOCOL.md §12.3). After the landing it resolves, or is
/// `unknown_net` then.
fn held_by_the_body(loaded: &Loaded, request: &Request) -> bool {
    ADDRESSED_READS.contains(&request.method.as_str())
        && matches!(
            &request.params,
            Value::Object(args) if matches!(loaded.resolve(args), Err((ErrorCode::UnknownNet, _)))
        )
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

/// A request's filter that could not be used, as the error it answers.
fn filter_error(error: FilterError) -> (ErrorCode, String) {
    (code_of(CoreError::Filter(&error)), error.to_string())
}

fn not_place_names() -> (ErrorCode, String) {
    (
        ErrorCode::InvalidParams,
        "\"places\" is a list of place names".to_owned(),
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use petritype::{MutateEdge, NetBuilder, TakeEdge};

    /// A `Yard` of bare nets, each with a `Todo -Work-> Done`, and a second
    /// level: a `Crate` place of bare nets inside each.
    fn yard() -> (Net, Vec<NetId>) {
        fn worker(todo: u32) -> Net {
            let mut b = NetBuilder::new();
            let pending = b.place::<u32>("Todo");
            let done = b.place::<u32>("Done");
            b.seed(pending, 0..todo);
            b.transition("Work", (TakeEdge(pending),), (done,), |n: u32| (n,));
            let inside = b.place_nested::<Net>("Crate");
            let mut c = NetBuilder::new();
            c.place::<u32>("Straw");
            b.seed(inside, [c.build().unwrap()]);
            b.build().unwrap()
        }
        let mut b = NetBuilder::new();
        let yard = b.place_nested::<Net>("Yard");
        b.seed(yard, [worker(1), worker(2)]);
        let net = b.build().unwrap();
        let mut ids = Vec::new();
        walk(&net, &mut Vec::new(), "city", &mut |_, _, _, n| {
            ids.push(n.id());
            None::<()>
        });
        (net, ids)
    }

    #[test]
    fn the_walk_goes_down_before_it_goes_on() {
        let (net, ids) = yard();
        assert_eq!(ids.len(), 4, "two workers, a crate in each");
        let mut paths = Vec::new();
        walk(&net, &mut Vec::new(), "city", &mut |_, path, _, _| {
            paths.push(path.to_owned());
            None::<()>
        });
        assert_eq!(
            paths,
            [
                "city/Yard/#0/",
                // The crate is seeded after the work, so it has the next id.
                "city/Yard/#0//Crate/#1/",
                "city/Yard/#1/",
                "city/Yard/#1//Crate/#2/",
            ]
        );
    }

    #[test]
    fn a_path_resolves_level_by_level_and_names_the_segment_that_failed() {
        let (net, ids) = yard();
        let (found, inner) = resolve_path(&net, "city", "city/Yard/#1//Crate/#2/")
            .unwrap()
            .unwrap();
        assert_eq!(found.nested_id, ids[3]);
        assert_eq!(inner.id(), ids[3]);
        assert_eq!(found.levels.len(), 2);
        assert_eq!(found.name, "", "a bare net is unnamed");
        assert!(resolve_path(&net, "city", "city").unwrap().is_none());
        for (path, says) in [
            ("town/Yard/#0/", "\"city\""),
            ("city/Lot/#0/", "no place \"Lot\""),
            ("city/Yard/#0", "stops after \"#0\""),
            ("city/Yard/0/", "\"0\" is not a token"),
            ("city/Yard/#9/", "no token #9"),
            ("city/Yard/#0/cabin", "no net \"cabin\""),
            ("city/Yard/#0//Crate/#1", "stops after \"#1\""),
        ] {
            let error = resolve_path(&net, "city", path).err().unwrap();
            assert!(error.contains(says), "{path}: {error}");
        }
    }

    #[test]
    fn descending_mutably_lifts_at_every_level() {
        let (mut net, ids) = yard();
        let (found, _) = find(&net, "city", |n| n.id() == ids[1]).unwrap();
        let added = descend_mut(&mut net, &found.levels, |crate_net| {
            let straw = crate_net.place_ref::<u32>("Straw").unwrap();
            crate_net.put(straw, 7)
        })
        .unwrap();
        let events: Vec<Event> = net.drain_events().collect();
        assert_eq!(events.len(), 1);
        let lifted = Lifted::of(&events[0]).unwrap();
        assert_eq!(lifted.chain(), [ids[0], ids[1]]);
        assert!(matches!(lifted.event, Event::Added { token, .. } if *token == added));
        assert!(lifted.live(&net).is_some_and(|n| n.id() == ids[1]));
    }

    #[test]
    fn the_wire_tells_a_landing_s_inner_effects_before_the_landing() {
        // A mutate edge and a produced nested token in one firing: the
        // core's rules 2 and 4 put the inner events on both sides of the
        // `Mutated` and after the `Distributed`.
        let mut inner = NetBuilder::new();
        let seen = inner.place::<u32>("Seen");
        let inner = inner.build().unwrap();
        let mut b = NetBuilder::new();
        let here = b.place_nested::<Net>("Here");
        let there = b.place_nested::<Net>("There");
        let ticks = b.place::<u32>("Ticks");
        b.seed(here, [inner]);
        b.seed(ticks, [1]);
        b.transition(
            "Touch",
            (TakeEdge(ticks), MutateEdge(here)),
            (there,),
            move |n: u32, held: &mut Net| {
                held.put(seen, n);
                let mut copy = NetBuilder::new();
                let marks = copy.place::<u32>("Marks");
                let mut copy = copy.build().unwrap();
                copy.put(marks, n);
                (copy,)
            },
        );
        let mut net = b.build().unwrap();
        net.step().unwrap();
        let events: Vec<Event> = net.drain_events().collect();
        let kinds = |events: &[&Event]| -> Vec<&'static str> {
            events
                .iter()
                .map(|e| match e {
                    Event::Nested { .. } => "nested",
                    Event::Mutated { .. } => "mutated",
                    Event::Fired { .. } => "fired",
                    _ => "other",
                })
                .collect()
        };
        let core: Vec<&Event> = events.iter().collect();
        assert_eq!(kinds(&core), ["nested", "mutated", "fired", "nested"]);
        assert_eq!(
            kinds(&wire_order(&events)),
            ["nested", "nested", "mutated", "fired"]
        );
    }
}
