//! A panic of the net's own code leaves nothing stranded (since
//! 2026-11-28). The code is the author's, a guard, a priority, a selector,
//! a key extractor, a `Clone`, a driver's `each_tick` or a scope's
//! closure, and it should not panic; when it does, the panic goes on to the
//! caller, and these tests hold the net to what it promises on the way:
//!
//! - a launch that panics before its firing is made puts the body back, so
//!   the transition is not in flight for good, and takes none of its
//!   tokens (since 0.4.0);
//! - a concurrent driver that panics on its own thread lands every firing
//!   airborne first, so none is in flight when the panic comes out;
//! - a scope whose closure panics still indexes the token again, lifts the
//!   inner events and checks that the subnet is the one it lent.
//!
//! The Python library's `tests/test_concurrent_run_lands_in_flight.py` is
//! the same rule for its concurrent runs: a guard, a priority or a selector
//! that raises mid-sweep lands the firings in flight before the error goes
//! on.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use petritype::drivers::{self, Limits, Mode, OnFailure};
use petritype::{
    Event, Filter, Firing, KeyValue, MutateEdge, Nested, Net, NetBuilder, Param, PlaceRef,
    SnapshotEdge, Stepping, SubnetRef, TakeEdge, TokenId, TransitionId,
};

/// The message a caught panic carried.
fn message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_default()
}

/// Runs `act`, which must panic, and gives back the panic's message.
fn panics<R>(act: impl FnOnce() -> R) -> String {
    match catch_unwind(AssertUnwindSafe(act)) {
        Ok(_) => panic!("it was to panic"),
        Err(payload) => message(&*payload),
    }
}

// ----- a launch that panics ------------------------------------------------

/// A place of names keyed by the name, whose extractor panics while
/// `fragile` is set, and a transition that takes a name out of it.
fn keyed_names() -> (Net, PlaceRef<String>, TransitionId, Arc<AtomicBool>) {
    let fragile = Arc::new(AtomicBool::new(false));
    let mut b = NetBuilder::new();
    let names = b.place::<String>("Names");
    let gone = b.place::<String>("Gone");
    let breaks = Arc::clone(&fragile);
    b.keys(names, ["name"], move |name: &String| {
        assert!(!breaks.load(Ordering::SeqCst), "the extractor broke");
        [name.as_str().into()]
    });
    b.seed(names, ["Ada".to_owned()]);
    let leave = b
        .transition("Leave", (TakeEdge(names),), (gone,), |name: String| (name,))
        .id();
    (b.build().expect("a keyed place"), names, leave, fragile)
}

#[test]
fn a_key_extractor_that_panics_in_a_take_puts_the_body_back() {
    for how in ["step", "sequential", "concurrent"] {
        let (mut net, names, leave, fragile) = keyed_names();
        fragile.store(true, Ordering::SeqCst);
        let said = panics(|| match how {
            "step" => drop(net.step()),
            "sequential" => drop(drivers::run(
                &mut net,
                Mode::Sequential,
                Limits::default(),
                OnFailure::Stop,
            )),
            _ => drop(drivers::run(
                &mut net,
                Mode::Concurrent,
                Limits::default(),
                OnFailure::Stop,
            )),
        });
        assert!(said.contains("the extractor broke"), "{how}: {said}");
        assert!(net.in_flight().is_empty(), "{how}: Leave is not in flight");
        assert_eq!(net.count(names.id()), 1, "{how}: Ada is where she was");

        fragile.store(false, Ordering::SeqCst);
        assert_eq!(net.step().expect("Leave fires"), Some(leave), "{how}");
    }
}

/// A token whose `Clone` panics while its flag is set.
#[derive(Debug)]
struct Brittle {
    n: u32,
    breaks: Arc<AtomicBool>,
}

impl Clone for Brittle {
    fn clone(&self) -> Brittle {
        assert!(!self.breaks.load(Ordering::SeqCst), "the clone broke");
        Brittle {
            n: self.n,
            breaks: Arc::clone(&self.breaks),
        }
    }
}

#[test]
fn a_clone_that_panics_in_a_launch_puts_the_body_back() {
    let breaks = Arc::new(AtomicBool::new(false));
    let mut b = NetBuilder::new();
    let config = b.place::<Brittle>("Config");
    let jobs = b.place::<u32>("Jobs");
    let out = b.place::<u32>("Out");
    b.seed(
        config,
        [Brittle {
            n: 10,
            breaks: Arc::clone(&breaks),
        }],
    );
    b.seed(jobs, [1]);
    let scale = b
        .transition(
            "Scale",
            (SnapshotEdge(config), TakeEdge(jobs)),
            (out,),
            |config: Brittle, n: u32| (n * config.n,),
        )
        .id();
    let mut net = b.build().expect("the net builds");

    breaks.store(true, Ordering::SeqCst);
    let said = panics(|| net.launch(scale));
    assert!(said.contains("the clone broke"), "{said}");
    assert!(!net.is_in_flight(scale), "Scale is not in flight");

    breaks.store(false, Ordering::SeqCst);
    let done = net.launch(scale).expect("fireable").run();
    net.distribute(done).expect("it fires");
    assert_eq!(net.count(out.id()), 1);
}

// ----- a launch that panics takes nothing -----------------------------------

/// The tokens of a place, with their ids, oldest first.
fn held_u32(net: &Net, place: PlaceRef<u32>) -> Vec<(TokenId, u32)> {
    net.tokens(place.id())
        .map(|(id, _)| (id, *net.token::<u32>(place.id(), id).expect("a u32")))
        .collect()
}

/// A net whose `Use` takes a job from `Jobs` first and then reads, takes or
/// asks for something whose own code panics while `breaks` is set, as
/// `declare` wires it; `Jobs` holds one job, 7.
fn job_then(
    declare: impl FnOnce(&mut NetBuilder, PlaceRef<u32>, PlaceRef<u32>, Arc<AtomicBool>) -> TransitionId,
) -> (Net, PlaceRef<u32>, TransitionId, Arc<AtomicBool>) {
    let breaks = Arc::new(AtomicBool::new(false));
    let mut b = NetBuilder::new();
    let jobs = b.place::<u32>("Jobs");
    let out = b.place::<u32>("Out");
    b.seed(jobs, [7]);
    let used = declare(&mut b, jobs, out, Arc::clone(&breaks));
    (b.build().expect("the net builds"), jobs, used, breaks)
}

/// A token that carries a net, whose `nets_mut` panics while its flag is
/// set.
#[derive(Debug)]
struct Crate {
    net: Net,
    breaks: Arc<AtomicBool>,
}

impl Nested for Crate {
    fn nets(&self) -> Vec<(&str, &Net)> {
        vec![("inside", &self.net)]
    }

    fn nets_mut(&mut self) -> Vec<(&str, &mut Net)> {
        assert!(!self.breaks.load(Ordering::SeqCst), "nets_mut broke");
        vec![("inside", &mut self.net)]
    }
}

/// Each way stage one runs the net's own code after a consuming edge has
/// chosen its token: a snapshot's `Clone`, a parameter's `Clone`, the
/// clone a restoring transition keeps, a later take's key extractor, a
/// nested token's `nets_mut` and a lent token's key extractor and
/// `nets_mut`. Each panics, and the job is still in `Jobs`, with its id,
/// and indexed, since stage one moves no token until all of that code has
/// run (since 0.4.0; until then the job was lost). Through `step`, `launch`
/// and both drivers.
#[test]
fn a_launch_that_panics_takes_none_of_its_tokens() {
    type Declare =
        fn(&mut NetBuilder, PlaceRef<u32>, PlaceRef<u32>, Arc<AtomicBool>) -> TransitionId;
    let cases: [(&str, &str, Declare); 7] = [
        (
            "a snapshot's clone",
            "the clone broke",
            |b, jobs, out, breaks| {
                let config = b.place::<Brittle>("Config");
                b.seed(config, [Brittle { n: 3, breaks }]);
                b.transition(
                    "Use",
                    (TakeEdge(jobs), SnapshotEdge(config)),
                    (out,),
                    |n: u32, c: Brittle| (n * c.n,),
                )
                .id()
            },
        ),
        (
            "a parameter's clone",
            "the clone broke",
            |b, jobs, out, breaks| {
                b.param(Brittle { n: 3, breaks });
                b.transition(
                    "Use",
                    (TakeEdge(jobs), Param::<Brittle>::new()),
                    (out,),
                    |n: u32, c: Brittle| (n * c.n,),
                )
                .id()
            },
        ),
        (
            "a restoring clone",
            "the clone broke",
            |b, jobs, out, breaks| {
                let config = b.place::<Brittle>("Config");
                b.seed(config, [Brittle { n: 3, breaks }]);
                b.transition(
                    "Use",
                    (TakeEdge(jobs), TakeEdge(config)),
                    (out,),
                    |n: u32, c: Brittle| (n * c.n,),
                )
                .restore_tokens_on_failure()
                .id()
            },
        ),
        (
            "a later take's keys",
            "the extractor broke",
            |b, jobs, out, breaks| {
                let names = b.place::<String>("Names");
                b.keys(names, ["name"], move |name: &String| {
                    assert!(!breaks.load(Ordering::SeqCst), "the extractor broke");
                    [name.as_str().into()]
                });
                b.seed(names, ["Ada".to_owned()]);
                b.transition(
                    "Use",
                    (TakeEdge(jobs), TakeEdge(names)),
                    (out,),
                    |n: u32, _: String| (n,),
                )
                .id()
            },
        ),
        (
            "a nested token's nets",
            "nets_mut broke",
            |b, jobs, out, breaks| {
                let crates = b.place_nested::<Crate>("Crates");
                let net = NetBuilder::new().build().expect("an empty net");
                b.seed(crates, [Crate { net, breaks }]);
                // So that a deep step asks nothing of the crate's net.
                b.stepping(crates, Stepping::Passive);
                b.transition(
                    "Use",
                    (TakeEdge(jobs), TakeEdge(crates)),
                    (out,),
                    |n: u32, _: Crate| (n,),
                )
                .id()
            },
        ),
        (
            "a lent token's keys",
            "the extractor broke",
            |b, jobs, out, breaks| {
                let tally = b.place::<u32>("Tally");
                b.keys(tally, ["n"], move |n: &u32| {
                    assert!(!breaks.load(Ordering::SeqCst), "the extractor broke");
                    [KeyValue::from(i64::from(*n))]
                });
                b.seed(tally, [0]);
                b.transition(
                    "Use",
                    (TakeEdge(jobs), MutateEdge(tally)),
                    (out,),
                    |n: u32, tally: &mut u32| {
                        *tally += n;
                        (n,)
                    },
                )
                .id()
            },
        ),
        (
            "a lent token's nets",
            "nets_mut broke",
            |b, jobs, out, breaks| {
                let crates = b.place_nested::<Crate>("Crates");
                let net = NetBuilder::new().build().expect("an empty net");
                b.seed(crates, [Crate { net, breaks }]);
                // So that a deep step asks nothing of the crate's net.
                b.stepping(crates, Stepping::Passive);
                b.transition(
                    "Use",
                    (TakeEdge(jobs), MutateEdge(crates)),
                    (out,),
                    |n: u32, _: &mut Crate| (n,),
                )
                .id()
            },
        ),
    ];
    for (what, says, declare) in cases {
        for how in ["step", "launch", "sequential", "concurrent"] {
            let (mut net, jobs, used, breaks) = job_then(declare);
            let before = held_u32(&net, jobs);
            breaks.store(true, Ordering::SeqCst);
            let said = panics(|| match how {
                "step" => drop(net.step()),
                "launch" => drop(net.launch(used).map(Firing::run)),
                "sequential" => drop(drivers::run(
                    &mut net,
                    Mode::Sequential,
                    Limits::default(),
                    OnFailure::Stop,
                )),
                _ => drop(drivers::run(
                    &mut net,
                    Mode::Concurrent,
                    Limits::default(),
                    OnFailure::Stop,
                )),
            });
            assert!(said.contains(says), "{what}, {how}: {said}");
            assert!(net.in_flight().is_empty(), "{what}, {how}: not in flight");
            assert_eq!(
                held_u32(&net, jobs),
                before,
                "{what}, {how}: the job is where it was"
            );
            assert!(
                net.drain_events().next().is_none(),
                "{what}, {how}: nothing happened"
            );

            breaks.store(false, Ordering::SeqCst);
            assert_eq!(net.step().expect("it fires"), Some(used), "{what}, {how}");
            assert!(net.tokens(jobs.id()).next().is_none(), "{what}, {how}");
        }
    }
}

// ----- a concurrent driver that panics on its own thread -------------------

/// `Slow` takes from `A` and `Other` from `B`, each into `Done`; `Slow` is
/// declared first, so it launches first, and its body takes 50 ms.
/// `question` says what of `Other`'s panics the second time it is asked:
/// its `"guard"`, its `"priority"`, or the net's `"selector"`.
fn two_lanes(question: &str) -> (Net, [PlaceRef<u32>; 3]) {
    let asked = Arc::new(AtomicUsize::new(0));
    let second = move || asked.fetch_add(1, Ordering::SeqCst) > 0;
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
    let mut other = b.transition("Other", (TakeEdge(bee),), (done,), |n: u32| (n,));
    match question {
        "guard" => {
            let second = second.clone();
            other.guard(move |_| {
                assert!(!second(), "the guard panics");
                true
            });
        }
        "priority" => {
            let second = second.clone();
            other.priority(move |_| {
                assert!(!second(), "the priority panics");
                0.0
            });
        }
        _ => {}
    }
    let mut net = b.build().expect("the net builds");
    if question == "selector" {
        net.set_selector(move |_, offered| {
            assert!(!second(), "the selector panics");
            offered.first().copied()
        });
    }
    (net, [a, bee, done])
}

/// What two lanes look like after a run that panicked on its second
/// question: `Slow` landed, `Other` never launched, nothing in flight.
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
}

#[test]
fn a_question_that_panics_mid_sweep_lands_the_firing_in_flight_first() {
    for indefinitely in [false, true] {
        for question in ["guard", "priority", "selector"] {
            let (mut net, places) = two_lanes(question);
            let said = panics(|| {
                if indefinitely {
                    let stop = AtomicBool::new(false);
                    drop(drivers::run_indefinitely(
                        &mut net,
                        Mode::Concurrent,
                        Duration::from_millis(1),
                        &stop,
                        OnFailure::Stop,
                        |_| {},
                    ));
                } else {
                    drop(drivers::run(
                        &mut net,
                        Mode::Concurrent,
                        Limits::default(),
                        OnFailure::Stop,
                    ));
                }
            });
            assert!(
                said.contains(&format!("the {question} panics")),
                "{question}: {said}"
            );
            slow_landed_first(&net, places);
            let launched = net
                .drain_events()
                .filter(|event| matches!(event, Event::Launched { .. }))
                .count();
            assert_eq!(launched, 1, "{question}: Slow alone launched");
        }
    }
}

#[test]
fn a_panic_of_each_tick_lands_the_firing_in_flight_first() {
    let (mut net, places) = two_lanes("none");
    let ticks = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let said = panics(|| {
        drivers::run_indefinitely(
            &mut net,
            Mode::Concurrent,
            Duration::from_millis(1),
            &stop,
            OnFailure::Stop,
            |net| {
                // The first tick launches both, and Slow is still airborne
                // at the second.
                if ticks.fetch_add(1, Ordering::SeqCst) == 1 {
                    assert!(!net.in_flight().is_empty(), "Slow is airborne");
                    panic!("each_tick panics");
                }
            },
        )
    });
    assert!(said.contains("each_tick panics"), "{said}");
    let [a, _, done] = places;
    assert!(net.in_flight().is_empty(), "nothing is left in flight");
    assert_eq!(net.count(a.id()), 0);
    assert_eq!(net.count(done.id()), 2, "Slow landed beside Other");
}

#[test]
fn a_landing_that_panics_lets_the_others_land_before_the_panic_goes_on() {
    // `Done` is keyed by its value, and the extractor panics on 1, which
    // `Fast` makes; `Slow` makes 2, 60 ms later.
    let mut b = NetBuilder::new();
    let left = b.place::<u32>("Left");
    let right = b.place::<u32>("Right");
    let done = b.place::<u32>("Done");
    b.keys(done, ["n"], |n: &u32| {
        assert_ne!(*n, 1, "the extractor broke on 1");
        [KeyValue::from(i64::from(*n))]
    });
    b.seed(left, [1]);
    b.seed(right, [2]);
    let fast = b
        .transition("Fast", (TakeEdge(left),), (done,), |n: u32| (n,))
        .id();
    let slow = b
        .transition("Slow", (TakeEdge(right),), (done,), |n: u32| {
            thread::sleep(Duration::from_millis(60));
            (n,)
        })
        .id();
    let mut net = b.build().expect("the net builds");

    let said = panics(|| {
        drivers::run(
            &mut net,
            Mode::Concurrent,
            Limits::default(),
            OnFailure::Stop,
        )
    });

    assert!(said.contains("the extractor broke on 1"), "{said}");
    assert!(net.in_flight().is_empty(), "both bodies are home");
    assert!(!net.is_in_flight(fast) && !net.is_in_flight(slow));
    let landed = net
        .query(&Filter::new().key_in("n", [2_i64]))
        .expect("every place exists");
    assert_eq!(landed.len(), 1, "Slow's token landed, and is indexed");
}

// ----- a scope whose closure panics ----------------------------------------

#[test]
fn a_nested_scope_that_panics_still_indexes_the_token_and_lifts_its_events() {
    let mut inner = NetBuilder::new();
    let todo = inner.place::<u32>("Todo");
    inner.port(todo);
    let idle = inner.build().expect("the inner net builds");

    let mut b = NetBuilder::new();
    let desks = b.place_nested::<Net>("Desks");
    b.keys(desks, ["busy"], move |net: &Net| {
        [(net.count(todo.id()) > 0).into()]
    });
    b.seed(desks, [idle]);
    b.open();
    let mut net = b.build().expect("the net builds");
    let (desk, _) = net.tokens(desks.id()).next().expect("one desk");
    let busy = Filter::new().key_in("busy", [true]);
    assert!(net.query(&busy).unwrap().is_empty());

    let said = panics(|| {
        net.nested_scope(desks.id(), desk, |nets| {
            nets[0].1.put(todo, 1);
            panic!("the closure panics");
        })
    });

    assert!(said.contains("the closure panics"), "{said}");
    let found: Vec<_> = net
        .query(&busy)
        .unwrap()
        .into_iter()
        .map(|found| found.token)
        .collect();
    assert_eq!(found, [desk], "indexed again as the closure left it");
    let events: Vec<Event> = net.drain_events().collect();
    assert!(
        matches!(
            &events[..],
            [Event::Nested { place, event, .. }]
                if *place == desks.id() && matches!(**event, Event::Added { .. })
        ),
        "the put is lifted: {events:?}"
    );
}

/// A world with one subnet place, `Level`, whose subnet has a port
/// `Lobby`, and the world declared open so the program may write there.
fn world() -> (Net, SubnetRef, PlaceRef<String>) {
    let mut b = NetBuilder::new();
    let lobby = b.place::<String>("Lobby");
    b.port(lobby);
    let level = b.build().expect("the level builds");
    let mut b = NetBuilder::new();
    let place = b.subnet_place("Level", level);
    b.open();
    (b.build().expect("the world builds"), place, lobby)
}

#[test]
fn a_subnet_scope_that_panics_still_lifts_its_events() {
    let (mut net, level, lobby) = world();
    let said = panics(|| {
        net.subnet_scope(level.id(), |subnet| {
            subnet.put(lobby, "Ada".to_owned());
            panic!("the closure panics");
        })
    });
    assert!(said.contains("the closure panics"), "{said}");
    let events: Vec<Event> = net.drain_events().collect();
    assert!(
        matches!(
            &events[..],
            [Event::Subnet { place, event, .. }]
                if *place == level.id() && matches!(**event, Event::Added { .. })
        ),
        "the put is lifted: {events:?}"
    );
}

#[test]
fn a_subnet_scope_that_panics_after_swapping_the_subnet_says_so() {
    let (mut net, level, _) = world();
    let said = panics(|| {
        net.subnet_scope(level.id(), |subnet| {
            *subnet = NetBuilder::new().build().expect("an empty net");
            panic!("the closure panics");
        })
    });
    assert!(
        said.contains("the closure put another net in the place of the subnet"),
        "{said}"
    );
}
