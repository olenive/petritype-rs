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
//! - [`HostServer`] is a [`Host`] behind a TCP listener: any number of
//!   viewers connect, each speaks the protocol line by line, and every one
//!   of them is told what the program does to the net.
//!
//! A host that nobody is listening to makes no lines: it keeps the log, the
//! progress and the marking version, and leaves out the work of telling.
//! [`Host::set_listening`] says when that is; a [`HostServer`] works it out
//! from its viewers.

use std::fmt;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use petritype::Net;

use crate::owner::{Input, Outgoing, Owner};
use crate::protocol::{ErrorCode, Notification, Request, Response};

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
        Host {
            owner: Owner::hosted(name, net, Arc::default(), DEFAULT_HEARTBEAT),
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
            Ok(request) => lines(self.owner.handle(Input::Request(request), now)),
            Err(malformed) => {
                vec![
                    Response::error(malformed.id, ErrorCode::InvalidParams, malformed.message)
                        .to_line(),
                ]
            }
        }
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
            marking_version: self.owner.marking_version(),
            running: self.owner.running(),
        }
        .to_line()
    }

    /// How often a viewer expects a [`heartbeat`](Host::heartbeat): 30
    /// seconds.
    pub fn heartbeat_period(&self) -> Duration {
        self.owner.heartbeat()
    }
}

impl fmt::Debug for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Host").field("owner", &self.owner).finish()
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

/// How many lines may wait for one viewer before it is dropped as too far
/// behind. A viewer that is dropped connects again and reads the state
/// afresh.
const BACKLOG: usize = 16_384;

/// What the listener and the readers tell the server.
enum Arrival {
    /// A viewer connected.
    Joined(u64, TcpStream),
    /// A viewer sent a line.
    Line(u64, String),
    /// A viewer's connection ended.
    Left(u64),
}

/// One connected viewer.
struct Viewer {
    id: u64,
    /// To the thread that writes to it.
    lines: SyncSender<Vec<String>>,
    /// The connection, to close it.
    stream: TcpStream,
    /// Set by its first request. Until then it is told nothing: it has no
    /// state for a notification to apply to.
    listening: bool,
}

/// A [`Host`] behind a TCP listener: the program's net, watched by any
/// number of viewers.
///
/// [`bind`](HostServer::bind) starts listening. The program then calls
/// [`change`](HostServer::change) wherever it fires or changes the net, and
/// [`serve`](HostServer::serve) once per frame or tick to let viewers in
/// and answer them. Neither blocks on a viewer: connections are accepted,
/// read and written on threads of their own, and a viewer that falls too
/// far behind is dropped.
///
/// Each connection speaks the protocol as newline-delimited JSON, the way a
/// net process does on its pipes, with no `describe` or `load` needed: the
/// first request is `get_state`. A viewer is told nothing until its first
/// request, and everything from then on.
///
/// While no viewer has made its first request, which is how a game spends
/// most of its time, `change` makes no lines: it keeps the log, the
/// progress and the marking version and leaves out the rest of the work
/// ([`Host::set_listening`]). The first request of the first viewer to
/// listen turns the telling on before it is answered, and the last
/// listening viewer leaving turns it off again.
///
/// It is `Send` and `Sync`, so it can live wherever the program keeps its
/// state: a Bevy resource, for one.
///
/// Anyone who can connect can read the net. Bind to the loopback address
/// (`127.0.0.1:0` picks a free port) unless every machine that can reach
/// the port may.
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
    /// A `Mutex` only because a `Receiver` is not `Sync`, and a program may
    /// keep its server where every thread can see it.
    arrivals: Mutex<Receiver<Arrival>>,
    viewers: Vec<Viewer>,
    last_heartbeat: Instant,
    /// Tells the listener to stop, when the server is dropped.
    closed: Arc<AtomicBool>,
}

impl HostServer {
    /// Hosts `net` as `name` and listens for viewers on `address`.
    ///
    /// # Errors
    ///
    /// When the address cannot be bound (in use, not this machine's) or the
    /// listening thread cannot be started.
    pub fn bind(address: impl ToSocketAddrs, name: &'static str, net: Net) -> io::Result<Self> {
        let listener = TcpListener::bind(address)?;
        let address = listener.local_addr()?;
        let (to_server, arrivals) = mpsc::channel();
        let closed = Arc::new(AtomicBool::new(false));
        thread::Builder::new()
            .name("petritype host listener".into())
            .spawn({
                let closed = Arc::clone(&closed);
                move || listen(&listener, &to_server, &closed)
            })?;
        let mut host = Host::new(name, net);
        // Nobody listens until a viewer's first request.
        host.set_listening(false);
        Ok(HostServer {
            host,
            address,
            arrivals: Mutex::new(arrivals),
            viewers: Vec::new(),
            last_heartbeat: Instant::now(),
            closed,
        })
    }

    /// The address it listens on: the port the system picked, when it was
    /// bound to port 0.
    pub fn local_addr(&self) -> SocketAddr {
        self.address
    }

    /// The net, to read.
    pub fn net(&self) -> &Net {
        self.host.net()
    }

    /// How many viewers are connected.
    pub fn viewers(&self) -> usize {
        self.viewers.len()
    }

    /// Hands the net to `change`, tells every listening viewer what it did,
    /// and returns what it returned; see [`Host::change`]. With no viewer
    /// listening it tells nobody and makes no lines, and keeps the log and
    /// the progress all the same. Never blocks on a viewer.
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

    /// Lets in the viewers that have connected, answers the requests that
    /// have arrived, forgets the viewers that have gone, and sends the
    /// heartbeat when it is due. Never blocks: call it once per frame or
    /// tick.
    ///
    /// A viewer's first request, when no other viewer is listening, turns
    /// the host's telling on before the request is answered; forgetting
    /// the last listening viewer turns it off.
    pub fn serve(&mut self) {
        loop {
            let arrival = self
                .arrivals
                .get_mut()
                .unwrap_or_else(PoisonError::into_inner)
                .try_recv();
            match arrival {
                Ok(Arrival::Joined(id, stream)) => self.admit(id, stream),
                Ok(Arrival::Line(id, line)) => {
                    let at = self.viewers.iter().position(|viewer| viewer.id == id);
                    if let Some(at) = at
                        && !self.viewers[at].listening
                    {
                        // Its first request. The host tells again, if it
                        // was quiet, before the answer is made, so that
                        // what follows the answer starts from it.
                        self.viewers[at].listening = true;
                        self.host.set_listening(true);
                    }
                    let answer = self.host.request(&line, SystemTime::now());
                    if let Some(at) = at
                        && !send(&self.viewers[at], answer)
                    {
                        self.drop_viewer(at);
                    }
                }
                Ok(Arrival::Left(id)) => {
                    if let Some(at) = self.viewers.iter().position(|viewer| viewer.id == id) {
                        self.drop_viewer(at);
                    }
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        if self.last_heartbeat.elapsed() >= self.host.heartbeat_period() {
            self.last_heartbeat = Instant::now();
            self.broadcast(vec![self.host.heartbeat()]);
        }
    }

    /// Starts writing to a viewer that has connected.
    fn admit(&mut self, id: u64, stream: TcpStream) {
        let Ok(writing) = stream.try_clone() else {
            let _ = stream.shutdown(Shutdown::Both);
            return;
        };
        let (lines, backlog) = mpsc::sync_channel::<Vec<String>>(BACKLOG);
        let started = thread::Builder::new()
            .name("petritype host writer".into())
            .spawn(move || write_lines(writing, &backlog));
        if started.is_err() {
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
        self.viewers.push(Viewer {
            id,
            lines,
            stream,
            listening: false,
        });
    }

    /// Sends `lines` to every viewer that is listening, dropping any that
    /// cannot take them.
    fn broadcast(&mut self, lines: Vec<String>) {
        if lines.is_empty() {
            return;
        }
        let mut at = 0;
        while at < self.viewers.len() {
            let viewer = &self.viewers[at];
            if !viewer.listening || send(viewer, lines.clone()) {
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
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Relaxed);
        for viewer in &self.viewers {
            let _ = viewer.stream.shutdown(Shutdown::Both);
        }
        // The listener is waiting in `accept`: a connection wakes it, and
        // it sees the flag.
        let _ = TcpStream::connect(self.address);
    }
}

impl fmt::Debug for HostServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostServer")
            .field("address", &self.address)
            .field("viewers", &self.viewers.len())
            .field("host", &self.host)
            .finish_non_exhaustive()
    }
}

/// Hands `lines` to a viewer's writer. False when the viewer has gone or is
/// a full backlog behind.
fn send(viewer: &Viewer, lines: Vec<String>) -> bool {
    match viewer.lines.try_send(lines) {
        Ok(()) => true,
        Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => false,
    }
}

/// Accepts connections until the server is dropped, starting a reader for
/// each.
fn listen(listener: &TcpListener, to_server: &Sender<Arrival>, closed: &AtomicBool) {
    let mut next_id = 0_u64;
    for stream in listener.incoming() {
        if closed.load(Ordering::Relaxed) {
            return;
        }
        let Ok(stream) = stream else {
            continue;
        };
        // Lines are small and a viewer waits for each answer.
        let _ = stream.set_nodelay(true);
        let Ok(reading) = stream.try_clone() else {
            continue;
        };
        next_id += 1;
        let id = next_id;
        if to_server.send(Arrival::Joined(id, stream)).is_err() {
            return;
        }
        let started = thread::Builder::new()
            .name("petritype host reader".into())
            .spawn({
                let to_server = to_server.clone();
                move || read_lines(id, reading, &to_server)
            });
        // Nobody will read it: the server closes it when told it left.
        if started.is_err() && to_server.send(Arrival::Left(id)).is_err() {
            return;
        }
    }
}

/// One viewer's requests, line by line, into the server's channel.
fn read_lines(id: u64, stream: TcpStream, to_server: &Sender<Arrival>) {
    let mut reader = BufReader::new(stream);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let line = String::from_utf8_lossy(&buffer);
        let line = line.trim_end_matches(['\n', '\r']);
        if line.trim().is_empty() {
            continue;
        }
        if to_server.send(Arrival::Line(id, line.to_owned())).is_err() {
            return;
        }
    }
    let _ = to_server.send(Arrival::Left(id));
}

/// Writes whole batches to one viewer, flushing after each, until the
/// server forgets the viewer or the connection fails.
fn write_lines(stream: TcpStream, backlog: &Receiver<Vec<String>>) {
    let mut out = BufWriter::new(stream);
    for lines in backlog {
        for line in lines {
            if writeln!(out, "{line}").is_err() {
                return;
            }
        }
        if out.flush().is_err() {
            return;
        }
    }
}
