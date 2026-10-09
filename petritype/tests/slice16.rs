//! Integration tests for the sixteenth slice: saving and forking nets as
//! tokens (DESIGN §6.6, phase 2), all behind the `serde` feature.
//!
//! A marking goes out as JSON by place name and comes back wholesale, with
//! one `Resynced`; a template builds a net by name and stamps it, so that the
//! net can be saved (`Serialize`), restored (`Deserialize`, through a
//! registry in scope) and forked; and a snapshot copies a nested place's
//! tokens by forking them. The transit example (`examples/transit.rs`) is the
//! acceptance script's step 6 on top of this.

#![cfg(feature = "serde")]

use petritype::serde_json::{self, json};
use petritype::{
    CommandError, Event, JsonCommand, Marking, MutateEdge, Nested, Net, NetBuilder, NetError,
    Param, Params, TakeEdge, TemplateError, Templates, TieBreak,
};
use serde::{Deserialize, Serialize};

// ----- a few nets -------------------------------------------------------------

/// How fast a queue works: a parameter with a wire name.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct Speed(u64);

/// A note on a queue: a parameter with no wire name, which a save leaves
/// out and a fork keeps.
#[derive(Debug, Clone, PartialEq)]
struct Note(String);

/// `Queue`: `Todo -> Work -> Done`, numbers, `speed` from its parameters.
fn queue(params: &Params) -> Result<Net, Box<dyn std::error::Error + Send + Sync>> {
    let speed = params
        .get("speed")
        .and_then(|speed| speed.as_u64())
        .unwrap_or(1);
    let mut b = NetBuilder::new();
    let todo = b.place::<u64>("Todo");
    let done = b.place::<u64>("Done");
    b.serializable(todo);
    b.serializable(done);
    b.param(Speed(speed));
    b.serializable_param::<Speed>("speed");
    b.param(Note(String::from("as built")));
    b.transition(
        "Work",
        (TakeEdge(todo), Param::<Speed>::new()),
        (done,),
        |n: u64, speed: Speed| (n * speed.0,),
    );
    Ok(b.build()?)
}

fn queues() -> Templates {
    let mut templates = Templates::new();
    templates.register("Queue", queue);
    templates
}

/// A token owning one net, with a fork.
#[derive(Debug, Serialize, Deserialize)]
struct Bus {
    name: String,
    cabin: Net,
}

impl Nested for Bus {
    fn nets(&self) -> Vec<(&str, &Net)> {
        vec![("cabin", &self.cabin)]
    }
    fn nets_mut(&mut self) -> Vec<(&str, &mut Net)> {
        vec![("cabin", &mut self.cabin)]
    }
    fn fork(&self) -> Result<Self, NetError> {
        Ok(Bus {
            name: self.name.clone(),
            cabin: self.cabin.fork()?,
        })
    }
}

/// A token owning one net, without a fork: phase 1's implementors look like
/// this, and still compile.
#[derive(Debug, Serialize, Deserialize)]
struct Crate {
    cargo: Net,
}

impl Nested for Crate {
    fn nets(&self) -> Vec<(&str, &Net)> {
        vec![("cargo", &self.cargo)]
    }
    fn nets_mut(&mut self) -> Vec<(&str, &mut Net)> {
        vec![("cargo", &mut self.cargo)]
    }
}

/// A depot of buses, its queues rebuilt from `templates`.
fn depot(templates: &Templates, buses: Vec<Bus>) -> (Net, petritype::PlaceRef<Bus>) {
    let mut b = NetBuilder::new();
    b.templates(templates);
    let yard = b.place_nested::<Bus>("Yard");
    b.serializable(yard);
    b.seed(yard, buses);
    (b.build().expect("builds"), yard)
}

fn bus(templates: &Templates, name: &str, todo: &[u64]) -> Bus {
    let mut cabin = templates.build("Queue", &Params::new()).expect("a queue");
    let place = cabin.place_ref::<u64>("Todo").expect("Todo");
    cabin.extend(place, todo.iter().copied());
    Bus {
        name: name.to_owned(),
        cabin,
    }
}

// ----- R5a: marking export and import ---------------------------------------

#[test]
fn a_plain_net_round_trips_its_marking_through_json_text() {
    fn build() -> (Net, petritype::PlaceRef<String>, petritype::PlaceRef<u32>) {
        let mut b = NetBuilder::new();
        let names = b.place::<String>("Names");
        let ages = b.place::<u32>("Ages");
        let empty = b.place::<u32>("Empty");
        b.serializable(names);
        b.serializable(ages);
        b.serializable(empty);
        (b.build().expect("builds"), names, ages)
    }
    let (mut net, names, ages) = build();
    net.extend(names, ["Ada", "Bram", "Cato"].map(String::from));
    net.extend(ages, [36, 41]);

    let text = serde_json::to_string(&net.export_marking().expect("all serializable")).unwrap();
    assert_eq!(
        text, r#"{"Ages":[36,41],"Empty":[],"Names":["Ada","Bram","Cato"]}"#,
        "by place name, in name order, oldest first"
    );

    let (mut fresh, names, _) = build();
    let marking: Marking = serde_json::from_str(&text).unwrap();
    fresh.import_marking(&marking).expect("imports");
    assert_eq!(
        fresh.export_marking().unwrap(),
        net.export_marking().unwrap()
    );
    assert_eq!(
        fresh.drain_events().collect::<Vec<_>>(),
        [Event::Resynced],
        "one event for the whole import"
    );
    assert_eq!(fresh.state_counter(), 1);
    let ids: Vec<u64> = fresh.tokens(names.id()).map(|(id, _)| id.get()).collect();
    assert_eq!(ids, [0, 1, 2], "fresh ids, minted by the net that imports");
}

#[test]
fn a_marking_has_a_small_api_of_its_own() {
    let mut marking = Marking::new();
    assert!(marking.is_empty());
    assert_eq!(marking.insert("Door", vec![json!("Ada")]), None);
    marking.insert("Seats", vec![]);
    assert_eq!(marking.len(), 2);
    assert_eq!(marking.get("Door"), Some(&[json!("Ada")][..]));
    assert_eq!(marking.get("Roof"), None);
    let places: Vec<&str> = marking.iter().map(|(name, _)| name).collect();
    assert_eq!(places, ["Door", "Seats"]);
    assert_eq!(
        serde_json::to_value(&marking).unwrap(),
        json!({"Door": ["Ada"], "Seats": []})
    );
}

#[test]
fn export_wants_every_place_serializable_and_names_the_first_that_is_not() {
    let mut b = NetBuilder::new();
    let marked = b.place::<u32>("Marked");
    b.serializable(marked);
    b.place::<u32>("Plain");
    b.place::<u32>("Also Plain");
    let net = b.build().unwrap();
    let error = net.export_marking().expect_err("Plain has no encoder");
    assert!(
        matches!(&error, NetError::NotSerializable { name, .. } if name == "Plain"),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "place \"Plain\" (PlaceId(1)) is not marked serializable"
    );
}

#[test]
fn a_missing_place_loads_empty_and_a_rejected_import_changes_nothing() {
    let mut b = NetBuilder::new();
    let a = b.place::<u32>("A");
    let c = b.place::<u32>("C");
    b.serializable(a);
    b.serializable(c);
    b.seed(a, [1]);
    b.seed(c, [2, 3]);
    let mut net = b.build().unwrap();

    let only_a: Marking = serde_json::from_value(json!({"A": [7, 8]})).unwrap();
    net.import_marking(&only_a).unwrap();
    assert_eq!(net.count(a.id()), 2);
    assert_eq!(net.count(c.id()), 0, "not mentioned: loads empty");
    net.drain_events().for_each(drop);
    let before = net.export_marking().unwrap();
    let counter = net.state_counter();

    for (marking, check) in [
        (
            json!({"A": [1], "B": [2]}),
            (|e: &CommandError| matches!(e, CommandError::NoSuchPlaceName { name } if name == "B"))
                as fn(&CommandError) -> bool,
        ),
        (
            json!({"A": [1], "C": ["three"]}),
            |e| matches!(e, CommandError::InvalidToken { name, .. } if name == "C"),
        ),
    ] {
        let marking: Marking = serde_json::from_value(marking).unwrap();
        let error = net.import_marking(&marking).expect_err("rejected");
        assert!(check(&error), "{error:?}");
        assert_eq!(net.export_marking().unwrap(), before, "untouched");
        assert_eq!(net.state_counter(), counter);
        assert_eq!(net.drain_events().count(), 0, "silent");
    }

    // A place that is not serializable cannot be filled, mentioned or not.
    let mut b = NetBuilder::new();
    let marked = b.place::<u32>("Marked");
    b.serializable(marked);
    b.place::<u32>("Plain");
    let mut net = b.build().unwrap();
    let error = net
        .import_marking(&Marking::new())
        .expect_err("Plain cannot be filled");
    assert!(
        matches!(&error, CommandError::NotSerializable { name, .. } if name == "Plain"),
        "{error:?}"
    );
}

#[test]
fn resynced_has_a_wire_form_and_no_fields() {
    assert_eq!(
        serde_json::to_value(Event::Resynced).unwrap(),
        json!({"event": "resynced"})
    );
}

// ----- R5b: templates ---------------------------------------------------------

#[test]
fn a_template_builds_and_stamps_a_net() {
    let mut templates = queues();
    templates.register("Broken", |_: &Params| -> Result<Net, String> {
        Err("no parts".into())
    });
    assert_eq!(templates.names().collect::<Vec<_>>(), ["Broken", "Queue"]);
    assert_eq!(
        format!("{templates:?}"),
        r#"Templates { names: ["Broken", "Queue"] }"#
    );

    let net = templates.build("Queue", &Params::new()).expect("builds");
    assert_eq!(net.template(), Some("Queue"));
    assert_eq!(net.param::<Speed>(), Some(&Speed(1)));
    let mut params = Params::new();
    params.insert("speed".into(), json!(3));
    let fast = templates.build("Queue", &params).expect("builds");
    assert_eq!(fast.param::<Speed>(), Some(&Speed(3)));

    assert!(matches!(
        templates.build("Tram", &Params::new()),
        Err(TemplateError::NoSuchTemplate { ref name }) if name == "Tram"
    ));
    let error = templates
        .build("Broken", &Params::new())
        .expect_err("the factory fails");
    assert_eq!(
        error.to_string(),
        "template \"Broken\" failed to build its net: no parts"
    );

    let plain = NetBuilder::new().build().unwrap();
    assert_eq!(plain.template(), None, "a builder does not stamp");
}

#[test]
fn the_builder_takes_templates_one_by_one_or_a_registry_at_a_time() {
    let mut b = NetBuilder::new();
    b.templates(&queues())
        .template("Empty", |_: &Params| NetBuilder::new().build());
    let net = b.build().unwrap();
    assert_eq!(
        net.templates().names().collect::<Vec<_>>(),
        ["Empty", "Queue"]
    );
    assert_eq!(
        NetBuilder::new()
            .build()
            .unwrap()
            .templates()
            .names()
            .count(),
        0
    );
}

// ----- R5: a net as JSON --------------------------------------------------------

#[test]
fn a_stamped_net_saves_as_template_marking_params_and_switches() {
    let mut net = queues().build("Queue", &Params::new()).unwrap();
    let todo = net.place_ref::<u64>("Todo").unwrap();
    net.extend(todo, [4, 5]);
    net.set_param(Speed(2)).unwrap();
    net.set_param(Note("changed".into())).unwrap();
    let work = net.transition_named("Work").unwrap();
    net.disable(work).unwrap();

    assert_eq!(
        net.to_json().expect("a stamped net saves"),
        json!({
            "template": "Queue",
            "marking": {"Done": [], "Todo": [4, 5]},
            "params": {"speed": 2},
            "enabled": {"Work": false},
        }),
        "exactly four keys; Note has no wire name and is not saved",
    );
}

#[test]
fn a_net_that_could_not_be_restored_does_not_pretend_to_save() {
    let unstamped = NetBuilder::new().build().unwrap();
    let error = serde_json::to_value(&unstamped).expect_err("no template");
    assert_eq!(
        error.to_string(),
        "the net was not built from a template, so it cannot be rebuilt"
    );

    let mut templates = Templates::new();
    templates.register("Unmarked", |_: &Params| {
        let mut b = NetBuilder::new();
        b.place::<u32>("Plain");
        b.build()
    });
    let unmarked = templates.build("Unmarked", &Params::new()).unwrap();
    assert!(
        unmarked
            .to_json()
            .expect_err("Plain")
            .to_string()
            .contains("not marked serializable")
    );
}

#[test]
fn a_saved_net_is_restored_through_the_registry_in_scope() {
    let templates = queues();
    let mut net = templates.build("Queue", &Params::new()).unwrap();
    let todo = net.place_ref::<u64>("Todo").unwrap();
    net.extend(todo, [1, 2]);
    net.set_param(Speed(5)).unwrap();
    net.disable(net.transition_named("Work").unwrap()).unwrap();
    let saved = net.to_json().unwrap();

    let restored = templates.restore(saved.clone()).expect("restores");
    assert_eq!(restored.template(), Some("Queue"));
    assert_eq!(
        restored.export_marking().unwrap(),
        net.export_marking().unwrap()
    );
    assert_eq!(
        restored.param::<Speed>(),
        Some(&Speed(5)),
        "the factory was given 5 and it was set again"
    );
    assert!(!restored.is_enabled(restored.transition_named("Work").unwrap()));
    assert_ne!(restored.id(), net.id());
    assert_eq!(restored.state_counter(), 0, "restoring is setup");
    assert_eq!(restored.fired_count(), 0);
    let mut restored = restored;
    assert_eq!(restored.drain_events().count(), 0);

    // `scope` is what `restore` is: the same through `from_value` by hand.
    let by_hand: Net = templates
        .scope(|| serde_json::from_value(saved.clone()))
        .expect("restores");
    assert_eq!(by_hand.to_json().unwrap(), saved);
}

#[test]
fn restoring_says_what_is_wrong() {
    let templates = queues();
    let saved = templates
        .build("Queue", &Params::new())
        .unwrap()
        .to_json()
        .unwrap();

    let outside = serde_json::from_value::<Net>(saved.clone()).expect_err("no scope");
    assert!(
        outside
            .to_string()
            .starts_with("no template registry in scope: deserialise inside Templates::scope"),
        "{outside}"
    );

    let mut bad = saved.clone();
    bad["template"] = json!("Tram");
    let error = templates.restore(bad).expect_err("unknown template");
    assert!(
        error
            .to_string()
            .starts_with(r#"no template named "Tram" in scope (registered: ["Queue"])"#),
        "{error}"
    );

    for (field, value, says) in [
        ("params", json!({"sped": 3}), r#"no parameter named "sped""#),
        ("params", json!({"speed": "fast"}), r#"parameter "speed""#),
        (
            "enabled",
            json!({"Wrok": true}),
            r#"no transition named "Wrok""#,
        ),
        ("marking", json!({"Roof": []}), r#"no place named "Roof""#),
        ("marking", json!({"Todo": ["one"]}), r#"place "Todo""#),
    ] {
        let mut bad = saved.clone();
        bad[field] = value;
        let error = templates.restore(bad).expect_err(says).to_string();
        assert!(
            error.starts_with("template \"Queue\": ") && error.contains(says),
            "{error}"
        );
    }

    // A field this version does not know is ignored, as the protocol reads
    // every object (PROTOCOL.md §2): a later runtime may add one.
    let mut extra = saved;
    extra["template_params"] = json!({});
    templates
        .restore(extra)
        .expect("an unknown field is ignored, not refused");
}

#[test]
fn a_nested_place_saves_and_loads_its_tokens_inner_nets() {
    let templates = queues();
    let (net, yard) = depot(&templates, vec![bus(&templates, "Bus 1", &[1, 2])]);
    let marking = net.export_marking().expect("the bus saves");
    assert_eq!(
        serde_json::to_value(&marking).unwrap(),
        json!({"Yard": [{
            "name": "Bus 1",
            "cabin": {"template": "Queue", "marking": {"Done": [], "Todo": [1, 2]},
                      "params": {"speed": 1}, "enabled": {"Work": true}},
        }]}),
    );

    let (mut fresh, _) = depot(&templates, Vec::new());
    fresh
        .import_marking(&marking)
        .expect("the depot rebuilds the cabin");
    let (token, _) = fresh.tokens(yard.id()).next().expect("the bus");
    let cabin = fresh.nested(yard.id(), token).unwrap()[0].1;
    assert_eq!(cabin.template(), Some("Queue"));
    assert_eq!(
        cabin.export_marking().unwrap(),
        net.nested(yard.id(), net.tokens(yard.id()).next().unwrap().0)
            .unwrap()[0]
            .1
            .export_marking()
            .unwrap()
    );

    // What the cabin did while it was being rebuilt is setup: running it now
    // lifts only the firing, and nothing stale.
    fresh.drain_events().for_each(drop);
    fresh
        .step_deep()
        .expect("works")
        .expect("the cabin has work");
    let events: Vec<Event> = fresh.drain_events().collect();
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(
        matches!(&events[0], Event::Nested { event, .. } if matches!(**event, Event::Fired { .. }))
    );
}

#[test]
fn a_json_command_rebuilds_a_nested_token_from_the_nets_templates() {
    let templates = queues();
    let (mut net, yard) = depot(&templates, Vec::new());
    net.apply_json(JsonCommand::Extend {
        place: "Yard".into(),
        tokens: vec![json!({
            "name": "Bus 2",
            "cabin": {"template": "Queue", "marking": {"Todo": [9]},
                      "params": {"speed": 2}, "enabled": {}},
        })],
    })
    .expect("injects");
    let (token, bus) = net.tokens(yard.id()).next().expect("the bus");
    let bus = bus.downcast_ref::<Bus>().unwrap();
    assert_eq!(bus.cabin.param::<Speed>(), Some(&Speed(2)));
    assert_eq!(net.count(yard.id()), 1);
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Added {
            place: yard.id(),
            token
        }],
        "the inner net's own setup is not lifted"
    );

    // A net with no templates of its own cannot rebuild one.
    let (mut bare, _) = depot(&Templates::new(), Vec::new());
    let error = bare
        .apply_json(JsonCommand::Extend {
            place: "Yard".into(),
            tokens: vec![json!({"name": "Bus 3",
                "cabin": {"template": "Queue", "marking": {}}})],
        })
        .expect_err("no Queue template here");
    assert!(
        matches!(&error, CommandError::InvalidToken { .. })
            && error.to_string().contains("no template named \"Queue\""),
        "{error}"
    );
}

/// Depth two: a cabin whose seats hold passengers carrying luggage, a net
/// each.
mod depth_two {
    use super::*;

    #[derive(Debug, Serialize, Deserialize)]
    pub(super) struct Passenger {
        pub(super) name: String,
        pub(super) luggage: Net,
    }

    impl Nested for Passenger {
        fn nets(&self) -> Vec<(&str, &Net)> {
            vec![("luggage", &self.luggage)]
        }
        fn nets_mut(&mut self) -> Vec<(&str, &mut Net)> {
            vec![("luggage", &mut self.luggage)]
        }
        fn fork(&self) -> Result<Self, NetError> {
            Ok(Passenger {
                name: self.name.clone(),
                luggage: self.luggage.fork()?,
            })
        }
    }

    pub(super) fn luggage(_: &Params) -> Result<Net, petritype::BuildError> {
        let mut b = NetBuilder::new();
        let items = b.place::<String>("Items");
        b.serializable(items);
        b.build()
    }

    /// A cabin with a nested place of passengers. With `own`, it carries the
    /// `Luggage` template itself; without, it finds it in an outer scope.
    pub(super) fn cabin(own: bool) -> impl Fn(&Params) -> Result<Net, petritype::BuildError> {
        move |_: &Params| {
            let mut b = NetBuilder::new();
            if own {
                b.template("Luggage", luggage);
            }
            let seats = b.place_nested::<Passenger>("Seats");
            b.serializable(seats);
            b.build()
        }
    }

    pub(super) fn registry(own: bool) -> Templates {
        let mut templates = Templates::new();
        templates
            .register("Cabin", cabin(own))
            .register("Luggage", luggage);
        templates
    }
}

#[test]
fn two_levels_down_restores_through_nested_scopes() {
    use depth_two::{Passenger, registry};

    for own in [true, false] {
        let templates = registry(own);
        let mut luggage = templates.build("Luggage", &Params::new()).unwrap();
        let items = luggage.place_ref::<String>("Items").unwrap();
        luggage.put(items, "umbrella".into());
        let mut cabin = templates.build("Cabin", &Params::new()).unwrap();
        let seats = cabin.place_ref::<Passenger>("Seats").unwrap();
        cabin.put(
            seats,
            Passenger {
                name: "Ada".into(),
                luggage,
            },
        );
        let (net, yard) = depot(
            &templates,
            vec![Bus {
                name: "Bus 1".into(),
                cabin,
            }],
        );

        let text = serde_json::to_string(&net.export_marking().unwrap()).unwrap();
        assert!(text.contains("umbrella"), "{text}");
        let (mut fresh, _) = depot(&templates, Vec::new());
        fresh
            .import_marking(&serde_json::from_str(&text).unwrap())
            .unwrap_or_else(|error| panic!("own = {own}: {error}"));
        assert_eq!(
            serde_json::to_string(&fresh.export_marking().unwrap()).unwrap(),
            text,
            "own = {own}"
        );

        // And a snapshot forks all the way down.
        let snapshot = net.snapshot().expect("forks two levels down");
        let (token, _) = net.tokens(yard.id()).next().unwrap();
        let copy = snapshot.nested(yard.id(), token).unwrap()[0].1;
        let live = net.nested(yard.id(), token).unwrap()[0].1;
        assert_eq!(
            copy.export_marking().unwrap(),
            live.export_marking().unwrap()
        );
    }
}

// ----- R6: fork and snapshots ------------------------------------------------------

#[test]
fn a_fork_is_a_new_net_with_the_same_state() {
    let mut net = queues().build("Queue", &Params::new()).unwrap();
    let todo = net.place_ref::<u64>("Todo").unwrap();
    net.extend(todo, [1, 2, 3]);
    net.step().unwrap();
    net.set_param(Speed(4)).unwrap();
    net.set_param(Note("kept".into())).unwrap();
    let work = net.transition_named("Work").unwrap();
    net.disable(work).unwrap();
    net.set_tie_break(TieBreak::LastDeclared);
    net.set_selector(|_, _| None);

    let mut fork = net.fork().expect("a stamped net forks");
    assert_ne!(fork.id(), net.id());
    assert_eq!(fork.template(), Some("Queue"));
    assert_eq!(
        fork.export_marking().unwrap(),
        net.export_marking().unwrap()
    );
    assert_eq!(fork.param::<Speed>(), Some(&Speed(4)));
    assert_eq!(
        fork.param::<Note>(),
        Some(&Note("kept".into())),
        "every parameter, wire name or not"
    );
    assert!(!fork.is_enabled(work));
    assert_eq!(fork.label_limit(), net.label_limit());
    assert_eq!(fork.state_counter(), 0);
    assert_eq!(fork.fired_count(), 0);
    assert_eq!(fork.drain_events().count(), 0);

    // The selector is a closure and stays behind: the fork fires.
    fork.enable(work).unwrap();
    assert!(fork.step().unwrap().is_some());
    assert_eq!(net.count(todo.id()), 2, "the original is untouched");
}

#[test]
fn a_fork_needs_a_template_and_nothing_in_flight() {
    let plain = NetBuilder::new().build().unwrap();
    assert!(matches!(plain.fork(), Err(NetError::NoTemplate)));

    let mut net = queues().build("Queue", &Params::new()).unwrap();
    let todo = net.place_ref::<u64>("Todo").unwrap();
    net.put(todo, 1);
    let work = net.transition_named("Work").unwrap();
    let firing = net.launch(work).unwrap();
    let error = net.fork().expect_err("Work is in flight");
    assert!(
        matches!(&error, NetError::InFlight { transition, .. } if *transition == work),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        format!("transition \"Work\" ({work:?}) is in flight")
    );
    assert!(net.to_json().is_err(), "nor can it be saved");
    net.distribute(firing.run()).unwrap();
    assert!(net.fork().is_ok());
}

#[test]
fn a_snapshot_forks_a_nested_place_and_reads_it_back() {
    let templates = queues();
    let (mut net, yard) = depot(
        &templates,
        vec![
            bus(&templates, "Bus 1", &[1]),
            bus(&templates, "Bus 2", &[2, 3]),
        ],
    );
    let snapshot = net.snapshot().expect("Bus forks");
    for (token, _) in net.tokens(yard.id()) {
        let live = net.nested(yard.id(), token).unwrap()[0].1;
        let copy = snapshot.nested(yard.id(), token).expect("same token id")[0].1;
        assert_eq!(
            copy.export_marking().unwrap(),
            live.export_marking().unwrap()
        );
        assert_ne!(copy.id(), live.id());
    }
    assert_eq!(
        snapshot
            .token::<Bus>(yard.id(), net.tokens(yard.id()).next().unwrap().0)
            .map(|bus| bus.name.as_str()),
        Some("Bus 1"),
    );
    // The snapshot is a moment: the live cabins move on without it.
    petritype::drivers::run(
        &mut net,
        Default::default(),
        Default::default(),
        Default::default(),
    )
    .unwrap();
    let (token, _) = net.tokens(yard.id()).next().unwrap();
    let copy = snapshot.nested(yard.id(), token).unwrap()[0].1;
    assert_eq!(copy.count(copy.place_named("Todo").unwrap()), 1);
    assert_eq!(
        snapshot.clone().nested(yard.id(), token).unwrap()[0].1.id(),
        copy.id(),
        "shared by a clone"
    );
}

#[test]
fn a_snapshot_names_the_place_and_the_token_it_could_not_fork() {
    let templates = queues();
    let mut b = NetBuilder::new();
    let ok = b.place_nested::<Bus>("Buses");
    let crates = b.place_nested::<Crate>("Crates");
    b.seed(ok, [bus(&templates, "Bus 1", &[])]);
    b.seed(
        crates,
        [Crate {
            cargo: templates.build("Queue", &Params::new()).unwrap(),
        }],
    );
    let net = b.build().unwrap();
    let (token, _) = net.tokens(crates.id()).next().unwrap();

    let error = net.snapshot().expect_err("Crate keeps the default fork");
    let NetError::CannotFork {
        place,
        name,
        token: which,
        source,
    } = &error
    else {
        panic!("{error:?}");
    };
    assert_eq!(
        (*place, name.as_str(), *which),
        (crates.id(), "Crates", token)
    );
    assert!(
        matches!(**source, NetError::NotForkable { type_name } if type_name.ends_with("Crate"))
    );
    assert_eq!(
        error.to_string(),
        format!(
            "place \"Crates\" ({:?}): token {token:?} cannot be forked: {} does not implement \
             Nested::fork, so it cannot be copied",
            crates.id(),
            std::any::type_name::<Crate>(),
        )
    );

    // A token whose inner net no template built cannot be forked either.
    let mut b = NetBuilder::new();
    let yard = b.place_nested::<Bus>("Yard");
    b.seed(
        yard,
        [Bus {
            name: "Bus 9".into(),
            cabin: NetBuilder::new().build().unwrap(),
        }],
    );
    let net = b.build().unwrap();
    assert!(matches!(
        net.snapshot(),
        Err(NetError::CannotFork { source, .. }) if matches!(*source, NetError::NoTemplate)
    ));
}

#[test]
fn snapshot_edges_and_restores_from_a_nested_place_stay_build_errors() {
    /// A token that could be cloned and could be forked: the place still
    /// records no cloner, so a snapshot edge, which would have to fork on
    /// the launch path where nothing may fail, is still refused.
    #[derive(Debug, Clone)]
    struct Hollow;
    impl Nested for Hollow {
        fn nets(&self) -> Vec<(&str, &Net)> {
            Vec::new()
        }
        fn nets_mut(&mut self) -> Vec<(&str, &mut Net)> {
            Vec::new()
        }
        fn fork(&self) -> Result<Self, NetError> {
            Ok(Hollow)
        }
    }

    let mut b = NetBuilder::new();
    let shells = b.place_nested::<Hollow>("Shells");
    let out = b.place::<u32>("Out");
    b.transition(
        "Peek",
        (petritype::SnapshotEdge(shells),),
        (out,),
        |_shell: Hollow| (1_u32,),
    );
    assert!(matches!(
        b.build(),
        Err(petritype::BuildError::SnapshotOfNoClonePlace { .. })
    ));

    let mut b = NetBuilder::new();
    let yard = b.place_nested::<Bus>("Yard");
    b.transition("Scrap", (TakeEdge(yard),), (), |_bus: Bus| {})
        .restore_tokens_on_failure();
    assert!(matches!(
        b.build(),
        Err(petritype::BuildError::RestoreOfNoClonePlace { .. })
    ));

    // A mutating edge is fine, as ever.
    let mut b = NetBuilder::new();
    let yard = b.place_nested::<Bus>("Yard");
    b.transition("Touch", (MutateEdge(yard),), (), |_bus: &mut Bus| {});
    assert!(b.build().is_ok());
}

#[test]
fn a_net_is_its_own_fork_as_a_nested_token() {
    let templates = queues();
    let mut b = NetBuilder::new();
    let shelf = b.place_nested::<Net>("Shelf");
    b.seed(shelf, [templates.build("Queue", &Params::new()).unwrap()]);
    let net = b.build().unwrap();
    let snapshot = net.snapshot().expect("Net::fork");
    let (token, _) = net.tokens(shelf.id()).next().unwrap();
    assert_eq!(snapshot.nested(shelf.id(), token).unwrap()[0].0, "");
}
