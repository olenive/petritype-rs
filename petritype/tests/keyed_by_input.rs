//! Integration tests for a keyed edge that chooses its token by another
//! input of the same firing (DESIGN §8.3, "Choosing by another input",
//! 2026-11-23).
//!
//! A `Clause::IsInput` in the filter of a `TakeWhereEdge` or a
//! `MutateWhereEdge` binds the edge to another input of its transition, its
//! source: "the unit this order names". The firing takes the oldest source
//! token that has a match, with the oldest match for it, and every other
//! token stays where it is with its id. What is tested here is that rule
//! (both forms, the token passed over, several bound edges, a source with a
//! filter of its own, the kinds of source, the restore), the clause beside
//! the others, the build checks, the clause outside a firing, ports, and
//! the serialised form.

use petritype::{
    BuildError, DrainEdge, EdgeKind, Event, Filter, MutateEdge, MutateWhereEdge, Net, NetBuilder,
    PlaceRef, SnapshotEdge, SurveyEdge, TakeAllEdge, TakeEdge, TakeWhereEdge, TokenId,
};

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
struct Order {
    item: String,
    unit: String,
    depot: String,
}

fn order(item: &str, unit: &str) -> Order {
    Order {
        item: item.to_owned(),
        unit: unit.to_owned(),
        depot: "North".to_owned(),
    }
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
struct Unit {
    name: String,
    kind: String,
    orders: u32,
}

fn unit(name: &str, kind: &str) -> Unit {
    Unit {
        name: name.to_owned(),
        kind: kind.to_owned(),
        orders: 0,
    }
}

#[derive(Debug)]
struct Empty(String);

impl std::fmt::Display for Empty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} has nothing to carry", self.0)
    }
}

impl std::error::Error for Empty {}

/// `Orders` keyed by `unit`, `depot` and `item`; `Units` and `Depots` by
/// `name` and `kind`; `Done` and `Out`, without keys.
struct Depot {
    b: NetBuilder,
    orders: PlaceRef<Order>,
    units: PlaceRef<Unit>,
    depots: PlaceRef<Unit>,
    done: PlaceRef<Order>,
    out: PlaceRef<Unit>,
}

fn depot(orders: &[Order], units: &[Unit]) -> Depot {
    let mut b = NetBuilder::new();
    let orders_place = b.place::<Order>("Orders");
    let units_place = b.place::<Unit>("Units");
    let depots = b.place::<Unit>("Depots");
    let done = b.place::<Order>("Done");
    let out = b.place::<Unit>("Out");
    b.keys(orders_place, ["unit", "depot", "item"], |o: &Order| {
        [
            o.unit.as_str().into(),
            o.depot.as_str().into(),
            o.item.as_str().into(),
        ]
    });
    for place in [units_place, depots] {
        b.keys(place, ["name", "kind"], |u: &Unit| {
            [u.name.as_str().into(), u.kind.as_str().into()]
        });
    }
    b.seed(orders_place, orders.iter().cloned());
    b.seed(units_place, units.iter().cloned());
    Depot {
        b,
        orders: orders_place,
        units: units_place,
        depots,
        done,
        out,
    }
}

/// The unit an order names.
fn named_by(orders: PlaceRef<Order>) -> Filter {
    Filter::new().key_is_input("name", orders, "unit")
}

fn ids<T: 'static>(net: &Net, place: PlaceRef<T>) -> Vec<TokenId> {
    net.tokens(place.id()).map(|(id, _)| id).collect()
}

fn items(net: &Net, place: PlaceRef<Order>) -> Vec<String> {
    net.tokens(place.id())
        .map(|(id, _)| net.token::<Order>(place.id(), id).unwrap().item.clone())
        .collect()
}

fn units(net: &Net, place: PlaceRef<Unit>) -> Vec<(String, u32)> {
    net.tokens(place.id())
        .map(|(id, _)| {
            let unit = net.token::<Unit>(place.id(), id).unwrap();
            (unit.name.clone(), unit.orders)
        })
        .collect()
}

/// `Load`: an order, and the unit it names, which carries one order more.
fn load(d: &mut Depot) -> petritype::TransitionId {
    d.b.transition(
        "Load",
        (
            TakeEdge(d.orders),
            MutateWhereEdge(d.units, named_by(d.orders)),
        ),
        (d.done,),
        |order: Order, unit: &mut Unit| {
            unit.orders += 1;
            (order,)
        },
    )
    .id()
}

// ----- which pair fires --------------------------------------------------------

/// The mutate form: the oldest order that has its unit, with that unit; an
/// order whose unit is not there waits where it is, with its id, and does
/// not hold back the orders behind it.
#[test]
fn the_mutate_form_pairs_the_oldest_order_that_has_its_unit() {
    let mut d = depot(
        &[
            order("Lamp", "Cleo"),
            order("Book", "Ben"),
            order("Kite", "Ada"),
        ],
        &[unit("Ada", "Van"), unit("Ben", "Bike")],
    );
    let load = load(&mut d);
    let Depot {
        b,
        orders,
        units: unit_place,
        done,
        ..
    } = d;
    let mut net = b.build().unwrap();
    let order_ids = ids(&net, orders);
    let unit_ids = ids(&net, unit_place);
    net.drain_events().for_each(drop);

    assert!(
        net.is_fireable(load),
        "the book has its unit, behind the lamp"
    );
    assert_eq!(net.step().unwrap(), Some(load));
    assert_eq!(items(&net, done), ["Book"]);
    assert_eq!(
        units(&net, unit_place),
        [("Ada".into(), 0), ("Ben".into(), 1)]
    );
    assert_eq!(
        ids(&net, unit_place),
        unit_ids,
        "the units keep their places and ids"
    );
    assert_eq!(
        ids(&net, orders),
        [order_ids[0], order_ids[2]],
        "the lamp waits"
    );
    let events: Vec<Event> = net.drain_events().collect();
    let produced = ids(&net, done)[0];
    assert_eq!(
        events,
        [
            Event::Mutated {
                place: unit_place.id(),
                token: unit_ids[1],
            },
            Event::Fired {
                transition: load,
                consumed: vec![(orders.id(), order_ids[1])],
                produced: vec![(done.id(), produced)],
            },
        ]
    );

    assert_eq!(net.step().unwrap(), Some(load), "the kite, for Ada");
    assert_eq!(
        units(&net, unit_place),
        [("Ada".into(), 1), ("Ben".into(), 1)]
    );
    assert_eq!(net.step().unwrap(), None, "the lamp has no unit");
    assert_eq!(
        ids(&net, orders),
        [order_ids[0]],
        "still there, with its id"
    );

    net.put(unit_place, unit("Cleo", "Van"));
    assert_eq!(net.step().unwrap(), Some(load), "now it has one");
    assert_eq!(items(&net, done), ["Book", "Kite", "Lamp"]);
}

/// The take form: the unit a call names is taken from between the others,
/// which keep their places and ids, and the firing consumes both.
#[test]
fn the_take_form_takes_the_named_unit_from_the_middle() {
    let mut d = depot(
        &[order("Call", "Ben")],
        &[unit("Ada", "Van"), unit("Ben", "Bike"), unit("Dot", "Van")],
    );
    let dispatch =
        d.b.transition(
            "Dispatch",
            (
                TakeEdge(d.orders),
                TakeWhereEdge(d.units, named_by(d.orders)),
            ),
            (d.out,),
            |_: Order, unit: Unit| (unit,),
        )
        .id();
    let Depot {
        b,
        orders,
        units: unit_place,
        out,
        ..
    } = d;
    let mut net = b.build().unwrap();
    let call = ids(&net, orders)[0];
    let unit_ids = ids(&net, unit_place);
    net.drain_events().for_each(drop);

    net.fire(dispatch).unwrap();
    assert_eq!(units(&net, out), [("Ben".into(), 0)]);
    assert_eq!(ids(&net, unit_place), [unit_ids[0], unit_ids[2]]);
    let produced = ids(&net, out)[0];
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Fired {
            transition: dispatch,
            consumed: vec![(orders.id(), call), (unit_place.id(), unit_ids[1])],
            produced: vec![(out.id(), produced)],
        }]
    );
    assert!(!net.is_fireable(dispatch), "no call left");
}

/// Two edges bound to one source: the source token must have a match in
/// each, and the first that does is the one taken.
#[test]
fn two_edges_bound_to_one_source_need_a_match_each() {
    let mut d = depot(
        &[
            Order {
                depot: "East".into(),
                ..order("Lamp", "Ada")
            },
            Order {
                depot: "South".into(),
                ..order("Book", "Ben")
            },
        ],
        &[unit("Ada", "Van"), unit("Ben", "Bike")],
    );
    d.b.seed(d.depots, [unit("South", "Depot"), unit("North", "Depot")]);
    let both =
        d.b.transition(
            "Both",
            (
                TakeEdge(d.orders),
                MutateWhereEdge(d.units, named_by(d.orders)),
                TakeWhereEdge(
                    d.depots,
                    Filter::new().key_is_input("name", d.orders, "depot"),
                ),
            ),
            (d.done,),
            |order: Order, unit: &mut Unit, _depot: Unit| {
                unit.orders += 1;
                (order,)
            },
        )
        .id();
    let Depot {
        b,
        orders,
        units: unit_place,
        depots,
        done,
        ..
    } = d;
    let mut net = b.build().unwrap();
    let depot_ids = ids(&net, depots);

    net.fire(both).unwrap();
    assert_eq!(
        items(&net, done),
        ["Book"],
        "the lamp's depot, East, is not there"
    );
    assert_eq!(
        units(&net, unit_place),
        [("Ada".into(), 0), ("Ben".into(), 1)]
    );
    assert_eq!(
        ids(&net, depots),
        [depot_ids[1]],
        "South went with the book"
    );
    assert_eq!(items(&net, orders), ["Lamp"]);
    assert!(!net.is_fireable(both));
}

/// A source that is keyed itself offers only the tokens its own filter
/// accepts, oldest first.
#[test]
fn a_source_with_a_filter_of_its_own_offers_only_its_matches() {
    let mut d = depot(
        &[
            order("Lamp", "Ada"),
            order("Book", "Ben"),
            order("Kite", "Ada"),
        ],
        &[unit("Ada", "Van"), unit("Ben", "Bike")],
    );
    let toys =
        d.b.transition(
            "Toys",
            (
                TakeWhereEdge(d.orders, Filter::new().key_in("item", ["Kite", "Book"])),
                MutateWhereEdge(d.units, named_by(d.orders)),
            ),
            (d.done,),
            |order: Order, unit: &mut Unit| {
                unit.orders += 1;
                (order,)
            },
        )
        .id();
    let Depot {
        b, orders, done, ..
    } = d;
    let mut net = b.build().unwrap();
    net.fire(toys).unwrap();
    assert_eq!(items(&net, done), ["Book"], "the lamp is no toy");
    net.fire(toys).unwrap();
    assert_eq!(items(&net, orders), ["Lamp"]);
    assert!(!net.is_fireable(toys));
}

/// A snapshot source copies the paired token and a mutate source lends it,
/// wherever it stands, and neither moves it.
#[test]
fn a_snapshot_or_a_mutate_source_reads_the_paired_token() {
    let mut d = depot(
        &[order("Lamp", "Cleo"), order("Book", "Ben")],
        &[unit("Ada", "Van"), unit("Ben", "Bike")],
    );
    let copy =
        d.b.transition(
            "Copy",
            (
                SnapshotEdge(d.orders),
                TakeWhereEdge(d.units, named_by(d.orders)),
            ),
            (d.done,),
            |order: Order, _: Unit| (order,),
        )
        .id();
    let mark =
        d.b.transition(
            "Mark",
            (
                MutateEdge(d.orders),
                MutateWhereEdge(d.units, named_by(d.orders)),
            ),
            (),
            |order: &mut Order, unit: &mut Unit| {
                order.item.push('!');
                unit.orders += 1;
            },
        )
        .id();
    let Depot {
        b,
        orders,
        units: unit_place,
        done,
        ..
    } = d;
    let mut net = b.build().unwrap();
    let order_ids = ids(&net, orders);

    net.fire(mark).unwrap();
    assert_eq!(
        items(&net, orders),
        ["Lamp", "Book!"],
        "the book, where it stands"
    );
    assert_eq!(
        units(&net, unit_place),
        [("Ada".into(), 0), ("Ben".into(), 1)]
    );
    assert!(net.drain_events().any(|event| event
        == Event::Mutated {
            place: orders.id(),
            token: order_ids[1]
        }));

    net.fire(copy).unwrap();
    assert_eq!(items(&net, done), ["Book!"], "a copy of the book");
    assert_eq!(ids(&net, orders), order_ids, "both orders stay");
    assert_eq!(units(&net, unit_place), [("Ada".into(), 0)]);
}

/// A failed firing that restores puts both tokens back at the front of their
/// places, with their ids, as every restore does.
#[test]
fn a_failed_firing_puts_both_back_at_the_front_with_their_ids() {
    let mut d = depot(
        &[order("Call", "Dot")],
        &[unit("Ada", "Van"), unit("Dot", "Van")],
    );
    let dispatch =
        d.b.transition(
            "Dispatch",
            (
                TakeEdge(d.orders),
                TakeWhereEdge(d.units, named_by(d.orders)),
            ),
            (d.out,),
            |_: Order, unit: Unit| -> Result<(Unit,), Empty> {
                if unit.orders == 0 {
                    Err(Empty(unit.name))
                } else {
                    Ok((unit,))
                }
            },
        )
        .restore_tokens_on_failure()
        .id();
    let Depot {
        b,
        orders,
        units: unit_place,
        ..
    } = d;
    let mut net = b.build().unwrap();
    let call = ids(&net, orders)[0];
    let unit_ids = ids(&net, unit_place);
    net.drain_events().for_each(drop);

    let failed = net.fire(dispatch).unwrap_err();
    assert_eq!(
        failed.to_string(),
        format!("transition \"Dispatch\" ({dispatch:?}) failed: Dot has nothing to carry")
    );
    assert_eq!(ids(&net, orders), [call]);
    assert_eq!(
        ids(&net, unit_place),
        [unit_ids[1], unit_ids[0]],
        "Dot at the front"
    );
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Failed {
            transition: dispatch,
            consumed: vec![(orders.id(), call), (unit_place.id(), unit_ids[1])],
            restored: true,
            error: "Dot has nothing to carry".into(),
        }]
    );
    assert!(net.is_fireable(dispatch), "the same pair, again");
}

/// A constant clause stands beside the input clause, and both must pass.
#[test]
fn a_constant_clause_stands_beside_the_input_clause() {
    let mut d = depot(
        &[order("Lamp", "Ben")],
        &[unit("Ben", "Bike"), unit("Ben", "Van")],
    );
    let by_van =
        d.b.transition(
            "By Van",
            (
                TakeEdge(d.orders),
                TakeWhereEdge(d.units, named_by(d.orders).key_in("kind", ["Van"])),
            ),
            (d.out,),
            |_: Order, unit: Unit| (unit,),
        )
        .id();
    let Depot {
        b,
        units: unit_place,
        out,
        ..
    } = d;
    let mut net = b.build().unwrap();
    let unit_ids = ids(&net, unit_place);
    net.fire(by_van).unwrap();
    assert_eq!(
        ids(&net, unit_place),
        [unit_ids[0]],
        "the Ben on a bike stays"
    );
    assert_eq!(units(&net, out), [("Ben".into(), 0)]);
}

/// A parameter clause stands beside it too, read as the parameter is now.
#[cfg(feature = "serde")]
#[test]
fn a_parameter_clause_stands_beside_the_input_clause() {
    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    struct Kind(String);

    let mut d = depot(
        &[order("Lamp", "Ben")],
        &[unit("Ben", "Bike"), unit("Ben", "Van")],
    );
    d.b.param(Kind("Van".to_owned()));
    d.b.serializable_param::<Kind>("kind");
    let send =
        d.b.transition(
            "Send",
            (
                TakeEdge(d.orders),
                TakeWhereEdge(d.units, named_by(d.orders).key_is_param("kind", "kind")),
            ),
            (d.out,),
            |_: Order, unit: Unit| (unit,),
        )
        .id();
    let Depot {
        b,
        units: unit_place,
        ..
    } = d;
    let mut net = b.build().unwrap();
    let unit_ids = ids(&net, unit_place);
    net.set_param(Kind("Boat".to_owned())).unwrap();
    assert!(!net.is_fireable(send), "no Ben on a boat");
    net.set_param(Kind("Bike".to_owned())).unwrap();
    net.fire(send).unwrap();
    assert_eq!(ids(&net, unit_place), [unit_ids[1]]);
}

/// The guard is asked after the pair is found, and the staged path launches
/// the pair that fireability found; `select` offers the transition as it
/// offers any other.
#[test]
fn the_guard_select_and_the_staged_path_see_the_pair() {
    let mut d = depot(
        &[order("Lamp", "Cleo"), order("Book", "Ben")],
        &[unit("Ben", "Bike")],
    );
    let out_id = d.out.id();
    let dispatch =
        d.b.transition(
            "Dispatch",
            (
                TakeEdge(d.orders),
                TakeWhereEdge(d.units, named_by(d.orders)),
            ),
            (d.out,),
            |_: Order, unit: Unit| (unit,),
        )
        .guard(move |net: &Net| net.count(out_id) == 0)
        .id();
    let Depot {
        b,
        orders,
        units: unit_place,
        out,
        ..
    } = d;
    let mut net = b.build().unwrap();
    assert_eq!(net.select(), Some(dispatch));
    let lamp = ids(&net, orders)[0];
    let firing = net.launch(dispatch).unwrap();
    assert_eq!(ids(&net, orders), [lamp], "the book left, the lamp stayed");
    assert_eq!(net.count(unit_place.id()), 0);
    net.distribute(firing.run()).unwrap();
    assert_eq!(units(&net, out), [("Ben".into(), 0)]);

    net.put(unit_place, unit("Cleo", "Van"));
    assert!(
        !net.is_fireable(dispatch),
        "the guard says no while Out holds a unit"
    );
}

// ----- outside a firing --------------------------------------------------------

/// With no firing there is no source token, so a query, a filter on its own
/// and `first_match` accept nothing through the clause.
#[test]
fn outside_a_firing_the_clause_accepts_nothing() {
    let d = depot(&[order("Book", "Ben")], &[unit("Ben", "Bike")]);
    let filter = named_by(d.orders);
    let net = d.b.build().unwrap();
    assert!(net.query(&filter).unwrap().is_empty());
    assert_eq!(net.first_match(d.units.id(), &filter), None);
    let ben = net.tokens(d.units.id()).next().unwrap().0;
    assert!(!filter.matches(&net.keys(d.units.id(), ben).unwrap()));
    assert!(!net.matches(&filter, d.units.id(), ben));
}

// ----- build checks ------------------------------------------------------------

fn refused(d: Depot) -> BuildError {
    d.b.build().expect_err("the net is refused")
}

#[test]
fn a_source_the_transition_does_not_read_is_refused() {
    let mut d = depot(&[], &[]);
    d.b.transition(
        "Load",
        (TakeWhereEdge(d.units, named_by(d.orders)),),
        (d.out,),
        |unit: Unit| (unit,),
    );
    let error = refused(d);
    assert!(
        matches!(&error, BuildError::BoundSourceNotAnInput { transition, place, input }
            if transition == "Load" && place == "Units" && input == "Orders"),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "transition \"Load\" chooses its token from place \"Units\" by place \"Orders\", which it \
         does not read"
    );
}

#[test]
fn a_source_that_gives_every_token_is_refused() {
    fn with_source<E: petritype::InputEdge>(
        edge: impl Fn(PlaceRef<Order>) -> E,
        body: impl Fn(&mut Depot, E),
    ) -> BuildError {
        let mut d = depot(&[], &[]);
        let edge = edge(d.orders);
        body(&mut d, edge);
        refused(d)
    }
    let errors = [
        with_source(TakeAllEdge, |d, edge| {
            d.b.transition(
                "T",
                (edge, TakeWhereEdge(d.units, named_by(d.orders))),
                (),
                |_: Vec<Order>, _: Unit| {},
            );
        }),
        with_source(DrainEdge, |d, edge| {
            d.b.transition(
                "T",
                (edge, TakeWhereEdge(d.units, named_by(d.orders))),
                (),
                |_: Vec<Order>, _: Unit| {},
            );
        }),
        with_source(SurveyEdge, |d, edge| {
            d.b.transition(
                "T",
                (edge, TakeWhereEdge(d.units, named_by(d.orders))),
                (),
                |_: Vec<Order>, _: Unit| {},
            );
        }),
    ];
    for error in errors {
        assert!(
            matches!(&error, BuildError::BoundSourceGivesMany { place, input, .. }
                if place == "Units" && input == "Orders"),
            "{error:?}"
        );
    }
}

/// A chain (a source whose own token is chosen by an input), a cycle of two,
/// and a clause naming the edge's own place, the shortest cycle.
#[test]
fn a_chain_and_a_cycle_are_refused() {
    let mut d = depot(&[], &[]);
    d.b.transition(
        "Chain",
        (
            TakeEdge(d.orders),
            TakeWhereEdge(d.units, named_by(d.orders)),
            TakeWhereEdge(
                d.depots,
                Filter::new().key_is_input("name", d.units, "kind"),
            ),
        ),
        (),
        |_: Order, _: Unit, _: Unit| {},
    );
    let error = refused(d);
    assert!(
        matches!(&error, BuildError::BoundSourceIsBound { place, input, .. }
            if place == "Depots" && input == "Units"),
        "{error:?}"
    );

    let mut d = depot(&[], &[]);
    d.b.transition(
        "Cycle",
        (
            TakeWhereEdge(
                d.units,
                Filter::new().key_is_input("name", d.depots, "name"),
            ),
            TakeWhereEdge(
                d.depots,
                Filter::new().key_is_input("name", d.units, "name"),
            ),
        ),
        (),
        |_: Unit, _: Unit| {},
    );
    assert!(matches!(
        refused(d),
        BuildError::BoundSourceIsBound { place, input, .. } if place == "Units" && input == "Depots"
    ));

    let mut d = depot(&[], &[]);
    d.b.transition(
        "Itself",
        (TakeWhereEdge(
            d.units,
            Filter::new().key_is_input("name", d.units, "kind"),
        ),),
        (),
        |_: Unit| {},
    );
    let error = refused(d);
    assert!(
        matches!(&error, BuildError::BoundSourceIsBound { place, input, .. }
            if place == "Units" && input == "Units"),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "transition \"Itself\" chooses its token from place \"Units\" by place \"Units\", whose \
         own token is chosen by an input"
    );
}

#[test]
fn a_filter_naming_two_sources_is_refused() {
    let mut d = depot(&[], &[]);
    d.b.transition(
        "Two",
        (
            TakeEdge(d.orders),
            TakeEdge(d.depots),
            TakeWhereEdge(
                d.units,
                named_by(d.orders).key_is_input("kind", d.depots, "kind"),
            ),
        ),
        (),
        |_: Order, _: Unit, _: Unit| {},
    );
    let error = refused(d);
    assert!(
        matches!(&error, BuildError::BoundToTwoSources { place, first, second, .. }
            if place == "Units" && first == "Orders" && second == "Depots"),
        "{error:?}"
    );

    // Two keys of one source are one source.
    let mut d = depot(&[order("Book", "Ben")], &[unit("Ben", "North")]);
    let both =
        d.b.transition(
            "Both",
            (
                TakeEdge(d.orders),
                TakeWhereEdge(
                    d.units,
                    named_by(d.orders).key_is_input("kind", d.orders, "depot"),
                ),
            ),
            (),
            |_: Order, _: Unit| {},
        )
        .id();
    let mut net = d.b.build().unwrap();
    net.fire(both).unwrap();
}

/// Each key must be declared: the bound edge's on its place, the source's
/// on the source's.
#[test]
fn an_undeclared_key_is_refused_on_either_side() {
    let mut d = depot(&[], &[]);
    d.b.transition(
        "Own",
        (
            TakeEdge(d.orders),
            TakeWhereEdge(
                d.units,
                Filter::new().key_is_input("colour", d.orders, "unit"),
            ),
        ),
        (),
        |_: Order, _: Unit| {},
    );
    let error = refused(d);
    assert!(
        matches!(&error, BuildError::BoundKeyNotDeclared { place, key, owner, .. }
            if place == "Units" && key == "colour" && owner == "Units"),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "transition \"Own\" chooses its token from place \"Units\" by key \"colour\", which place \
         \"Units\" does not declare"
    );

    let mut d = depot(&[], &[]);
    d.b.transition(
        "Source",
        (
            TakeEdge(d.orders),
            TakeWhereEdge(
                d.units,
                Filter::new().key_is_input("name", d.orders, "colour"),
            ),
        ),
        (),
        |_: Order, _: Unit| {},
    );
    assert!(matches!(
        refused(d),
        BuildError::BoundKeyNotDeclared { key, owner, .. } if key == "colour" && owner == "Orders"
    ));
}

/// The read API carries the clause as declared, on the bound edge's filter.
#[test]
fn the_read_api_carries_the_clause() {
    let mut d = depot(&[], &[]);
    let load = load(&mut d);
    let net = d.b.build().unwrap();
    let (_, info) = net.transitions().find(|(id, _)| *id == load).unwrap();
    assert_eq!(
        info.inputs,
        [
            (d.orders.id(), EdgeKind::Take),
            (d.units.id(), EdgeKind::MutateWhere)
        ]
    );
    assert_eq!(info.filters, [None, Some(named_by(d.orders))]);
}

// ----- ports ---------------------------------------------------------------------

/// A bound edge on a port of a subnet place, and a source on one: each
/// reads the port's tokens, as any edge on a port does.
#[test]
fn a_source_and_a_bound_edge_may_be_ports() {
    // A yard of units, its bay a port.
    let mut inner = NetBuilder::new();
    let bay = inner.place::<Unit>("Bay");
    inner.keys(bay, ["name", "kind"], |u: &Unit| {
        [u.name.as_str().into(), u.kind.as_str().into()]
    });
    inner.port(bay);
    inner.seed(bay, [unit("Ada", "Van"), unit("Ben", "Bike")]);
    let yard = inner.build().unwrap();

    let mut d = depot(&[order("Lamp", "Cleo"), order("Book", "Ben")], &[]);
    let yard = d.b.subnet_place("Yard", yard);
    let bay = d.b.port_of::<Unit>(yard, "Bay");
    let load =
        d.b.transition(
            "Load",
            (TakeEdge(d.orders), MutateWhereEdge(bay, named_by(d.orders))),
            (d.done,),
            |order: Order, unit: &mut Unit| {
                unit.orders += 1;
                (order,)
            },
        )
        .id();
    // A unit in the bay, as the source, sends the order it names.
    let send =
        d.b.transition(
            "Send",
            (
                SnapshotEdge(bay),
                TakeWhereEdge(d.orders, Filter::new().key_is_input("unit", bay, "name")),
            ),
            (d.done,),
            |_: Unit, order: Order| (order,),
        )
        .id();
    let Depot {
        b, orders, done, ..
    } = d;
    let mut net = b.build().unwrap();
    net.disable(send).unwrap();

    net.fire(load).unwrap();
    assert_eq!(items(&net, done), ["Book"]);
    assert_eq!(units(&net, bay), [("Ada".into(), 0), ("Ben".into(), 1)]);
    assert!(!net.is_fireable(load), "no Cleo in the bay");

    net.enable(send).unwrap();
    net.put(orders, order("Kite", "Ada"));
    net.fire(send).unwrap();
    assert_eq!(
        items(&net, done),
        ["Book", "Kite"],
        "Ada, in the bay, sent the kite"
    );
    assert_eq!(items(&net, orders), ["Lamp"]);
}

// ----- the serialised form --------------------------------------------------------

#[cfg(feature = "serde")]
mod json {
    use super::*;
    use petritype::serde_json::{self, json};
    use petritype::{Clause, InputKey};

    /// The clause is on its edge as declared, its source by the core's id.
    #[test]
    fn an_edge_serialises_the_clause_and_a_filter_reads_it_back() {
        let mut d = depot(&[], &[]);
        load(&mut d);
        let net = d.b.build().unwrap();
        let topology = serde_json::to_value(net.topology()).unwrap();
        let bound = topology["edges"]
            .as_array()
            .unwrap()
            .iter()
            .find(|edge| edge["kind"] == "mutate_where")
            .unwrap();
        assert_eq!(
            bound["filter"],
            json!({"clauses": [{"key": "name", "input": {"place": 0, "key": "unit"}}]})
        );
        let filter: Filter = serde_json::from_value(bound["filter"].clone()).unwrap();
        assert_eq!(
            filter.clauses,
            [Clause::IsInput {
                key: "name".into(),
                input: InputKey {
                    place: d.orders.id(),
                    key: "unit".into(),
                },
            }]
        );
    }
}

// ----- a struct of inputs ------------------------------------------------------------

/// A body that takes its inputs as a struct, bound by name, may have a bound
/// edge among them: the struct of edges describes each edge with its filter.
#[test]
fn a_bound_edge_may_fill_a_field_of_a_struct_of_inputs() {
    #[derive(petritype::Inputs)]
    struct Loading<'a> {
        order: Order,
        unit: &'a mut Unit,
    }

    let mut d = depot(
        &[order("Lamp", "Cleo"), order("Book", "Ben")],
        &[unit("Ada", "Van"), unit("Ben", "Bike")],
    );
    let load =
        d.b.transition(
            "Load",
            (LoadingEdges {
                order: TakeEdge(d.orders),
                unit: MutateWhereEdge(d.units, named_by(d.orders)),
            },),
            (d.done,),
            |loading: Loading<'_>| {
                loading.unit.orders += 1;
                (loading.order,)
            },
        )
        .id();
    let Depot {
        b,
        units: unit_place,
        done,
        ..
    } = d;
    let mut net = b.build().unwrap();
    net.fire(load).unwrap();
    assert_eq!(items(&net, done), ["Book"]);
    assert_eq!(
        units(&net, unit_place),
        [("Ada".into(), 0), ("Ben".into(), 1)]
    );
}
