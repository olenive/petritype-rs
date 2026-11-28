//! Deep concurrent firing (since 0.4.0): the concurrent driver launches the
//! detachable firings of the nets inside a net, at any depth, beside the
//! net's own, onto the one bounded pool of the call, and lands each in the
//! net that launched it, by the way down to it.
//!
//! These tests count the bodies that run at once with deadlines, never
//! sleeps: a body waits until the bodies it should run beside have started,
//! or until [`DEADLINE`], which only a run that cannot overlap them meets.
//! They show the cap holding across levels; the busy-token rule for every
//! kind of edge that can read a place of nets, readers and guards included;
//! a drain holding its place until it has had its turn, on nets that never
//! rest, and holding nothing else; each stepping rule, with no net starved, beside the sequential deep step;
//! the same final marking and the same firings as the sequential deep step
//! on confluent nets, under several caps; two nets of one name in one token
//! under `OnFailure::Disable`; a failure, a choice that cannot be made and a
//! panic inside, each landing what is in flight; the limits counted at every
//! level; subnets whose ports the net around them uses; three levels;
//! closed nets at every level; and a body that drives a net it holds.

use std::any::Any;
use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, ThreadId};
use std::time::{Duration, Instant};

use petritype::drivers::{self, DriverError, Limits, Mode, OnFailure, RunSummary};
use petritype::{
    DrainEdge, Event, Filter, KeyValue, MutateEdge, MutateWhereEdge, Nested, Net, NetBuilder,
    PathStep, PlaceId, PlaceRef, ReadAllEdge, ReadEdge, ReadWhereEdge, SelectionFault, Stepping,
    TakeAllEdge, TakeEdge, TakeWhereEdge,
};

/// Long enough for a loaded machine to start every body a test waits for.
/// A body waits this long only when the bodies it waits for cannot run
/// beside it, and the test then fails on what it counted.
const DEADLINE: Duration = Duration::from_secs(20);

// ----- watching the bodies --------------------------------------------------------

/// How many bodies have run at once, and on which threads.
#[derive(Default)]
struct Overlap {
    /// Bodies running now, and the most there have been at once.
    state: Mutex<(usize, usize)>,
    changed: Condvar,
    threads: Mutex<HashSet<ThreadId>>,
}

impl Overlap {
    /// One body: counted in, then held until `together` bodies have been
    /// running at once, or the deadline has passed; then counted out.
    fn body(&self, together: usize) {
        self.threads.lock().unwrap().insert(thread::current().id());
        let deadline = Instant::now() + DEADLINE;
        let mut state = self.state.lock().unwrap();
        state.0 += 1;
        state.1 = state.1.max(state.0);
        self.changed.notify_all();
        while state.1 < together {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            state = self.changed.wait_timeout(state, left).unwrap().0;
        }
        state.0 -= 1;
    }

    fn peak(&self) -> usize {
        self.state.lock().unwrap().1
    }

    fn threads(&self) -> usize {
        self.threads.lock().unwrap().len()
    }
}

/// A count a body can wait on, up to the deadline.
#[derive(Default)]
struct Count {
    value: Mutex<usize>,
    changed: Condvar,
}

impl Count {
    fn add(&self) {
        *self.value.lock().unwrap() += 1;
        self.changed.notify_all();
    }

    /// Waits until the count is at least `at_least`, or the deadline.
    fn wait_for(&self, at_least: usize) {
        let deadline = Instant::now() + DEADLINE;
        let mut value = self.value.lock().unwrap();
        while *value < at_least {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            value = self.changed.wait_timeout(value, left).unwrap().0;
        }
    }
}

fn workers(n: usize) -> Mode {
    Mode::ConcurrentWith {
        workers: NonZeroUsize::new(n).expect("not zero"),
    }
}

fn run(net: &mut Net, mode: Mode) -> RunSummary {
    drivers::run(net, mode, Limits::default(), OnFailure::Stop).expect("nothing fails")
}

// ----- a few nets -------------------------------------------------------------------

/// An inner net with `jobs` things to do, `Todo -Work-> Done`, `body`
/// called with each job. Its `Todo` is a port.
fn worker(jobs: u32, body: impl Fn(u32) + Send + Sync + 'static) -> Net {
    let mut b = NetBuilder::new();
    let todo = b.place::<u32>("Todo");
    let done = b.place::<u32>("Done");
    b.port(todo);
    b.seed(todo, 0..jobs);
    b.transition("Work", (TakeEdge(todo),), (done,), move |n: u32| {
        body(n);
        (n,)
    });
    b.build().expect("a worker")
}

/// A worker whose bodies each wait in `overlap` for `together`.
fn overlapping(jobs: u32, overlap: &Arc<Overlap>, together: usize) -> Net {
    let overlap = Arc::clone(overlap);
    worker(jobs, move |_| overlap.body(together))
}

/// An inner net that never comes to rest: `Spin` puts back what it takes.
fn spinner() -> Net {
    let mut b = NetBuilder::new();
    let wheel = b.place::<u32>("Wheel");
    b.seed(wheel, [0]);
    b.transition("Spin", (TakeEdge(wheel),), (wheel,), |n: u32| (n + 1,));
    b.build().expect("a spinner")
}

/// The tokens of a place in `net`, in order, with the first of each
/// token's nets.
fn nets_in(net: &Net, place: PlaceId) -> Vec<&Net> {
    net.tokens(place)
        .map(|(token, _)| net.nested(place, token).expect("a nested place")[0].1)
        .collect()
}

/// How many tokens a place of `net` named `name` holds.
fn count(net: &Net, name: &str) -> usize {
    net.count(net.place_named(name).expect("a place of that name"))
}

/// Whether nothing is in flight in `net` or in any net inside it.
fn nothing_in_flight(net: &Net) -> bool {
    net.in_flight().is_empty()
        && net.places().all(|(place, info)| {
            if info.subnet {
                return net.subnet(place).is_none_or(nothing_in_flight);
            }
            net.tokens(place).all(|(token, _)| {
                net.nested(place, token)
                    .is_none_or(|nets| nets.iter().all(|(_, inner)| nothing_in_flight(inner)))
            })
        })
}

/// The events of `net`'s log, each unwrapped from every `Nested` and
/// `Subnet` around it, with how deep it was.
fn unwrapped(events: &[Event]) -> Vec<(usize, &Event)> {
    fn open(event: &Event, depth: usize) -> (usize, &Event) {
        match event {
            Event::Nested { event, .. } | Event::Subnet { event, .. } => open(event, depth + 1),
            other => (depth, other),
        }
    }
    events.iter().map(|event| open(event, 0)).collect()
}

// ----- bodies that overlap ---------------------------------------------------------

#[test]
fn sibling_inner_nets_run_their_bodies_at_once_under_each_rule() {
    for stepping in [
        Stepping::InnerFirst,
        Stepping::TakeTurns,
        Stepping::OuterFirst,
    ] {
        let overlap = Arc::new(Overlap::default());
        let mut b = NetBuilder::new();
        let yard = b.place_nested::<Net>("Yard");
        b.seed(yard, (0..4).map(|_| overlapping(1, &overlap, 4)));
        b.stepping(yard, stepping);
        let mut net = b.build().expect("a yard of four workers");

        let summary = run(&mut net, workers(4));

        assert!(summary.quiesced, "{stepping:?}");
        assert_eq!(summary.fired_count, 4, "{stepping:?}");
        assert_eq!(
            overlap.peak(),
            4,
            "{stepping:?}: the four bodies ran at once"
        );
        for inner in nets_in(&net, yard.id()) {
            assert_eq!(count(inner, "Done"), 1, "{stepping:?}");
        }
        assert!(nothing_in_flight(&net));
    }
}

#[test]
fn subnets_and_the_net_around_them_run_their_bodies_at_once() {
    let overlap = Arc::new(Overlap::default());
    let mut b = NetBuilder::new();
    let first = b.subnet_place("First", overlapping(1, &overlap, 3));
    let second = b.subnet_place("Second", overlapping(1, &overlap, 3));
    let inbox = b.place::<u32>("Inbox");
    let outbox = b.place::<u32>("Outbox");
    b.seed(inbox, [7]);
    let mine = Arc::clone(&overlap);
    b.transition("Forward", (TakeEdge(inbox),), (outbox,), move |n: u32| {
        mine.body(3);
        (n,)
    });
    // Taking turns, the two subnets go beside the net's own sweep.
    b.stepping(first, Stepping::TakeTurns);
    b.stepping(second, Stepping::TakeTurns);
    let mut net = b.build().expect("two subnets beside a transition");

    let summary = run(&mut net, workers(3));

    assert_eq!(summary.fired_count, 3);
    assert_eq!(
        overlap.peak(),
        3,
        "two subnets' bodies and the outer one at once"
    );
    for place in [first.id(), second.id()] {
        assert_eq!(count(net.subnet(place).expect("a subnet"), "Done"), 1);
    }
    assert_eq!(net.count(outbox.id()), 1);
}

#[test]
fn nets_three_levels_down_run_together_and_land_through_every_level() {
    let overlap = Arc::new(Overlap::default());
    // Two middle nets, each with two bottom workers in an inner-first place.
    let middle = |overlap: &Arc<Overlap>| {
        let mut b = NetBuilder::new();
        let bench = b.place_nested::<Net>("Bench");
        b.seed(bench, (0..2).map(|_| overlapping(1, overlap, 4)));
        b.build().expect("a middle net")
    };
    let mut b = NetBuilder::new();
    let yard = b.place_nested::<Net>("Yard");
    b.seed(yard, [middle(&overlap), middle(&overlap)]);
    let mut net = b.build().expect("three levels");

    let summary = run(&mut net, workers(4));

    assert_eq!(summary.fired_count, 4);
    assert_eq!(overlap.peak(), 4, "the four bottom bodies at once");
    let events: Vec<Event> = net.drain_events().collect();
    let landed = unwrapped(&events)
        .into_iter()
        .filter(|(depth, event)| *depth == 2 && matches!(event, Event::Distributed { .. }))
        .count();
    assert_eq!(landed, 4, "each landing told two levels up");
    assert!(nothing_in_flight(&net));
}

#[test]
fn run_indefinitely_runs_sibling_inner_nets_at_once() {
    let overlap = Arc::new(Overlap::default());
    let mut b = NetBuilder::new();
    let yard = b.place_nested::<Net>("Yard");
    b.seed(yard, (0..3).map(|_| overlapping(1, &overlap, 3)));
    let mut net = b.build().expect("a yard");
    let stop = AtomicBool::new(false);
    let started = Instant::now();
    let summary = drivers::run_indefinitely(
        &mut net,
        workers(3),
        Duration::from_millis(1),
        &stop,
        OnFailure::Stop,
        |net| {
            let done: usize = nets_in(net, yard.id())
                .iter()
                .map(|inner| count(inner, "Done"))
                .sum();
            if done == 3 || started.elapsed() > DEADLINE {
                stop.store(true, Ordering::Release);
            }
        },
    )
    .expect("nothing fails");
    assert_eq!(summary.fired_count, 3);
    assert_eq!(overlap.peak(), 3);
}

// ----- the cap --------------------------------------------------------------------

/// A crew member: a worker with `jobs`, and an inner-first `Tools` place
/// holding a tool with `tool_jobs`, every body waiting in `overlap`.
fn crew_member(jobs: u32, tool_jobs: u32, overlap: &Arc<Overlap>, together: usize) -> Net {
    let mut b = NetBuilder::new();
    let todo = b.place::<u32>("Todo");
    let done = b.place::<u32>("Done");
    let tools = b.place_nested::<Net>("Tools");
    b.seed(todo, 0..jobs);
    b.seed(tools, [overlapping(tool_jobs, overlap, together)]);
    let mine = Arc::clone(overlap);
    b.transition("Work", (TakeEdge(todo),), (done,), move |n: u32| {
        mine.body(together);
        (n,)
    });
    b.build().expect("a crew member")
}

#[test]
fn the_cap_holds_across_levels() {
    for cap in [1, 2, 3, 5] {
        let overlap = Arc::new(Overlap::default());
        let mut b = NetBuilder::new();
        // Four bodies of the net's own, three crew members with two jobs
        // each, and each member's tool with two: sixteen bodies, three
        // levels.
        for i in 0..4 {
            let inbox = b.place::<u32>(format!("In{i}"));
            let out = b.place::<u32>(format!("Out{i}"));
            b.seed(inbox, [i]);
            let mine = Arc::clone(&overlap);
            b.transition(
                format!("Own{i}"),
                (TakeEdge(inbox),),
                (out,),
                move |n: u32| {
                    mine.body(cap);
                    (n,)
                },
            );
        }
        let crew = b.place_nested::<Net>("Crew");
        b.seed(crew, (0..3).map(|_| crew_member(2, 2, &overlap, cap)));
        b.stepping(crew, Stepping::TakeTurns);
        let mut net = b.build().expect("three levels of bodies");

        let summary = run(&mut net, workers(cap));

        assert!(summary.quiesced, "cap {cap}");
        assert_eq!(summary.fired_count, 16, "cap {cap}: every body ran");
        assert_eq!(overlap.peak(), cap, "cap {cap}: as many at once as the cap");
        assert!(
            overlap.threads() <= cap,
            "cap {cap}: {} threads",
            overlap.threads()
        );
        assert!(nothing_in_flight(&net));
    }
}

// ----- the busy-token rule ------------------------------------------------------

/// A token that carries a net, with a name a filter can choose it by.
#[derive(Debug)]
struct Unit {
    name: String,
    net: Net,
}

impl Nested for Unit {
    fn nets(&self) -> Vec<(&str, &Net)> {
        vec![("unit", &self.net)]
    }
    fn nets_mut(&mut self) -> Vec<(&str, &mut Net)> {
        vec![("unit", &mut self.net)]
    }
}

/// Every kind of edge that can read a place of nets. Snapshot and survey
/// edges copy tokens, which a place of nets cannot do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Take,
    TakeWhere,
    TakeAll,
    Drain,
    Mutate,
    MutateWhere,
    Read,
    ReadWhere,
    ReadAll,
}

impl Kind {
    fn reads(self) -> bool {
        matches!(self, Kind::Read | Kind::ReadWhere | Kind::ReadAll)
    }
}

/// What the edge under test was handed.
#[derive(Default)]
struct Handed {
    /// Firings of the transition under test.
    fired: AtomicUsize,
    /// Units it was handed with a firing in flight inside.
    in_flight: AtomicUsize,
    /// Units it was handed with nothing in flight, by name.
    at_rest: Mutex<Vec<String>>,
}

impl Handed {
    fn hand(&self, unit: &Unit) {
        if unit.net.in_flight().is_empty() {
            self.at_rest.lock().unwrap().push(unit.name.clone());
        } else {
            self.in_flight.fetch_add(1, Ordering::SeqCst);
        }
    }
}

/// A take-turns place of two units: `a`, whose one `Work` waits for three
/// `Tick`s of the net around it before it lands, and `b`, idle. Beside it,
/// `Tick` three times, and `Under test`, which reads the place through an
/// edge of `kind` once `a` has launched (its guard reads that `a` has
/// nothing left to do) and has a token each in `Once` for two firings.
/// `Watch`'s guard fires it once, while `a`'s firing is in flight. With
/// `waits` false, `a` does not wait, for a sequential run, which fires
/// `Tick` in other steps.
fn busy_net(kind: Kind, waits: bool) -> (Net, Arc<Handed>, Arc<AtomicUsize>) {
    let ticks = Arc::new(Count::default());
    let handed = Arc::new(Handed::default());
    let watched = Arc::new(AtomicUsize::new(0));
    let waiting = Arc::clone(&ticks);
    let a = worker(1, move |_| {
        if waits {
            waiting.wait_for(3);
        }
    });
    let b_net = worker(0, |_| {});

    let mut b = NetBuilder::new();
    let units = b.place_nested::<Unit>("Units");
    let once = b.place::<()>("Once");
    let clock = b.place::<u32>("Clock");
    let looks = b.place::<()>("Looks");
    b.keys(units, ["name"], |unit: &Unit| {
        [KeyValue::from(unit.name.as_str())]
    });
    b.seed(
        units,
        [
            Unit {
                name: "a".to_owned(),
                net: a,
            },
            Unit {
                name: "b".to_owned(),
                net: b_net,
            },
        ],
    );
    b.stepping(units, Stepping::TakeTurns);
    b.seed(once, [(), ()]);
    b.seed(clock, 0..3);
    b.seed(looks, [()]);
    b.transition("Tick", (TakeEdge(clock),), (), move |_: u32| ticks.add());

    // Once `a` has launched its work, nothing in `Units` has a job left.
    let launched = move |net: &Net| {
        net.tokens(units.id()).all(|(_, value)| {
            let unit = value.downcast_ref::<Unit>().expect("a unit");
            count(&unit.net, "Todo") == 0
        })
    };
    let just_a = || Filter::new().key_in("name", ["a"]);
    let mine = Arc::clone(&handed);
    let mut under_test = match kind {
        Kind::Take => b.transition(
            "Under test",
            (TakeEdge(once), TakeEdge(units)),
            (),
            move |(): (), unit: Unit| {
                mine.hand(&unit);
                mine.fired.fetch_add(1, Ordering::SeqCst);
            },
        ),
        Kind::TakeWhere => b.transition(
            "Under test",
            (TakeEdge(once), TakeWhereEdge(units, just_a())),
            (),
            move |(): (), unit: Unit| {
                mine.hand(&unit);
                mine.fired.fetch_add(1, Ordering::SeqCst);
            },
        ),
        Kind::TakeAll | Kind::Drain => {
            let body = move |(): (), every: Vec<Unit>| {
                every.iter().for_each(|unit| mine.hand(unit));
                mine.fired.fetch_add(1, Ordering::SeqCst);
            };
            if kind == Kind::TakeAll {
                b.transition("Under test", (TakeEdge(once), TakeAllEdge(units)), (), body)
            } else {
                b.transition("Under test", (TakeEdge(once), DrainEdge(units)), (), body)
            }
        }
        Kind::Mutate => b.transition(
            "Under test",
            (TakeEdge(once), MutateEdge(units)),
            (),
            move |(): (), unit: &mut Unit| {
                mine.hand(unit);
                mine.fired.fetch_add(1, Ordering::SeqCst);
            },
        ),
        Kind::MutateWhere => b.transition(
            "Under test",
            (TakeEdge(once), MutateWhereEdge(units, just_a())),
            (),
            move |(): (), unit: &mut Unit| {
                mine.hand(unit);
                mine.fired.fetch_add(1, Ordering::SeqCst);
            },
        ),
        Kind::Read => b.transition(
            "Under test",
            (TakeEdge(once), ReadEdge(units)),
            (),
            move |(): (), unit: &Unit| {
                mine.hand(unit);
                mine.fired.fetch_add(1, Ordering::SeqCst);
            },
        ),
        Kind::ReadWhere => b.transition(
            "Under test",
            (TakeEdge(once), ReadWhereEdge(units, just_a())),
            (),
            move |(): (), unit: &Unit| {
                mine.hand(unit);
                mine.fired.fetch_add(1, Ordering::SeqCst);
            },
        ),
        Kind::ReadAll => b.transition(
            "Under test",
            (TakeEdge(once), ReadAllEdge(units)),
            (),
            move |(): (), every: Vec<&Unit>| {
                every.iter().for_each(|unit| mine.hand(unit));
                mine.fired.fetch_add(1, Ordering::SeqCst);
            },
        ),
    };
    under_test.guard(launched);
    // A guard sees the net as it stands, the firing in flight and all.
    let seen = Arc::clone(&watched);
    b.transition("Watch", (TakeEdge(looks),), (), move |(): ()| {
        seen.fetch_add(1, Ordering::SeqCst);
    })
    .guard(move |net: &Net| {
        net.tokens(units.id()).any(|(_, value)| {
            let unit = value.downcast_ref::<Unit>().expect("a unit");
            unit.name == "a" && !unit.net.in_flight().is_empty()
        })
    });
    (b.build().expect("a busy net"), handed, watched)
}

#[test]
fn a_token_with_a_firing_in_flight_inside_is_passed_over_by_takes_and_mutations() {
    for kind in [Kind::Take, Kind::TakeWhere, Kind::Mutate, Kind::MutateWhere] {
        let (mut net, handed, watched) = busy_net(kind, true);
        let summary = run(&mut net, workers(4));
        assert!(summary.quiesced, "{kind:?}");
        assert_eq!(
            handed.in_flight.load(Ordering::SeqCst),
            0,
            "{kind:?}: never handed a unit with a firing in flight"
        );
        let at_rest = handed.at_rest.lock().unwrap().clone();
        if matches!(kind, Kind::TakeWhere | Kind::MutateWhere) {
            // Only `a` passes the filter: the transition waited for its
            // firing to land.
            assert_eq!(at_rest.first().map(String::as_str), Some("a"), "{kind:?}");
            // A take has nothing left once `a` is gone; a mutation keeps it.
            let fired = if kind == Kind::TakeWhere { 1 } else { 2 };
            assert_eq!(handed.fired.load(Ordering::SeqCst), fired, "{kind:?}");
        } else {
            // The front token, passing `a` over as if it were not there
            // while it was busy: `b` first. A mutating edge leaves `b`
            // where it is, so it may well have been handed `b` both times.
            assert_eq!(
                at_rest.first().map(String::as_str),
                Some("b"),
                "{kind:?}: {at_rest:?}"
            );
        }
        assert_eq!(
            watched.load(Ordering::SeqCst),
            1,
            "{kind:?}: the guard saw it"
        );
        assert!(nothing_in_flight(&net), "{kind:?}");
    }
}

#[test]
fn a_take_all_or_a_drain_waits_until_every_token_of_its_place_has_landed() {
    // `a` is busy until three `Tick`s have fired. Neither edge takes `b`
    // alone meanwhile, nor fires on nothing: each takes both once `a` has
    // landed, `a` first, as the place holds them.
    for kind in [Kind::TakeAll, Kind::Drain] {
        let (mut net, handed, watched) = busy_net(kind, true);
        let summary = run(&mut net, workers(4));
        assert!(summary.quiesced, "{kind:?}");
        assert_eq!(
            handed.in_flight.load(Ordering::SeqCst),
            0,
            "{kind:?}: never handed a unit with a firing in flight"
        );
        assert_eq!(*handed.at_rest.lock().unwrap(), ["a", "b"], "{kind:?}");
        // A take-all has nothing left to take; a drain fires once more, on
        // the place it emptied, with its second token in `Once`.
        let fired = if kind == Kind::TakeAll { 1 } else { 2 };
        assert_eq!(handed.fired.load(Ordering::SeqCst), fired, "{kind:?}");
        assert_eq!(
            watched.load(Ordering::SeqCst),
            1,
            "{kind:?}: the guard saw `a` busy"
        );
        assert!(nothing_in_flight(&net), "{kind:?}");
    }
}

#[test]
fn a_drain_whose_place_holds_only_busy_tokens_waits_for_them_rather_than_take_nothing() {
    // One unit, whose one `Work` lands only once `Tick` has fired three
    // times; the drain's guard lets it go once that work has launched.
    let ticks = Arc::new(Count::default());
    let waiting = Arc::clone(&ticks);
    let unit = worker(1, move |_| waiting.wait_for(3));
    let mut b = NetBuilder::new();
    let units = b.place_nested::<Net>("Units");
    let once = b.place::<()>("Once");
    let clock = b.place::<u32>("Clock");
    b.seed(units, [unit]);
    b.stepping(units, Stepping::TakeTurns);
    b.seed(once, [()]);
    b.seed(clock, 0..3);
    b.transition("Tick", (TakeEdge(clock),), (), move |_: u32| ticks.add());
    let drained = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&drained);
    b.transition(
        "Drain",
        (TakeEdge(once), DrainEdge(units)),
        (),
        move |(): (), nets: Vec<Net>| {
            let done: Vec<usize> = nets.iter().map(|net| count(net, "Done")).collect();
            seen.lock().unwrap().push(done);
        },
    )
    .guard(move |net: &Net| {
        net.tokens(units.id()).all(|(_, value)| {
            let unit = value.downcast_ref::<Net>().expect("a net");
            count(unit, "Todo") == 0
        })
    });
    let mut net = b.build().expect("a drained net");

    let summary = run(&mut net, workers(4));
    assert!(summary.quiesced);
    // One firing, handed the unit with its work done, never an empty batch.
    assert_eq!(*drained.lock().unwrap(), [vec![1]]);
    assert_eq!(net.count(units.id()), 0);
    assert!(nothing_in_flight(&net));
}

#[test]
fn a_token_with_a_firing_in_flight_inside_is_read_as_it_stands() {
    for kind in [Kind::Read, Kind::ReadWhere, Kind::ReadAll] {
        assert!(kind.reads());
        let (mut net, handed, watched) = busy_net(kind, true);
        let summary = run(&mut net, workers(4));
        assert!(summary.quiesced, "{kind:?}");
        assert!(
            handed.in_flight.load(Ordering::SeqCst) >= 1,
            "{kind:?}: a reader saw `a` with its firing in flight"
        );
        assert_eq!(watched.load(Ordering::SeqCst), 1, "{kind:?}");
        assert!(nothing_in_flight(&net), "{kind:?}");
    }
}

#[test]
fn a_transition_that_could_only_use_busy_tokens_waits_and_the_others_go_on() {
    // `Take` with `a` busy takes `b`, the token behind it; `Take where a`
    // waits for `a`. Both end with every unit taken once it was at rest.
    let (mut net, handed, _) = busy_net(Kind::Take, true);
    run(&mut net, workers(4));
    assert_eq!(*handed.at_rest.lock().unwrap(), ["b", "a"]);
    let (mut net, handed, _) = busy_net(Kind::TakeWhere, true);
    run(&mut net, workers(4));
    assert_eq!(*handed.at_rest.lock().unwrap(), ["a"]);
    assert_eq!(handed.fired.load(Ordering::SeqCst), 1);
}

#[test]
fn the_sequential_driver_chooses_as_it_always_did() {
    // Nothing is ever in flight between two sequential steps, so the same
    // net hands `Take` the front unit, `a`, once it has done its work.
    let (mut net, handed, watched) = busy_net(Kind::Take, false);
    let summary = run(&mut net, Mode::Sequential);
    assert!(summary.quiesced);
    assert_eq!(*handed.at_rest.lock().unwrap(), ["a", "b"]);
    assert_eq!(
        watched.load(Ordering::SeqCst),
        0,
        "never in flight to watch"
    );
}

// ----- a take-all or a drain holds its place ------------------------------------

/// Two legs that hand a baton to each other, so that, while both may
/// launch, one of them always has a body in flight: the `k`th body of leg 0
/// waits until leg 1 has started `k` times, and the `k`th of leg 1 until
/// leg 0 has started `k + 1` times. A body waits at most its patience, or
/// until the relay is released; only a leg that may not launch, because
/// the driver holds its net still or has stopped launching, makes the
/// other wait that long.
struct Relay {
    legs: Mutex<Legs>,
    changed: Condvar,
    patience: Duration,
    /// Released once this many bodies have landed, so that a run that its
    /// limit ends does not leave a leg waiting out its patience.
    release_at: Option<usize>,
}

#[derive(Default)]
struct Legs {
    started: [usize; 2],
    landed: usize,
    /// Bodies that waited out their patience.
    waited_out: usize,
    /// How many bodies had landed when the first had waited [`STALL`].
    stalled_at: Option<usize>,
    released: bool,
}

/// How long a leg waits before it counts as stalled: far longer than the
/// other leg takes to start while it may.
const STALL: Duration = Duration::from_millis(10);

impl Relay {
    fn new(patience: Duration) -> Arc<Relay> {
        Relay::released_at(patience, None)
    }

    fn released_at(patience: Duration, release_at: Option<usize>) -> Arc<Relay> {
        Arc::new(Relay {
            legs: Mutex::default(),
            changed: Condvar::new(),
            patience,
            release_at,
        })
    }

    /// One body of leg `side`, waiting for the other leg when `waits`:
    /// true when it waited out its patience.
    fn leg(&self, side: usize, waits: bool) -> bool {
        let mut legs = self.legs.lock().unwrap();
        legs.started[side] += 1;
        let needed = legs.started[side] + side;
        self.changed.notify_all();
        let since = Instant::now();
        let deadline = since + self.patience;
        let mut waited_out = false;
        let released =
            |legs: &Legs| legs.released || self.release_at.is_some_and(|at| legs.landed >= at);
        while waits && !released(&legs) && legs.started[1 - side] < needed {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                waited_out = true;
                legs.waited_out += 1;
                break;
            }
            legs = self.changed.wait_timeout(legs, left.min(STALL)).unwrap().0;
            if since.elapsed() >= STALL && legs.stalled_at.is_none() {
                legs.stalled_at = Some(legs.landed);
                self.changed.notify_all();
            }
        }
        legs.landed += 1;
        self.changed.notify_all();
        waited_out
    }

    /// Lets every body go at once, from now on.
    fn release(&self) {
        self.legs.lock().unwrap().released = true;
        self.changed.notify_all();
    }

    /// Waits until `at_least` bodies have landed, or the deadline.
    fn wait_landed(&self, at_least: usize) {
        let deadline = Instant::now() + DEADLINE;
        let mut legs = self.legs.lock().unwrap();
        while legs.landed < at_least {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            legs = self.changed.wait_timeout(legs, left).unwrap().0;
        }
    }

    fn landed(&self) -> usize {
        self.legs.lock().unwrap().landed
    }

    fn waited_out(&self) -> usize {
        self.legs.lock().unwrap().waited_out
    }

    fn stalled_at(&self) -> Option<usize> {
        self.legs.lock().unwrap().stalled_at
    }
}

/// A net that never comes to rest: for each of `sides`, `Run <side>` takes
/// its lap and puts it back, each body a leg of `relay` on that side,
/// waiting for the other leg when `waits`. With `fails`, a body that waited
/// out its patience fails.
fn legs(relay: &Arc<Relay>, sides: &[usize], waits: bool, fails: bool) -> Net {
    let mut b = NetBuilder::new();
    for &side in sides {
        let lap = b.place::<u32>(format!("Lap {side}"));
        b.seed(lap, [0]);
        let relay = Arc::clone(relay);
        b.transition(
            format!("Run {side}"),
            (TakeEdge(lap),),
            (lap,),
            move |n: u32| -> Result<(u32,), Broken> {
                if relay.leg(side, waits) && fails {
                    Err(Broken)
                } else {
                    Ok((n + 1,))
                }
            },
        );
    }
    b.build().expect("a runner")
}

/// The two shapes of nets that never rest: two nets of one leg each, or
/// one net whose two legs are in flight at once.
fn runners(relay: &Arc<Relay>, two_nets: bool, waits: bool, fails: bool) -> Vec<Net> {
    if two_nets {
        vec![
            legs(relay, &[0], waits, fails),
            legs(relay, &[1], waits, fails),
        ]
    } else {
        vec![legs(relay, &[0, 1], waits, fails)]
    }
}

/// For each firing of a drain: how many nets it took, and how many of them
/// had a firing in flight.
type Drained = Arc<Mutex<Vec<(usize, usize)>>>;

/// A net under construction whose `Field`, under `stepping`, holds `nets`,
/// and whose `Drain` takes every one of them, its one token in `Once`, once
/// `guard` lets it.
struct Meet {
    b: NetBuilder,
    field: PlaceId,
    once: PlaceRef<()>,
    drained: Drained,
}

fn meet(
    stepping: Stepping,
    nets: Vec<Net>,
    guard: impl Fn(&Net) -> bool + Send + Sync + 'static,
) -> Meet {
    let mut b = NetBuilder::new();
    let field = b.place_nested::<Net>("Field");
    let once = b.place::<()>("Once");
    b.seed(field, nets);
    b.stepping(field, stepping);
    b.seed(once, [()]);
    let drained = Drained::default();
    let seen = Arc::clone(&drained);
    b.transition(
        "Drain",
        (TakeEdge(once), DrainEdge(field)),
        (),
        move |(): (), nets: Vec<Net>| {
            let busy = nets.iter().filter(|net| !net.in_flight().is_empty());
            seen.lock().unwrap().push((nets.len(), busy.count()));
        },
    )
    .guard(guard);
    Meet {
        b,
        field: field.id(),
        once,
        drained,
    }
}

/// A guard that lets a drain go once `relay` has landed six bodies.
fn after_six(relay: &Arc<Relay>) -> impl Fn(&Net) -> bool + Send + Sync + 'static {
    let relay = Arc::clone(relay);
    move |_: &Net| relay.landed() >= 6
}

#[test]
fn a_drain_on_nets_that_never_rest_holds_their_place_and_takes_it_whole() {
    // Until the hold, whichever leg had its body in flight kept the drain
    // waiting at every look, and the run went on until its limit.
    let limits = Limits {
        stop_after_n_firings: Some(2_000),
        ..Limits::default()
    };
    for stepping in [Stepping::TakeTurns, Stepping::OuterFirst] {
        for two_nets in [true, false] {
            for mode in [Mode::Sequential, workers(4), Mode::Concurrent] {
                // A sequential step runs one body at a time: no leg waits.
                let waits = mode != Mode::Sequential;
                let relay = Relay::new(Duration::from_millis(50));
                let nets = runners(&relay, two_nets, waits, false);
                let whole = nets.len();
                let meet = meet(stepping, nets, after_six(&relay));
                let mut net = meet.b.build().expect("a meet");
                let case = format!("{stepping:?}, two nets {two_nets}, {mode:?}");

                let summary =
                    drivers::run(&mut net, mode, limits, OnFailure::Stop).expect("nothing fails");

                assert!(summary.quiesced, "{case}: the drain ended the run");
                assert_eq!(
                    *meet.drained.lock().unwrap(),
                    [(whole, 0)],
                    "{case}: one firing took the whole place, every net at rest"
                );
                assert_eq!(net.count(meet.field), 0, "{case}");
                assert!(nothing_in_flight(&net), "{case}");
            }
        }
    }

    // And on a clock: the hold is kept from tick to tick.
    let relay = Relay::new(Duration::from_millis(50));
    let meet = meet(
        Stepping::TakeTurns,
        runners(&relay, true, true, false),
        after_six(&relay),
    );
    let mut net = meet.b.build().expect("a meet");
    let stop = AtomicBool::new(false);
    let started = Instant::now();
    let drained = Arc::clone(&meet.drained);
    drivers::run_indefinitely(
        &mut net,
        workers(4),
        Duration::from_millis(1),
        &stop,
        OnFailure::Stop,
        |_| {
            if !drained.lock().unwrap().is_empty() || started.elapsed() > DEADLINE {
                stop.store(true, Ordering::Release);
            }
        },
    )
    .expect("nothing fails");
    assert_eq!(*meet.drained.lock().unwrap(), [(2, 0)]);
    assert!(nothing_in_flight(&net));
}

#[test]
fn a_drain_on_a_port_of_a_subnet_holds_the_nets_there() {
    // The runners are in the subnet's port, stepped by the subnet's round;
    // the drain of the net around reaches them through its window.
    let limits = Limits {
        stop_after_n_firings: Some(2_000),
        ..Limits::default()
    };
    for mode in [workers(4), Mode::Concurrent] {
        let relay = Relay::new(Duration::from_millis(50));
        let mut s = NetBuilder::new();
        let field = s.place_nested::<Net>("Field");
        s.port(field);
        s.seed(field, runners(&relay, true, true, false));
        s.stepping(field, Stepping::TakeTurns);
        let stadium = s.build().expect("a stadium");

        let mut b = NetBuilder::new();
        let stadium = b.subnet_place("Stadium", stadium);
        b.stepping(stadium, Stepping::TakeTurns);
        let gate = b.port_of::<Net>(stadium, "Field");
        let once = b.place::<()>("Once");
        b.seed(once, [()]);
        let drained = Drained::default();
        let seen = Arc::clone(&drained);
        b.transition(
            "Drain",
            (TakeEdge(once), DrainEdge(gate)),
            (),
            move |(): (), nets: Vec<Net>| {
                let busy = nets.iter().filter(|net| !net.in_flight().is_empty());
                seen.lock().unwrap().push((nets.len(), busy.count()));
            },
        )
        .guard(after_six(&relay));
        let mut net = b.build().expect("a stadium in a net");

        let summary = drivers::run(&mut net, mode, limits, OnFailure::Stop).expect("no failure");

        assert!(summary.quiesced, "{mode:?}: the drain ended the run");
        assert_eq!(*drained.lock().unwrap(), [(2, 0)], "{mode:?}");
        assert_eq!(net.count(gate.id()), 0, "{mode:?}");
        assert!(nothing_in_flight(&net), "{mode:?}");
    }
}

#[test]
fn a_drain_that_could_not_fire_anyway_holds_nothing_and_a_hold_lapses_when_it_cannot() {
    let limits = Limits {
        stop_after_n_firings: Some(200),
        ..Limits::default()
    };
    // Three bodies at once at most: two legs and `Tick`.
    for mode in [workers(4), workers(8)] {
        // A guard that is never true holds nothing: no leg waits out a
        // patience of the whole deadline, which only a hold would make it
        // do before the relay is released near the limit.
        for stepping in [Stepping::TakeTurns, Stepping::OuterFirst] {
            let relay = Relay::released_at(DEADLINE, Some(190));
            let meet = meet(stepping, runners(&relay, true, true, false), |_| false);
            let mut net = meet.b.build().expect("a meet");
            let summary =
                drivers::run(&mut net, mode, limits, OnFailure::Stop).expect("no failure");
            assert!(
                !summary.quiesced,
                "{mode:?}, {stepping:?}: the runners never rest"
            );
            assert_eq!(summary.fired_count, 200, "{mode:?}, {stepping:?}");
            assert_eq!(relay.waited_out(), 0, "{mode:?}, {stepping:?}: never held");
            assert!(meet.drained.lock().unwrap().is_empty());
        }

        // A drain that waits holds its place, and the leg in flight stalls,
        // its partner held. Then, while it waits, its guard turns false, or
        // `Claim` takes its other input: the hold lapses, the partner
        // launches, and the stalled leg lands long before its patience of
        // the whole deadline. `Tick`, of the net around, keeps the driver
        // looking meanwhile.
        for claimed in [false, true] {
            let relay = Relay::released_at(DEADLINE, Some(150));
            let watched = Arc::clone(&relay);
            let guard =
                move |_: &Net| watched.landed() >= 6 && (claimed || watched.stalled_at().is_none());
            let mut meet = meet(
                Stepping::TakeTurns,
                runners(&relay, true, true, false),
                guard,
            );
            if claimed {
                let watched = Arc::clone(&relay);
                meet.b
                    .transition("Claim", (TakeEdge(meet.once),), (), |(): ()| {})
                    .guard(move |_: &Net| watched.stalled_at().is_some());
            }
            let clock = meet.b.place::<u32>("Clock");
            meet.b.seed(clock, [0]);
            meet.b
                .transition("Tick", (TakeEdge(clock),), (clock,), |n: u32| {
                    thread::sleep(Duration::from_millis(1));
                    (n + 1,)
                });
            let mut net = meet.b.build().expect("a meet");
            let limits = Limits {
                stop_after_n_firings: Some(2_000),
                ..Limits::default()
            };
            let summary =
                drivers::run(&mut net, mode, limits, OnFailure::Stop).expect("no failure");
            let case = format!("{mode:?}, claimed {claimed}");
            assert!(!summary.quiesced, "{case}");
            assert!(
                meet.drained.lock().unwrap().is_empty(),
                "{case}: never fired"
            );
            let held_at = relay.stalled_at().expect("a leg was held");
            assert_eq!(
                relay.waited_out(),
                0,
                "{case}: the hold lapsed while it waited"
            );
            assert!(
                relay.landed() >= held_at + 100,
                "{case}: the runners went on after the hold lapsed: {} after {held_at}",
                relay.landed()
            );
        }
    }
}

#[test]
fn a_hold_keeps_back_only_the_nets_of_its_place() {
    // Five bodies wait at once at most: four legs and the desk's job.
    for mode in [workers(6), workers(16)] {
        // Two fields of runners, each with a drain, and a desk whose worker
        // waits until both drains may go; its last job releases both
        // relays, which a held leg waits for, its patience the whole
        // deadline. So both drains fire only if the desk's worker went on
        // while both fields were held.
        let relays = [Relay::new(DEADLINE), Relay::new(DEADLINE)];
        let mut b = NetBuilder::new();
        let mut drained = Vec::new();
        for (i, relay) in relays.iter().enumerate() {
            let field = b.place_nested::<Net>(format!("Field {i}"));
            let once = b.place::<()>(format!("Once {i}"));
            b.seed(field, runners(relay, true, true, false));
            b.stepping(field, Stepping::TakeTurns);
            b.seed(once, [()]);
            let seen = Drained::default();
            let mine = Arc::clone(&seen);
            b.transition(
                format!("Drain {i}"),
                (TakeEdge(once), DrainEdge(field)),
                (),
                move |(): (), nets: Vec<Net>| {
                    let busy = nets.iter().filter(|net| !net.in_flight().is_empty());
                    mine.lock().unwrap().push((nets.len(), busy.count()));
                },
            )
            .guard(after_six(relay));
            drained.push(seen);
        }
        let desk = b.place_nested::<Net>("Desk");
        let gate = relays.clone();
        b.seed(
            desk,
            [worker(3, move |job| {
                gate.iter().for_each(|relay| relay.wait_landed(6));
                if job == 2 {
                    gate.iter().for_each(|relay| relay.release());
                }
            })],
        );
        b.stepping(desk, Stepping::TakeTurns);
        let mut net = b.build().expect("two fields and a desk");

        let summary = run(&mut net, mode);

        assert!(summary.quiesced, "{mode:?}");
        for (i, relay) in relays.iter().enumerate() {
            assert_eq!(*drained[i].lock().unwrap(), [(2, 0)], "{mode:?}, field {i}");
            assert_eq!(
                relay.waited_out(),
                0,
                "{mode:?}, field {i}: released, not waited out"
            );
        }
        assert_eq!(count(nets_in(&net, desk.id())[0], "Done"), 3, "{mode:?}");
    }
}

#[test]
fn others_read_and_take_from_the_place_a_drain_holds() {
    for mode in [workers(4), Mode::Concurrent] {
        // `a`'s one job lands only once the reader and the taker have both
        // had the place, which their guards give them only while `a` is
        // busy; the drain waits for `a` meanwhile, holding the place.
        let others = Arc::new(Count::default());
        let waits = Arc::clone(&others);
        let a = worker(1, move |_| waits.wait_for(2));
        // `b`'s one job lands at once, and so passes again while `a` is busy.
        let mut b = NetBuilder::new();
        let units = b.place_nested::<Unit>("Units");
        let (once, look, ticket) = (
            b.place::<()>("Once"),
            b.place::<()>("Look"),
            b.place::<()>("Ticket"),
        );
        b.seed(
            units,
            [
                Unit {
                    name: "a".to_owned(),
                    net: a,
                },
                Unit {
                    name: "b".to_owned(),
                    net: worker(1, |_| {}),
                },
            ],
        );
        b.stepping(units, Stepping::TakeTurns);
        for place in [once, look, ticket] {
            b.seed(place, [()]);
        }
        let a_busy = move |net: &Net| {
            net.tokens(units.id()).any(|(_, value)| {
                let unit = value.downcast_ref::<Unit>().expect("a unit");
                unit.name == "a" && !unit.net.in_flight().is_empty()
            })
        };
        let seen = |unit: &Unit| (unit.name.clone(), !unit.net.in_flight().is_empty());
        let log = Arc::new(Mutex::new(Vec::new()));

        let (mine, told) = (Arc::clone(&log), Arc::clone(&others));
        b.transition(
            "Read",
            (TakeEdge(look), ReadAllEdge(units)),
            (),
            move |(): (), every: Vec<&Unit>| {
                let every: Vec<_> = every.into_iter().map(seen).collect();
                mine.lock().unwrap().push(("Read", every));
                told.add();
            },
        )
        .guard(a_busy);
        let (mine, told) = (Arc::clone(&log), Arc::clone(&others));
        b.transition(
            "Take",
            (TakeEdge(ticket), TakeEdge(units)),
            (),
            move |(): (), unit: Unit| {
                mine.lock().unwrap().push(("Take", vec![seen(&unit)]));
                told.add();
            },
        )
        .guard(a_busy);
        let mine = Arc::clone(&log);
        b.transition(
            "Drain",
            (TakeEdge(once), DrainEdge(units)),
            (),
            move |(): (), every: Vec<Unit>| {
                let every: Vec<_> = every.iter().map(seen).collect();
                mine.lock().unwrap().push(("Drain", every));
            },
        )
        .guard(move |net: &Net| {
            net.tokens(units.id()).all(|(_, value)| {
                let unit = value.downcast_ref::<Unit>().expect("a unit");
                count(&unit.net, "Todo") == 0
            })
        });
        let mut net = b.build().expect("a place read, taken and drained");
        let started = Instant::now();

        let summary = run(&mut net, mode);

        assert!(summary.quiesced, "{mode:?}");
        assert!(
            started.elapsed() < DEADLINE,
            "{mode:?}: `a` did not wait it out"
        );
        let mut log = log.lock().unwrap().clone();
        let drain = log.pop().expect("the drain fired last");
        log.sort();
        let (a, b) = (("a".to_owned(), true), ("b".to_owned(), false));
        assert_eq!(
            log,
            [("Read", vec![a, b.clone()]), ("Take", vec![b])],
            "{mode:?}: read as they stood, and the one at rest taken"
        );
        assert_eq!(drain, ("Drain", vec![("a".to_owned(), false)]), "{mode:?}");
        assert!(nothing_in_flight(&net), "{mode:?}");
    }
}

#[test]
fn a_selector_that_declines_a_waiting_drain_lets_the_held_nets_go_on() {
    // The hold lasts until the drain has been offered: declined, it lapses
    // and the nets go on until the drain waits again. Each offer finds more
    // bodies landed than the one before it.
    let declining = |never: bool, limits: Limits, mode: Mode| {
        let relay = Relay::new(Duration::from_millis(20));
        let meet = meet(
            Stepping::TakeTurns,
            runners(&relay, true, true, false),
            after_six(&relay),
        );
        let mut net = meet.b.build().expect("a meet");
        let drain = net.transition_named("Drain").expect("Drain");
        let offers = Arc::new(Mutex::new(Vec::new()));
        let (seen, watched) = (Arc::clone(&offers), Arc::clone(&relay));
        net.set_selector(move |_, offered| {
            if !offered.contains(&drain) {
                return offered.first().copied();
            }
            let mut offers = seen.lock().unwrap();
            offers.push(watched.landed());
            (!never && offers.len() > 3).then_some(drain)
        });
        let summary = drivers::run(&mut net, mode, limits, OnFailure::Stop).expect("no failure");
        let offers = offers.lock().unwrap().clone();
        assert!(
            offers.windows(2).all(|two| two[0] < two[1]),
            "{mode:?}: the nets went on between the offers: {offers:?}"
        );
        assert!(nothing_in_flight(&net), "{mode:?}");
        let drained = meet.drained.lock().unwrap().clone();
        (summary, offers.len(), drained)
    };
    for mode in [workers(4), Mode::Concurrent] {
        // Declined three times, then chosen.
        let (summary, offers, drained) = declining(false, Limits::default(), mode);
        assert!(summary.quiesced, "{mode:?}");
        assert_eq!(offers, 4, "{mode:?}");
        assert_eq!(drained, [(2, 0)], "{mode:?}");

        // Never chosen: nothing is held for good, and the run ends on its
        // limit with the runners still running.
        let limits = Limits {
            stop_after_n_firings: Some(30),
            ..Limits::default()
        };
        let (summary, offers, drained) = declining(true, limits, mode);
        assert!(!summary.quiesced, "{mode:?}");
        assert_eq!(summary.fired_count, 30, "{mode:?}");
        assert!(offers >= 3, "{mode:?}: offered {offers} times");
        assert!(drained.is_empty(), "{mode:?}");
    }
}

#[test]
fn the_limits_and_the_failures_stop_a_run_while_a_drain_holds_its_place() {
    for mode in [workers(4), Mode::Concurrent] {
        let build = |fails: bool| {
            let relay = Relay::new(Duration::from_millis(50));
            let meet = meet(
                Stepping::TakeTurns,
                runners(&relay, true, true, fails),
                after_six(&relay),
            );
            let net = meet.b.build().expect("a meet");
            (net, meet.drained, meet.field, relay)
        };

        // Paced: the call stops launching with the place held, lands what
        // is in flight, and the next call goes on to the drain.
        let (mut net, drained, _, relay) = build(false);
        let paced = Limits {
            stop_after_n_firings: Some(7),
            ..Limits::default()
        };
        let first = drivers::run(&mut net, mode, paced, OnFailure::Stop).expect("paced");
        assert!(!first.quiesced, "{mode:?}");
        assert_eq!(first.fired_count, 7, "{mode:?}");
        assert!(relay.landed() >= 6, "{mode:?}");
        assert!(nothing_in_flight(&net), "{mode:?}");
        let rest = run(&mut net, mode);
        assert!(rest.quiesced, "{mode:?}");
        assert_eq!(*drained.lock().unwrap(), [(2, 0)], "{mode:?}");

        // The fuse trips before the launch it would make.
        let (mut net, _, _, _) = build(false);
        let fused = Limits {
            error_after_n_firings: Some(7),
            ..Limits::default()
        };
        match drivers::run(&mut net, mode, fused, OnFailure::Stop) {
            Err(DriverError::TooManyFirings { summary, .. }) => {
                assert_eq!(summary.fired_count, 7, "{mode:?}");
            }
            other => panic!("{mode:?}: the fuse, not {other:?}"),
        }
        assert!(nothing_in_flight(&net), "{mode:?}");

        // A held leg that waits out its patience fails. Under `Stop` the
        // run ends with it, the drain unfired; under `Disable` that leg is
        // switched off, its net comes to rest, and the drain takes both.
        let (mut net, drained, field, _) = build(true);
        match drivers::run(&mut net, mode, Limits::default(), OnFailure::Stop) {
            Err(DriverError::FiringFailed { error, .. }) => {
                assert!(
                    matches!(&error.path[..], [PathStep::Token { place, .. }] if *place == field),
                    "{mode:?}: {:?}",
                    error.path
                );
            }
            other => panic!("{mode:?}: the held leg's failure, not {other:?}"),
        }
        assert!(drained.lock().unwrap().is_empty(), "{mode:?}");
        assert!(nothing_in_flight(&net), "{mode:?}");

        let (mut net, drained, _, _) = build(true);
        let summary = drivers::run(&mut net, mode, Limits::default(), OnFailure::Disable)
            .expect("Disable goes on");
        assert!(summary.quiesced, "{mode:?}");
        assert!(summary.failed_count >= 1, "{mode:?}");
        assert_eq!(*drained.lock().unwrap(), [(2, 0)], "{mode:?}");
    }
}

// ----- the stepping rules ---------------------------------------------------------

/// An outer net with `inbox` jobs of its own (`Inbox -Forward-> Outbox`)
/// and a `Desk` under `stepping` with `workers` workers of `jobs` each.
fn office(stepping: Stepping, inbox: u32, workers: usize, jobs: u32) -> (Net, PlaceId) {
    let mut b = NetBuilder::new();
    let desk = b.place_nested::<Net>("Desk");
    let tray = b.place::<u32>("Inbox");
    let outbox = b.place::<u32>("Outbox");
    b.seed(desk, (0..workers).map(|_| worker(jobs, |_| {})));
    b.seed(tray, 0..inbox);
    b.transition("Forward", (TakeEdge(tray),), (outbox,), |n: u32| (n,));
    b.stepping(desk, stepping);
    (b.build().expect("an office"), desk.id())
}

/// Where each firing that landed in a run was, in log order: `O` for the
/// outer net, `I` for a net inside; and where each launch was, `o` and `i`.
fn story(events: &[Event]) -> String {
    unwrapped(events)
        .into_iter()
        .filter_map(|(depth, event)| match (depth, event) {
            (0, Event::Distributed { .. } | Event::Fired { .. }) => Some('O'),
            (0, Event::Launched { .. }) => Some('o'),
            (_, Event::Distributed { .. } | Event::Fired { .. }) => Some('I'),
            (_, Event::Launched { .. }) => Some('i'),
            _ => None,
        })
        .collect()
}

#[test]
fn inner_first_lands_everything_inside_before_the_net_around_it_launches() {
    for mode in [Mode::Sequential, workers(1), workers(4), Mode::Concurrent] {
        let (mut net, desk) = office(Stepping::InnerFirst, 3, 3, 2);
        let summary = run(&mut net, mode);
        assert_eq!(summary.fired_count, 9, "{mode:?}");
        let events: Vec<Event> = net.drain_events().collect();
        let story = story(&events);
        let first_outer = story.find(['o', 'O']).expect("the outer net fired");
        assert_eq!(
            story[..first_outer].matches('I').count(),
            6,
            "{mode:?}: every inner firing landed before the outer net's first: {story}"
        );
        assert!(
            nets_in(&net, desk)
                .iter()
                .all(|inner| count(inner, "Todo") == 0)
        );
    }
}

#[test]
fn outer_first_waits_for_the_net_around_it_to_rest() {
    for mode in [Mode::Sequential, workers(1), workers(4), Mode::Concurrent] {
        let (mut net, _) = office(Stepping::OuterFirst, 3, 3, 2);
        let summary = run(&mut net, mode);
        assert_eq!(summary.fired_count, 9, "{mode:?}");
        let events: Vec<Event> = net.drain_events().collect();
        let story = story(&events);
        let first_inner = story.find(['i', 'I']).expect("the inner nets fired");
        assert_eq!(
            story[..first_inner].matches('O').count(),
            3,
            "{mode:?}: every outer firing landed before the first inside: {story}"
        );
    }
}

#[test]
fn an_inner_first_net_that_never_rests_stops_the_net_around_it_in_both_modes() {
    for mode in [Mode::Sequential, workers(2)] {
        let mut b = NetBuilder::new();
        let desk = b.place_nested::<Net>("Desk");
        let inbox = b.place::<u32>("Inbox");
        let outbox = b.place::<u32>("Outbox");
        b.seed(desk, [spinner()]);
        b.seed(inbox, [1]);
        b.transition("Forward", (TakeEdge(inbox),), (outbox,), |n: u32| (n,));
        let mut net = b.build().expect("a spinner inside");
        let limits = Limits {
            stop_after_n_firings: Some(40),
            ..Limits::default()
        };
        let summary = drivers::run(&mut net, mode, limits, OnFailure::Stop).expect("no failure");
        assert!(!summary.quiesced);
        assert_eq!(net.fired_count(), 0, "{mode:?}: the outer net never fired");
        assert_eq!(summary.fired_count, 40, "{mode:?}");
    }
}

#[test]
fn take_turns_starves_no_one_in_either_direction() {
    for mode in [Mode::Sequential, workers(1), workers(3), Mode::Concurrent] {
        let limits = Limits {
            stop_after_n_firings: Some(300),
            ..Limits::default()
        };
        // An outer net that can always fire, beside three take-turns
        // workers with work: the workers finish.
        let mut b = NetBuilder::new();
        let wheel = b.place::<u32>("Wheel");
        let desk = b.place_nested::<Net>("Desk");
        b.seed(wheel, [0]);
        b.seed(desk, (0..3).map(|_| worker(4, |_| {})));
        b.transition("Spin", (TakeEdge(wheel),), (wheel,), |n: u32| (n + 1,));
        b.stepping(desk, Stepping::TakeTurns);
        let mut net = b.build().expect("a busy outer net");
        let summary = drivers::run(&mut net, mode, limits, OnFailure::Stop).expect("no failure");
        assert!(!summary.quiesced, "{mode:?}: Spin never stops");
        for inner in nets_in(&net, desk.id()) {
            assert_eq!(
                count(inner, "Done"),
                4,
                "{mode:?}: every worker had its turns"
            );
        }

        // Inner nets that never rest, beside an outer net with work: the
        // outer net finishes.
        let mut b = NetBuilder::new();
        let desk = b.place_nested::<Net>("Desk");
        let inbox = b.place::<u32>("Inbox");
        let outbox = b.place::<u32>("Outbox");
        b.seed(desk, [spinner(), spinner()]);
        b.seed(inbox, 0..5);
        b.transition("Forward", (TakeEdge(inbox),), (outbox,), |n: u32| (n,));
        b.stepping(desk, Stepping::TakeTurns);
        let mut net = b.build().expect("spinners inside");
        drivers::run(&mut net, mode, limits, OnFailure::Stop).expect("no failure");
        assert_eq!(
            net.count(outbox.id()),
            5,
            "{mode:?}: the outer net had its turns"
        );
    }
}

#[test]
fn take_turns_at_two_levels_starves_no_one() {
    for mode in [Mode::Sequential, workers(2), Mode::Concurrent] {
        // The middle net can always fire too, and holds a take-turns worker.
        let mut b = NetBuilder::new();
        let wheel = b.place::<u32>("Wheel");
        let held = b.place_nested::<Net>("Held");
        b.seed(wheel, [0]);
        b.seed(held, [worker(5, |_| {})]);
        b.transition("Spin", (TakeEdge(wheel),), (wheel,), |n: u32| (n + 1,));
        b.stepping(held, Stepping::TakeTurns);
        let middle = b.build().expect("a middle net");

        let mut b = NetBuilder::new();
        let wheel = b.place::<u32>("Wheel");
        let yard = b.place_nested::<Net>("Yard");
        b.seed(wheel, [0]);
        b.seed(yard, [middle]);
        b.transition("Spin", (TakeEdge(wheel),), (wheel,), |n: u32| (n + 1,));
        b.stepping(yard, Stepping::TakeTurns);
        let mut net = b.build().expect("two levels of spinning");
        let limits = Limits {
            stop_after_n_firings: Some(300),
            ..Limits::default()
        };
        drivers::run(&mut net, mode, limits, OnFailure::Stop).expect("no failure");
        let middle = nets_in(&net, yard.id())[0];
        let bottom = nets_in(middle, middle.place_named("Held").unwrap())[0];
        assert_eq!(
            count(bottom, "Done"),
            5,
            "{mode:?}: the bottom worker finished"
        );
    }
}

#[test]
fn passive_nets_are_never_stepped() {
    for mode in [Mode::Sequential, workers(2), Mode::Concurrent] {
        let (mut net, desk) = office(Stepping::Passive, 3, 2, 2);
        let summary = run(&mut net, mode);
        assert!(summary.quiesced);
        assert_eq!(summary.fired_count, 3, "{mode:?}");
        assert!(
            nets_in(&net, desk)
                .iter()
                .all(|inner| count(inner, "Todo") == 2)
        );
    }
}

// ----- the same as the sequential deep step, on confluent nets -------------------

/// Each firing a body made, as `"net/transition/job"`.
type Log = Arc<Mutex<Vec<String>>>;

/// A worker named `name` whose bodies log what they do.
fn logged(name: &str, jobs: u32, log: &Log) -> Net {
    let (name, log) = (name.to_owned(), Arc::clone(log));
    worker(jobs, move |n| {
        // A body that yields lets the others run in between.
        thread::yield_now();
        log.lock().unwrap().push(format!("{name}/Work/{n}"));
    })
}

/// A worker with an inner-first `Tools` place holding a logged tool.
fn equipped(name: &str, jobs: u32, tool_jobs: u32, log: &Log) -> Net {
    let mut b = NetBuilder::new();
    let todo = b.place::<u32>("Todo");
    let done = b.place::<u32>("Done");
    let tools = b.place_nested::<Net>("Tools");
    b.seed(todo, 0..jobs);
    b.seed(tools, [logged(&format!("{name}'s tool"), tool_jobs, log)]);
    let (name, log) = (name.to_owned(), Arc::clone(log));
    b.transition("Work", (TakeEdge(todo),), (done,), move |n: u32| {
        thread::yield_now();
        log.lock().unwrap().push(format!("{name}/Work/{n}"));
        (n,)
    });
    b.build().expect("an equipped worker")
}

/// A mill, for a subnet place: `In -Grind-> Out`, both ports, each job
/// ground into ten times itself.
fn mill(name: &str, log: &Log) -> Net {
    let mut b = NetBuilder::new();
    let input = b.place::<u32>("In");
    let output = b.place::<u32>("Out");
    b.port(input);
    b.port(output);
    let (name, log) = (name.to_owned(), Arc::clone(log));
    b.transition("Grind", (TakeEdge(input),), (output,), move |n: u32| {
        thread::yield_now();
        log.lock().unwrap().push(format!("{name}/Grind/{n}"));
        (n * 10,)
    });
    b.build().expect("a mill")
}

/// A confluent net three levels deep: eight jobs fed through the ports of
/// two mills (take turns, outer first), even ones to one and odd to the
/// other, and collected; a take-turns crew of three equipped workers, two
/// inner-first workers and two outer-first ones.
fn factory(log: &Log) -> Net {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let done = b.place::<u32>("Done");
    b.keys(inbox, ["even"], |n: &u32| {
        [KeyValue::from(n.is_multiple_of(2))]
    });
    b.seed(inbox, 0..8);
    let mills = [
        (b.subnet_place("Mill A", mill("Mill A", log)), true),
        (b.subnet_place("Mill B", mill("Mill B", log)), false),
    ];
    for (at, (mill, even)) in mills.into_iter().enumerate() {
        let input = b.port_of::<u32>(mill, "In");
        let output = b.port_of::<u32>(mill, "Out");
        let jobs = Filter::new().key_in("even", [even]);
        b.transition(
            format!("Feed {at}"),
            (TakeWhereEdge(inbox, jobs),),
            (input,),
            |n: u32| (n,),
        );
        b.transition(
            format!("Collect {at}"),
            (TakeEdge(output),),
            (done,),
            |n: u32| (n,),
        );
        b.stepping(
            mill,
            if even {
                Stepping::TakeTurns
            } else {
                Stepping::OuterFirst
            },
        );
    }
    let crew = b.place_nested::<Net>("Crew");
    b.seed(
        crew,
        (0..3).map(|i| equipped(&format!("crew {i}"), 3, 2, log)),
    );
    b.stepping(crew, Stepping::TakeTurns);
    let early = b.place_nested::<Net>("Early");
    b.seed(early, (0..2).map(|i| logged(&format!("early {i}"), 2, log)));
    let late = b.place_nested::<Net>("Late");
    b.seed(late, (0..2).map(|i| logged(&format!("late {i}"), 2, log)));
    b.stepping(late, Stepping::OuterFirst);
    b.build().expect("a factory")
}

/// The marking of `net` and of every net inside it, place by place, the
/// numbers of each place sorted, the nets inside in walk order.
fn marking(net: &Net) -> Vec<String> {
    fn describe(net: &Net, at: &str, out: &mut Vec<String>) {
        for (place, info) in net.places() {
            // A window's tokens are its subnet's, told there.
            if info.port_of.is_some() {
                continue;
            }
            let here = format!("{at}/{}", info.name);
            if info.subnet {
                describe(net.subnet(place).expect("a subnet"), &here, out);
                continue;
            }
            let mut values: Vec<String> = net
                .tokens(place)
                .map(|(_, value)| {
                    let value: &dyn Any = value;
                    value
                        .downcast_ref::<u32>()
                        .map_or_else(|| String::from("net"), u32::to_string)
                })
                .collect();
            values.sort();
            out.push(format!("{here}: {}", values.join(",")));
            for (at, (token, _)) in net.tokens(place).enumerate() {
                for (name, inner) in net.nested(place, token).unwrap_or_default() {
                    describe(inner, &format!("{here}#{at}{name}"), out);
                }
            }
        }
    }
    let mut out = Vec::new();
    describe(net, "", &mut out);
    out
}

/// What a run of a fresh factory in `mode` ends with: its marking, what
/// fired, sorted, and its summary.
fn factory_run(mode: Mode) -> (Vec<String>, Vec<String>, RunSummary) {
    let log = Log::default();
    let mut net = factory(&log);
    let summary = run(&mut net, mode);
    assert!(nothing_in_flight(&net), "{mode:?}");
    let mut fired = log.lock().unwrap().clone();
    fired.sort();
    (marking(&net), fired, summary)
}

#[test]
fn a_confluent_net_ends_as_the_sequential_deep_step_leaves_it_under_every_cap() {
    let (marking, fired, summary) = factory_run(Mode::Sequential);
    assert!(summary.quiesced);
    assert!(
        marking.contains(&"/Done: 0,10,20,30,40,50,60,70".to_owned()),
        "{marking:?}"
    );
    for mode in [
        workers(1),
        workers(2),
        workers(3),
        workers(8),
        Mode::Concurrent,
    ] {
        let (concurrent, fired_here, summary_here) = factory_run(mode);
        assert_eq!(
            concurrent, marking,
            "{mode:?}: the same marking at every level"
        );
        assert_eq!(fired_here, fired, "{mode:?}: the same bodies ran");
        assert_eq!(summary_here, summary, "{mode:?}: the same counts");
    }
}

#[test]
fn a_deep_run_comes_out_the_same_many_times() {
    let (marking, fired, _) = factory_run(Mode::Sequential);
    for round in 0..60 {
        let mode = workers(1 + round % 6);
        let (concurrent, fired_here, summary) = factory_run(mode);
        assert!(summary.quiesced, "round {round}");
        assert_eq!(concurrent, marking, "round {round}, {mode:?}");
        assert_eq!(fired_here, fired, "round {round}, {mode:?}");
    }
}

// ----- subnets that share their ports ------------------------------------------

#[test]
fn mills_fed_through_their_ports_run_beside_the_net_around_them() {
    // Two mills whose grinding waits for the other's, fed by the net
    // around them and emptied by it, under every rule a mill may take.
    for stepping in [
        Stepping::InnerFirst,
        Stepping::TakeTurns,
        Stepping::OuterFirst,
    ] {
        let overlap = Arc::new(Overlap::default());
        let mut b = NetBuilder::new();
        let inbox = b.place::<u32>("Inbox");
        let done = b.place::<u32>("Done");
        b.seed(inbox, 0..4);
        for at in 0..2 {
            let mut m = NetBuilder::new();
            let input = m.place::<u32>("In");
            let output = m.place::<u32>("Out");
            m.port(input);
            m.port(output);
            let mine = Arc::clone(&overlap);
            m.transition("Grind", (TakeEdge(input),), (output,), move |n: u32| {
                mine.body(2);
                (n + 100,)
            });
            let mill = b.subnet_place(format!("Mill {at}"), m.build().expect("a mill"));
            let input = b.port_of::<u32>(mill, "In");
            let output = b.port_of::<u32>(mill, "Out");
            b.transition(
                format!("Feed {at}"),
                (TakeEdge(inbox),),
                (input,),
                |n: u32| (n,),
            );
            b.transition(
                format!("Collect {at}"),
                (TakeEdge(output),),
                (done,),
                |n: u32| (n,),
            );
            b.stepping(mill, stepping);
        }
        let mut net = b.build().expect("two mills");
        let summary = run(&mut net, workers(4));
        assert!(summary.quiesced, "{stepping:?}");
        let mut out: Vec<u32> = net
            .tokens(done.id())
            .map(|(_, value)| *value.downcast_ref::<u32>().unwrap())
            .collect();
        out.sort_unstable();
        assert_eq!(out, [100, 101, 102, 103], "{stepping:?}");
        assert_eq!(
            overlap.peak(),
            2,
            "{stepping:?}: the two mills ground at once"
        );
        assert!(nothing_in_flight(&net));
    }
}

// ----- failures inside ------------------------------------------------------------

/// What a failing body returns.
#[derive(Debug)]
struct Broken;

impl std::fmt::Display for Broken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("broken")
    }
}

impl std::error::Error for Broken {}

/// A net with `jobs` jobs whose `Go` fails each time when `fails`, after
/// `before` has run.
fn going(jobs: u32, fails: bool, before: impl Fn() + Send + Sync + 'static) -> Net {
    let mut b = NetBuilder::new();
    let todo = b.place::<u32>("Todo");
    let done = b.place::<u32>("Done");
    b.seed(todo, 0..jobs);
    b.transition(
        "Go",
        (TakeEdge(todo),),
        (done,),
        move |n: u32| -> Result<(u32,), Broken> {
            before();
            if fails { Err(Broken) } else { Ok((n,)) }
        },
    );
    b.build().expect("a net that goes")
}

/// Two nets of one name, `twin`, in one token.
#[derive(Debug)]
struct Twins {
    left: Net,
    right: Net,
}

impl Nested for Twins {
    fn nets(&self) -> Vec<(&str, &Net)> {
        vec![("twin", &self.left), ("twin", &self.right)]
    }
    fn nets_mut(&mut self) -> Vec<(&str, &mut Net)> {
        vec![("twin", &mut self.left), ("twin", &mut self.right)]
    }
}

#[test]
fn disable_switches_off_the_net_that_failed_when_two_in_a_token_share_a_name() {
    for mode in [Mode::Sequential, workers(1), workers(2), Mode::Concurrent] {
        let mut b = NetBuilder::new();
        let pairs = b.place_nested::<Twins>("Pairs");
        let (left, right) = (going(3, false, || {}), going(3, true, || {}));
        let right_id = right.id();
        b.seed(pairs, [Twins { left, right }]);
        let mut net = b.build().expect("twins");

        let summary = drivers::run(&mut net, mode, Limits::default(), OnFailure::Disable)
            .expect("Disable goes on");

        assert!(summary.quiesced, "{mode:?}");
        assert_eq!(
            (summary.fired_count, summary.failed_count),
            (3, 1),
            "{mode:?}: the right twin failed once and was switched off"
        );
        let (token, _) = net.tokens(pairs.id()).next().expect("the twins");
        let nets = net.nested(pairs.id(), token).expect("nested");
        let (left, right) = (nets[0].1, nets[1].1);
        assert_eq!(right.id(), right_id);
        let go = left.transition_named("Go").expect("Go");
        assert!(
            left.is_enabled(go),
            "{mode:?}: the left twin, which did not fail, still goes"
        );
        assert!(
            !right.is_enabled(go),
            "{mode:?}: the right twin is switched off"
        );
        assert_eq!(
            (count(left, "Done"), count(right, "Todo")),
            (3, 2),
            "{mode:?}"
        );

        // Under Stop, the failure names the right twin by its id.
        let mut b = NetBuilder::new();
        let pairs = b.place_nested::<Twins>("Pairs");
        let (left, right) = (going(3, false, || {}), going(3, true, || {}));
        let right_id = right.id();
        b.seed(pairs, [Twins { left, right }]);
        let mut net = b.build().expect("twins");
        match drivers::run(&mut net, mode, Limits::default(), OnFailure::Stop) {
            Err(DriverError::FiringFailed { error, .. }) => {
                assert_eq!(error.nested_ids, [right_id], "{mode:?}");
                assert!(
                    matches!(&error.path[..], [PathStep::Token { place, net, .. }]
                        if *place == pairs.id() && net == "twin"),
                    "{mode:?}: {:?}",
                    error.path
                );
            }
            other => panic!("{mode:?}: the right twin's failure, not {other:?}"),
        }
        assert!(nothing_in_flight(&net), "{mode:?}");
    }
}

#[test]
fn each_on_failure_behaves_inside_as_at_the_top() {
    for on_failure in [OnFailure::Disable, OnFailure::Continue] {
        let build = || {
            let mut b = NetBuilder::new();
            let desk = b.place_nested::<Net>("Desk");
            b.seed(desk, [going(3, true, || {}), going(2, false, || {})]);
            b.stepping(desk, Stepping::TakeTurns);
            b.build().expect("a desk with a failing worker")
        };
        let mut net = build();
        let sequential = drivers::run(&mut net, Mode::Sequential, Limits::default(), on_failure)
            .expect("goes on");
        for mode in [workers(1), workers(2), Mode::Concurrent] {
            let mut net = build();
            let concurrent =
                drivers::run(&mut net, mode, Limits::default(), on_failure).expect("goes on");
            assert_eq!(concurrent, sequential, "{on_failure:?}, {mode:?}");
            assert!(nothing_in_flight(&net));
        }
        let expected = match on_failure {
            OnFailure::Disable => (2, 1),
            _ => (2, 3),
        };
        assert_eq!(
            (sequential.fired_count, sequential.failed_count),
            expected,
            "{on_failure:?}"
        );
    }
}

#[test]
fn a_failure_inside_stops_the_run_once_everything_in_flight_has_landed() {
    for mode in [workers(2), Mode::Concurrent] {
        let failed = Arc::new(Count::default());
        let waits = Arc::clone(&failed);
        // `Slow` is still running when `Fast` fails: it waits for that.
        let slow = going(1, false, move || waits.wait_for(1));
        let tells = Arc::clone(&failed);
        let fast = going(1, true, move || tells.add());
        let fast_id = fast.id();
        let mut b = NetBuilder::new();
        let yard = b.place_nested::<Net>("Yard");
        b.seed(yard, [slow, fast]);
        let mut net = b.build().expect("a yard");

        match drivers::run(&mut net, mode, Limits::default(), OnFailure::Stop) {
            Err(DriverError::FiringFailed { error, summary }) => {
                assert_eq!(error.nested_ids, [fast_id], "{mode:?}");
                assert_eq!(error.path.len(), 1, "{mode:?}");
                assert_eq!(
                    (summary.fired_count, summary.failed_count),
                    (1, 1),
                    "{mode:?}: the slow one landed too"
                );
            }
            other => panic!("{mode:?}: the fast one's failure, not {other:?}"),
        }
        assert!(nothing_in_flight(&net), "{mode:?}");
        assert_eq!(count(nets_in(&net, yard.id())[0], "Done"), 1, "{mode:?}");
    }
}

#[test]
fn a_choice_inside_that_cannot_be_made_stops_the_run_once_everything_in_flight_has_landed() {
    for mode in [Mode::Sequential, workers(2), Mode::Concurrent] {
        let mut lost = NetBuilder::new();
        let todo = lost.place::<u32>("Todo");
        let done = lost.place::<u32>("Done");
        lost.seed(todo, [0]);
        lost.transition("Lost", (TakeEdge(todo),), (done,), |n: u32| (n,))
            .priority(|_| f64::NAN);
        let lost = lost.build().expect("a net that cannot choose");
        let mut b = NetBuilder::new();
        let yard = b.place_nested::<Net>("Yard");
        b.seed(yard, [worker(1, |_| {}), lost]);
        let mut net = b.build().expect("a yard");
        let (lost_token, _) = net.tokens(yard.id()).nth(1).expect("the second");

        match drivers::run(&mut net, mode, Limits::default(), OnFailure::Continue) {
            Err(DriverError::SelectionFailed { error, summary }) => {
                assert!(
                    matches!(error.why, SelectionFault::NanPriority { .. }),
                    "{mode:?}"
                );
                assert!(
                    matches!(&error.path[..], [PathStep::Token { place, token, .. }]
                        if *place == yard.id() && *token == lost_token),
                    "{mode:?}: the path names the net: {:?}",
                    error.path
                );
                assert_eq!(summary.fired_count, 1, "{mode:?}: the worker landed");
            }
            other => panic!("{mode:?}: the choice, not {other:?}"),
        }
        assert!(nothing_in_flight(&net), "{mode:?}");
        assert_eq!(count(nets_in(&net, yard.id())[0], "Done"), 1, "{mode:?}");
    }
}

#[test]
fn a_panic_inside_lands_everything_in_flight_and_lifts_what_the_net_logged() {
    for mode in [workers(2), Mode::Concurrent] {
        // `Trapped` launches `First`, and then the guard of `Second`
        // panics when it is asked with `First` in flight.
        let mut t = NetBuilder::new();
        let todo = t.place::<u32>("Todo");
        let done = t.place::<u32>("Done");
        let trap = t.place::<u32>("Trap");
        t.seed(todo, [0]);
        t.seed(trap, [0]);
        let first = t
            .transition("First", (TakeEdge(todo),), (done,), |n: u32| (n,))
            .id();
        t.transition("Second", (TakeEdge(trap),), (), |_: u32| {})
            .guard(move |net: &Net| {
                assert!(!net.is_in_flight(first), "a guard that panics");
                false
            });
        let trapped = t.build().expect("a trapped net");
        let trapped_id = trapped.id();
        let mut b = NetBuilder::new();
        let yard = b.place_nested::<Net>("Yard");
        b.seed(yard, [worker(1, |_| {}), trapped]);
        b.open();
        let mut net = b.build().expect("a yard");

        let ran = catch_unwind(AssertUnwindSafe(|| {
            drivers::run(&mut net, mode, Limits::default(), OnFailure::Stop)
        }));

        assert!(ran.is_err(), "{mode:?}: the guard's panic went on");
        assert!(
            nothing_in_flight(&net),
            "{mode:?}: every firing landed first"
        );
        let inside = nets_in(&net, yard.id());
        assert_eq!(count(inside[0], "Done"), 1, "{mode:?}");
        assert_eq!(count(inside[1], "Done"), 1, "{mode:?}");
        let events: Vec<Event> = net.drain_events().collect();
        let of_trapped: Vec<&Event> = events
            .iter()
            .filter_map(|event| match event {
                Event::Nested {
                    nested_id, event, ..
                } if *nested_id == trapped_id => Some(&**event),
                _ => None,
            })
            .collect();
        assert!(
            matches!(
                &of_trapped[..],
                [Event::Launched { .. }, Event::Distributed { .. }]
            ),
            "{mode:?}: what the net logged before the panic is lifted: {of_trapped:?}"
        );
        let (token, _) = net.tokens(yard.id()).nth(1).expect("the trapped net");
        let left = net
            .nested_scope(yard.id(), token, |nets| nets[0].1.drain_events().count())
            .expect("nested");
        assert_eq!(left, 0, "{mode:?}: nothing left behind in its log");
    }
}

#[test]
fn a_panic_inside_a_deep_step_lifts_what_the_net_inside_had_logged() {
    // `Bags` keys its token by a key that panics once the net it carries
    // has fired, so the middle net panics as it indexes the token again,
    // with the bottom net's firing in its log.
    let build = || {
        let mut m = NetBuilder::new();
        let bags = m.place_nested::<Net>("Bags");
        m.keys(bags, ["fresh"], |bottom: &Net| {
            assert_eq!(
                bottom.fired_count(),
                0,
                "a key that panics once its net has fired"
            );
            [KeyValue::from(true)]
        });
        m.seed(bags, [worker(1, |_| {})]);
        let middle = m.build().expect("a middle net");
        let mut b = NetBuilder::new();
        let yard = b.place_nested::<Net>("Yard");
        b.seed(yard, [middle]);
        b.build().expect("three levels")
    };
    for mode in [None, Some(workers(2))] {
        let mut net = build();
        let ran = catch_unwind(AssertUnwindSafe(|| match mode {
            None => net.step_deep().map(drop).map_err(drop),
            Some(mode) => drivers::run(&mut net, mode, Limits::default(), OnFailure::Stop)
                .map(drop)
                .map_err(drop),
        }));
        assert!(ran.is_err(), "{mode:?}: the key's panic went on");
        let events: Vec<Event> = net.drain_events().collect();
        let landed = unwrapped(&events).into_iter().any(|(depth, event)| {
            depth == 2 && matches!(event, Event::Fired { .. } | Event::Distributed { .. })
        });
        assert!(
            landed,
            "{mode:?}: the bottom firing reached the top log: {events:?}"
        );
    }
}

// ----- limits ----------------------------------------------------------------------

#[test]
fn the_limits_count_launches_at_every_level() {
    let build = |bodies: &Arc<AtomicUsize>| {
        let counted = |bodies: &Arc<AtomicUsize>| {
            let bodies = Arc::clone(bodies);
            move |_: u32| {
                bodies.fetch_add(1, Ordering::SeqCst);
            }
        };
        let mut b = NetBuilder::new();
        let inbox = b.place::<u32>("Inbox");
        let outbox = b.place::<u32>("Outbox");
        b.seed(inbox, 0..4);
        let mine = counted(bodies);
        b.transition("Forward", (TakeEdge(inbox),), (outbox,), move |n: u32| {
            mine(n);
            (n,)
        });
        let crew = b.place_nested::<Net>("Crew");
        b.seed(crew, (0..3).map(|_| worker(4, counted(bodies))));
        b.stepping(crew, Stepping::TakeTurns);
        b.build().expect("sixteen bodies")
    };
    for mode in [Mode::Sequential, workers(1), workers(3), Mode::Concurrent] {
        let bodies = Arc::new(AtomicUsize::new(0));
        let mut net = build(&bodies);
        let paced = Limits {
            stop_after_n_firings: Some(7),
            ..Limits::default()
        };
        let first = drivers::run(&mut net, mode, paced, OnFailure::Stop).expect("paced");
        assert!(!first.quiesced, "{mode:?}");
        assert_eq!(first.fired_count, 7, "{mode:?}");
        assert_eq!(
            bodies.load(Ordering::SeqCst),
            7,
            "{mode:?}: seven launches, at any level"
        );
        assert!(nothing_in_flight(&net), "{mode:?}");
        let rest = run(&mut net, mode);
        assert!(rest.quiesced, "{mode:?}");
        assert_eq!(rest.fired_count, 9, "{mode:?}");

        let bodies = Arc::new(AtomicUsize::new(0));
        let mut net = build(&bodies);
        let fused = Limits {
            error_after_n_firings: Some(5),
            ..Limits::default()
        };
        match drivers::run(&mut net, mode, fused, OnFailure::Stop) {
            Err(DriverError::TooManyFirings { summary, .. }) => {
                assert_eq!(summary.fired_count, 5, "{mode:?}");
            }
            other => panic!("{mode:?}: the fuse, not {other:?}"),
        }
        assert_eq!(bodies.load(Ordering::SeqCst), 5, "{mode:?}");
        assert!(nothing_in_flight(&net), "{mode:?}");
    }
}

// ----- a body that drives a net it holds ----------------------------------------

/// A net with a take-turns `Desk` of three workers whose bodies wait for
/// each other.
fn office_of_three(overlap: &Arc<Overlap>) -> Net {
    let mut b = NetBuilder::new();
    let desk = b.place_nested::<Net>("Desk");
    b.seed(desk, (0..3).map(|_| overlapping(1, overlap, 3)));
    b.stepping(desk, Stepping::TakeTurns);
    b.build().expect("an office of three")
}

#[test]
fn a_body_that_drives_a_net_it_holds_gets_deep_concurrency_on_its_own_pool() {
    for mode in [Mode::Sequential, workers(2), Mode::Concurrent] {
        let (lent, owned) = (Arc::new(Overlap::default()), Arc::new(Overlap::default()));
        let mut b = NetBuilder::new();
        let shelf = b.place_nested::<Net>("Shelf");
        let crates = b.place_nested::<Net>("Crates");
        let hauled = b.place_nested::<Net>("Hauled");
        let go = b.place::<()>("Go");
        b.seed(shelf, [office_of_three(&lent)]);
        b.seed(crates, [office_of_three(&owned)]);
        b.seed(go, [()]);
        for place in [shelf, crates, hauled] {
            b.stepping(place, Stepping::Passive);
        }
        // Lent where it lies, and driven on the driver's thread.
        b.transition(
            "Drive",
            (TakeEdge(go), MutateEdge(shelf)),
            (),
            |(): (), office: &mut Net| -> Result<(), DriverError> {
                drivers::run(office, workers(3), Limits::default(), OnFailure::Stop).map(drop)
            },
        );
        // Owned by a body that may run on a worker.
        b.transition(
            "Haul",
            (TakeEdge(crates),),
            (hauled,),
            |mut office: Net| -> Result<(Net,), DriverError> {
                drivers::run(&mut office, workers(3), Limits::default(), OnFailure::Stop)?;
                Ok((office,))
            },
        );
        let mut net = b.build().expect("a shelf and a crate");

        let summary = run(&mut net, mode);

        assert!(summary.quiesced, "{mode:?}");
        assert_eq!(
            lent.peak(),
            3,
            "{mode:?}: the lent office ran its three at once"
        );
        assert_eq!(owned.peak(), 3, "{mode:?}: and the owned one");
        for place in [shelf.id(), hauled.id()] {
            let office = nets_in(&net, place)[0];
            let desk = office.place_named("Desk").expect("a desk");
            assert!(
                nets_in(office, desk)
                    .iter()
                    .all(|inner| count(inner, "Done") == 1)
            );
        }
        let events: Vec<Event> = net.drain_events().collect();
        let deep = unwrapped(&events)
            .into_iter()
            .filter(|(depth, event)| *depth == 2 && matches!(event, Event::Distributed { .. }))
            .count();
        assert_eq!(
            deep, 6,
            "{mode:?}: what the bodies drove is in the outer log"
        );
    }
}

#[test]
fn a_take_turns_subnet_and_the_net_around_it_share_a_port_they_both_take_from() {
    for mode in [Mode::Sequential, workers(1), workers(2), Mode::Concurrent] {
        // Twenty jobs at the port `Pool`, which the subnet's `Inside` and
        // the outer net's `Outside` both take from.
        let mut s = NetBuilder::new();
        let pool = s.place::<u32>("Pool");
        let kept = s.place::<u32>("Kept");
        s.port(pool);
        s.seed(pool, 0..20);
        s.transition("Inside", (TakeEdge(pool),), (kept,), |n: u32| (n,));
        let mut b = NetBuilder::new();
        let ring = b.subnet_place("Ring", s.build().expect("a subnet"));
        let window = b.port_of::<u32>(ring, "Pool");
        let taken = b.place::<u32>("Taken");
        b.transition("Outside", (TakeEdge(window),), (taken,), |n: u32| (n,));
        b.stepping(ring, Stepping::TakeTurns);
        let mut net = b.build().expect("a shared port");

        let summary = run(&mut net, mode);

        assert!(summary.quiesced, "{mode:?}");
        let inside = count(net.subnet(ring.id()).expect("the subnet"), "Kept");
        let outside = net.count(taken.id());
        assert_eq!(inside + outside, 20, "{mode:?}");
        assert!(
            inside >= 5 && outside >= 5,
            "{mode:?}: each had its turns, {inside} inside and {outside} outside"
        );
        if mode == Mode::Sequential {
            assert_eq!((inside, outside), (10, 10), "turn and turn about");
        }
    }
}

// ----- closed nets ------------------------------------------------------------------

#[test]
fn the_firings_inside_closed_nets_are_their_own_moves() {
    for mode in [workers(1), workers(3), Mode::Concurrent] {
        // Closed at every level, and no port anywhere but the bottom
        // workers' `Todo`: nothing from outside could put a token in, and
        // still every net inside fires, lands and is lifted.
        let cap = match mode {
            Mode::ConcurrentWith { workers } => workers.get().min(3),
            _ => thread::available_parallelism()
                .map_or(1, NonZeroUsize::get)
                .min(3),
        };
        let overlap = Arc::new(Overlap::default());
        let mut m = NetBuilder::new();
        let bench = m.place_nested::<Net>("Bench");
        m.seed(bench, (0..3).map(|_| overlapping(2, &overlap, cap)));
        let middle = m.build().expect("a closed middle net");
        assert!(middle.is_closed());
        let mut b = NetBuilder::new();
        let yard = b.place_nested::<Net>("Yard");
        b.seed(yard, [middle]);
        let mut net = b.build().expect("a closed net");
        assert!(net.is_closed());

        let summary = run(&mut net, mode);

        assert!(summary.quiesced, "{mode:?}");
        assert_eq!(summary.fired_count, 6, "{mode:?}");
        assert_eq!(overlap.peak(), cap, "{mode:?}");
        let events: Vec<Event> = net.drain_events().collect();
        assert_eq!(
            unwrapped(&events)
                .iter()
                .filter(|(depth, event)| *depth == 2 && matches!(event, Event::Distributed { .. }))
                .count(),
            6,
            "{mode:?}: every landing lifted through the closed middle net"
        );

        // The program is still outside: the bottom nets are shielded by the
        // closed nets around them.
        let (token, _) = net.tokens(yard.id()).next().expect("the middle net");
        let refused = net
            .nested_scope(yard.id(), token, |nets| {
                let middle = &mut *nets[0].1;
                let bench = middle.place_named("Bench").expect("Bench");
                let (bottom, _) = middle.tokens(bench).next().expect("a worker");
                middle
                    .nested_scope(bench, bottom, |nets| {
                        let todo = nets[0].1.place_ref::<u32>("Todo").expect("Todo");
                        nets[0].1.try_put(todo, 9).is_err()
                    })
                    .expect("nested")
            })
            .expect("nested");
        assert!(refused, "{mode:?}: the shield holds for the program");
    }
}
