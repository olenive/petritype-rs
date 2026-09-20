//! Integration tests for the ninth slice: the drivers, the loops that run a
//! net rather than step it.
//!
//! A driver adds no semantics of its own — it is `step`, or
//! `launch`/`run`/`distribute`, in a loop — so what these tests are about is
//! the loop's own decisions: when it stops (quiescence, a pacing limit, a
//! fuse), what it does about a failed firing, and the promise that it never
//! goes home with a firing still in flight. Timing assertions are deliberately
//! loose: the concurrent mode is wall-clock ordered, so nothing here asserts
//! *which* body finished first, only that several of them overlapped.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use petritype::drivers::{self, DriverError, Limits, Mode, OnFailure, RunSummary};
use petritype::{Event, MutateEdge, Net, NetBuilder, PlaceId, PlaceRef, TakeEdge, TransitionId};

/// A body's failure, erased into `petritype::FiringFailed` by the engine.
#[derive(Debug)]
struct Refused;

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("refused")
    }
}

impl std::error::Error for Refused {}

/// Reads a place back as plain values, oldest first.
fn values<T: Clone + Send + Sync + 'static>(net: &Net, place: PlaceId) -> Vec<T> {
    net.tokens(place)
        .map(|(_, value)| {
            value
                .downcast_ref::<T>()
                .expect("the place holds this type")
                .clone()
        })
        .collect()
}

/// The same, sorted, for a concurrent run where arrival order is wall-clock
/// order and so is not a property of the net.
fn sorted(net: &Net, place: PlaceId) -> Vec<u32> {
    let mut found = values::<u32>(net, place);
    found.sort_unstable();
    found
}

/// `Inbox -> Double -> Middle -> AddOne -> Out`: two firings per token, and
/// the second transition only becomes fireable once the first has fired.
fn chain(
    seed: impl IntoIterator<Item = u32>,
) -> (Net, PlaceRef<u32>, PlaceRef<u32>, PlaceRef<u32>) {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let middle = b.place::<u32>("Middle");
    let out = b.place::<u32>("Out");
    b.seed(inbox, seed);
    b.transition("Double", (TakeEdge(inbox),), (middle,), |n: u32| (n * 2,));
    b.transition("AddOne", (TakeEdge(middle),), (out,), |n: u32| (n + 1,));
    let net = b.build().expect("a two-transition chain");
    (net, inbox, middle, out)
}

/// One transition, `n` tokens waiting: a net that needs exactly `n` firings.
fn straight(
    seed: impl IntoIterator<Item = u32>,
) -> (Net, PlaceRef<u32>, PlaceRef<u32>, TransitionId) {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, seed);
    let double = b
        .transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,))
        .id();
    let net = b.build().expect("a one-transition net");
    (net, inbox, out, double)
}

/// `width` independent transitions, each with its own input and output place,
/// each body sleeping `body` before it answers: the shape that shows whether
/// a driver runs bodies side by side or one after another.
fn independent(width: u32, body: Duration) -> (Net, Vec<PlaceRef<u32>>, Vec<PlaceRef<u32>>) {
    let mut b = NetBuilder::new();
    let mut ins = Vec::new();
    let mut outs = Vec::new();
    for i in 0..width {
        let inbox = b.place::<u32>(format!("In{i}"));
        let out = b.place::<u32>(format!("Out{i}"));
        b.seed(inbox, [i]);
        b.transition(
            format!("Work{i}"),
            (TakeEdge(inbox),),
            (out,),
            move |n: u32| {
                thread::sleep(body);
                (n + 100,)
            },
        );
        ins.push(inbox);
        outs.push(out);
    }
    let net = b.build().expect("independent transitions");
    (net, ins, outs)
}

fn failed_events(net: &mut Net) -> Vec<Event> {
    net.drain_events()
        .filter(|event| matches!(event, Event::Failed { .. }))
        .collect()
}

fn assert_send_sync<T: Send + Sync + 'static>() {}
fn assert_copy<T: Copy>() {}

// ----- sequential: quiescence and pacing -----------------------------------

#[test]
fn sequential_run_reaches_quiescence() {
    let (mut net, inbox, middle, out) = chain([1, 2, 3]);

    let summary = drivers::run(
        &mut net,
        Mode::Sequential,
        Limits::default(),
        OnFailure::Stop,
    )
    .expect("nothing fails");

    assert!(summary.quiesced, "the net ran out of work");
    assert_eq!(summary.fired_count, 6, "two firings per token");
    assert_eq!(summary.failed_count, 0);
    assert_eq!(values::<u32>(&net, out.id()), [3, 5, 7]);
    assert_eq!(net.count(inbox.id()), 0);
    assert_eq!(net.count(middle.id()), 0);
    assert!(net.fireable().is_empty());
    assert!(net.in_flight().is_empty());
    assert_eq!(net.fired_count(), 6, "the summary is a delta of this");
}

#[test]
fn sequential_stop_after_n_firings_pauses_and_resumes() {
    let (mut net, inbox, _middle, out) = chain([1, 2, 3]);
    let paced = Limits {
        stop_after_n_firings: Some(2),
        ..Limits::default()
    };

    let first = drivers::run(&mut net, Mode::Sequential, paced, OnFailure::Stop).expect("paced");

    assert!(!first.quiesced, "a limit ended the call, not the net");
    assert_eq!(first.fired_count, 2);
    assert_eq!(first.failed_count, 0);
    assert_eq!(net.count(inbox.id()), 1, "one token never left the inbox");
    assert!(!net.fireable().is_empty(), "work remains");

    let rest = drivers::run(
        &mut net,
        Mode::Sequential,
        Limits::default(),
        OnFailure::Stop,
    )
    .expect("the remainder");

    assert!(rest.quiesced);
    assert_eq!(rest.fired_count, 4, "counts are per call, not per net");
    assert_eq!(values::<u32>(&net, out.id()), [3, 5, 7]);
    assert_eq!(net.fired_count(), 6);
}

// ----- the fuse ------------------------------------------------------------

#[test]
fn fuse_trips_before_consuming_anything() {
    let (mut net, inbox, out, double) = straight([1, 2, 3]);
    let limits = Limits {
        error_after_n_firings: Some(0),
        ..Limits::default()
    };

    let error = drivers::run(&mut net, Mode::Sequential, limits, OnFailure::Stop)
        .expect_err("a spent fuse and a fireable transition");

    match error {
        DriverError::TooManyFirings {
            limit,
            transition,
            summary,
        } => {
            assert_eq!(limit, 0);
            assert_eq!(transition, double);
            assert_eq!(summary.fired_count, 0);
            assert_eq!(summary.failed_count, 0);
            assert!(!summary.quiesced);
        }
        other => panic!("expected TooManyFirings, got {other:?}"),
    }

    assert_eq!(
        values::<u32>(&net, inbox.id()),
        [1, 2, 3],
        "nothing was consumed for the launch that did not happen"
    );
    assert_eq!(net.count(out.id()), 0);
    assert_eq!(net.fired_count(), 0);
    assert!(net.in_flight().is_empty());
    assert!(failed_events(&mut net).is_empty(), "no firing failed");
}

#[test]
fn fuse_at_exactly_the_firings_needed_does_not_trip() {
    let (mut net, _inbox, out, _double) = straight([1, 2, 3]);
    let limits = Limits {
        error_after_n_firings: Some(3),
        ..Limits::default()
    };

    let summary = drivers::run(&mut net, Mode::Sequential, limits, OnFailure::Stop)
        .expect("nothing attempted to fire past the limit");

    assert!(summary.quiesced);
    assert_eq!(summary.fired_count, 3);
    assert_eq!(values::<u32>(&net, out.id()), [2, 4, 6]);
}

#[test]
fn fuse_trips_mid_run() {
    let (mut net, inbox, out, _double) = straight([1, 2, 3, 4, 5]);
    let limits = Limits {
        error_after_n_firings: Some(2),
        ..Limits::default()
    };

    let error = drivers::run(&mut net, Mode::Sequential, limits, OnFailure::Stop)
        .expect_err("the third firing was attempted");

    let DriverError::TooManyFirings { limit, summary, .. } = error else {
        panic!("expected TooManyFirings");
    };
    assert_eq!(limit, 2);
    assert_eq!(summary.fired_count, 2);
    assert_eq!(values::<u32>(&net, out.id()), [2, 4]);
    assert_eq!(
        values::<u32>(&net, inbox.id()),
        [3, 4, 5],
        "the three that never fired are still waiting"
    );
}

#[test]
fn fuse_trips_even_though_the_selector_would_decline() {
    let (mut net, _inbox, _out, double) = straight([1, 2, 3]);
    net.set_selector(|_net, _fireable| None);
    let limits = Limits {
        error_after_n_firings: Some(0),
        ..Limits::default()
    };

    let error = drivers::run(&mut net, Mode::Sequential, limits, OnFailure::Stop)
        .expect_err("an enabled net with a spent fuse counts as attempting");

    let DriverError::TooManyFirings { transition, .. } = error else {
        panic!("expected TooManyFirings");
    };
    assert_eq!(
        transition, double,
        "the fuse reports the first fireable transition, unasked of the selector"
    );
    assert_eq!(net.fired_count(), 0);
}

// ----- sequential: what a failure means for the loop -----------------------

/// `Inbox -> Break` fails every time; `Spare -> Copy` is fireable beside it
/// and declared second, so the default selector reaches it only once `Break`
/// has had its turn.
fn one_broken_one_sound() -> (
    Net,
    PlaceRef<u32>,
    PlaceRef<u32>,
    PlaceRef<u32>,
    TransitionId,
) {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let spare = b.place::<u32>("Spare");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [1, 2]);
    b.seed(spare, [7, 8]);
    let broken = b
        .transition(
            "Break",
            (TakeEdge(inbox),),
            (out,),
            |_n: u32| -> Result<(u32,), Refused> { Err(Refused) },
        )
        .id();
    b.transition("Copy", (TakeEdge(spare),), (out,), |n: u32| (n,));
    let net = b.build().expect("one broken, one sound");
    (net, inbox, spare, out, broken)
}

#[test]
fn stop_on_failure_ends_a_sequential_run() {
    let (mut net, _inbox, spare, out, broken) = one_broken_one_sound();

    let error = drivers::run(
        &mut net,
        Mode::Sequential,
        Limits::default(),
        OnFailure::Stop,
    )
    .expect_err("the first firing failed");

    match error {
        DriverError::FiringFailed { summary, error } => {
            assert_eq!(error.transition, broken);
            assert_eq!(error.error.to_string(), "refused");
            assert!(!error.restored, "this transition does not restore");
            assert_eq!(summary.failed_count, 1);
            assert_eq!(summary.fired_count, 0);
            assert!(!summary.quiesced);
        }
        other => panic!("expected FiringFailed, got {other:?}"),
    }

    assert_eq!(
        net.count(spare.id()),
        2,
        "the run stopped: the sound transition never got a turn"
    );
    assert_eq!(net.count(out.id()), 0);
    assert!(net.in_flight().is_empty());
}

#[test]
fn disable_on_failure_switches_the_transition_off_and_carries_on() {
    let (mut net, inbox, spare, out, broken) = one_broken_one_sound();

    let summary = drivers::run(
        &mut net,
        Mode::Sequential,
        Limits::default(),
        OnFailure::Disable,
    )
    .expect("the run survived the failure");

    assert!(summary.quiesced, "the rest of the net finished");
    assert_eq!(summary.failed_count, 1, "one failure, then never again");
    assert_eq!(summary.fired_count, 2, "both spare tokens got through");
    assert!(!net.is_enabled(broken), "switched off by the driver");
    assert_eq!(
        net.count(inbox.id()),
        1,
        "the failed firing consumed its token; the other is stuck behind the switch"
    );
    assert_eq!(net.count(spare.id()), 0);
    assert_eq!(values::<u32>(&net, out.id()), [7, 8]);

    let events: Vec<Event> = net.drain_events().collect();
    assert!(
        events.contains(&Event::Toggled {
            transition: broken,
            enabled: false,
        }),
        "disabling is the net's event, not the driver's: {events:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Event::Failed { .. }))
            .count(),
        1,
    );
}

#[test]
fn continue_on_failure_with_restored_tokens_loops_until_the_fuse() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [1]);
    b.transition(
        "Break",
        (TakeEdge(inbox),),
        (out,),
        |_n: u32| -> Result<(u32,), Refused> { Err(Refused) },
    )
    .restore_tokens_on_failure();
    let mut net = b.build().expect("a transition that puts its token back");
    let limits = Limits {
        error_after_n_firings: Some(5),
        ..Limits::default()
    };

    let error = drivers::run(&mut net, Mode::Sequential, limits, OnFailure::Continue)
        .expect_err("restore plus Continue retries for ever; the fuse is the guard");

    let DriverError::TooManyFirings { limit, summary, .. } = error else {
        panic!("expected TooManyFirings");
    };
    assert_eq!(limit, 5);
    assert_eq!(summary.failed_count, 5, "the fuse counts attempts");
    assert_eq!(summary.fired_count, 0, "not one of them succeeded");
    assert_eq!(
        values::<u32>(&net, inbox.id()),
        [1],
        "the token is back where it started, which is why this loops"
    );
}

// ----- concurrent ----------------------------------------------------------

#[test]
fn concurrent_runs_independent_bodies_side_by_side() {
    let body = Duration::from_millis(50);
    let (mut net, _ins, outs) = independent(4, body);

    let started = Instant::now();
    let summary = drivers::run(
        &mut net,
        Mode::Concurrent,
        Limits::default(),
        OnFailure::Stop,
    )
    .expect("nothing fails");
    let elapsed = started.elapsed();

    assert!(summary.quiesced);
    assert_eq!(summary.fired_count, 4);
    assert_eq!(summary.failed_count, 0);
    assert!(net.in_flight().is_empty(), "nothing goes home in flight");
    for (i, out) in outs.iter().enumerate() {
        assert_eq!(values::<u32>(&net, out.id()), [100 + i as u32]);
    }
    assert!(
        elapsed < Duration::from_millis(150),
        "four 50 ms bodies ran side by side, not one after another: {elapsed:?}"
    );
}

#[test]
fn concurrent_chain_reaches_quiescence() {
    let (mut net, inbox, middle, out) = chain([1, 2, 3]);

    let summary = drivers::run(
        &mut net,
        Mode::Concurrent,
        Limits::default(),
        OnFailure::Stop,
    )
    .expect("nothing fails");

    assert!(summary.quiesced);
    assert_eq!(summary.fired_count, 6);
    assert_eq!(net.count(inbox.id()), 0);
    assert_eq!(net.count(middle.id()), 0);
    assert_eq!(
        sorted(&net, out.id()),
        [3, 5, 7],
        "arrival order is wall-clock order; the multiset is the net's"
    );
    assert!(net.in_flight().is_empty());
}

#[test]
fn concurrent_mixes_inline_and_detachable_firings() {
    let mut b = NetBuilder::new();
    let ticks = b.place::<u32>("Ticks");
    let live = b.place::<u32>("Live");
    let jobs = b.place::<u32>("Jobs");
    let done = b.place::<u32>("Done");
    b.seed(ticks, [5]);
    b.seed(live, [10]);
    b.seed(jobs, [1, 2]);
    let bump = b
        .transition(
            "Bump",
            (TakeEdge(ticks), MutateEdge(live)),
            (),
            |tick: u32, live: &mut u32| {
                *live += tick;
            },
        )
        .id();
    b.transition("Work", (TakeEdge(jobs),), (done,), |n: u32| {
        thread::sleep(Duration::from_millis(30));
        (n * 10,)
    });
    let mut net = b.build().expect("one inline and one detachable transition");

    let summary = drivers::run(
        &mut net,
        Mode::Concurrent,
        Limits::default(),
        OnFailure::Stop,
    )
    .expect("nothing fails");

    assert!(summary.quiesced);
    assert_eq!(summary.fired_count, 3, "one inline firing and two detached");
    assert_eq!(
        values::<u32>(&net, live.id()),
        [15],
        "the inline body ran inside launch, where the token lies"
    );
    assert_eq!(sorted(&net, done.id()), [10, 20]);
    assert!(net.in_flight().is_empty());
    assert!(!net.is_in_flight(bump));

    let events: Vec<Event> = net.drain_events().collect();
    assert!(
        events.contains(&Event::Mutated {
            place: live.id(),
            token: net.tokens(live.id()).next().expect("one token").0,
        }),
        "a mutating edge reports itself: {events:?}"
    );
}

/// Several transitions launched in one sweep, exactly one of which fails
/// after a moment, the others sleeping a little longer so that the failure is
/// seen while they are still airborne.
fn one_failing_among_sleepers() -> (Net, Vec<PlaceRef<u32>>, TransitionId) {
    let mut b = NetBuilder::new();
    let bad_in = b.place::<u32>("BadIn");
    let bad_out = b.place::<u32>("BadOut");
    b.seed(bad_in, [0]);
    let broken = b
        .transition(
            "Break",
            (TakeEdge(bad_in),),
            (bad_out,),
            |_n: u32| -> Result<(u32,), Refused> { Err(Refused) },
        )
        .id();
    let mut outs = Vec::new();
    for i in 0..3_u32 {
        let inbox = b.place::<u32>(format!("In{i}"));
        let out = b.place::<u32>(format!("Out{i}"));
        b.seed(inbox, [i]);
        b.transition(
            format!("Work{i}"),
            (TakeEdge(inbox),),
            (out,),
            move |n: u32| {
                thread::sleep(Duration::from_millis(40));
                (n + 100,)
            },
        );
        outs.push(out);
    }
    let net = b.build().expect("one broken among three sleepers");
    (net, outs, broken)
}

#[test]
fn concurrent_stop_on_failure_drains_everything_first() {
    let (mut net, outs, broken) = one_failing_among_sleepers();

    let error = drivers::run(
        &mut net,
        Mode::Concurrent,
        Limits::default(),
        OnFailure::Stop,
    )
    .expect_err("one body returned Err");

    match error {
        DriverError::FiringFailed { summary, error } => {
            assert_eq!(error.transition, broken);
            assert_eq!(summary.failed_count, 1);
            assert_eq!(summary.fired_count, 3, "the siblings were waited for");
            assert!(!summary.quiesced);
        }
        other => panic!("expected FiringFailed, got {other:?}"),
    }

    assert!(
        net.in_flight().is_empty(),
        "a dropped Completed would strand its transition for good"
    );
    for (i, out) in outs.iter().enumerate() {
        assert_eq!(
            values::<u32>(&net, out.id()),
            [100 + i as u32],
            "every firing that was already airborne landed"
        );
    }
}

#[test]
fn concurrent_disable_on_failure_switches_off_and_finishes() {
    let (mut net, outs, broken) = one_failing_among_sleepers();

    let summary = drivers::run(
        &mut net,
        Mode::Concurrent,
        Limits::default(),
        OnFailure::Disable,
    )
    .expect("the run survived");

    assert!(summary.quiesced);
    assert_eq!(summary.failed_count, 1);
    assert_eq!(summary.fired_count, 3);
    assert!(!net.is_enabled(broken));
    assert!(net.in_flight().is_empty());
    for (i, out) in outs.iter().enumerate() {
        assert_eq!(values::<u32>(&net, out.id()), [100 + i as u32]);
    }
}

#[test]
fn concurrent_continue_on_failure_only_counts_it() {
    let (mut net, _outs, broken) = one_failing_among_sleepers();

    let summary = drivers::run(
        &mut net,
        Mode::Concurrent,
        Limits::default(),
        OnFailure::Continue,
    )
    .expect("a counted failure is not an error");

    assert!(summary.quiesced);
    assert_eq!(summary.failed_count, 1);
    assert_eq!(summary.fired_count, 3);
    assert!(
        net.is_enabled(broken),
        "Continue leaves the switch exactly as it found it"
    );
    assert!(net.in_flight().is_empty());
    assert_eq!(
        failed_events(&mut net).len(),
        1,
        "the event log is where a continued failure lives"
    );
}

#[test]
fn concurrent_stop_after_n_firings_caps_launches() {
    let (mut net, ins, outs) = independent(4, Duration::from_millis(10));
    let paced = Limits {
        stop_after_n_firings: Some(2),
        ..Limits::default()
    };

    let summary = drivers::run(&mut net, Mode::Concurrent, paced, OnFailure::Stop).expect("paced");

    assert!(!summary.quiesced);
    assert_eq!(summary.fired_count, 2, "the cap is on launches");
    assert_eq!(summary.failed_count, 0);
    assert!(
        net.in_flight().is_empty(),
        "the two that were launched were waited for"
    );
    assert_eq!(net.count(outs[0].id()), 1);
    assert_eq!(net.count(outs[1].id()), 1);
    assert_eq!(net.count(outs[2].id()), 0);
    assert_eq!(net.count(ins[2].id()), 1, "never consumed");
    assert_eq!(net.count(ins[3].id()), 1);
}

#[test]
fn concurrent_fuse_trips_in_the_launch_sweep() {
    let (mut net, ins, _outs) = independent(3, Duration::from_millis(10));
    let limits = Limits {
        error_after_n_firings: Some(1),
        ..Limits::default()
    };

    let error = drivers::run(&mut net, Mode::Concurrent, limits, OnFailure::Stop)
        .expect_err("the second launch of the sweep was refused");

    let DriverError::TooManyFirings {
        limit,
        transition,
        summary,
    } = error
    else {
        panic!("expected TooManyFirings");
    };
    assert_eq!(limit, 1);
    assert_eq!(
        transition,
        net.transition_named("Work1").expect("declared second"),
        "the one that was about to be launched"
    );
    assert_eq!(summary.fired_count, 1, "the first launch was seen through");
    assert!(
        net.in_flight().is_empty(),
        "the fuse drains before it reports"
    );
    assert_eq!(
        net.count(ins[1].id()),
        1,
        "nothing was consumed for the launch that did not happen"
    );
}

// ----- run_indefinitely ----------------------------------------------------

/// Sets `stop` after `after`, recording the instant it did, so a test can
/// measure how long the driver took to notice.
fn stopper(
    stop: Arc<AtomicBool>,
    after: Duration,
) -> (thread::JoinHandle<()>, Arc<Mutex<Option<Instant>>>) {
    let set_at = Arc::new(Mutex::new(None));
    let recorder = Arc::clone(&set_at);
    let handle = thread::spawn(move || {
        thread::sleep(after);
        *recorder.lock().expect("no panic in this test") = Some(Instant::now());
        stop.store(true, Ordering::Relaxed);
    });
    (handle, set_at)
}

#[test]
fn run_indefinitely_sequential_fires_a_token_a_tick_until_stopped() {
    let (mut net, inbox, out, _double) = straight([]);
    let stop = Arc::new(AtomicBool::new(false));
    let (handle, set_at) = stopper(Arc::clone(&stop), Duration::from_millis(100));
    let ticks = AtomicUsize::new(0);

    let summary = drivers::run_indefinitely(
        &mut net,
        Mode::Sequential,
        Duration::from_millis(10),
        &stop,
        OnFailure::Stop,
        |net| {
            ticks.fetch_add(1, Ordering::Relaxed);
            net.put(inbox, 1);
        },
    )
    .expect("nothing fails");
    let returned = Instant::now();
    handle.join().expect("the stopper finished");

    let set_at = set_at.lock().expect("no panic").expect("stop was set");
    assert!(
        returned.duration_since(set_at) < Duration::from_millis(500),
        "the driver noticed the stop promptly"
    );
    assert!(summary.fired_count > 0, "it fired while it waited");
    assert_eq!(summary.failed_count, 0);
    assert!(ticks.load(Ordering::Relaxed) > 0, "each_tick ran");
    assert_eq!(
        net.count(out.id()) as u64,
        summary.fired_count,
        "one firing per tick, each doubling one token"
    );
    assert!(net.in_flight().is_empty());
}

#[test]
fn run_indefinitely_concurrent_drains_before_it_returns() {
    let (mut net, ins, outs) = independent(3, Duration::from_millis(30));
    let stop = Arc::new(AtomicBool::new(false));
    let (handle, _set_at) = stopper(Arc::clone(&stop), Duration::from_millis(80));
    let refilled = AtomicUsize::new(0);

    let summary = drivers::run_indefinitely(
        &mut net,
        Mode::Concurrent,
        Duration::from_millis(10),
        &stop,
        OnFailure::Stop,
        |net| {
            // Keep it busy: put a token back into the first place each tick,
            // so there is something airborne when the stop arrives.
            if refilled.fetch_add(1, Ordering::Relaxed) < 6 {
                net.put(ins[0], 1);
            }
        },
    )
    .expect("nothing fails");
    handle.join().expect("the stopper finished");

    assert!(summary.fired_count >= 3, "at least the seeded tokens");
    assert_eq!(summary.failed_count, 0);
    assert!(
        net.in_flight().is_empty(),
        "every airborne firing was waited for and distributed"
    );
    assert!(net.count(outs[0].id()) >= 1);
}

#[test]
fn run_indefinitely_waits_for_stop_on_an_idle_net() {
    let (mut net, _inbox, _out, _double) = straight([]);
    let stop = Arc::new(AtomicBool::new(false));
    let (handle, _set_at) = stopper(Arc::clone(&stop), Duration::from_millis(60));

    let started = Instant::now();
    let summary = drivers::run_indefinitely(
        &mut net,
        Mode::Sequential,
        Duration::from_millis(10),
        &stop,
        OnFailure::Stop,
        |_net| {},
    )
    .expect("an idle net is not an error");
    let elapsed = started.elapsed();
    handle.join().expect("the stopper finished");

    assert!(
        elapsed >= Duration::from_millis(50),
        "an idle tick is not a finish line: {elapsed:?}"
    );
    assert_eq!(summary.fired_count, 0);
    assert_eq!(summary.failed_count, 0);
    assert!(summary.quiesced, "nothing was fireable when it returned");
}

// ----- the selector, and the shapes the API promises -----------------------

#[test]
fn clear_selector_restores_the_default() {
    let (mut net, _inbox, out, double) = straight([1]);

    net.set_selector(|_net, _fireable| None);
    assert_eq!(net.step().expect("no body ran"), None);
    assert_eq!(net.fireable(), [double], "fireable, but not chosen");

    net.clear_selector();
    assert_eq!(net.step().expect("it fires now"), Some(double));
    assert_eq!(values::<u32>(&net, out.id()), [2]);
}

#[test]
fn a_cleared_selector_lets_a_whole_run_finish() {
    let (mut net, _inbox, out, _double) = straight([1, 2, 3]);
    net.set_selector(|_net, _fireable| None);

    let stalled = drivers::run(
        &mut net,
        Mode::Sequential,
        Limits::default(),
        OnFailure::Stop,
    )
    .expect("a declining selector is quiescence to this driver");
    assert!(stalled.quiesced);
    assert_eq!(stalled.fired_count, 0);

    net.clear_selector();
    let summary = drivers::run(
        &mut net,
        Mode::Sequential,
        Limits::default(),
        OnFailure::Stop,
    )
    .expect("nothing fails");
    assert!(summary.quiesced);
    assert_eq!(summary.fired_count, 3);
    assert_eq!(values::<u32>(&net, out.id()), [2, 4, 6]);
}

#[test]
fn a_zero_stop_limit_launches_nothing_in_either_mode() {
    let limits = Limits {
        stop_after_n_firings: Some(0),
        ..Limits::default()
    };
    for mode in [Mode::Sequential, Mode::Concurrent] {
        let (mut net, inbox, out, _double) = straight([1, 2, 3]);

        let summary = drivers::run(&mut net, mode, limits, OnFailure::Stop).expect("no firing");

        assert!(
            !summary.quiesced,
            "{mode:?}: a capped call says nothing about the net's own finish line"
        );
        assert_eq!(summary.fired_count, 0);
        assert_eq!(net.count(inbox.id()), 3, "{mode:?}: nothing was consumed");
        assert_eq!(net.count(out.id()), 0);
        assert!(net.in_flight().is_empty());
    }
}

#[test]
fn defaults_are_what_the_docs_say() {
    assert_eq!(Mode::default(), Mode::Sequential);
    assert_eq!(OnFailure::default(), OnFailure::Stop);
    assert_eq!(
        Limits::default(),
        Limits {
            stop_after_n_firings: None,
            error_after_n_firings: None,
        },
    );
    assert_eq!(
        RunSummary::default(),
        RunSummary {
            quiesced: false,
            fired_count: 0,
            failed_count: 0,
        },
    );
}

#[test]
fn driver_types_travel() {
    assert_send_sync::<DriverError>();
    assert_send_sync::<RunSummary>();
    assert_copy::<RunSummary>();
    assert_copy::<Limits>();
    assert_copy::<Mode>();
    assert_copy::<OnFailure>();
}
