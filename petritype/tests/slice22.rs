//! Integration tests for the twenty-second slice: input edges that accept an
//! empty place (DESIGN §4.4, §6.1;
//! `dev-docs/EDGES_ON_EMPTY_PLACES.md`).
//!
//! A `DrainEdge` takes, and a `SurveyEdge` reads a clone of, every token in
//! its place, zero or more, and neither holds its transition back. What is
//! tested here is that promise — fireability with the place empty or full,
//! the tokens handed over, the events, the restore, detachability — and the
//! edges' place in everything else that reads edges: the build checks, the
//! two opt-in checks, the read API and its serialised form, and the drivers.
//!
//! The proposal's example was one shop that surveyed and drained its
//! promotions; since 2026-10-16 it is two nets, as the examples are. The
//! survey is tested on the shop, which prices each order with whatever
//! promotions are running (`the_shop_runs_the_script_to_the_table`); the
//! drain on the traffic light, whose `Turn Green` takes every car waiting at
//! red, possibly none (`the_traffic_light_runs_the_script`). The examples
//! `examples/shop.rs` and `examples/traffic_light.rs` carry the same nets as
//! the protocol's conformance nets `shop` and `traffic-light`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use petritype::drivers::{self, Limits, Mode, OnFailure};
use petritype::{
    BuildError, DrainEdge, EdgeInfo, EdgeKind, Event, Filter, Nested, Net, NetBuilder, NetError,
    PlaceRef, SnapshotEdge, SurveyEdge, TakeAllEdge, TakeEdge, TokenId, TransitionId,
};

#[derive(Debug)]
struct Broken;

impl std::fmt::Display for Broken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the signal jammed")
    }
}

impl std::error::Error for Broken {}

/// The shop's places and transition.
struct Shop {
    orders: PlaceRef<String>,
    promotions: PlaceRef<String>,
    priced: PlaceRef<String>,
    discounts: PlaceRef<String>,
    price_order: TransitionId,
}

/// The shop of `examples/shop/net.rs`, declared into `b`, nothing seeded:
/// four places of strings, `Price Order` taking an order and surveying the
/// promotions.
fn declare_shop(b: &mut NetBuilder) -> Shop {
    let orders = b.place::<String>("Orders");
    let promotions = b.place::<String>("Promotions");
    let priced = b.place::<String>("Priced");
    let discounts = b.place::<String>("Discounts Applied");
    let price_order = b
        .transition(
            "Price Order",
            (TakeEdge(orders), SurveyEdge(promotions)),
            (priced, discounts),
            |order: String, promotions: Vec<String>| {
                let applied: Vec<String> = promotions
                    .iter()
                    .map(|promotion| format!("{promotion} on {order}"))
                    .collect();
                (order, applied)
            },
        )
        .id();
    Shop {
        orders,
        promotions,
        priced,
        discounts,
        price_order,
    }
}

/// The shop, built.
fn shop() -> (Net, Shop) {
    let mut b = NetBuilder::new();
    let shop = declare_shop(&mut b);
    (b.build().expect("the shop builds"), shop)
}

/// The traffic light's places and transitions.
struct Junction {
    light: PlaceRef<String>,
    waiting: PlaceRef<String>,
    passed: PlaceRef<String>,
    turn_green: TransitionId,
    pass_on_green: TransitionId,
    change: TransitionId,
}

/// The light's colour, as the guards read it: the front token of `Light`.
fn colour(net: &Net, light: PlaceRef<String>) -> Option<&str> {
    let (token, _) = net.tokens(light.id()).next()?;
    net.token::<String>(light.id(), token).map(String::as_str)
}

/// The colour after `colour`: red, green, amber, red.
fn next_colour(colour: &str) -> String {
    match colour {
        "red" => "green",
        "green" => "amber",
        _ => "red",
    }
    .to_owned()
}

/// The traffic light of `examples/traffic_light/net.rs`, declared into `b`,
/// `"red"` seeded in `Light`: `Turn Green`, guarded on red, takes the light
/// and drains `Waiting`, writing green back to `Light` and the cars to
/// `Passed`; `Pass on Green`, guarded on green, takes a car and snapshots
/// `Light`; `Change`, guarded on anything but red, writes the next colour
/// back. With `jammed`, `Turn Green` fails and restores what it took.
fn declare_light(b: &mut NetBuilder, jammed: bool) -> Junction {
    let light = b.place::<String>("Light");
    let waiting = b.place::<String>("Waiting");
    let passed = b.place::<String>("Passed");
    b.seed(light, [s("red")]);
    let mut turn = b.transition(
        "Turn Green",
        (TakeEdge(light), DrainEdge(waiting)),
        (light, passed),
        move |light: String, cars: Vec<String>| -> Result<(String, Vec<String>), Broken> {
            if jammed {
                return Err(Broken);
            }
            Ok((next_colour(&light), cars))
        },
    );
    turn.guard(move |net| colour(net, light) == Some("red"));
    if jammed {
        turn.restore_tokens_on_failure();
    }
    let turn_green = turn.id();
    let pass_on_green = b
        .transition(
            "Pass on Green",
            (TakeEdge(waiting), SnapshotEdge(light)),
            (passed,),
            |car: String, _light: String| (car,),
        )
        .guard(move |net| colour(net, light) == Some("green"))
        .id();
    let change = b
        .transition("Change", (TakeEdge(light),), (light,), |l: String| {
            (next_colour(&l),)
        })
        .guard(move |net| colour(net, light).is_some_and(|colour| colour != "red"))
        .id();
    Junction {
        light,
        waiting,
        passed,
        turn_green,
        pass_on_green,
        change,
    }
}

/// The traffic light, built.
fn junction() -> (Net, Junction) {
    let mut b = NetBuilder::new();
    let junction = declare_light(&mut b, false);
    (b.build().expect("the light builds"), junction)
}

fn s(text: &str) -> String {
    text.to_owned()
}

/// The strings in a place, in queue order.
fn strings(net: &Net, place: PlaceRef<String>) -> Vec<String> {
    net.tokens(place.id())
        .map(|(id, _)| net.token::<String>(place.id(), id).unwrap().clone())
        .collect()
}

/// The ids of a place's tokens, in queue order.
fn ids<T>(net: &Net, place: PlaceRef<T>) -> Vec<TokenId> {
    net.tokens(place.id()).map(|(id, _)| id).collect()
}

/// The shop's table: after each of its script's two steps, `Priced`,
/// `Discounts Applied` and `Promotions`.
const TABLE: [[&[&str]; 3]; 2] = [
    // Price Order, no promotions.
    [&["Order 1"], &[], &[]],
    // Price Order, two running.
    [
        &["Order 1", "Order 2"],
        &["10% off on Order 2", "Free shipping on Order 2"],
        &["10% off", "Free shipping"],
    ],
];

/// `Priced`, `Discounts Applied` and `Promotions`, as the table has them.
fn row(net: &Net, shop: &Shop) -> [Vec<String>; 3] {
    [
        strings(net, shop.priced),
        strings(net, shop.discounts),
        strings(net, shop.promotions),
    ]
}

// ----- the acceptance tests -----------------------------------------------------

/// The shop's script (the shared transcript `07_empty_place_edges.json`),
/// stepped with the default selector, and its table after each step: the
/// survey reads the promotions and leaves them running.
#[test]
fn the_shop_runs_the_script_to_the_table() {
    let (mut net, shop) = shop();

    net.put(shop.orders, s("Order 1"));
    assert_eq!(net.step().unwrap(), Some(shop.price_order));
    assert_eq!(row(&net, &shop), TABLE[0]);

    net.extend(shop.promotions, [s("10% off"), s("Free shipping")]);
    assert!(net.fireable().is_empty(), "promotions alone fire nothing");
    net.put(shop.orders, s("Order 2"));
    let running = ids(&net, shop.promotions);
    assert_eq!(net.step().unwrap(), Some(shop.price_order));
    assert_eq!(row(&net, &shop), TABLE[1]);
    assert_eq!(ids(&net, shop.promotions), running, "surveyed, not touched");

    assert_eq!(net.step().unwrap(), None, "quiescent");
    assert_eq!(net.fired_count(), 2);
}

/// The traffic light's script (the shared transcript
/// `08_traffic_light.json`), stepped with the default selector: the light
/// turns green with nobody waiting, two cars wait through amber and red and
/// are drained the moment it turns green again, a third arrives at green and
/// passes on its own, and the light never stops. The light is one token in
/// `Light`, its colour, and a fresh one after every `Turn Green` and
/// `Change`.
#[test]
fn the_traffic_light_runs_the_script() {
    let (mut net, junction) = junction();

    assert_eq!(
        net.step().unwrap(),
        Some(junction.turn_green),
        "nobody waiting"
    );
    assert_eq!(strings(&net, junction.light), ["green"]);
    assert!(
        strings(&net, junction.passed).is_empty(),
        "the drain took none"
    );
    assert_eq!(net.fireable(), [junction.change]);
    assert_eq!(net.step().unwrap(), Some(junction.change));
    assert_eq!(strings(&net, junction.light), ["amber"]);

    net.extend(junction.waiting, [s("Car 1"), s("Car 2")]);
    assert_eq!(net.fireable(), [junction.change], "the cars wait at amber");
    assert_eq!(net.step().unwrap(), Some(junction.change));
    assert_eq!(strings(&net, junction.light), ["red"]);
    assert_eq!(net.fireable(), [junction.turn_green], "and at red");
    assert_eq!(net.step().unwrap(), Some(junction.turn_green));
    assert_eq!(strings(&net, junction.passed), ["Car 1", "Car 2"]);
    assert_eq!(net.count(junction.waiting.id()), 0, "everyone waiting went");

    net.put(junction.waiting, s("Car 3"));
    assert_eq!(
        net.fireable(),
        [junction.pass_on_green, junction.change],
        "the car before the light changes"
    );
    let lamp = ids(&net, junction.light);
    assert_eq!(net.step().unwrap(), Some(junction.pass_on_green));
    assert_eq!(ids(&net, junction.light), lamp, "read, not taken");
    assert_eq!(strings(&net, junction.light), ["green"]);
    assert_eq!(net.step().unwrap(), Some(junction.change));

    assert_eq!(strings(&net, junction.light), ["amber"]);
    assert_eq!(strings(&net, junction.passed), ["Car 1", "Car 2", "Car 3"]);
    assert_eq!(net.count(junction.waiting.id()), 0);
    assert_eq!(net.fired_count(), 6);
    assert_eq!(net.fireable(), [junction.change], "never quiescent");
}

// ----- fireability --------------------------------------------------------------

/// With `Orders` empty `Price Order` is not fireable, however many
/// promotions are running; the survey edge is never what holds it back.
#[test]
fn price_order_waits_for_an_order_whatever_the_promotions() {
    let (mut net, shop) = shop();
    assert!(!net.is_fireable(shop.price_order));
    for promotion in ["10% off", "Free shipping", "Two for one"] {
        net.put(shop.promotions, s(promotion));
        assert!(!net.fireable().contains(&shop.price_order));
        assert!(!net.is_fireable(shop.price_order));
    }
    assert!(net.fireable().is_empty(), "no order");

    net.put(shop.orders, s("Order 1"));
    assert_eq!(net.fireable(), [shop.price_order]);
    net.set_tokens(shop.promotions, Vec::<String>::new());
    assert_eq!(net.fireable(), [shop.price_order], "none running is fine");
}

/// A transition whose only inputs are a drain and a survey is fireable on
/// two empty places, and its guard can still hold it back.
#[test]
fn an_empty_place_holds_nothing_back_and_a_guard_still_does() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let notes = b.place::<u32>("Notes");
    let sums = b.place::<usize>("Sums");
    let open = Arc::new(AtomicUsize::new(1));
    let gate = Arc::clone(&open);
    let sum = b
        .transition(
            "Sum",
            (DrainEdge(inbox), SurveyEdge(notes)),
            (sums,),
            |batch: Vec<u32>, read: Vec<u32>| (batch.len() + read.len(),),
        )
        .guard(move |_| gate.load(Ordering::SeqCst) == 1)
        .id();
    let mut net = b.build().unwrap();

    assert_eq!(net.fireable(), [sum], "two empty places, and fireable");
    open.store(0, Ordering::SeqCst);
    assert!(net.fireable().is_empty(), "the guard says no");
    net.extend(inbox, [1, 2]);
    assert!(!net.is_fireable(sum), "and tokens do not change its mind");

    open.store(1, Ordering::SeqCst);
    net.fire(sum).unwrap();
    net.fire(sum).unwrap();
    let totals: Vec<usize> = net
        .tokens(sums.id())
        .map(|(id, _)| *net.token::<usize>(sums.id(), id).unwrap())
        .collect();
    assert_eq!(totals, [2, 0], "two tokens, then none");
}

/// A drain of a keyed place leaves the index as a take-all does, and a
/// survey leaves it alone.
#[test]
fn a_drained_keyed_place_is_empty_to_the_index_too() {
    let mut b = NetBuilder::new();
    let shelf = b.place::<String>("Shelf");
    let read = b.place::<String>("Read");
    let out = b.place::<usize>("Out");
    for place in [shelf, read] {
        b.keys(place, ["name"], |name: &String| [name.as_str().into()]);
    }
    b.seed(shelf, [s("Tea"), s("Jam")]);
    b.seed(read, [s("Map")]);
    b.transition(
        "Clear",
        (DrainEdge(shelf), SurveyEdge(read)),
        (out,),
        |all: Vec<String>, seen: Vec<String>| (all.len() + seen.len(),),
    );
    let mut net = b.build().unwrap();
    net.step().unwrap();

    let anything = Filter::new();
    assert_eq!(net.first_match(shelf.id(), &anything), None);
    assert!(net.distinct_values("name", Some(&[shelf.id()])).is_empty());
    assert_eq!(
        net.distinct_values("name", None),
        [petritype::KeyValue::from("Map")],
        "the surveyed token is still indexed"
    );
}

// ----- the tokens handed over, and the events -----------------------------------

/// A survey lists none of what it read in the firing's `consumed`, full or
/// empty.
#[test]
fn consumed_lists_nothing_a_survey_read() {
    let (mut net, shop) = shop();
    for running in [&[][..], &[s("10% off"), s("Free shipping")][..]] {
        net.extend(shop.promotions, running.to_vec());
        let order = net.put(shop.orders, s("Order 1"));
        net.drain_events().for_each(drop);
        net.step().unwrap();
        let events: Vec<Event> = net.drain_events().collect();
        assert!(
            matches!(&events[..], [Event::Fired { transition, consumed, produced }]
                if *transition == shop.price_order
                    && consumed == &[(shop.orders.id(), order)]
                    && produced.len() == 1 + running.len()),
            "the survey consumed nothing: {events:?}"
        );
    }
}

/// A drain lists every token it took in the firing's `consumed`, after the
/// light and oldest first, which from an empty place is none.
#[test]
fn consumed_lists_what_a_drain_took_oldest_first() {
    let (mut net, junction) = junction();
    let lamp = ids(&net, junction.light)[0];
    net.drain_events().for_each(drop);

    net.step().unwrap();
    let events: Vec<Event> = net.drain_events().collect();
    assert!(
        matches!(&events[..], [Event::Fired { transition, consumed, produced }]
            if *transition == junction.turn_green
                && consumed == &[(junction.light.id(), lamp)]
                && produced.len() == 1
                && produced[0].0 == junction.light.id()),
        "nothing from the empty Waiting: {events:?}"
    );

    assert_eq!(net.step().unwrap(), Some(junction.change));
    let cars = net.extend(junction.waiting, [s("Car 1"), s("Car 2")]);
    assert_eq!(net.step().unwrap(), Some(junction.change));
    let lamp = ids(&net, junction.light)[0];
    net.drain_events().for_each(drop);
    net.step().unwrap();
    let events: Vec<Event> = net.drain_events().collect();
    let expected: Vec<_> = [(junction.light.id(), lamp)]
        .into_iter()
        .chain(cars.iter().map(|&id| (junction.waiting.id(), id)))
        .collect();
    assert!(
        matches!(&events[..], [Event::Fired { consumed, .. }] if consumed == &expected),
        "the light, then the cars oldest first: {events:?}"
    );
}

/// The staged path says the same in its `Launched`: no surveyed token.
#[test]
fn launched_names_no_surveyed_token() {
    let (mut net, shop) = shop();
    net.extend(shop.promotions, [s("10% off")]);
    let order = net.put(shop.orders, s("Order 1"));
    net.drain_events().for_each(drop);

    let firing = net.launch(shop.price_order).unwrap();
    assert!(matches!(
        net.drain_events().collect::<Vec<_>>()[..],
        [Event::Launched { ref consumed, .. }] if consumed == &[(shop.orders.id(), order)]
    ));
    net.distribute(firing.run()).unwrap();
    assert_eq!(strings(&net, shop.discounts), ["10% off on Order 1"]);
}

/// The staged path says the same in its `Launched`: every drained token,
/// taken from the place at launch. A drain borrows nothing, so the firing is
/// detachable.
#[test]
fn launched_names_the_drained_tokens() {
    let (mut net, junction) = junction();
    let detachable = |net: &Net, transition: TransitionId| {
        net.transitions()
            .find(|(id, _)| *id == transition)
            .unwrap()
            .1
            .detachable
    };
    assert!(
        detachable(&net, junction.turn_green),
        "a drain borrows nothing"
    );
    assert!(
        detachable(&net, junction.pass_on_green),
        "nor does a snapshot"
    );

    let lamp = ids(&net, junction.light)[0];
    let car = net.put(junction.waiting, s("Car 1"));
    net.drain_events().for_each(drop);
    let firing = net.launch(junction.turn_green).unwrap();
    assert!(firing.is_pending(), "not run inline");
    assert_eq!(net.count(junction.waiting.id()), 0, "drained at launch");
    assert_eq!(net.count(junction.light.id()), 0, "the light taken too");
    assert!(matches!(
        net.drain_events().collect::<Vec<_>>()[..],
        [Event::Launched { ref consumed, .. }]
            if consumed == &[(junction.light.id(), lamp), (junction.waiting.id(), car)]
    ));
    net.distribute(firing.run()).unwrap();
    assert_eq!(strings(&net, junction.passed), ["Car 1"]);
    assert_eq!(strings(&net, junction.light), ["green"]);
}

// ----- detachable -----------------------------------------------------------------

/// A survey hands over clones taken at launch and borrows nothing, so its
/// firing leaves the net: it is detachable, runs on another thread, and
/// sees the promotions as they were when it started, whatever happens to
/// the place meanwhile.
#[test]
fn a_survey_keeps_the_firing_detachable_and_sees_the_launch_marking() {
    let (mut net, shop) = shop();
    let detachable = net
        .transitions()
        .find(|(id, _)| *id == shop.price_order)
        .unwrap()
        .1
        .detachable;
    assert!(detachable, "a survey borrows nothing");

    let running = net.extend(shop.promotions, [s("10% off"), s("Free shipping")]);
    net.put(shop.orders, s("Order 1"));
    let firing = net.launch(shop.price_order).unwrap();
    assert!(firing.is_pending(), "not run inline");
    assert_eq!(ids(&net, shop.promotions), running, "nothing moved");

    // Meanwhile the first promotion ends and a third starts.
    net.remove(shop.promotions.id(), running[0]).unwrap();
    net.put(shop.promotions, s("Two for one"));

    let completed = std::thread::spawn(move || firing.run()).join().unwrap();
    net.distribute(completed).unwrap();
    assert_eq!(
        strings(&net, shop.discounts),
        ["10% off on Order 1", "Free shipping on Order 1"],
        "the promotions at launch"
    );
}

/// A transition whose reads are snapshots and surveys, and nothing that
/// borrows, runs under the concurrent driver, its firings on worker
/// threads.
#[test]
fn the_concurrent_driver_runs_the_shop() {
    let (mut net, shop) = shop();
    net.extend(shop.promotions, [s("10% off")]);
    net.extend(shop.orders, [s("Order 1"), s("Order 2"), s("Order 3")]);

    let summary = drivers::run(
        &mut net,
        Mode::Concurrent,
        Limits::default(),
        OnFailure::Stop,
    )
    .unwrap();
    assert!(summary.quiesced);
    assert_eq!(summary.fired_count, 3);
    assert_eq!(
        strings(&net, shop.priced),
        ["Order 1", "Order 2", "Order 3"]
    );
    assert_eq!(
        strings(&net, shop.discounts),
        [
            "10% off on Order 1",
            "10% off on Order 2",
            "10% off on Order 3"
        ]
    );
    assert_eq!(strings(&net, shop.promotions), ["10% off"]);
}

// ----- failure --------------------------------------------------------------------

/// A failing `Turn Green` that restores its inputs puts the cars back at the
/// front of `Waiting`, oldest first, with their ids, ahead of one that
/// arrived while the firing was in flight; the light goes back to `Light`,
/// still red, under its id, and no green is written.
#[test]
fn a_failing_turn_green_restores_the_cars_oldest_first_with_their_ids() {
    let mut b = NetBuilder::new();
    let junction = declare_light(&mut b, true);
    let mut net = b.build().unwrap();
    let lamp = ids(&net, junction.light)[0];
    let cars = net.extend(junction.waiting, [s("Car 1"), s("Car 2")]);
    net.drain_events().for_each(drop);

    let firing = net.launch(junction.turn_green).unwrap();
    let late = net.put(junction.waiting, s("Car 3"));
    let failure = net.distribute(firing.run()).expect_err("the signal jams");
    assert!(matches!(failure, NetError::FiringFailed(ref f) if f.restored));

    let events: Vec<Event> = net.drain_events().collect();
    assert!(
        matches!(events.last(), Some(Event::Failed { transition, consumed, restored: true, error })
            if *transition == junction.turn_green
                && consumed == &[
                    (junction.light.id(), lamp),
                    (junction.waiting.id(), cars[0]),
                    (junction.waiting.id(), cars[1]),
                ]
                && error == "the signal jammed"),
        "{events:?}"
    );
    assert_eq!(
        ids(&net, junction.waiting),
        [cars[0], cars[1], late],
        "back at the front, oldest first, ids kept"
    );
    assert_eq!(strings(&net, junction.waiting), ["Car 1", "Car 2", "Car 3"]);
    assert_eq!(ids(&net, junction.light), [lamp]);
    assert_eq!(strings(&net, junction.light), ["red"]);
    assert!(net.tokens(junction.passed.id()).next().is_none());

    // With nobody waiting, the failing firing has only the light to put
    // back.
    net.set_tokens(junction.waiting, Vec::<String>::new());
    net.drain_events().for_each(drop);
    net.fire(junction.turn_green).expect_err("still jammed");
    assert!(matches!(
        net.drain_events().collect::<Vec<_>>()[..],
        [Event::Failed { ref consumed, restored: true, .. }]
            if consumed == &[(junction.light.id(), lamp)]
    ));
    assert_eq!(ids(&net, junction.light), [lamp]);
    assert_eq!(strings(&net, junction.light), ["red"]);
}

// ----- build checks ---------------------------------------------------------------

/// A survey clones every token, so a place that cannot clone cannot be
/// surveyed, and the error is the snapshot's. A type that is not `Clone` is
/// refused by the compiler (`SurveyEdge<T>` asks for it, as `SnapshotEdge<T>`
/// does); what the build refuses is a place that gave cloning up, a nested
/// place among them. A drain from such a place is fine, unless its
/// transition restores.
#[test]
fn a_survey_of_a_place_that_cannot_clone_is_refused() {
    /// A token type that is `Clone` and nests no net, so that it may stand
    /// in a nested place and still be named by a survey edge.
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

    #[derive(Debug)]
    struct Handle;

    let mut b = NetBuilder::new();
    let numbers = b.place_no_clone::<u32>("Numbers");
    let out = b.place::<usize>("Out");
    b.transition("Count", (SurveyEdge(numbers),), (out,), |all: Vec<u32>| {
        (all.len(),)
    });
    let error = b.build().unwrap_err();
    assert!(
        matches!(error, BuildError::SnapshotOfNoClonePlace { ref transition, ref place }
            if transition == "Count" && place == "Numbers"),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "transition \"Count\" snapshots place \"Numbers\", which cannot clone its tokens"
    );

    let mut b = NetBuilder::new();
    let depot = b.place_nested::<Hollow>("Depot");
    let out = b.place::<usize>("Out");
    b.transition("Count", (SurveyEdge(depot),), (out,), |all: Vec<Hollow>| {
        (all.len(),)
    });
    assert!(matches!(
        b.build(),
        Err(BuildError::SnapshotOfNoClonePlace { ref place, .. }) if place == "Depot"
    ));

    let mut b = NetBuilder::new();
    let handles = b.place_no_clone::<Handle>("Handles");
    let out = b.place::<usize>("Out");
    b.transition(
        "Close",
        (DrainEdge(handles),),
        (out,),
        |all: Vec<Handle>| (all.len(),),
    );
    b.build().expect("a drain moves its tokens and clones none");

    let mut b = NetBuilder::new();
    let handles = b.place_no_clone::<Handle>("Handles");
    let out = b.place::<usize>("Out");
    b.transition(
        "Close",
        (DrainEdge(handles),),
        (out,),
        |all: Vec<Handle>| (all.len(),),
    )
    .restore_tokens_on_failure();
    assert!(matches!(
        b.build(),
        Err(BuildError::RestoreOfNoClonePlace { ref transition, ref place })
            if transition == "Close" && place == "Handles"
    ));
}

/// A drain or a survey reads its place through the one edge a transition
/// may have into it.
#[test]
fn a_drain_or_a_survey_reads_its_place_like_any_other_edge() {
    let mut b = NetBuilder::new();
    let waiting = b.place::<String>("Waiting");
    let out = b.place::<String>("Out");
    b.transition(
        "Twice",
        (DrainEdge(waiting), SurveyEdge(waiting)),
        (out,),
        |taken: Vec<String>, _: Vec<String>| (taken,),
    );
    assert!(matches!(
        b.build(),
        Err(BuildError::DuplicateInputPlace { ref transition, ref place })
            if transition == "Twice" && place == "Waiting"
    ));

    let mut b = NetBuilder::new();
    let waiting = b.place::<String>("Waiting");
    let out = b.place::<String>("Out");
    b.transition(
        "Twice",
        (TakeEdge(waiting), SurveyEdge(waiting)),
        (out,),
        |one: String, _: Vec<String>| (one,),
    );
    assert!(matches!(
        b.build(),
        Err(BuildError::DuplicateInputPlace { .. })
    ));
}

/// A transition whose only input is a drain is a source: a drain may take
/// nothing, and does not hold its transition back on an empty place. A
/// survey alone is a source as every read is. Beside a take, neither
/// matters: the shop passes both checks, and the traffic light, whose every
/// transition takes the light or a car, passes the source rule; its light
/// is taken from `Light` and written back to it, a loop of one take, which
/// the acyclicity check refuses, as it should.
#[test]
fn the_source_rule_does_not_count_a_drain() {
    let mut b = NetBuilder::new();
    b.assert_no_source_transitions();
    let waiting = b.place::<String>("Waiting");
    let passed = b.place::<String>("Passed");
    b.transition(
        "Wave Through",
        (DrainEdge(waiting),),
        (passed,),
        |all: Vec<String>| (all,),
    );
    assert!(matches!(
        b.build(),
        Err(BuildError::SourceTransition { ref transition }) if transition == "Wave Through"
    ));

    let mut b = NetBuilder::new();
    b.assert_no_source_transitions();
    let promotions = b.place::<String>("Promotions");
    let seen = b.place::<usize>("Seen");
    b.transition(
        "Look",
        (SurveyEdge(promotions),),
        (seen,),
        |all: Vec<String>| (all.len(),),
    );
    assert!(matches!(
        b.build(),
        Err(BuildError::SourceTransition { ref transition }) if transition == "Look"
    ));

    // Left to run, the drain alone fires on an empty place, and again.
    let mut b = NetBuilder::new();
    let waiting = b.place::<String>("Waiting");
    let passed = b.place::<String>("Passed");
    let wave = b
        .transition(
            "Wave Through",
            (DrainEdge(waiting),),
            (passed,),
            |all: Vec<String>| (all,),
        )
        .id();
    let mut net = b.build().unwrap();
    for _ in 0..3 {
        assert_eq!(net.step().unwrap(), Some(wave));
    }

    let mut b = NetBuilder::new();
    b.assert_no_source_transitions().assert_acyclic();
    declare_shop(&mut b);
    b.build().expect("Price Order takes the order it waits for");

    let mut b = NetBuilder::new();
    b.assert_no_source_transitions();
    declare_light(&mut b, false);
    b.build()
        .expect("each transition takes one token it waits for");

    let mut b = NetBuilder::new();
    b.assert_acyclic();
    declare_light(&mut b, false);
    let error = b.build().unwrap_err();
    assert!(
        matches!(error, BuildError::Cycle { ref path }
            if path == &["Turn Green", "Light", "Turn Green"]),
        "{error:?}"
    );
}

/// The acyclicity walk follows the edges that always take a token: a loop
/// closed only by a drain or a survey is not reported, a loop through a
/// take still is.
#[test]
fn the_acyclicity_walk_leaves_drains_and_surveys_out() {
    // `Recycle` takes a ticket and puts back whatever the bin held: a loop
    // through `Bin`, but each firing pays a ticket, and tickets run out.
    let mut b = NetBuilder::new();
    b.assert_acyclic().assert_no_source_transitions();
    let tickets = b.place::<u32>("Tickets");
    let bin = b.place::<u32>("Bin");
    b.seed(tickets, [1, 2]);
    b.seed(bin, [7]);
    b.transition(
        "Recycle",
        (TakeEdge(tickets), DrainEdge(bin)),
        (bin,),
        |_: u32, all: Vec<u32>| (all,),
    );
    let mut net = b.build().expect("a loop closed only by a drain");
    let summary = drivers::run(
        &mut net,
        Mode::Sequential,
        Limits::default(),
        OnFailure::Stop,
    )
    .unwrap();
    assert!(summary.quiesced);
    assert_eq!(summary.fired_count, 2, "one firing per ticket");

    let mut b = NetBuilder::new();
    b.assert_acyclic();
    let notes = b.place::<u32>("Notes");
    let tickets = b.place::<u32>("Tickets");
    b.transition(
        "Note",
        (TakeEdge(tickets), SurveyEdge(notes)),
        (notes,),
        |n: u32, _: Vec<u32>| (n,),
    );
    b.build().expect("a loop closed only by a survey");

    let mut b = NetBuilder::new();
    b.assert_acyclic();
    let bin = b.place::<u32>("Bin");
    b.transition("Churn", (TakeAllEdge(bin),), (bin,), |all: Vec<u32>| (all,));
    assert!(matches!(
        b.build(),
        Err(BuildError::Cycle { ref path }) if path == &["Churn", "Bin", "Churn"]
    ));
}

// ----- the read API ---------------------------------------------------------------

/// `TransitionInfo::inputs`, `Net::edges` and the topology carry the two
/// kinds as they carry the others, with no filter: the survey on the shop,
/// the drain on the traffic light.
#[test]
fn the_read_api_carries_the_kinds() {
    let (net, shop) = shop();
    let infos: Vec<_> = net.transitions().collect();
    assert_eq!(
        infos[0].1.inputs,
        [
            (shop.orders.id(), EdgeKind::Take),
            (shop.promotions.id(), EdgeKind::Survey)
        ]
    );
    assert_eq!(infos[0].1.filters, [None, None]);
    let expected = vec![
        EdgeInfo::Input {
            place: shop.orders.id(),
            transition: shop.price_order,
            kind: EdgeKind::Take,
            filter: None,
        },
        EdgeInfo::Input {
            place: shop.promotions.id(),
            transition: shop.price_order,
            kind: EdgeKind::Survey,
            filter: None,
        },
        EdgeInfo::Output {
            transition: shop.price_order,
            place: shop.priced.id(),
            slot: 0,
        },
        EdgeInfo::Output {
            transition: shop.price_order,
            place: shop.discounts.id(),
            slot: 1,
        },
    ];
    assert_eq!(net.edges().collect::<Vec<_>>(), expected);
    assert_eq!(
        net.snapshot().unwrap().edges().collect::<Vec<_>>(),
        expected
    );
    let topology = net.topology();
    assert_eq!(topology.edges, expected);
    assert_eq!(topology.transitions[0].filters, [None, None]);
    assert_eq!(net.snapshot().unwrap().topology(), topology);

    let (net, junction) = junction();
    let infos: Vec<_> = net.transitions().collect();
    assert_eq!(
        infos[0].1.inputs,
        [
            (junction.light.id(), EdgeKind::Take),
            (junction.waiting.id(), EdgeKind::Drain)
        ]
    );
    assert_eq!(infos[0].1.filters, [None, None]);
    assert_eq!(
        infos[1].1.inputs,
        [
            (junction.waiting.id(), EdgeKind::Take),
            (junction.light.id(), EdgeKind::Snapshot)
        ]
    );
    let input = |place: PlaceRef<String>, transition, kind| EdgeInfo::Input {
        place: place.id(),
        transition,
        kind,
        filter: None,
    };
    let output = |transition, place: PlaceRef<String>, slot| EdgeInfo::Output {
        transition,
        place: place.id(),
        slot,
    };
    let Junction {
        light,
        waiting,
        passed,
        turn_green,
        pass_on_green,
        change,
    } = junction;
    let expected = vec![
        input(light, turn_green, EdgeKind::Take),
        input(waiting, turn_green, EdgeKind::Drain),
        output(turn_green, light, 0),
        output(turn_green, passed, 1),
        input(waiting, pass_on_green, EdgeKind::Take),
        input(light, pass_on_green, EdgeKind::Snapshot),
        output(pass_on_green, passed, 0),
        input(light, change, EdgeKind::Take),
        output(change, light, 0),
    ];
    assert_eq!(net.edges().collect::<Vec<_>>(), expected);
    assert_eq!(
        net.snapshot().unwrap().edges().collect::<Vec<_>>(),
        expected
    );
    let topology = net.topology();
    assert_eq!(topology.edges, expected);
    assert_eq!(topology.transitions[0].filters, [None, None]);
    assert_eq!(net.snapshot().unwrap().topology(), topology);

    assert!(EdgeKind::Drain.consumes() && !EdgeKind::Drain.requires_presence());
    assert!(!EdgeKind::Survey.consumes() && !EdgeKind::Survey.requires_presence());
    assert!(!EdgeKind::Drain.is_keyed() && !EdgeKind::Survey.is_keyed());
    assert!(EdgeKind::TakeAll.requires_presence() && EdgeKind::Snapshot.requires_presence());
}

/// A snapshot edge still needs its place non-empty: the survey is the read
/// that does not.
#[test]
fn a_snapshot_waits_where_a_survey_does_not() {
    let mut b = NetBuilder::new();
    let config = b.place::<u32>("Config");
    let out = b.place::<u32>("Out");
    let snap = b
        .transition("Snap", (SnapshotEdge(config),), (out,), |n: u32| (n,))
        .id();
    let survey = b
        .transition("Survey", (SurveyEdge(config),), (out,), |all: Vec<u32>| {
            (all,)
        })
        .id();
    let net = b.build().unwrap();
    assert_eq!(net.fireable(), [survey]);
    assert!(!net.is_fireable(snap));
}

#[cfg(feature = "serde")]
mod json {
    use super::*;
    use petritype::serde_json::{self, json};

    #[test]
    fn the_two_kinds_serialise_by_the_protocols_names() {
        assert_eq!(
            [EdgeKind::Drain, EdgeKind::Survey].map(|kind| serde_json::to_value(kind).unwrap()),
            [json!("drain"), json!("survey")]
        );
    }

    /// The topology writes the kinds on their edges, with no `filter` and
    /// no `filters`: the survey on the shop, the drain on the traffic light.
    #[test]
    fn the_topology_serialises_the_kinds() {
        let (net, _) = shop();
        let topology = serde_json::to_value(net.topology()).unwrap();
        assert_eq!(
            topology["edges"],
            json!([
                {"edge": "input", "place": 0, "transition": 0, "kind": "take"},
                {"edge": "input", "place": 1, "transition": 0, "kind": "survey"},
                {"edge": "output", "transition": 0, "place": 2, "slot": 0},
                {"edge": "output", "transition": 0, "place": 3, "slot": 1},
            ])
        );
        assert!(
            topology["transitions"]
                .as_array()
                .unwrap()
                .iter()
                .all(|transition| transition.get("filters").is_none())
        );

        // `Light` is place 0, `Waiting` 1 and `Passed` 2; `Turn Green` and
        // `Change` each read from and write to place 0.
        let (net, _) = junction();
        let topology = serde_json::to_value(net.topology()).unwrap();
        assert_eq!(
            topology["edges"],
            json!([
                {"edge": "input", "place": 0, "transition": 0, "kind": "take"},
                {"edge": "input", "place": 1, "transition": 0, "kind": "drain"},
                {"edge": "output", "transition": 0, "place": 0, "slot": 0},
                {"edge": "output", "transition": 0, "place": 2, "slot": 1},
                {"edge": "input", "place": 1, "transition": 1, "kind": "take"},
                {"edge": "input", "place": 0, "transition": 1, "kind": "snapshot"},
                {"edge": "output", "transition": 1, "place": 2, "slot": 0},
                {"edge": "input", "place": 0, "transition": 2, "kind": "take"},
                {"edge": "output", "transition": 2, "place": 0, "slot": 0},
            ])
        );
        assert!(
            topology["transitions"]
                .as_array()
                .unwrap()
                .iter()
                .all(|transition| transition.get("filters").is_none())
        );
    }
}
