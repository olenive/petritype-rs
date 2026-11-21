//! Integration tests for saving, restoring and forking nets with subnet
//! places, behind the `serde` feature.
//!
//! A subnet built through a template is stamped, and is saved, restored and
//! forked with the net around it: in a saved marking a subnet place's entry
//! is a list of one, the subnet in its saved form, and a port's tokens are
//! written once, in the subnet's marking. Restoring rebuilds the subnets as
//! the outer template builds them and fills each from its entry, checked
//! against the template that built it. A subnet built without a template
//! runs, and the net around it cannot be saved.
//!
//! These tests are the saved form, a save and a restore that steps as the
//! original would, the checks a restore makes, the unstamped refusal, a
//! fork at every level, a tree of subnets three deep, nets carried by
//! tokens through a port and subnets inside a token's net, the events of an
//! import, and the saved bytes of a net without subnet places, which this
//! change leaves as they were.

#![cfg(feature = "serde")]

use std::error::Error;

use petritype::serde_json::{self, Value, json};
use petritype::{
    BuildError, CommandError, DeepStep, Event, Marking, Net, NetBuilder, NetError, Param, Params,
    PlaceId, Stepping, TakeEdge, Templates, TieBreak, TokenId,
};
use serde::{Deserialize, Serialize};

type BoxedError = Box<dyn Error + Send + Sync>;

// ----- heroes, levels and worlds --------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Hero {
    name: String,
    xp: u64,
}

fn hero(name: &str) -> Hero {
    Hero {
        name: name.to_owned(),
        xp: 0,
    }
}

/// A level's depth: what a hero earns for leaving it. A wire parameter,
/// `"depth"`, which the level's factory reads from its params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Depth(u64);

/// A toll, a parameter with no wire name: a save does not carry it, a fork
/// does.
#[derive(Debug, Clone, PartialEq)]
struct Toll(u64);

/// How many levels a world has: its wire parameter `"levels"`, which its
/// factory reads to build that many subnet places.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Levels(u64);

/// The `Level` template: `Lobby -Walk-> Arena -Out-> Exit Hall`, the lobby
/// and the exit hall ports, `Out` adding the level's depth to the hero's xp.
fn level(params: &Params) -> Result<Net, BuildError> {
    let depth = params.get("depth").and_then(Value::as_u64).unwrap_or(1);
    let mut b = NetBuilder::new();
    let lobby = b.place::<Hero>("Lobby");
    let arena = b.place::<Hero>("Arena");
    let hall = b.place::<Hero>("Exit Hall");
    for place in [lobby, arena, hall] {
        b.serializable(place);
    }
    b.keys(lobby, ["name"], |h: &Hero| [h.name.as_str().into()]);
    b.port(lobby);
    b.port(hall);
    b.param(Depth(depth));
    b.serializable_param::<Depth>("depth");
    b.param(Toll(0));
    b.transition("Walk", (TakeEdge(lobby),), (arena,), |h: Hero| (h,));
    b.transition(
        "Out",
        (TakeEdge(arena), Param::<Depth>::new()),
        (hall,),
        |mut h: Hero, depth: Depth| {
            h.xp += depth.0;
            (h,)
        },
    );
    b.build()
}

/// The `World` template: a `Door`, an `Outside`, and as many levels as its
/// `"levels"` parameter says (two by default), `Level 1` to `Level n`, each
/// built by the `Level` template with its number as its depth, heroes
/// passing from the door through each level in turn and out.
fn world_net(levels: &Templates, params: &Params) -> Result<Net, BoxedError> {
    let count = params.get("levels").and_then(Value::as_u64).unwrap_or(2);
    let mut b = NetBuilder::new();
    let door = b.place::<Hero>("Door");
    let outside = b.place::<Hero>("Outside");
    b.serializable(door);
    b.serializable(outside);
    b.param(Levels(count));
    b.serializable_param::<Levels>("levels");
    let mut from = door;
    for n in 1..=count {
        let mut given = Params::new();
        given.insert("depth".into(), n.into());
        let place = b.subnet_place(format!("Level {n}"), levels.build("Level", &given)?);
        let lobby = b.port_of::<Hero>(place, "Lobby");
        let hall = b.port_of::<Hero>(place, "Exit Hall");
        b.transition(
            format!("Into Level {n}"),
            (TakeEdge(from),),
            (lobby,),
            |h: Hero| (h,),
        );
        from = hall;
    }
    b.transition("Leave", (TakeEdge(from),), (outside,), |h: Hero| (h,));
    Ok(b.build()?)
}

/// A registry with `Level` and `World`.
fn templates() -> Templates {
    let mut levels = Templates::new();
    levels.register("Level", level);
    let mut templates = levels.clone();
    templates.register("World", move |params: &Params| world_net(&levels, params));
    templates
}

/// A world of `levels` levels, with these heroes at the door.
fn world(templates: &Templates, levels: u64, heroes: &[&str]) -> Net {
    let mut params = Params::new();
    params.insert("levels".into(), levels.into());
    let mut net = templates.build("World", &params).expect("a world");
    let door = net.place_ref::<Hero>("Door").expect("a door");
    net.extend(door, heroes.iter().map(|name| hero(name)));
    net.drain_events().for_each(drop);
    net
}

fn place(net: &Net, name: &str) -> PlaceId {
    net.place_named(name).expect("a place of that name")
}

fn subnet<'a>(net: &'a Net, name: &str) -> &'a Net {
    net.subnet(place(net, name)).expect("a subnet place")
}

fn heroes(net: &Net, place: PlaceId) -> Vec<Hero> {
    net.tokens(place)
        .map(|(_, value)| value.downcast_ref::<Hero>().expect("a hero").clone())
        .collect()
}

fn ids(net: &Net, place: PlaceId) -> Vec<TokenId> {
    net.tokens(place).map(|(id, _)| id).collect()
}

/// A deep step as the names of the places it went through and the
/// transition that fired: `"Level 1 / Walk"`. Token ids are left out, since
/// a restored net numbers its tokens afresh.
fn describe(net: &Net, step: &DeepStep) -> String {
    let name_of = |net: &Net, place: PlaceId| {
        net.places()
            .find(|(id, _)| *id == place)
            .expect("a place")
            .1
            .name
            .to_owned()
    };
    match step {
        DeepStep::Here { transition } => net
            .transitions()
            .find(|(id, _)| id == transition)
            .expect("a transition")
            .1
            .name
            .to_owned(),
        DeepStep::Subnet { place, step, .. } => format!(
            "{} / {}",
            name_of(net, *place),
            describe(net.subnet(*place).expect("a subnet"), step)
        ),
        DeepStep::Inside {
            place, token, step, ..
        } => {
            let nets = net.nested(*place, *token).expect("a nested token");
            format!("{} / {}", name_of(net, *place), describe(nets[0].1, step))
        }
        other => unreachable!("a deep step these tests do not know: {other:?}"),
    }
}

/// Steps two nets side by side until both are at rest, checking at every
/// step that they fired the same thing and hold the same marking.
fn in_lockstep(a: &mut Net, b: &mut Net) -> usize {
    let mut steps = 0;
    loop {
        let (left, right) = (a.step_deep().expect("a"), b.step_deep().expect("b"));
        match (&left, &right) {
            (None, None) => return steps,
            (Some(left), Some(right)) => {
                assert_eq!(describe(a, left), describe(b, right), "step {steps}");
            }
            _ => panic!("one net stopped and the other did not: {left:?} {right:?}"),
        }
        assert_eq!(
            a.export_marking().unwrap(),
            b.export_marking().unwrap(),
            "after step {steps}"
        );
        steps += 1;
    }
}

// ----- the saved form -------------------------------------------------------------

#[test]
fn a_subnet_place_is_saved_as_a_list_of_one_and_a_port_once() {
    let templates = templates();
    let mut net = world(&templates, 2, &["Ada"]);
    // Nothing to do inside, so the world's own turn: Ada enters level 1.
    let step = net.step_deep().unwrap().unwrap();
    assert_eq!(describe(&net, &step), "Into Level 1");

    let level = |lobby: Value, depth: u64| {
        json!([{
            "template": "Level",
            "marking": {"Arena": [], "Exit Hall": [], "Lobby": lobby},
            "params": {"depth": depth},
            "enabled": {"Out": true, "Walk": true},
        }])
    };
    let marking = serde_json::to_value(net.export_marking().unwrap()).unwrap();
    assert_eq!(
        marking,
        json!({
            "Door": [],
            "Outside": [],
            "Level 1": level(json!([{"name": "Ada", "xp": 0}]), 1),
            "Level 2": level(json!([]), 2),
        }),
        "no entry of its own for a port: its tokens are in its subnet's marking"
    );
    let saved = net.to_json().unwrap();
    assert_eq!(
        saved,
        json!({
            "template": "World",
            "marking": marking,
            "params": {"levels": 2},
            "enabled": {"Into Level 1": true, "Into Level 2": true, "Leave": true},
        })
    );
}

// ----- saving and restoring ------------------------------------------------------

#[test]
fn a_restored_net_steps_as_the_original_would_at_every_level() {
    let templates = templates();
    let mut original = world(&templates, 2, &["Ada", "Bram", "Cato"]);
    // Part of the way, so that both levels and their ports hold someone.
    for _ in 0..6 {
        original.step_deep().unwrap().expect("something to do");
    }
    // A change of a subnet's wire parameter at run time, which the save
    // carries, and of a switch.
    let level_2 = place(&original, "Level 2");
    original
        .subnet_scope(level_2, |level| level.set_param(Depth(10)))
        .unwrap()
        .unwrap();
    assert_eq!(heroes(&original, place(&original, "Door")).len(), 1);
    let hall = original.port_ref::<Hero>(place(&original, "Level 1"), "Exit Hall");
    assert_eq!(original.count(hall.unwrap().id()), 2, "two in a port");

    let saved = original.to_json().unwrap();
    let mut restored = templates.restore(saved).expect("restores");
    assert_eq!(
        restored.export_marking().unwrap(),
        original.export_marking().unwrap()
    );
    assert_eq!(
        subnet(&restored, "Level 2").param::<Depth>(),
        Some(&Depth(10)),
        "the saved parameter, set over what the factory built"
    );
    // Restoring is setup, at every level.
    assert_eq!(restored.state_counter(), 0);
    assert_eq!(restored.drain_events().count(), 0);
    for name in ["Level 1", "Level 2"] {
        assert_eq!(subnet(&restored, name).state_counter(), 0);
        assert_ne!(
            subnet(&restored, name).id(),
            subnet(&original, name).id(),
            "a new net"
        );
    }
    // A port reads the same from both of the restored net's views.
    for name in ["Level 1", "Level 2"] {
        let level = place(&restored, name);
        for port in ["Lobby", "Exit Hall"] {
            let outside = restored.port_ref::<Hero>(level, port).unwrap();
            let inside = subnet(&restored, name).place_named(port).unwrap();
            assert_eq!(
                ids(&restored, outside.id()),
                ids(subnet(&restored, name), inside)
            );
        }
    }

    let steps = in_lockstep(&mut original, &mut restored);
    assert!(steps > 5, "{steps} steps to the end");
    let outside = place(&restored, "Outside");
    assert_eq!(heroes(&restored, outside), heroes(&original, outside));
    assert!(
        heroes(&restored, outside).iter().all(|h| h.xp == 11),
        "one for level 1, ten for level 2"
    );

    // The next id minted into a port is its subnet's, whichever net mints
    // it, and larger than every id already in the subnet.
    let level_1 = place(&restored, "Level 1");
    let lobby = restored.port_ref::<Hero>(level_1, "Lobby").unwrap();
    restored.put(lobby, hero("Cleo"));
    let inside = subnet(&restored, "Level 1");
    let before: Vec<TokenId> = inside
        .places()
        .flat_map(|(id, _)| ids(inside, id))
        .collect();
    assert!(!before.is_empty());
    let from_outside = restored.put(lobby, hero("Dora"));
    let from_inside = restored
        .subnet_scope(level_1, |level| {
            let lobby = level.place_ref::<Hero>("Lobby").unwrap();
            level.put(lobby, hero("Eli"))
        })
        .unwrap();
    assert!(before.iter().all(|id| *id < from_outside));
    assert!(from_outside < from_inside);
    assert_eq!(ids(&restored, lobby.id())[1..], [from_outside, from_inside]);
}

#[test]
fn a_world_built_from_params_is_rebuilt_from_them() {
    let templates = templates();
    let mut original = world(&templates, 3, &["Ada"]);
    original.step_deep().unwrap();
    let restored = templates.restore(original.to_json().unwrap()).unwrap();
    let names: Vec<&str> = restored
        .places()
        .filter(|(_, info)| info.subnet)
        .map(|(_, info)| info.name)
        .collect();
    assert_eq!(names, ["Level 1", "Level 2", "Level 3"]);
    assert_eq!(
        subnet(&restored, "Level 3").param::<Depth>(),
        Some(&Depth(3))
    );
    let mut original = original;
    let mut restored = restored;
    in_lockstep(&mut original, &mut restored);
}

#[test]
fn restoring_checks_each_saved_subnet_against_the_template_that_built_it() {
    let templates = templates();
    let mut net = world(&templates, 2, &["Ada"]);
    net.step_deep().unwrap();
    let saved = net.to_json().unwrap();

    // Saved from another template.
    let mut wrong = saved.clone();
    wrong["marking"]["Level 2"][0]["template"] = json!("Cellar");
    let error = templates.restore(wrong.clone()).unwrap_err().to_string();
    assert!(
        error.contains("\"Level 2\"")
            && error.contains("saved from template \"Cellar\"")
            && error.contains("built by \"Level\""),
        "{error}"
    );

    // The same refusal into a live net, which it leaves untouched and
    // silent.
    let marking: Marking = serde_json::from_value(wrong["marking"].clone()).unwrap();
    let before = net.export_marking().unwrap();
    net.drain_events().for_each(drop);
    let counter = net.state_counter();
    let error = net.import_marking(&marking).unwrap_err();
    assert!(
        matches!(&error, CommandError::CannotLoadSubnet { place, name, .. }
            if *place == net.place_named("Level 2").unwrap() && name == "Level 2"),
        "{error}"
    );
    assert_eq!(net.export_marking().unwrap(), before);
    assert_eq!(net.drain_events().count(), 0);
    assert_eq!(net.state_counter(), counter);

    // Each other way an entry can fail to fit, with its reason underneath.
    let mut source = |edit: &dyn Fn(&mut Value)| -> String {
        let mut marking = saved["marking"].clone();
        edit(&mut marking);
        let marking: Marking = serde_json::from_value(marking).unwrap();
        let error = net.import_marking(&marking).unwrap_err();
        let CommandError::CannotLoadSubnet { source, .. } = &error else {
            panic!("not a subnet's refusal: {error}");
        };
        assert_eq!(net.export_marking().unwrap(), before, "untouched");
        source.to_string()
    };
    assert_eq!(
        source(&|m| m["Level 1"] = json!([])),
        "a subnet place's entry is a list of one subnet in its saved form, and this one has 0"
    );
    assert!(source(&|m| m["Level 1"] = json!([{"marking": {}}])).contains("template"));
    assert_eq!(
        source(&|m| m["Level 1"][0]["marking"]["Kitchen"] = json!([])),
        "no place named \"Kitchen\""
    );
    assert_eq!(
        source(&|m| m["Level 1"][0]["params"]["speed"] = json!(3)),
        "no parameter named \"speed\""
    );
    assert_eq!(
        source(&|m| m["Level 1"][0]["enabled"]["Fly"] = json!(true)),
        "no transition named \"Fly\""
    );
    assert!(
        source(&|m| m["Level 1"][0]["marking"]["Lobby"] = json!(["Ada"]))
            .contains("token does not parse")
    );
}

#[test]
fn a_subnet_no_template_built_runs_and_its_net_cannot_be_saved_or_forked() {
    let mut templates = Templates::new();
    templates.register("Plain World", |_: &Params| -> Result<Net, BoxedError> {
        let mut b = NetBuilder::new();
        let door = b.place::<Hero>("Door");
        b.serializable(door);
        b.seed(door, [hero("Ada")]);
        let level_3 = b.subnet_place("Level 3", level(&Params::new())?);
        let lobby = b.port_of::<Hero>(level_3, "Lobby");
        b.transition("Enter", (TakeEdge(door),), (lobby,), |h: Hero| (h,));
        Ok(b.build()?)
    });
    let mut net = templates.build("Plain World", &Params::new()).unwrap();
    let level_3 = place(&net, "Level 3");
    while net.step_deep().expect("it runs").is_some() {}
    assert_eq!(heroes(subnet(&net, "Level 3"), PlaceId::new(2)).len(), 1);
    assert!(net.snapshot().is_ok(), "a snapshot needs no template");

    let error = net.export_marking().unwrap_err();
    assert!(
        matches!(&error, NetError::CannotSaveSubnet { place, name, source }
            if *place == level_3 && name == "Level 3"
                && matches!(source.downcast_ref::<NetError>(), Some(NetError::NoTemplate))),
        "{error}"
    );
    assert_eq!(
        error.to_string(),
        "place \"Level 3\" (PlaceId(1)) holds a subnet that cannot be saved: \
         the net was not built from a template, so it cannot be rebuilt"
    );
    assert!(
        net.to_json()
            .unwrap_err()
            .to_string()
            .contains("\"Level 3\"")
    );
    assert!(matches!(
        net.fork(),
        Err(NetError::CannotSaveSubnet { place, .. }) if place == level_3
    ));
    assert!(matches!(
        net.import_marking(&Marking::new()),
        Err(CommandError::CannotLoadSubnet { place, source, .. })
            if place == level_3 && matches!(source.downcast_ref::<NetError>(), Some(NetError::NoTemplate))
    ));
}

// ----- forking --------------------------------------------------------------------

#[test]
fn a_fork_copies_every_level_and_is_independent_of_the_original() {
    let templates = templates();
    let mut net = world(&templates, 2, &["Ada", "Bram"]);
    net.step_deep().unwrap();
    let level_1 = place(&net, "Level 1");
    let level_2 = place(&net, "Level 2");
    // What a save does not carry and a fork does: a parameter with no wire
    // name, the tie-break, the stepping rule out here; and a switch.
    net.subnet_scope(level_1, |level| {
        level.set_param(Toll(5)).unwrap();
        level.set_tie_break(TieBreak::RoundRobin);
        let out = level.transition_named("Out").unwrap();
        level.disable(out).unwrap();
    })
    .unwrap();
    net.set_stepping(level_2, Stepping::TakeTurns).unwrap();
    net.drain_events().for_each(drop);

    let mut fork = net.fork().expect("forks");
    assert_eq!(
        fork.export_marking().unwrap(),
        net.export_marking().unwrap()
    );
    let copy = subnet(&fork, "Level 1");
    assert_eq!(copy.param::<Toll>(), Some(&Toll(5)));
    assert_eq!(copy.tie_break(), TieBreak::RoundRobin);
    assert!(!copy.is_enabled(copy.transition_named("Out").unwrap()));
    assert_eq!(fork.stepping(level_2), Some(Stepping::TakeTurns));
    assert_ne!(copy.id(), subnet(&net, "Level 1").id());
    assert_eq!(fork.state_counter(), 0);
    assert_eq!(copy.state_counter(), 0);

    // Firing in the fork's subnet changes nothing in the original's.
    let lobby = fork.port_ref::<Hero>(level_1, "Lobby").unwrap();
    assert_eq!(fork.count(lobby.id()), 1);
    let step = fork.step_deep().unwrap().unwrap();
    assert_eq!(describe(&fork, &step), "Level 1 / Walk");
    assert_eq!(fork.count(lobby.id()), 0);
    assert_eq!(
        net.count(lobby.id()),
        1,
        "the original's Ada is still there"
    );
    assert_eq!(subnet(&net, "Level 1").fired_count(), 0);
    assert_eq!(net.drain_events().count(), 0);
}

// ----- a tree of subnets ------------------------------------------------------------

/// `Room`: a `Den` port where a hero sits down. `Floor`: a `Stairs` port up
/// to a room. `Tower`: a `Gate` up to a floor. Each built by the template
/// below it; `Bare Tower` is a tower whose floor holds a room no template
/// built.
fn tower_templates() -> Templates {
    fn room() -> Result<Net, BuildError> {
        let mut b = NetBuilder::new();
        let den = b.place::<Hero>("Den");
        let seat = b.place::<Hero>("Seat");
        b.serializable(den);
        b.serializable(seat);
        b.port(den);
        b.transition("Sit", (TakeEdge(den),), (seat,), |h: Hero| (h,));
        b.build()
    }
    fn floor(room: Net) -> Result<Net, BuildError> {
        let mut b = NetBuilder::new();
        let stairs = b.place::<Hero>("Stairs");
        b.serializable(stairs);
        b.port(stairs);
        let room = b.subnet_place("Room", room);
        let den = b.port_of::<Hero>(room, "Den");
        b.transition("Up", (TakeEdge(stairs),), (den,), |h: Hero| (h,));
        b.build()
    }
    fn tower(floor: Net) -> Result<Net, BuildError> {
        let mut b = NetBuilder::new();
        let gate = b.place::<Hero>("Gate");
        b.serializable(gate);
        let floor = b.subnet_place("Floor", floor);
        let stairs = b.port_of::<Hero>(floor, "Stairs");
        b.transition("In", (TakeEdge(gate),), (stairs,), |h: Hero| (h,));
        b.build()
    }
    let mut rooms = Templates::new();
    rooms.register("Room", |_: &Params| room());
    let mut floors = rooms.clone();
    let inner = rooms.clone();
    floors.register("Floor", move |params: &Params| -> Result<Net, BoxedError> {
        Ok(floor(inner.build("Room", params)?)?)
    });
    floors.register("Bare Floor", |_: &Params| -> Result<Net, BoxedError> {
        Ok(floor(room()?)?)
    });
    let mut towers = floors.clone();
    let inner = floors.clone();
    towers.register("Tower", move |params: &Params| -> Result<Net, BoxedError> {
        Ok(tower(inner.build("Floor", params)?)?)
    });
    let inner = floors.clone();
    towers.register(
        "Bare Tower",
        move |params: &Params| -> Result<Net, BoxedError> {
            Ok(tower(inner.build("Bare Floor", params)?)?)
        },
    );
    towers
}

#[test]
fn a_tree_of_subnets_saves_restores_forks_and_snapshots_at_every_depth() {
    let templates = tower_templates();
    let mut tower = templates.build("Tower", &Params::new()).unwrap();
    let gate = tower.place_ref::<Hero>("Gate").unwrap();
    tower.extend(gate, [hero("Ada"), hero("Bram")]);
    let steps: Vec<String> = (0..3)
        .map(|_| {
            let step = tower.step_deep().unwrap().unwrap();
            describe(&tower, &step)
        })
        .collect();
    assert_eq!(steps, ["In", "Floor / Up", "Floor / Room / Sit"]);

    let saved = tower.to_json().unwrap();
    assert_eq!(
        saved["marking"]["Floor"][0]["marking"]["Room"][0],
        json!({"template": "Room", "marking": {"Den": [], "Seat": [{"name": "Ada", "xp": 0}]},
               "params": {}, "enabled": {"Sit": true}})
    );
    let mut restored = templates.restore(saved).unwrap();
    let mut fork = tower.fork().unwrap();
    let taken = tower.snapshot().unwrap();
    let floor = place(&tower, "Floor");
    let room = subnet(&tower, "Floor").place_named("Room").unwrap();
    let seat = PlaceId::new(1);
    assert_eq!(
        taken
            .subnet(floor)
            .and_then(|floor| floor.subnet(room))
            .map(|room| room.count(seat)),
        Some(1),
        "a snapshot three levels down"
    );
    for copy in [&restored, &fork] {
        let room = subnet(copy, "Floor").subnet(room).expect("two levels down");
        assert_eq!(heroes(room, seat), [hero("Ada")]);
        assert_eq!(room.state_counter(), 0);
    }
    let mut also = tower.fork().unwrap();
    in_lockstep(&mut tower, &mut restored);
    in_lockstep(&mut fork, &mut also);
    assert_eq!(
        taken
            .subnet(floor)
            .unwrap()
            .subnet(room)
            .unwrap()
            .count(seat),
        1,
        "the snapshot is a moment"
    );

    // A room no template built, two levels down: the message names both.
    let mut bare = templates.build("Bare Tower", &Params::new()).unwrap();
    assert!(
        bare.step_deep().unwrap().is_none(),
        "it runs, with nothing to do"
    );
    assert_eq!(
        bare.to_json().unwrap_err().to_string(),
        "place \"Floor\" (PlaceId(1)) holds a subnet that cannot be saved: \
         place \"Room\" (PlaceId(1)) holds a subnet that cannot be saved: \
         the net was not built from a template, so it cannot be rebuilt"
    );
}

// ----- nets carried by tokens, and subnets inside them -----------------------------

#[test]
fn a_port_holding_nets_and_a_tokens_net_holding_a_subnet_are_saved_and_forked() {
    // A worker, a bare net with one job, by template.
    fn worker() -> Result<Net, BuildError> {
        let mut b = NetBuilder::new();
        let todo = b.place::<u64>("Todo");
        let done = b.place::<u64>("Done");
        b.serializable(todo);
        b.serializable(done);
        b.seed(todo, [1]);
        b.transition("Work", (TakeEdge(todo),), (done,), |n: u64| (n,));
        b.build()
    }
    let mut workers = Templates::new();
    workers.register("Worker", |_: &Params| worker());
    // A workshop whose bench, a port, holds workers.
    let mut shops = workers.clone();
    shops.register("Workshop", |_: &Params| {
        let mut b = NetBuilder::new();
        let bench = b.place_nested::<Net>("Bench");
        b.serializable(bench);
        b.port(bench);
        b.build()
    });
    // A yard of idle workers, and a workshop they are hired into.
    let mut templates = shops.clone();
    let inner = shops.clone();
    templates.register("Yard", move |_: &Params| -> Result<Net, BoxedError> {
        let mut b = NetBuilder::new();
        let yard = b.place_nested::<Net>("Yard");
        b.serializable(yard);
        b.seed(yard, [inner.build("Worker", &Params::new())?]);
        b.stepping(yard, Stepping::Passive);
        let shop = b.subnet_place("Workshop", inner.build("Workshop", &Params::new())?);
        let bench = b.port_of::<Net>(shop, "Bench");
        b.transition("Hire", (TakeEdge(yard),), (bench,), |n: Net| (n,));
        b.templates(&inner);
        Ok(b.build()?)
    });
    let mut net = templates.build("Yard", &Params::new()).unwrap();
    let step = net.step_deep().unwrap().unwrap();
    assert_eq!(describe(&net, &step), "Hire");
    let saved = net.to_json().unwrap();
    assert_eq!(
        saved["marking"]["Workshop"][0]["marking"]["Bench"][0]["template"], "Worker",
        "a worker in the port, saved once, in the workshop's marking"
    );
    let mut restored = templates.restore(saved).unwrap();
    let mut fork = net.fork().unwrap();
    let mut also = net.fork().unwrap();
    assert_eq!(in_lockstep(&mut net, &mut restored), 1, "the worker works");
    in_lockstep(&mut fork, &mut also);

    // A house, a token's net, with a room in a subnet place; a street of
    // houses.
    let mut rooms = Templates::new();
    rooms.register("Room", |_: &Params| {
        let mut b = NetBuilder::new();
        let den = b.place::<Hero>("Den");
        let seat = b.place::<Hero>("Seat");
        b.serializable(den);
        b.serializable(seat);
        b.port(den);
        b.seed(den, [hero("Ada")]);
        b.transition("Sit", (TakeEdge(den),), (seat,), |h: Hero| (h,));
        b.build()
    });
    let mut houses = rooms.clone();
    let inner = rooms.clone();
    houses.register("House", move |_: &Params| -> Result<Net, BoxedError> {
        let mut b = NetBuilder::new();
        let _room = b.subnet_place("Room", inner.build("Room", &Params::new())?);
        Ok(b.build()?)
    });
    let mut towns = houses.clone();
    let inner = houses.clone();
    towns.register("Town", move |_: &Params| -> Result<Net, BoxedError> {
        let mut b = NetBuilder::new();
        let street = b.place_nested::<Net>("Street");
        b.serializable(street);
        b.seed(street, [inner.build("House", &Params::new())?]);
        b.templates(&inner);
        Ok(b.build()?)
    });
    let town = towns.build("Town", &Params::new()).unwrap();
    let saved = town.to_json().unwrap();
    assert_eq!(
        saved["marking"]["Street"][0]["marking"]["Room"][0]["marking"]["Den"],
        json!([{"name": "Ada", "xp": 0}])
    );
    let street = place(&town, "Street");
    let taken = town.snapshot().expect("its house forks");
    let (house, _) = taken.tokens(street).next().unwrap();
    let nets = taken.nested(street, house).unwrap();
    assert_eq!(
        nets[0]
            .1
            .subnet(PlaceId::new(0))
            .map(|room| room.count(PlaceId::new(0))),
        Some(1),
        "the forked house in the snapshot holds its room"
    );
    let mut town = town;
    let mut restored = towns.restore(saved).unwrap();
    let mut fork = town.fork().unwrap();
    assert_eq!(in_lockstep(&mut town, &mut restored), 1, "Ada sits");
    let step = fork.step_deep().unwrap().unwrap();
    assert!(describe(&fork, &step).ends_with("/ Room / Sit"));
}

// ----- the events of an import ---------------------------------------------------

#[test]
fn an_import_tells_each_subnets_resync_and_then_its_own() {
    let templates = templates();
    let mut net = world(&templates, 2, &["Ada"]);
    net.step_deep().unwrap();
    let marking = net.export_marking().unwrap();
    let ids_before: Vec<_> = ["Level 1", "Level 2"]
        .map(|name| subnet(&net, name).id())
        .to_vec();
    net.drain_events().for_each(drop);
    let counter = net.state_counter();

    net.import_marking(&marking).unwrap();
    let (level_1, level_2) = (place(&net, "Level 1"), place(&net, "Level 2"));
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [
            Event::Subnet {
                place: level_1,
                nested_id: ids_before[0],
                event: Box::new(Event::Resynced)
            },
            Event::Subnet {
                place: level_2,
                nested_id: ids_before[1],
                event: Box::new(Event::Resynced)
            },
            Event::Resynced,
        ]
    );
    assert_eq!(net.state_counter(), counter + 3);
    assert_eq!(net.export_marking().unwrap(), marking);
    assert_eq!(
        ["Level 1", "Level 2"]
            .map(|name| subnet(&net, name).id())
            .to_vec(),
        ids_before,
        "the same subnets"
    );

    // A subnet place the marking does not mention has its subnet emptied,
    // its parameters and switches as they were.
    net.subnet_scope(level_1, |level| {
        let walk = level.transition_named("Walk").unwrap();
        level.disable(walk).unwrap();
    })
    .unwrap();
    let mut only_the_door = Marking::new();
    only_the_door.insert("Door", vec![json!({"name": "Bram", "xp": 0})]);
    net.import_marking(&only_the_door).unwrap();
    let inside = subnet(&net, "Level 1");
    assert!(inside.places().all(|(_, info)| info.count == 0));
    assert!(!inside.is_enabled(inside.transition_named("Walk").unwrap()));
    assert_eq!(inside.param::<Depth>(), Some(&Depth(1)));
    assert_eq!(heroes(&net, place(&net, "Door")), [hero("Bram")]);
}

// ----- nets without subnet places --------------------------------------------------

/// A net of the kind saved before subnet places existed: a nested place of
/// cabins, a plain place, a wire parameter and switches. Its saved text is
/// the bytes the commit before subnet places wrote for the same net.
#[test]
fn a_net_without_subnet_places_saves_to_the_bytes_it_always_did() {
    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct Stopped(bool);

    let mut cabins = Templates::new();
    cabins.register("Cabin", |_: &Params| {
        let mut b = NetBuilder::new();
        let seats = b.place::<String>("Seats");
        let door = b.place::<String>("Door");
        b.serializable(seats);
        b.serializable(door);
        b.seed(seats, ["Ada".to_owned()]);
        b.param(Stopped(true));
        b.serializable_param::<Stopped>("stopped");
        b.transition("Stand", (TakeEdge(seats),), (door,), |p: String| (p,));
        b.build()
    });
    let mut templates = cabins.clone();
    let inner = cabins.clone();
    templates.register("Depot", move |_: &Params| -> Result<Net, BoxedError> {
        let mut b = NetBuilder::new();
        let depot = b.place_nested::<Net>("Depot");
        let queue = b.place::<u32>("Queue");
        b.serializable(depot);
        b.serializable(queue);
        b.seed(depot, [inner.build("Cabin", &Params::new())?]);
        b.seed(queue, [1, 2]);
        b.stepping(depot, Stepping::Passive);
        b.transition("Serve", (TakeEdge(queue),), (queue,), |n: u32| (n,));
        b.templates(&inner);
        Ok(b.build()?)
    });
    let mut net = templates.build("Depot", &Params::new()).unwrap();
    let serve = net.transition_named("Serve").unwrap();
    net.disable(serve).unwrap();
    assert_eq!(
        serde_json::to_string(&net).unwrap(),
        concat!(
            r#"{"template":"Depot","marking":{"Depot":[{"enabled":{"Stand":true},"#,
            r#""marking":{"Door":[],"Seats":["Ada"]},"params":{"stopped":true},"template":"Cabin"}],"#,
            r#""Queue":[1,2]},"params":{},"enabled":{"Serve":false}}"#
        )
    );
    assert_eq!(
        serde_json::to_string(&net.export_marking().unwrap()).unwrap(),
        concat!(
            r#"{"Depot":[{"enabled":{"Stand":true},"marking":{"Door":[],"Seats":["Ada"]},"#,
            r#""params":{"stopped":true},"template":"Cabin"}],"Queue":[1,2]}"#
        )
    );
    let topology = serde_json::to_string(&net.topology()).unwrap();
    assert!(
        !topology.contains("subnet") && !topology.contains("port"),
        "{topology}"
    );
}
