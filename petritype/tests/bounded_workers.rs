//! The concurrent driver runs its bodies on a bounded pool of worker
//! threads (since 2026-11-28; until then each launch had a thread of its
//! own): at most `available_parallelism` for `Mode::Concurrent`, at most
//! `workers` for `Mode::ConcurrentWith`, each spawned only when a body is
//! handed over and no worker is free to take it. The bodies count
//! themselves here, how many run at once and on which threads.

use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread::{self, ThreadId};
use std::time::Duration;

use petritype::drivers::{self, Limits, Mode, OnFailure, RunSummary};
use petritype::{MutateEdge, Net, NetBuilder, TakeEdge};

/// What the bodies saw: how many ran at once at most, and the threads they
/// ran on.
#[derive(Default)]
struct Watch {
    live: AtomicUsize,
    peak: AtomicUsize,
    threads: Mutex<HashSet<ThreadId>>,
    names: Mutex<HashSet<Option<String>>>,
}

impl Watch {
    /// One body's run: counted in, held for `hold`, counted out.
    fn body(&self, hold: Duration) {
        let inside = self.live.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(inside, Ordering::SeqCst);
        let current = thread::current();
        self.threads.lock().unwrap().insert(current.id());
        self.names
            .lock()
            .unwrap()
            .insert(current.name().map(str::to_owned));
        thread::sleep(hold);
        self.live.fetch_sub(1, Ordering::SeqCst);
    }

    fn peak(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }

    fn threads(&self) -> HashSet<ThreadId> {
        self.threads.lock().unwrap().clone()
    }
}

/// `width` independent transitions, each with one token of its own, each
/// body watched and holding for `hold`: everything fireable at once.
fn independent(width: usize, hold: Duration) -> (Net, Arc<Watch>) {
    let watch = Arc::new(Watch::default());
    let mut b = NetBuilder::new();
    for i in 0..width {
        let inbox = b.place::<usize>(format!("In{i}"));
        let out = b.place::<usize>(format!("Out{i}"));
        b.seed(inbox, [i]);
        let watch = Arc::clone(&watch);
        b.transition(
            format!("Work{i}"),
            (TakeEdge(inbox),),
            (out,),
            move |n: usize| {
                watch.body(hold);
                (n,)
            },
        );
    }
    (b.build().expect("independent transitions"), watch)
}

/// A chain of `stages` transitions with one token at its top: never more
/// than one body to run at a time.
fn chain(stages: usize, hold: Duration) -> (Net, Arc<Watch>) {
    let watch = Arc::new(Watch::default());
    let mut b = NetBuilder::new();
    let mut from = b.place::<u32>("Stage0");
    b.seed(from, [0]);
    for i in 1..=stages {
        let to = b.place::<u32>(format!("Stage{i}"));
        let watch = Arc::clone(&watch);
        b.transition(
            format!("Step{i}"),
            (TakeEdge(from),),
            (to,),
            move |n: u32| {
                watch.body(hold);
                (n + 1,)
            },
        );
        from = to;
    }
    (b.build().expect("a chain"), watch)
}

fn workers(n: usize) -> Mode {
    Mode::ConcurrentWith {
        workers: NonZeroUsize::new(n).expect("not zero"),
    }
}

fn run(net: &mut Net, mode: Mode) -> RunSummary {
    drivers::run(net, mode, Limits::default(), OnFailure::Stop).expect("nothing fails")
}

#[test]
fn a_run_never_has_more_bodies_running_than_its_cap() {
    for cap in [1, 2, 3, 5] {
        let (mut net, watch) = independent(16, Duration::from_millis(10));
        let summary = run(&mut net, workers(cap));
        assert!(summary.quiesced);
        assert_eq!(summary.fired_count, 16, "cap {cap}: every body ran");
        assert!(watch.peak() <= cap, "cap {cap}: {} at once", watch.peak());
        assert!(
            watch.threads().len() <= cap,
            "cap {cap}: {} threads",
            watch.threads().len()
        );
        assert!(net.in_flight().is_empty());
    }
}

#[test]
fn the_default_cap_is_the_machine_s_parallelism() {
    let cap = thread::available_parallelism().map_or(1, NonZeroUsize::get);
    let (mut net, watch) = independent(2 * cap + 1, Duration::from_millis(20));
    let summary = run(&mut net, Mode::Concurrent);
    assert_eq!(summary.fired_count, 2 * cap as u64 + 1);
    assert!(watch.peak() <= cap, "{} at once, cap {cap}", watch.peak());
    assert!(watch.threads().len() <= cap);
}

#[test]
fn with_one_worker_a_run_still_makes_progress() {
    let (mut net, watch) = chain(4, Duration::from_millis(1));
    let summary = run(&mut net, workers(1));
    assert!(summary.quiesced);
    assert_eq!(summary.fired_count, 4);
    assert_eq!(watch.peak(), 1);
    let threads = watch.threads();
    assert_eq!(threads.len(), 1, "one worker ran every body");
    assert!(
        !threads.contains(&thread::current().id()),
        "off the driver's thread"
    );

    let (mut net, watch) = independent(6, Duration::from_millis(1));
    let stop = AtomicBool::new(false);
    let summary = drivers::run_indefinitely(
        &mut net,
        workers(1),
        Duration::from_millis(1),
        &stop,
        OnFailure::Stop,
        |net| {
            if net.fired_count() == 6 {
                stop.store(true, Ordering::Release);
            }
        },
    )
    .expect("nothing fails");
    assert_eq!(summary.fired_count, 6);
    assert_eq!(watch.peak(), 1);
}

#[test]
fn a_worker_is_spawned_only_when_no_worker_is_free() {
    // One body at a time down a chain: the one worker is free each time.
    let (mut net, watch) = chain(5, Duration::from_millis(1));
    run(&mut net, workers(8));
    assert_eq!(watch.threads().len(), 1);

    // Two bodies that each wait for the other to start: two workers, of
    // eight allowed.
    let watch = Arc::new(Watch::default());
    let both = Arc::new(Barrier::new(2));
    let mut b = NetBuilder::new();
    for i in 0..2 {
        let inbox = b.place::<u32>(format!("In{i}"));
        let out = b.place::<u32>(format!("Out{i}"));
        b.seed(inbox, [i]);
        let (watch, both) = (Arc::clone(&watch), Arc::clone(&both));
        b.transition(
            format!("Work{i}"),
            (TakeEdge(inbox),),
            (out,),
            move |n: u32| {
                both.wait();
                watch.body(Duration::ZERO);
                (n,)
            },
        );
    }
    let mut net = b.build().expect("two independent transitions");
    run(&mut net, workers(8));
    assert_eq!(watch.threads().len(), 2);
}

#[test]
fn the_workers_are_named() {
    let (mut net, watch) = independent(3, Duration::from_millis(1));
    run(&mut net, Mode::Concurrent);
    assert_eq!(
        *watch.names.lock().unwrap(),
        HashSet::from([Some("petritype worker".to_owned())])
    );
}

#[test]
fn a_net_whose_firings_all_run_inline_spawns_no_worker() {
    let threads: Arc<Mutex<HashSet<ThreadId>>> = Arc::default();
    let mut b = NetBuilder::new();
    let ticks = b.place::<u32>("Ticks");
    let live = b.place::<u32>("Live");
    b.seed(ticks, [1, 2, 3]);
    b.seed(live, [0]);
    let seen = Arc::clone(&threads);
    b.transition(
        "Bump",
        (TakeEdge(ticks), MutateEdge(live)),
        (),
        move |tick: u32, live: &mut u32| {
            seen.lock().unwrap().insert(thread::current().id());
            *live += tick;
        },
    );
    let mut net = b.build().expect("one inline transition");

    let summary = run(&mut net, Mode::Concurrent);

    assert_eq!(summary.fired_count, 3);
    assert_eq!(
        *threads.lock().unwrap(),
        HashSet::from([thread::current().id()]),
        "every body ran on the driver's thread"
    );
}

#[test]
fn bodies_beyond_the_cap_wait_their_turn_in_launch_order() {
    let order: Arc<Mutex<Vec<usize>>> = Arc::default();
    let mut b = NetBuilder::new();
    for i in 0..5 {
        let inbox = b.place::<usize>(format!("In{i}"));
        let out = b.place::<usize>(format!("Out{i}"));
        b.seed(inbox, [i]);
        let order = Arc::clone(&order);
        b.transition(
            format!("Work{i}"),
            (TakeEdge(inbox),),
            (out,),
            move |n: usize| {
                order.lock().unwrap().push(n);
                (n,)
            },
        );
    }
    let mut net = b.build().expect("independent transitions");

    run(&mut net, workers(1));

    assert_eq!(*order.lock().unwrap(), [0, 1, 2, 3, 4]);
}
