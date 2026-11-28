//! One definition, two ways to run it: sequential and concurrent.
//!
//! A port of the Python `examples/execution_modes/net.py`,
//! `01_sequential.py` and `02_concurrent.py`, which are the same notebook
//! twice over with one word changed.
//!
//! The net is two **independent** three-stage chains,
//! `Start -> s1 -> 1 -> s2 -> 2 -> s3 -> Done`, where every stage sleeps for
//! [`STEP`]. Nothing connects the branches, so the mode is visible in the
//! wall clock:
//!
//! - [`Mode::Sequential`]: the selector picks one fireable transition, it
//!   runs to completion, and the loop asks again (branch A all the way down,
//!   then branch B). Six stages, so about the **sum**, `6 × STEP`.
//! - [`Mode::Concurrent`]: everything fireable is launched at once, each
//!   detachable body on a worker thread. A run spawns a worker only when no
//!   other is free, up to the machine's parallelism, so two here, one per
//!   branch. The branches advance in lockstep, so about the **max**,
//!   `3 × STEP`.
//!
//! The final marking is the same either way, which is the other half of the
//! point: the mode is a scheduling choice, not a semantic one.
//!
//! The same two branches can also ride inside a net, each the inner net of
//! a token in one place, `Yard`, as a game's characters or a room's
//! machines do (a part the Python notebooks do not have). The concurrent
//! driver launches the bodies of the nets inside a net beside one another
//! too, so the run inside takes about the max as well (since 0.4.0; until
//! then the inner nets fired one transition at a time, and the run inside
//! took the sum in both modes).

use std::time::{Duration, Instant};

use petritype::drivers::{self, Limits, Mode, OnFailure};
use petritype::{BuildError, Net, NetBuilder, PlaceId, PlaceRef, TakeEdge};

/// How long one stage takes. Small enough to keep the test quick, large
/// enough that thread overhead is noise beside it.
const STEP: Duration = Duration::from_millis(50);

/// The net, and the `Done` place of each branch.
struct Branches {
    net: Net,
    done: Vec<PlaceRef<String>>,
}

/// Two branches of three slow stages each, each seeded with one token.
fn build() -> Result<Branches, BuildError> {
    let mut b = NetBuilder::new();
    let mut done = Vec::new();
    for letter in ['A', 'B'] {
        let start = b.place::<String>(format!("Start{letter}"));
        let first = b.place::<String>(format!("{letter}1"));
        let second = b.place::<String>(format!("{letter}2"));
        let end = b.place::<String>(format!("Done{letter}"));
        b.seed(start, [String::from("go")]);

        for (name, from, to) in [
            (format!("Slow{letter}1"), start, first),
            (format!("Slow{letter}2"), first, second),
            (format!("Slow{letter}3"), second, end),
        ] {
            b.transition(name, (TakeEdge(from),), (to,), |token: String| {
                std::thread::sleep(STEP);
                (token,)
            });
        }
        done.push(end);
    }
    Ok(Branches {
        net: b.build()?,
        done,
    })
}

/// One branch on its own, as an inner net: `Start -> s1 -> 1 -> s2 -> 2 ->
/// s3 -> Done`, with one token at its start.
fn branch() -> Result<Net, BuildError> {
    let mut b = NetBuilder::new();
    let start = b.place::<String>("Start");
    let first = b.place::<String>("1");
    let second = b.place::<String>("2");
    let end = b.place::<String>("Done");
    b.seed(start, [String::from("go")]);
    for (name, from, to) in [
        ("Slow1", start, first),
        ("Slow2", first, second),
        ("Slow3", second, end),
    ] {
        b.transition(name, (TakeEdge(from),), (to,), |token: String| {
            std::thread::sleep(STEP);
            (token,)
        });
    }
    b.build()
}

/// A net whose place `Yard` holds the two branches, each as a token's net.
fn build_inside() -> Result<(Net, PlaceId), BuildError> {
    let mut b = NetBuilder::new();
    let yard = b.place_nested::<Net>("Yard");
    b.seed(yard, [branch()?, branch()?]);
    Ok((b.build()?, yard.id()))
}

/// [`run_in`] for the branches inside the net: how long it took, and what
/// each branch's `Done` holds.
fn run_inside(mode: Mode) -> (Duration, Vec<Vec<String>>) {
    let (mut net, yard) = build_inside().expect("the net builds");
    let started = Instant::now();
    let summary =
        drivers::run(&mut net, mode, Limits::default(), OnFailure::Stop).expect("nothing fails");
    let elapsed = started.elapsed();
    assert!(summary.quiesced);
    assert_eq!(summary.fired_count, 6);

    let marking = net
        .tokens(yard)
        .map(|(token, _)| {
            let inner = net.nested(yard, token).expect("a nested place")[0].1;
            let done = inner.place_named("Done").expect("a branch has a Done");
            inner
                .tokens(done)
                .filter_map(|(_, value)| value.downcast_ref::<String>())
                .cloned()
                .collect()
        })
        .collect();
    (elapsed, marking)
}

/// Runs a fresh net in one mode, and reports how long it took and where the
/// tokens ended up.
fn run_in(mode: Mode) -> (Duration, Vec<Vec<String>>) {
    let mut branches = build().expect("the net builds");
    let started = Instant::now();
    let summary = drivers::run(&mut branches.net, mode, Limits::default(), OnFailure::Stop)
        .expect("nothing fails");
    let elapsed = started.elapsed();
    assert!(summary.quiesced);
    assert_eq!(summary.fired_count, 6);

    let marking = branches
        .done
        .iter()
        .map(|place| {
            branches
                .net
                .tokens(place.id())
                .filter_map(|(_, value)| value.downcast_ref::<String>())
                .cloned()
                .collect()
        })
        .collect();
    (elapsed, marking)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("{}", petritype_dot::dot(&build()?.net));

    let (sequential, sequential_marking) = run_in(Mode::Sequential);
    let (concurrent, concurrent_marking) = run_in(Mode::Concurrent);

    println!("one stage takes {STEP:?}, and there are six of them");
    println!("sequential: {sequential:?}, the sum of the branches");
    println!("concurrent: {concurrent:?}, the longer of them");
    println!("both end with {sequential_marking:?}");
    assert_eq!(sequential_marking, concurrent_marking);

    let (sequential, sequential_marking) = run_inside(Mode::Sequential);
    let (concurrent, concurrent_marking) = run_inside(Mode::Concurrent);
    println!("with the branches inside the net, as two tokens' nets:");
    println!("sequential: {sequential:?}, the sum of the branches");
    println!("concurrent: {concurrent:?}, the longer of them");
    println!("both end with {sequential_marking:?}");
    assert_eq!(sequential_marking, concurrent_marking);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{STEP, build, run_in, run_inside};
    use petritype::drivers::Mode;

    #[test]
    fn both_modes_end_in_the_same_marking() {
        let (_, sequential) = run_in(Mode::Sequential);
        let (_, concurrent) = run_in(Mode::Concurrent);

        assert_eq!(sequential, concurrent);
        assert_eq!(sequential, [[String::from("go")], [String::from("go")]]);
    }

    #[test]
    fn nothing_is_left_anywhere_but_the_done_places() {
        let (_, marking) = run_in(Mode::Concurrent);
        assert_eq!(marking.len(), 2);

        let branches = build().expect("the net builds");
        let waiting: usize = branches
            .net
            .places()
            .filter(|(_, info)| !info.name.starts_with("Done"))
            .map(|(_, info)| info.count)
            .sum();
        assert_eq!(
            waiting, 2,
            "before the run: one token at the top of each branch"
        );
    }

    #[test]
    fn the_concurrent_run_overlaps_the_branches() {
        let (sequential, _) = run_in(Mode::Sequential);
        let (concurrent, _) = run_in(Mode::Concurrent);

        // Six stages one after another against three pairs at a time: the gap
        // should be about `3 × STEP`. Asserting a third of that leaves room
        // for a loaded machine without letting a regression through.
        assert!(
            concurrent + STEP <= sequential,
            "sequential {sequential:?} was not clear of concurrent {concurrent:?}",
        );
        assert!(
            sequential >= STEP * 6,
            "six stages cannot take less than six steps: {sequential:?}",
        );
    }

    #[test]
    fn the_concurrent_run_overlaps_the_branches_inside_the_net_too() {
        let (sequential, sequential_marking) = run_inside(Mode::Sequential);
        let (concurrent, concurrent_marking) = run_inside(Mode::Concurrent);

        assert_eq!(sequential_marking, concurrent_marking);
        assert_eq!(
            sequential_marking,
            [[String::from("go")], [String::from("go")]]
        );
        // As for the branches out in the open: about `3 × STEP` apart.
        assert!(
            concurrent + STEP <= sequential,
            "sequential {sequential:?} was not clear of concurrent {concurrent:?}",
        );
    }
}
