//! A runner binary whose bodies take their time: what the heartbeat says
//! while one runs.
//!
//! ```text
//! PETRITYPE_RUNNER_HEARTBEAT_MS=100 cargo run -p petritype-runner --example slow_bodies
//! {"id":"1","method":"load","params":{"net":"slow-office","params":{"ms":500}}}
//! {"id":"2","method":"step","params":{}}
//! {"id":"3","method":"step","params":{}}
//! {"id":"4","method":"shutdown","params":{}}
//! ```
//!
//! It carries one net, `slow-office`: a clerk at the `Desk`, a net the
//! desk's one token carries, who files `jobs` letters one at a time, and
//! out in the office a `Tally` with a mutate edge, which counts `tallies`
//! receipts. Every body sleeps `ms` milliseconds. Both kinds of body run
//! inline, on the owner's thread, so while one runs the process answers no
//! request and its heartbeat names the body: `File` with the clerk's `path`
//! and `nested_id`, `Tally` with neither. The desk is
//! inner first, so the clerk files every letter before the tally counts.
//! The runner's tests over real pipes start this binary.

use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use petritype::{MutateEdge, Net, NetBuilder, TakeEdge};
use petritype_runner::{LoadError, NetEntry, ParamSpec};
use serde::Deserialize;

/// `load {params}` for `slow-office`.
#[derive(Debug, Deserialize)]
struct Office {
    /// How long every body takes, in milliseconds.
    ms: u64,
    /// How many letters the clerk files.
    jobs: u32,
    /// How many receipts the tally counts.
    tallies: u32,
}

/// The clerk's net, `Letters -File-> Filed`, with `jobs` letters to file.
fn clerk(jobs: u32, pause: Duration) -> Result<Net, LoadError> {
    let mut b = NetBuilder::new();
    let letters = b.place::<u32>("Letters");
    let filed = b.place::<u32>("Filed");
    b.seed(letters, 0..jobs);
    b.transition("File", (TakeEdge(letters),), (filed,), move |n: u32| {
        thread::sleep(pause);
        (n,)
    });
    Ok(b.build()?)
}

/// The office: the clerk on the `Desk`, and `Receipts -Tally->`, which
/// counts each receipt in `Count`.
fn slow_office(office: Office) -> Result<Net, LoadError> {
    let pause = Duration::from_millis(office.ms);
    let mut b = NetBuilder::new();
    let desk = b.place_nested::<Net>("Desk");
    let receipts = b.place::<u32>("Receipts");
    let count = b.place::<u64>("Count");
    b.seed(desk, [clerk(office.jobs, pause)?]);
    b.seed(receipts, 0..office.tallies);
    b.seed(count, [0]);
    b.transition(
        "Tally",
        (TakeEdge(receipts), MutateEdge(count)),
        (),
        move |_: u32, count: &mut u64| {
            thread::sleep(pause);
            *count += 1;
        },
    );
    Ok(b.build()?)
}

fn main() -> ExitCode {
    petritype_runner::main([NetEntry::typed("slow-office", slow_office)
        .description("A clerk files letters and a tally counts receipts, each taking a while")
        .param(ParamSpec::new("ms", "u64").default(300))
        .param(ParamSpec::new("jobs", "u32").default(2))
        .param(ParamSpec::new("tallies", "u32").default(1))])
}
