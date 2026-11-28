//! A concurrent sweep launches what the net chooses (since 2026-11-28):
//! each launch is the custom selector's choice if one is set, else the
//! highest priority with ties by the tie-break, the round robin's cursor
//! moving with each launch, among the transitions fireable now that the
//! sweep has not launched yet. After each launch the choice is made afresh
//! on the marking that launch left. Until then a sweep launched everything
//! fireable in declaration order and never asked the selector.
//!
//! The Python library's concurrent sweep (`_launch_sweep` in
//! `petritype/runtime.py`) asks its selector the same way. The cases here
//! are ports of its tests: the priority, tie-break and selector cases of
//! `tests/test_concurrent_token_locking.py`, the concurrent half of
//! `TestANaNPriorityIsAnError` (`tests/test_guards_and_priorities.py`), the
//! guard and bound-edge cases of
//! `tests/test_sweep_replans_what_a_launch_changed.py`, and the selector
//! and priority cases of `tests/test_concurrent_run_lands_in_flight.py`.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use petritype::drivers::{self, DriverError, Limits, Mode, OnFailure};
use petritype::{
    Event, Filter, MutateEdge, Net, NetBuilder, PlaceId, PlaceRef, SelectionFault, TakeEdge,
    TakeWhereEdge, TieBreak, TransitionId,
};

const TIE_BREAKS: [TieBreak; 3] = [
    TieBreak::FirstDeclared,
    TieBreak::LastDeclared,
    TieBreak::RoundRobin,
];

/// One port, `In`, holding one token and feeding every named transition,
/// each of which puts into an `Out` of its own, so exactly one of them can
/// fire. `priority` gives each its priority, `None` for one that sets none.
fn shared_input(
    names: &[&str],
    priority: impl Fn(&str) -> Option<f64>,
    tie_break: TieBreak,
) -> (Net, PlaceRef<u32>, Vec<TransitionId>, Vec<PlaceId>) {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("In");
    b.port(inbox);
    b.seed(inbox, [1]);
    let mut ids = Vec::new();
    let mut outs = Vec::new();
    for name in names {
        let out = b.place::<u32>(format!("{name} Out"));
        let mut transition = b.transition(*name, (TakeEdge(inbox),), (out,), |n: u32| (n,));
        if let Some(score) = priority(name) {
            transition.priority(move |_| score);
        }
        ids.push(transition.id());
        outs.push(out.id());
    }
    b.tie_break(tie_break);
    (b.build().expect("the net builds"), inbox, ids, outs)
}

/// `width` independent transitions `Work0`, `Work1`, ..., each taking the
/// one token of a place of its own.
fn independent(width: u32) -> (Net, Vec<TransitionId>) {
    let mut b = NetBuilder::new();
    let mut ids = Vec::new();
    for i in 0..width {
        let inbox = b.place::<u32>(format!("In{i}"));
        let out = b.place::<u32>(format!("Out{i}"));
        b.seed(inbox, [i]);
        ids.push(
            b.transition(
                format!("Work{i}"),
                (TakeEdge(inbox),),
                (out,),
                |n: u32| (n,),
            )
            .id(),
        );
    }
    (b.build().expect("independent transitions"), ids)
}

/// The transitions launched, in the order of their `Launched` events.
fn launches(net: &mut Net) -> Vec<TransitionId> {
    net.drain_events()
        .filter_map(|event| match event {
            Event::Launched { transition, .. } => Some(transition),
            _ => None,
        })
        .collect()
}

/// Which of `outs` holds a token, as an index into it.
fn winner(net: &Net, outs: &[PlaceId]) -> Vec<usize> {
    outs.iter()
        .enumerate()
        .filter(|(_, out)| net.count(**out) > 0)
        .map(|(at, _)| at)
        .collect()
}

fn concurrently(net: &mut Net) -> drivers::RunSummary {
    drivers::run(net, Mode::Concurrent, Limits::default(), OnFailure::Stop).expect("nothing fails")
}

/// A concurrent `run_indefinitely` that stops once anything has fired.
fn ticking(net: &mut Net) {
    let stop = AtomicBool::new(false);
    drivers::run_indefinitely(
        net,
        Mode::Concurrent,
        Duration::from_millis(1),
        &stop,
        OnFailure::Stop,
        |net| {
            if net.fired_count() > 0 {
                stop.store(true, Ordering::Release);
            }
        },
    )
    .expect("nothing fails");
}

#[test]
fn the_higher_priority_takes_the_token_though_declared_later() {
    let priority = |name: &str| Some(if name == "High" { 10.0 } else { 1.0 });
    let (mut net, _, _, outs) = shared_input(&["Low", "High"], priority, TieBreak::FirstDeclared);
    concurrently(&mut net);
    assert_eq!(winner(&net, &outs), [1], "High's out");

    let (mut net, _, _, outs) = shared_input(&["Low", "High"], priority, TieBreak::FirstDeclared);
    ticking(&mut net);
    assert_eq!(
        winner(&net, &outs),
        [1],
        "High's out, in run_indefinitely too"
    );
}

#[test]
fn a_tie_goes_by_the_tie_break() {
    for (tie_break, expected) in [
        (TieBreak::FirstDeclared, 0),
        (TieBreak::LastDeclared, 2),
        (TieBreak::RoundRobin, 0),
    ] {
        let (mut net, _, _, outs) = shared_input(&["A", "B", "C"], |_| None, tie_break);
        concurrently(&mut net);
        assert_eq!(winner(&net, &outs), [expected], "{tie_break:?}");
    }
}

#[test]
fn the_round_robin_cursor_moves_with_each_concurrent_launch() {
    let (mut net, inbox, ids, _) = shared_input(&["A", "B", "C"], |_| None, TieBreak::RoundRobin);
    let mut order = Vec::new();
    for n in 0..4 {
        if n > 0 {
            net.put(inbox, n);
        }
        concurrently(&mut net);
        order.extend(launches(&mut net));
    }
    assert_eq!(order, [ids[0], ids[1], ids[2], ids[0]], "in turn, wrapping");
    assert_eq!(
        net.select().expect("no NaN"),
        None,
        "nothing left to choose"
    );
}

#[test]
fn a_custom_selector_chooses_each_launch() {
    let (mut net, _, ids, outs) = shared_input(&["A", "Picked"], |_| None, TieBreak::FirstDeclared);
    let picked = ids[1];
    net.set_selector(move |_, offered| offered.iter().copied().find(|&t| t == picked));
    concurrently(&mut net);
    assert_eq!(winner(&net, &outs), [1], "Picked's out");
}

#[test]
fn the_selector_is_offered_what_is_still_fireable_after_each_launch() {
    let (mut net, ids) = independent(3);
    let offers: Arc<Mutex<Vec<Vec<TransitionId>>>> = Arc::default();
    let log = Arc::clone(&offers);
    net.set_selector(move |_, offered| {
        log.lock().unwrap().push(offered.to_vec());
        offered.last().copied()
    });

    let summary = concurrently(&mut net);

    assert!(summary.quiesced);
    assert_eq!(
        *offers.lock().unwrap(),
        [ids.clone(), ids[..2].to_vec(), ids[..1].to_vec()],
        "the launched ones are in flight, and offered no more"
    );
    assert_eq!(launches(&mut net), [ids[2], ids[1], ids[0]]);
}

#[test]
fn without_selector_priority_or_tie_break_the_sweep_launches_in_declaration_order() {
    let (mut net, ids) = independent(5);
    concurrently(&mut net);
    assert_eq!(launches(&mut net), ids);
}

#[test]
fn each_launch_is_chosen_on_the_marking_the_launches_before_it_left() {
    // `B` ranks by what `In` holds, `C` at 2.5: with three tokens `B`
    // outranks `C`, and once `A` has taken one it no longer does.
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("In");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [1, 2, 3]);
    let a = b
        .transition("A", (TakeEdge(inbox),), (out,), |n: u32| (n,))
        .priority(|_| 10.0)
        .id();
    let bee = b
        .transition("B", (TakeEdge(inbox),), (out,), |n: u32| (n,))
        .priority(move |net| net.count(inbox.id()) as f64)
        .id();
    let c = b
        .transition("C", (TakeEdge(inbox),), (out,), |n: u32| (n,))
        .priority(|_| 2.5)
        .id();
    let mut net = b.build().expect("the net builds");

    concurrently(&mut net);

    assert_eq!(launches(&mut net), [a, c, bee]);
}

#[test]
fn a_guard_is_asked_again_after_each_launch() {
    // `Gated` takes from a place of its own, and its guard asks for a token
    // in `Shared`, which `Taker`, declared first, takes from.
    let mut b = NetBuilder::new();
    let shared = b.place::<u32>("Shared");
    let own = b.place::<u32>("Own");
    let out = b.place::<u32>("Out");
    b.seed(shared, [1]);
    b.seed(own, [2]);
    let taker = b
        .transition("Taker", (TakeEdge(shared),), (out,), |n: u32| (n,))
        .id();
    let gated = b
        .transition("Gated", (TakeEdge(own),), (out,), |n: u32| (n,))
        .guard(move |net| net.count(shared.id()) > 0)
        .id();
    let mut net = b.build().expect("the net builds");
    assert_eq!(net.fireable(), [taker, gated]);

    let summary = concurrently(&mut net);

    assert!(summary.quiesced);
    assert_eq!(launches(&mut net), [taker], "Gated's guard said no");
    assert_eq!(net.count(own.id()), 1);
}

#[test]
fn a_bound_edge_pairs_on_the_marking_the_launch_before_it_left() {
    // `Load` pairs an order with the courier it names: the oldest order
    // with a courier, `y`. `Cancel`, which outranks it, launches first and
    // takes the front order, `x`; `Load` then pairs `y` with `y`.
    let mut b = NetBuilder::new();
    let orders = b.place::<String>("Orders");
    let couriers = b.place::<String>("Couriers");
    let cancelled = b.place::<String>("Cancelled");
    let loaded = b.place::<String>("Loaded");
    b.keys(orders, ["courier"], |order: &String| {
        [order.as_str().into()]
    });
    b.keys(couriers, ["name"], |name: &String| [name.as_str().into()]);
    b.seed(orders, ["x", "y", "z"].map(String::from));
    b.seed(couriers, ["y".to_owned()]);
    let paired: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let log = Arc::clone(&paired);
    b.transition(
        "Cancel",
        (TakeEdge(orders),),
        (cancelled,),
        |order: String| (order,),
    )
    .priority(|_| 1.0);
    b.transition(
        "Load",
        (
            TakeEdge(orders),
            TakeWhereEdge(
                couriers,
                Filter::new().key_is_input("name", orders, "courier"),
            ),
        ),
        (loaded,),
        move |order: String, courier: String| {
            log.lock().unwrap().push((order, courier.clone()));
            (courier,)
        },
    );
    let mut net = b.build().expect("the net builds");
    let limits = Limits {
        stop_after_n_firings: Some(2),
        ..Limits::default()
    };

    drivers::run(&mut net, Mode::Concurrent, limits, OnFailure::Stop).expect("nothing fails");

    let names = |place: PlaceRef<String>| -> Vec<String> {
        net.tokens(place.id())
            .map(|(id, _)| net.token::<String>(place.id(), id).unwrap().clone())
            .collect()
    };
    assert_eq!(names(cancelled), ["x"]);
    assert_eq!(*paired.lock().unwrap(), [("y".to_owned(), "y".to_owned())]);
}

#[test]
fn a_declining_selector_ends_the_sweep_and_the_run_is_at_rest() {
    let (mut net, ids) = independent(3);
    net.set_selector(|_, offered| (offered.len() > 1).then(|| offered[0]));

    let summary = concurrently(&mut net);

    assert!(
        summary.quiesced,
        "a declining selector is rest to this driver"
    );
    assert_eq!(summary.fired_count, 2);
    assert_eq!(net.fireable(), [ids[2]], "fireable, and not chosen");
}

#[test]
fn an_inline_transition_has_one_turn_a_sweep() {
    // `Bump` borrows `Live`, so it runs inside `launch`, and stays fireable
    // while `Ticks` lasts; `Work` is detachable and slow.
    let mut b = NetBuilder::new();
    let ticks = b.place::<u32>("Ticks");
    let live = b.place::<u32>("Live");
    let jobs = b.place::<u32>("Jobs");
    let done = b.place::<u32>("Done");
    b.seed(ticks, [1, 1, 1]);
    b.seed(live, [0]);
    b.seed(jobs, [7]);
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
    let work = b
        .transition("Work", (TakeEdge(jobs),), (done,), |n: u32| {
            thread::sleep(Duration::from_millis(30));
            (n,)
        })
        .id();
    let mut net = b.build().expect("the net builds");
    let offers: Arc<Mutex<Vec<Vec<TransitionId>>>> = Arc::default();
    let log = Arc::clone(&offers);
    net.set_selector(move |_, offered| {
        log.lock().unwrap().push(offered.to_vec());
        offered.first().copied()
    });

    let summary = concurrently(&mut net);

    assert_eq!(summary.fired_count, 4);
    assert_eq!(
        *offers.lock().unwrap(),
        [vec![bump, work], vec![work], vec![bump], vec![bump]],
        "Bump is offered once a sweep; the driver waited for Work before the second"
    );
}

#[test]
fn the_fuse_blames_the_transition_the_sweep_chose_next() {
    // Three independent transitions, Work2 outranking the others.
    let mut b = NetBuilder::new();
    let mut ids = Vec::new();
    for i in 0..3_u32 {
        let inbox = b.place::<u32>(format!("In{i}"));
        let out = b.place::<u32>(format!("Out{i}"));
        b.seed(inbox, [i]);
        let mut work = b.transition(
            format!("Work{i}"),
            (TakeEdge(inbox),),
            (out,),
            |n: u32| (n,),
        );
        if i == 2 {
            work.priority(|_| 1.0);
        }
        ids.push(work.id());
    }
    let mut net = b.build().expect("the net builds");
    let limits = Limits {
        error_after_n_firings: Some(1),
        ..Limits::default()
    };

    let error = drivers::run(&mut net, Mode::Concurrent, limits, OnFailure::Stop)
        .expect_err("the second launch is refused");

    let DriverError::TooManyFirings {
        transition,
        name,
        summary,
        ..
    } = error
    else {
        panic!("TooManyFirings, not {error:?}");
    };
    assert_eq!(launches(&mut net), [ids[2]], "Work2 first, by priority");
    assert_eq!((transition, name.as_str()), (ids[0], "Work0"), "then Work0");
    assert_eq!(summary.fired_count, 1);
    assert!(net.in_flight().is_empty());
}

#[test]
fn a_spent_fuse_blames_what_the_sweep_would_have_launched() {
    let priority = |name: &str| Some(if name == "High" { 10.0 } else { 1.0 });
    let (mut net, inbox, ids, _) =
        shared_input(&["Low", "High"], priority, TieBreak::FirstDeclared);
    let spent = Limits {
        error_after_n_firings: Some(0),
        ..Limits::default()
    };

    let error = drivers::run(&mut net, Mode::Concurrent, spent, OnFailure::Stop)
        .expect_err("the fuse is spent");
    let DriverError::TooManyFirings { transition, .. } = error else {
        panic!("TooManyFirings, not {error:?}");
    };
    assert_eq!(transition, ids[1], "High, the sweep's choice");
    assert_eq!(net.count(inbox.id()), 1);

    // A selector that declines attempts nothing, so the fuse holds.
    net.set_selector(|_, _| None);
    let summary = drivers::run(&mut net, Mode::Concurrent, spent, OnFailure::Stop)
        .expect("nothing was attempted");
    assert!(summary.quiesced);
}

#[test]
fn a_nan_priority_stops_a_concurrent_run_and_nothing_fires() {
    for tie_break in TIE_BREAKS {
        for nan_on in ["A", "B"] {
            let priority = |name: &str| (name == nan_on).then_some(f64::NAN);
            let (mut net, inbox, ids, _) = shared_input(&["A", "B"], priority, tie_break);
            let error = drivers::run(
                &mut net,
                Mode::Concurrent,
                Limits::default(),
                OnFailure::Continue,
            )
            .expect_err("a NaN priority");
            let DriverError::SelectionFailed { summary, error } = error else {
                panic!("{tie_break:?}, {nan_on}: a choice, not {error:?}");
            };
            let SelectionFault::NanPriority { transition, .. } = error.why else {
                panic!("a NaN priority, not {:?}", error.why);
            };
            assert_eq!(transition, ids[usize::from(nan_on == "B")]);
            assert_eq!((summary.fired_count, summary.failed_count), (0, 0));
            assert_eq!(net.count(inbox.id()), 1, "{tie_break:?}, {nan_on}");
            assert!(launches(&mut net).is_empty());

            let stop = AtomicBool::new(false);
            let error = drivers::run_indefinitely(
                &mut net,
                Mode::Concurrent,
                Duration::from_millis(1),
                &stop,
                OnFailure::Stop,
                |_| {},
            )
            .expect_err("a NaN priority");
            assert!(
                matches!(error, DriverError::SelectionFailed { .. }),
                "{error:?}"
            );
            assert_eq!(net.count(inbox.id()), 1);
        }
    }
}

/// `Slow` takes from `A` and `Other` from `B`, each into `Done`, `Slow`
/// declared first and so launched first; `Slow`'s body takes 50 ms.
fn two_lanes() -> (Net, [PlaceRef<u32>; 3], [TransitionId; 2]) {
    let mut b = NetBuilder::new();
    let a = b.place::<u32>("A");
    let bee = b.place::<u32>("B");
    let done = b.place::<u32>("Done");
    b.seed(a, [1]);
    b.seed(bee, [2]);
    let slow = b
        .transition("Slow", (TakeEdge(a),), (done,), |n: u32| {
            thread::sleep(Duration::from_millis(50));
            (n * 10,)
        })
        .id();
    let other = b
        .transition("Other", (TakeEdge(bee),), (done,), |n: u32| (n,))
        .id();
    (
        b.build().expect("the net builds"),
        [a, bee, done],
        [slow, other],
    )
}

/// What two lanes look like after a run that stopped on its second choice:
/// `Slow` landed, `Other` never launched.
fn slow_landed_first(net: &Net, places: [PlaceRef<u32>; 3]) {
    let [a, bee, done] = places;
    assert!(net.in_flight().is_empty(), "nothing is left in flight");
    assert_eq!(net.count(a.id()), 0);
    assert_eq!(net.count(bee.id()), 1, "Other did not launch");
    let landed: Vec<u32> = net
        .tokens(done.id())
        .map(|(id, _)| *net.token::<u32>(done.id(), id).unwrap())
        .collect();
    assert_eq!(landed, [10], "Slow landed");
    assert_eq!(net.fired_count(), 1);
}

#[test]
fn a_choice_that_cannot_be_made_mid_sweep_lands_the_firing_in_flight_first() {
    for indefinitely in [false, true] {
        for what in ["selector", "priority"] {
            let (mut net, places, [slow, other]) = two_lanes();
            let asked = Arc::new(AtomicUsize::new(0));
            if what == "selector" {
                // The second time, it names Slow, which is in flight.
                let asked = Arc::clone(&asked);
                net.set_selector(
                    move |_, offered| match asked.fetch_add(1, Ordering::SeqCst) {
                        0 => offered.first().copied(),
                        _ => Some(slow),
                    },
                );
            } else {
                // Other's priority is NaN the second time it is asked.
                net = {
                    let mut b = NetBuilder::new();
                    let a = b.place::<u32>("A");
                    let bee = b.place::<u32>("B");
                    let done = b.place::<u32>("Done");
                    b.seed(a, [1]);
                    b.seed(bee, [2]);
                    b.transition("Slow", (TakeEdge(a),), (done,), |n: u32| {
                        thread::sleep(Duration::from_millis(50));
                        (n * 10,)
                    });
                    let asked = Arc::clone(&asked);
                    b.transition("Other", (TakeEdge(bee),), (done,), |n: u32| (n,))
                        .priority(move |_| match asked.fetch_add(1, Ordering::SeqCst) {
                            0 => 0.0,
                            _ => f64::NAN,
                        });
                    b.build().expect("the net builds")
                };
            }

            let error = if indefinitely {
                let stop = AtomicBool::new(false);
                drivers::run_indefinitely(
                    &mut net,
                    Mode::Concurrent,
                    Duration::from_millis(1),
                    &stop,
                    OnFailure::Stop,
                    |_| {},
                )
            } else {
                drivers::run(
                    &mut net,
                    Mode::Concurrent,
                    Limits::default(),
                    OnFailure::Stop,
                )
            }
            .expect_err("the second choice cannot be made");

            let DriverError::SelectionFailed { error, .. } = error else {
                panic!("{what}: a choice, not {error:?}");
            };
            match (what, error.why) {
                (
                    "selector",
                    SelectionFault::NotOffered {
                        chosen, offered, ..
                    },
                ) => {
                    assert_eq!(chosen, slow);
                    assert_eq!(offered, [(other, "Other".to_owned())]);
                }
                ("priority", SelectionFault::NanPriority { transition, .. }) => {
                    assert_eq!(transition, other);
                }
                (what, why) => panic!("{what}: not {why:?}"),
            }
            slow_landed_first(&net, places);
        }
    }
}

#[test]
fn sequential_and_concurrent_runs_give_a_contested_token_to_the_same_transition() {
    let rankings: [fn(&str) -> Option<f64>; 4] = [
        |_| None,
        |name| (name == "C").then_some(1.0),
        |name| (name != "A").then_some(2.0),
        |name| Some(if name == "B" { f64::INFINITY } else { 0.0 }),
    ];
    for tie_break in TIE_BREAKS {
        for ranking in rankings {
            let names = ["A", "B", "C"];
            let (mut sequential, _, _, outs) = shared_input(&names, ranking, tie_break);
            drivers::run(
                &mut sequential,
                Mode::Sequential,
                Limits::default(),
                OnFailure::Stop,
            )
            .expect("nothing fails");
            let (mut concurrent, _, _, _) = shared_input(&names, ranking, tie_break);
            concurrently(&mut concurrent);
            assert_eq!(
                winner(&concurrent, &outs),
                winner(&sequential, &outs),
                "{tie_break:?}"
            );
        }
    }
}
