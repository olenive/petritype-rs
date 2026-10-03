//! The outer layer, and the only one that does I/O: the stdout guard, the
//! threads, and the loop around [`Owner`] (DESIGN §19.3.2 item 3).

use std::ffi::OsString;
use std::io::{self, BufRead, Write};
use std::process::ExitCode;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, SystemTime};

use crate::owner::{Input, Outgoing, Owner, StatusCell};
use crate::protocol::{self, ErrorCode, Notification, Request, Response};
use crate::registry::NetEntry;
use crate::time::iso8601;

/// The heartbeat and tick period when the environment does not set one.
const DEFAULT_HEARTBEAT: Duration = Duration::from_secs(30);

/// The environment variable that overrides the heartbeat period, in
/// milliseconds. Meant for tests, which cannot wait 30 seconds.
pub(crate) const HEARTBEAT_ENV: &str = "PETRITYPE_RUNNER_HEARTBEAT_MS";

const USAGE: &str = "usage: <binary>              serve the net protocol on stdin and stdout\n       \
                     <binary> --describe   print the nets this binary carries as one JSON line";

/// Runs a net process: the whole of a runner binary's `main`.
///
/// - With no arguments, serves the net protocol on stdin and stdout until
///   `shutdown` or end of file, and exits 0.
/// - With `--describe`, prints the `describe` answer as one JSON line and
///   exits 0; this is how a build step learns what the binary carries.
/// - With anything else, prints usage on stderr and exits 2.
///
/// While serving, file descriptor 1 is moved: the protocol keeps a private
/// copy of the real stdout, and fd 1 becomes stderr, so a `println!` in a
/// transition body lands on stderr instead of corrupting the protocol (Unix
/// only; elsewhere stdout is shared and bodies must not print to it).
///
/// The heartbeat (and the ticker that drives a running net) fires every 30
/// seconds, or every `PETRITYPE_RUNNER_HEARTBEAT_MS` milliseconds when that
/// variable is set.
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
    let nets: Vec<NetEntry> = nets.into_iter().collect();
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match args.as_slice() {
        [] => serve(nets),
        [flag] if flag == "--describe" => {
            let owner = Owner::new(nets, StatusCell::default(), DEFAULT_HEARTBEAT);
            println!("{}", protocol::to_line(owner.describe()));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
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
    /// Lines to write together, in order, then flush.
    Batch(Vec<String>),
    /// Flush and stop.
    Done,
}

fn heartbeat_period() -> Duration {
    std::env::var(HEARTBEAT_ENV)
        .ok()
        .and_then(|ms| ms.trim().parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .map_or(DEFAULT_HEARTBEAT, Duration::from_millis)
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
    let gauges = owner.gauges();
    let period = owner.heartbeat();

    let (to_owner, inbox) = mpsc::channel::<Message>();
    let (to_writer, writer_inbox) = mpsc::channel::<WriterMessage>();
    let writer = thread::spawn(move || write_lines(stdout, writer_inbox));

    thread::spawn({
        let to_owner = to_owner.clone();
        move || read_lines(&to_owner)
    });
    thread::spawn({
        let to_owner = to_owner.clone();
        let to_writer = to_writer.clone();
        move || loop {
            thread::sleep(period);
            if gauges.loaded.load(Ordering::Relaxed) {
                let running = lock(&status).clone();
                let line = Notification::Heartbeat {
                    step_started_at: running.as_ref().map(|(_, at)| iso8601(*at)),
                    executing_transition: running.map(|(name, _)| name),
                    marking_version: gauges.marking_version.load(Ordering::Relaxed),
                    running: gauges.running.load(Ordering::Relaxed),
                }
                .to_line();
                if to_writer.send(WriterMessage::Batch(vec![line])).is_err() {
                    return;
                }
            }
            if to_owner.send(Message::Input(Input::Tick)).is_err() {
                return;
            }
        }
    });

    while let Ok(message) = inbox.recv() {
        let input = match message {
            Message::Input(input) => input,
            Message::Line(line) => {
                if to_writer.send(WriterMessage::Batch(vec![line])).is_err() {
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
        if !batch.is_empty() && to_writer.send(WriterMessage::Batch(batch)).is_err() {
            break;
        }
        for firing in launches {
            let to_owner = to_owner.clone();
            thread::spawn(move || {
                let completed = firing.run();
                // The loop may have exited; the body is abandoned with the
                // process, as `shutdown` asked.
                let _ = to_owner.send(Message::Input(Input::Completed(completed)));
            });
        }
        if exit {
            break;
        }
        if owner.wants_tick() {
            let _ = to_owner.send(Message::Input(Input::Tick));
        }
    }

    let _ = to_writer.send(WriterMessage::Done);
    let _ = writer.join();
    ExitCode::SUCCESS
}

/// Stdin, line by line, into the owner's channel.
fn read_lines(to_owner: &Sender<Message>) {
    let mut stdin = io::stdin().lock();
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match stdin.read_until(b'\n', &mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let message = match std::str::from_utf8(&buffer) {
            Ok(line) if line.trim().is_empty() => continue,
            Ok(line) => match Request::parse(line.trim_end_matches(['\n', '\r'])) {
                Ok(request) => Message::Input(Input::Request(request)),
                Err(malformed) => Message::Line(
                    Response::error(malformed.id, ErrorCode::InvalidParams, malformed.message)
                        .to_line(),
                ),
            },
            Err(_) => Message::Line(
                Response::error("", ErrorCode::InvalidParams, "a request line must be UTF-8")
                    .to_line(),
            ),
        };
        if to_owner.send(message).is_err() {
            return;
        }
    }
    let _ = to_owner.send(Message::Eof);
}

/// Writes whole batches, flushing after each, so that nothing can come
/// between a request's notifications and its response.
fn write_lines(mut out: Box<dyn Write + Send>, inbox: Receiver<WriterMessage>) {
    for message in inbox {
        match message {
            WriterMessage::Batch(lines) => {
                for line in lines {
                    if writeln!(out, "{line}").is_err() {
                        return;
                    }
                }
                if out.flush().is_err() {
                    return;
                }
            }
            WriterMessage::Done => break,
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
