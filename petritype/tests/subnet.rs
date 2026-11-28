//! Integration tests for subnet places and port places.
//!
//! A subnet place holds a net of its own, a subnet, which the net around it
//! reaches only through the places the subnet's definition marks as ports.
//! The outer net names a port through a handle of its own
//! ([`NetBuilder::port_of`]), and uses it as any other place: every input
//! edge kind, every way of choosing an output, guards, queries and external
//! input. A port has one queue and one set of ids, the subnet's, whichever
//! net looks at it, and a change is told once, by the net that made it.
//!
//! These tests are each of those in turn: the edges and the outputs on a
//! port, ids and keys across the border both ways, failure, the staged path
//! and the rule that holds a subnet still while a firing on one of its
//! ports is in flight, external input, the events, stepping under each rule
//! and at two depths, tokens that carry nets in a port, reading a subnet
//! place and its ports (`port_ref`, `PlaceInfo`, the topology, snapshots),
//! where a failure says it happened, subnets and nets carried by tokens
//! inside each other, and the build errors. Saving, restoring and forking
//! are in `subnet_saving.rs`, behind the `serde` feature.

use petritype::drivers::{self, Limits, Mode, OnFailure};
use petritype::{
    BuildError, Command, DeepStep, Distribution, DrainEdge, EdgeInfo, EdgeKind, Either, Event,
    Filter, KeyValue, MutateEdge, MutateWhereEdge, Net, NetBuilder, NetError, NetId, PathStep,
    PlaceId, PlaceInfo, PlaceRef, Routed, SnapshotEdge, StagedStep, Stepping, SubnetRef,
    SurveyEdge, TakeAllEdge, TakeEdge, TakeWhereEdge, TokenId, TransitionId,
};

// ----- heroes, levels and worlds --------------------------------------------------

#[derive(Debug, Clone, PartialEq, Routed)]
struct Hero {
    name: String,
    class: &'static str,
    hp: i64,
}

fn hero(name: &str, class: &'static str) -> Hero {
    Hero {
        name: name.to_owned(),
        class,
        hp: 10,
    }
}

#[derive(Debug, Clone, PartialEq, Routed)]
struct Note(String);

#[derive(Debug)]
struct Broken;

impl std::fmt::Display for Broken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("broken")
    }
}

impl std::error::Error for Broken {}

fn heroes_keys(b: &mut NetBuilder, place: PlaceRef<Hero>) {
    b.keys(place, ["name", "class", "hp"], |h: &Hero| {
        [h.name.as_str().into(), h.class.into(), h.hp.into()]
    });
}

/// A level, as its definition builds it: `Lobby` and `Exit Hall` are ports,
/// keyed by name, class and hit points, and `Arena` is the level's own. With
/// `walk`, `Walk` takes the first hero in the lobby into the arena, and
/// `Out` takes the first in the arena to the exit hall.
struct Level {
    net: Net,
    lobby: PlaceRef<Hero>,
    arena: PlaceRef<Hero>,
    hall: PlaceRef<Hero>,
}

fn level(walk: bool, lobby_seed: &[Hero]) -> Level {
    let mut b = NetBuilder::new();
    let lobby = b.place::<Hero>("Lobby");
    let arena = b.place::<Hero>("Arena");
    let hall = b.place::<Hero>("Exit Hall");
    heroes_keys(&mut b, lobby);
    heroes_keys(&mut b, hall);
    b.port(lobby);
    b.port(hall);
    b.seed(lobby, lobby_seed.iter().cloned());
    if walk {
        b.transition("Walk", (TakeEdge(lobby),), (arena,), |h: Hero| (h,));
        b.transition("Out", (TakeEdge(arena),), (hall,), |h: Hero| (h,));
    }
    Level {
        net: b.build().expect("a level"),
        lobby,
        arena,
        hall,
    }
}

/// A world holding one level as `Level 3`, with a `Door` and a `Gone` of
/// its own and its handles to the level's two ports, still being built.
struct World {
    b: NetBuilder,
    level: SubnetRef,
    lobby: PlaceRef<Hero>,
    hall: PlaceRef<Hero>,
    door: PlaceRef<Hero>,
    gone: PlaceRef<Hero>,
    inner: (PlaceRef<Hero>, PlaceRef<Hero>, PlaceRef<Hero>),
}

fn world(walk: bool, lobby_seed: &[Hero]) -> World {
    let level = level(walk, lobby_seed);
    let inner = (level.lobby, level.arena, level.hall);
    let mut b = NetBuilder::new();
    let door = b.place::<Hero>("Door");
    let level_3 = b.subnet_place("Level 3", level.net);
    let lobby = b.port_of::<Hero>(level_3, "Lobby");
    let hall = b.port_of::<Hero>(level_3, "Exit Hall");
    let gone = b.place::<Hero>("Gone");
    World {
        b,
        level: level_3,
        lobby,
        hall,
        door,
        gone,
        inner,
    }
}

/// The names of the heroes in a place, oldest first.
fn names(net: &Net, place: PlaceId) -> Vec<String> {
    net.tokens(place)
        .map(|(_, value)| value.downcast_ref::<Hero>().expect("a hero").name.clone())
        .collect()
}

/// The ids of the tokens in a place, oldest first.
fn ids(net: &Net, place: PlaceId) -> Vec<TokenId> {
    net.tokens(place).map(|(id, _)| id).collect()
}

fn subnet(net: &Net, level: SubnetRef) -> &Net {
    net.subnet(level.id()).expect("a subnet place")
}

/// A port reads the same from both nets: the same tokens, the same ids, in
/// the same order.
fn same_from_both(net: &Net, window: PlaceRef<Hero>, level: SubnetRef, port: PlaceRef<Hero>) {
    let inside = subnet(net, level);
    assert_eq!(ids(net, window.id()), ids(inside, port.id()), "one queue");
    assert_eq!(net.count(window.id()), inside.count(port.id()));
    assert_eq!(names(net, window.id()), names(inside, port.id()));
}

fn fire(net: &mut Net, name: &str) -> Result<(), NetError> {
    let transition = net.transition_named(name).expect("a transition");
    net.fire(transition)
}

// ----- every input edge kind on a port --------------------------------------------

#[test]
fn a_take_edge_takes_the_front_of_a_port() {
    let mut w = world(false, &[hero("Ada", "Knight"), hero("Bo", "Mage")]);
    let leave =
        w.b.transition("Leave", (TakeEdge(w.lobby),), (w.gone,), |h: Hero| (h,))
            .id();
    let mut net = w.b.build().expect("a world");
    let ada = ids(&net, w.lobby.id())[0];

    fire(&mut net, "Leave").expect("Ada leaves");
    assert_eq!(names(&net, w.gone.id()), ["Ada"]);
    assert_eq!(names(&net, w.lobby.id()), ["Bo"]);
    same_from_both(&net, w.lobby, w.level, w.inner.0);
    let gone = ids(&net, w.gone.id())[0];
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Fired {
            transition: leave,
            consumed: vec![(w.lobby.id(), ada)],
            produced: vec![(w.gone.id(), gone)],
        }],
        "told once, by the outer net, under its own place for the port"
    );
}

#[test]
fn take_all_and_drain_take_every_token_of_a_port() {
    let mut w = world(false, &[hero("Ada", "Knight"), hero("Bo", "Mage")]);
    w.b.transition(
        "Evacuate",
        (TakeAllEdge(w.lobby), DrainEdge(w.hall)),
        (w.gone,),
        |mut lobby: Vec<Hero>, hall: Vec<Hero>| {
            lobby.extend(hall);
            (lobby,)
        },
    );
    let mut net = w.b.build().expect("a world");
    assert!(
        net.is_fireable(net.transition_named("Evacuate").unwrap()),
        "an empty port holds a drain back no more than an empty place"
    );
    fire(&mut net, "Evacuate").expect("everyone out");
    assert_eq!(names(&net, w.gone.id()), ["Ada", "Bo"]);
    assert_eq!(net.count(w.lobby.id()), 0);
    same_from_both(&net, w.lobby, w.level, w.inner.0);
}

#[test]
fn a_keyed_take_finds_its_token_in_a_port() {
    let mut w = world(
        false,
        &[
            hero("Ada", "Knight"),
            hero("Bo", "Mage"),
            hero("Cy", "Mage"),
        ],
    );
    let mages = Filter::new().key_in("class", ["Mage"]);
    w.b.transition(
        "Pick a Mage",
        (TakeWhereEdge(w.lobby, mages.clone()),),
        (w.gone,),
        |h: Hero| (h,),
    );
    let mut net = w.b.build().expect("a world");
    let bo = ids(&net, w.lobby.id())[1];
    assert_eq!(net.first_match(w.lobby.id(), &mages), Some(bo));

    fire(&mut net, "Pick a Mage").expect("Bo, from between the others");
    assert_eq!(names(&net, w.gone.id()), ["Bo"]);
    assert_eq!(names(&net, w.lobby.id()), ["Ada", "Cy"]);
    fire(&mut net, "Pick a Mage").expect("Cy");
    let pick = net.transition_named("Pick a Mage").unwrap();
    assert!(!net.is_fireable(pick), "no mage left");
    same_from_both(&net, w.lobby, w.level, w.inner.0);
}

#[test]
fn snapshot_and_survey_read_a_port_and_leave_it_alone() {
    let mut w = world(false, &[hero("Ada", "Knight"), hero("Bo", "Mage")]);
    let notes = w.b.place::<Note>("Notes");
    w.b.transition(
        "Look",
        (SnapshotEdge(w.lobby), SurveyEdge(w.hall)),
        (notes,),
        |front: Hero, hall: Vec<Hero>| (Note(format!("{} first, {} out", front.name, hall.len())),),
    );
    w.b.transition(
        "Count",
        (SurveyEdge(w.lobby),),
        (notes,),
        |all: Vec<Hero>| (Note(format!("{} in", all.len())),),
    );
    let mut net = w.b.build().expect("a world");
    let before = ids(&net, w.lobby.id());
    fire(&mut net, "Look").expect("a look");
    fire(&mut net, "Count").expect("a count");
    let seen: Vec<&Note> = net
        .tokens(notes.id())
        .map(|(_, v)| v.downcast_ref::<Note>().unwrap())
        .collect();
    assert_eq!(
        seen,
        [&Note("Ada first, 0 out".into()), &Note("2 in".into())]
    );
    assert_eq!(ids(&net, w.lobby.id()), before, "nothing moved");
    same_from_both(&net, w.lobby, w.level, w.inner.0);
}

#[test]
fn mutating_edges_change_a_token_where_it_lies_in_the_port() {
    let mut w = world(false, &[hero("Ada", "Knight"), hero("Bo", "Mage")]);
    let heal =
        w.b.transition("Heal", (MutateEdge(w.lobby),), (), |h: &mut Hero| {
            h.hp += 5;
        })
        .id();
    w.b.transition(
        "Hurt a Mage",
        (MutateWhereEdge(
            w.lobby,
            Filter::new().key_in("class", ["Mage"]),
        ),),
        (),
        |h: &mut Hero| {
            h.hp -= 3;
        },
    );
    let mut net = w.b.build().expect("a world");
    let (ada, bo) = (ids(&net, w.lobby.id())[0], ids(&net, w.lobby.id())[1]);
    net.drain_events().for_each(drop);

    fire(&mut net, "Heal").expect("Ada healed");
    fire(&mut net, "Hurt a Mage").expect("Bo hurt");
    assert_eq!(net.token::<Hero>(w.lobby.id(), ada).map(|h| h.hp), Some(15));
    assert_eq!(net.token::<Hero>(w.lobby.id(), bo).map(|h| h.hp), Some(7));
    assert_eq!(ids(&net, w.lobby.id()), [ada, bo], "same ids, same order");
    // The subnet's index followed the change, so a query from either side
    // finds the token by its new key.
    let fifteen = Filter::new().key_in("hp", [15]);
    assert_eq!(net.query_ids(&fifteen).unwrap(), [(w.lobby.id(), ada)]);
    assert_eq!(
        subnet(&net, w.level).query_ids(&fifteen).unwrap(),
        [(w.inner.0.id(), ada)]
    );
    let events: Vec<Event> = net.drain_events().collect();
    assert!(matches!(
        &events[..2],
        [Event::Mutated { place, token }, Event::Fired { transition, .. }]
            if *place == w.lobby.id() && *token == ada && *transition == heal
    ));
}

#[test]
fn one_body_may_hold_tokens_of_two_ports_and_a_place_of_its_own_at_once() {
    let mut w = world(false, &[hero("Ada", "Knight")]);
    w.b.seed(w.door, [hero("Bo", "Mage")]);
    w.b.seed(w.hall, [hero("Cy", "Rogue")]);
    let pair = w.b.transition(
        "Swap Classes",
        (MutateEdge(w.lobby), MutateEdge(w.door), MutateEdge(w.hall)),
        (),
        |lobby: &mut Hero, door: &mut Hero, hall: &mut Hero| {
            std::mem::swap(&mut lobby.class, &mut door.class);
            hall.hp = 1;
        },
    );
    let swap = pair.id();
    let mut net = w.b.build().expect("a world");
    let (ada, cy) = (ids(&net, w.lobby.id())[0], ids(&net, w.hall.id())[0]);
    net.drain_events().for_each(drop);

    net.fire(swap).expect("three tokens lent at once");
    assert_eq!(
        net.token::<Hero>(w.lobby.id(), ada).map(|h| h.class),
        Some("Mage")
    );
    assert_eq!(names(&net, w.door.id()), ["Bo"]);
    assert_eq!(net.token::<Hero>(w.hall.id(), cy).map(|h| h.hp), Some(1));
    let mages = Filter::new().key_in("class", ["Mage"]);
    assert_eq!(net.query_ids(&mages).unwrap(), [(w.lobby.id(), ada)]);
    let events: Vec<Event> = net.drain_events().collect();
    assert!(
        matches!(
            &events[..],
            [
                Event::Mutated { place: a, .. },
                Event::Mutated { place: b, .. },
                Event::Mutated { place: c, .. },
                Event::Fired { .. },
            ] if [*a, *b, *c] == [w.lobby.id(), w.door.id(), w.hall.id()]
        ),
        "{events:?}"
    );
}

// ----- every way of choosing an output onto a port ------------------------------

#[test]
fn every_way_of_choosing_an_output_can_choose_a_port() {
    let mut w = world(false, &[]);
    let notes = w.b.place::<Note>("Notes");
    w.b.seed(
        w.door,
        [
            hero("Ada", "Knight"),
            hero("Bo", "Mage"),
            hero("Cy", "Rogue"),
        ],
    );
    w.b.transition("By Position", (TakeEdge(w.door),), (w.lobby,), |h: Hero| {
        (h,)
    });
    w.b.transition(
        "By Type",
        (TakeEdge(w.door),),
        (w.lobby, notes),
        |h: Hero| -> Either<Hero, Note> {
            if h.class == "Mage" {
                Either::Left(h)
            } else {
                Either::Right(Note(h.name))
            }
        },
    );
    let (lobby, hall) = (w.lobby, w.hall);
    w.b.transition(
        "By Distribution",
        (TakeEdge(w.door),),
        (w.lobby, w.hall),
        move |h: Hero| {
            if h.class == "Rogue" {
                Distribution::new().to(hall, h)
            } else {
                Distribution::new().to(lobby, h)
            }
        },
    );
    let mut net = w.b.build().expect("a world");

    fire(&mut net, "By Position").expect("Ada in");
    fire(&mut net, "By Type").expect("Bo in, by type");
    fire(&mut net, "By Distribution").expect("Cy to the hall");
    assert_eq!(names(&net, w.lobby.id()), ["Ada", "Bo"]);
    assert_eq!(names(&net, w.hall.id()), ["Cy"]);
    assert_eq!(net.count(notes.id()), 0);
    same_from_both(&net, w.lobby, w.level, w.inner.0);
    same_from_both(&net, w.hall, w.level, w.inner.2);
}

// ----- ids and keys across the border ---------------------------------------------

#[test]
fn a_token_keeps_one_id_across_the_border_both_ways() {
    let mut w = world(true, &[]);
    w.b.seed(w.door, [hero("Ada", "Knight")]);
    let enter =
        w.b.transition("Enter", (TakeEdge(w.door),), (w.lobby,), |h: Hero| (h,))
            .id();
    let leave =
        w.b.transition("Leave", (TakeEdge(w.hall),), (w.gone,), |h: Hero| (h,))
            .id();
    let mut net = w.b.build().expect("a world");
    let level = w.level.id();
    let nested_id = subnet(&net, w.level).id();
    let walk = subnet(&net, w.level).transition_named("Walk").unwrap();
    let out = subnet(&net, w.level).transition_named("Out").unwrap();

    while net.step_deep().expect("nothing fails").is_some() {}
    assert_eq!(names(&net, w.gone.id()), ["Ada"]);

    let events: Vec<Event> = net.drain_events().collect();
    let [
        Event::Fired {
            transition: t0,
            produced: into_lobby,
            ..
        },
        Event::Subnet {
            place: p1,
            nested_id: n1,
            event: e1,
        },
        Event::Subnet {
            place: p2,
            nested_id: n2,
            event: e2,
        },
        Event::Fired {
            transition: t3,
            consumed: from_hall,
            ..
        },
    ] = &events[..]
    else {
        panic!("Enter, Walk and Out inside, Leave: {events:?}");
    };
    assert_eq!((*t0, *t3), (enter, leave));
    assert_eq!((*p1, *p2, *n1, *n2), (level, level, nested_id, nested_id));
    let Event::Fired {
        transition: t1,
        consumed: from_lobby,
        ..
    } = &**e1
    else {
        panic!("Walk: {e1:?}");
    };
    let Event::Fired {
        transition: t2,
        produced: into_hall,
        ..
    } = &**e2
    else {
        panic!("Out: {e2:?}");
    };
    assert_eq!((*t1, *t2), (walk, out));
    // Into the port from outside, out of it inside: one id.
    assert_eq!(into_lobby, &[(w.lobby.id(), from_lobby[0].1)]);
    assert_eq!(from_lobby, &[(w.inner.0.id(), into_lobby[0].1)]);
    // Into the port inside, out of it from outside: one id.
    assert_eq!(into_hall, &[(w.inner.2.id(), from_hall[0].1)]);
    assert_eq!(from_hall, &[(w.hall.id(), into_hall[0].1)]);
}

#[test]
fn ids_in_a_port_are_the_subnets_and_stay_in_increasing_order() {
    let mut w = world(false, &[hero("Ada", "Knight")]);
    w.b.seed(w.door, [hero("Bo", "Mage"), hero("Cy", "Rogue")]);
    w.b.transition("Enter", (TakeEdge(w.door),), (w.lobby,), |h: Hero| (h,));
    // Open, so that the program may put a hero at the window.
    w.b.open();
    let mut net = w.b.build().expect("a world");
    let inner_lobby = w.inner.0;
    // The subnet mints, wherever the token comes from: the outer net, the
    // program through the outer net, the program inside the subnet.
    fire(&mut net, "Enter").expect("Bo in");
    net.put(w.lobby, hero("Di", "Bard"));
    net.subnet_scope(w.level.id(), |level| {
        level.put(inner_lobby, hero("Ed", "Monk"))
    })
    .expect("the level is not held");
    fire(&mut net, "Enter").expect("Cy in");
    let lobby = ids(&net, w.lobby.id());
    assert_eq!(names(&net, w.lobby.id()), ["Ada", "Bo", "Di", "Ed", "Cy"]);
    assert!(lobby.is_sorted(), "increasing: {lobby:?}");
    let mut all = lobby.clone();
    all.dedup();
    assert_eq!(all.len(), 5, "one set of ids");
    same_from_both(&net, w.lobby, w.level, inner_lobby);
}

#[test]
fn keyed_edges_and_queries_see_across_the_border_both_ways() {
    // The level picks mages out of its lobby, whoever put them there.
    let mut b = NetBuilder::new();
    let lobby = b.place::<Hero>("Lobby");
    let arena = b.place::<Hero>("Arena");
    let hall = b.place::<Hero>("Exit Hall");
    heroes_keys(&mut b, lobby);
    heroes_keys(&mut b, hall);
    b.port(lobby);
    b.port(hall);
    b.transition(
        "Train a Mage",
        (TakeWhereEdge(
            lobby,
            Filter::new().key_in("class", ["Mage"]),
        ),),
        (hall,),
        |mut h: Hero| {
            h.hp = 99;
            (h,)
        },
    );
    let _ = arena;
    let level = b.build().expect("a level");

    // The world picks trained heroes out of the hall.
    let mut b = NetBuilder::new();
    let door = b.place::<Hero>("Door");
    let level_3 = b.subnet_place("Level 3", level);
    let w_lobby = b.port_of::<Hero>(level_3, "Lobby");
    let w_hall = b.port_of::<Hero>(level_3, "Exit Hall");
    let gone = b.place::<Hero>("Gone");
    b.seed(door, [hero("Ada", "Knight"), hero("Bo", "Mage")]);
    b.transition("Enter", (TakeAllEdge(door),), (w_lobby,), |h: Vec<Hero>| {
        (h,)
    });
    b.transition(
        "Take the Trained",
        (TakeWhereEdge(w_hall, Filter::new().key_in("hp", [99])),),
        (gone,),
        |h: Hero| (h,),
    );
    let mut net = b.build().expect("a world");

    while net.step_deep().expect("nothing fails").is_some() {}
    assert_eq!(
        names(&net, gone.id()),
        ["Bo"],
        "trained inside, taken outside"
    );
    assert_eq!(names(&net, w_lobby.id()), ["Ada"]);

    // Queries from out here read the port through the subnet's index.
    let knights = Filter::new().key_in("class", ["Knight"]);
    let ada = ids(&net, w_lobby.id())[0];
    assert_eq!(net.query_ids(&knights).unwrap(), [(w_lobby.id(), ada)]);
    let found = net.query(&knights).unwrap();
    assert_eq!(found[0].keys.get("name"), Some(&KeyValue::from("Ada")));
    assert!(net.matches(&knights, w_lobby.id(), ada));
    assert_eq!(
        net.keys(w_lobby.id(), ada).unwrap().get("hp"),
        Some(&KeyValue::Int(10))
    );
    assert_eq!(
        net.distinct_values("class", None),
        [KeyValue::from("Knight")],
        "the lobby's Ada; Bo has gone, and Gone declares no keys"
    );
    assert_eq!(
        net.query_ids(&Filter::new().in_places([w_lobby.id()]))
            .unwrap(),
        [(w_lobby.id(), ada)],
        "a filter without clauses reads the port too"
    );
    // A filter by name prefix covers a port by its own name.
    assert_eq!(
        net.query_ids(&Filter::new().with_prefix("Lob"))
            .unwrap()
            .len(),
        1
    );
}

// ----- failure --------------------------------------------------------------------

#[test]
fn a_failed_outer_firing_puts_back_what_it_took_from_two_ports() {
    let mut w = world(false, &[hero("Ada", "Knight"), hero("Bo", "Mage")]);
    w.b.seed(w.hall, [hero("Cy", "Rogue"), hero("Di", "Bard")]);
    w.b.transition(
        "Ambush",
        (TakeEdge(w.lobby), TakeAllEdge(w.hall)),
        (w.gone,),
        |_: Hero, _: Vec<Hero>| -> Result<(Hero,), Broken> { Err(Broken) },
    )
    .restore_tokens_on_failure();
    let mut net = w.b.build().expect("a world");
    let (lobby, hall) = (ids(&net, w.lobby.id()), ids(&net, w.hall.id()));
    net.drain_events().for_each(drop);

    let failed = fire(&mut net, "Ambush").expect_err("the body fails");
    assert!(matches!(failed, NetError::FiringFailed(ref f) if f.restored));
    assert_eq!(
        ids(&net, w.lobby.id()),
        lobby,
        "in the place and order it came from"
    );
    assert_eq!(ids(&net, w.hall.id()), hall);
    same_from_both(&net, w.lobby, w.level, w.inner.0);
    same_from_both(&net, w.hall, w.level, w.inner.2);
    let events: Vec<Event> = net.drain_events().collect();
    assert!(
        matches!(
            &events[..],
            [Event::Failed { consumed, restored: true, .. }]
                if consumed[0] == (w.lobby.id(), lobby[0]) && consumed[1..] == [(w.hall.id(), hall[0]), (w.hall.id(), hall[1])]
        ),
        "{events:?}"
    );
    // The subnet's index has the restored tokens back.
    assert_eq!(
        subnet(&net, w.level)
            .query_ids(&Filter::new().key_in("name", ["Ada"]))
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn a_failed_outer_firing_puts_nothing_into_a_port() {
    let mut w = world(false, &[hero("Ada", "Knight")]);
    w.b.seed(w.door, [hero("Bo", "Mage")]);
    w.b.transition(
        "Stumble In",
        (TakeEdge(w.door),),
        (w.lobby,),
        |_: Hero| -> Result<(Hero,), Broken> { Err(Broken) },
    );
    let mut net = w.b.build().expect("a world");
    let before = ids(&net, w.lobby.id());
    fire(&mut net, "Stumble In").expect_err("the body fails");
    assert_eq!(ids(&net, w.lobby.id()), before, "nothing delivered");
    assert_eq!(net.count(w.door.id()), 0, "and Bo is lost, as by default");
    same_from_both(&net, w.lobby, w.level, w.inner.0);
    assert!(
        !net.drain_events()
            .any(|e| matches!(e, Event::Subnet { .. } | Event::Added { .. })),
        "the subnet was not touched"
    );
}

// ----- the staged path and the in-flight rule ------------------------------------

#[test]
fn a_firing_in_flight_on_a_port_holds_its_subnet_still() {
    let mut w = world(true, &[hero("Ada", "Knight")]);
    w.b.seed(w.door, [hero("Bo", "Mage")]);
    let enter =
        w.b.transition("Enter", (TakeEdge(w.door),), (w.lobby,), |h: Hero| (h,))
            .id();
    let mut net = w.b.build().expect("a world");

    let firing = net.launch(enter).expect("Enter is fireable");
    assert!(firing.is_pending());
    // The level has work (Ada in its lobby), and the rule is inner first,
    // but Enter is airborne with a token for the lobby: nothing fires.
    assert_eq!(net.step_deep_staged().expect("nothing fails"), None);
    assert_eq!(net.step_deep().expect("nothing fails"), None);
    assert!(!net.is_quiescent_deep(), "something is in flight");
    let refused = net
        .subnet_scope(w.level.id(), |_| ())
        .expect_err("the level is held still");
    assert!(
        matches!(refused, NetError::InFlight { transition, .. } if transition == enter),
        "{refused}"
    );
    // Both views of the port stay right meanwhile.
    assert_eq!(net.count(w.lobby.id()), 1);
    same_from_both(&net, w.lobby, w.level, w.inner.0);

    net.distribute(firing.run()).expect("it lands");
    assert_eq!(names(&net, w.lobby.id()), ["Ada", "Bo"]);
    assert!(matches!(
        net.step_deep_staged(),
        Ok(Some(StagedStep::Inside(DeepStep::Subnet { .. })))
    ));
    net.subnet_scope(w.level.id(), |_| ())
        .expect("free again once it has landed");
}

#[test]
fn a_firing_in_flight_holds_only_the_subnet_it_reaches() {
    let first = level(true, &[hero("Ada", "Knight")]);
    let second = level(true, &[hero("Bo", "Mage")]);
    let mut b = NetBuilder::new();
    let door = b.place::<Hero>("Door");
    let one = b.subnet_place("Level 1", first.net);
    let two = b.subnet_place("Level 2", second.net);
    let lobby_one = b.port_of::<Hero>(one, "Lobby");
    b.seed(door, [hero("Cy", "Rogue")]);
    let enter = b
        .transition("Enter", (TakeEdge(door),), (lobby_one,), |h: Hero| (h,))
        .id();
    let mut net = b.build().expect("two levels");
    let two_id = net.subnet(two.id()).unwrap().id();

    let firing = net.launch(enter).expect("fireable");
    let step = net
        .step_deep()
        .expect("nothing fails")
        .expect("level 2 goes");
    assert!(
        matches!(step, DeepStep::Subnet { place, nested_id, .. } if place == two.id() && nested_id == two_id),
        "{step:?}"
    );
    net.distribute(firing.run()).expect("it lands");
}

#[test]
fn one_firing_on_ports_of_two_subnets_holds_both_still() {
    let first = level(true, &[hero("Ada", "Knight")]);
    let second = level(true, &[hero("Bo", "Mage")]);
    let mut b = NetBuilder::new();
    let one = b.subnet_place("Level 1", first.net);
    let two = b.subnet_place("Level 2", second.net);
    let hall_one = b.port_of::<Hero>(one, "Exit Hall");
    let lobby_two = b.port_of::<Hero>(two, "Lobby");
    let transfer = b
        .transition(
            "Transfer",
            (TakeEdge(hall_one),),
            (lobby_two,),
            |h: Hero| (h,),
        )
        .id();
    // Cy starts in level 1's exit hall, a seed of the world's and so inside
    // it: the closed world shields its levels from the program.
    b.seed(hall_one, [hero("Cy", "Rogue")]);
    let mut net = b.build().expect("two levels");
    net.subnet_scope(one.id(), |_| ()).expect("free");

    let firing = net.launch(transfer).expect("Cy goes over");
    assert_eq!(
        net.step_deep().expect("nothing fails"),
        None,
        "both levels held"
    );
    for level in [one, two] {
        assert!(matches!(
            net.subnet_scope(level.id(), |_| ()),
            Err(NetError::InFlight { transition, .. }) if transition == transfer
        ));
    }
    net.distribute(firing.run()).expect("it lands");
    assert_eq!(names(&net, lobby_two.id()), ["Bo", "Cy"]);
    while net.step_deep().expect("nothing fails").is_some() {}
    assert_eq!(
        net.count(hall_one.id()),
        0,
        "Ada walked out of level 1, and over"
    );
    assert_eq!(
        names(subnet(&net, two), second.hall.id()),
        ["Bo", "Cy", "Ada"]
    );
}

#[test]
#[should_panic(expected = "put another net in the place of the subnet")]
fn a_subnet_place_keeps_the_subnet_it_was_built_with() {
    let w = world(false, &[]);
    let mut net = w.b.build().expect("a world");
    let _ = net.subnet_scope(w.level.id(), |level| {
        *level = NetBuilder::new().build().expect("another net");
    });
}

#[test]
fn outer_transitions_read_a_port_right_while_another_is_in_flight_on_it() {
    let mut w = world(false, &[hero("Ada", "Knight"), hero("Bo", "Mage")]);
    let first =
        w.b.transition("First Out", (TakeEdge(w.lobby),), (w.gone,), |h: Hero| (h,))
            .id();
    let second =
        w.b.transition(
            "Second Out",
            (TakeEdge(w.lobby),),
            (w.gone,),
            |h: Hero| (h,),
        )
        .id();
    let mut net = w.b.build().expect("a world");

    let one = net.launch(first).expect("Ada goes first");
    assert_eq!(net.count(w.lobby.id()), 1, "taken at launch");
    same_from_both(&net, w.lobby, w.level, w.inner.0);
    assert!(net.is_fireable(second), "Bo is still there for the other");
    let two = net.launch(second).expect("Bo goes too");
    assert!(!net.is_fireable(second));
    assert_eq!(net.count(w.lobby.id()), 0);
    net.distribute(two.run()).expect("Bo lands");
    net.distribute(one.run()).expect("Ada lands");
    assert_eq!(names(&net, w.gone.id()), ["Bo", "Ada"]);
}

#[test]
fn two_restoring_firings_in_flight_on_one_port_both_put_back() {
    let mut w = world(false, &[hero("Ada", "Knight"), hero("Bo", "Mage")]);
    let failing = |_: Hero| -> Result<(Hero,), Broken> { Err(Broken) };
    let first =
        w.b.transition("First", (TakeEdge(w.lobby),), (w.gone,), failing)
            .restore_tokens_on_failure()
            .id();
    let second =
        w.b.transition("Second", (TakeEdge(w.lobby),), (w.gone,), failing)
            .restore_tokens_on_failure()
            .id();
    let mut net = w.b.build().expect("a world");
    let before = ids(&net, w.lobby.id());

    let one = net.launch(first).expect("Ada");
    let two = net.launch(second).expect("Bo");
    net.distribute(one.run()).expect_err("fails, Ada back");
    net.distribute(two.run()).expect_err("fails, Bo back");
    let mut after = ids(&net, w.lobby.id());
    after.sort();
    assert_eq!(after, before, "both back in the port");
    same_from_both(&net, w.lobby, w.level, w.inner.0);
    // Back in front out of order, so the subnet's index reads the queue.
    let found: Vec<_> = net
        .query_ids(&Filter::new().key_in("class", ["Knight", "Mage"]))
        .unwrap()
        .into_iter()
        .map(|(_, token)| token)
        .collect();
    assert_eq!(found, ids(&net, w.lobby.id()), "queue order");
}

// ----- external input -------------------------------------------------------------

#[test]
fn external_input_reaches_a_port_through_the_outer_net() {
    let mut w = world(false, &[hero("Ada", "Knight")]);
    // Open, so that the program may write at the window: a window is never a
    // port of the net that holds it.
    w.b.open();
    let mut net = w.b.build().expect("a world");
    let lobby = w.lobby.id();

    let bo = net.put(w.lobby, hero("Bo", "Mage"));
    let more = net.extend(w.lobby, [hero("Cy", "Rogue"), hero("Di", "Bard")]);
    net.replace(w.lobby, bo, hero("Bo", "Archmage"))
        .expect("Bo is there");
    net.remove(lobby, more[0]).expect("Cy is there");
    net.apply(Command::put(w.lobby, hero("Ed", "Monk")))
        .expect("an erased put");
    assert_eq!(names(&net, lobby), ["Ada", "Bo", "Di", "Ed"]);
    assert_eq!(
        net.token::<Hero>(lobby, bo).map(|h| h.class),
        Some("Archmage")
    );
    same_from_both(&net, w.lobby, w.level, w.inner.0);

    let events: Vec<Event> = net.drain_events().collect();
    assert!(
        events.iter().all(|event| matches!(
            event,
            Event::Added { place, .. } | Event::Mutated { place, .. } | Event::Removed { place, .. }
                if *place == lobby
        )),
        "each change once, in the outer log, under its place for the port: {events:?}"
    );
    assert_eq!(events.len(), 6);

    net.set_tokens(w.lobby, [hero("Fy", "Druid")]);
    assert_eq!(names(&net, lobby), ["Fy"]);
    same_from_both(&net, w.lobby, w.level, w.inner.0);
    let wrong: Vec<Box<dyn std::any::Any + Send + Sync>> = vec![Box::new(7_u32)];
    assert!(
        net.apply(Command::Extend {
            place: lobby,
            tokens: wrong
        })
        .is_err(),
        "a port's type is checked as any place's"
    );
    // A subnet place holds no tokens, and refuses any offered to it.
    let offered: Vec<Box<dyn std::any::Any + Send + Sync>> = vec![Box::new(hero("Gu", "Bard"))];
    let refused = net
        .apply(Command::Extend {
            place: w.level.id(),
            tokens: offered,
        })
        .expect_err("no tokens of its own");
    assert!(refused.to_string().contains("<subnet>"), "{refused}");
}

// ----- the events -----------------------------------------------------------------

#[test]
fn each_change_is_told_once_by_the_net_that_made_it() {
    let mut w = world(true, &[]);
    w.b.seed(w.door, [hero("Ada", "Knight")]);
    let enter =
        w.b.transition("Enter", (TakeEdge(w.door),), (w.lobby,), |h: Hero| (h,))
            .id();
    let mut net = w.b.build().expect("a world");
    let level = w.level.id();
    let walk = subnet(&net, w.level).transition_named("Walk").unwrap();

    assert!(
        matches!(net.step_deep(), Ok(Some(DeepStep::Here { transition })) if transition == enter)
    );
    let ada = ids(&net, w.lobby.id())[0];
    assert_eq!(net.state_counter(), 1);
    assert_eq!(
        subnet(&net, w.level).state_counter(),
        0,
        "the subnet told nothing"
    );
    let step = net.step_deep().expect("nothing fails").expect("Walk");
    assert!(matches!(
        &step,
        DeepStep::Subnet { place, step, .. }
            if *place == level && **step == DeepStep::Here { transition: walk }
    ));
    let arena = ids(subnet(&net, w.level), w.inner.1.id())[0];

    let events: Vec<Event> = net.drain_events().collect();
    let door_token = TokenId::new(0);
    assert_eq!(
        events,
        [
            Event::Fired {
                transition: enter,
                consumed: vec![(w.door.id(), door_token)],
                produced: vec![(w.lobby.id(), ada)],
            },
            Event::Subnet {
                place: level,
                nested_id: subnet(&net, w.level).id(),
                event: Box::new(Event::Fired {
                    transition: walk,
                    consumed: vec![(w.inner.0.id(), ada)],
                    produced: vec![(w.inner.1.id(), arena)],
                }),
            },
        ]
    );
}

// ----- stepping -------------------------------------------------------------------

/// Where a deep step fired: the innermost nested id, `None` for the net
/// that was stepped.
fn fired_in(step: &DeepStep) -> Option<NetId> {
    match step {
        DeepStep::Here { .. } => None,
        DeepStep::Inside {
            nested_id, step, ..
        }
        | DeepStep::Subnet {
            nested_id, step, ..
        } => Some(fired_in(step).unwrap_or(*nested_id)),
        _ => unreachable!("a deep step this test does not know: {step:?}"),
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

/// An outer net with two jobs of its own (`Inbox -Forward-> Outbox`) and,
/// at `Level`, a level whose lobby holds one hero: two inner firings.
fn guild(stepping: Option<Stepping>) -> (Net, SubnetRef, NetId) {
    let level = level(true, &[hero("Ada", "Knight")]);
    let id = level.net.id();
    let mut b = NetBuilder::new();
    let level_ref = b.subnet_place("Level", level.net);
    let inbox = b.place::<u32>("Inbox");
    let outbox = b.place::<u32>("Outbox");
    b.seed(inbox, [1, 2]);
    b.transition("Forward", (TakeEdge(inbox),), (outbox,), |n: u32| (n,));
    if let Some(stepping) = stepping {
        b.stepping(level_ref, stepping);
    }
    (b.build().expect("a guild"), level_ref, id)
}

#[test]
fn a_subnet_steps_under_each_rule() {
    for (stepping, expected) in [
        (None, ["level", "level", "outer", "outer", "-"]),
        (
            Some(Stepping::InnerFirst),
            ["level", "level", "outer", "outer", "-"],
        ),
        (
            Some(Stepping::TakeTurns),
            ["outer", "level", "outer", "level", "-"],
        ),
        (
            Some(Stepping::OuterFirst),
            ["outer", "outer", "level", "level", "-"],
        ),
        (Some(Stepping::Passive), ["outer", "outer", "-", "-", "-"]),
    ] {
        let (mut net, level, id) = guild(stepping);
        assert_eq!(
            net.stepping(level.id()),
            Some(stepping.unwrap_or_default()),
            "{stepping:?}"
        );
        assert_eq!(
            story(&mut net, 5, &[(id, "level")]),
            expected,
            "{stepping:?}"
        );
        assert!(net.is_quiescent_deep(), "{stepping:?}");
    }

    // A passive subnet is the program's to step.
    let (mut net, level, _) = guild(Some(Stepping::Passive));
    while net.step_deep().unwrap().is_some() {}
    let walked = net
        .subnet_scope(level.id(), |level| level.step().expect("nothing fails"))
        .expect("a subnet place");
    assert!(walked.is_some());
    assert!(
        net.drain_events()
            .any(|e| matches!(e, Event::Subnet { .. }))
    );
}

#[test]
fn set_stepping_changes_a_subnet_places_rule() {
    let (mut net, level, id) = guild(None);
    net.set_stepping(level.id(), Stepping::OuterFirst)
        .expect("a subnet place holds a net");
    assert_eq!(net.stepping(level.id()), Some(Stepping::OuterFirst));
    assert_eq!(
        story(&mut net, 5, &[(id, "level")]),
        ["outer", "outer", "level", "level", "-"]
    );
}

#[test]
fn two_subnets_with_different_rules_take_their_bands() {
    let early = level(true, &[hero("Ada", "Knight")]);
    let late = level(true, &[hero("Bo", "Mage")]);
    let ring = level(true, &[hero("Cy", "Rogue")]);
    let names = [
        (early.net.id(), "early"),
        (late.net.id(), "late"),
        (ring.net.id(), "ring"),
    ];
    let mut b = NetBuilder::new();
    // Declared against the bands' order, as the stepping tests do.
    let late_ref = b.subnet_place("Late", late.net);
    let ring_ref = b.subnet_place("Ring", ring.net);
    let _early = b.subnet_place("Early", early.net);
    let inbox = b.place::<u32>("Inbox");
    let outbox = b.place::<u32>("Outbox");
    b.seed(inbox, [1, 2, 3]);
    b.transition("Forward", (TakeEdge(inbox),), (outbox,), |n: u32| (n,));
    b.stepping(late_ref, Stepping::OuterFirst);
    b.stepping(ring_ref, Stepping::TakeTurns);
    let mut net = b.build().expect("three levels");
    assert_eq!(
        story(&mut net, 10, &names),
        [
            "early", "early", "outer", "ring", "outer", "ring", "outer", "late", "late", "-"
        ]
    );
}

#[test]
fn a_subnet_inside_a_subnet() {
    // A room: whoever is at its door sits down.
    let mut b = NetBuilder::new();
    let den = b.place::<Hero>("Den");
    let seat = b.place::<Hero>("Seat");
    b.port(den);
    let sit = b
        .transition("Sit", (TakeEdge(den),), (seat,), |h: Hero| (h,))
        .id();
    let room = b.build().expect("a room");
    let room_id = room.id();

    // A level holding the room: whoever is in its lobby goes to the room.
    let mut b = NetBuilder::new();
    let lobby = b.place::<Hero>("Lobby");
    b.port(lobby);
    let room_ref = b.subnet_place("Room", room);
    let room_den = b.port_of::<Hero>(room_ref, "Den");
    let go = b
        .transition(
            "Go to the Room",
            (TakeEdge(lobby),),
            (room_den,),
            |h: Hero| (h,),
        )
        .id();
    let level = b.build().expect("a level");
    let level_id = level.id();

    // The world: Ada enters.
    let mut b = NetBuilder::new();
    let door = b.place::<Hero>("Door");
    let level_ref = b.subnet_place("Level 3", level);
    let w_lobby = b.port_of::<Hero>(level_ref, "Lobby");
    b.seed(door, [hero("Ada", "Knight")]);
    let enter = b
        .transition("Enter", (TakeEdge(door),), (w_lobby,), |h: Hero| (h,))
        .id();
    let mut net = b.build().expect("a world");

    let steps: Vec<DeepStep> =
        std::iter::from_fn(|| net.step_deep().expect("nothing fails")).collect();
    assert_eq!(
        steps,
        [
            DeepStep::Here { transition: enter },
            DeepStep::Subnet {
                place: level_ref.id(),
                nested_id: level_id,
                step: Box::new(DeepStep::Here { transition: go }),
            },
            DeepStep::Subnet {
                place: level_ref.id(),
                nested_id: level_id,
                step: Box::new(DeepStep::Subnet {
                    place: room_ref.id(),
                    nested_id: room_id,
                    step: Box::new(DeepStep::Here { transition: sit }),
                }),
            },
        ]
    );
    let events: Vec<Event> = net.drain_events().collect();
    assert!(
        matches!(
            &events[2],
            Event::Subnet { place, event, .. }
                if *place == level_ref.id()
                    && matches!(&**event, Event::Subnet { place, event, .. }
                        if *place == room_ref.id() && matches!(**event, Event::Fired { transition, .. } if transition == sit))
        ),
        "{events:?}"
    );
    let room = net
        .subnet(level_ref.id())
        .and_then(|level| level.subnet(room_ref.id()))
        .expect("two levels down");
    assert_eq!(names(room, seat.id()), ["Ada"]);
    assert!(net.is_quiescent_deep());
}

#[test]
fn the_concurrent_driver_runs_a_world_with_a_subnet() {
    let mut w = world(true, &[]);
    w.b.seed(w.door, (0..5).map(|n| hero(&format!("Hero {n}"), "Knight")));
    w.b.transition("Enter", (TakeEdge(w.door),), (w.lobby,), |h: Hero| {
        std::thread::sleep(std::time::Duration::from_millis(1));
        (h,)
    });
    w.b.transition("Leave", (TakeEdge(w.hall),), (w.gone,), |h: Hero| (h,));
    let mut net = w.b.build().expect("a world");
    let summary = drivers::run(
        &mut net,
        Mode::Concurrent,
        Limits::default(),
        OnFailure::Stop,
    )
    .expect("nothing fails");
    assert!(summary.quiesced);
    assert_eq!(net.count(w.gone.id()), 5);
    assert_eq!(net.count(w.lobby.id()), 0);
    assert_eq!(net.count(w.hall.id()), 0);
    // Every inner firing was made while nothing reaching the level was
    // airborne: between an outer launch and its landing, no subnet event.
    let mut airborne = 0;
    for event in net.drain_events() {
        match event {
            Event::Launched { .. } => airborne += 1,
            Event::Distributed { .. } | Event::Failed { .. } => airborne -= 1,
            Event::Subnet { .. } => {
                // Only `Enter` reaches the lobby; `Leave` reaches the hall,
                // so either holds the level still while it is airborne.
                assert_eq!(
                    airborne, 0,
                    "the level fired with a firing on its port in flight"
                );
            }
            _ => {}
        }
    }
}

// ----- tokens that carry nets in a port -----------------------------------------

#[test]
fn a_port_may_hold_tokens_that_carry_nets() {
    // A worker with one job, as a bare net.
    let worker = || {
        let mut b = NetBuilder::new();
        let todo = b.place::<u32>("Todo");
        let done = b.place::<u32>("Done");
        b.seed(todo, [1]);
        b.transition("Work", (TakeEdge(todo),), (done,), |n: u32| (n,));
        b.build().expect("a worker")
    };
    // A workshop whose bench, a port, holds workers.
    let mut b = NetBuilder::new();
    let bench = b.place_nested::<Net>("Bench");
    b.port(bench);
    let shop = b.build().expect("a workshop");

    let mut b = NetBuilder::new();
    let yard = b.place_nested::<Net>("Yard");
    let shop_ref = b.subnet_place("Workshop", shop);
    let w_bench = b.port_of::<Net>(shop_ref, "Bench");
    b.seed(yard, [worker()]);
    b.transition("Hire", (TakeEdge(yard),), (w_bench,), |n: Net| (n,));
    // The yard's worker waits for the hire: the yard is passive.
    b.stepping(yard, Stepping::Passive);
    let mut net = b.build().expect("a world");
    assert_eq!(
        net.stepping(w_bench.id()),
        None,
        "the port's rule is the workshop's"
    );

    while net.step_deep().expect("nothing fails").is_some() {}
    let (token, _) = net.tokens(w_bench.id()).next().expect("hired");
    let nets = net
        .nested(w_bench.id(), token)
        .expect("a worker, read through the port");
    assert_eq!(nets[0].1.fired_count(), 1, "the workshop stepped it");
    let events: Vec<Event> = net.drain_events().collect();
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::Subnet { event, .. } if matches!(**event, Event::Nested { .. })
        )),
        "the worker's firing, lifted twice: {events:?}"
    );

    // Stepping a port from out here is refused: it holds no nets of the
    // outer net's own to step.
    let mut b = NetBuilder::new();
    let shop_ref = b.subnet_place("Workshop", {
        let mut s = NetBuilder::new();
        let bench = s.place_nested::<Net>("Bench");
        s.port(bench);
        s.build().unwrap()
    });
    let w_bench = b.port_of::<Net>(shop_ref, "Bench");
    b.stepping(w_bench, Stepping::TakeTurns);
    assert!(matches!(
        b.build(),
        Err(BuildError::NotNested { place }) if place == "Bench"
    ));
}

// ----- names, reading, copies -----------------------------------------------------

#[test]
fn a_port_is_named_by_its_subnet_place_and_its_name() {
    let first = level(false, &[]);
    let second = level(false, &[]);
    let mut b = NetBuilder::new();
    let own = b.place::<Hero>("Lobby");
    let one = b.subnet_place("Level 1", first.net);
    let two = b.subnet_place("Level 2", second.net);
    let lobby_one = b.port_of::<Hero>(one, "Lobby");
    let lobby_two = b.port_of::<Hero>(two, "Lobby");
    b.seed(lobby_two, [hero("Ada", "Knight")]);
    let mut net = b
        .build()
        .expect("three places named Lobby, one of them this net's");

    assert_eq!(
        net.place_named("Lobby"),
        Some(own.id()),
        "only its own place"
    );
    assert_ne!(lobby_one, lobby_two);
    let counts: Vec<(&str, usize)> = net.places().map(|(_, p)| (p.name, p.count)).collect();
    assert_eq!(
        counts,
        [
            ("Lobby", 0),
            ("Level 1", 0),
            ("Lobby", 0),
            ("Exit Hall", 0),
            ("Level 2", 0),
            ("Lobby", 1),
            ("Exit Hall", 0),
        ]
    );
    let info = net.places().nth(1).unwrap().1;
    assert_eq!(
        info.stepping,
        Some(Stepping::InnerFirst),
        "a subnet place has a rule"
    );
    assert!(net.subnet(own.id()).is_none());
    assert!(matches!(
        net.subnet_scope(own.id(), |_| ()),
        Err(NetError::NotSubnet { place, .. }) if place == own.id()
    ));
    assert!(matches!(
        net.subnet_scope(PlaceId::new(99), |_| ()),
        Err(NetError::NoSuchPlace { .. })
    ));
}

#[test]
fn port_ref_finds_a_port_by_its_subnet_place_and_its_name() {
    let first = level(false, &[]);
    let second = level(false, &[]);
    let mut b = NetBuilder::new();
    let own = b.place::<Hero>("Lobby");
    let one = b.subnet_place("Level 1", first.net);
    let two = b.subnet_place("Level 2", second.net);
    let lobby_one = b.port_of::<Hero>(one, "Lobby");
    let lobby_two = b.port_of::<Hero>(two, "Lobby");
    let hall_two = b.port_of::<Hero>(two, "Exit Hall");
    let net = b.build().expect("a world");

    assert_eq!(net.port_ref::<Hero>(one.id(), "Lobby"), Some(lobby_one));
    assert_eq!(net.port_ref::<Hero>(two.id(), "Lobby"), Some(lobby_two));
    assert_eq!(net.port_ref::<Hero>(two.id(), "Exit Hall"), Some(hall_two));
    assert_eq!(net.port_ref::<Hero>(two.id(), "Arena"), None, "not a port");
    assert_eq!(
        net.port_ref::<Note>(two.id(), "Lobby"),
        None,
        "the wrong type"
    );
    assert_eq!(
        net.port_ref::<Hero>(own.id(), "Lobby"),
        None,
        "not a subnet place"
    );
    assert_eq!(net.port_ref::<Hero>(PlaceId::new(99), "Lobby"), None);
    assert_eq!(
        net.place_ref::<Hero>("Lobby"),
        Some(own),
        "a name alone finds the net's own place"
    );
}

#[test]
fn place_info_says_what_a_subnet_place_and_a_port_are_from_each_side() {
    let mut w = world(false, &[hero("Ada", "Knight")]);
    w.b.seed(w.door, [hero("Bo", "Mage")]);
    let net = w.b.build().expect("a world");
    let places: Vec<(PlaceId, PlaceInfo<'_>)> = net.places().collect();
    let names: Vec<&str> = places.iter().map(|(_, info)| info.name).collect();
    assert_eq!(names, ["Door", "Level 3", "Lobby", "Exit Hall", "Gone"]);

    let door = places[0].1;
    assert!(!door.subnet && !door.port && door.port_of.is_none());

    let level = places[1].1;
    assert_eq!(places[1].0, w.level.id());
    assert!(level.subnet);
    assert!(!level.port && !level.nested);
    assert_eq!(level.port_of, None);
    assert_eq!(level.type_name, "<subnet>");
    assert_eq!(level.count, 0, "a subnet place holds no tokens of its own");
    assert_eq!(level.stepping, Some(Stepping::InnerFirst));

    // The port seen from outside: the port's own name, type and keys, its
    // count, and the subnet place it belongs to.
    let lobby = places[2].1;
    assert_eq!(places[2].0, w.lobby.id());
    assert_eq!(lobby.port_of, Some(w.level.id()));
    assert!(!lobby.subnet && !lobby.port);
    assert_eq!(lobby.type_name, std::any::type_name::<Hero>());
    assert_eq!(lobby.key_names, ["name", "class", "hp"]);
    assert_eq!(lobby.count, 1);
    assert_eq!(lobby.stepping, None, "a port's rule is its subnet's");
    assert_eq!(places[3].1.port_of, Some(w.level.id()));

    // The port seen from inside: an ordinary place, marked a port.
    let inside: Vec<(&str, bool, Option<PlaceId>)> = subnet(&net, w.level)
        .places()
        .map(|(_, info)| (info.name, info.port, info.port_of))
        .collect();
    assert_eq!(
        inside,
        [
            ("Lobby", true, None),
            ("Arena", false, None),
            ("Exit Hall", true, None)
        ]
    );
}

#[test]
fn the_topology_lists_a_subnet_place_its_ports_and_the_edges_on_them() {
    let mut w = world(false, &[]);
    let enter =
        w.b.transition("Enter", (TakeEdge(w.door),), (w.lobby,), |h: Hero| (h,))
            .id();
    let leave =
        w.b.transition("Leave", (TakeEdge(w.hall),), (w.gone,), |h: Hero| (h,))
            .id();
    let net = w.b.build().expect("a world");
    let topology = net.topology();

    let level = &topology.places[w.level.id().index() as usize];
    assert!(level.subnet && !level.port && level.port_of.is_none());
    assert_eq!(level.type_name, "<subnet>");
    assert_eq!(level.stepping, Some(Stepping::InnerFirst));
    // Its ports, in the subnet's declaration order, each with its own name.
    let ports: Vec<(PlaceId, &str)> = topology
        .places
        .iter()
        .filter(|place| place.port_of == Some(w.level.id()))
        .map(|place| (place.id, place.name.as_str()))
        .collect();
    assert_eq!(ports, [(w.lobby.id(), "Lobby"), (w.hall.id(), "Exit Hall")]);
    assert_eq!(
        topology.places[w.lobby.id().index() as usize].key_names,
        ["name", "class", "hp"]
    );
    // An edge on a port names this net's place for it.
    assert!(topology.edges.contains(&EdgeInfo::Output {
        transition: enter,
        place: w.lobby.id(),
        slot: 0
    }));
    assert!(topology.edges.contains(&EdgeInfo::Input {
        place: w.hall.id(),
        transition: leave,
        kind: EdgeKind::Take,
        filter: None,
        reaches: Vec::new()
    }));
    // In the subnet's own topology the ports are marked, and nothing is a
    // port of anything.
    let inside = subnet(&net, w.level).topology();
    let marked: Vec<(&str, bool)> = inside
        .places
        .iter()
        .map(|place| (place.name.as_str(), place.port))
        .collect();
    assert_eq!(
        marked,
        [("Lobby", true), ("Arena", false), ("Exit Hall", true)]
    );
    assert!(inside.places.iter().all(|place| place.port_of.is_none()));

    #[cfg(feature = "serde")]
    {
        use petritype::serde_json::{json, to_value};
        let places = to_value(&topology).unwrap()["places"].clone();
        assert_eq!(places[1]["subnet"], json!(true));
        assert_eq!(places[2]["port_of"], json!(w.level.id().index()));
        assert_eq!(
            places[0]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["id", "name", "serializable", "type_name"],
            "a place of a net's own is written as it always was"
        );
        let inside = to_value(&inside).unwrap()["places"].clone();
        assert_eq!(inside[0]["port"], json!(true));
        assert!(inside[1].get("port").is_none());
    }
}

#[test]
fn a_snapshot_holds_a_snapshot_of_each_subnet_and_a_port_reads_the_same_from_both() {
    let mut w = world(true, &[hero("Ada", "Knight")]);
    w.b.seed(w.door, [hero("Bo", "Mage")]);
    w.b.seed(w.hall, [hero("Cy", "Rogue")]);
    w.b.transition("Enter", (TakeEdge(w.door),), (w.lobby,), |h: Hero| (h,));
    let mut net = w.b.build().expect("a world");
    let cy = ids(&net, w.hall.id())[0];
    let lobby = ids(&net, w.lobby.id());

    let taken = net.snapshot().expect("every place clones");
    let inside = taken.subnet(w.level.id()).expect("a subnet place");
    assert_eq!(inside.id(), subnet(&net, w.level).id());
    let in_snapshot = |snapshot: &petritype::Snapshot, place: PlaceId| -> Vec<TokenId> {
        snapshot.tokens(place).map(|(id, _)| id).collect()
    };
    assert_eq!(
        in_snapshot(&taken, w.lobby.id()),
        lobby,
        "the live ids, from outside"
    );
    assert_eq!(
        in_snapshot(inside, w.inner.0.id()),
        lobby,
        "and from inside"
    );
    assert_eq!(in_snapshot(&taken, w.hall.id()), [cy]);
    assert_eq!(taken.topology(), net.topology());
    assert_eq!(inside.topology(), subnet(&net, w.level).topology());
    assert_eq!(
        taken
            .query_ids(&Filter::new().key_in("name", ["Ada"]))
            .unwrap(),
        [(w.lobby.id(), lobby[0])]
    );
    assert!(
        taken
            .label(w.hall.id(), cy)
            .expect("a label")
            .contains("Cy")
    );
    assert!(taken.subnet(w.door.id()).is_none());

    // A moment: the net moves on, the snapshot and its clone do not.
    let clone = taken.clone();
    while net.step_deep().expect("nothing fails").is_some() {}
    assert_eq!(net.count(w.lobby.id()), 0);
    for snapshot in [&taken, &clone] {
        assert_eq!(snapshot.count(w.lobby.id()), 1);
        let inside = snapshot.subnet(w.level.id()).unwrap();
        assert_eq!(inside.count(w.inner.0.id()), 1);
        assert_eq!(inside.fired_count(), 0);
    }
}

#[cfg(not(feature = "serde"))]
#[test]
fn without_serde_a_snapshot_refuses_a_subnet_that_holds_nets_as_any_nested_place() {
    let mut b = NetBuilder::new();
    let bench = b.place_nested::<Net>("Bench");
    b.port(bench);
    let shop = b.build().unwrap();
    let mut b = NetBuilder::new();
    let shop_ref = b.subnet_place("Workshop", shop);
    let w_bench = b.port_of::<Net>(shop_ref, "Bench");
    let net = b.build().unwrap();
    assert!(matches!(
        net.snapshot(),
        Err(NetError::NoClonePlace { place, name }) if place == w_bench.id() && name == "Bench"
    ));
}

// ----- where a failure says it happened ---------------------------------------------

/// A room whose one transition, `Sit`, refuses: a `Den` that is a port and
/// holds one hero, and a `Seat`.
fn broken_room() -> (Net, TransitionId) {
    let mut b = NetBuilder::new();
    let den = b.place::<Hero>("Den");
    let seat = b.place::<Hero>("Seat");
    b.port(den);
    b.seed(den, [hero("Ada", "Knight")]);
    let sit = b
        .transition(
            "Sit",
            (TakeEdge(den),),
            (seat,),
            |_: Hero| -> Result<(Hero,), Broken> { Err(Broken) },
        )
        .id();
    (b.build().expect("a room"), sit)
}

/// A net holding `inner` in a subnet place called `name`, and nothing else.
fn holding(name: &str, inner: Net) -> (Net, SubnetRef) {
    let mut b = NetBuilder::new();
    let place = b.subnet_place(name, inner);
    (b.build().expect("a holder"), place)
}

#[test]
fn a_failure_three_subnets_down_names_every_level() {
    let (room, sit) = broken_room();
    let (floor, room_ref) = holding("Room", room);
    let (tower, floor_ref) = holding("Floor", floor);
    let (mut city, tower_ref) = holding("Tower", tower);

    let failed = city
        .step_deep()
        .expect_err("Sit refuses")
        .into_firing()
        .unwrap();
    assert_eq!(failed.transition, sit);
    assert_eq!(
        failed.path,
        [
            PathStep::Subnet {
                place: tower_ref.id()
            },
            PathStep::Subnet {
                place: floor_ref.id()
            },
            PathStep::Subnet {
                place: room_ref.id()
            },
        ]
    );
    assert!(
        failed
            .path
            .iter()
            .all(|step| step.place() == PlaceId::new(0))
    );
    assert_eq!(
        failed.to_string(),
        "transition \"Sit\" (TransitionId(0)) inside PlaceId(0) / PlaceId(0) / PlaceId(0) failed: broken"
    );
    // The `Failed` came up three levels, lifted at each.
    let events: Vec<Event> = city.drain_events().collect();
    assert!(
        matches!(
            &events[..],
            [Event::Subnet { event, .. }]
                if matches!(&**event, Event::Subnet { event, .. }
                    if matches!(&**event, Event::Subnet { event, .. }
                        if matches!(**event, Event::Failed { .. })))
        ),
        "{events:?}"
    );
}

#[test]
fn a_failure_through_a_tokens_net_and_a_subnet_names_both_kinds_of_level() {
    // A token's net that holds the room in a subnet place.
    let (room, _) = broken_room();
    let (house, room_ref) = holding("Room", room);
    let mut b = NetBuilder::new();
    let street = b.place_nested::<Net>("Street");
    b.seed(street, [house]);
    let mut town = b.build().expect("a town");
    let house_token = ids(&town, street.id())[0];
    let failed = town
        .step_deep()
        .expect_err("Sit refuses")
        .into_firing()
        .unwrap();
    assert_eq!(
        failed.path,
        [
            PathStep::Token {
                place: street.id(),
                token: house_token,
                net: String::new()
            },
            PathStep::Subnet {
                place: room_ref.id()
            },
        ]
    );

    // And the other way round: a subnet whose own place holds a net that
    // fails, a worker on a bench that is no port.
    let (worker, work) = {
        let mut b = NetBuilder::new();
        let todo = b.place::<u32>("Todo");
        let done = b.place::<u32>("Done");
        b.seed(todo, [1]);
        let work = b
            .transition(
                "Work",
                (TakeEdge(todo),),
                (done,),
                |_: u32| -> Result<(u32,), Broken> { Err(Broken) },
            )
            .id();
        (b.build().unwrap(), work)
    };
    let mut b = NetBuilder::new();
    let bench = b.place_nested::<Net>("Bench");
    b.seed(bench, [worker]);
    let shop = b.build().unwrap();
    let worker_token = ids(&shop, bench.id())[0];
    let (mut world, shop_ref) = holding("Workshop", shop);
    let failed = world
        .step_deep()
        .expect_err("Work refuses")
        .into_firing()
        .unwrap();
    assert_eq!(failed.transition, work);
    assert_eq!(
        failed.path,
        [
            PathStep::Subnet {
                place: shop_ref.id()
            },
            PathStep::Token {
                place: bench.id(),
                token: worker_token,
                net: String::new()
            },
        ]
    );
    assert!(
        failed
            .to_string()
            .contains("inside PlaceId(0) / PlaceId(0) #")
    );
}

#[test]
fn the_drivers_switch_off_a_failing_transition_inside_a_subnet_and_count_its_firings() {
    let (room, sit) = broken_room();
    let (floor, room_ref) = holding("Room", room);
    let (mut tower, floor_ref) = holding("Floor", floor);
    let summary = drivers::run(
        &mut tower,
        Mode::Sequential,
        Limits::default(),
        OnFailure::Disable,
    )
    .expect("a failure switches its transition off");
    assert!(summary.quiesced);
    assert_eq!(summary.failed_count, 1);
    let room = tower
        .subnet(floor_ref.id())
        .and_then(|floor| floor.subnet(room_ref.id()))
        .expect("two levels down");
    assert!(!room.is_enabled(sit), "switched off where it is");
    assert!(
        tower.drain_events().any(|e| matches!(
            &e,
            Event::Subnet { event, .. }
                if matches!(&**event, Event::Subnet { event, .. }
                    if matches!(**event, Event::Toggled { enabled: false, .. }))
        )),
        "its `Toggled` lifted twice"
    );

    // A subnet's firings count in a run's summary, as a token's net's do.
    for mode in [Mode::Sequential, Mode::Concurrent] {
        let (mut net, _, _) = guild(None);
        let summary =
            drivers::run(&mut net, mode, Limits::default(), OnFailure::Stop).expect("no failure");
        assert_eq!(
            summary.fired_count, 4,
            "two in the level, two out here: {mode:?}"
        );
    }
}

// ----- subnets and nets carried by tokens, inside each other ---------------------

#[test]
fn nets_carried_by_tokens_and_subnets_step_inside_each_other() {
    // A worker, as a bare net, with one job.
    let worker = || {
        let mut b = NetBuilder::new();
        let todo = b.place::<u32>("Todo");
        let done = b.place::<u32>("Done");
        b.seed(todo, [1]);
        b.transition("Work", (TakeEdge(todo),), (done,), |n: u32| (n,));
        b.build().expect("a worker")
    };
    // A shop whose own bench holds a worker, and a house whose room is a
    // subnet holding the shop.
    let mut b = NetBuilder::new();
    let bench = b.place_nested::<Net>("Bench");
    b.seed(bench, [worker()]);
    let shop = b.build().unwrap();
    let (house, shop_ref) = holding("Shop", shop);
    // A street of such houses, as tokens.
    let mut b = NetBuilder::new();
    let street = b.place_nested::<Net>("Street");
    b.seed(street, [house]);
    let mut town = b.build().unwrap();
    let house_token = ids(&town, street.id())[0];

    let step = town
        .step_deep()
        .expect("nothing fails")
        .expect("the worker works");
    let DeepStep::Inside {
        place,
        token,
        step: in_house,
        ..
    } = &step
    else {
        panic!("a firing in the house: {step:?}");
    };
    assert_eq!((*place, *token), (street.id(), house_token));
    assert!(
        matches!(&**in_house, DeepStep::Subnet { place, step, .. }
            if *place == shop_ref.id() && matches!(&**step, DeepStep::Inside { place, .. } if *place == bench.id())),
        "{step:?}"
    );
    assert_eq!(town.step_deep().expect("nothing fails"), None);
    assert!(town.is_quiescent_deep());
    let events: Vec<Event> = town.drain_events().collect();
    assert!(
        matches!(
            &events[..],
            [Event::Nested { event, .. }]
                if matches!(&**event, Event::Subnet { event, .. }
                    if matches!(&**event, Event::Nested { event, .. }
                        if matches!(**event, Event::Fired { .. })))
        ),
        "lifted through every level: {events:?}"
    );
}

impl petritype::style::TokenStyle for Hero {
    fn style(&self) -> petritype::style::Style {
        petritype::style::Style::default()
    }
}

/// What a port is, beyond its tokens, is its subnet's to declare: an option
/// that says so, given the port's handle out here, is refused at build as
/// `BuildError::PortOption` (since 0.4.0; until then the call panicked),
/// naming the first port given one.
#[test]
fn a_ports_options_are_its_subnets_to_declare() {
    use petritype::style::{PlaceStyle, Style};
    type Call = fn(&mut NetBuilder, PlaceRef<Hero>);
    let calls: [(&str, Call); 4] = [
        ("keys", heroes_keys),
        ("style", |b, port| {
            b.style(port, |_: &Hero| Style::default())
        }),
        ("styled", |b, port| b.styled(port)),
        ("place_style", |b, port| {
            b.place_style(port, PlaceStyle::fill_token());
        }),
    ];
    for (method, call) in calls {
        let mut w = world(false, &[]);
        call(&mut w.b, w.lobby);
        call(&mut w.b, w.hall);
        let error = w.b.build().expect_err("a port's option is its subnet's");
        assert_eq!(
            error,
            BuildError::PortOption {
                method,
                place: "Lobby".to_owned(),
                subnet: "Level 3".to_owned(),
            },
            "{method}"
        );
        assert_eq!(
            error.to_string(),
            format!(
                "place \"Lobby\" is a port of subnet place \"Level 3\", so `{method}` is not \
                 called on it here: what a port is, beyond its tokens, is declared in its \
                 subnet's definition"
            )
        );
    }
}

/// `serializable` on a port is refused at build as the other options are,
/// and reported before anything else the build checks, here a duplicate
/// place name.
#[cfg(feature = "serde")]
#[test]
fn a_ports_json_form_is_its_subnets_to_declare() {
    let mut level = NetBuilder::new();
    let lobby = level.place::<u32>("Lobby");
    level.port(lobby);
    let level = level.build().expect("a level");
    let mut b = NetBuilder::new();
    let level_3 = b.subnet_place("Level 3", level);
    let door = b.port_of::<u32>(level_3, "Lobby");
    b.serializable(door);
    b.place::<u32>("Twice");
    b.place::<u32>("Twice");
    assert_eq!(
        b.build().expect_err("a port's JSON form is its subnet's"),
        BuildError::PortOption {
            method: "serializable",
            place: "Lobby".to_owned(),
            subnet: "Level 3".to_owned(),
        }
    );
}

// ----- the build errors -----------------------------------------------------------

#[test]
fn port_of_refuses_a_port_the_subnet_does_not_have() {
    for name in ["Kitchen", "Arena"] {
        let level = level(false, &[]);
        let mut b = NetBuilder::new();
        let level_3 = b.subnet_place("Level 3", level.net);
        let ghost = b.port_of::<Hero>(level_3, name);
        // The handle works until the build refuses the net.
        b.seed(ghost, [hero("Ada", "Knight")]);
        let error = b.build().expect_err("no such port");
        assert!(
            matches!(&error, BuildError::NoSuchPort { subnet, port } if subnet == "Level 3" && port == name),
            "{error}"
        );
        assert_eq!(
            error.to_string(),
            format!("subnet place \"Level 3\" has no port \"{name}\"")
        );
    }
}

#[test]
fn port_of_refuses_the_wrong_type() {
    let level = level(false, &[]);
    let mut b = NetBuilder::new();
    let level_3 = b.subnet_place("Level 3", level.net);
    let _ = b.port_of::<u32>(level_3, "Lobby");
    let error = b.build().expect_err("the lobby holds heroes");
    assert!(
        matches!(&error, BuildError::PortTypeMismatch { port, found, .. } if port == "Lobby" && *found == "u32"),
        "{error}"
    );
}

#[test]
fn a_port_of_a_subnet_place_cannot_be_a_port_here() {
    let level = level(false, &[]);
    let mut b = NetBuilder::new();
    let level_3 = b.subnet_place("Level 3", level.net);
    let lobby = b.port_of::<Hero>(level_3, "Lobby");
    b.port(lobby);
    let error = b.build().expect_err("no edge crosses two borders");
    assert!(
        matches!(&error, BuildError::PortIsSubnetPlace { place, subnet } if place == "Lobby" && subnet == "Level 3"),
        "{error}"
    );
}
