//! Integration tests for the fifteenth slice: nets as tokens (DESIGN §6.6).
//!
//! A token may carry Petri nets of its own. The engine runs them after the
//! outer net (`step_deep`, the drivers), lets the outer net read and reach
//! them (`nested`, a `MutateEdge` body, `nested_scope`), and *lifts* what
//! they log into the outer log as `Event::Nested`, at moments fixed by six
//! rules. Most of what is tested here is those moments and that order; the
//! transit example (`examples/transit.rs`) is the acceptance script.

use std::fmt;

use petritype::drivers::{self, DriverError, Limits, Mode, OnFailure};
use petritype::{
    BuildError, DeepStep, Event, FiringFailed, MutateEdge, Nested, Net, NetBuilder, PlaceId,
    PlaceRef, SnapshotEdge, TakeEdge, TokenId, TransitionId,
};

/// An inner net with work to do: `Todo -> Work -> Done`.
struct Worker {
    net: Net,
    todo: PlaceRef<u32>,
    done: PlaceRef<u32>,
    work: TransitionId,
}

fn worker(jobs: u32) -> Worker {
    let mut b = NetBuilder::new();
    let todo = b.place::<u32>("Todo");
    let done = b.place::<u32>("Done");
    b.seed(todo, 0..jobs);
    let work = b
        .transition("Work", (TakeEdge(todo),), (done,), |n: u32| (n,))
        .id();
    Worker {
        net: b.build().expect("a worker builds"),
        todo,
        done,
        work,
    }
}

/// A token owning one net under a name, as a vehicle owns its cabin.
#[derive(Debug)]
struct Bus {
    inner: Net,
}

impl Nested for Bus {
    fn nets(&self) -> Vec<(&str, &Net)> {
        vec![("cabin", &self.inner)]
    }
    fn nets_mut(&mut self) -> Vec<(&str, &mut Net)> {
        vec![("cabin", &mut self.inner)]
    }
}

/// A token owning two nets, to see the order `nets` gives them in.
#[derive(Debug)]
struct Pair {
    left: Net,
    right: Net,
}

impl Nested for Pair {
    fn nets(&self) -> Vec<(&str, &Net)> {
        vec![("left", &self.left), ("right", &self.right)]
    }
    fn nets_mut(&mut self) -> Vec<(&str, &mut Net)> {
        vec![("left", &mut self.left), ("right", &mut self.right)]
    }
}

/// A body's failure.
#[derive(Debug)]
struct Refused;

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("refused")
    }
}

impl std::error::Error for Refused {}

/// The only token of a place.
fn only(net: &Net, place: PlaceId) -> TokenId {
    let mut tokens = net.tokens(place);
    let (token, _) = tokens.next().expect("one token");
    assert!(tokens.next().is_none(), "exactly one token");
    token
}

/// The inner net of the only token of a nested place.
fn inside(net: &Net, place: PlaceId) -> &Net {
    net.nested(place, only(net, place))
        .expect("a nested place")
        .remove(0)
        .1
}

/// An event with the wrapper taken off, and where it was: `None` for an event
/// of the net itself.
fn unwrap(event: &Event) -> (Option<(PlaceId, TokenId, &str)>, &Event) {
    match event {
        Event::Nested {
            place,
            token,
            net,
            event,
            ..
        } => (Some((*place, *token, net.as_str())), event),
        event => (None, event),
    }
}

/// A yard of buses with two transitions over it: `Tinker` reaches inside
/// with a mutating edge and adds a job, `Scrap` consumes the bus.
///
/// Launching `Tinker` runs its body at once (it is not detachable) and its
/// write inside is lifted only when that firing lands: in between, the bus
/// carries an event nobody has lifted yet, which is the one way to see the
/// rules for tokens leaving a place do their work.
struct Yard {
    net: Net,
    yard: PlaceRef<Bus>,
    todo: PlaceRef<u32>,
    tinker: TransitionId,
    scrap: TransitionId,
}

fn yard() -> Yard {
    let Worker {
        net: inner, todo, ..
    } = worker(0);
    let mut b = NetBuilder::new();
    let yard = b.place_nested::<Bus>("Yard");
    b.seed(yard, [Bus { inner }]);
    // By name rather than by the handle above: a bus put into the yard later
    // has a cabin of its own, which that handle does not belong to.
    let tinker = b
        .transition("Tinker", (MutateEdge(yard),), (), |bus: &mut Bus| {
            let todo = bus
                .inner
                .place_ref::<u32>("Todo")
                .expect("a worker's cabin");
            bus.inner.put(todo, 7);
        })
        .id();
    let scrap = b
        .transition("Scrap", (TakeEdge(yard),), (), |_bus: Bus| {})
        .id();
    Yard {
        net: b.build().expect("the yard builds"),
        yard,
        todo,
        tinker,
        scrap,
    }
}

// ----- declaring ----------------------------------------------------------

#[test]
fn a_bare_net_is_a_nested_token_with_no_name() {
    let Worker {
        net: inner,
        done,
        work,
        ..
    } = worker(1);
    let inner_id = inner.id();
    let mut b = NetBuilder::new();
    let desks = b.place_nested::<Net>("Desks");
    b.seed(desks, [inner]);
    let mut net = b.build().expect("builds");
    let token = only(&net, desks.id());

    let nets = net.nested(desks.id(), token).expect("nested");
    assert_eq!(nets.len(), 1);
    assert_eq!(nets[0].0, "");
    assert_eq!(nets[0].1.id(), inner_id);

    let step = net
        .step_deep()
        .expect("nothing fails")
        .expect("work inside");
    assert_eq!(
        step,
        DeepStep::Inside {
            place: desks.id(),
            token,
            net: String::new(),
            nested_id: inner_id,
            step: Box::new(DeepStep::Here { transition: work }),
        }
    );
    assert_eq!(inside(&net, desks.id()).count(done.id()), 1);
    let events: Vec<Event> = net.drain_events().collect();
    assert!(
        matches!(&events[..], [Event::Nested { net, nested_id, .. }] if net.is_empty() && *nested_id == inner_id),
        "{events:?}"
    );
}

#[test]
fn nested_answers_only_for_a_nested_place_and_a_token_in_it() {
    let mut b = NetBuilder::new();
    let plain = b.place::<u32>("Plain");
    let depot = b.place_nested::<Bus>("Depot");
    b.seed(plain, [1]);
    b.seed(
        depot,
        [Bus {
            inner: worker(0).net,
        }],
    );
    let net = b.build().expect("builds");

    assert!(
        net.nested(plain.id(), only(&net, plain.id())).is_none(),
        "not nested"
    );
    assert!(
        net.nested(depot.id(), TokenId::new(99)).is_none(),
        "no such token"
    );
    assert!(
        net.nested(PlaceId::new(9), TokenId::new(0)).is_none(),
        "no such place"
    );
    let nets = net
        .nested(depot.id(), only(&net, depot.id()))
        .expect("the bus");
    assert_eq!(nets[0].0, "cabin");

    let places: Vec<(bool, bool)> = net
        .places()
        .map(|(_, info)| (info.nested, info.passive))
        .collect();
    assert_eq!(places, [(false, false), (true, false)]);
    let topology = net.topology();
    assert!(!topology.places[0].nested && topology.places[1].nested);
}

#[test]
fn passive_on_a_place_that_is_not_nested_is_a_build_error() {
    let mut b = NetBuilder::new();
    let plain = b.place::<u32>("Plain");
    let other = b.place::<u32>("Other");
    b.passive(plain);
    b.passive(other);
    let error = b.build().expect_err("Plain holds no nets");
    assert!(
        matches!(&error, BuildError::NotNested { place } if place == "Plain"),
        "the first one marked: {error:?}"
    );
    assert_eq!(
        error.to_string(),
        "place \"Plain\" is marked passive but was not declared with place_nested"
    );
}

#[test]
fn a_nested_place_is_a_no_clone_place() {
    /// A token that could be cloned, and owns no net: the place still has
    /// no cloner, because `place_nested` records none.
    #[derive(Debug, Clone)]
    struct Hollow;
    impl Nested for Hollow {
        fn nets(&self) -> Vec<(&str, &Net)> {
            Vec::new()
        }
        fn nets_mut(&mut self) -> Vec<(&str, &mut Net)> {
            Vec::new()
        }
    }

    let mut b = NetBuilder::new();
    let shells = b.place_nested::<Hollow>("Shells");
    let out = b.place::<u32>("Out");
    b.transition("Peek", (SnapshotEdge(shells),), (out,), |_shell: Hollow| {
        (1_u32,)
    });
    assert!(matches!(
        b.build(),
        Err(BuildError::SnapshotOfNoClonePlace { .. })
    ));

    let mut b = NetBuilder::new();
    let depot = b.place_nested::<Bus>("Depot");
    b.transition("Scrap", (TakeEdge(depot),), (), |_bus: Bus| {})
        .restore_tokens_on_failure();
    assert!(matches!(
        b.build(),
        Err(BuildError::RestoreOfNoClonePlace { .. })
    ));

    // A whole-net snapshot: without `serde` a nested place cannot be copied
    // at all; with it, its tokens are forked from their templates (slice 16,
    // `tests/slice16.rs`), and an empty one has nothing to fork.
    let mut b = NetBuilder::new();
    let depot = b.place_nested::<Bus>("Depot");
    let net = b.build().expect("builds");
    #[cfg(not(feature = "serde"))]
    assert!(matches!(
        net.snapshot(),
        Err(petritype::NetError::NoClonePlace { place, .. }) if place == depot.id()
    ));
    #[cfg(feature = "serde")]
    assert_eq!(
        net.snapshot().expect("nothing to fork").count(depot.id()),
        0
    );
}

#[test]
fn what_a_seeded_inner_net_logged_before_the_build_is_setup() {
    let Worker {
        net: mut inner,
        todo,
        ..
    } = worker(0);
    inner.put(todo, 1);
    assert_eq!(inner.drain_events().count(), 1, "the inner net did log it");
    inner.put(todo, 2);

    let mut b = NetBuilder::new();
    let desks = b.place_nested::<Net>("Desks");
    b.seed(desks, [inner]);
    let mut net = b.build().expect("builds");
    assert_eq!(net.state_counter(), 0);

    net.step_deep()
        .expect("nothing fails")
        .expect("work inside");
    let events: Vec<Event> = net.drain_events().collect();
    assert_eq!(events.len(), 1, "the firing and nothing older: {events:?}");
    assert!(matches!(unwrap(&events[0]).1, Event::Fired { .. }));
}

// ----- running ------------------------------------------------------------

#[test]
fn step_deep_goes_outer_first_then_place_order_then_token_order() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let outbox = b.place::<u32>("Outbox");
    let first = b.place_nested::<Net>("First");
    let second = b.place_nested::<Pair>("Second");
    b.seed(inbox, [1]);
    let a = worker(1);
    let b_ = worker(1);
    let (a_id, b_id) = (a.net.id(), b_.net.id());
    b.seed(first, [a.net, b_.net]);
    let (left, right) = (worker(1).net, worker(1).net);
    let (left_id, right_id) = (left.id(), right.id());
    b.seed(second, [Pair { left, right }]);
    let outer = b
        .transition("Outer", (TakeEdge(inbox),), (outbox,), |n: u32| (n,))
        .id();
    let mut net = b.build().expect("builds");

    let mut order = Vec::new();
    while let Some(step) = net.step_deep().expect("nothing fails") {
        order.push(match step {
            DeepStep::Here { transition } => {
                assert_eq!(transition, outer);
                ("outer".to_owned(), None)
            }
            DeepStep::Inside {
                place,
                net: name,
                nested_id,
                ..
            } => (format!("{place:?}/{name}"), Some(nested_id)),
        });
        // Once, after the first inner firing: the outer net goes first again
        // as soon as it has something.
        if order.len() == 2 {
            net.put(inbox, 2);
        }
    }

    let first = format!("{:?}/", first.id());
    let second = format!("{:?}/", second.id());
    assert_eq!(
        order,
        [
            ("outer".to_owned(), None),
            (first.clone(), Some(a_id)),
            ("outer".to_owned(), None),
            (first, Some(b_id)),
            (format!("{second}left"), Some(left_id)),
            (format!("{second}right"), Some(right_id)),
        ],
    );
    assert!(net.is_quiescent_deep());
}

#[test]
fn quiescence_is_deep_and_fireable_stays_shallow() {
    let mut b = NetBuilder::new();
    let desks = b.place_nested::<Net>("Desks");
    b.seed(desks, [worker(1).net]);
    let mut net = b.build().expect("builds");

    assert!(net.fireable().is_empty(), "nothing out here");
    assert!(!net.is_quiescent_deep(), "but there is work inside");
    net.step_deep().expect("nothing fails");
    assert!(net.is_quiescent_deep());
}

#[test]
fn a_passive_place_is_stepped_only_by_a_body() {
    let mut b = NetBuilder::new();
    let machines = b.place_nested::<Net>("Machines");
    let cranks = b.place::<()>("Cranks");
    b.passive(machines);
    let Worker {
        net: inner,
        todo,
        done,
        work,
    } = worker(2);
    b.seed(machines, [inner]);
    let crank = b
        .transition(
            "Crank",
            (TakeEdge(cranks), MutateEdge(machines)),
            (),
            |(): (), machine: &mut Net| -> Result<(), FiringFailed> {
                machine.step()?;
                Ok(())
            },
        )
        .id();
    let mut net = b.build().expect("builds");
    let machine = only(&net, machines.id());
    assert!(net.places().next().expect("Machines").1.passive);

    // Left alone: the inner net has work, and nothing does it.
    assert!(net.is_quiescent_deep(), "a passive place does not count");
    assert_eq!(net.step_deep().expect("nothing fails"), None);
    let summary = drivers::run(
        &mut net,
        Mode::Sequential,
        Limits::default(),
        OnFailure::Stop,
    )
    .expect("nothing fails");
    assert_eq!((summary.quiesced, summary.fired_count), (true, 0));
    assert_eq!(inside(&net, machines.id()).count(todo.id()), 2);

    // A crank steps it once, and the step is reported before the crank's own
    // mutation.
    net.put(cranks, ());
    net.drain_events().for_each(drop);
    assert_eq!(
        net.step_deep().expect("nothing fails"),
        Some(DeepStep::Here { transition: crank })
    );
    assert_eq!(inside(&net, machines.id()).count(done.id()), 1);
    let events: Vec<Event> = net.drain_events().collect();
    assert_eq!(events.len(), 3, "{events:?}");
    assert!(matches!(
        unwrap(&events[0]),
        (Some((place, token, "")), Event::Fired { transition, .. })
            if place == machines.id() && token == machine && *transition == work
    ));
    assert_eq!(
        events[1],
        Event::Mutated {
            place: machines.id(),
            token: machine
        }
    );
    assert!(matches!(events[2], Event::Fired { transition, .. } if transition == crank));
}

#[test]
fn two_levels_down_is_a_nested_inside_a_nested() {
    /// A passenger who carries luggage: a net inside a token inside a net.
    #[derive(Debug)]
    struct Passenger {
        luggage: Net,
    }
    impl Nested for Passenger {
        fn nets(&self) -> Vec<(&str, &Net)> {
            vec![("luggage", &self.luggage)]
        }
        fn nets_mut(&mut self) -> Vec<(&str, &mut Net)> {
            vec![("luggage", &mut self.luggage)]
        }
    }

    let Worker {
        net: luggage,
        work: stow,
        done: stowed,
        ..
    } = worker(1);
    let luggage_id = luggage.id();
    let mut cabin = NetBuilder::new();
    let seats = cabin.place_nested::<Passenger>("Seats");
    cabin.seed(seats, [Passenger { luggage }]);
    let cabin = cabin.build().expect("the cabin builds");
    let cabin_id = cabin.id();

    let mut b = NetBuilder::new();
    let depot = b.place_nested::<Bus>("Depot");
    b.seed(depot, [Bus { inner: cabin }]);
    let mut net = b.build().expect("the city builds");
    let bus = only(&net, depot.id());
    let passenger = only(inside(&net, depot.id()), seats.id());
    assert!(!net.is_quiescent_deep(), "two levels down counts");

    let step = net
        .step_deep()
        .expect("nothing fails")
        .expect("the luggage moves");
    assert_eq!(
        step,
        DeepStep::Inside {
            place: depot.id(),
            token: bus,
            net: "cabin".to_owned(),
            nested_id: cabin_id,
            step: Box::new(DeepStep::Inside {
                place: seats.id(),
                token: passenger,
                net: "luggage".to_owned(),
                nested_id: luggage_id,
                step: Box::new(DeepStep::Here { transition: stow }),
            }),
        }
    );
    assert_eq!(step.transition(), stow);

    let events: Vec<Event> = net.drain_events().collect();
    let [
        Event::Nested {
            place,
            token,
            net: outer_name,
            nested_id,
            event,
        },
    ] = &events[..]
    else {
        panic!("one doubly wrapped event: {events:?}");
    };
    assert_eq!(
        (*place, *token, outer_name.as_str(), *nested_id),
        (depot.id(), bus, "cabin", cabin_id)
    );
    let Event::Nested {
        place,
        token,
        net: inner_name,
        nested_id,
        event,
    } = &**event
    else {
        panic!("wrapped twice: {event:?}");
    };
    assert_eq!(
        (*place, *token, inner_name.as_str(), *nested_id),
        (seats.id(), passenger, "luggage", luggage_id)
    );
    assert!(matches!(**event, Event::Fired { transition, .. } if transition == stow));

    // The middle net's log was emptied on the way up.
    assert_eq!(inside(&net, depot.id()).state_counter(), 1);
    let luggage = inside(&net, depot.id())
        .nested(seats.id(), passenger)
        .expect("the passenger")
        .remove(0)
        .1;
    assert_eq!(luggage.count(stowed.id()), 1);
    assert!(net.is_quiescent_deep());
}

#[test]
fn the_concurrent_driver_reaches_deep_quiescence() {
    let mut b = NetBuilder::new();
    let desks = b.place_nested::<Net>("Desks");
    let shelf = b.place_nested::<Net>("Shelf");
    let inbox = b.place::<u32>("Inbox");
    let outbox = b.place::<u32>("Outbox");
    let Worker {
        net: inner,
        todo,
        done,
        ..
    } = worker(2);
    b.seed(desks, [inner, worker(1).net]);
    b.seed(inbox, [1, 2]);
    b.transition("Double", (TakeEdge(inbox),), (outbox,), |n: u32| (n * 2,));
    // Collect a desk once its work is done: an outer guard reading an inner
    // marking, so the outer sweep has to look again after inner firings.
    b.transition("Collect", (TakeEdge(desks),), (shelf,), |desk: Net| (desk,))
        .guard(move |net| {
            net.tokens(desks.id()).next().is_some_and(|(token, _)| {
                net.nested(desks.id(), token)
                    .is_some_and(|nets| nets[0].1.count(todo.id()) == 0)
            })
        });
    let mut net = b.build().expect("builds");

    let summary = drivers::run(
        &mut net,
        Mode::Concurrent,
        Limits::default(),
        OnFailure::Stop,
    )
    .expect("nothing fails");
    assert!(summary.quiesced);
    assert_eq!(
        summary.fired_count,
        2 + 3 + 2,
        "two doubles, three jobs, two collections"
    );
    assert!(net.is_quiescent_deep());
    assert!(net.in_flight().is_empty());
    assert_eq!(net.count(desks.id()), 0);
    assert_eq!(net.count(shelf.id()), 2);
    let done_counts: Vec<usize> = net
        .tokens(shelf.id())
        .map(|(token, _)| {
            net.nested(shelf.id(), token).expect("nested")[0]
                .1
                .count(done.id())
        })
        .collect();
    assert_eq!(done_counts.iter().sum::<usize>(), 3);
}

#[test]
fn run_indefinitely_steps_inner_nets_in_both_modes() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    for mode in [Mode::Sequential, Mode::Concurrent] {
        let mut b = NetBuilder::new();
        let desks = b.place_nested::<Net>("Desks");
        let Worker {
            net: inner, done, ..
        } = worker(2);
        b.seed(desks, [inner]);
        let mut net = b.build().expect("builds");

        let stop = AtomicBool::new(false);
        let summary = drivers::run_indefinitely(
            &mut net,
            mode,
            Duration::from_millis(1),
            &stop,
            OnFailure::Stop,
            |net| {
                if net.is_quiescent_deep() {
                    stop.store(true, Ordering::Relaxed);
                }
            },
        )
        .expect("nothing fails");
        assert!(summary.quiesced, "{mode:?}");
        assert_eq!(
            summary.fired_count, 2,
            "{mode:?}: one inner firing per tick"
        );
        assert_eq!(inside(&net, desks.id()).count(done.id()), 2);
    }
}

#[test]
fn limits_count_inner_launches() {
    let build = || {
        let mut b = NetBuilder::new();
        let desks = b.place_nested::<Net>("Desks");
        b.seed(desks, [worker(3).net]);
        b.build().expect("builds")
    };

    let mut net = build();
    let paced = Limits {
        stop_after_n_firings: Some(2),
        ..Limits::default()
    };
    let summary =
        drivers::run(&mut net, Mode::Sequential, paced, OnFailure::Stop).expect("nothing fails");
    assert_eq!((summary.quiesced, summary.fired_count), (false, 2));

    for mode in [Mode::Sequential, Mode::Concurrent] {
        let mut net = build();
        let fuse = Limits {
            error_after_n_firings: Some(2),
            ..Limits::default()
        };
        let error = drivers::run(&mut net, mode, fuse, OnFailure::Stop)
            .expect_err("three jobs, a fuse of two");
        assert!(
            matches!(&error, DriverError::TooManyFirings { name, summary, .. }
                if name == "Work" && summary.fired_count == 2),
            "{mode:?}: {error:?}"
        );
    }
}

// ----- reaching inside ----------------------------------------------------

#[test]
fn a_failing_body_restores_the_outer_token_and_leaves_its_inner_write_standing() {
    let Worker {
        net: cabin,
        todo: door,
        ..
    } = worker(0);
    let mut b = NetBuilder::new();
    let platform = b.place::<u32>("Platform");
    let depot = b.place_nested::<Bus>("Depot");
    b.seed(platform, [42]);
    b.seed(depot, [Bus { inner: cabin }]);
    let board = b
        .transition(
            "Board",
            (TakeEdge(platform), MutateEdge(depot)),
            (),
            move |passenger: u32, bus: &mut Bus| -> Result<(), Refused> {
                bus.inner.put(door, passenger);
                Err(Refused)
            },
        )
        .restore_tokens_on_failure()
        .id();
    let mut net = b.build().expect("builds");
    let waiting = only(&net, platform.id());
    let bus = only(&net, depot.id());

    let failure = net.step().expect_err("the body refuses");
    assert_eq!(failure.transition, board);
    assert!(failure.restored);
    assert!(failure.path.is_empty(), "the outer net's own transition");

    assert_eq!(only(&net, platform.id()), waiting, "back on the platform");
    let inner = inside(&net, depot.id());
    let at_door: Vec<u32> = inner
        .tokens(door.id())
        .filter_map(|(_, value)| value.downcast_ref::<u32>().copied())
        .collect();
    assert_eq!(at_door, [42], "the write inside was not rolled back");

    let events: Vec<Event> = net.drain_events().collect();
    assert_eq!(events.len(), 3, "{events:?}");
    assert!(
        matches!(unwrap(&events[0]), (Some((place, token, "cabin")), Event::Added { .. }) if place == depot.id() && token == bus)
    );
    assert_eq!(
        events[1],
        Event::Mutated {
            place: depot.id(),
            token: bus
        }
    );
    assert!(matches!(events[2], Event::Failed { restored: true, .. }));
}

#[test]
fn nested_scope_lifts_when_it_returns_and_only_what_changed() {
    let Worker {
        net: inner, todo, ..
    } = worker(0);
    let mut b = NetBuilder::new();
    let plain = b.place::<u32>("Plain");
    let depot = b.place_nested::<Bus>("Depot");
    b.seed(plain, [1]);
    b.seed(depot, [Bus { inner }]);
    let mut net = b.build().expect("builds");
    let bus = only(&net, depot.id());

    let names = net
        .nested_scope(depot.id(), bus, |nets| {
            nets.iter()
                .map(|(name, _)| name.to_string())
                .collect::<Vec<_>>()
        })
        .expect("nested");
    assert_eq!(names, ["cabin"]);
    assert_eq!(
        net.drain_events().count(),
        0,
        "nothing changed, nothing said"
    );
    assert_eq!(net.state_counter(), 0);

    let added = net
        .nested_scope(depot.id(), bus, |nets| {
            let cabin = &mut *nets[0].1;
            (cabin.put(todo, 1), cabin.put(todo, 2))
        })
        .expect("nested");
    let events: Vec<Event> = net.drain_events().collect();
    let added_inside: Vec<TokenId> = events
        .iter()
        .map(|event| match unwrap(event) {
            (Some((place, token, "cabin")), Event::Added { token: inner, .. })
                if place == depot.id() && token == bus =>
            {
                *inner
            }
            other => panic!("only lifted additions: {other:?}"),
        })
        .collect();
    assert_eq!(added_inside, [added.0, added.1]);
    assert_eq!(net.state_counter(), 2, "one tick per lifted event");

    let plain_token = only(&net, plain.id());
    assert!(net.nested_scope(plain.id(), plain_token, |_| ()).is_none());
    assert!(
        net.nested_scope(depot.id(), TokenId::new(99), |_| ())
            .is_none()
    );
}

// ----- the lifting rules --------------------------------------------------

#[test]
fn rule_1_an_inner_firing_is_lifted_at_once_under_where_the_token_sits() {
    let Worker {
        net: inner, work, ..
    } = worker(1);
    let inner_id = inner.id();
    let mut b = NetBuilder::new();
    let depot = b.place_nested::<Bus>("Depot");
    b.seed(depot, [Bus { inner }]);
    let mut net = b.build().expect("builds");
    let bus = only(&net, depot.id());

    net.step_deep().expect("nothing fails");
    assert_eq!(net.state_counter(), 1);
    let events: Vec<Event> = net.drain_events().collect();
    assert!(
        matches!(&events[..], [Event::Nested { place, token, net, nested_id, event }]
            if *place == depot.id() && *token == bus && net == "cabin" && *nested_id == inner_id
                && matches!(**event, Event::Fired { transition, .. } if transition == work)),
        "{events:?}"
    );
}

#[test]
fn rule_2_a_mutating_body_is_reported_inside_first_then_mutated() {
    for staged in [false, true] {
        let Yard {
            mut net,
            yard,
            tinker,
            ..
        } = yard();
        let bus = only(&net, yard.id());
        if staged {
            let firing = net.launch(tinker).expect("fireable");
            net.distribute(firing.run()).expect("nothing fails");
        } else {
            net.fire(tinker).expect("fireable");
        }

        let shapes: Vec<String> = net
            .drain_events()
            .map(|event| match unwrap(&event) {
                (Some((place, token, "cabin")), Event::Added { .. })
                    if place == yard.id() && token == bus =>
                {
                    "nested added".to_owned()
                }
                (None, Event::Mutated { place, token }) if *place == yard.id() && *token == bus => {
                    "mutated".to_owned()
                }
                (None, Event::Fired { .. }) => "fired".to_owned(),
                (None, Event::Launched { .. }) => "launched".to_owned(),
                (None, Event::Distributed { .. }) => "distributed".to_owned(),
                other => panic!("unexpected: {other:?}"),
            })
            .collect();
        let expected: &[&str] = if staged {
            &["launched", "nested added", "mutated", "distributed"]
        } else {
            &["nested added", "mutated", "fired"]
        };
        assert_eq!(shapes, expected);
    }
}

#[test]
fn rule_3_a_consumed_token_speaks_at_launch_under_its_old_id() {
    let Yard {
        mut net,
        yard,
        todo,
        tinker,
        scrap,
    } = yard();
    let bus = only(&net, yard.id());

    // Tinker's body has run and its write inside is not lifted yet.
    let tinkering = net.launch(tinker).expect("fireable");
    assert!(!tinkering.is_pending(), "a mutating edge runs inline");
    net.drain_events().for_each(drop);

    let scrapping = net.launch(scrap).expect("the bus is still in the yard");
    let events: Vec<Event> = net.drain_events().collect();
    assert_eq!(events.len(), 2, "{events:?}");
    assert!(matches!(
        unwrap(&events[0]),
        (Some((place, token, "cabin")), Event::Added { place: inner, .. })
            if place == yard.id() && token == bus && *inner == todo.id()
    ));
    assert_eq!(
        events[1],
        Event::Launched {
            transition: scrap,
            consumed: vec![(yard.id(), bus)],
        }
    );

    net.distribute(scrapping.run()).expect("nothing fails");
    net.distribute(tinkering.run()).expect("nothing fails");
}

#[test]
fn rule_4_a_produced_token_speaks_after_its_new_id_is_announced() {
    for staged in [false, true] {
        let Worker {
            net: inner, todo, ..
        } = worker(0);
        let mut b = NetBuilder::new();
        let yard = b.place_nested::<Bus>("Yard");
        let shop = b.place_nested::<Bus>("Shop");
        b.seed(yard, [Bus { inner }]);
        let paint = b
            .transition("Paint", (TakeEdge(yard),), (shop,), move |mut bus: Bus| {
                bus.inner.put(todo, 1);
                (bus,)
            })
            .id();
        let mut net = b.build().expect("builds");
        let old = only(&net, yard.id());

        if staged {
            let firing = net.launch(paint).expect("fireable");
            net.distribute(firing.run()).expect("nothing fails");
        } else {
            net.fire(paint).expect("fireable");
        }
        let new = only(&net, shop.id());
        assert_ne!(old, new);

        let events: Vec<Event> = net.drain_events().collect();
        let last = events.len() - 1;
        assert!(
            matches!(&events[last - 1], Event::Fired { produced, .. } | Event::Distributed { produced, .. }
                if *produced == [(shop.id(), new)]),
            "{events:?}"
        );
        assert!(matches!(
            unwrap(&events[last]),
            (Some((place, token, "cabin")), Event::Added { .. }) if place == shop.id() && token == new
        ));
        assert!(
            events[..last - 1]
                .iter()
                .all(|event| !matches!(event, Event::Nested { .. })),
            "nothing inside was said before the firing: {events:?}"
        );
    }
}

#[test]
fn rule_5_arriving_tokens_speak_after_and_leaving_ones_before() {
    let Yard {
        mut net,
        yard,
        todo,
        tinker,
        ..
    } = yard();

    // Arriving: a bus whose cabin already logged something.
    let Worker {
        net: mut cabin,
        todo: cabin_todo,
        ..
    } = worker(0);
    cabin.put(cabin_todo, 1);
    let arrived = net.put(yard, Bus { inner: cabin });
    let events: Vec<Event> = net.drain_events().collect();
    assert_eq!(
        events[0],
        Event::Added {
            place: yard.id(),
            token: arrived
        }
    );
    assert!(
        matches!(unwrap(&events[1]), (Some((_, token, "cabin")), Event::Added { .. }) if token == arrived)
    );
    assert_eq!(events.len(), 2);

    // Leaving: the front bus carries an unlifted write (see `yard`).
    let front = net.tokens(yard.id()).next().expect("the first bus").0;
    let tinkering = net.launch(tinker).expect("fireable");
    net.drain_events().for_each(drop);
    net.remove(yard.id(), front).expect("the bus is there");
    let events: Vec<Event> = net.drain_events().collect();
    assert!(matches!(
        unwrap(&events[0]),
        (Some((_, token, "cabin")), Event::Added { place, .. }) if token == front && *place == todo.id()
    ));
    assert_eq!(
        events[1],
        Event::Removed {
            place: yard.id(),
            token: front
        }
    );
    assert_eq!(events.len(), 2);
    net.distribute(tinkering.run()).expect("nothing fails");
    net.drain_events().for_each(drop);

    // Replacing: the old value's words, the `Mutated`, the new value's.
    let tinkering = net.launch(tinker).expect("the second bus is in front");
    net.drain_events().for_each(drop);
    let Worker {
        net: mut fresh,
        todo: fresh_todo,
        ..
    } = worker(0);
    fresh.put(fresh_todo, 9);
    net.replace(yard, arrived, Bus { inner: fresh })
        .expect("the bus is there");
    let shapes: Vec<&str> = net
        .drain_events()
        .map(|event| match event {
            Event::Nested { token, .. } if token == arrived => "nested",
            Event::Mutated { token, .. } if token == arrived => "mutated",
            other => panic!("unexpected: {other:?}"),
        })
        .collect();
    assert_eq!(shapes, ["nested", "mutated", "nested"]);
    net.distribute(tinkering.run()).expect("nothing fails");
    net.drain_events().for_each(drop);

    // And `set_tokens` is both: everything out, everything in.
    let Worker {
        net: mut fresh,
        todo: fresh_todo,
        ..
    } = worker(0);
    fresh.put(fresh_todo, 3);
    net.set_tokens(yard, [Bus { inner: fresh }]);
    let events: Vec<Event> = net.drain_events().collect();
    assert!(matches!(events[0], Event::Removed { token, .. } if token == arrived));
    let added = only(&net, yard.id());
    assert_eq!(
        events[1],
        Event::Added {
            place: yard.id(),
            token: added
        }
    );
    assert!(
        matches!(unwrap(&events[2]), (Some((_, token, "cabin")), Event::Added { .. }) if token == added)
    );
    assert_eq!(events.len(), 3);
}

// ----- failures -----------------------------------------------------------

/// A bus whose cabin has one job that fails, behind a `Depot`.
fn breakdown() -> (Net, PlaceRef<Bus>, TransitionId) {
    let mut cabin = NetBuilder::new();
    let todo = cabin.place::<u32>("Todo");
    cabin.seed(todo, [1]);
    let fail = cabin
        .transition(
            "Break",
            (TakeEdge(todo),),
            (),
            |_: u32| -> Result<(), Refused> { Err(Refused) },
        )
        .restore_tokens_on_failure()
        .id();
    let cabin = cabin.build().expect("the cabin builds");
    let mut b = NetBuilder::new();
    let depot = b.place_nested::<Bus>("Depot");
    b.seed(depot, [Bus { inner: cabin }]);
    (b.build().expect("builds"), depot, fail)
}

#[test]
fn an_inner_failure_says_where_it_happened() {
    let (mut net, depot, fail) = breakdown();
    let bus = only(&net, depot.id());

    let failure = net.step_deep().expect_err("the inner body refuses");
    assert_eq!(failure.transition, fail);
    assert_eq!(failure.name, "Break");
    assert_eq!(failure.path, [(depot.id(), bus, "cabin".to_owned())]);
    assert_eq!(
        failure.to_string(),
        format!(
            "transition \"Break\" ({fail:?}) inside {:?} #{bus} \"cabin\" failed: refused",
            depot.id()
        )
    );
    let events: Vec<Event> = net.drain_events().collect();
    assert!(matches!(
        unwrap(&events[0]),
        (Some((_, token, "cabin")), Event::Failed { restored: true, .. }) if token == bus
    ));
}

#[test]
fn drivers_stop_on_or_disable_an_inner_failure() {
    let (mut net, depot, fail) = breakdown();
    let error = drivers::run(
        &mut net,
        Mode::Sequential,
        Limits::default(),
        OnFailure::Stop,
    )
    .expect_err("Stop");
    assert!(
        matches!(&error, DriverError::FiringFailed { error, summary }
            if error.path.len() == 1 && summary.failed_count == 1),
        "{error:?}"
    );

    for mode in [Mode::Sequential, Mode::Concurrent] {
        let (mut net, depot, fail) = (breakdown().0, depot, fail);
        let summary = drivers::run(&mut net, mode, Limits::default(), OnFailure::Disable)
            .expect("Disable switches it off and carries on");
        assert!(summary.quiesced, "{mode:?}");
        assert_eq!(summary.failed_count, 1);
        assert!(
            !inside(&net, depot.id()).is_enabled(fail),
            "switched off inside"
        );
        let toggled = net.drain_events().any(|event| {
            matches!(unwrap(&event).1, Event::Toggled { transition, enabled: false } if *transition == fail)
        });
        assert!(toggled, "{mode:?}: the switch is reported, lifted");
    }
}

// ----- the wire -----------------------------------------------------------

#[cfg(feature = "serde")]
mod wire {
    use super::*;
    use petritype::serde_json::{self, json};

    #[test]
    fn a_nested_event_carries_its_inner_event_under_inner() {
        let Worker {
            net: inner, work, ..
        } = worker(1);
        let inner_id = inner.id();
        let mut b = NetBuilder::new();
        let depot = b.place_nested::<Bus>("Depot");
        b.seed(depot, [Bus { inner }]);
        let mut net = b.build().expect("builds");
        net.step_deep().expect("nothing fails");
        let event = net.drain_events().next().expect("one event");

        let Event::Nested { event: fired, .. } = &event else {
            panic!("{event:?}");
        };
        let Event::Fired {
            consumed, produced, ..
        } = &**fired
        else {
            panic!("{fired:?}");
        };
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            json!({
                "event": "nested",
                "place": depot.id(),
                "token": 0,
                "net": "cabin",
                "nested_id": inner_id.get(),
                "inner": {
                    "event": "fired",
                    "transition": work,
                    "consumed": consumed,
                    "produced": produced,
                },
            }),
        );
        assert_eq!(
            serde_json::to_value(inner_id).unwrap(),
            json!(inner_id.get())
        );
    }

    #[test]
    fn a_topology_marks_nested_and_passive_places_and_only_those() {
        let mut b = NetBuilder::new();
        let _plain = b.place::<u32>("Plain");
        let depot = b.place_nested::<Bus>("Depot");
        b.passive(depot);
        let net = b.build().expect("builds");
        assert_eq!(
            serde_json::to_value(net.topology()).unwrap()["places"],
            json!([
                {"id": 0, "name": "Plain", "type_name": "u32", "serializable": false},
                {"id": 1, "name": "Depot", "type_name": std::any::type_name::<Bus>(),
                 "serializable": false, "nested": true, "passive": true},
            ]),
        );
    }
}
