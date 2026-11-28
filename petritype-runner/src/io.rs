//! The outer layer, and the only one that does I/O: the stdout guard, the
//! threads, and the loop around [`Owner`] (DESIGN §19.3.2 item 3).

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{self, Write};
use std::process::ExitCode;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use petritype::Firing;

use crate::host::refusal;
use crate::lines::{self, Line, Lines, MAX_LINE_ENV};
use crate::owner::{Gauges, InnerAddress, Input, Outgoing, Owner, StatusCell};
use crate::protocol::{self, HEARTBEAT_WRITES, Notification, Request};
use crate::registry::NetEntry;
use crate::time::iso8601;

/// The heartbeat and tick period when the environment does not set one.
const DEFAULT_HEARTBEAT: Duration = Duration::from_secs(30);

/// The environment variable that overrides the heartbeat period, in
/// milliseconds. Meant for tests, which cannot wait 30 seconds.
pub(crate) const HEARTBEAT_ENV: &str = "PETRITYPE_RUNNER_HEARTBEAT_MS";

/// How many messages may wait for the owner loop: requests, landings and
/// ticks. While that many wait, the reader reads no more of stdin, so a
/// host that writes faster than the net is served is slowed down, never
/// held in memory.
const OWNER_QUEUE: usize = 1024;

/// How many batches of lines may wait for the writer. While that many wait,
/// the host is reading nothing, and the owner loop waits for it.
const WRITER_QUEUE: usize = 1024;

/// How long the process waits at exit for the writer to write what waits:
/// a host that reads nothing more cannot keep the process from ending.
const EXIT_FLUSH: Duration = Duration::from_secs(5);

const USAGE: &str = "usage: <binary> [--net <name>]              serve the net protocol on stdin and stdout\n       \
                     <binary> [--net <name>] --describe   print the nets this binary carries as one JSON line\n\n  \
                     --net <name>   carry the net registered as <name> alone, so that describe lists it only";

/// Runs a net process: the whole of a runner binary's `main`.
///
/// - With no arguments, serves the net protocol on stdin and stdout until
///   `shutdown` or end of file, and exits 0.
/// - With `--describe`, prints the `describe` answer as one JSON line and
///   exits 0; this is how a build step learns what the binary carries.
/// - With `--net <name>`, before or after `--describe` or alone, carries
///   the one net registered as `<name>` and no other, so that `describe`
///   lists that net alone and `load` knows no other. This is how a host
///   replays a shared transcript of PROTOCOL.md §11 against the binary
///   over pipes, since a shared transcript is recorded against a runner
///   carrying its own net alone and may pin `describe` in full. A name the
///   binary does not carry is refused on stderr, with the names it does
///   carry, and the binary exits 2.
/// - With anything else, prints usage on stderr and exits 2.
///
/// While serving, file descriptor 1 is moved, for the rest of the process
/// (Unix only): the protocol keeps a private copy of the real stdout, opened
/// close-on-exec, and fd 1 becomes a copy of stderr. Whatever writes to
/// stdout from then on, a `println!` in a transition body, a library, another
/// thread, lands on stderr instead of corrupting the protocol, and a child
/// process a body starts inherits stderr as its stdout and never the
/// protocol stream. Anything written to stdout before `main` is flushed
/// first. Elsewhere stdout is shared, and nothing else may print to it. So
/// `main` is meant to be the whole of the binary's `main`, called once:
/// called again, it would take stderr for the protocol.
///
/// The heartbeat (and the ticker that drives a running net) fires every 30
/// seconds, or every `PETRITYPE_RUNNER_HEARTBEAT_MS` milliseconds when that
/// variable is set. A heartbeat's `marking_version` is that of the lines
/// written before it, so it never runs ahead of a notification or falls
/// behind one (since 2026-11-28). A request line is at most 4 MiB long
/// without its newline, or `PETRITYPE_RUNNER_MAX_LINE_BYTES` bytes when that
/// variable holds a positive integer (since 2026-11-28; until then a line
/// of any length was read whole): a longer one is skipped up to its newline
/// without being held and answered with `invalid_params` and the `id`
/// `""`, and the process serves the lines after it, as the Python runner
/// does (PROTOCOL.md §2). A tick waits behind every request that has come
/// in, so a request that arrives while a body runs inline is handled when
/// that body lands, before the net fires again: a `stop` sent then is
/// answered after that one body.
///
/// What waits between the threads is bounded (since 2026-11-28): while
/// 1024 requests and landings wait for the owner, stdin is not read
/// further, and while 1024 batches of lines wait to be written, the owner
/// serves nothing more, so neither a host that writes faster than it is
/// served nor one that stops reading is buffered for without end.
/// At exit the process waits 5 seconds at most for its last lines to be
/// written, then ends with whatever the host has not read. It exits 1 when
/// it cannot start one of its threads, and a body whose own thread cannot
/// be started runs on the owner's thread instead.
///
/// `nets` is any list of entries, for example an array:
///
/// ```no_run
/// # use petritype_runner::{NetEntry, Params, LoadError};
/// # fn build(_: &Params) -> Result<petritype::Net, LoadError> { unimplemented!() }
/// fn main() -> std::process::ExitCode {
///     petritype_runner::main([NetEntry::new("my-net", build)])
/// }
/// ```
pub fn main(nets: impl IntoIterator<Item = NetEntry>) -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let Some(command) = parse_args(&args) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let mut nets: Vec<NetEntry> = nets.into_iter().collect();
    if let Some(name) = &command.net {
        match one_net(nets, name) {
            Ok(net) => nets = vec![net],
            Err(message) => {
                eprintln!("{message}");
                return ExitCode::from(2);
            }
        }
    }
    if command.describe {
        let owner = Owner::new(nets, StatusCell::default(), DEFAULT_HEARTBEAT);
        match protocol::to_line(owner.describe()) {
            Ok(line) => {
                println!("{line}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("petritype-runner: the describe answer is not JSON: {error}");
                ExitCode::FAILURE
            }
        }
    } else {
        serve(nets)
    }
}

/// What the command line asks for: `--describe`, and `--net <name>`, each
/// at most once and in either order.
#[derive(Clone, Debug, Default, PartialEq)]
struct Command {
    describe: bool,
    net: Option<OsString>,
}

/// The command the arguments spell, or `None` for anything else: an
/// unknown argument, one given twice, or `--net` with no name after it.
fn parse_args(args: &[OsString]) -> Option<Command> {
    let mut command = Command::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--describe" && !command.describe {
            command.describe = true;
        } else if arg == "--net" && command.net.is_none() {
            command.net = Some(args.next()?.clone());
        } else {
            return None;
        }
    }
    Some(command)
}

/// The entry registered as `name`, the first if several are, as `load`
/// would find it; or the message that refuses the name, listing the nets
/// there are.
fn one_net(nets: Vec<NetEntry>, name: &OsString) -> Result<NetEntry, String> {
    let carried: Vec<&str> = nets.iter().map(NetEntry::name).collect();
    let carried = if carried.is_empty() {
        "no nets".to_owned()
    } else {
        carried.join(", ")
    };
    let message = format!(
        "petritype-runner: unknown net {:?} for --net; this binary carries {carried}",
        name.to_string_lossy(),
    );
    nets.into_iter()
        .find(|entry| name == entry.name())
        .ok_or(message)
}

/// What the owner loop receives.
enum Message {
    Input(Input),
    /// A line the reader answered itself: a malformed request.
    Line(String),
    /// Stdin is at end of file.
    Eof,
}

/// What the writer receives.
enum WriterMessage {
    /// Lines to write together, in order, then flush, and, for the owner's
    /// lines, the owner's state once they are written: what the heartbeats
    /// after them say.
    Batch(Vec<String>, Option<Stamp>),
    /// A heartbeat to write, if a net is loaded, with the version of the
    /// lines written before it.
    Heartbeat(Beat),
    /// Flush and stop.
    Done,
}

/// The owner's state after a batch of its lines, as the writer keeps it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Stamp {
    loaded: bool,
    marking_version: u64,
}

impl Stamp {
    fn of(owner: &Owner) -> Stamp {
        Stamp {
            loaded: owner.is_loaded(),
            marking_version: owner.marking_version(),
        }
    }
}

/// What the heartbeat thread read when it woke: the body that runs, if one
/// does, and whether the net runs.
#[derive(Debug, Clone, Default)]
struct Beat {
    executing: Option<(String, SystemTime, Option<InnerAddress>)>,
    running: bool,
}

impl Beat {
    /// The `heartbeat` line, after lines that left the owner as `stamp`
    /// says; `None` while no net is loaded.
    fn line(self, stamp: Stamp) -> Option<String> {
        if !stamp.loaded {
            return None;
        }
        let (step_started_at, executing_transition, address) = match self.executing {
            Some((name, at, address)) => (Some(iso8601(at)), Some(name), address),
            None => (None, None, None),
        };
        let (path, nested_id) = address.map_or((None, None), |address| {
            (Some(address.path), Some(address.nested_id))
        });
        let line = Notification::Heartbeat {
            step_started_at,
            executing_transition,
            path,
            nested_id,
            marking_version: stamp.marking_version,
            running: self.running,
        }
        .to_line()
        .expect(HEARTBEAT_WRITES);
        Some(line)
    }
}

fn heartbeat_period() -> Duration {
    std::env::var(HEARTBEAT_ENV)
        .ok()
        .and_then(|ms| ms.trim().parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .map_or(DEFAULT_HEARTBEAT, Duration::from_millis)
}

/// The longest request line read, in bytes without its newline:
/// `PETRITYPE_RUNNER_MAX_LINE_BYTES` when it holds a positive integer,
/// else 4 MiB (since 2026-11-28).
fn max_line_bytes() -> usize {
    lines::max_line_bytes(std::env::var(MAX_LINE_ENV).ok().as_deref())
}

/// Starts a thread named `name`, or says on stderr why it could not.
fn spawn<T: Send + 'static>(
    name: &str,
    run: impl FnOnce() -> T + Send + 'static,
) -> Option<thread::JoinHandle<T>> {
    match thread::Builder::new().name(name.to_owned()).spawn(run) {
        Ok(handle) => Some(handle),
        Err(error) => {
            eprintln!("petritype-runner: cannot start the {name} thread: {error}");
            None
        }
    }
}

fn serve(nets: Vec<NetEntry>) -> ExitCode {
    let stdout = match guard_stdout() {
        Ok(stdout) => stdout,
        Err(error) => {
            eprintln!("petritype-runner: cannot take stdout for the protocol: {error}");
            return ExitCode::FAILURE;
        }
    };
    let status = StatusCell::default();
    let mut owner = Owner::new(nets, Arc::clone(&status), heartbeat_period());
    let max_line = max_line_bytes();
    let gauges = owner.gauges();
    let period = owner.heartbeat();

    let (to_owner, inbox) = mpsc::sync_channel::<Message>(OWNER_QUEUE);
    let (to_writer, writer_inbox) = mpsc::sync_channel::<WriterMessage>(WRITER_QUEUE);
    // Never sent on: the writer drops its end when it stops, which is what
    // the loop waits for at exit.
    let (writer_alive, writer_stopped) = mpsc::channel::<()>();
    let writer = spawn("petritype-runner writer", move || {
        let _alive = writer_alive;
        let mut stdout = stdout;
        write_lines(&mut stdout, &writer_inbox);
    });
    let Some(writer) = writer else {
        return ExitCode::FAILURE;
    };
    let reader = spawn("petritype-runner reader", {
        let to_owner = to_owner.clone();
        move || read_lines(&to_owner, max_line)
    });
    let heartbeat = spawn("petritype-runner heartbeat", {
        let to_owner = to_owner.clone();
        let to_writer = to_writer.clone();
        move || beat(period, &status, &gauges, &to_owner, &to_writer)
    });
    if reader.is_none() || heartbeat.is_none() {
        finish(writer, &to_writer, &writer_stopped);
        return ExitCode::FAILURE;
    }

    // What the loop gave itself to handle next, before the inbox: a body
    // that ran on the loop's own thread because no thread could be started.
    let mut own: VecDeque<Message> = VecDeque::new();
    let mut tick_waiting = false;
    // The stamp the writer has last been given.
    let mut written = Stamp::default();
    while let Some(message) = own
        .pop_front()
        .or_else(|| next_message(&inbox, &mut tick_waiting))
    {
        let input = match message {
            Message::Input(input) => input,
            Message::Line(line) => {
                if to_writer
                    .send(WriterMessage::Batch(vec![line], None))
                    .is_err()
                {
                    break;
                }
                continue;
            }
            Message::Eof => break,
        };
        let mut batch = Vec::new();
        let mut launches = Vec::new();
        let mut exit = false;
        for outgoing in owner.handle(input, SystemTime::now()) {
            match outgoing {
                Outgoing::Line(line) => batch.push(line),
                Outgoing::Launch(firing) => launches.push(firing),
                Outgoing::Exit => exit = true,
            }
        }
        // The version moves with the lines that tell of it, so a batch of
        // none rarely changes the stamp; when it does, it goes all the same.
        let stamp = Stamp::of(&owner);
        if (!batch.is_empty() || stamp != written)
            && to_writer
                .send(WriterMessage::Batch(batch, Some(stamp)))
                .is_err()
        {
            break;
        }
        written = stamp;
        for firing in launches {
            if let Some(firing) = launch(firing, &to_owner) {
                // No thread for it: it runs here, and lands before anything
                // else is read.
                own.push_back(Message::Input(Input::Completed(firing.run())));
            }
        }
        if exit {
            break;
        }
        if owner.wants_tick() {
            tick_waiting = true;
        }
    }

    finish(writer, &to_writer, &writer_stopped);
    ExitCode::SUCCESS
}

/// Runs `firing` on a thread of its own, whose result comes back to the
/// loop as [`Input::Completed`]; or hands the firing back when no thread
/// can be started, for the loop to run itself.
fn launch(firing: Firing, to_owner: &SyncSender<Message>) -> Option<Firing> {
    // The firing goes to the thread through the cell, so that it is still
    // here when the thread cannot be started.
    let cell = Arc::new(Mutex::new(Some(firing)));
    let started = thread::Builder::new()
        .name("petritype-runner body".into())
        .spawn({
            let cell = Arc::clone(&cell);
            let to_owner = to_owner.clone();
            move || {
                let Some(firing) = lock(&cell).take() else {
                    return;
                };
                let completed = firing.run();
                // The loop may have exited; the body is abandoned with the
                // process, as `shutdown` asked.
                let _ = to_owner.send(Message::Input(Input::Completed(completed)));
            }
        });
    match started {
        Ok(_) => None,
        Err(error) => {
            eprintln!(
                "petritype-runner: cannot start a thread for a body ({error}); it runs on the owner's thread"
            );
            lock(&cell).take()
        }
    }
}

/// The heartbeat thread: every `period`, a heartbeat to the writer, which
/// writes it with the version of the lines before it, and a tick to the
/// owner loop, until either has gone. Neither is waited for: while the
/// writer is a full queue behind, the host is reading nothing and the
/// heartbeat is let go, and while the loop is a full queue behind, it has
/// work enough without a tick, each request and each landing running the
/// net on.
fn beat(
    period: Duration,
    status: &StatusCell,
    gauges: &Gauges,
    to_owner: &SyncSender<Message>,
    to_writer: &SyncSender<WriterMessage>,
) {
    loop {
        thread::sleep(period);
        let beat = Beat {
            executing: lock(status).clone(),
            running: gauges.running.load(Ordering::Relaxed),
        };
        if let Err(TrySendError::Disconnected(_)) =
            to_writer.try_send(WriterMessage::Heartbeat(beat))
        {
            return;
        }
        if let Err(TrySendError::Disconnected(_)) = to_owner.try_send(Message::Input(Input::Tick)) {
            return;
        }
    }
}

/// Tells the writer to stop once it has written what waits, and waits for
/// it [`EXIT_FLUSH`] at most: a host that reads none of the process's
/// output cannot keep it from ending. A writer still writing then is left
/// to end with the process.
fn finish(
    writer: thread::JoinHandle<()>,
    to_writer: &SyncSender<WriterMessage>,
    stopped: &Receiver<()>,
) {
    let deadline = Instant::now() + EXIT_FLUSH;
    let gone = |left: Duration| {
        matches!(
            stopped.recv_timeout(left),
            Err(RecvTimeoutError::Disconnected)
        )
    };
    let mut done = WriterMessage::Done;
    let ended = loop {
        match to_writer.try_send(done) {
            Ok(()) => break gone(deadline.saturating_duration_since(Instant::now())),
            Err(TrySendError::Disconnected(_)) => break true,
            Err(TrySendError::Full(back)) => {
                done = back;
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break false;
                }
                // A full queue: the writer is behind. Wait a moment for room,
                // or for the writer to stop.
                if gone(left.min(Duration::from_millis(10))) {
                    break true;
                }
            }
        }
    };
    if ended {
        let _ = writer.join();
    } else {
        eprintln!(
            "petritype-runner: the protocol stream was not read to the end within {EXIT_FLUSH:?} of exit; the lines still waiting are lost"
        );
    }
}

/// The next message for the owner loop, with every tick held back until
/// nothing else waits: `None` once every sender is gone.
///
/// A tick that comes in is not handed out at once. It sets `tick_waiting`,
/// and the loop gets [`Input::Tick`] only when the channel is empty, so the
/// ticks that came in while the owner was busy count as one, and a request
/// that arrived during an inline body is handled when that body lands,
/// before any tick fires the net again. The loop sets `tick_waiting` itself
/// when the owner [wants a tick](Owner::wants_tick).
fn next_message(inbox: &Receiver<Message>, tick_waiting: &mut bool) -> Option<Message> {
    loop {
        let message = match inbox.try_recv() {
            Ok(message) => message,
            Err(TryRecvError::Empty) if *tick_waiting => {
                *tick_waiting = false;
                return Some(Message::Input(Input::Tick));
            }
            Err(TryRecvError::Empty) => inbox.recv().ok()?,
            Err(TryRecvError::Disconnected) => return None,
        };
        if let Message::Input(Input::Tick) = message {
            *tick_waiting = true;
            continue;
        }
        return Some(message);
    }
}

/// Stdin, line by line, each at most `max` bytes, into the owner's
/// channel. A line that is not a request, too long or not UTF-8 among them,
/// is answered here, with `invalid_params` (PROTOCOL.md §2).
fn read_lines(to_owner: &SyncSender<Message>, max: usize) {
    for line in Lines::new(io::stdin().lock(), max) {
        let refused = match line {
            Line::Text(text) => match Request::parse(&text) {
                Ok(request) => {
                    if to_owner
                        .send(Message::Input(Input::Request(request)))
                        .is_err()
                    {
                        return;
                    }
                    continue;
                }
                Err(malformed) => refusal(malformed.id, malformed.message),
            },
            Line::Refused(message) => refusal("", message),
        };
        if let Some(answer) = refused
            && to_owner.send(Message::Line(answer)).is_err()
        {
            return;
        }
    }
    let _ = to_owner.send(Message::Eof);
}

/// Writes whole batches, flushing after each, so that nothing can come
/// between a request's notifications and its response, and the heartbeats
/// between them, each with the marking version of the owner's last batch
/// (since 2026-11-28; until then the heartbeat thread read the version
/// itself, and a heartbeat could carry a version a line written after it
/// had not reached yet, or one older than a line written before it).
/// Stops on [`WriterMessage::Done`], when every sender has gone, or when a
/// write fails.
fn write_lines(out: &mut dyn Write, inbox: &Receiver<WriterMessage>) {
    let mut stamp = Stamp::default();
    for message in inbox {
        let lines = match message {
            WriterMessage::Batch(lines, after) => {
                if let Some(after) = after {
                    stamp = after;
                }
                lines
            }
            WriterMessage::Heartbeat(beat) => match beat.line(stamp) {
                Some(line) => vec![line],
                None => continue,
            },
            WriterMessage::Done => break,
        };
        for line in lines {
            if writeln!(out, "{line}").is_err() {
                return;
            }
        }
        if out.flush().is_err() {
            return;
        }
    }
    let _ = out.flush();
}

/// Keeps a private copy of the real stdout for the protocol and points file
/// descriptor 1 at stderr, so that a body's `println!` cannot corrupt the
/// protocol stream.
#[cfg(unix)]
fn guard_stdout() -> io::Result<Box<dyn Write + Send>> {
    io::stdout().flush()?;
    // Close-on-exec, and above the standard three: a child process a body
    // spawns must not inherit the protocol stream.
    let protocol = rustix::io::fcntl_dupfd_cloexec(io::stdout(), 3)?;
    rustix::stdio::dup2_stdout(io::stderr())?;
    Ok(Box::new(io::BufWriter::new(std::fs::File::from(protocol))))
}

/// No guard where there are no file descriptors to move: stdout is shared.
#[cfg(not(unix))]
fn guard_stdout() -> io::Result<Box<dyn Write + Send>> {
    Ok(Box::new(io::stdout()))
}

fn lock<T>(cell: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    cell.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }

    fn net(name: &str) -> Option<OsString> {
        Some(OsString::from(name))
    }

    #[test]
    fn the_arguments() {
        assert_eq!(parse_args(&args(&[])), Some(Command::default()));
        let describe = Command {
            describe: true,
            net: None,
        };
        assert_eq!(parse_args(&args(&["--describe"])), Some(describe));
        let one = Command {
            describe: false,
            net: net("odd-names"),
        };
        assert_eq!(parse_args(&args(&["--net", "odd-names"])), Some(one));
        let both = Command {
            describe: true,
            net: net("workshop"),
        };
        for words in [
            ["--net", "workshop", "--describe"],
            ["--describe", "--net", "workshop"],
        ] {
            assert_eq!(parse_args(&args(&words)), Some(both.clone()));
        }
        // A name may be anything, a flag's spelling included.
        let odd = Command {
            describe: false,
            net: net("--describe"),
        };
        assert_eq!(parse_args(&args(&["--net", "--describe"])), Some(odd));
        for words in [
            &["--net"][..],
            &["--describe", "--describe"],
            &["--net", "a", "--net", "b"],
            &["--frobnicate"],
            &["workshop"],
        ] {
            assert_eq!(parse_args(&args(words)), None, "{words:?}");
        }
    }

    #[test]
    fn a_heartbeat_carries_the_version_of_the_lines_written_before_it() {
        let (to_writer, inbox) = mpsc::sync_channel(16);
        let beat = || {
            WriterMessage::Heartbeat(Beat {
                executing: None,
                running: true,
            })
        };
        let after = |version| {
            Some(Stamp {
                loaded: true,
                marking_version: version,
            })
        };
        let messages = [
            // Nothing loaded yet: no heartbeat.
            beat(),
            WriterMessage::Batch(vec![r#"{"method":"fired"}"#.into()], after(3)),
            beat(),
            // A refusal the reader made moves nothing.
            WriterMessage::Batch(vec![r#"{"id":""}"#.into()], None),
            beat(),
            WriterMessage::Batch(vec![r#"{"method":"added"}"#.into()], after(5)),
            beat(),
            WriterMessage::Done,
            beat(),
        ];
        for message in messages {
            to_writer.send(message).unwrap();
        }
        let mut out = Vec::new();
        write_lines(&mut out, &inbox);
        let lines: Vec<serde_json::Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let told: Vec<String> = lines
            .iter()
            .map(|line| match line["method"].as_str() {
                Some("heartbeat") => format!("heartbeat {}", line["params"]["marking_version"]),
                Some(method) => method.to_owned(),
                None => "refusal".to_owned(),
            })
            .collect();
        assert_eq!(
            told,
            [
                "fired",
                "heartbeat 3",
                "refusal",
                "heartbeat 3",
                "added",
                "heartbeat 5"
            ]
        );
    }

    #[test]
    fn one_net_of_several() {
        let build = |_: &crate::Params| -> Result<petritype::Net, crate::LoadError> {
            Ok(petritype::NetBuilder::new().build()?)
        };
        let nets = || {
            vec![
                NetEntry::new("first", build),
                NetEntry::new("second", build),
            ]
        };
        let found = one_net(nets(), &OsString::from("second")).unwrap();
        assert_eq!(found.name(), "second");
        let refused = one_net(nets(), &OsString::from("third")).err().unwrap();
        assert_eq!(
            refused,
            "petritype-runner: unknown net \"third\" for --net; this binary carries first, second"
        );
        let none = one_net(Vec::new(), &OsString::from("x")).err().unwrap();
        assert!(none.ends_with("this binary carries no nets"), "{none}");
    }
}
