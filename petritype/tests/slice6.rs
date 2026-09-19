//! Integration tests for the sixth slice: one firing in three stages, so that
//! a body can run somewhere else while the net stays the single writer.
//!
//! `launch` takes the tokens and the body out of the net, `run` calls the body
//! wherever it likes, and `distribute` hands the outputs — and the body — back.
//! Between the two the transition is *in flight*: its body slot is empty, which
//! is the whole of the mark, and which is why it cannot be launched twice and
//! why a peer that wanted the same token is no longer fireable.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, mpsc};

use petritype::{
    Completed, Distribution, Event, Firing, MutateEdge, Net, NetBuilder, NetError, PlaceId,
    PlaceRef, Routed, SnapshotEdge, TakeEdge, TokenId, TransitionId,
};

#[derive(Debug, Clone, PartialEq)]
struct Config {
    max_weight: f32,
}

/// The DESIGN §14 coloured balls: three output places of one type, and only
/// the ball's colour to choose between them.
#[derive(Debug, Clone, PartialEq)]
struct Ball {
    color: String,
}

/// Two payload types, so that a routed body has somewhere to send each.
#[derive(Debug, Clone, PartialEq, Routed)]
struct Small(u32);

#[derive(Debug, Clone, PartialEq, Routed)]
struct Large(u32);

#[derive(Routed)]
enum Sorted {
    Small(Small),
    Large(Large),
}

/// A body's failure, erased into [`petritype::FiringFailed`] by the engine.
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

fn ids(net: &Net, place: PlaceId) -> Vec<TokenId> {
    net.tokens(place).map(|(id, _)| id).collect()
}

/// Every token of a place as the `(place, token)` pairs an event carries.
fn pairs(net: &Net, place: PlaceId) -> Vec<(PlaceId, TokenId)> {
    net.tokens(place).map(|(id, _)| (place, id)).collect()
}

/// What the read API says about one transition.
fn info(net: &Net, transition: TransitionId) -> (bool, bool, bool) {
    let (_, info) = net
        .transitions()
        .find(|(id, _)| *id == transition)
        .expect("a transition of this net");
    (info.enabled, info.in_flight, info.detachable)
}

/// The smallest staged net: one token in, twice as much out.
fn doubling_net() -> (Net, TransitionId, PlaceRef<u32>, PlaceRef<u32>) {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [21]);
    let double = b
        .transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,))
        .id();
    let net = b.build().expect("a one-transition net");
    (net, double, inbox, out)
}

// ----- stage one -----------------------------------------------------------

#[test]
fn launch_consumes_its_inputs_now() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [21, 22]);
    let double = b
        .transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,))
        .id();
    let mut net = b.build().unwrap();
    let consumed = vec![pairs(&net, inbox.id())[0]];

    let firing = net.launch(double).expect("fireable");

    assert_eq!(firing.transition(), double);
    assert!(firing.is_pending(), "a detachable body has not been called");
    assert_eq!(
        values::<u32>(&net, inbox.id()),
        [22],
        "the front token left its place at launch, before the body ran"
    );
    assert_eq!(net.count(out.id()), 0, "nothing is produced yet");
    assert_eq!(net.fired_count(), 0, "a launch is not a firing");
    assert_eq!(net.state_counter(), 1, "one event, one tick");
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Launched {
            transition: double,
            consumed,
        }]
    );

    net.distribute(firing.run()).expect("its body comes home");
}

#[test]
fn a_launched_transition_is_in_flight_until_it_is_distributed() {
    let (mut net, double, _, _) = doubling_net();

    let firing = net.launch(double).expect("fireable");
    assert_eq!(net.in_flight(), [double]);
    assert!(net.is_in_flight(double));
    assert_eq!(
        info(&net, double),
        (true, true, true),
        "switched on, in flight, detachable"
    );

    let completed = firing.run();
    assert_eq!(completed.transition(), double);
    assert!(
        net.is_in_flight(double),
        "running the body changes nothing in the net"
    );

    net.distribute(completed).expect("in flight");
    assert!(net.in_flight().is_empty());
    assert!(!net.is_in_flight(double));
    assert_eq!(info(&net, double), (true, false, true));
}

#[test]
fn an_in_flight_transition_is_not_fireable_but_stays_switched_on() {
    let (mut net, double, inbox, _) = doubling_net();
    net.put(inbox, 1);

    let firing = net.launch(double).expect("fireable");

    assert!(
        !net.is_fireable(double),
        "its body is not in its slot, however full Inbox is"
    );
    assert!(net.fireable().is_empty());
    assert!(net.is_enabled(double), "in flight is not switched off");
    assert_eq!(
        net.count(inbox.id()),
        1,
        "a token is waiting for the next firing"
    );
    assert_eq!(net.step().expect("no body here fails"), None);

    net.distribute(firing.run()).unwrap();
    assert!(net.is_fireable(double), "and now it can fire again");
}

#[test]
fn launching_a_transition_that_cannot_fire_is_an_error() {
    let (mut net, double, _, _) = doubling_net();

    let firing = net.launch(double).expect("fireable");
    assert!(matches!(
        net.launch(double),
        Err(NetError::NotFireable { transition }) if transition == double
    ));
    assert!(matches!(
        net.fire(double),
        Err(NetError::NotFireable { transition }) if transition == double
    ));
    net.distribute(firing.run()).unwrap();

    // Empty input place, body home: not fireable for the ordinary reason.
    assert!(matches!(
        net.launch(double),
        Err(NetError::NotFireable { .. })
    ));
}

#[test]
fn launching_an_unknown_transition_is_an_error() {
    // The second transition of another net: an id this one-transition net has
    // never handed out, since ids are never reused within a net.
    let mut other = NetBuilder::new();
    let p = other.place::<u32>("P");
    other.transition("First", (), (p,), || (1_u32,));
    let stranger = other.transition("Second", (), (p,), || (2_u32,)).id();

    let (mut net, _, _, _) = doubling_net();
    assert!(matches!(
        net.launch(stranger),
        Err(NetError::NoSuchTransition { transition }) if transition == stranger
    ));
    assert!(net.in_flight().is_empty());
    assert_eq!(net.drain_events().count(), 0, "the net was not touched");
}

#[test]
fn a_snapshot_place_is_untouched_while_its_firing_is_airborne() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let config = b.place::<Config>("Config");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [2]);
    b.seed(config, [Config { max_weight: 10.0 }]);
    let scale = b
        .transition(
            "Scale",
            (TakeEdge(inbox), SnapshotEdge(config)),
            (out,),
            |n: u32, cfg: Config| (n * cfg.max_weight as u32,),
        )
        .id();
    let mut net = b.build().unwrap();
    let before = ids(&net, config.id());

    let firing = net.launch(scale).expect("fireable");
    assert_eq!(net.count(inbox.id()), 0, "the taking edge consumed");
    assert_eq!(
        ids(&net, config.id()),
        before,
        "the reading edge did not: the body took a clone with it"
    );
    assert_eq!(
        values::<Config>(&net, config.id()),
        [Config { max_weight: 10.0 }]
    );

    net.distribute(firing.run()).unwrap();
    assert_eq!(ids(&net, config.id()), before, "and still holds it");
    assert_eq!(values::<u32>(&net, out.id()), [20]);
}

// ----- stage three ---------------------------------------------------------

#[test]
fn distribute_lands_the_tokens_and_says_which() {
    let (mut net, double, _, out) = doubling_net();

    let firing = net.launch(double).expect("fireable");
    net.drain_events().for_each(drop);

    net.distribute(firing.run()).expect("in flight");

    assert_eq!(values::<u32>(&net, out.id()), [42]);
    let produced = pairs(&net, out.id());
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Distributed {
            transition: double,
            produced,
        }],
        "what landed, and not what was consumed: Launched said that"
    );
}

#[test]
fn fired_count_bumps_at_distribute_and_not_before() {
    let (mut net, double, _, _) = doubling_net();

    let firing = net.launch(double).expect("fireable");
    assert_eq!(net.fired_count(), 0, "launched");
    let completed = firing.run();
    assert_eq!(net.fired_count(), 0, "run, but nothing has landed");

    net.distribute(completed).unwrap();
    assert_eq!(net.fired_count(), 1, "a firing is one that finished");
}

#[test]
fn a_transition_is_home_again_after_it_is_distributed() {
    let (mut net, double, inbox, _) = doubling_net();
    net.put(inbox, 1);

    let firing = net.launch(double).expect("fireable");
    net.distribute(firing.run()).expect("in flight");

    // Distributing consumes the `Completed`, and only `launch` mints one, so
    // "distribute the same firing twice" cannot be written down. What would
    // answer it — `NetError::NotInFlight` — is checked in the crate's own
    // tests, where such a value can be built by hand. Here the observable
    // property is that the body really did come home.
    assert!(!net.is_in_flight(double));
    assert!(net.is_fireable(double));
    let again = net.launch(double).expect("fireable again");
    assert_eq!(again.transition(), double);
    net.distribute(again.run()).expect("in flight");
}

#[test]
fn a_firing_from_another_net_is_refused_and_changes_nothing() {
    let (mut here, double, _, out) = doubling_net();
    let (mut there, elsewhere, _, _) = doubling_net();

    let foreign = there.launch(elsewhere).expect("fireable").run();
    let net_id = there.id();
    here.drain_events().for_each(drop);

    match here.distribute(foreign) {
        Err(NetError::ForeignFiring { transition, from }) => {
            assert_eq!(transition, elsewhere);
            assert_eq!(from, net_id, "the net that launched it, named");
        }
        other => panic!("expected a foreign firing, got {other:?}"),
    }

    assert_eq!(here.count(out.id()), 0, "nothing was written");
    assert_eq!(here.fired_count(), 0);
    assert!(here.in_flight().is_empty(), "this net launched nothing");
    assert_eq!(here.drain_events().count(), 0);
    assert_eq!(
        there.in_flight(),
        [elsewhere],
        "and the net it belongs to keeps the transition in flight: the \
         rejected firing took the body with it"
    );

    // The net that was written to at least still works.
    assert_eq!(here.step().expect("no body here fails"), Some(double));
    assert_eq!(values::<u32>(&here, out.id()), [42]);
}

// ----- the body's failure --------------------------------------------------

#[test]
fn a_failing_body_is_reported_when_it_is_distributed() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [1, 2]);
    let fussy = b
        .transition(
            "Fussy",
            (TakeEdge(inbox),),
            (out,),
            |n: u32| -> Result<(u32,), Refused> { if n == 1 { Err(Refused) } else { Ok((n,)) } },
        )
        .id();
    let mut net = b.build().unwrap();
    let consumed = vec![pairs(&net, inbox.id())[0]];

    let completed = net.launch(fussy).expect("fireable").run();
    assert!(!completed.succeeded(), "the body returned Err");
    net.drain_events().for_each(drop);

    match net.distribute(completed) {
        Err(NetError::FiringFailed(failure)) => {
            assert_eq!(failure.transition, fussy);
            assert!(
                !failure.restored,
                "this transition does not restore its inputs"
            );
            assert_eq!(failure.error.to_string(), "refused");
        }
        other => panic!("expected a wrapped firing failure, got {other:?}"),
    }

    assert_eq!(net.count(inbox.id()), 1, "the consumed token is gone");
    assert_eq!(net.count(out.id()), 0, "nothing was distributed");
    assert_eq!(net.fired_count(), 0, "a failure is not a firing");
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Failed {
            transition: fussy,
            consumed,
            restored: false,
            error: "refused".to_owned(),
        }],
        "Failed and not Distributed"
    );

    // The body came home on the failing path too, so the next token fires.
    assert!(!net.is_in_flight(fussy));
    let firing = net.launch(fussy).expect("fireable");
    net.distribute(firing.run())
        .expect("the second token is acceptable");
    assert_eq!(values::<u32>(&net, out.id()), [2]);
    assert_eq!(net.fired_count(), 1);
}

// ----- inline firings ------------------------------------------------------

#[test]
fn a_mutating_transition_runs_inline_at_launch() {
    let mut b = NetBuilder::new();
    let ticks = b.place::<u32>("Ticks");
    let live = b.place::<u32>("Live");
    b.seed(ticks, [5]);
    b.seed(live, [10]);
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
    let mut net = b.build().unwrap();
    let mutated = pairs(&net, live.id());
    let consumed = pairs(&net, ticks.id());

    let firing = net.launch(bump).expect("fireable");

    assert!(
        !firing.is_pending(),
        "a transition with a mutating edge is not detachable: launch ran it"
    );
    assert_eq!(
        info(&net, bump),
        (true, true, false),
        "in flight all the same, and declared undetachable"
    );
    assert_eq!(
        values::<u32>(&net, live.id()),
        [15],
        "the mutation happened where the token lies, inside launch"
    );

    let completed = firing.run();
    assert!(completed.succeeded(), "run only unwraps an inline firing");
    net.distribute(completed).expect("in flight");

    assert_eq!(
        ids(&net, live.id()),
        vec![mutated[0].1],
        "same id, in place"
    );
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [
            Event::Launched {
                transition: bump,
                consumed,
            },
            Event::Mutated {
                place: live.id(),
                token: mutated[0].1,
            },
            Event::Distributed {
                transition: bump,
                produced: Vec::new(),
            },
        ],
        "Launched, then the mutations, then the event that ends the firing"
    );
}

// ----- conflict resolution -------------------------------------------------

#[test]
fn taking_the_tokens_at_launch_is_what_resolves_a_conflict() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let left = b.place::<u32>("Left");
    let right = b.place::<u32>("Right");
    b.seed(inbox, [1]);
    let first = b
        .transition("First", (TakeEdge(inbox),), (left,), |n: u32| (n,))
        .id();
    let second = b
        .transition("Second", (TakeEdge(inbox),), (right,), |n: u32| (n,))
        .id();
    let mut net = b.build().unwrap();
    assert_eq!(net.fireable(), [first, second], "both want the one token");

    let firing = net.launch(first).expect("fireable");

    assert!(
        net.fireable().is_empty(),
        "the token is airborne, so the peer has nothing to take"
    );
    assert!(!net.is_fireable(second));
    assert!(matches!(
        net.launch(second),
        Err(NetError::NotFireable { transition }) if transition == second
    ));

    net.distribute(firing.run()).unwrap();
    assert_eq!(values::<u32>(&net, left.id()), [1]);
    assert_eq!(net.count(right.id()), 0, "one token, one firing");
}

// ----- off the thread ------------------------------------------------------

/// `Firing` and `Completed` are `Send` whatever the transition: the body is
/// `Send + Sync` and the airborne inputs are owned tokens. A static
/// assertion — the property is checked when this file compiles.
#[test]
fn a_firing_and_a_completed_can_cross_threads() {
    const fn assert_send<T: Send>() {}
    assert_send::<Firing>();
    assert_send::<Completed>();
}

#[test]
fn three_firings_run_at_the_same_time_on_three_threads() {
    // Each body waits for the other two. If the three ran one after another
    // this would never return, so reaching the end is the proof that they
    // overlapped; `peak` says so in an assertion as well.
    let gate = Arc::new(Barrier::new(3));
    let live = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));

    let mut b = NetBuilder::new();
    let out = b.place::<u32>("Out");
    let mut launched = Vec::new();
    for n in 1..=3_u32 {
        let inbox = b.place::<u32>(format!("Inbox {n}"));
        b.seed(inbox, [n]);
        let gate = Arc::clone(&gate);
        let live = Arc::clone(&live);
        let peak = Arc::clone(&peak);
        launched.push(
            b.transition(
                format!("Work {n}"),
                (TakeEdge(inbox),),
                (out,),
                move |n: u32| {
                    let inside = live.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(inside, Ordering::SeqCst);
                    gate.wait();
                    live.fetch_sub(1, Ordering::SeqCst);
                    (n * 10,)
                },
            )
            .id(),
        );
    }
    let mut net = b.build().expect("three independent transitions");

    // The launch sweep: every token leaves its place before any body runs.
    let firings: Vec<Firing> = launched
        .iter()
        .map(|&t| net.launch(t).expect("fireable"))
        .collect();
    assert_eq!(net.in_flight(), launched, "all three are airborne");
    assert!(net.fireable().is_empty());

    let (sender, inbox) = mpsc::channel();
    let arrived: Vec<Completed> = std::thread::scope(|scope| {
        for firing in firings {
            let sender = sender.clone();
            scope.spawn(move || {
                sender
                    .send(firing.run())
                    .expect("the receiver outlives this scope");
            });
        }
        // The last sender, so that the iterator below ends when the workers do.
        drop(sender);
        inbox.iter().collect()
    });

    assert_eq!(peak.load(Ordering::SeqCst), 3, "three bodies at once");
    assert_eq!(
        net.in_flight(),
        launched,
        "the bodies have run, but nothing has landed: the net writes only here"
    );

    // In arrival order, whatever that turned out to be.
    for completed in arrived {
        net.distribute(completed).expect("in flight");
    }

    assert!(net.in_flight().is_empty());
    assert_eq!(net.fired_count(), 3);
    let mut landed = values::<u32>(&net, out.id());
    landed.sort_unstable();
    assert_eq!(landed, [10, 20, 30], "all three landed, in some order");
}

// ----- the sequential path is unchanged ------------------------------------

#[test]
fn step_still_reports_one_firing_as_one_fired_event() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let ticks = b.place::<u32>("Ticks");
    let live = b.place::<u32>("Live");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [21]);
    b.seed(ticks, [5]);
    b.seed(live, [10]);
    let double = b
        .transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,))
        .priority(|_net| 1.0)
        .id();
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
    let mut net = b.build().unwrap();
    let consumed = pairs(&net, inbox.id());
    let mutated = pairs(&net, live.id());
    let tick = pairs(&net, ticks.id());

    // A detachable transition, fired inline by `step`.
    assert_eq!(net.step().expect("no body here fails"), Some(double));
    assert!(net.in_flight().is_empty(), "step leaves nothing airborne");
    let produced = pairs(&net, out.id());
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Fired {
            transition: double,
            consumed,
            produced,
        }],
        "one Fired, and no Launched or Distributed"
    );

    // And one that is not detachable, which `step` fires the same way.
    assert_eq!(net.step().unwrap(), Some(bump));
    assert_eq!(values::<u32>(&net, live.id()), [15]);
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [
            Event::Mutated {
                place: live.id(),
                token: mutated[0].1,
            },
            Event::Fired {
                transition: bump,
                consumed: tick,
                produced: Vec::new(),
            },
        ]
    );
    assert_eq!(net.fired_count(), 2);
}

// ----- the other two output forms, staged ----------------------------------

/// The coloured balls, seeded with one paint of each colour.
fn balls_net() -> (Net, TransitionId, [PlaceId; 3]) {
    let mut b = NetBuilder::new();
    let paint = b.place::<String>("Paint");
    let red = b.place::<Ball>("Red");
    let blue = b.place::<Ball>("Blue");
    let other = b.place::<Ball>("Other");
    b.seed(
        paint,
        [
            String::from("red"),
            String::from("blue"),
            String::from("green"),
        ],
    );
    let make = b
        .transition(
            "Make a Ball",
            (TakeEdge(paint),),
            (red, blue, other),
            move |color: String| {
                let ball = Ball { color };
                let target = match ball.color.as_str() {
                    "red" => red,
                    "blue" => blue,
                    _ => other,
                };
                Distribution::new().to(target, ball)
            },
        )
        .id();
    let net = b.build().expect("a distribution needs no routing table");
    (net, make, [red.id(), blue.id(), other.id()])
}

#[test]
fn a_distribution_lands_where_it_says_through_the_stages() {
    let (mut staged, make, places) = balls_net();
    let (mut sequential, _, _) = balls_net();

    for _ in 0..3 {
        let firing = staged.launch(make).expect("fireable");
        staged.distribute(firing.run()).expect("in flight");
        assert_eq!(
            sequential.step().expect("this body cannot fail"),
            Some(make)
        );
    }

    for place in places {
        assert_eq!(
            values::<Ball>(&staged, place),
            values::<Ball>(&sequential, place),
            "the same balls in the same places, whichever path fired them"
        );
    }
    assert_eq!(
        values::<Ball>(&staged, places[0]),
        [Ball {
            color: String::from("red")
        }]
    );
    assert_eq!(staged.fired_count(), sequential.fired_count());
}

/// The routed form: no place is named and no slot is counted, so the payload's
/// own type picks its place — which stage three does exactly as `step` does.
fn sorting_net() -> (Net, TransitionId, [PlaceId; 2]) {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let small = b.place::<Small>("Small");
    let large = b.place::<Large>("Large");
    b.seed(inbox, [1, 100]);
    let sort = b
        .transition("Sort", (TakeEdge(inbox),), (small, large), |n: u32| {
            if n < 10 {
                Sorted::Small(Small(n))
            } else {
                Sorted::Large(Large(n))
            }
        })
        .id();
    let net = b.build().expect("one output place per payload type");
    (net, sort, [small.id(), large.id()])
}

#[test]
fn a_routed_body_lands_by_type_through_the_stages() {
    let (mut staged, sort, places) = sorting_net();
    let (mut sequential, _, _) = sorting_net();

    for _ in 0..2 {
        let firing = staged.launch(sort).expect("fireable");
        staged.distribute(firing.run()).expect("in flight");
        assert_eq!(
            sequential.step().expect("this body cannot fail"),
            Some(sort)
        );
    }

    assert_eq!(values::<Small>(&staged, places[0]), [Small(1)]);
    assert_eq!(values::<Large>(&staged, places[1]), [Large(100)]);
    assert_eq!(
        values::<Small>(&staged, places[0]),
        values::<Small>(&sequential, places[0])
    );
    assert_eq!(
        values::<Large>(&staged, places[1]),
        values::<Large>(&sequential, places[1])
    );
}
