//! Integration tests for the stepping rule per place (DESIGN §6.6, §13).
//!
//! Each place that holds nets has one rule, [`Stepping`]: inner first (the
//! default), take turns, outer first, or passive. A deep step tries three
//! bands, inner-first nets, then the ring of the net and its take-turns
//! nets, then outer-first nets, and a member's turn is a deep step of its
//! own, so every level follows the rules of its own places. These tests are
//! the bands on one net, two levels with rules of their own, the ring and
//! its cursor, the default, a net that never comes to rest, the in-flight
//! rule of outer first, and the drivers. `tests/slice21.rs` holds the
//! evidence that one level of take turns steps as the interleaved policy
//! it replaced did; the transit example's tests, that outer first at every
//! place steps as the old default did.

use petritype::drivers::{self, DriverError, Limits, Mode, OnFailure};
use petritype::{
    DeepStep, Event, Net, NetBuilder, NetId, PlaceRef, StagedStep, Stepping, TakeEdge,
};

// ----- a few nets -----------------------------------------------------------------

/// A bare inner net with `jobs` things to do: `Todo -Work-> Done`.
fn worker(jobs: u32) -> Net {
    let mut b = NetBuilder::new();
    let todo = b.place::<u32>("Todo");
    let done = b.place::<u32>("Done");
    b.seed(todo, 0..jobs);
    b.transition("Work", (TakeEdge(todo),), (done,), |n: u32| (n,));
    b.build().expect("a worker")
}

/// An inner net that never comes to rest: `Spin` puts back what it takes.
fn spinner() -> Net {
    let mut b = NetBuilder::new();
    let wheel = b.place::<u32>("Wheel");
    b.seed(wheel, [0]);
    b.transition("Spin", (TakeEdge(wheel),), (wheel,), |n: u32| (n + 1,));
    b.build().expect("a spinner")
}

/// How much a worker has left to do.
fn todo(net: &Net) -> usize {
    net.count(net.place_named("Todo").expect("a worker has a Todo"))
}

/// The nets of the front tokens of `place`, one each, in token order.
fn nets_in(net: &Net, place: petritype::PlaceId) -> Vec<&Net> {
    net.tokens(place)
        .map(|(token, _)| net.nested(place, token).expect("a nested place")[0].1)
        .collect()
}

/// Where a deep step fired: the innermost nested id, `None` for the net
/// that was stepped.
fn fired_in(step: &DeepStep) -> Option<NetId> {
    match step {
        DeepStep::Here { .. } => None,
        DeepStep::Inside {
            nested_id, step, ..
        } => Some(fired_in(step).unwrap_or(*nested_id)),
    }
}

/// Where each of `n` deep steps fired, named by `names` (`"outer"` for the
/// net stepped, `"-"` for a step that fired nothing).
fn story(net: &mut Net, n: usize, names: &[(NetId, &'static str)]) -> Vec<&'static str> {
    (0..n)
        .map(|_| match net.step_deep().expect("nothing fails") {
            None => "-",
            Some(step) => match fired_in(&step) {
                None => "outer",
                Some(id) => {
                    names
                        .iter()
                        .find(|(named, _)| *named == id)
                        .expect("every net is named")
                        .1
                }
            },
        })
        .collect()
}

/// An outer net with `inbox` jobs of its own (`Inbox -Forward-> Outbox`)
/// and a `Desk` under `stepping` holding one worker with `jobs`.
struct Office {
    net: Net,
    desk: PlaceRef<Net>,
    inbox: PlaceRef<u32>,
    worker: NetId,
}

fn office(stepping: Option<Stepping>, inbox: u32, jobs: u32) -> Office {
    let mut b = NetBuilder::new();
    let desk = b.place_nested::<Net>("Desk");
    let tray = b.place::<u32>("Inbox");
    let outbox = b.place::<u32>("Outbox");
    let worker = worker(jobs);
    let id = worker.id();
    b.seed(desk, [worker]);
    b.seed(tray, 0..inbox);
    b.transition("Forward", (TakeEdge(tray),), (outbox,), |n: u32| (n,));
    if let Some(stepping) = stepping {
        b.stepping(desk, stepping);
    }
    Office {
        net: b.build().expect("an office"),
        desk,
        inbox: tray,
        worker: id,
    }
}

// ----- the default ----------------------------------------------------------------

#[test]
fn the_default_is_inner_first() {
    let mut o = office(None, 2, 2);
    assert_eq!(o.net.stepping(o.desk.id()), Some(Stepping::InnerFirst));
    assert_eq!(Stepping::default(), Stepping::InnerFirst);
    let names = [(o.worker, "worker")];
    assert_eq!(
        story(&mut o.net, 5, &names),
        ["worker", "worker", "outer", "outer", "-"],
        "the inner net that can fire goes before the outer transition that can"
    );
}

#[test]
fn inner_first_fires_one_transition_a_deep_step() {
    let mut o = office(Some(Stepping::InnerFirst), 0, 5);
    let step = o.net.step_deep().expect("nothing fails").expect("work");
    assert_eq!(fired_in(&step), Some(o.worker));
    let worker = nets_in(&o.net, o.desk.id())[0];
    assert_eq!(todo(worker), 4, "one firing, not a run to rest");
    assert_eq!(worker.fired_count(), 1);
}

#[test]
fn an_inner_net_that_never_rests_stops_the_net_around_it_under_inner_first_only() {
    let build = |stepping| {
        let mut b = NetBuilder::new();
        let desk = b.place_nested::<Net>("Desk");
        let inbox = b.place::<u32>("Inbox");
        let outbox = b.place::<u32>("Outbox");
        let spin = spinner();
        let id = spin.id();
        b.seed(desk, [spin]);
        b.seed(inbox, [1]);
        b.transition("Forward", (TakeEdge(inbox),), (outbox,), |n: u32| (n,));
        b.stepping(desk, stepping);
        (b.build().expect("builds"), id)
    };

    let (mut net, spin) = build(Stepping::InnerFirst);
    let names = [(spin, "spinner")];
    assert_eq!(
        story(&mut net, 20, &names),
        ["spinner"; 20],
        "the outer net never fires"
    );
    assert_eq!(net.fired_count(), 0);

    let (mut net, spin) = build(Stepping::TakeTurns);
    let names = [(spin, "spinner")];
    assert_eq!(
        story(&mut net, 4, &names),
        ["outer", "spinner", "spinner", "spinner"],
        "the outer net has its turn, and then nothing left to fire"
    );
    assert_eq!(net.fired_count(), 1);
}

// ----- the bands ------------------------------------------------------------------

/// One net with a place of each rule, declared in the opposite order to the
/// bands': `Late` (outer first) holding a worker with one job, `Ring` (take
/// turns) one with two, `Early` (inner first) one with two, and `Shelf`
/// (passive) one with five; and three jobs of its own.
struct Bands {
    net: Net,
    late: PlaceRef<Net>,
    early: PlaceRef<Net>,
    inbox: PlaceRef<u32>,
    names: Vec<(NetId, &'static str)>,
}

fn bands() -> Bands {
    let mut b = NetBuilder::new();
    let late = b.place_nested::<Net>("Late");
    let ring = b.place_nested::<Net>("Ring");
    let early = b.place_nested::<Net>("Early");
    let shelf = b.place_nested::<Net>("Shelf");
    let inbox = b.place::<u32>("Inbox");
    let outbox = b.place::<u32>("Outbox");
    let (l, t, e, s) = (worker(1), worker(2), worker(2), worker(5));
    let names = vec![
        (l.id(), "late"),
        (t.id(), "ring"),
        (e.id(), "early"),
        (s.id(), "shelf"),
    ];
    b.seed(late, [l]);
    b.seed(ring, [t]);
    b.seed(early, [e]);
    b.seed(shelf, [s]);
    b.seed(inbox, 0..3);
    b.transition("Forward", (TakeEdge(inbox),), (outbox,), |n: u32| (n,));
    b.stepping(late, Stepping::OuterFirst);
    b.stepping(ring, Stepping::TakeTurns);
    b.stepping(early, Stepping::InnerFirst);
    b.stepping(shelf, Stepping::Passive);
    Bands {
        net: b.build().expect("one net, four rules"),
        late,
        early,
        inbox,
        names,
    }
}

#[test]
fn a_deep_step_tries_inner_first_then_the_ring_then_outer_first() {
    let mut c = bands();
    assert_eq!(
        story(&mut c.net, 9, &c.names),
        [
            // The inner-first net until it is at rest.
            "early", "early",
            // The ring, from the net itself: it and the take-turns net in
            // turn, until both are at rest.
            "outer", "ring", "outer", "ring", "outer",
            // Then the outer-first net, and nothing: the shelf is passive.
            "late", "-",
        ],
    );
    assert!(
        c.net.is_quiescent_deep(),
        "the passive shelf does not count"
    );

    // Work everywhere again: each band in its order, the ring going on
    // after the net itself, which took its last turn.
    for place in [c.early, c.late] {
        let (token, _) = c.net.tokens(place.id()).next().expect("a worker");
        c.net
            .nested_scope(place.id(), token, |nets| {
                let todo = nets[0].1.place_ref::<u32>("Todo").expect("Todo");
                nets[0].1.put(todo, 9);
            })
            .expect("a nested place");
    }
    c.net.put(c.inbox, 9);
    assert_eq!(
        story(&mut c.net, 4, &c.names),
        ["early", "outer", "late", "-"]
    );
}

#[test]
fn step_inside_takes_the_bands_without_the_net_s_own_turn() {
    let mut c = bands();
    let mut order = Vec::new();
    while let Some(step) = c.net.step_inside().expect("nothing fails") {
        let id = fired_in(&step).expect("never the net itself");
        order.push(c.names.iter().find(|(named, _)| *named == id).unwrap().1);
    }
    assert_eq!(order, ["early", "early", "ring", "ring", "late"]);
    assert_eq!(c.net.count(c.inbox.id()), 3, "Forward never fired");
}

// ----- two levels -------------------------------------------------------------------

/// An outer net that can always fire, with a `Yard` under `outer` holding a
/// middle net, which has two jobs of its own and a `Held` place under
/// `middle` holding a worker with two.
fn two_levels(outer: Stepping, middle: Stepping) -> (Net, Vec<(NetId, &'static str)>) {
    let mut b = NetBuilder::new();
    let todo = b.place::<u32>("Todo");
    let done = b.place::<u32>("Done");
    let held = b.place_nested::<Net>("Held");
    let bottom = worker(2);
    let bottom_id = bottom.id();
    b.seed(todo, 0..2);
    b.seed(held, [bottom]);
    b.transition("Work", (TakeEdge(todo),), (done,), |n: u32| (n,));
    b.stepping(held, middle);
    let middle = b.build().expect("the middle net");
    let middle_id = middle.id();

    let mut b = NetBuilder::new();
    let wheel = b.place::<u32>("Wheel");
    let yard = b.place_nested::<Net>("Yard");
    b.seed(wheel, [0]);
    b.seed(yard, [middle]);
    b.transition("Spin", (TakeEdge(wheel),), (wheel,), |n: u32| (n + 1,));
    b.stepping(yard, outer);
    (
        b.build().expect("the outer net"),
        vec![(middle_id, "middle"), (bottom_id, "bottom")],
    )
}

#[test]
fn each_level_follows_the_rules_of_its_own_places() {
    // The outer net takes turns with the middle one, whose turn is its own
    // deep step: the bottom net first, inner first, until it rests.
    let (mut net, names) = two_levels(Stepping::TakeTurns, Stepping::InnerFirst);
    assert_eq!(
        story(&mut net, 10, &names),
        [
            "outer", "bottom", "outer", "bottom", "outer", "middle", "outer", "middle", "outer",
            "outer",
        ],
    );

    // The middle net goes first, inner first, and takes turns with the
    // bottom one; the outer net fires only once both are at rest.
    let (mut net, names) = two_levels(Stepping::InnerFirst, Stepping::TakeTurns);
    assert_eq!(
        story(&mut net, 6, &names),
        ["middle", "bottom", "middle", "bottom", "outer", "outer"],
    );

    // Outer first above and inner first below: the outer net can always
    // fire, so neither inner net ever has a turn.
    let (mut net, names) = two_levels(Stepping::OuterFirst, Stepping::InnerFirst);
    assert_eq!(story(&mut net, 4, &names), ["outer"; 4]);
}

// ----- the ring -----------------------------------------------------------------------

/// An outer net that can always fire, a take-turns `Ring` of two workers
/// with three jobs each, and an inner-first `Pen` it can move them to.
struct Carousel {
    net: Net,
    ring: PlaceRef<Net>,
    shelve: petritype::TransitionId,
    names: Vec<(NetId, &'static str)>,
}

fn carousel() -> Carousel {
    let mut b = NetBuilder::new();
    let wheel = b.place::<u32>("Wheel");
    let ring = b.place_nested::<Net>("Ring");
    let pen = b.place_nested::<Net>("Pen");
    let (a, c) = (worker(3), worker(3));
    let names = vec![(a.id(), "a"), (c.id(), "b")];
    b.seed(wheel, [0]);
    b.seed(ring, [a, c]);
    b.transition("Spin", (TakeEdge(wheel),), (wheel,), |n: u32| (n + 1,));
    let shelve = b
        .transition("Shelve", (TakeEdge(ring),), (pen,), |net: Net| (net,))
        .id();
    b.stepping(ring, Stepping::TakeTurns);
    let mut net = b.build().expect("a carousel");
    // Shelving is the test's to do, by name.
    net.disable(shelve).expect("Shelve exists");
    Carousel {
        net,
        ring,
        shelve,
        names,
    }
}

#[test]
fn the_ring_starts_at_the_net_and_goes_on_after_whoever_fired() {
    let mut c = carousel();
    assert_eq!(
        story(&mut c.net, 6, &c.names),
        ["outer", "a", "b", "outer", "a", "b"],
        "the net itself first, then each member after the last to fire"
    );
}

#[test]
fn a_member_that_moved_to_a_place_with_another_rule_has_left_the_ring() {
    let mut c = carousel();
    assert_eq!(story(&mut c.net, 2, &c.names), ["outer", "a"]);
    // `a` fired last. Move it to the inner-first pen: the ring has nowhere
    // to go on from but the start, and `a` now goes before everyone.
    c.net.enable(c.shelve).expect("Shelve exists");
    c.net.fire(c.shelve).expect("a is at the front of the ring");
    c.net.disable(c.shelve).expect("Shelve exists");
    assert_eq!(
        story(&mut c.net, 5, &c.names),
        ["a", "a", "outer", "b", "outer"]
    );
    assert_eq!(nets_in(&c.net, c.ring.id()).len(), 1);
}

#[cfg(feature = "serde")]
mod saved {
    use petritype::serde_json;
    use petritype::{Net, NetBuilder, Params, Stepping, TakeEdge, Templates};

    use super::{fired_in, todo};

    /// `Worker`, three jobs, and `Carousel`: `Spin` can always fire, beside
    /// a take-turns `Desk` holding a `Worker`.
    fn templates() -> Templates {
        fn worker(_: &Params) -> Result<Net, petritype::BuildError> {
            let mut b = NetBuilder::new();
            let todo = b.place::<u32>("Todo");
            let done = b.place::<u32>("Done");
            b.serializable(todo);
            b.serializable(done);
            b.seed(todo, 0..3);
            b.transition("Work", (TakeEdge(todo),), (done,), |n: u32| (n,));
            b.build()
        }
        let mut templates = Templates::new();
        templates.register("Worker", worker);
        templates.register(
            "Carousel",
            |_: &Params| -> Result<Net, Box<dyn std::error::Error + Send + Sync>> {
                let mut inner = Templates::new();
                inner.register("Worker", worker);
                let mut b = NetBuilder::new();
                b.templates(&inner);
                let wheel = b.place::<u32>("Wheel");
                let desk = b.place_nested::<Net>("Desk");
                b.serializable(wheel);
                b.serializable(desk);
                b.seed(wheel, [0]);
                b.seed(desk, [inner.build("Worker", &Params::new())?]);
                b.transition("Spin", (TakeEdge(wheel),), (wheel,), |n: u32| (n + 1,));
                b.stepping(desk, Stepping::TakeTurns);
                Ok(b.build()?)
            },
        );
        templates
    }

    #[test]
    fn the_ring_s_cursor_is_neither_saved_nor_forked() {
        let templates = templates();
        let mut net = templates.build("Carousel", &Params::new()).expect("built");
        let first = net.step_deep().expect("fires").expect("Spin");
        assert_eq!(fired_in(&first), None, "the net itself first");

        let fork = net.fork().expect("a fork");
        let saved = serde_json::to_value(&net).expect("a stamped net saves");
        let restored = templates.restore(saved).expect("and restores");
        for (mut copy, what) in [(fork, "forked"), (restored, "restored")] {
            let step = copy.step_deep().expect("fires").expect("Spin");
            assert_eq!(fired_in(&step), None, "{what}: the round starts afresh");
        }

        // The original keeps its cursor in memory: the worker's turn.
        let step = net.step_deep().expect("fires").expect("Work");
        assert!(fired_in(&step).is_some());
        let desk = net.place_named("Desk").expect("Desk");
        let (token, _) = net.tokens(desk).next().expect("the worker");
        assert_eq!(todo(net.nested(desk, token).expect("nested")[0].1), 2);
    }
}

#[test]
fn a_failed_turn_moves_the_cursor_and_a_set_rule_keeps_it() {
    // A take-turns worker whose one transition fails and puts its token
    // back, so it could fail for ever, beside an outer net with work.
    let mut inner = NetBuilder::new();
    let todo = inner.place::<u32>("Todo");
    let done = inner.place::<u32>("Done");
    inner.seed(todo, [1]);
    inner
        .transition(
            "Fail",
            (TakeEdge(todo),),
            (done,),
            |_: u32| -> Result<(u32,), std::fmt::Error> { Err(std::fmt::Error) },
        )
        .restore_tokens_on_failure();
    let inner = inner.build().expect("a failing worker");
    let mut o = office(Some(Stepping::TakeTurns), 3, 0);
    let (token, _) = o.net.tokens(o.desk.id()).next().expect("the idle worker");
    o.net.remove(o.desk.id(), token).expect("removable");
    o.net.put(o.desk, inner);

    assert!(matches!(o.net.step_deep(), Ok(Some(DeepStep::Here { .. }))));
    let failed = o.net.step_deep().expect_err("the worker's turn fails");
    assert_eq!(failed.path.len(), 1);
    // The failure was the worker's turn, so the next is the net's. A rule
    // set meanwhile, and set back, leaves the cursor where it was.
    for stepping in [Stepping::OuterFirst, Stepping::TakeTurns] {
        o.net
            .set_stepping(o.desk.id(), stepping)
            .expect("Desk holds nets");
    }
    assert!(matches!(o.net.step_deep(), Ok(Some(DeepStep::Here { .. }))));
    assert!(o.net.step_deep().is_err(), "and the worker's again");
}

// ----- outer first and a body in flight ----------------------------------------

#[test]
fn outer_first_waits_for_a_body_in_flight_and_the_other_rules_do_not() {
    for (stepping, goes) in [
        (Stepping::InnerFirst, true),
        (Stepping::TakeTurns, true),
        (Stepping::OuterFirst, false),
        (Stepping::Passive, false),
    ] {
        let mut o = office(Some(stepping), 1, 1);
        let forward = o.net.transition_named("Forward").expect("Forward");
        let firing = o.net.launch(forward).expect("Forward is fireable");
        let step = o.net.step_deep_staged().expect("nothing fails");
        assert_eq!(
            matches!(step, Some(StagedStep::Inside(_))),
            goes,
            "{stepping:?}, with Forward airborne: {step:?}"
        );
        o.net.distribute(firing.run()).expect("it lands");
        if stepping == Stepping::OuterFirst {
            assert!(
                matches!(o.net.step_deep_staged(), Ok(Some(StagedStep::Inside(_)))),
                "once it has landed, the outer-first worker's turn"
            );
        }
    }
}

// ----- the drivers --------------------------------------------------------------

/// What a concurrent run did, in log order: `F` for an outer firing that
/// landed, `W` for a firing inside.
fn concurrent_story(net: &mut Net) -> (String, drivers::RunSummary) {
    net.drain_events().for_each(drop);
    let summary = drivers::run(net, Mode::Concurrent, Limits::default(), OnFailure::Stop)
        .expect("nothing fails");
    let story = net
        .drain_events()
        .filter_map(|event| match event {
            Event::Distributed { .. } => Some('F'),
            Event::Nested { event, .. } if matches!(*event, Event::Fired { .. }) => Some('W'),
            _ => None,
        })
        .collect();
    (story, summary)
}

#[test]
fn the_concurrent_driver_follows_each_rule() {
    for (stepping, expected, left) in [
        (Stepping::InnerFirst, "WWFFF", 0),
        (Stepping::TakeTurns, "FWFWF", 0),
        (Stepping::OuterFirst, "FFFWW", 0),
        (Stepping::Passive, "FFF", 2),
    ] {
        let mut o = office(Some(stepping), 3, 2);
        let (story, summary) = concurrent_story(&mut o.net);
        assert_eq!(story, expected, "{stepping:?}");
        assert!(summary.quiesced, "{stepping:?}");
        assert_eq!(todo(nets_in(&o.net, o.desk.id())[0]), left, "{stepping:?}");
        assert_eq!(o.net.count(o.inbox.id()), 0);
    }
}

#[test]
fn the_fuse_blames_what_the_rule_would_try_next() {
    for (stepping, blamed) in [
        (Stepping::InnerFirst, "Work"),
        (Stepping::TakeTurns, "Work"),
        (Stepping::OuterFirst, "Forward"),
        (Stepping::Passive, "Forward"),
    ] {
        for mode in [Mode::Sequential, Mode::Concurrent] {
            let mut o = office(Some(stepping), 3, 3);
            let limits = Limits {
                error_after_n_firings: Some(1),
                ..Limits::default()
            };
            match drivers::run(&mut o.net, mode, limits, OnFailure::Stop) {
                Err(DriverError::TooManyFirings { name, .. }) => {
                    assert_eq!(name, blamed, "{stepping:?}, {mode:?}");
                }
                other => panic!("{stepping:?}, {mode:?}: the fuse, not {other:?}"),
            }
        }
    }
}
