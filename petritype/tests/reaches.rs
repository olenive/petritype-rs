//! Integration tests for an input edge that names the ports it reaches in
//! the nets its tokens carry (DESIGN §8.5, 2026-11-23): `Reaching`, made by
//! `.reaching([..])` on every edge kind.
//!
//! It is carried on the read API, the topology and the snapshot, refused on
//! a place whose tokens carry no nets and for an empty name, and since
//! 2026-11-28 a body is held to it (DESIGN §6.9): it writes a closed net it
//! is handed at the ports its edge names alone, by every edge kind that
//! hands a net over, on a thread or not, and an edge that names none lets
//! it write no port. The names are an upper bound, a net declared open does
//! without them, and a place that is no port stays closed whatever they
//! say. The rules for the nets inside are in `tests/closed_nets.rs`.

use std::panic::{AssertUnwindSafe, catch_unwind};

use petritype::drivers::{self, DriverError, Limits, Mode, OnFailure};
use petritype::{
    BuildError, ClosedRefusal, Command, DrainEdge, EdgeInfo, Event, Filter, FiringFailed, KeyValue,
    MutateEdge, MutateWhereEdge, Net, NetBuilder, NetError, PlaceRef, ReadAllEdge, ReadEdge,
    ReadWhereEdge, StepError, SurveyEdge, TakeAllEdge, TakeEdge, TakeWhereEdge, TokenId,
};

/// A unit: `Orders In` and `Shots Out`, both ports, and `Notes`, which is
/// not. Closed.
fn unit() -> Net {
    unit_of(true, false, &[])
}

/// The reaches of every input edge, in edge order.
fn reaches(net: &Net) -> Vec<Vec<String>> {
    net.edges()
        .filter_map(|edge| match edge {
            EdgeInfo::Input { reaches, .. } => Some(reaches),
            _ => None,
        })
        .collect()
}

fn names(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

#[test]
fn every_edge_kind_may_name_the_ports_it_reaches() {
    let mut b = NetBuilder::new();
    let places: Vec<PlaceRef<Net>> = (0..11)
        .map(|i| b.place_nested::<Net>(format!("Field {i}")))
        .collect();
    for &place in &places[4..6] {
        b.keys(place, ["size"], |net: &Net| {
            [petritype::KeyValue::Int(net.places().count() as i64)]
        });
    }
    b.keys(places[9], ["size"], |net: &Net| {
        [petritype::KeyValue::Int(net.places().count() as i64)]
    });
    let any = Filter::new();
    b.transition(
        "Everything",
        (
            TakeEdge(places[0]).reaching(["Orders In"]),
            TakeAllEdge(places[1]).reaching(["Orders In"]),
            MutateEdge(places[2]).reaching(["Orders In", "Shots Out"]),
            DrainEdge(places[3]).reaching(["Shots Out"]),
            TakeWhereEdge(places[4], any.clone()).reaching(["Orders In"]),
            MutateWhereEdge(places[5], any.clone()).reaching(["Orders In"]),
            ReadEdge(places[6]).reaching(["Shots Out"]),
            ReadAllEdge(places[7]).reaching(["Shots Out"]),
            TakeEdge(places[8]),
            ReadWhereEdge(places[9], any).reaching(["Orders In"]),
        ),
        (),
        |_: Net,
         _: Vec<Net>,
         _: &mut Net,
         _: Vec<Net>,
         _: Net,
         _: &mut Net,
         _: &Net,
         _: Vec<&Net>,
         _: Net,
         _: &Net| {},
    );
    let net = b.build().unwrap();
    assert_eq!(
        reaches(&net),
        [
            names(&["Orders In"]),
            names(&["Orders In"]),
            names(&["Orders In", "Shots Out"]),
            names(&["Shots Out"]),
            names(&["Orders In"]),
            names(&["Orders In"]),
            names(&["Shots Out"]),
            names(&["Shots Out"]),
            Vec::new(),
            names(&["Orders In"]),
        ],
        "in declaration order, and none for the edge that names none"
    );
    let (_, info) = net.transitions().next().unwrap();
    assert_eq!(info.reaches.len(), info.inputs.len());
    // A snapshot copies a place of nets by forking its tokens, which takes
    // the `serde` feature.
    #[cfg(feature = "serde")]
    assert_eq!(
        net.snapshot().unwrap().edges().collect::<Vec<_>>(),
        net.edges().collect::<Vec<_>>(),
        "a snapshot carries them too"
    );
    let _ = (SurveyEdge(places[10]).reaching(["x"]), places[10]);
}

// ----- the reach enforced (since 2026-11-28) -------------------------------------

/// A unit: `Orders In` and `Shots Out`, its ports unless `ports` is false,
/// and `Notes`, which is not; `Shots Out` starts with `shots`, which are
/// inside. Declared open when `open` is.
fn unit_of(ports: bool, open: bool, shots: &[&str]) -> Net {
    let mut b = NetBuilder::new();
    let orders_in = b.place::<String>("Orders In");
    let shots_out = b.place::<String>("Shots Out");
    b.place::<String>("Notes");
    if ports {
        b.port(orders_in);
        b.port(shots_out);
    }
    if open {
        b.open();
    }
    b.seed(shots_out, shots.iter().map(|shot| (*shot).to_owned()));
    b.build().expect("a unit")
}

fn strings(net: &Net, name: &str) -> Vec<String> {
    let id = net.place_named(name).expect("a place");
    net.tokens(id)
        .filter_map(|(token, _)| net.token::<String>(id, token).cloned())
        .collect()
}

/// Puts the order `"Hold"` at `place` of `net`.
fn put_at(net: &mut Net, place: &str) -> Result<TokenId, NetError> {
    let place = net.place_ref::<String>(place).expect("a place of strings");
    net.try_put(place, "Hold".to_owned())
}

/// What a body does with the unit it is handed. The refusal it may meet is
/// dropped: the firing fails with it all the same.
type Act = Box<dyn Fn(&mut Net) + Send + Sync>;

/// Puts `"Hold"` at `place`, dropping a refusal.
fn putting(place: &'static str) -> Act {
    Box::new(move |net: &mut Net| {
        let _ = put_at(net, place);
    })
}

/// The front unit of `Field`, to read.
fn front(net: &Net) -> &Net {
    let field = net.place_named("Field").expect("a field");
    let (token, _) = net.tokens(field).next().expect("a unit in the field");
    net.token::<Net>(field, token).expect("a net")
}

/// The refusal a firing failed with: which, and at which place.
fn refused(failed: &FiringFailed) -> (ClosedRefusal, Option<String>) {
    match failed.error.downcast_ref::<NetError>() {
        Some(NetError::ClosedNet { why, place, .. }) => (why.clone(), place.clone()),
        _ => panic!("a ClosedNet refusal, not {:?}", failed.error),
    }
}

/// A field around `unit`: `Orders` holding `"Hold"`, `Field` holding the
/// unit, `Done`, and `Use`, which takes the order and lends the body the
/// unit through a mutate edge naming `reach`; the body does `act` to it and
/// puts the order in `Done`. Declared open when `open` is; `Use` restores
/// what it took on failure when `restore` is.
fn field(unit: Net, reach: &[&str], act: Act, open: bool, restore: bool) -> Net {
    let mut b = NetBuilder::new();
    let orders = b.place::<String>("Orders");
    let field = b.place_nested::<Net>("Field");
    let done = b.place::<String>("Done");
    b.seed(orders, ["Hold".to_owned()]);
    b.seed(field, [unit]);
    if open {
        b.open();
    }
    let reach: Vec<String> = reach.iter().map(|name| (*name).to_owned()).collect();
    let mut used = b.transition(
        "Use",
        (TakeEdge(orders), MutateEdge(field).reaching(reach)),
        (done,),
        move |order: String, unit: &mut Net| {
            act(unit);
            (order,)
        },
    );
    if restore {
        used.restore_tokens_on_failure();
    }
    b.build().expect("a field")
}

/// The one firing of a field: its `Use`.
fn fails(mut net: Net) -> (Net, FiringFailed) {
    let failed = net
        .step()
        .expect_err("the body is refused")
        .into_firing()
        .unwrap();
    (net, failed)
}

#[test]
fn a_put_lands_at_a_port_the_edge_names_and_is_refused_at_another() {
    let mut net = field(unit(), &["Orders In"], putting("Orders In"), false, false);
    net.step().expect("a port the edge names").expect("Use");
    assert_eq!(strings(front(&net), "Orders In"), ["Hold"]);
    assert_eq!(strings(&net, "Done"), ["Hold"]);

    let (net, failed) = fails(field(
        unit(),
        &["Orders In"],
        putting("Shots Out"),
        false,
        false,
    ));
    assert_eq!(
        refused(&failed),
        (ClosedRefusal::Undeclared, Some("Shots Out".to_owned()))
    );
    assert_eq!(strings(front(&net), "Shots Out"), Vec::<String>::new());
}

#[test]
fn the_refusal_names_the_port_and_how_to_name_it_and_fails_the_firing_as_any_refusal() {
    let unit = unit();
    let name = format!("{:?}", unit.id());
    let (net, failed) = fails(field(
        unit,
        &["Orders In"],
        putting("Shots Out"),
        false,
        true,
    ));
    let error = failed.error.downcast_ref::<NetError>().expect("a NetError");
    assert!(
        matches!(error, NetError::ClosedNet { net, place: Some(place), act: "put a token into", why: ClosedRefusal::Undeclared }
            if *net == name && place == "Shots Out"),
        "{error:?}"
    );
    let message = error.to_string();
    assert!(
        message.contains("\"Shots Out\"") && message.contains(".reaching([\"Shots Out\"])"),
        "says what to name: {message}"
    );
    assert!(failed.restored);
    assert_eq!(strings(&net, "Orders"), ["Hold"], "the order is back");
    assert_eq!(strings(&net, "Done"), Vec::<String>::new());
    assert_eq!(net.count(net.place_named("Field").unwrap()), 1);
}

#[test]
fn a_refusal_the_body_drops_or_catches_fails_the_firing_all_the_same() {
    let caught: Act = Box::new(|net: &mut Net| {
        let shots = net.place_ref::<String>("Shots Out").unwrap();
        let _ = catch_unwind(AssertUnwindSafe(|| net.put(shots, "Hold".to_owned())));
    });
    for act in [putting("Shots Out"), caught] {
        let (net, failed) = fails(field(unit(), &["Orders In"], act, false, false));
        assert_eq!(refused(&failed).0, ClosedRefusal::Undeclared);
        assert_eq!(strings(front(&net), "Shots Out"), Vec::<String>::new());
    }
}

#[test]
fn an_edge_that_names_no_port_lets_its_body_write_none() {
    for port in ["Orders In", "Shots Out"] {
        let (net, failed) = fails(field(unit(), &[], putting(port), false, false));
        assert_eq!(
            refused(&failed),
            (ClosedRefusal::Undeclared, Some(port.to_owned()))
        );
        assert_eq!(strings(front(&net), port), Vec::<String>::new());
    }
}

#[test]
fn a_removal_is_refused_at_a_port_the_edge_does_not_name_and_lands_at_one_it_does() {
    let take_a_shot: fn() -> Act = || {
        Box::new(|net: &mut Net| {
            let shots = net.place_named("Shots Out").unwrap();
            let (shot, _) = net.tokens(shots).next().expect("a shot");
            let _ = net.remove(shots, shot);
        })
    };
    let (net, failed) = fails(field(
        unit_of(true, false, &["Shot: Ready"]),
        &["Orders In"],
        take_a_shot(),
        false,
        false,
    ));
    let error = failed.error.downcast_ref::<NetError>().unwrap();
    assert!(
        matches!(
            error,
            NetError::ClosedNet {
                act: "remove a token from",
                why: ClosedRefusal::Undeclared,
                ..
            }
        ),
        "{error:?}"
    );
    assert_eq!(strings(front(&net), "Shots Out"), ["Shot: Ready"]);

    let mut net = field(
        unit_of(true, false, &["Shot: Ready"]),
        &["Shots Out"],
        take_a_shot(),
        false,
        false,
    );
    net.step().expect("named").expect("Use");
    assert_eq!(strings(front(&net), "Shots Out"), Vec::<String>::new());
}

#[test]
fn a_command_a_body_applies_is_held_to_it_even_when_dropped() {
    let commands: [fn(PlaceRef<String>) -> Command; 2] = [
        |shots| Command::put(shots, "Hold".to_owned()),
        |shots| Command::set_tokens(shots, ["Hold".to_owned()]),
    ];
    for command in commands {
        let applying = move || -> Act {
            Box::new(move |net: &mut Net| {
                let shots = net.place_ref::<String>("Shots Out").unwrap();
                let _ = net.apply(command(shots));
            })
        };
        let mut net = field(unit(), &["Shots Out"], applying(), false, false);
        net.step().expect("named").expect("Use");
        assert_eq!(strings(front(&net), "Shots Out"), ["Hold"]);
        let (net, failed) = fails(field(unit(), &["Orders In"], applying(), false, false));
        assert_eq!(refused(&failed).0, ClosedRefusal::Undeclared);
        assert_eq!(strings(front(&net), "Shots Out"), Vec::<String>::new());
    }
}

/// The edge kinds that hand a body a token's net to write: the takes and
/// the drain own it, the mutates lend it. A read edge lends `&Net`, which
/// has nothing to write with, and a snapshot or a survey cannot stand on a
/// place of nets, which cannot clone its tokens.
#[derive(Debug, Clone, Copy)]
enum Kind {
    Take,
    TakeWhere,
    TakeAll,
    Drain,
    Mutate,
    MutateWhere,
}

/// A field around a unit whose `Use` hands the body the unit by `kind`,
/// naming `Orders In`, and puts `"Hold"` at `port` of it (of the first, for
/// a batch); a unit taken goes back to the field.
fn by_kind(kind: Kind, port: &'static str) -> Net {
    let mut b = NetBuilder::new();
    let orders = b.place::<String>("Orders");
    let field = b.place_nested::<Net>("Field");
    let done = b.place::<String>("Done");
    b.keys(field, ["name"], |_: &Net| [KeyValue::from("unit")]);
    b.seed(orders, ["Hold".to_owned()]);
    b.seed(field, [unit()]);
    let the_unit = || Filter::new().key_in("name", ["unit"]);
    let reach = ["Orders In"];
    let one = move |order: String, mut unit: Net| {
        let _ = put_at(&mut unit, port);
        (order, unit)
    };
    let all = move |order: String, mut units: Vec<Net>| {
        let _ = put_at(&mut units[0], port);
        (order, units)
    };
    let lent = move |order: String, unit: &mut Net| {
        let _ = put_at(unit, port);
        (order,)
    };
    let order = TakeEdge(orders);
    match kind {
        Kind::Take => {
            b.transition(
                "Use",
                (order, TakeEdge(field).reaching(reach)),
                (done, field),
                one,
            );
        }
        Kind::TakeWhere => {
            let edge = TakeWhereEdge(field, the_unit()).reaching(reach);
            b.transition("Use", (order, edge), (done, field), one);
        }
        Kind::TakeAll => {
            let edge = TakeAllEdge(field).reaching(reach);
            b.transition("Use", (order, edge), (done, field), all);
        }
        Kind::Drain => {
            let edge = DrainEdge(field).reaching(reach);
            b.transition("Use", (order, edge), (done, field), all);
        }
        Kind::Mutate => {
            b.transition(
                "Use",
                (order, MutateEdge(field).reaching(reach)),
                (done,),
                lent,
            );
        }
        Kind::MutateWhere => {
            let edge = MutateWhereEdge(field, the_unit()).reaching(reach);
            b.transition("Use", (order, edge), (done,), lent);
        }
    }
    b.build().expect("a field")
}

#[test]
fn every_edge_that_hands_the_net_over_is_held_to_its_reach() {
    for kind in [
        Kind::Take,
        Kind::TakeWhere,
        Kind::TakeAll,
        Kind::Drain,
        Kind::Mutate,
        Kind::MutateWhere,
    ] {
        let mut net = by_kind(kind, "Orders In");
        net.step().expect("named").expect("Use");
        assert_eq!(strings(front(&net), "Orders In"), ["Hold"], "{kind:?}");
        let (_, failed) = fails(by_kind(kind, "Shots Out"));
        assert_eq!(
            refused(&failed),
            (ClosedRefusal::Undeclared, Some("Shots Out".to_owned())),
            "{kind:?}"
        );
    }
}

#[test]
fn each_edge_of_a_firing_opens_only_its_own_tokens_nets_ports() {
    /// `Front` and `Back`, a unit each, and `Use`, whose edge on the front
    /// names `Orders In` and on the back `Shots Out`; the body puts the
    /// order at `front` of the front unit and at `back` of the back one.
    fn two_fields(front: Option<&'static str>, back: Option<&'static str>) -> Net {
        let mut b = NetBuilder::new();
        let orders = b.place::<String>("Orders");
        let front_field = b.place_nested::<Net>("Front");
        let back_field = b.place_nested::<Net>("Back");
        b.seed(orders, ["Hold".to_owned()]);
        b.seed(front_field, [unit()]);
        b.seed(back_field, [unit()]);
        b.transition(
            "Use",
            (
                TakeEdge(orders),
                MutateEdge(front_field).reaching(["Orders In"]),
                MutateEdge(back_field).reaching(["Shots Out"]),
            ),
            (),
            move |_: String, front_unit: &mut Net, back_unit: &mut Net| {
                for (unit, port) in [(front_unit, front), (back_unit, back)] {
                    if let Some(port) = port {
                        let _ = put_at(unit, port);
                    }
                }
            },
        );
        b.build().expect("two fields")
    }
    fn unit_in<'a>(net: &'a Net, place: &str) -> &'a Net {
        let place = net.place_named(place).unwrap();
        let (token, _) = net.tokens(place).next().unwrap();
        net.token::<Net>(place, token).unwrap()
    }

    let mut net = two_fields(Some("Orders In"), Some("Shots Out"));
    net.step().expect("each its own").expect("Use");
    assert_eq!(strings(unit_in(&net, "Front"), "Orders In"), ["Hold"]);
    assert_eq!(strings(unit_in(&net, "Back"), "Shots Out"), ["Hold"]);
    for (front, back, port) in [
        (Some("Shots Out"), None, "Shots Out"),
        (None, Some("Orders In"), "Orders In"),
    ] {
        let (net, failed) = fails(two_fields(front, back));
        assert_eq!(
            refused(&failed),
            (ClosedRefusal::Undeclared, Some(port.to_owned()))
        );
        for place in ["Front", "Back"] {
            assert_eq!(strings(unit_in(&net, place), port), Vec::<String>::new());
        }
    }
}

#[test]
fn a_body_on_another_thread_is_held_to_it() {
    for (port, caught) in [
        ("Orders In", false),
        ("Shots Out", false),
        ("Shots Out", true),
    ] {
        let mut b = NetBuilder::new();
        let field = b.place_nested::<Net>("Field");
        b.seed(field, [unit()]);
        let threads = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = std::sync::Arc::clone(&threads);
        b.transition(
            "Use",
            (TakeEdge(field).reaching(["Orders In"]),),
            (field,),
            move |mut unit: Net| {
                seen.lock().unwrap().push(std::thread::current().id());
                if caught {
                    let tray = unit.place_ref::<String>(port).unwrap();
                    let _ = catch_unwind(AssertUnwindSafe(|| unit.put(tray, "Hold".to_owned())));
                } else {
                    let _ = put_at(&mut unit, port);
                }
                (unit,)
            },
        )
        .guard(move |net| net.count(field.id()) == 1);
        let mut net = b.build().expect("a field");
        let firing = net.launch(net.transition_named("Use").unwrap()).unwrap();
        assert!(firing.is_pending(), "detachable");
        let completed = std::thread::spawn(move || firing.run()).join().unwrap();
        let landed = net.distribute(completed).map_err(NetError::from);
        assert_ne!(threads.lock().unwrap()[0], std::thread::current().id());
        if port == "Orders In" {
            landed.expect("named");
            assert_eq!(strings(front(&net), "Orders In"), ["Hold"]);
        } else {
            let Err(NetError::FiringFailed(failed)) = landed else {
                panic!("refused: {landed:?}");
            };
            assert_eq!(
                refused(&failed).0,
                ClosedRefusal::Undeclared,
                "caught: {caught}"
            );
        }
    }
}

#[test]
fn the_concurrent_driver_holds_a_body_to_it() {
    let mut b = NetBuilder::new();
    let field = b.place_nested::<Net>("Field");
    let done = b.place_nested::<Net>("Done");
    b.seed(field, [unit()]);
    b.transition(
        "Use",
        (TakeEdge(field).reaching(["Orders In"]),),
        (done,),
        |mut unit: Net| {
            let _ = put_at(&mut unit, "Shots Out");
            (unit,)
        },
    );
    let mut net = b.build().expect("a field");
    let error = drivers::run(
        &mut net,
        Mode::Concurrent,
        Limits::default(),
        OnFailure::Stop,
    )
    .expect_err("refused");
    let DriverError::FiringFailed { error: failed, .. } = error else {
        panic!("a failed firing: {error:?}");
    };
    assert_eq!(refused(&failed).0, ClosedRefusal::Undeclared);
    assert!(net.in_flight().is_empty());
}

#[test]
fn a_body_writes_any_port_of_a_net_it_builds_and_none_of_the_net_it_holds() {
    fn built_beside(write_the_held_one: bool) -> Net {
        let mut b = NetBuilder::new();
        let orders = b.place::<String>("Orders");
        let field = b.place_nested::<Net>("Field");
        let built = b.place_nested::<Net>("Built");
        b.seed(orders, ["Hold".to_owned()]);
        b.seed(field, [unit()]);
        b.transition(
            "Build",
            (TakeEdge(orders), MutateEdge(field)),
            (built,),
            move |order: String, held: &mut Net| {
                let mut fresh = unit();
                let target = if write_the_held_one { held } else { &mut fresh };
                let tray = target.place_ref::<String>("Orders In").unwrap();
                let _ = target.try_put(tray, order);
                (fresh,)
            },
        );
        b.build().expect("a builder")
    }

    let mut net = built_beside(false);
    net.step().expect("a net no edge handed it").expect("Build");
    let built = net.place_named("Built").unwrap();
    let (token, _) = net.tokens(built).next().expect("the new unit");
    let fresh = net.token::<Net>(built, token).unwrap();
    assert!(fresh.is_closed());
    assert_eq!(strings(fresh, "Orders In"), ["Hold"]);

    let (net, failed) = fails(built_beside(true));
    assert_eq!(refused(&failed).0, ClosedRefusal::Undeclared);
    assert_eq!(strings(front(&net), "Orders In"), Vec::<String>::new());
}

#[test]
fn the_program_writes_the_port_the_body_was_refused_once_the_field_is_declared_open() {
    // The field is closed: it shields the unit from the program too, which
    // is refused there as the net around, not as an undeclared port.
    let (mut net, failed) = fails(field(
        unit_of(true, false, &["Shot: Ready"]),
        &[],
        putting("Orders In"),
        false,
        false,
    ));
    assert_eq!(refused(&failed).0, ClosedRefusal::Undeclared);
    let around = format!("{:?}", net.id());
    let field_place = net.place_named("Field").unwrap();
    let (token, _) = net.tokens(field_place).next().unwrap();
    let refusal = net
        .nested_scope(field_place, token, |nets| put_at(nets[0].1, "Orders In"))
        .expect("a nested place")
        .expect_err("shielded");
    assert!(
        matches!(&refusal, NetError::ClosedNet { why: ClosedRefusal::Around { net }, .. } if *net == around),
        "{refusal:?}"
    );
    assert_eq!(strings(front(&net), "Orders In"), Vec::<String>::new());

    // Declared open: the body is refused as before, its unit being closed,
    // and the program writes both ports.
    let (mut net, failed) = fails(field(
        unit_of(true, false, &["Shot: Ready"]),
        &[],
        putting("Orders In"),
        true,
        false,
    ));
    assert_eq!(refused(&failed).0, ClosedRefusal::Undeclared);
    let field_place = net.place_named("Field").unwrap();
    let (token, _) = net.tokens(field_place).next().unwrap();
    net.nested_scope(field_place, token, |nets| {
        let unit = &mut *nets[0].1;
        put_at(unit, "Orders In").expect("a port, inside an open field");
        let shots = unit.place_named("Shots Out").unwrap();
        let (shot, _) = unit.tokens(shots).next().unwrap();
        unit.remove(shots, shot).expect("a port");
        let shots = unit.place_ref::<String>("Shots Out").unwrap();
        unit.apply(Command::put(shots, "Shot: Again".to_owned()))
            .expect("a port");
    })
    .expect("a nested place");
    assert_eq!(strings(front(&net), "Orders In"), ["Hold"]);
    assert_eq!(strings(front(&net), "Shots Out"), ["Shot: Again"]);
}

#[test]
fn only_a_net_declared_open_does_without_it() {
    let everywhere = || -> Act {
        Box::new(|net: &mut Net| {
            for place in ["Orders In", "Shots Out", "Notes"] {
                let _ = put_at(net, place);
            }
        })
    };
    for ports in [true, false] {
        let mut net = field(unit_of(ports, true, &[]), &[], everywhere(), false, false);
        net.step().expect("open").expect("Use");
        for place in ["Orders In", "Shots Out", "Notes"] {
            assert_eq!(strings(front(&net), place), ["Hold"], "{place}");
        }
    }
    let (net, failed) = fails(field(unit(), &[], everywhere(), false, false));
    assert_eq!(
        refused(&failed),
        (ClosedRefusal::Undeclared, Some("Orders In".to_owned()))
    );
    assert_eq!(strings(front(&net), "Orders In"), Vec::<String>::new());
    // A net that marks no port has nothing an edge could name.
    let (net, failed) = fails(field(
        unit_of(false, false, &[]),
        &["Orders In"],
        everywhere(),
        false,
        false,
    ));
    assert_eq!(
        refused(&failed),
        (ClosedRefusal::NotAPort, Some("Orders In".to_owned()))
    );
    assert_eq!(strings(front(&net), "Orders In"), Vec::<String>::new());
}

#[test]
fn a_place_that_is_no_port_stays_closed_whatever_the_edge_names() {
    let (net, failed) = fails(field(unit(), &["Notes"], putting("Notes"), false, false));
    assert_eq!(
        refused(&failed),
        (ClosedRefusal::NotAPort, Some("Notes".to_owned()))
    );
    assert_eq!(strings(front(&net), "Notes"), Vec::<String>::new());
    let (_, failed) = fails(field(
        unit(),
        &["Notes"],
        putting("Orders In"),
        false,
        false,
    ));
    assert_eq!(refused(&failed).0, ClosedRefusal::Undeclared);
}

/// The reach is an upper bound on what a body writes: naming more fires
/// exactly as naming what it writes, with the same events, and leaving out
/// what it writes is refused.
#[test]
fn naming_a_port_the_body_never_writes_or_one_the_net_lacks_changes_nothing() {
    /// The events, each as its `Debug` with the inner net's id left out:
    /// every unit built is a new net with an id of its own.
    fn run(reach: &[&str]) -> Result<(Vec<String>, usize), StepError> {
        let mut b = NetBuilder::new();
        let orders = b.place::<String>("Orders");
        let field = b.place_nested::<Net>("Field");
        b.seed(field, [unit()]);
        b.seed(orders, ["Hold".to_owned()]);
        let reach: Vec<String> = reach.iter().map(|name| (*name).to_owned()).collect();
        b.transition(
            "Hand Over",
            (TakeEdge(orders), MutateEdge(field).reaching(reach)),
            (),
            |order: String, unit: &mut Net| {
                let tray = unit.place_ref::<String>("Orders In").unwrap();
                unit.put(tray, order);
            },
        );
        let mut net = b.build().unwrap();
        net.drain_events().for_each(drop);
        net.step()?;
        let count = strings(front(&net), "Orders In").len();
        let events = net
            .drain_events()
            .map(|event: Event| {
                let text = format!("{event:?}");
                match text.find("nested_id: NetId(") {
                    Some(at) => {
                        let close = text[at..].find(')').map_or(text.len(), |end| at + end + 1);
                        format!("{}{}", &text[..at], &text[close..])
                    }
                    None => text,
                }
            })
            .collect();
        Ok((events, count))
    }

    let exact = run(&["Orders In"]).expect("named");
    assert_eq!(exact.1, 1);
    for reach in [
        &["Orders In", "Shots Out"][..],
        &["Orders In", "Mess Hall"],
        &["Mess Hall", "Orders In"],
    ] {
        assert_eq!(run(reach).expect("named"), exact, "{reach:?}");
    }
    let failed = run(&["Shots Out", "Mess Hall"])
        .expect_err("Orders In is not named")
        .into_firing()
        .unwrap();
    assert_eq!(
        refused(&failed),
        (ClosedRefusal::Undeclared, Some("Orders In".to_owned()))
    );
}

#[test]
fn a_reach_is_refused_on_a_place_of_no_nets_and_for_an_empty_name() {
    let mut b = NetBuilder::new();
    let orders = b.place::<String>("Orders");
    b.transition(
        "Hand Over",
        (TakeEdge(orders).reaching(["Orders In"]),),
        (),
        |_: String| {},
    );
    let error = b.build().expect_err("orders carry no nets");
    assert!(
        matches!(&error, BuildError::ReachIntoNoNets { transition, place }
            if transition == "Hand Over" && place == "Orders"),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "transition \"Hand Over\" names ports it reaches through place \"Orders\", whose tokens \
         carry no nets"
    );

    let mut b = NetBuilder::new();
    let field = b.place_nested::<Net>("Field");
    b.transition(
        "Look",
        (ReadEdge(field).reaching(["Orders In", ""]),),
        (),
        |_: &Net| {},
    );
    assert!(matches!(
        b.build(),
        Err(BuildError::EmptyReachName { place, .. }) if place == "Field"
    ));
}

/// A field of a struct of inputs may name the ports it reaches.
#[test]
fn a_field_of_a_struct_of_inputs_may_reach() {
    #[derive(petritype::Inputs)]
    struct Handing<'a> {
        order: String,
        unit: &'a mut Net,
    }

    let mut b = NetBuilder::new();
    let orders = b.place::<String>("Orders");
    let field = b.place_nested::<Net>("Field");
    b.transition(
        "Hand Over",
        (HandingEdges {
            order: TakeEdge(orders),
            unit: MutateEdge(field).reaching(["Orders In"]),
        },),
        (),
        |h: Handing<'_>| {
            let tray = h.unit.place_ref::<String>("Orders In").unwrap();
            h.unit.put(tray, h.order);
        },
    );
    let net = b.build().unwrap();
    assert_eq!(reaches(&net), [Vec::new(), names(&["Orders In"])]);
}

#[cfg(feature = "serde")]
mod json {
    use super::*;
    use petritype::serde_json::{self, json};

    /// With `serde` an edge writes `reaches` only when it names some, and the
    /// topology's transitions likewise.
    #[test]
    fn reaches_are_written_only_when_there_are_some() {
        let mut b = NetBuilder::new();
        let orders = b.place::<String>("Orders");
        let field = b.place_nested::<Net>("Field");
        b.transition(
            "Hand Over",
            (TakeEdge(orders), MutateEdge(field).reaching(["Orders In"])),
            (),
            |_: String, _: &mut Net| {},
        );
        let net = b.build().unwrap();
        let topology = serde_json::to_value(net.topology()).unwrap();
        assert_eq!(
            topology["edges"][0],
            json!({"edge": "input", "place": 0, "transition": 0, "kind": "take"})
        );
        assert_eq!(topology["edges"][1]["reaches"], json!(["Orders In"]));
        assert_eq!(
            topology["transitions"][0]["reaches"],
            json!([[], ["Orders In"]])
        );
    }
}
