//! Integration tests for the read edges (DESIGN §8.4, 2026-11-23):
//! `ReadEdge`, `ReadWhereEdge` and `ReadAllEdge`, which lend the body tokens
//! to look at without taking or copying them.
//!
//! What is tested here: each form, and that nothing moves, no id changes and
//! no event names a token read; a place of net-carrying tokens read without
//! a copy, its nets keeping their ids; an empty place under a read-all; a
//! read edge as the source of a clause that chooses by another input, and a
//! bound read-where; a port; detachability and the inline firing; a struct
//! of inputs; the build checks; and the serialised kinds. The compile-time
//! half, that the body cannot change what it is lent, is in `tests/ui`.

use petritype::{
    BuildError, EdgeKind, Event, Filter, Net, NetBuilder, PlaceRef, ReadAllEdge, ReadEdge,
    ReadWhereEdge, TakeEdge, TokenId,
};

/// A token type that is not `Clone`: what a read edge never asks for.
#[derive(Debug, PartialEq)]
struct Gauge {
    name: String,
    level: i64,
}

fn gauge(name: &str, level: i64) -> Gauge {
    Gauge {
        name: name.to_owned(),
        level,
    }
}

/// `Gauges`, declared with `place_no_clone` and keyed by `name`; `Asks`,
/// strings, a port where asks arrive; `Log`, strings.
struct Panel {
    b: NetBuilder,
    gauges: PlaceRef<Gauge>,
    asks: PlaceRef<String>,
    log: PlaceRef<String>,
}

fn panel(gauges: Vec<Gauge>) -> Panel {
    let mut b = NetBuilder::new();
    let gauges_place = b.place_no_clone::<Gauge>("Gauges");
    let asks = b.place::<String>("Asks");
    let log = b.place::<String>("Log");
    b.port(asks);
    b.keys(gauges_place, ["name"], |g: &Gauge| [g.name.as_str().into()]);
    b.seed(gauges_place, gauges);
    Panel {
        b,
        gauges: gauges_place,
        asks,
        log,
    }
}

fn ids<T: 'static>(net: &Net, place: PlaceRef<T>) -> Vec<TokenId> {
    net.tokens(place.id()).map(|(id, _)| id).collect()
}

fn lines(net: &Net, place: PlaceRef<String>) -> Vec<String> {
    net.tokens(place.id())
        .map(|(id, _)| net.token::<String>(place.id(), id).unwrap().clone())
        .collect()
}

/// The front token, lent: nothing moves, no id changes, and the firing's
/// one event names only the token taken beside it.
#[test]
fn a_read_edge_lends_the_front_token_and_nothing_moves() {
    let mut p = panel(vec![gauge("Oil", 3), gauge("Fuel", 7)]);
    let look =
        p.b.transition(
            "Look",
            (TakeEdge(p.asks), ReadEdge(p.gauges)),
            (p.log,),
            |who: String, front: &Gauge| (format!("{who}: {} {}", front.name, front.level),),
        )
        .id();
    let Panel {
        b,
        gauges,
        asks,
        log,
    } = p;
    let mut net = b.build().unwrap();
    let before = ids(&net, gauges);
    let ask = net.put(asks, "Ada".to_owned());
    net.drain_events().for_each(drop);

    assert_eq!(net.step().unwrap(), Some(look));
    assert_eq!(lines(&net, log), ["Ada: Oil 3"]);
    assert_eq!(ids(&net, gauges), before, "both gauges stay, same ids");
    let produced = ids(&net, log)[0];
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Fired {
            transition: look,
            consumed: vec![(asks.id(), ask)],
            produced: vec![(log.id(), produced)],
        }],
        "no event names a gauge"
    );
    assert!(!net.is_fireable(look), "no ask left");
}

/// The oldest token the filter accepts, lent where it stands.
#[test]
fn a_read_where_edge_lends_the_oldest_match() {
    let mut p = panel(vec![gauge("Oil", 3), gauge("Fuel", 7), gauge("Fuel", 9)]);
    // The gauges are reset from outside below.
    p.b.port(p.gauges);
    let look =
        p.b.transition(
            "Fuel",
            (
                TakeEdge(p.asks),
                ReadWhereEdge(p.gauges, Filter::new().key_in("name", ["Fuel"])),
            ),
            (p.log,),
            |_: String, fuel: &Gauge| (format!("{}", fuel.level),),
        )
        .id();
    let Panel {
        b,
        gauges,
        asks,
        log,
        ..
    } = p;
    let mut net = b.build().unwrap();
    let before = ids(&net, gauges);
    net.put(asks, "Ada".to_owned());
    net.fire(look).unwrap();
    assert_eq!(lines(&net, log), ["7"]);
    assert_eq!(ids(&net, gauges), before);

    net.set_tokens(gauges, [gauge("Oil", 1)]);
    net.put(asks, "Bo".to_owned());
    assert!(!net.is_fireable(look), "no fuel gauge, no firing");
}

/// Every token, oldest first, and an empty place does not hold the
/// transition back: the body is lent nothing.
#[test]
fn a_read_all_edge_lends_every_token_and_fires_on_an_empty_place() {
    let mut p = panel(vec![gauge("Oil", 3), gauge("Fuel", 7)]);
    // The gauges are emptied from outside below.
    p.b.port(p.gauges);
    let all =
        p.b.transition(
            "All",
            (TakeEdge(p.asks), ReadAllEdge(p.gauges)),
            (p.log,),
            |_: String, every: Vec<&Gauge>| {
                let names: Vec<&str> = every.iter().map(|g| g.name.as_str()).collect();
                (format!("[{}]", names.join(", ")),)
            },
        )
        .id();
    let Panel {
        b,
        gauges,
        asks,
        log,
        ..
    } = p;
    let mut net = b.build().unwrap();
    let before = ids(&net, gauges);
    net.put(asks, "Ada".to_owned());
    net.fire(all).unwrap();
    assert_eq!(ids(&net, gauges), before);

    net.set_tokens(gauges, []);
    net.put(asks, "Bo".to_owned());
    assert!(
        net.is_fireable(all),
        "an empty place holds a read-all back no more than a survey"
    );
    net.fire(all).unwrap();
    assert_eq!(lines(&net, log), ["[Oil, Fuel]", "[]"]);
}

/// A read edge ties the firing to the net, as a mutate edge does: the
/// transition is not detachable, and `launch` runs its body there and then.
#[test]
fn a_transition_with_a_read_edge_runs_inline() {
    let mut p = panel(vec![gauge("Oil", 3)]);
    let look =
        p.b.transition(
            "Look",
            (TakeEdge(p.asks), ReadAllEdge(p.gauges)),
            (),
            |_: String, _: Vec<&Gauge>| {},
        )
        .id();
    let Panel { b, asks, .. } = p;
    let mut net = b.build().unwrap();
    let (_, info) = net.transitions().find(|(id, _)| *id == look).unwrap();
    assert!(!info.detachable);
    net.put(asks, "Ada".to_owned());
    let firing = net.launch(look).unwrap();
    assert!(!firing.is_pending(), "the body ran inline");
    net.distribute(firing.run()).unwrap();
}

/// A place of net-carrying tokens is read without a copy: the token keeps
/// its id, its net keeps its id, and no event names it.
#[test]
fn a_place_of_nets_is_read_without_copying() {
    let mut inner = NetBuilder::new();
    let members = inner.place::<String>("Members");
    inner.seed(members, ["Ada".to_owned(), "Ben".to_owned()]);
    let squad = inner.build().unwrap();
    let squad_id = squad.id();

    let mut b = NetBuilder::new();
    let barracks = b.place_nested::<Net>("Barracks");
    let calls = b.place::<String>("Calls");
    let roll = b.place::<String>("Roll");
    b.port(calls);
    b.seed(barracks, [squad]);
    let call = b
        .transition(
            "Call",
            (TakeEdge(calls), ReadEdge(barracks)),
            (roll,),
            move |caller: String, squad: &Net| {
                (format!("{caller}: {}", squad.count(members.id())),)
            },
        )
        .id();
    let mut net = b.build().unwrap();
    let token = ids(&net, barracks)[0];
    let call_token = net.put(calls, "Sarge".to_owned());
    net.drain_events().for_each(drop);

    net.fire(call).unwrap();
    assert_eq!(lines(&net, roll), ["Sarge: 2"]);
    assert_eq!(ids(&net, barracks), [token]);
    let nets = net.nested(barracks.id(), token).unwrap();
    assert_eq!(nets[0].1.id(), squad_id, "the same net, not a copy");
    let events: Vec<Event> = net.drain_events().collect();
    assert!(
        matches!(&events[..], [Event::Fired { consumed, .. }] if consumed == &[(calls.id(), call_token)]),
        "{events:?}"
    );
}

/// A read edge gives one token, so it may be the source of a clause that
/// chooses by another input; and a read-where may itself be bound.
#[test]
fn a_read_edge_may_be_a_source_and_a_read_where_may_be_bound() {
    let mut p = panel(vec![gauge("Oil", 3), gauge("Fuel", 7)]);
    let names = p.b.place::<String>("Names");
    p.b.keys(names, ["name"], |n: &String| [n.as_str().into()]);
    p.b.seed(names, ["Gas".to_owned(), "Fuel".to_owned()]);
    // The asked-for gauge, read by the name an ask carries.
    p.b.keys(p.asks, ["name"], |a: &String| [a.as_str().into()]);
    let by_ask =
        p.b.transition(
            "By Ask",
            (
                TakeEdge(p.asks),
                ReadWhereEdge(p.gauges, Filter::new().key_is_input("name", p.asks, "name")),
            ),
            (p.log,),
            |ask: String, g: &Gauge| (format!("{ask} {}", g.level),),
        )
        .id();
    // A name read as the source, and the gauge it names read too.
    let by_name =
        p.b.transition(
            "By Name",
            (
                ReadEdge(names),
                ReadWhereEdge(p.gauges, Filter::new().key_is_input("name", names, "name")),
            ),
            (p.log,),
            |name: &String, g: &Gauge| (format!("{name}={}", g.level),),
        )
        .id();
    let Panel {
        b,
        gauges,
        asks,
        log,
    } = p;
    let mut net = b.build().unwrap();
    let before = ids(&net, gauges);
    net.disable(by_name).unwrap();
    net.put(asks, "Oil".to_owned());
    net.fire(by_ask).unwrap();
    assert_eq!(lines(&net, log), ["Oil 3"]);

    net.disable(by_ask).unwrap();
    net.enable(by_name).unwrap();
    assert!(
        net.is_fireable(by_name),
        "Gas has no gauge, Fuel behind it has"
    );
    net.fire(by_name).unwrap();
    assert_eq!(lines(&net, log), ["Oil 3", "Fuel=7"]);
    assert_eq!(ids(&net, gauges), before);
    assert_eq!(net.count(names.id()), 2, "a read source moves nothing");
}

/// A read edge on a port of a subnet place reads the port's tokens.
#[test]
fn a_read_edge_may_be_a_port() {
    let mut inner = NetBuilder::new();
    let dial = inner.place_no_clone::<Gauge>("Dial");
    inner.keys(dial, ["name"], |g: &Gauge| [g.name.as_str().into()]);
    inner.port(dial);
    inner.seed(dial, [gauge("Oil", 3), gauge("Fuel", 7)]);
    let cabin = inner.build().unwrap();

    let mut p = panel(vec![]);
    let cabin = p.b.subnet_place("Cabin", cabin);
    let dial = p.b.port_of::<Gauge>(cabin, "Dial");
    let spare = dial_twin(&mut p.b);
    let look =
        p.b.transition(
            "Look",
            (
                TakeEdge(p.asks),
                ReadWhereEdge(dial, Filter::new().key_in("name", ["Fuel"])),
                ReadAllEdge(spare),
            ),
            (p.log,),
            |_: String, fuel: &Gauge, others: Vec<&Gauge>| {
                (format!("{} of {}", fuel.level, others.len()),)
            },
        )
        .id();
    let Panel { b, asks, log, .. } = p;
    let mut net = b.build().unwrap();
    let before = ids(&net, dial);
    net.put(asks, "Ada".to_owned());
    net.fire(look).unwrap();
    assert_eq!(lines(&net, log), ["7 of 0"]);
    assert_eq!(ids(&net, dial), before);
}

/// A second place for the port test's read-all, so that no two edges of
/// one transition read one place.
fn dial_twin(b: &mut NetBuilder) -> PlaceRef<Gauge> {
    b.place_no_clone::<Gauge>("Spare Dials")
}

/// A body that takes its inputs as a struct may have read fields.
#[test]
fn read_edges_may_fill_the_fields_of_a_struct_of_inputs() {
    #[derive(petritype::Inputs)]
    struct Looking<'a> {
        who: String,
        front: &'a Gauge,
        every: Vec<&'a Gauge>,
    }

    let mut p = panel(vec![gauge("Oil", 3)]);
    let spare = p.b.place_no_clone::<Gauge>("Spare");
    p.b.seed(spare, [gauge("Fuel", 7), gauge("Gas", 1)]);
    let look =
        p.b.transition(
            "Look",
            (LookingEdges {
                who: TakeEdge(p.asks),
                front: ReadEdge(p.gauges),
                every: ReadAllEdge(spare),
            },),
            (p.log,),
            |l: Looking<'_>| (format!("{}: {} +{}", l.who, l.front.name, l.every.len()),),
        )
        .id();
    let Panel { b, asks, log, .. } = p;
    let mut net = b.build().unwrap();
    net.put(asks, "Ada".to_owned());
    net.fire(look).unwrap();
    assert_eq!(lines(&net, log), ["Ada: Oil +2"]);
}

/// A read edge shares its place with no other edge of its transition, and a
/// read-all gives every token, so it is no source.
#[test]
fn the_build_checks_hold_for_read_edges() {
    let mut p = panel(vec![]);
    p.b.transition(
        "Twice",
        (ReadEdge(p.gauges), ReadAllEdge(p.gauges)),
        (),
        |_: &Gauge, _: Vec<&Gauge>| {},
    );
    assert!(matches!(
        p.b.build(),
        Err(BuildError::EdgeCannotSharePlace { place, kind: EdgeKind::Read, .. })
            if place == "Gauges"
    ));

    let mut p = panel(vec![]);
    let names = p.b.place::<String>("Names");
    p.b.keys(names, ["name"], |n: &String| [n.as_str().into()]);
    p.b.transition(
        "Many",
        (
            ReadAllEdge(names),
            ReadWhereEdge(p.gauges, Filter::new().key_is_input("name", names, "name")),
        ),
        (),
        |_: Vec<&String>, _: &Gauge| {},
    );
    assert!(matches!(
        p.b.build(),
        Err(BuildError::BoundSourceGivesMany { input, .. }) if input == "Names"
    ));

    let mut p = panel(vec![]);
    p.b.transition(
        "Unkeyed",
        (ReadWhereEdge(p.asks, Filter::new().key_in("name", ["x"])),),
        (),
        |_: &String| {},
    );
    assert!(matches!(
        p.b.build(),
        Err(BuildError::KeyedEdgeOnUnkeyedPlace { place, .. }) if place == "Asks"
    ));
}

/// The read API reports the kinds, a read-where with its filter, and none of
/// them consumes or counts towards the opt-in checks.
#[test]
fn the_read_api_reports_the_kinds() {
    let mut p = panel(vec![]);
    let spare = p.b.place_no_clone::<Gauge>("Spare");
    let other = p.b.place_no_clone::<Gauge>("Other");
    p.b.keys(other, ["name"], |g: &Gauge| [g.name.as_str().into()]);
    let fuel = Filter::new().key_in("name", ["Fuel"]);
    p.b.transition(
        "Look",
        (
            ReadEdge(p.gauges),
            ReadAllEdge(spare),
            ReadWhereEdge(other, fuel.clone()),
        ),
        (),
        |_: &Gauge, _: Vec<&Gauge>, _: &Gauge| {},
    );
    p.b.assert_no_source_transitions();
    assert!(
        matches!(p.b.build(), Err(BuildError::SourceTransition { transition }) if transition == "Look"),
        "read edges consume nothing"
    );

    let mut p = panel(vec![]);
    let spare = p.b.place_no_clone::<Gauge>("Spare");
    p.b.transition(
        "Look",
        (ReadEdge(p.gauges), ReadAllEdge(spare)),
        (),
        |_: &Gauge, _: Vec<&Gauge>| {},
    );
    let net = p.b.build().unwrap();
    let (_, info) = net.transitions().next().unwrap();
    assert_eq!(
        info.inputs,
        [
            (p.gauges.id(), EdgeKind::Read),
            (spare.id(), EdgeKind::ReadAll)
        ]
    );
    assert!(EdgeKind::ReadWhere.is_keyed() && !EdgeKind::ReadWhere.consumes());
}

#[cfg(feature = "serde")]
mod json {
    use petritype::EdgeKind;
    use petritype::serde_json::{self, json};

    #[test]
    fn the_read_kinds_serialise_by_the_protocols_names() {
        assert_eq!(
            [EdgeKind::Read, EdgeKind::ReadWhere, EdgeKind::ReadAll]
                .map(|kind| serde_json::to_value(kind).unwrap()),
            [json!("read"), json!("read_where"), json!("read_all")]
        );
    }
}
