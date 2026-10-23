//! Integration tests for the twenty-second slice: input edges that accept an
//! empty place (DESIGN §4.4, §6.1;
//! `dev-docs/EDGES_ON_EMPTY_PLACES.md`).
//!
//! A `DrainEdge` takes, and a `SurveyEdge` reads a clone of, every token in
//! its place, zero or more, and neither holds its transition back. What is
//! tested here is that promise (fireability with the place empty or full,
//! the tokens handed over, the events, the restore, detachability) and the
//! edges' place in everything else that reads edges: the build checks, the
//! two opt-in checks, the read API and its serialised form, and the drivers.
//!
//! The proposal's example was one shop that surveyed and drained its
//! promotions; since 2026-10-16 it is two nets, as the examples are. The
//! survey is tested on the shop, which prices each order with whatever
//! promotions are running (`the_shop_runs_the_script_to_the_table`); the
//! drain on the traffic light, whose `Go` takes every car waiting while the
//! light is green (`the_traffic_light_runs_the_script`), and, with its guard
//! left off, takes none from an empty queue. The examples
//! `examples/shop.rs` and `examples/traffic_light.rs` carry the same nets as
//! the protocol's conformance nets `shop` and `traffic-light`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use petritype::drivers::{self, Limits, Mode, OnFailure};
use petritype::{
    BuildError, DrainEdge, EdgeInfo, EdgeKind, Event, Filter, Nested, Net, NetBuilder, NetError,
    PlaceId, PlaceRef, SnapshotEdge, SurveyEdge, TakeAllEdge, TakeEdge, TokenId, TransitionId,
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

/// The light's colour, as the example's `Colour` is: the one token of
/// `Light`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Colour {
    Red,
    Green,
    Amber,
}

/// The traffic light's places and transitions.
struct Junction {
    light: PlaceRef<Colour>,
    waiting: PlaceRef<String>,
    passed: PlaceRef<String>,
    go: TransitionId,
    change: TransitionId,
}

/// How [`declare_light`] declares `Go`: with the example's guard or none,
/// and with a body that works or one that fails and restores what it took.
#[derive(Clone, Copy)]
struct Go {
    guarded: bool,
    jammed: bool,
}

/// `Go` as the example has it.
const AS_BUILT: Go = Go {
    guarded: true,
    jammed: false,
};

/// `Go` with no guard: fireable whenever the light is lit, cars or none.
const UNGUARDED: Go = Go {
    guarded: false,
    jammed: false,
};

/// The light's colour, as `Go`'s guard reads it: the front token of `Light`.
fn colour(net: &Net, light: PlaceRef<Colour>) -> Option<Colour> {
    let (token, _) = net.tokens(light.id()).next()?;
    net.token::<Colour>(light.id(), token).copied()
}

/// The colour after `colour`: red, green, amber, red.
fn next_colour(colour: Colour) -> Colour {
    match colour {
        Colour::Red => Colour::Green,
        Colour::Green => Colour::Amber,
        Colour::Amber => Colour::Red,
    }
}

/// The colours in `Light`, in queue order: one, always.
fn colours(net: &Net, light: PlaceRef<Colour>) -> Vec<Colour> {
    net.tokens(light.id())
        .map(|(id, _)| *net.token::<Colour>(light.id(), id).unwrap())
        .collect()
}

/// The traffic light of `examples/traffic_light/net.rs`, declared into `b`,
/// red seeded in `Light` (its controls and its style left out: they are
/// `tests/controls.rs`'s and the example's): `Go`, guarded on green and someone waiting,
/// drains `Waiting` and snapshots `Light`, writing the cars to `Passed`;
/// `Change`, unguarded, takes the light and writes the next colour back.
/// `go` says whether `Go` keeps its guard and whether it fails, restoring
/// what it took.
fn declare_light(b: &mut NetBuilder, go: Go) -> Junction {
    let light = b.place::<Colour>("Light");
    let waiting = b.place::<String>("Waiting");
    let passed = b.place::<String>("Passed");
    b.seed(light, [Colour::Red]);
    let Go { guarded, jammed } = go;
    let mut drain = b.transition(
        "Go",
        (DrainEdge(waiting), SnapshotEdge(light)),
        (passed,),
        move |cars: Vec<String>, _light: Colour| -> Result<(Vec<String>,), Broken> {
            if jammed {
                return Err(Broken);
            }
            Ok((cars,))
        },
    );
    if guarded {
        drain.guard(move |net| {
            colour(net, light) == Some(Colour::Green) && net.count(waiting.id()) > 0
        });
    }
    if jammed {
        drain.restore_tokens_on_failure();
    }
    let go = drain.id();
    let change = b
        .transition("Change", (TakeEdge(light),), (light,), |l: Colour| {
            (next_colour(l),)
        })
        .id();
    Junction {
        light,
        waiting,
        passed,
        go,
        change,
    }
}

/// The traffic light, built, `Go` as given.
fn junction_with(go: Go) -> (Net, Junction) {
    let mut b = NetBuilder::new();
    let junction = declare_light(&mut b, go);
    (b.build().expect("the light builds"), junction)
}

/// The traffic light, built as the example has it.
fn junction() -> (Net, Junction) {
    junction_with(AS_BUILT)
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
/// changes to green and to amber with nobody waiting, two cars wait through
/// amber and red and are drained in one firing once it is green again, a
/// third arrives at green and goes on its own, and the light never stops.
/// The light is one token in `Light`, its colour, a fresh one after every
/// `Change` and the same one through every `Go`.
#[test]
fn the_traffic_light_runs_the_script() {
    let (mut net, junction) = junction();

    assert_eq!(net.fireable(), [junction.change], "red, nobody waiting");
    assert_eq!(net.step().unwrap(), Some(junction.change));
    assert_eq!(colours(&net, junction.light), [Colour::Green]);
    assert_eq!(net.fireable(), [junction.change], "green, nobody waiting");
    assert_eq!(net.step().unwrap(), Some(junction.change));
    assert_eq!(colours(&net, junction.light), [Colour::Amber]);

    net.extend(junction.waiting, [s("Car 1"), s("Car 2")]);
    assert_eq!(net.fireable(), [junction.change], "the cars wait at amber");
    assert_eq!(net.step().unwrap(), Some(junction.change));
    assert_eq!(colours(&net, junction.light), [Colour::Red]);
    assert_eq!(net.fireable(), [junction.change], "and at red");
    assert_eq!(net.step().unwrap(), Some(junction.change));
    assert_eq!(colours(&net, junction.light), [Colour::Green]);
    assert_eq!(
        net.fireable(),
        [junction.go, junction.change],
        "the cars before the light changes"
    );
    let lamp = ids(&net, junction.light);
    assert_eq!(net.step().unwrap(), Some(junction.go));
    assert_eq!(strings(&net, junction.passed), ["Car 1", "Car 2"]);
    assert_eq!(net.count(junction.waiting.id()), 0, "everyone waiting went");
    assert_eq!(ids(&net, junction.light), lamp, "read, not taken");
    assert_eq!(net.fireable(), [junction.change]);

    net.put(junction.waiting, s("Car 3"));
    assert_eq!(net.fireable(), [junction.go, junction.change]);
    assert_eq!(net.step().unwrap(), Some(junction.go));
    assert_eq!(ids(&net, junction.light), lamp, "read, not taken");
    assert_eq!(colours(&net, junction.light), [Colour::Green]);
    assert_eq!(net.step().unwrap(), Some(junction.change));

    assert_eq!(colours(&net, junction.light), [Colour::Amber]);
    assert_eq!(strings(&net, junction.passed), ["Car 1", "Car 2", "Car 3"]);
    assert_eq!(net.count(junction.waiting.id()), 0);
    assert_eq!(net.fired_count(), 7);
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

/// A drain lists every token it took in the firing's `consumed`, oldest
/// first, which from an empty place is none: `Go` reads the light, so the
/// cars are all it lists. The example's guard never lets `Go` fire on an
/// empty queue; with the guard left off, it does, and lists nothing.
#[test]
fn consumed_lists_what_a_drain_took_oldest_first() {
    let (mut net, junction) = junction();
    assert_eq!(net.step().unwrap(), Some(junction.change));
    let cars = net.extend(junction.waiting, [s("Car 1"), s("Car 2")]);
    net.drain_events().for_each(drop);
    assert_eq!(net.step().unwrap(), Some(junction.go));
    let events: Vec<Event> = net.drain_events().collect();
    let expected: Vec<_> = cars.iter().map(|&id| (junction.waiting.id(), id)).collect();
    assert!(
        matches!(&events[..], [Event::Fired { transition, consumed, produced }]
            if *transition == junction.go
                && consumed == &expected
                && produced.len() == 2),
        "the cars oldest first, and not the light: {events:?}"
    );

    let (mut net, junction) = junction_with(UNGUARDED);
    net.set_tokens(junction.light, [Colour::Green]);
    net.drain_events().for_each(drop);
    assert_eq!(net.step().unwrap(), Some(junction.go));
    let events: Vec<Event> = net.drain_events().collect();
    assert!(
        matches!(&events[..], [Event::Fired { transition, consumed, produced }]
            if *transition == junction.go && consumed.is_empty() && produced.is_empty()),
        "nothing from the empty Waiting: {events:?}"
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
/// taken from the place at launch, and not the light, which is read. A drain
/// and a snapshot borrow nothing, so the firing is detachable.
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
        detachable(&net, junction.go),
        "a drain and a snapshot borrow nothing"
    );

    assert_eq!(net.step().unwrap(), Some(junction.change));
    let lamp = ids(&net, junction.light);
    let car = net.put(junction.waiting, s("Car 1"));
    net.drain_events().for_each(drop);
    let firing = net.launch(junction.go).unwrap();
    assert!(firing.is_pending(), "not run inline");
    assert_eq!(net.count(junction.waiting.id()), 0, "drained at launch");
    assert_eq!(ids(&net, junction.light), lamp, "the light left lit");
    assert!(matches!(
        net.drain_events().collect::<Vec<_>>()[..],
        [Event::Launched { ref consumed, .. }]
            if consumed == &[(junction.waiting.id(), car)]
    ));
    net.distribute(firing.run()).unwrap();
    assert_eq!(strings(&net, junction.passed), ["Car 1"]);
    assert_eq!(ids(&net, junction.light), lamp);
    assert_eq!(colours(&net, junction.light), [Colour::Green]);
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

/// A failing `Go` that restores its inputs puts the cars back at the front
/// of `Waiting`, oldest first, with their ids, ahead of one that arrived
/// while the firing was in flight; the light, only read, never left
/// `Light`, and nothing reaches `Passed`.
#[test]
fn a_failing_go_restores_the_cars_oldest_first_with_their_ids() {
    let (mut net, junction) = junction_with(Go {
        guarded: true,
        jammed: true,
    });
    net.set_tokens(junction.light, [Colour::Green]);
    let lamp = ids(&net, junction.light);
    let cars = net.extend(junction.waiting, [s("Car 1"), s("Car 2")]);
    net.drain_events().for_each(drop);

    let firing = net.launch(junction.go).unwrap();
    let late = net.put(junction.waiting, s("Car 3"));
    let failure = net.distribute(firing.run()).expect_err("the signal jams");
    assert!(matches!(failure, NetError::FiringFailed(ref f) if f.restored));

    let events: Vec<Event> = net.drain_events().collect();
    assert!(
        matches!(events.last(), Some(Event::Failed { transition, consumed, restored: true, error })
            if *transition == junction.go
                && consumed == &[
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
    assert_eq!(ids(&net, junction.light), lamp);
    assert_eq!(colours(&net, junction.light), [Colour::Green]);
    assert!(net.tokens(junction.passed.id()).next().is_none());

    // With the guard left off and nobody waiting, the failing firing took
    // nothing and has nothing to put back.
    let (mut net, junction) = junction_with(Go {
        guarded: false,
        jammed: true,
    });
    net.set_tokens(junction.light, [Colour::Green]);
    let lamp = ids(&net, junction.light);
    net.drain_events().for_each(drop);
    net.fire(junction.go).expect_err("still jammed");
    assert!(matches!(
        net.drain_events().collect::<Vec<_>>()[..],
        [Event::Failed { ref consumed, restored: true, .. }] if consumed.is_empty()
    ));
    assert_eq!(ids(&net, junction.light), lamp);
    assert_eq!(net.count(junction.waiting.id()), 0);
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
/// matters: the shop passes both checks. The traffic light's `Go` drains the
/// cars and only reads the light, so the source rule names it, guarded or
/// not: the rule reads edges, not guards, and it is `Go`'s guard, green and
/// someone waiting, that keeps it from firing on an empty queue. Its light
/// is taken from `Light` and written back to it by `Change`, a loop of one
/// take, which the acyclicity check refuses, as it should.
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

    for go in [AS_BUILT, UNGUARDED] {
        let mut b = NetBuilder::new();
        b.assert_no_source_transitions();
        declare_light(&mut b, go);
        let error = b.build().unwrap_err();
        assert!(
            matches!(error, BuildError::SourceTransition { ref transition } if transition == "Go"),
            "{error:?}"
        );
    }

    let mut b = NetBuilder::new();
    b.assert_acyclic();
    declare_light(&mut b, AS_BUILT);
    let error = b.build().unwrap_err();
    assert!(
        matches!(error, BuildError::Cycle { ref path }
            if path == &["Change", "Light", "Change"]),
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
            (junction.waiting.id(), EdgeKind::Drain),
            (junction.light.id(), EdgeKind::Snapshot)
        ]
    );
    assert_eq!(infos[0].1.filters, [None, None]);
    assert_eq!(infos[1].1.inputs, [(junction.light.id(), EdgeKind::Take)]);
    let input = |place: PlaceId, transition, kind| EdgeInfo::Input {
        place,
        transition,
        kind,
        filter: None,
    };
    let output = |transition, place: PlaceId, slot| EdgeInfo::Output {
        transition,
        place,
        slot,
    };
    let Junction {
        light,
        waiting,
        passed,
        go,
        change,
    } = junction;
    let expected = vec![
        input(waiting.id(), go, EdgeKind::Drain),
        input(light.id(), go, EdgeKind::Snapshot),
        output(go, passed.id(), 0),
        input(light.id(), change, EdgeKind::Take),
        output(change, light.id(), 0),
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

        // `Light` is place 0, `Waiting` 1 and `Passed` 2; `Go` drains place
        // 1 and reads place 0, and `Change` takes from and writes to place 0.
        let (net, _) = junction();
        let topology = serde_json::to_value(net.topology()).unwrap();
        assert_eq!(
            topology["edges"],
            json!([
                {"edge": "input", "place": 1, "transition": 0, "kind": "drain"},
                {"edge": "input", "place": 0, "transition": 0, "kind": "snapshot"},
                {"edge": "output", "transition": 0, "place": 2, "slot": 0},
                {"edge": "input", "place": 0, "transition": 1, "kind": "take"},
                {"edge": "output", "transition": 1, "place": 0, "slot": 0},
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
