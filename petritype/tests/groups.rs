//! Integration tests for groups (DESIGN §6.8).
//!
//! A group is a named set of a net's places and transitions, declared in
//! its definition, that only drawings read. The engine never does: firing,
//! stepping, ids, saving and the events are those of the same net with no
//! groups. A subnet, or a net carried by a token, has groups of its own,
//! and the place a net has for a port is in none.
//!
//! These tests are declaring and reading, the order of groups, a subnet's
//! and a token's net's own groups, the topology and a snapshot, the build
//! errors and the panics, the same net run with and without groups under
//! each tie-break, and, with the `serde` feature, a fork, a template, a
//! save and a restore, and the topology's JSON with and without groups.

use petritype::{
    BuildError, Event, GroupRef, MutateEdge, Net, NetBuilder, PlaceHandle, PlaceId, TakeEdge,
    TieBreak, TransitionBuilder,
};

// ----- a level and a world --------------------------------------------------------

/// Puts a place in `group`, if there is one.
fn join(b: &mut NetBuilder, group: Option<GroupRef>, place: impl PlaceHandle) {
    if let Some(group) = group {
        b.in_group(group, place);
    }
}

/// Puts a transition in `group`, if there is one.
fn put(transition: &mut TransitionBuilder<'_>, group: Option<GroupRef>) {
    if let Some(group) = group {
        transition.group(group);
    }
}

/// A level: `Lobby -Fight-> Arena -Win-> Exit Hall`, whose lobby and exit
/// hall are its ports. Grouped, the lobby, the arena and both transitions
/// are in the level's own group, `Inside`: inside its subnet a port is an
/// ordinary place, and may be in a group.
fn level(grouped: bool, tie_break: TieBreak) -> Result<Net, BuildError> {
    let mut b = NetBuilder::new();
    b.tie_break(tie_break);
    let inside = grouped.then(|| b.group("Inside"));
    let lobby = b.place::<u32>("Lobby");
    let arena = b.place::<u32>("Arena");
    let exit = b.place::<u32>("Exit Hall");
    #[cfg(feature = "serde")]
    for place in [lobby, arena, exit] {
        b.serializable(place);
    }
    b.port(lobby);
    b.port(exit);
    join(&mut b, inside, lobby);
    join(&mut b, inside, arena);
    put(
        &mut b.transition("Fight", (TakeEdge(lobby),), (arena,), |n: u32| (n,)),
        inside,
    );
    put(
        &mut b.transition("Win", (TakeEdge(arena),), (exit,), |n: u32| (n + 10,)),
        inside,
    );
    b.build()
}

/// The energy left: the one token in `place`.
fn energy(net: &Net, place: PlaceId) -> u32 {
    net.tokens(place)
        .find_map(|(_, value)| value.downcast_ref::<u32>())
        .copied()
        .unwrap_or(0)
}

/// The world around a level: walkers at the `Door` pay one `Energy` each to
/// `Enter` the level's lobby, `Eat` tops the energy up from the `Pack` once
/// it is spent, and walkers `Leave` the level's exit hall for `Outside`.
/// Grouped, `Walk` holds the door, the subnet place, outside, `Enter` and
/// `Leave`, and `Food` holds the pack and `Eat`; `Energy`, which both
/// change, is in none.
fn world(grouped: bool, tie_break: TieBreak, level: Net) -> Result<Net, BuildError> {
    let mut b = NetBuilder::new();
    b.tie_break(tie_break);
    let walk = grouped.then(|| b.group("Walk"));
    let food = grouped.then(|| b.group("Food"));
    let fuel = b.place::<u32>("Energy");
    let door = b.place::<u32>("Door");
    let level = b.subnet_place("Level", level);
    let lobby = b.port_of::<u32>(level, "Lobby");
    let exit = b.port_of::<u32>(level, "Exit Hall");
    let outside = b.place::<u32>("Outside");
    let pack = b.place::<u32>("Pack");
    #[cfg(feature = "serde")]
    for place in [fuel, door, outside, pack] {
        b.serializable(place);
    }
    join(&mut b, walk, door);
    join(&mut b, walk, level);
    join(&mut b, walk, outside);
    join(&mut b, food, pack);
    b.seed(fuel, [2]);
    b.seed(door, [1, 2, 3, 4]);
    b.seed(pack, [3, 3]);
    let mut enter = b.transition(
        "Enter",
        (TakeEdge(door), MutateEdge(fuel)),
        (lobby,),
        |walker: u32, fuel: &mut u32| {
            *fuel -= 1;
            (walker,)
        },
    );
    enter.guard(move |net| energy(net, fuel.id()) >= 1);
    put(&mut enter, walk);
    put(
        &mut b.transition("Leave", (TakeEdge(exit),), (outside,), |n: u32| (n,)),
        walk,
    );
    let mut eat = b.transition(
        "Eat",
        (TakeEdge(pack), MutateEdge(fuel)),
        (),
        |meal: u32, fuel: &mut u32| *fuel += meal,
    );
    eat.guard(move |net| energy(net, fuel.id()) == 0);
    put(&mut eat, food);
    b.build()
}

#[cfg(feature = "serde")]
type BoxedError = Box<dyn std::error::Error + Send + Sync>;

/// A registry with the `Level` and `World` templates, grouped or not, so
/// that a world can be saved, restored and forked.
#[cfg(feature = "serde")]
fn templates(grouped: bool, tie_break: TieBreak) -> petritype::Templates {
    use petritype::{Params, Templates};
    let mut levels = Templates::new();
    levels.register("Level", move |_: &Params| level(grouped, tie_break));
    let mut templates = levels.clone();
    templates.register("World", move |_: &Params| -> Result<Net, BoxedError> {
        Ok(world(
            grouped,
            tie_break,
            levels.build("Level", &Params::new())?,
        )?)
    });
    templates
}

/// A world, grouped or not, built through its template with the `serde`
/// feature and directly without it.
fn built(grouped: bool, tie_break: TieBreak) -> Net {
    #[cfg(feature = "serde")]
    let net = templates(grouped, tie_break)
        .build("World", &petritype::Params::new())
        .expect("the world builds");
    #[cfg(not(feature = "serde"))]
    let net = world(
        grouped,
        tie_break,
        level(grouped, tie_break).expect("the level builds"),
    )
    .expect("the world builds");
    net
}

/// Every place's tokens, by name, id and label, and every subnet's under
/// its place's name.
fn marking(net: &Net) -> Vec<String> {
    net.places()
        .flat_map(|(id, info)| {
            let tokens: Vec<String> = net
                .tokens(id)
                .map(|(token, _)| format!("{token}={}", net.label(id, token).unwrap()))
                .collect();
            let mut lines = vec![format!("{}: {}", info.name, tokens.join(" "))];
            if let Some(subnet) = net.subnet(id) {
                lines.extend(
                    marking(subnet)
                        .into_iter()
                        .map(|line| format!("{} / {line}", info.name)),
                );
            }
            lines
        })
        .collect()
}

/// Every net id in `text` replaced by its position in order of first
/// appearance: two builds of one net mint different ids, and are the same
/// net if they are the same in every other way and their ids line up.
fn renumber(text: &str) -> String {
    const ID: &str = "NetId(";
    let mut seen: Vec<String> = Vec::new();
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find(ID) {
        let (before, after) = rest.split_at(at + ID.len());
        out.push_str(before);
        let end = after.find(')').expect("an id ends with a bracket");
        let number = after[..end].to_owned();
        let position = match seen.iter().position(|seen| *seen == number) {
            Some(position) => position,
            None => {
                seen.push(number);
                seen.len() - 1
            }
        };
        out.push_str(&format!("net {position}"));
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// Deep steps to the end, at most a hundred: one line each, the step and
/// the events it logged, with the net ids renumbered.
fn journey(net: &mut Net) -> Vec<String> {
    let mut lines = Vec::new();
    while lines.len() < 100 {
        let Some(step) = net.step_deep().expect("nothing fails") else {
            break;
        };
        let events: Vec<Event> = net.drain_events().collect();
        lines.push(format!("{step:?} {events:?}"));
    }
    renumber(&lines.join("\n"))
        .lines()
        .map(str::to_owned)
        .collect()
}

const TIE_BREAKS: [TieBreak; 3] = [
    TieBreak::FirstDeclared,
    TieBreak::LastDeclared,
    TieBreak::RoundRobin,
];

// ----- declaring and reading ------------------------------------------------------

#[test]
fn each_node_says_which_group_it_is_in() {
    let net = built(true, TieBreak::FirstDeclared);
    assert_eq!(net.groups().collect::<Vec<_>>(), ["Walk", "Food"]);

    let places: Vec<(&str, Option<&str>)> = net
        .places()
        .map(|(_, info)| (info.name, info.group))
        .collect();
    assert_eq!(
        places,
        [
            ("Energy", None),
            ("Door", Some("Walk")),
            ("Level", Some("Walk")),
            ("Lobby", None),
            ("Exit Hall", None),
            ("Outside", Some("Walk")),
            ("Pack", Some("Food")),
        ],
        "a subnet place may be in a group, and the places for its ports are in none",
    );
    let transitions: Vec<(&str, Option<&str>)> = net
        .transitions()
        .map(|(_, info)| (info.name, info.group))
        .collect();
    assert_eq!(
        transitions,
        [
            ("Enter", Some("Walk")),
            ("Leave", Some("Walk")),
            ("Eat", Some("Food"))
        ]
    );
}

#[test]
fn a_net_with_no_groups_has_none_and_no_node_is_in_one() {
    let net = built(false, TieBreak::FirstDeclared);
    assert_eq!(net.groups().count(), 0);
    assert!(net.places().all(|(_, info)| info.group.is_none()));
    assert!(net.transitions().all(|(_, info)| info.group.is_none()));
    assert!(net.topology().groups.is_empty());
}

#[test]
fn declaring_groups_changes_no_id_and_no_order() {
    let grouped = built(true, TieBreak::FirstDeclared);
    let plain = built(false, TieBreak::FirstDeclared);
    let names = |net: &Net| {
        let places: Vec<(PlaceId, String)> = net
            .places()
            .map(|(id, info)| (id, info.name.to_owned()))
            .collect();
        let transitions: Vec<_> = net
            .transitions()
            .map(|(id, info)| (id, info.name.to_owned()))
            .collect();
        (places, transitions)
    };
    assert_eq!(names(&grouped), names(&plain));
    assert_eq!(
        marking(&grouped),
        marking(&plain),
        "the same seeds, ids and all"
    );

    // The shape is the same but for the groups.
    let mut shape = grouped.topology();
    assert_eq!(shape.groups, ["Walk", "Food"]);
    shape.groups.clear();
    for place in &mut shape.places {
        place.group = None;
    }
    for transition in &mut shape.transitions {
        transition.group = None;
    }
    assert_eq!(shape, plain.topology());
}

#[test]
fn groups_keep_the_order_they_were_declared_in() {
    let mut b = NetBuilder::new();
    let zebra = b.group("Zebra");
    let apple = b.group("Apple");
    let mango = b.group("Mango");
    let first = b.place::<u32>("First");
    let second = b.place::<u32>("Second");
    b.in_group(apple, first);
    b.in_group(zebra, second);
    b.transition("Move", (TakeEdge(first),), (second,), |n: u32| (n,))
        .group(apple);
    let net = b.build().expect("the net builds");

    assert_eq!(
        net.groups().collect::<Vec<_>>(),
        ["Zebra", "Apple", "Mango"],
        "declared order, not the order of their members, and an empty group is a group",
    );
    assert_eq!(net.topology().groups, ["Zebra", "Apple", "Mango"]);
    assert_eq!([zebra.index(), apple.index(), mango.index()], [0, 1, 2]);
    assert_eq!(net.groups().nth(apple.index() as usize), Some("Apple"));
    assert_eq!(apple.net(), net.id());
}

#[test]
fn the_same_group_again_changes_nothing() {
    let mut b = NetBuilder::new();
    let walk = b.group("Walk");
    let trail = b.place::<u32>("Trail");
    b.in_group(walk, trail);
    b.in_group(walk, trail);
    b.transition("Walk", (TakeEdge(trail),), (), |_: u32| {})
        .group(walk)
        .group(walk);
    let net = b.build().expect("one group, twice");
    assert_eq!(net.places().next().unwrap().1.group, Some("Walk"));
    assert_eq!(net.transitions().next().unwrap().1.group, Some("Walk"));
}

#[test]
fn a_group_name_may_hold_any_characters_and_any_node_s_name() {
    // At least one: the empty string is refused, as every empty name is.
    let names = ["Walk", "Walk / Eat", "Level 1 # 2", "a\"b\\c\nd", " "];
    let mut b = NetBuilder::new();
    let groups: Vec<GroupRef> = names.iter().map(|name| b.group(*name)).collect();
    // A group may share its name with a place and with a transition.
    let walk = b.place::<u32>("Walk");
    b.in_group(groups[1], walk);
    b.transition("Walk", (TakeEdge(walk),), (), |_: u32| {})
        .group(groups[0]);
    let net = b.build().expect("no character means anything");
    assert_eq!(net.groups().collect::<Vec<_>>(), names);
    assert_eq!(net.places().next().unwrap().1.group, Some("Walk / Eat"));
    assert_eq!(net.transitions().next().unwrap().1.group, Some("Walk"));
}

#[test]
fn a_subnet_has_groups_of_its_own() {
    let net = built(true, TieBreak::FirstDeclared);
    let level = net
        .subnet(net.place_named("Level").unwrap())
        .expect("a subnet place");
    assert_eq!(level.groups().collect::<Vec<_>>(), ["Inside"]);
    let places: Vec<(&str, Option<&str>)> = level
        .places()
        .map(|(_, info)| (info.name, info.group))
        .collect();
    assert_eq!(
        places,
        [
            ("Lobby", Some("Inside")),
            ("Arena", Some("Inside")),
            ("Exit Hall", None)
        ]
    );
    assert!(
        level
            .transitions()
            .all(|(_, info)| info.group == Some("Inside"))
    );
    assert_eq!(level.topology().groups, ["Inside"], "read alone, its own");
    assert!(
        !net.groups().any(|group| group == "Inside"),
        "the world has the groups its own definition gives it"
    );
}

#[test]
fn a_net_carried_by_a_token_has_groups_of_its_own() {
    let mut inner = NetBuilder::new();
    let work = inner.group("Work");
    let todo = inner.place::<u32>("Todo");
    inner.in_group(work, todo);
    let inner = inner.build().expect("a desk's work");

    let mut b = NetBuilder::new();
    let office = b.group("Office");
    let desk = b.place_nested::<Net>("Desk");
    b.in_group(office, desk);
    b.seed(desk, [inner]);
    let net = b.build().expect("an office");

    let (token, _) = net.tokens(desk.id()).next().unwrap();
    let nets = net.nested(desk.id(), token).expect("a nested place");
    assert_eq!(nets[0].1.groups().collect::<Vec<_>>(), ["Work"]);
    assert_eq!(nets[0].1.places().next().unwrap().1.group, Some("Work"));
    assert_eq!(net.groups().collect::<Vec<_>>(), ["Office"]);
    assert_eq!(net.places().next().unwrap().1.group, Some("Office"));
}

#[test]
fn a_snapshot_reads_and_shapes_the_groups_as_its_net_does() {
    let net = built(true, TieBreak::FirstDeclared);
    let taken = net.snapshot().expect("every place clones");
    assert_eq!(
        taken.groups().collect::<Vec<_>>(),
        net.groups().collect::<Vec<_>>()
    );
    assert_eq!(taken.topology(), net.topology());
    assert!(
        taken
            .places()
            .map(|(_, info)| info.group)
            .eq(net.places().map(|(_, info)| info.group))
    );
    assert!(
        taken
            .transitions()
            .map(|(_, info)| info.group)
            .eq(net.transitions().map(|(_, info)| info.group))
    );
    let level = net.place_named("Level").unwrap();
    assert_eq!(
        taken.subnet(level).unwrap().topology(),
        net.subnet(level).unwrap().topology()
    );
    assert_eq!(
        taken.clone().topology(),
        net.topology(),
        "and so does a clone"
    );
}

// ----- nothing about running changes -------------------------------------------

#[test]
fn a_net_steps_the_same_with_and_without_groups_under_each_tie_break() {
    for tie_break in TIE_BREAKS {
        let mut grouped = built(true, tie_break);
        let mut plain = built(false, tie_break);
        assert_eq!(grouped.groups().count(), 2);
        assert_eq!(plain.groups().count(), 0);

        let steps = journey(&mut grouped);
        assert!(steps.len() > 15, "{tie_break:?}: {steps:#?}");
        assert!(grouped.is_quiescent_deep(), "{tie_break:?}");
        assert_eq!(steps, journey(&mut plain), "{tie_break:?}");
        assert_eq!(marking(&grouped), marking(&plain), "{tie_break:?}");
        assert_eq!(grouped.fired_count(), plain.fired_count());
        assert_eq!(grouped.state_counter(), plain.state_counter());
        let outside = grouped.place_named("Outside").unwrap();
        assert_eq!(
            grouped.count(outside),
            4,
            "{tie_break:?}: everyone came out"
        );
    }
}

#[cfg(feature = "serde")]
#[test]
fn a_net_saves_the_same_with_and_without_groups_under_each_tie_break() {
    use petritype::serde_json;
    for tie_break in TIE_BREAKS {
        let mut grouped = built(true, tie_break);
        let mut plain = built(false, tie_break);
        let saved = |net: &Net| serde_json::to_string(net).expect("a stamped net saves");
        assert_eq!(
            saved(&grouped),
            saved(&plain),
            "{tie_break:?}, at the start"
        );
        for _ in 0..7 {
            grouped.step_deep().unwrap();
            plain.step_deep().unwrap();
        }
        assert_eq!(
            saved(&grouped),
            saved(&plain),
            "{tie_break:?}, part of the way"
        );
        assert_eq!(
            grouped.export_marking().unwrap(),
            plain.export_marking().unwrap()
        );
        journey(&mut grouped);
        journey(&mut plain);
        assert_eq!(saved(&grouped), saved(&plain), "{tie_break:?}, at the end");
    }
}

// ----- a fork, a template, a save -------------------------------------------------

#[cfg(feature = "serde")]
#[test]
fn a_saved_net_names_no_group_and_a_restore_has_them_from_its_template() {
    use petritype::serde_json;
    let templates = templates(true, TieBreak::FirstDeclared);
    let mut net = templates
        .build("World", &petritype::Params::new())
        .expect("a world");
    net.step_deep().unwrap();
    let saved = serde_json::to_string(&net).unwrap();
    for group in ["Walk", "Food", "Inside"] {
        assert!(!saved.contains(group), "{group} in {saved}");
    }

    let restored = templates
        .restore(serde_json::from_str(&saved).unwrap())
        .unwrap();
    assert_eq!(restored.topology(), net.topology(), "groups and all");
    let level = restored.place_named("Level").unwrap();
    assert_eq!(
        restored.subnet(level).unwrap().groups().collect::<Vec<_>>(),
        ["Inside"]
    );
    assert_eq!(
        restored.export_marking().unwrap(),
        net.export_marking().unwrap()
    );
}

#[cfg(feature = "serde")]
#[test]
fn a_fork_and_every_build_of_a_template_carry_the_groups() {
    let templates = templates(true, TieBreak::FirstDeclared);
    let mut net = templates
        .build("World", &petritype::Params::new())
        .expect("a world");
    net.step_deep().unwrap();
    let fork = net.fork().expect("a stamped world forks");
    let again = templates
        .build("World", &petritype::Params::new())
        .expect("and builds again");
    let level = net.place_named("Level").unwrap();
    for copy in [&fork, &again] {
        assert_eq!(copy.topology(), net.topology());
        assert_eq!(
            copy.subnet(level).unwrap().topology(),
            net.subnet(level).unwrap().topology()
        );
    }
    assert_eq!(
        fork.export_marking().unwrap(),
        net.export_marking().unwrap(),
        "the fork has the marking too, under ids of its own"
    );
}

// ----- the topology's JSON --------------------------------------------------------

#[cfg(feature = "serde")]
#[test]
fn the_topology_writes_the_groups_only_when_there_are_some() {
    use petritype::serde_json::{self, json};
    let net = built(true, TieBreak::FirstDeclared);
    let topology = serde_json::to_value(net.topology()).unwrap();
    assert_eq!(topology["groups"], json!(["Walk", "Food"]));
    assert_eq!(topology["places"][1]["group"], json!("Walk"));
    assert_eq!(
        topology["places"][2]["group"],
        json!("Walk"),
        "the subnet place"
    );
    assert_eq!(topology["places"][6]["group"], json!("Food"));
    for place in [0, 3, 4] {
        assert!(
            topology["places"][place].get("group").is_none(),
            "{}",
            topology["places"][place]
        );
    }
    assert_eq!(topology["transitions"][0]["group"], json!("Walk"));
    assert_eq!(topology["transitions"][2]["group"], json!("Food"));
    let level = net.subnet(net.place_named("Level").unwrap()).unwrap();
    assert_eq!(
        serde_json::to_value(level.topology()).unwrap()["groups"],
        json!(["Inside"])
    );
}

/// A world with no groups writes its topology, and saves, to the bytes the
/// commit before groups wrote for the same net. The grouped world saves to
/// the same bytes as this one (above), so to these too.
#[cfg(feature = "serde")]
#[test]
fn a_net_with_no_groups_writes_and_saves_as_it_did_before_them() {
    use petritype::serde_json;
    let net = built(false, TieBreak::FirstDeclared);
    assert_eq!(
        serde_json::to_string(&net.topology()).unwrap(),
        concat!(
            r#"{"places":[{"id":0,"name":"Energy","type_name":"u32","serializable":true},"#,
            r#"{"id":1,"name":"Door","type_name":"u32","serializable":true},"#,
            r#"{"id":2,"name":"Level","type_name":"<subnet>","serializable":false,"#,
            r#""stepping":"inner_first","subnet":true},"#,
            r#"{"id":3,"name":"Lobby","type_name":"u32","serializable":true,"port_of":2},"#,
            r#"{"id":4,"name":"Exit Hall","type_name":"u32","serializable":true,"port_of":2},"#,
            r#"{"id":5,"name":"Outside","type_name":"u32","serializable":true},"#,
            r#"{"id":6,"name":"Pack","type_name":"u32","serializable":true}],"#,
            r#""transitions":[{"id":0,"name":"Enter","inputs":[[1,"take"],[0,"mutate"]],"#,
            r#""params":[],"outputs":[3],"detachable":false},"#,
            r#"{"id":1,"name":"Leave","inputs":[[4,"take"]],"params":[],"outputs":[5],"#,
            r#""detachable":true},"#,
            r#"{"id":2,"name":"Eat","inputs":[[6,"take"],[0,"mutate"]],"params":[],"#,
            r#""outputs":[],"detachable":false}],"#,
            r#""edges":[{"edge":"input","place":1,"transition":0,"kind":"take"},"#,
            r#"{"edge":"input","place":0,"transition":0,"kind":"mutate"},"#,
            r#"{"edge":"output","transition":0,"place":3,"slot":0},"#,
            r#"{"edge":"input","place":4,"transition":1,"kind":"take"},"#,
            r#"{"edge":"output","transition":1,"place":5,"slot":0},"#,
            r#"{"edge":"input","place":6,"transition":2,"kind":"take"},"#,
            r#"{"edge":"input","place":0,"transition":2,"kind":"mutate"}],"#,
            r#""topology_counter":0}"#,
        )
    );
    assert_eq!(
        serde_json::to_string(&net).unwrap(),
        concat!(
            r#"{"template":"World","marking":{"Door":[1,2,3,4],"Energy":[2],"#,
            r#""Level":[{"enabled":{"Fight":true,"Win":true},"#,
            r#""marking":{"Arena":[],"Exit Hall":[],"Lobby":[]},"params":{},"template":"Level"}],"#,
            r#""Outside":[],"Pack":[3,3]},"params":{},"#,
            r#""enabled":{"Eat":true,"Enter":true,"Leave":true}}"#,
        )
    );
}

// ----- the build errors and the panics --------------------------------------------

#[test]
fn two_groups_of_one_name_are_refused() {
    let mut b = NetBuilder::new();
    let _ = b.group("Walk");
    let _ = b.group("Eat");
    let _ = b.group("Walk");
    let error = b.build().expect_err("two groups called Walk");
    assert!(
        matches!(&error, BuildError::DuplicateGroupName { name } if name == "Walk"),
        "{error}"
    );
    assert_eq!(error.to_string(), "duplicate group name: \"Walk\"");
}

#[test]
fn a_place_in_two_groups_is_refused() {
    let mut b = NetBuilder::new();
    let walk = b.group("Walk");
    let eat = b.group("Eat");
    let energy = b.place::<u32>("Energy");
    b.in_group(walk, energy);
    b.in_group(eat, energy);
    let error = b.build().expect_err("one place, two groups");
    assert!(
        matches!(&error, BuildError::PlaceInTwoGroups { place, first, second }
            if place == "Energy" && first == "Walk" && second == "Eat"),
        "{error}"
    );
    assert_eq!(
        error.to_string(),
        "place \"Energy\" is put in group \"Eat\", but it is in group \"Walk\" already"
    );
}

#[test]
fn a_subnet_place_in_two_groups_is_refused_as_a_place() {
    let mut b = NetBuilder::new();
    let walk = b.group("Walk");
    let eat = b.group("Eat");
    let level = b.subnet_place("Level", level(false, TieBreak::FirstDeclared).unwrap());
    b.in_group(walk, level);
    b.in_group(eat, level);
    let error = b.build().expect_err("one subnet place, two groups");
    assert!(
        matches!(&error, BuildError::PlaceInTwoGroups { place, .. } if place == "Level"),
        "{error}"
    );
}

#[test]
fn a_transition_in_two_groups_is_refused() {
    let mut b = NetBuilder::new();
    let walk = b.group("Walk");
    let eat = b.group("Eat");
    let trail = b.place::<u32>("Trail");
    b.transition("Fund Walk", (TakeEdge(trail),), (), |_: u32| {})
        .group(walk)
        .group(eat);
    let error = b.build().expect_err("one transition, two groups");
    assert!(
        matches!(&error, BuildError::TransitionInTwoGroups { transition, first, second }
            if transition == "Fund Walk" && first == "Walk" && second == "Eat"),
        "{error}"
    );
    assert_eq!(
        error.to_string(),
        "transition \"Fund Walk\" is put in group \"Eat\", but it is in group \"Walk\" already"
    );
}

#[test]
fn a_port_s_place_in_a_group_is_refused() {
    let mut b = NetBuilder::new();
    let walk = b.group("Walk");
    let level = b.subnet_place("Level 3", level(false, TieBreak::FirstDeclared).unwrap());
    let lobby = b.port_of::<u32>(level, "Lobby");
    b.in_group(walk, lobby);
    let error = b.build().expect_err("a port goes where its box goes");
    assert!(
        matches!(&error, BuildError::PortInGroup { place, subnet, group }
            if place == "Lobby" && subnet == "Level 3" && group == "Walk"),
        "{error}"
    );
    assert_eq!(
        error.to_string(),
        "place \"Lobby\" is a port of subnet place \"Level 3\", so it cannot be put in group \
         \"Walk\": it is drawn in its subnet place's box"
    );
}

#[test]
fn the_first_group_mistake_is_the_one_reported_after_the_names() {
    let mut b = NetBuilder::new();
    let walk = b.group("Walk");
    let eat = b.group("Eat");
    let trail = b.place::<u32>("Trail");
    b.transition("Go", (TakeEdge(trail),), (), |_: u32| {})
        .group(walk)
        .group(eat);
    b.in_group(walk, trail);
    b.in_group(eat, trail);
    let error = b.build().expect_err("two mistakes");
    assert!(
        matches!(&error, BuildError::TransitionInTwoGroups { .. }),
        "{error}"
    );

    let mut b = NetBuilder::new();
    let walk = b.group("Walk");
    let eat = b.group("Eat");
    let trail = b.place::<u32>("Trail");
    b.in_group(walk, trail);
    b.in_group(eat, trail);
    let _ = b.group("Walk");
    let error = b.build().expect_err("two mistakes");
    assert!(
        matches!(&error, BuildError::DuplicateGroupName { .. }),
        "{error}"
    );
}

#[test]
#[should_panic(expected = "in_group: place \"Here\" is put in group #0 of net #")]
fn a_group_from_another_builder_panics_for_a_place() {
    let mut other = NetBuilder::new();
    let foreign = other.group("Elsewhere");
    let mut b = NetBuilder::new();
    let here = b.place::<u32>("Here");
    b.in_group(foreign, here);
}

#[test]
#[should_panic(expected = "group: transition \"Go\" is put in group #0 of net #")]
fn a_group_from_another_builder_panics_for_a_transition() {
    let mut other = NetBuilder::new();
    let foreign = other.group("Elsewhere");
    let mut b = NetBuilder::new();
    let here = b.place::<u32>("Here");
    b.transition("Go", (TakeEdge(here),), (), |_: u32| {})
        .group(foreign);
}

#[test]
#[should_panic(expected = "but this is net")]
fn a_place_from_another_builder_panics_in_a_group() {
    let mut other = NetBuilder::new();
    let foreign = other.place::<u32>("Elsewhere");
    let mut b = NetBuilder::new();
    let walk = b.group("Walk");
    b.in_group(walk, foreign);
}
