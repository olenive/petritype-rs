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
        println!("{}", protocol::to_line(owner.describe()));
        ExitCode::SUCCESS
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
                let (step_started_at, executing_transition, address) = match lock(&status).clone() {
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
