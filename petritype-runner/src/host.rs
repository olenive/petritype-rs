//! A net the host program owns, read over the net protocol.
//!
//! [`main`](crate::main) serves nets a process builds for whoever spawned
//! it, and the wire drives them. Here it is the other way round: a program
//! (a game, a simulation, a service) builds a net, keeps it and fires it
//! from its own code, and a viewer attaches to watch.
//!
//! - [`Host`] is the state machine. It holds the net, hands it to the
//!   program's own code with [`change`](Host::change), and turns what that
//!   code did into the protocol's lines. It answers a viewer's request with
//!   [`request`](Host::request). It does no I/O.
//! - [`HostServer`] is a [`Host`] behind a TCP listener, for a viewer on
//!   the same machine: viewers connect, up to the cap its
//!   [`HostServerConfig`] sets, each speaks the protocol line by line, and
//!   every one of them is told what the program does to the net.
//!
//! A host that nobody is listening to makes no lines: it keeps the log, the
//! progress and the marking version, and leaves out the work of telling.
//! [`Host::set_listening`] says when that is; a [`HostServer`] works it out
//! from its viewers.

use std::collections::HashMap;
use std::fmt;
use std::io::{self, BufReader, BufWriter, Write};
use std::net::{
    IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use petritype::Net;

use crate::lines::{DEFAULT_MAX_LINE_BYTES, Line, Lines};
use crate::owner::{Input, Outgoing, Owner};
use crate::protocol::{ErrorCode, HEARTBEAT_WRITES, Notification, Request, Response};

/// The heartbeat period of a [`Host`] unless it is given another.
const DEFAULT_HEARTBEAT: Duration = Duration::from_secs(30);

/// A net the host program owns, and what the protocol keeps about it.
///
/// The program builds the net, gives it to [`Host::new`] under a name, and
/// from then on reaches it through the host: [`net`](Host::net) to read it
/// and [`change`](Host::change) to fire it, put tokens in or do anything
/// else a `&mut Net` allows. `change` returns the protocol lines that tell
/// of what was done, for the program to send to every viewer;
/// [`request`](Host::request) answers one viewer's request line.
///
/// The wire reads the net and nothing else. `describe`, `get_state`,
/// `get_place_tokens`, `get_token`, `get_log`, `query` and `distinct` are
/// answered, inner nets included (a `path` or a `nested_id`); every other
/// method is `unsupported`: `load`, `reset` and `shutdown`, because the net
/// and the process are the program's; `step`, `start`, `stop`, `inject`,
/// `enable`, `disable`, `set_param` and `press`, because the program fires
/// and changes it; and `set_filter`, because a projection is one
/// connection's and a host may have several.
///
/// The net is generation 1 from the start, and a viewer's first request is
/// `get_state`: there is nothing to `load`. `running` is `true` on every
/// line, the program firing the net, until
/// [`set_running`](Host::set_running) says otherwise.
///
/// A program with no viewer attached can say so with
/// [`set_listening`](Host::set_listening): `change` then makes no lines and
/// costs less, while the log, the progress and the marking version go on as
/// before.
///
/// ```
/// use std::time::SystemTime;
/// use petritype::{NetBuilder, TakeEdge};
/// use petritype_runner::Host;
///
/// let mut b = NetBuilder::new();
/// let inbox = b.place::<u32>("Inbox");
/// let out = b.place::<u32>("Out");
/// b.seed(inbox, [1, 2]);
/// b.transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,));
/// let mut host = Host::new("doubler", b.build().expect("the net builds"));
///
/// // The program fires its own net; the lines tell a viewer what happened.
/// let (fired, lines) = host.change(SystemTime::now(), |net| net.step());
/// assert!(fired.expect("the body does not fail").is_some());
/// assert!(lines[0].contains(r#""method":"fired""#));
///
/// // A viewer asks, and is answered.
/// let answer = host.request(r#"{"id":"1","method":"get_state"}"#, SystemTime::now());
/// assert!(answer[0].contains(r#""net":"doubler""#));
/// ```
pub struct Host {
    owner: Owner,
}

impl Host {
    /// A host of `net`, which a viewer will know as `name`.
    pub fn new(name: &'static str, net: Net) -> Host {
        Host::with_heartbeat(name, net, DEFAULT_HEARTBEAT)
    }

    /// [`new`](Host::new), with a viewer expecting a heartbeat every
    /// `period`.
    fn with_heartbeat(name: &'static str, net: Net, period: Duration) -> Host {
        Host {
            owner: Owner::hosted(name, net, Arc::default(), period),
        }
    }

    /// The net, to read.
    pub fn net(&self) -> &Net {
        self.owner.hosted_net()
    }

    /// Hands the net to `change` and returns what it returned, with the
    /// protocol lines that tell of what it did, in order, without their
    /// newlines: one notification per event, the fireable sets that
    /// changed, the outer net's and each inner net's. They are for every
    /// viewer. While [`set_listening`](Host::set_listening) has listening
    /// off there are none, and making none is cheaper.
    ///
    /// Every firing `change` makes is kept in the log `get_log` reads and
    /// counted in the progress `get_state` reports, whether it was made in
    /// one piece ([`Net::step`], [`Net::step_deep`], a driver) or staged
    /// across calls ([`Net::launch`] in one, [`Net::distribute`] in a later
    /// one), and whether anyone is listening or not.
    ///
    /// A call that changes nothing returns no lines and costs the closure
    /// alone, so it is cheap to call on every tick. `now` is the time the
    /// log entries and the progress carry.
    ///
    /// The lines are made from the net's event log, so `change` should
    /// leave the log to the host: events it drains itself are told to
    /// nobody. A staged firing whose landing was drained so is forgotten
    /// at the end of the next call that tells anything, and the fireable
    /// sets are compared again from there (since 2026-11-28; until then
    /// they were never compared again). A `Firing` the program drops
    /// strands its transition in flight for good, and while it is in flight
    /// the sets are not compared.
    pub fn change<R>(
        &mut self,
        now: SystemTime,
        change: impl FnOnce(&mut Net) -> R,
    ) -> (R, Vec<String>) {
        let (result, outgoing) = self.owner.host_changes(now, change);
        (result, lines(outgoing))
    }

    /// Answers one request line from a viewer, without its newline. The
    /// lines returned are for that viewer alone: its response, or an
    /// `invalid_params` error for a line that is not a request.
    pub fn request(&mut self, line: &str, now: SystemTime) -> Vec<String> {
        match Request::parse(line) {
            Ok(request) => self.answer(request, now),
            Err(malformed) => refusal(malformed.id, malformed.message)
                .into_iter()
                .collect(),
        }
    }

    /// Answers one request that has been parsed: the lines for the viewer
    /// that sent it.
    fn answer(&mut self, request: Request, now: SystemTime) -> Vec<String> {
        lines(self.owner.handle(Input::Request(request), now))
    }

    /// Says whether anyone is listening: whether the lines
    /// [`change`](Host::change) returns go anywhere. On from the start.
    ///
    /// While it is off, `change` returns no lines and does none of the work
    /// of making them: it builds no notification and compares no fireable
    /// set, which is most of what a change costs beyond the change itself.
    /// It still keeps the marking version, the log, the progress and the
    /// records of inner nets, so that what a viewer reads later is what it
    /// would have read with listening on throughout.
    ///
    /// Turned on, it first takes every fireable set it remembers afresh
    /// from the marking, telling no one. Turn it on before answering a
    /// viewer's first request: from then on a `fireable` line goes out
    /// exactly when a set differs from what that viewer's `get_state`
    /// reported. Setting it to what it already is does nothing. A
    /// [`HostServer`] turns it on and off itself.
    pub fn set_listening(&mut self, listening: bool) {
        self.owner.set_listening(listening);
    }

    /// Says whether the program is firing the net: the `running` of every
    /// later line. A paused game says `false`.
    pub fn set_running(&mut self, running: bool) {
        self.owner.set_running(running);
    }

    /// The `heartbeat` line as of now, for the program to send to every
    /// viewer once per [`heartbeat_period`](Host::heartbeat_period).
    pub fn heartbeat(&self) -> String {
        Notification::Heartbeat {
            step_started_at: None,
            executing_transition: None,
            path: None,
            nested_id: None,
            marking_version: self.owner.marking_version(),
            running: self.owner.running(),
        }
        .to_line()
        .expect(HEARTBEAT_WRITES)
    }

    /// How often a viewer expects a [`heartbeat`](Host::heartbeat): 30
    /// seconds, or a [`HostServer`]'s
    /// [`heartbeat_period`](HostServerConfig::heartbeat_period).
    pub fn heartbeat_period(&self) -> Duration {
        self.owner.heartbeat()
    }
}

impl fmt::Debug for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Host").field("owner", &self.owner).finish()
    }
}

/// The `invalid_params` answer to a line that is not a request, with the
/// `id` it had, or `""`. `None`, said on stderr, when it cannot be written
/// as JSON, which an answer of two strings and a code never fails to be.
pub(crate) fn refusal(id: impl Into<String>, message: impl Into<String>) -> Option<String> {
    let id = id.into();
    match Response::error(id.as_str(), ErrorCode::InvalidParams, message).to_line() {
        Ok(line) => Some(line),
        Err(error) => {
            eprintln!(
                "petritype-runner: the answer to a line that is not a request ({id:?}) is not JSON ({error}); left out"
            );
            None
        }
    }
}

/// The lines among `outgoing`. A hosted owner launches nothing and never
/// exits, so lines are all there is.
fn lines(outgoing: Vec<Outgoing>) -> Vec<String> {
    outgoing
        .into_iter()
        .filter_map(|outgoing| match outgoing {
            Outgoing::Line(line) => Some(line),
            Outgoing::Launch(_) | Outgoing::Exit => None,
        })
        .collect()
}

// ----- over TCP ------------------------------------------------------------------

/// How long the listener waits after `accept` fails before it tries again:
/// a failure such as running out of file descriptors lasts until some are
/// given back, and trying again at once would spin.
const ACCEPT_PAUSE: Duration = Duration::from_millis(50);

/// How long dropping a [`HostServer`] waits to connect to its own listener,
/// which wakes the listener's `accept` so that it sees the server has gone.
const WAKE_TIMEOUT: Duration = Duration::from_secs(1);

/// How a [`HostServer`] treats its viewers: how many it lets in, how much
/// it reads from and holds for each, how much of the program's time one
/// [`serve`](HostServer::serve) takes, and how long it waits on a viewer.
///
/// [`Default`] gives the value each field names (since 2026-11-28; until
/// then a server held any number of viewers and arrivals, took every
/// arrival in one `serve`, dropped a viewer 16,384 batches of lines behind
/// whatever their size, let a write to a viewer block for as long as the
/// system did, and kept a connection that asked nothing open for good).
/// Every field must be above zero. Change the fields that matter on a
/// default:
///
/// ```
/// use std::time::Duration;
/// use petritype_runner::HostServerConfig;
///
/// let mut config = HostServerConfig::default();
/// config.max_viewers = 4;
/// config.heartbeat_period = Duration::from_secs(5);
/// assert_eq!(config.max_line_bytes, 4 << 20);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct HostServerConfig {
    /// How many connections may be open at once: 64 by default. The
    /// listener closes a connection past this as soon as it accepts it,
    /// unread and unanswered, and its viewer may connect again once another
    /// has gone.
    pub max_viewers: usize,
    /// The longest request line read, in bytes without its newline: 4 MiB
    /// by default. A longer one is skipped up to its newline without being
    /// held, and answered with `invalid_params` and the `id` `""`.
    pub max_line_bytes: usize,
    /// How many arrivals (a viewer connecting, a request line, a viewer
    /// leaving) may wait for [`serve`](HostServer::serve): 1024 by
    /// default. While they fill it, a viewer's reader waits before it reads
    /// on, so a viewer that sends faster than the program serves is slowed
    /// down, never held in memory.
    pub arrival_queue_capacity: usize,
    /// How many bytes of lines may wait to be written to one viewer: 16
    /// MiB by default. A batch of lines that would take what waits past
    /// this drops the viewer, which connects again and reads the state
    /// afresh. A batch to a viewer with nothing waiting is taken however
    /// long it is, so an answer larger than this still goes out.
    pub writer_backlog_bytes: usize,
    /// How many arrivals one [`serve`](HostServer::serve) takes: 64 by
    /// default. Each request line counts one, and so does each viewer
    /// connecting or leaving, so that neither a flood of requests nor a
    /// viewer that reconnects in a loop holds the program's loop; the rest
    /// wait for the next call.
    pub max_requests_per_tick: usize,
    /// How long one write to a viewer may wait for the viewer to take
    /// bytes: 10 seconds by default. A viewer that takes none for that
    /// long is dropped.
    pub write_timeout: Duration,
    /// How long a viewer may stay connected without making a valid request
    /// (a line that is a request, whatever its method): 30 seconds by
    /// default, counted from when [`serve`](HostServer::serve) lets it in.
    /// One that has made none by then is dropped.
    pub idle_handshake_timeout: Duration,
    /// How often every listening viewer is sent a heartbeat: 30 seconds by
    /// default. [`Host::heartbeat_period`] says the same.
    pub heartbeat_period: Duration,
}

impl Default for HostServerConfig {
    fn default() -> Self {
        HostServerConfig {
            max_viewers: 64,
            max_line_bytes: DEFAULT_MAX_LINE_BYTES,
            arrival_queue_capacity: 1024,
            writer_backlog_bytes: 16 << 20,
            max_requests_per_tick: 64,
            write_timeout: Duration::from_secs(10),
            idle_handshake_timeout: Duration::from_secs(30),
            heartbeat_period: DEFAULT_HEARTBEAT,
        }
    }
}

impl HostServerConfig {
    /// The name of the first field that is zero, if one is.
    fn zero_field(&self) -> Option<&'static str> {
        let counts = [
            ("max_viewers", self.max_viewers),
            ("max_line_bytes", self.max_line_bytes),
            ("arrival_queue_capacity", self.arrival_queue_capacity),
            ("writer_backlog_bytes", self.writer_backlog_bytes),
            ("max_requests_per_tick", self.max_requests_per_tick),
        ];
        let spans = [
            ("write_timeout", self.write_timeout),
            ("idle_handshake_timeout", self.idle_handshake_timeout),
            ("heartbeat_period", self.heartbeat_period),
        ];
        counts
            .into_iter()
            .find(|&(_, count)| count == 0)
            .map(|(name, _)| name)
            .or_else(|| {
                spans
                    .into_iter()
                    .find(|(_, span)| span.is_zero())
                    .map(|(name, _)| name)
            })
    }
}

/// What the listener and the readers tell the server.
enum Arrival {
    /// A viewer connected.
    Joined(u64, TcpStream),
    /// A viewer sent a line.
    Line(u64, String),
    /// A viewer sent a line that could not be read as text, too long or
    /// not UTF-8: the message of its `invalid_params` answer.
    Refused(u64, String),
    /// A viewer's connection ended.
    Left(u64),
}

/// What the server shares with its listener and its readers.
#[derive(Default)]
struct Shared {
    /// Set when the server is dropped.
    closed: AtomicBool,
    /// A handle to every connection the listener let in whose reader has
    /// not ended, by viewer id: how many there are is what
    /// [`max_viewers`](HostServerConfig::max_viewers) caps, and the
    /// server shuts every one down when it is dropped, those still waiting
    /// among the arrivals included, which ends their readers.
    open: Mutex<HashMap<u64, TcpStream>>,
}

/// One connected viewer.
struct Viewer {
    id: u64,
    /// To the thread that writes to it.
    lines: Sender<Batch>,
    /// How many bytes of lines wait for that thread: what
    /// [`writer_backlog_bytes`](HostServerConfig::writer_backlog_bytes)
    /// caps. The server adds, the writer takes away what it wrote.
    queued: Arc<AtomicUsize>,
    /// The connection, to close it.
    stream: TcpStream,
    /// Set by its first valid request. Until then it is told nothing: it
    /// has no state for a notification to apply to.
    listening: bool,
    /// When [`serve`](HostServer::serve) let it in, from which its
    /// [`idle_handshake_timeout`](HostServerConfig::idle_handshake_timeout)
    /// counts.
    joined: Instant,
}

/// A [`Host`] behind a TCP listener: the program's net, watched by any
/// number of viewers up to a cap.
///
/// [`bind`](HostServer::bind) starts listening, or
/// [`bind_with`](HostServer::bind_with) with a [`HostServerConfig`] of the
/// program's own. The program then calls [`change`](HostServer::change)
/// wherever it fires or changes the net, and [`serve`](HostServer::serve)
/// once per frame or tick to let viewers in and answer them. Neither waits
/// for a viewer: connections are accepted, read and written on threads of
/// their own, what one `serve` takes is capped, and a viewer that falls too
/// far behind, takes no bytes for too long or never makes a valid request
/// is dropped; it connects again and reads the state afresh.
///
/// Each connection speaks the protocol as newline-delimited JSON, the way a
/// net process does on its pipes, with no `describe` or `load` needed: the
/// first request is `get_state`. A viewer is told nothing until its first
/// valid request, and everything from then on (since 2026-11-28; until
/// then its first line of any kind started the telling). A request line is
/// at most [`max_line_bytes`](HostServerConfig::max_line_bytes) long
/// without its newline, 4 MiB by default (since 2026-11-28): a longer one
/// is skipped up to its newline without being held and answered with
/// `invalid_params` and the `id` `""`, as a line that is not JSON is, and
/// the viewer's later lines are read as ever; a line that is not UTF-8 is
/// answered the same way, where it was read with its bad bytes replaced.
///
/// While no viewer has made its first request, which is how a game spends
/// most of its time, `change` makes no lines: it keeps the log, the
/// progress and the marking version and leaves out the rest of the work
/// ([`Host::set_listening`]). The first valid request of the first viewer
/// to listen turns the telling on before it is answered, and the last
/// listening viewer leaving turns it off again.
///
/// It is `Send` and `Sync`, so it can live wherever the program keeps its
/// state: a Bevy resource, for one.
///
/// # Loopback only
///
/// It is made for a viewer on the same machine, run by the person who
/// wrote the net. Anyone who can connect can read the whole net, nothing
/// is authenticated or encrypted, and its limits are there to keep the
/// program responsive in front of a viewer with a bug, not to stand up to
/// one that means harm. Bind it to the loopback address (`127.0.0.1:0`
/// picks a free port); bind it elsewhere only on a network where every
/// machine that can reach the port may read the net.
///
/// ```no_run
/// use petritype::{NetBuilder, TakeEdge};
/// use petritype_runner::HostServer;
///
/// let mut b = NetBuilder::new();
/// let inbox = b.place::<u32>("Inbox");
/// let out = b.place::<u32>("Out");
/// b.seed(inbox, 0..100);
/// b.transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,));
/// let net = b.build().expect("the net builds");
///
/// let mut server = HostServer::bind("127.0.0.1:7878", "doubler", net)?;
/// loop {
///     // The program's own tick: fire, then let the viewers catch up.
///     let fired = server.change(|net| net.step()).expect("the body does not fail");
///     server.serve();
///     if fired.is_none() {
///         break;
///     }
/// }
/// # Ok::<(), std::io::Error>(())
/// ```
pub struct HostServer {
    host: Host,
    address: SocketAddr,
    config: HostServerConfig,
    /// A `Mutex` only because a `Receiver` is not `Sync`, and a program may
    /// keep its server where every thread can see it.
    arrivals: Mutex<Receiver<Arrival>>,
    viewers: Vec<Viewer>,
    last_heartbeat: Instant,
    shared: Arc<Shared>,
}

impl HostServer {
    /// Hosts `net` as `name` and listens for viewers on `address`, with the
    /// [default](HostServerConfig::default) limits.
    ///
    /// # Errors
    ///
    /// When the address cannot be bound (in use, not this machine's) or the
    /// listening thread cannot be started.
    pub fn bind(address: impl ToSocketAddrs, name: &'static str, net: Net) -> io::Result<Self> {
        HostServer::bind_with(address, name, net, HostServerConfig::default())
    }

    /// [`bind`](HostServer::bind), with the limits `config` sets.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::InvalidInput`] when a field of `config` is zero,
    /// naming it; otherwise as [`bind`](HostServer::bind).
    pub fn bind_with(
        address: impl ToSocketAddrs,
        name: &'static str,
        net: Net,
        config: HostServerConfig,
    ) -> io::Result<Self> {
        if let Some(field) = config.zero_field() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("HostServerConfig::{field} must be above zero"),
            ));
        }
        let listener = TcpListener::bind(address)?;
        let address = listener.local_addr()?;
        let (to_server, arrivals) = mpsc::sync_channel(config.arrival_queue_capacity);
        let shared = Arc::new(Shared::default());
        thread::Builder::new()
            .name("petritype host listener".into())
            .spawn({
                let shared = Arc::clone(&shared);
                let limits = (config.max_viewers, config.max_line_bytes);
                move || listen(&listener, &to_server, &shared, limits)
            })?;
        let mut host = Host::with_heartbeat(name, net, config.heartbeat_period);
        // Nobody listens until a viewer's first request.
        host.set_listening(false);
        Ok(HostServer {
            host,
            address,
            config,
            arrivals: Mutex::new(arrivals),
            viewers: Vec::new(),
            last_heartbeat: Instant::now(),
            shared,
        })
    }

    /// The address it listens on: the port the system picked, when it was
    /// bound to port 0.
    pub fn local_addr(&self) -> SocketAddr {
        self.address
    }

    /// The limits it was bound with.
    pub fn config(&self) -> &HostServerConfig {
        &self.config
    }

    /// The net, to read.
    pub fn net(&self) -> &Net {
        self.host.net()
    }

    /// How many viewers [`serve`](HostServer::serve) has let in and not yet
    /// forgotten.
    pub fn viewers(&self) -> usize {
        self.viewers.len()
    }

    /// Hands the net to `change`, tells every listening viewer what it did,
    /// and returns what it returned; see [`Host::change`]. With no viewer
    /// listening it tells nobody and makes no lines, and keeps the log and
    /// the progress all the same. Never waits for a viewer: one whose
    /// backlog the lines would take past
    /// [`writer_backlog_bytes`](HostServerConfig::writer_backlog_bytes)
    /// is dropped instead.
    pub fn change<R>(&mut self, change: impl FnOnce(&mut Net) -> R) -> R {
        let (result, lines) = self.host.change(SystemTime::now(), change);
        self.broadcast(lines);
        result
    }

    /// Says whether the program is firing the net; see
    /// [`Host::set_running`].
    pub fn set_running(&mut self, running: bool) {
        self.host.set_running(running);
    }

    /// Takes what has arrived since the last call, up to
    /// [`max_requests_per_tick`](HostServerConfig::max_requests_per_tick)
    /// arrivals in their order: lets in the viewers that have connected,
    /// answers the requests and forgets the viewers that have gone. Then
    /// drops the viewers that have made no valid request within the
    /// [`idle_handshake_timeout`](HostServerConfig::idle_handshake_timeout),
    /// and sends the heartbeat when it is due. Never waits for a viewer,
    /// and what one call does is capped, so it can be called once per
    /// frame or tick: what is left over waits for the next call.
    ///
    /// A viewer's first valid request, when no other viewer is listening,
    /// turns the host's telling on before the request is answered;
    /// forgetting the last listening viewer turns it off. A line from a
    /// viewer already forgotten is not answered (since 2026-11-28; until
    /// then it was answered, to nobody).
    pub fn serve(&mut self) {
        for _ in 0..self.config.max_requests_per_tick {
            let arrival = self
                .arrivals
                .get_mut()
                .unwrap_or_else(PoisonError::into_inner)
                .try_recv();
            match arrival {
                Ok(arrival) => self.arrive(arrival),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        self.drop_idle(Instant::now());
        if self.last_heartbeat.elapsed() >= self.host.heartbeat_period() {
            self.last_heartbeat = Instant::now();
            self.broadcast(vec![self.host.heartbeat()]);
        }
    }

    /// Takes one arrival.
    fn arrive(&mut self, arrival: Arrival) {
        match arrival {
            Arrival::Joined(id, stream) => self.admit(id, stream),
            Arrival::Line(id, line) => {
                let Some(at) = self.position(id) else {
                    return;
                };
                let answer = match Request::parse(&line) {
                    Ok(request) => {
                        self.start_listening(at);
                        self.host.answer(request, SystemTime::now())
                    }
                    Err(malformed) => refusal(malformed.id, malformed.message)
                        .into_iter()
                        .collect(),
                };
                self.send_to(at, answer);
            }
            Arrival::Refused(id, message) => {
                if let Some(at) = self.position(id) {
                    self.send_to(at, refusal("", message).into_iter().collect());
                }
            }
            Arrival::Left(id) => {
                if let Some(at) = self.position(id) {
                    self.drop_viewer(at);
                }
            }
        }
    }

    /// Where the viewer `id` is among the viewers, if it has not been
    /// forgotten.
    fn position(&self, id: u64) -> Option<usize> {
        self.viewers.iter().position(|viewer| viewer.id == id)
    }

    /// The viewer at `at` has made a valid request. If it is its first, it
    /// listens from now on, and the host tells again if it was quiet,
    /// before the answer is made, so that what follows the answer starts
    /// from it.
    fn start_listening(&mut self, at: usize) {
        if !self.viewers[at].listening {
            self.viewers[at].listening = true;
            self.host.set_listening(true);
        }
    }

    /// Sends `lines` to the viewer at `at`, dropping it if it cannot take
    /// them.
    fn send_to(&mut self, at: usize, lines: Vec<String>) {
        if !send(
            &self.viewers[at],
            &batch(&lines),
            self.config.writer_backlog_bytes,
        ) {
            self.drop_viewer(at);
        }
    }

    /// Starts writing to a viewer that has connected.
    fn admit(&mut self, id: u64, stream: TcpStream) {
        // A connection that cannot be written to is shut down: that ends its
        // reader, which tells the server it left, and the server, knowing no
        // viewer by that id, lets it go.
        let refuse = |stream: &TcpStream| {
            let _ = stream.shutdown(Shutdown::Both);
        };
        let Ok(writing) = stream.try_clone() else {
            return refuse(&stream);
        };
        if writing
            .set_write_timeout(Some(self.config.write_timeout))
            .is_err()
        {
            return refuse(&stream);
        }
        let (lines, backlog) = mpsc::channel::<Batch>();
        let queued = Arc::new(AtomicUsize::new(0));
        let started = thread::Builder::new()
            .name("petritype host writer".into())
            .spawn({
                let queued = Arc::clone(&queued);
                move || write_lines(&writing, &backlog, &queued)
            });
        if started.is_err() {
            return refuse(&stream);
        }
        self.viewers.push(Viewer {
            id,
            lines,
            queued,
            stream,
            listening: false,
            joined: Instant::now(),
        });
    }

    /// Drops every viewer that has made no valid request within the
    /// handshake timeout, as of `now`.
    fn drop_idle(&mut self, now: Instant) {
        let timeout = self.config.idle_handshake_timeout;
        let mut at = 0;
        while at < self.viewers.len() {
            let viewer = &self.viewers[at];
            if !viewer.listening && now.saturating_duration_since(viewer.joined) >= timeout {
                self.drop_viewer(at);
            } else {
                at += 1;
            }
        }
    }

    /// Sends `lines` to every viewer that is listening, dropping any that
    /// cannot take them.
    fn broadcast(&mut self, lines: Vec<String>) {
        if lines.is_empty() {
            return;
        }
        // Written out once, whatever the number of viewers it goes to.
        let batch = batch(&lines);
        let mut at = 0;
        while at < self.viewers.len() {
            let viewer = &self.viewers[at];
            if !viewer.listening || send(viewer, &batch, self.config.writer_backlog_bytes) {
                at += 1;
            } else {
                self.drop_viewer(at);
            }
        }
    }

    /// Closes the connection of the viewer at `at` and forgets it. When no
    /// viewer is left listening, the host stops telling.
    fn drop_viewer(&mut self, at: usize) {
        let viewer = self.viewers.swap_remove(at);
        let _ = viewer.stream.shutdown(Shutdown::Both);
        if !self.viewers.iter().any(|viewer| viewer.listening) {
            self.host.set_listening(false);
        }
    }
}

impl Drop for HostServer {
    /// Closes every connection and stops the listener, without waiting for
    /// any thread: each reader and writer ends once its connection is shut
    /// down, and the listener once it is woken.
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::Release);
        // Every connection the listener let in, those still waiting among
        // the arrivals included: a reader whose connection was only
        // dropped, not shut down, would wait on it until the viewer hung up.
        for (_, stream) in lock(&self.shared.open).drain() {
            let _ = stream.shutdown(Shutdown::Both);
        }
        for viewer in &self.viewers {
            let _ = viewer.stream.shutdown(Shutdown::Both);
        }
        // What waits among the arrivals, as much as there was room for: a
        // reader waiting to add one more goes on, and stops at the next.
        let arrivals = self
            .arrivals
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner);
        for arrival in arrivals.try_iter().take(self.config.arrival_queue_capacity) {
            if let Arrival::Joined(_, stream) = arrival {
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
        // The listener is waiting in `accept`: a connection wakes it, and
        // it sees the flag. An address that names every interface is
        // reached through loopback.
        let _ = TcpStream::connect_timeout(&wake_address(self.address), WAKE_TIMEOUT);
    }
}

impl fmt::Debug for HostServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostServer")
            .field("address", &self.address)
            .field("viewers", &self.viewers.len())
            .field("config", &self.config)
            .field("host", &self.host)
            .finish_non_exhaustive()
    }
}

/// The address to connect to to reach a listener bound to `address`: the
/// loopback address of its family where it names every interface (`0.0.0.0`,
/// `[::]`), which is no address to connect to everywhere.
fn wake_address(address: SocketAddr) -> SocketAddr {
    let ip = match address.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    SocketAddr::new(ip, address.port())
}

/// A batch of lines as a viewer's writer takes it: each line and its
/// newline, in one text that every viewer it goes to shares.
type Batch = Arc<str>;

/// `lines` as one [`Batch`].
fn batch(lines: &[String]) -> Batch {
    let mut text = String::with_capacity(lines.iter().map(|line| line.len() + 1).sum());
    for line in lines {
        text.push_str(line);
        text.push('\n');
    }
    Arc::from(text)
}

/// Hands `batch` to a viewer's writer. False when the viewer has gone, or
/// when lines already wait for it and these would take what waits past
/// `cap` bytes.
fn send(viewer: &Viewer, batch: &Batch, cap: usize) -> bool {
    let bytes = batch.len();
    // Only the server adds to it, so it can only have fallen since.
    let queued = viewer.queued.load(Ordering::Acquire);
    if queued > 0 && queued.saturating_add(bytes) > cap {
        return false;
    }
    viewer.queued.fetch_add(bytes, Ordering::AcqRel);
    viewer.lines.send(Arc::clone(batch)).is_ok()
}

/// Accepts connections until the server is dropped, and starts a reader
/// for each, while fewer than `max_viewers` are open; one past that is
/// closed at once. Each line of a reader is at most `max_line` bytes.
fn listen(
    listener: &TcpListener,
    to_server: &SyncSender<Arrival>,
    shared: &Arc<Shared>,
    (max_viewers, max_line): (usize, usize),
) {
    let mut next_id = 0_u64;
    loop {
        let stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => {
                if shared.closed.load(Ordering::Acquire) {
                    return;
                }
                thread::sleep(ACCEPT_PAUSE);
                continue;
            }
        };
        let close = |stream: &TcpStream| {
            let _ = stream.shutdown(Shutdown::Both);
        };
        if shared.closed.load(Ordering::Acquire) {
            return close(&stream);
        }
        // Lines are small and a viewer waits for each answer.
        let _ = stream.set_nodelay(true);
        let (Ok(reading), Ok(kept)) = (stream.try_clone(), stream.try_clone()) else {
            close(&stream);
            continue;
        };
        next_id += 1;
        let id = next_id;
        {
            // Under the lock the server's drop takes: either the server sees
            // this connection and closes it, or this sees the server gone.
            let mut open = lock(&shared.open);
            if shared.closed.load(Ordering::Acquire) {
                return close(&stream);
            }
            if open.len() >= max_viewers {
                close(&stream);
                continue;
            }
            open.insert(id, kept);
        }
        if to_server.send(Arrival::Joined(id, stream)).is_err() {
            return;
        }
        let started = thread::Builder::new()
            .name("petritype host reader".into())
            .spawn({
                let to_server = to_server.clone();
                let shared = Arc::clone(shared);
                move || read_lines(id, reading, &to_server, &shared, max_line)
            });
        if started.is_err() {
            // Nobody will read it: the server closes it when told it left.
            lock(&shared.open).remove(&id);
            if to_server.send(Arrival::Left(id)).is_err() {
                return;
            }
        }
    }
}

/// One viewer's requests, line by line, each at most `max` bytes, into the
/// server's channel. A line too long or not UTF-8 is told as refused, and
/// answered as a line that is not JSON is (PROTOCOL.md §2); until
/// 2026-11-28 a line of any length was read whole, and one that was not
/// UTF-8 was read with its bad bytes replaced. When the connection ends,
/// its place among the open connections is given up before the server is
/// told it left, so a viewer the server has forgotten no longer counts
/// against the cap.
fn read_lines(
    id: u64,
    stream: TcpStream,
    to_server: &SyncSender<Arrival>,
    shared: &Shared,
    max: usize,
) {
    for line in Lines::new(BufReader::new(stream), max) {
        let arrival = match line {
            Line::Text(text) => Arrival::Line(id, text),
            Line::Refused(message) => Arrival::Refused(id, message),
        };
        if to_server.send(arrival).is_err() {
            break;
        }
    }
    lock(&shared.open).remove(&id);
    let _ = to_server.send(Arrival::Left(id));
}

/// Writes whole batches to one viewer, flushing after each and taking what
/// it wrote off `queued`, until the server forgets the viewer or a write
/// fails: the connection broke, or the viewer took no bytes for the write
/// timeout. Then it shuts the connection down, which ends the reader too,
/// and the reader tells the server the viewer left.
fn write_lines(stream: &TcpStream, backlog: &Receiver<Batch>, queued: &AtomicUsize) {
    let mut out = BufWriter::new(stream);
    for batch in backlog {
        let written = out.write_all(batch.as_bytes()).is_ok() && out.flush().is_ok();
        queued.fetch_sub(batch.len(), Ordering::AcqRel);
        if !written {
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
    }
}

fn lock<T>(cell: &Mutex<T>) -> MutexGuard<'_, T> {
    cell.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{self, SyncSender};
    use std::time::{Duration, Instant};

    use petritype::{Net, NetBuilder, TakeEdge};

    use super::{Arrival, Batch, HostServer, HostServerConfig, Viewer, batch, send, wake_address};

    /// The next line from `reader`, serving `server` while it waits.
    fn next(server: &mut HostServer, reader: &mut BufReader<TcpStream>) -> serde_json::Value {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut line = String::new();
        loop {
            server.serve();
            if let Ok(_) = reader.read_line(&mut line)
                && line.ends_with('\n')
            {
                return serde_json::from_str(&line).expect("a JSON line");
            }
            assert!(Instant::now() < deadline, "no line within five seconds");
        }
    }

    #[test]
    fn the_cap_of_a_viewers_lines_is_the_one_given() {
        let net = NetBuilder::new().build().expect("an empty net");
        let config = HostServerConfig {
            max_line_bytes: 32,
            ..HostServerConfig::default()
        };
        let mut server = HostServer::bind_with("127.0.0.1:0", "empty", net, config).unwrap();
        let mut stream = TcpStream::connect(server.local_addr()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        // 33 bytes, then 32.
        let long = r#"{"id": "1", "method": "describe"}"#;
        let short = r#"{"id":"2", "method": "describe"}"#;
        assert_eq!((long.len(), short.len()), (33, 32));
        writeln!(stream, "{long}\n{short}").unwrap();
        let refused = next(&mut server, &mut reader);
        assert_eq!(refused["id"], "");
        assert!(
            refused["error"]["message"]
                .as_str()
                .unwrap()
                .starts_with("a request line is at most 32 bytes long"),
            "{refused}"
        );
        assert_eq!(next(&mut server, &mut reader)["id"], "2");
    }

    /// `In -Move-> Out`, holding 1, whose priority counts in `reads` how
    /// often it is read: once for each `get_state` answered.
    fn counted(reads: &Arc<AtomicUsize>) -> Net {
        let mut b = NetBuilder::new();
        let input = b.place::<u32>("In");
        let output = b.place::<u32>("Out");
        b.seed(input, [1]);
        let reads = Arc::clone(reads);
        b.transition("Move", (TakeEdge(input),), (output,), |n: u32| (n,))
            .priority(move |_| {
                reads.fetch_add(1, Ordering::Relaxed);
                0.0
            });
        b.build().expect("the net builds")
    }

    /// Both ends of a connection: the server's and the viewer's.
    fn pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let viewer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        (server, viewer)
    }

    /// A server of [`counted`] whose arrivals are the test's to send, the
    /// sender, and the count of `get_state` answers.
    fn fed(config: HostServerConfig) -> (HostServer, SyncSender<Arrival>, Arc<AtomicUsize>) {
        let reads = Arc::new(AtomicUsize::new(0));
        let mut server =
            HostServer::bind_with("127.0.0.1:0", "counted", counted(&reads), config).unwrap();
        let (feed, arrivals) = mpsc::sync_channel(64);
        *server.arrivals.get_mut().unwrap() = arrivals;
        (server, feed, reads)
    }

    fn get_state(id: u64) -> Arrival {
        Arrival::Line(id, r#"{"id": "1", "method": "get_state"}"#.into())
    }

    #[test]
    fn a_line_from_a_viewer_already_forgotten_is_not_answered() {
        let (mut server, feed, reads) = fed(HostServerConfig::default());
        let (stream, _viewer) = pair();
        feed.send(Arrival::Joined(1, stream)).unwrap();
        feed.send(get_state(1)).unwrap();
        server.serve();
        assert_eq!(reads.load(Ordering::Relaxed), 1, "the viewer's request");

        // Gone before its next line is taken, or never there.
        feed.send(Arrival::Left(1)).unwrap();
        feed.send(get_state(1)).unwrap();
        feed.send(get_state(2)).unwrap();
        server.serve();
        assert_eq!(server.viewers(), 0);
        assert_eq!(
            reads.load(Ordering::Relaxed),
            1,
            "nothing answered for nobody"
        );
    }

    #[test]
    fn one_serve_takes_at_most_max_requests_per_tick_arrivals() {
        let config = HostServerConfig {
            max_requests_per_tick: 3,
            ..HostServerConfig::default()
        };
        let (mut server, feed, reads) = fed(config);
        let (stream, _viewer) = pair();
        feed.send(Arrival::Joined(1, stream)).unwrap();
        for _ in 0..7 {
            feed.send(get_state(1)).unwrap();
        }
        // The join counts as one of the three.
        let answered: Vec<usize> = (0..4)
            .map(|_| {
                server.serve();
                reads.load(Ordering::Relaxed)
            })
            .collect();
        assert_eq!(answered, [2, 5, 7, 7]);
    }

    #[test]
    fn a_first_line_that_is_no_request_does_not_start_the_telling() {
        let (mut server, feed, _) = fed(HostServerConfig::default());
        let (stream, _viewer) = pair();
        feed.send(Arrival::Joined(1, stream)).unwrap();
        feed.send(Arrival::Line(1, "not json".into())).unwrap();
        feed.send(Arrival::Refused(1, "too long".into())).unwrap();
        server.serve();
        assert!(!server.viewers[0].listening);
        assert!(format!("{server:?}").contains("listening: false"));
        feed.send(get_state(1)).unwrap();
        server.serve();
        assert!(server.viewers[0].listening);
        assert!(format!("{server:?}").contains("listening: true"));
    }

    #[test]
    fn a_viewer_with_no_valid_request_is_dropped_at_the_handshake_timeout() {
        let config = HostServerConfig {
            idle_handshake_timeout: Duration::from_secs(30),
            ..HostServerConfig::default()
        };
        let (mut server, feed, _) = fed(config);
        for id in [1, 2] {
            let (stream, _viewer) = pair();
            feed.send(Arrival::Joined(id, stream)).unwrap();
        }
        feed.send(Arrival::Line(2, "not json".into())).unwrap();
        feed.send(Arrival::Joined(3, pair().0)).unwrap();
        feed.send(get_state(3)).unwrap();
        server.serve();
        assert_eq!(server.viewers(), 3);
        let joined = server.viewers.iter().map(|v| v.joined).max().unwrap();

        server.drop_idle(joined + Duration::from_secs(29));
        assert_eq!(server.viewers(), 3, "not yet");
        server.drop_idle(joined + Duration::from_secs(30));
        let left: Vec<u64> = server.viewers.iter().map(|v| v.id).collect();
        assert_eq!(left, [3], "the one that asked stays");
        server.drop_idle(joined + Duration::from_secs(3_000));
        assert_eq!(server.viewers(), 1);
    }

    /// A viewer whose writer takes nothing: its lines wait.
    fn stalled() -> (Viewer, mpsc::Receiver<Batch>) {
        let (lines, backlog) = mpsc::channel();
        let viewer = Viewer {
            id: 1,
            lines,
            queued: Arc::new(AtomicUsize::new(0)),
            stream: pair().0,
            listening: true,
            joined: Instant::now(),
        };
        (viewer, backlog)
    }

    #[test]
    fn lines_past_the_backlog_cap_are_refused_but_a_lone_batch_always_goes() {
        let line = |bytes: usize| batch(&["x".repeat(bytes - 1)]);
        let (viewer, _backlog) = stalled();
        // Nothing waits: taken however long.
        assert!(send(&viewer, &line(500), 100));
        let (viewer, _backlog) = stalled();
        assert!(send(&viewer, &line(60), 100));
        assert!(send(&viewer, &line(40), 100), "exactly the cap");
        assert_eq!(viewer.queued.load(Ordering::Relaxed), 100);
        assert!(!send(&viewer, &line(1), 100), "one byte past it");
        // Once the writer has taken some, there is room again.
        viewer.queued.fetch_sub(60, Ordering::Relaxed);
        assert!(send(&viewer, &line(60), 100));
    }

    #[test]
    fn a_config_with_a_field_of_zero_is_refused() {
        let net = || NetBuilder::new().build().expect("an empty net");
        let config = HostServerConfig {
            max_viewers: 0,
            ..HostServerConfig::default()
        };
        let error = HostServer::bind_with("127.0.0.1:0", "empty", net(), config).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(
            error.to_string(),
            "HostServerConfig::max_viewers must be above zero"
        );
        let config = HostServerConfig {
            write_timeout: Duration::ZERO,
            ..HostServerConfig::default()
        };
        let error = HostServer::bind_with("127.0.0.1:0", "empty", net(), config).unwrap_err();
        assert!(error.to_string().contains("write_timeout"), "{error}");
    }

    #[test]
    fn an_address_of_every_interface_is_woken_through_loopback() {
        let v4 = SocketAddr::from((Ipv4Addr::UNSPECIFIED, 7878));
        let v6 = SocketAddr::from((Ipv6Addr::UNSPECIFIED, 7878));
        assert_eq!(
            wake_address(v4),
            SocketAddr::from((Ipv4Addr::LOCALHOST, 7878))
        );
        assert_eq!(
            wake_address(v6),
            SocketAddr::from((Ipv6Addr::LOCALHOST, 7878))
        );
        let named = SocketAddr::from((Ipv4Addr::new(192, 168, 1, 2), 7878));
        assert_eq!(wake_address(named), named);
    }
}
