//! Integration tests for closed nets (DESIGN §6.9): every net takes tokens
//! from outside it only at its ports, unless it declares itself open, and a
//! body that holds a net is refused its knobs and firing by name. Stage one
//! (2026-11-23) closed only a net that marked a port; stage two
//! (2026-11-28) closes every net, refuses a body every closed net inside
//! the one it holds, and shields the nets inside a closed net from the
//! program. A body held to its edge's `reaches` is in `tests/reaches.rs`.
//!
//! One test, or a few, per line of the rule: who is outside (a body by each
//! edge kind that holds a token's net, the program by each method,
//! `nested_scope`, `subnet_scope`, an erased `Command`), a net two levels
//! down, an open net, a net with no port, the nets inside a net a body
//! holds, the nets inside a closed net, and each thing that stays allowed
//! (reading, stepping, the whole-state acts, the program's knobs and firing
//! by name, a net's own control, its seeds and its own firings). The
//! Python cases Rust cannot express are those of a net reached outside the
//! scopes: a net put into a place's list directly, one held in flight by a
//! body, one whose outer net is gone, a subnet swapped for another, and a
//! copy made by `deepcopy` or `pickle`. The runner's `inject` is tested in
//! `petritype-runner`.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use petritype::control::{Control, Pressed, Value};
use petritype::drivers::{self, DriverError, Limits, Mode, OnFailure};
use petritype::{
    ClosedRefusal, Command, CommandError, Completed, DeepStep, DrainEdge, Event, Filter,
    FiringFailed, KeyValue, MutateEdge, MutateWhereEdge, Nested, Net, NetBuilder, NetError, NetId,
    PlaceId, PlaceRef, ReadEdge, StagedStep, StepError, Stepping, TakeAllEdge, TakeEdge,
    TakeWhereEdge, TieBreak, TransitionId,
};

// ----- the nets ----------------------------------------------------------------

/// A parameter of a unit, for the knobs.
#[derive(Debug, Clone, PartialEq)]
struct Speed(u32);

/// A unit, a net a token carries: `Orders In`, a port unless `port` is
/// false, and `Notes` and `Bay` (a place of nets, for a stepping rule),
/// which are not; a `Speed` parameter, a transition `Fire` that takes an
/// order, and a control `noop`. Declared open when `open` is.
fn unit_with(port: bool, open: bool) -> Net {
    unit_seeded(port, open, 0)
}

/// [`unit_with`], started with `seeded` orders in `Orders In`, which are
/// inside: a unit that marks no port can still fire by name.
fn unit_seeded(port: bool, open: bool, seeded: usize) -> Net {
    let mut b = NetBuilder::new();
    let orders = b.place::<String>("Orders In");
    b.place::<String>("Notes");
    b.place_nested::<Net>("Bay");
    if port {
        b.port(orders);
    }
    if open {
        b.open();
    }
    b.seed(orders, (0..seeded).map(|n| format!("Order {n}")));
    b.param(Speed(1));
    b.transition("Fire", (TakeEdge(orders),), (), |_: String| {});
    b.control(Control::button("noop", "No-op", |_, _| {
        Ok(Pressed::Applied)
    }));
    b.build().expect("a unit")
}

/// A closed unit: it marks `Orders In` a port.
fn unit() -> Net {
    unit_with(true, false)
}

fn place(net: &Net, name: &str) -> PlaceRef<String> {
    net.place_ref::<String>(name).expect("a place of strings")
}

fn strings(net: &Net, name: &str) -> Vec<String> {
    let id = net.place_named(name).expect("a place");
    net.tokens(id)
        .filter_map(|(token, _)| net.token::<String>(id, token).cloned())
        .collect()
}

/// An order through the port: lands on a closed net.
fn order(unit: &mut Net) {
    let orders = place(unit, "Orders In");
    unit.put(orders, "Advance".to_owned());
}

/// An order, then a note in `Notes`, which is not a port: the second is
/// refused on a closed net, and panics, since `put` has no `Result`.
fn order_then_note(unit: &mut Net) {
    order(unit);
    let notes = place(unit, "Notes");
    unit.put(notes, "Forged".to_owned());
}

/// A net around a unit: `Field`, a place of nets keyed by `name` (every
/// unit is `"unit"`), holding `unit`; `declare` adds its transitions.
fn field_of(unit: Net, declare: impl FnOnce(&mut NetBuilder, PlaceRef<Net>)) -> Net {
    let mut b = NetBuilder::new();
    let field = b.place_nested::<Net>("Field");
    b.keys(field, ["name"], |_: &Net| [KeyValue::from("unit")]);
    b.seed(field, [unit]);
    declare(&mut b, field);
    b.build().expect("a field")
}

/// The filter of the keyed edges below: the unit called `"unit"`.
fn the_unit() -> Filter {
    Filter::new().key_in("name", ["unit"])
}

/// The error a failed firing failed with, as a [`NetError`].
fn refusal(failed: &FiringFailed) -> &NetError {
    failed
        .error
        .downcast_ref::<NetError>()
        .unwrap_or_else(|| panic!("a NetError, not {:?}", failed.error))
}

/// The front unit of `Field`, to read.
fn front(net: &Net) -> &Net {
    let field = net.place_named("Field").expect("a field");
    let (token, _) = net.tokens(field).next().expect("a unit in the field");
    net.token::<Net>(field, token).expect("a net")
}

/// Calls `f` on the front unit of `Field` as the program, through
/// `nested_scope`.
fn with_front<R>(net: &mut Net, f: impl FnOnce(&mut Net) -> R) -> R {
    let field = net.place_named("Field").expect("a field");
    let (token, _) = net.tokens(field).next().expect("a unit in the field");
    net.nested_scope(field, token, |nets| f(nets[0].1))
        .expect("a nested place")
}

// ----- which nets are closed -------------------------------------------------

#[test]
fn every_net_is_closed_unless_it_declares_itself_open() {
    let closed = unit();
    assert!(closed.is_closed());
    assert!(!closed.is_open(), "it declares nothing");

    let plain = unit_with(false, false);
    assert!(plain.is_closed(), "no port, and closed all the same");
    assert!(!plain.is_open(), "it declares nothing");

    let declared = unit_with(true, true);
    assert!(declared.is_open(), "the declaration");
    assert!(!declared.is_closed(), "and the rule's answer");

    let both = unit_with(false, true);
    assert!(both.is_open() && !both.is_closed());
}

// ----- the program, by each method -------------------------------------------

#[test]
fn the_program_writes_at_a_port_of_a_closed_net() {
    let mut net = unit();
    let orders = place(&net, "Orders In");
    let first = net.put(orders, "Advance".to_owned());
    net.extend(orders, ["Hold".to_owned()]);
    net.try_put(orders, "Wait".to_owned()).expect("a port");
    net.try_extend(orders, ["Turn".to_owned()]).expect("a port");
    net.replace(orders, first, "Charge".to_owned())
        .expect("a port");
    let (second, _) = net.tokens(orders.id()).nth(1).expect("Hold");
    net.remove(orders.id(), second).expect("a port");
    assert_eq!(strings(&net, "Orders In"), ["Charge", "Wait", "Turn"]);
    net.try_set_tokens(orders, ["Rest".to_owned()])
        .expect("a port");
    net.set_tokens(orders, Vec::<String>::new());
    assert_eq!(net.count(orders.id()), 0);
}

#[test]
fn every_fallible_method_refuses_a_place_that_is_not_a_port_and_changes_nothing() {
    let mut b = NetBuilder::new();
    let orders = b.place::<String>("Orders In");
    let notes = b.place::<String>("Notes");
    b.port(orders);
    b.seed(notes, ["Seeded".to_owned()]);
    let mut net = b.build().expect("a closed net");
    let (seeded, _) = net.tokens(notes.id()).next().expect("the seed is inside");

    let closed = |error: &NetError, act: &str| {
        assert!(
            matches!(error, NetError::ClosedNet { place, act: said, .. }
                if place.as_deref() == Some("Notes") && *said == act),
            "{error:?}"
        );
    };
    closed(
        &net.try_put(notes, "x".to_owned()).unwrap_err(),
        "put a token into",
    );
    closed(
        &net.try_extend(notes, ["x".to_owned()]).unwrap_err(),
        "add tokens to",
    );
    closed(
        &net.try_set_tokens(notes, Vec::<String>::new()).unwrap_err(),
        "set the tokens of",
    );
    closed(
        &net.remove(notes.id(), seeded).unwrap_err(),
        "remove a token from",
    );
    closed(
        &net.replace(notes, seeded, "x".to_owned()).unwrap_err(),
        "replace a token in",
    );
    assert_eq!(strings(&net, "Notes"), ["Seeded"], "untouched");
    assert_eq!(net.drain_events().count(), 0, "and silent");
    assert_eq!(net.state_counter(), 0);
}

#[test]
fn the_methods_with_no_result_panic_with_the_refusal_s_message() {
    let mut net = unit();
    let notes = place(&net, "Notes");
    for (what, write) in [
        (
            "put",
            Box::new(|net: &mut Net| {
                net.put(notes, "x".to_owned());
            }) as Box<dyn Fn(&mut Net)>,
        ),
        (
            "extend",
            Box::new(|net: &mut Net| {
                net.extend(notes, ["x".to_owned()]);
            }),
        ),
        (
            "set_tokens",
            Box::new(|net: &mut Net| {
                net.set_tokens(notes, ["x".to_owned()]);
            }),
        ),
    ] {
        let payload = catch_unwind(AssertUnwindSafe(|| write(&mut net))).expect_err(what);
        let message = payload
            .downcast_ref::<String>()
            .unwrap_or_else(|| panic!("{what}: a message"));
        assert!(
            message.contains("\"Notes\"") && message.contains("declare the net open"),
            "{what}: {message}"
        );
    }
    assert_eq!(net.count(notes.id()), 0, "nothing landed, nothing cleared");
}

#[test]
fn the_refusal_names_the_net_the_place_and_the_act_and_says_how_to_put_it_right() {
    let mut net = unit();
    let notes = place(&net, "Notes");
    let error = net.try_put(notes, "x".to_owned()).unwrap_err();
    let id = format!("{:?}", net.id());
    assert_eq!(
        error.to_string(),
        format!(
            "cannot put a token into \"Notes\" from outside net {id:?}, which is closed: \
             from outside it, tokens are written only at its ports, and \"Notes\" is not one. \
             Mark the place as a port, start the net with the token in its definition, or \
             declare the net open"
        )
    );
}

#[test]
fn an_erased_command_is_refused_at_a_place_that_is_not_a_port() {
    let mut net = unit();
    let orders = place(&net, "Orders In");
    let notes = place(&net, "Notes");
    net.apply(Command::put(orders, "Advance".to_owned()))
        .expect("a port");
    let refused = [
        net.apply(Command::put(notes, "x".to_owned())),
        net.apply(Command::set_tokens(notes, Vec::<String>::new())),
    ];
    for result in refused {
        assert!(
            matches!(result, Err(CommandError::Net(NetError::ClosedNet { .. }))),
            "{result:?}"
        );
    }
    // A knob through a command is the program's.
    let fire = net.transition_named("Fire").expect("Fire");
    net.apply(Command::disable(fire)).expect("a knob");
    net.apply(Command::set_param(Speed(3))).expect("a knob");
    assert_eq!(net.param::<Speed>(), Some(&Speed(3)));
    assert_eq!(net.count(notes.id()), 0);
}

#[test]
fn an_erased_command_s_place_and_type_are_checked_before_the_rule() {
    // As the runner answers them: `unknown_place`, `invalid_token`, then
    // `closed_net`.
    let mut net = unit();
    let notes = place(&net, "Notes").id();
    let tokens: Vec<Box<dyn std::any::Any + Send + Sync>> = vec![Box::new(7_u32)];
    let mistyped = net.apply(Command::Extend {
        place: notes,
        tokens,
    });
    assert!(
        matches!(
            mistyped,
            Err(CommandError::Net(NetError::TypeMismatch { .. }))
        ),
        "{mistyped:?}"
    );
    let (token, _) = {
        let orders = place(&net, "Orders In");
        net.put(orders, "Advance".to_owned());
        net.tokens(orders.id()).next().unwrap()
    };
    let mistyped = net.apply(Command::Replace {
        place: notes,
        token,
        value: Box::new(7_u32),
    });
    assert!(
        matches!(
            mistyped,
            Err(CommandError::Net(NetError::TypeMismatch { .. }))
        ),
        "{mistyped:?}"
    );
    let typed = net.apply(Command::put(place(&net, "Notes"), "x".to_owned()));
    assert!(
        matches!(typed, Err(CommandError::Net(NetError::ClosedNet { .. }))),
        "{typed:?}"
    );
}

#[cfg(feature = "serde")]
#[test]
fn a_json_command_is_refused_at_a_place_that_is_not_a_port() {
    use petritype::JsonCommand;
    use petritype::serde_json::json;

    let mut b = NetBuilder::new();
    let orders = b.place::<String>("Orders In");
    let notes = b.place::<String>("Notes");
    b.serializable(orders);
    b.serializable(notes);
    b.port(orders);
    let mut net = b.build().expect("a closed net");
    let extend = |place: &str| JsonCommand::Extend {
        place: place.to_owned(),
        tokens: vec![json!("x")],
    };
    net.apply_json(extend("Orders In")).expect("a port");
    let refused = net.apply_json(extend("Notes")).unwrap_err();
    assert!(matches!(
        refused,
        CommandError::Net(NetError::ClosedNet { .. })
    ));
    assert_eq!(net.count(notes.id()), 0);
}

#[test]
fn a_net_that_marks_no_port_takes_no_token_from_outside_at_any_place() {
    let mut net = unit_with(false, false);
    let orders = place(&net, "Orders In");
    let notes = place(&net, "Notes");
    let bay = net.place_ref::<Net>("Bay").expect("a place of nets");
    let closed = |error: NetError, at: &str| {
        assert!(
            matches!(&error, NetError::ClosedNet { place, .. } if place.as_deref() == Some(at)),
            "{at}: {error:?}"
        );
    };
    closed(
        net.try_put(orders, "x".to_owned()).unwrap_err(),
        "Orders In",
    );
    closed(net.try_put(notes, "x".to_owned()).unwrap_err(), "Notes");
    closed(net.try_put(bay, unit()).unwrap_err(), "Bay");
    for (name, command) in [
        ("Orders In", Command::put(orders, "x".to_owned())),
        ("Notes", Command::put(notes, "x".to_owned())),
        ("Bay", Command::put(bay, unit())),
    ] {
        match net.apply(command) {
            Err(CommandError::Net(error)) => closed(error, name),
            other => panic!("{name}: {other:?}"),
        }
    }
    let payload = catch_unwind(AssertUnwindSafe(|| net.put(orders, "x".to_owned())))
        .expect_err("put has no Result, so it panics");
    let message = payload.downcast_ref::<String>().expect("a message");
    assert!(message.contains("\"Orders In\""), "{message}");
    for (place, _) in net.places() {
        assert_eq!(net.count(place), 0, "nothing landed");
    }
    assert_eq!(net.drain_events().count(), 0, "and nothing was said");
}

#[test]
fn an_open_net_takes_tokens_anywhere_with_or_without_ports() {
    for mut net in [unit_with(false, true), unit_with(true, true)] {
        let bay = net.place_ref::<Net>("Bay").expect("a place of nets");
        net.try_put(bay, unit()).expect("open");
        let orders = place(&net, "Orders In");
        net.try_put(orders, "x".to_owned()).expect("open");
        let notes = place(&net, "Notes");
        let token = net.put(notes, "a".to_owned());
        net.extend(notes, ["b".to_owned()]);
        net.replace(notes, token, "c".to_owned()).expect("open");
        net.apply(Command::put(notes, "d".to_owned()))
            .expect("open");
        assert_eq!(strings(&net, "Notes"), ["c", "b", "d"]);
        net.set_tokens(notes, Vec::<String>::new());
        assert_eq!(net.count(notes.id()), 0);
    }
}

// ----- inside: seeds, its own firings, its own controls ----------------------

#[test]
fn a_closed_net_s_seeds_and_its_own_firings_are_inside() {
    let mut b = NetBuilder::new();
    let orders = b.place::<String>("Orders In");
    let notes = b.place::<String>("Notes");
    b.port(orders);
    b.seed(notes, ["Seeded".to_owned()]);
    b.transition("File", (TakeEdge(orders),), (notes,), |order: String| {
        (order,)
    });
    let mut net = b.build().expect("a closed net");
    net.put(orders, "Advance".to_owned());
    net.step().expect("its own firing").expect("File");
    assert_eq!(strings(&net, "Notes"), ["Seeded", "Advance"]);
}

#[test]
fn a_closed_net_s_own_control_writes_where_it_likes_and_only_while_it_runs() {
    let mut b = NetBuilder::new();
    let orders = b.place::<String>("Orders In");
    let notes = b.place::<String>("Notes");
    b.port(orders);
    b.control(Control::inject("note", "+ Note", notes, |_, _| {
        "Pressed".to_owned()
    }));
    b.control(Control::button("break", "Break", move |net, _| {
        net.put(notes, "Before".to_owned());
        panic!("the handler broke");
    }));
    let mut net = b.build().expect("a closed net");

    assert_eq!(net.press("note", Value::None), Ok(Pressed::Applied));
    assert_eq!(strings(&net, "Notes"), ["Pressed"]);
    assert!(
        net.try_put(notes, "x".to_owned()).is_err(),
        "outside again once the handler returns"
    );

    let broke = catch_unwind(AssertUnwindSafe(|| net.press("break", Value::None)));
    assert!(broke.is_err(), "the handler's panic is its own");
    assert_eq!(strings(&net, "Notes"), ["Pressed", "Before"]);
    assert!(
        net.try_put(notes, "x".to_owned()).is_err(),
        "outside again once the handler has panicked"
    );
}

// ----- the program keeps its knobs, firing by name, reading and stepping -----

#[test]
fn the_program_keeps_every_knob_and_firing_by_name_on_a_closed_net() {
    let mut net = unit();
    let fire = net.transition_named("Fire").expect("Fire");
    let bay = net.place_named("Bay").expect("Bay");
    net.set_param(Speed(2)).expect("a parameter");
    net.disable(fire).expect("a switch");
    net.enable(fire).expect("a switch");
    net.set_tie_break(TieBreak::RoundRobin);
    net.set_stepping(bay, Stepping::Passive)
        .expect("a stepping rule");
    net.set_selector(|_, fireable| fireable.first().copied());
    net.clear_selector();
    net.try_set_tie_break(TieBreak::LastDeclared)
        .expect("a tie-break");
    assert_eq!(net.tie_break(), TieBreak::LastDeclared);
    net.try_set_selector(|_, _| None).expect("a selector");
    order(&mut net);
    assert_eq!(net.step().expect("the selector declines"), None);
    net.try_clear_selector().expect("the default back");
    assert_eq!(net.press("noop", Value::None), Ok(Pressed::Applied));
    net.fire(fire).expect("firing by name");
    order(&mut net);
    let firing = net.launch(fire).expect("launching by name");
    net.distribute(firing.run()).expect("and distributing");
    assert_eq!(net.step_inside().expect("nothing inside"), None);
    assert_eq!(net.fired_count(), 2);
}

#[test]
fn reading_and_stepping_a_closed_net_are_allowed() {
    let mut b = NetBuilder::new();
    let orders = b.place::<String>("Orders In");
    let notes = b.place::<String>("Notes");
    b.port(orders);
    b.seed(notes, ["Private".to_owned()]);
    b.transition("File", (TakeEdge(orders),), (notes,), |order: String| {
        (order,)
    })
    .guard(move |net| net.count(notes.id()) < 3);
    let mut net = b.build().expect("a closed net");
    let (token, _) = net.tokens(notes.id()).next().expect("a note");
    assert_eq!(net.label(notes.id(), token).as_deref(), Some("\"Private\""));
    assert_eq!(net.count(notes.id()), 1);
    net.put(orders, "a".to_owned());
    assert_eq!(
        net.step_deep().expect("a step").map(|s| s.transition()),
        net.transition_named("File")
    );
    assert_eq!(net.count(notes.id()), 2);
}

#[cfg(feature = "serde")]
#[test]
fn the_whole_state_acts_stay_allowed_for_the_program() {
    use petritype::serde_json::{self, json};
    use petritype::{Marking, Params, Templates};

    // A post that marks its inbox a port, and one that marks no port.
    for (template, port) in [("Post", true), ("Sealed Post", false)] {
        let mut templates = Templates::new();
        templates.register(template, move |_: &Params| {
            let mut b = NetBuilder::new();
            let inbox = b.place::<String>("Inbox");
            let ledger = b.place::<String>("Ledger");
            b.serializable(inbox);
            b.serializable(ledger);
            if port {
                b.port(inbox);
            }
            b.build()
        });
        let mut post = templates.build(template, &Params::new()).expect("a post");
        assert!(post.is_closed());

        let marking: Marking =
            serde_json::from_value(json!({"Inbox": ["Dear Ada"], "Ledger": ["IOU"]})).unwrap();
        post.import_marking(&marking)
            .expect("an import replaces the whole state");
        assert_eq!(strings(&post, "Ledger"), ["IOU"]);

        let fork = post.fork().expect("a fork");
        assert_eq!(strings(&fork, "Ledger"), ["IOU"]);
        assert!(fork.is_closed(), "the definition goes with it");

        let restored = templates
            .restore(post.to_json().expect("a save"))
            .expect("a restore");
        assert_eq!(strings(&restored, "Ledger"), ["IOU"]);
        assert_eq!(post.export_marking().expect("a save"), marking);
    }
}

#[cfg(feature = "serde")]
#[test]
fn a_fork_and_a_restored_net_keep_the_declaration_open() {
    use petritype::{Params, Templates};

    let mut templates = Templates::new();
    templates.register("Open Post", |_: &Params| {
        let mut b = NetBuilder::new();
        let inbox = b.place::<String>("Inbox");
        b.serializable(inbox);
        b.port(inbox);
        b.open();
        b.build()
    });
    let post = templates
        .build("Open Post", &Params::new())
        .expect("a post");
    assert!(post.fork().expect("a fork").is_open());
    let restored = templates
        .restore(post.to_json().expect("a save"))
        .expect("a restore");
    assert!(restored.is_open());
    let saved = post.to_json().expect("a save");
    assert!(
        saved.get("open").is_none(),
        "not in the saved form: {saved}"
    );
}

// ----- nested_scope and subnet_scope -----------------------------------------

#[test]
fn nested_scope_lends_a_token_s_net_under_its_own_rule() {
    // The field is declared open: a closed one would shield the unit.
    let mut net = field_of(unit(), |b, _| {
        b.open();
    });
    with_front(&mut net, order);
    let refused = with_front(&mut net, |unit| {
        let notes = place(unit, "Notes");
        unit.try_put(notes, "x".to_owned())
    });
    assert!(matches!(refused, Err(NetError::ClosedNet { .. })));
    // The program's knobs, through the scope.
    with_front(&mut net, |unit| unit.set_param(Speed(4))).expect("the program's");
    assert_eq!(strings(front(&net), "Orders In"), ["Advance"]);
    assert_eq!(front(&net).param::<Speed>(), Some(&Speed(4)));
}

/// A level: `Lobby` its port and `Arena`, which is not, each of strings,
/// and a `Speed` parameter.
fn level() -> Net {
    let mut b = NetBuilder::new();
    let lobby = b.place::<String>("Lobby");
    b.place::<String>("Arena");
    b.port(lobby);
    b.param(Speed(1));
    b.build().expect("a level")
}

#[test]
fn subnet_scope_lends_a_subnet_under_its_own_rule_and_keeps_the_program_s_knobs() {
    let mut b = NetBuilder::new();
    let level_place = b.subnet_place("Level", level());
    // Declared open: a closed world would shield its level.
    b.open();
    let mut world = b.build().expect("a world");
    let arena = world.subnet_scope(level_place.id(), |level| {
        let lobby = place(level, "Lobby");
        level.put(lobby, "Ada".to_owned());
        level.set_param(Speed(5)).expect("the program's");
        let arena = place(level, "Arena");
        level.try_put(arena, "Bram".to_owned())
    });
    assert!(matches!(arena, Ok(Err(NetError::ClosedNet { .. }))));
    let level = world.subnet(level_place.id()).expect("the subnet");
    assert_eq!(strings(level, "Lobby"), ["Ada"]);
    assert_eq!(strings(level, "Arena"), Vec::<String>::new());
    assert_eq!(level.param::<Speed>(), Some(&Speed(5)));
}

// ----- windows ---------------------------------------------------------------

#[test]
fn a_write_at_a_window_is_judged_by_the_holding_net_first() {
    // Open: the world declares itself open, and the level's lobby takes
    // the token.
    let mut b = NetBuilder::new();
    let level_place = b.subnet_place("Level", level());
    let door = b.port_of::<String>(level_place, "Lobby");
    b.open();
    let mut world = b.build().expect("an open world");
    world.put(door, "Ada".to_owned());
    assert_eq!(world.count(door.id()), 1);

    // Closed, with no port of its own: the window refuses.
    let mut b = NetBuilder::new();
    let level_place = b.subnet_place("Level", level());
    let door = b.port_of::<String>(level_place, "Lobby");
    let mut world = b.build().expect("a closed world");
    assert!(world.try_put(door, "Ada".to_owned()).is_err());
    assert_eq!(world.count(door.id()), 0);

    // Closed, with a port of its own: its window, which can never be one,
    // refuses a write from outside. Its seed and its own firing's output at
    // the window are inside, and land.
    let mut b = NetBuilder::new();
    let gate = b.place::<String>("Gate");
    let level_place = b.subnet_place("Level", level());
    let door = b.port_of::<String>(level_place, "Lobby");
    b.port(gate);
    b.seed(door, ["Seeded".to_owned()]);
    b.transition("Enter", (TakeEdge(gate),), (door,), |hero: String| (hero,));
    let mut world = b.build().expect("a closed world");
    assert!(world.is_closed());
    let refused = world.try_put(door, "Ada".to_owned()).unwrap_err();
    assert!(
        matches!(&refused, NetError::ClosedNet { place, .. } if place.as_deref() == Some("Lobby")),
        "{refused:?}"
    );
    world.put(gate, "Bram".to_owned());
    world.step().expect("Enter").expect("fired");
    assert_eq!(
        strings(world.subnet(level_place.id()).unwrap(), "Lobby"),
        ["Seeded", "Bram"]
    );
}

// ----- a net two levels down -------------------------------------------------

#[test]
fn a_net_two_levels_down_is_under_its_own_rule_whoever_reaches_it() {
    // The field and the middle net are declared open, so that nothing
    // around shields the unit; the unit inside them is closed.
    let mut middle = NetBuilder::new();
    let bay = middle.place_nested::<Net>("Bay");
    middle.seed(bay, [unit()]);
    middle.open();
    let middle = middle.build().expect("a middle net");
    let mut net = field_of(middle, |b, _| {
        b.open();
    });
    let refused = with_front(&mut net, |middle| {
        let bay = middle.place_named("Bay").expect("Bay");
        let (token, _) = middle.tokens(bay).next().expect("a unit");
        middle
            .nested_scope(bay, token, |nets| {
                order(nets[0].1);
                let notes = place(nets[0].1, "Notes");
                nets[0].1.try_put(notes, "x".to_owned())
            })
            .expect("a nested place")
    });
    assert!(matches!(refused, Err(NetError::ClosedNet { .. })));
}

// ----- a body, by each edge kind that holds a token's net --------------------

/// Fires the field's own transition once and gives back how it went.
fn fire_once(net: &mut Net) -> Result<Option<TransitionId>, StepError> {
    net.step()
}

/// Asserts that a firing failed with a [`NetError::ClosedNet`] at `Notes`.
fn failed_closed(result: Result<Option<TransitionId>, StepError>) -> FiringFailed {
    let failed = result.expect_err("refused").into_firing().unwrap();
    assert!(
        matches!(refusal(&failed), NetError::ClosedNet { place, .. } if place.as_deref() == Some("Notes")),
        "{failed}"
    );
    failed
}

#[test]
fn a_mutate_edge_body_writes_at_a_port_and_fails_at_a_private_place() {
    for (write, lands) in [(order as fn(&mut Net), true), (order_then_note, false)] {
        let mut net = field_of(unit(), |b, field| {
            b.transition(
                "Reach",
                (MutateEdge(field).reaching(["Orders In"]),),
                (),
                move |unit: &mut Net| write(unit),
            );
        });
        let result = fire_once(&mut net);
        if lands {
            result.expect("at a port");
        } else {
            failed_closed(result);
        }
        // What the body did before the refusal stays done; the refused call
        // changed nothing.
        assert_eq!(strings(front(&net), "Orders In"), ["Advance"]);
        assert_eq!(strings(front(&net), "Notes"), Vec::<String>::new());
    }
}

#[test]
fn a_mutate_where_edge_body_is_held_to_the_ports() {
    for (write, lands) in [(order as fn(&mut Net), true), (order_then_note, false)] {
        let mut net = field_of(unit(), |b, field| {
            b.transition(
                "Reach",
                (MutateWhereEdge(field, the_unit()).reaching(["Orders In"]),),
                (),
                move |unit: &mut Net| write(unit),
            );
        });
        let result = fire_once(&mut net);
        if lands {
            result.expect("at a port");
        } else {
            failed_closed(result);
        }
        assert_eq!(strings(front(&net), "Notes"), Vec::<String>::new());
    }
}

#[test]
fn a_take_edge_body_owns_the_same_net_under_the_same_rule() {
    for (write, lands) in [(order as fn(&mut Net), true), (order_then_note, false)] {
        let mut net = field_of(unit(), |b, field| {
            b.transition(
                "Reach",
                (TakeEdge(field).reaching(["Orders In"]),),
                (field,),
                move |mut unit: Net| {
                    write(&mut unit);
                    (unit,)
                },
            );
        });
        let result = fire_once(&mut net);
        let field = net.place_named("Field").expect("Field");
        if lands {
            result.expect("at a port");
            assert_eq!(strings(front(&net), "Orders In"), ["Advance"]);
        } else {
            let failed = failed_closed(result);
            // A place of nets cannot restore, so the unit went with the
            // failure, as in any failure.
            assert!(!failed.restored);
            assert_eq!(net.count(field), 0);
        }
    }
}

#[test]
fn a_take_where_edge_body_is_held_to_the_ports() {
    for (write, lands) in [(order as fn(&mut Net), true), (order_then_note, false)] {
        let mut net = field_of(unit(), |b, field| {
            b.transition(
                "Reach",
                (TakeWhereEdge(field, the_unit()).reaching(["Orders In"]),),
                (field,),
                move |mut unit: Net| {
                    write(&mut unit);
                    (unit,)
                },
            );
        });
        let result = fire_once(&mut net);
        if lands {
            result.expect("at a port");
        } else {
            failed_closed(result);
        }
    }
}

#[test]
fn a_take_all_edge_and_a_drain_edge_body_are_held_to_the_ports() {
    for drain in [false, true] {
        for (write, lands) in [(order as fn(&mut Net), true), (order_then_note, false)] {
            let body = move |mut units: Vec<Net>| {
                units.iter_mut().for_each(write);
                (units,)
            };
            let mut net = field_of(unit(), |b, field| {
                if drain {
                    let edge = DrainEdge(field).reaching(["Orders In"]);
                    b.transition("Reach", (edge,), (field,), body);
                } else {
                    let edge = TakeAllEdge(field).reaching(["Orders In"]);
                    b.transition("Reach", (edge,), (field,), body);
                }
            });
            let result = fire_once(&mut net);
            if lands {
                result.expect("at a port");
                assert_eq!(strings(front(&net), "Orders In"), ["Advance"]);
            } else {
                failed_closed(result);
            }
        }
    }
}

#[test]
fn a_read_edge_body_reads_a_closed_net_s_private_places() {
    let mut b = NetBuilder::new();
    let orders = b.place::<String>("Orders In");
    let notes = b.place::<String>("Notes");
    b.port(orders);
    b.seed(notes, ["Private".to_owned()]);
    let the_unit = b.build().expect("a unit");
    let mut b = NetBuilder::new();
    let field = b.place_nested::<Net>("Field");
    let read = b.place::<usize>("Read");
    b.seed(field, [the_unit]);
    // A read edge lends `&Net`: there is nothing it could write with.
    b.transition("Look", (ReadEdge(field),), (read,), |unit: &Net| {
        (strings(unit, "Notes").len(),)
    })
    .guard(move |net| net.count(read.id()) == 0);
    let mut net = b.build().expect("a field");
    net.step().expect("reading is allowed").expect("Look");
    assert_eq!(net.count(read.id()), 1);
}

#[test]
fn a_body_that_drops_the_refusal_or_catches_its_panic_fails_all_the_same() {
    let dropped = |unit: &mut Net| {
        let notes = place(unit, "Notes");
        let _ = unit.try_put(notes, "x".to_owned());
        let _ = unit.remove(notes.id(), token_id(0));
    };
    let caught = |unit: &mut Net| {
        let _ = catch_unwind(AssertUnwindSafe(|| order_then_note(unit)));
    };
    for write in [dropped as fn(&mut Net), caught] {
        let mut net = field_of(unit(), |b, field| {
            b.transition(
                "Reach",
                (MutateEdge(field).reaching(["Orders In"]),),
                (),
                move |unit: &mut Net| write(unit),
            );
        });
        let failed = failed_closed(fire_once(&mut net));
        assert!(
            matches!(
                refusal(&failed),
                NetError::ClosedNet {
                    act: "put a token into",
                    ..
                }
            ),
            "the first refusal: {failed}"
        );
    }
}

/// A token id, for a removal that is refused before the token is looked
/// for. `TokenId` has no public constructor, so it is read off a net.
fn token_id(n: u64) -> petritype::TokenId {
    let mut b = NetBuilder::new();
    let place = b.place::<u64>("P");
    b.port(place);
    let mut net = b.build().expect("a net");
    (0..=n).map(|i| net.put(place, i)).last().expect("one")
}

#[test]
fn a_failed_firing_restores_what_any_failure_restores_and_logs_the_refusal() {
    let mut net = {
        let mut b = NetBuilder::new();
        let field = b.place_nested::<Net>("Field");
        let letters = b.place::<String>("Letters");
        b.seed(field, [unit()]);
        b.seed(letters, ["Dear Ada".to_owned()]);
        b.transition(
            "Forge",
            (TakeEdge(letters), MutateEdge(field)),
            (),
            |letter: String, unit: &mut Net| {
                let notes = place(unit, "Notes");
                unit.put(notes, letter);
            },
        )
        .restore_tokens_on_failure();
        b.build().expect("a field")
    };
    let letters = net.place_named("Letters").expect("Letters");
    let (letter, _) = net.tokens(letters).next().expect("a letter");
    let failed = failed_closed(net.step());
    assert!(failed.restored);
    assert_eq!(
        net.tokens(letters).map(|(id, _)| id).collect::<Vec<_>>(),
        [letter],
        "the letter is back, with its id"
    );
    let failure = net
        .drain_events()
        .find_map(|event| match event {
            Event::Failed {
                restored, error, ..
            } => Some((restored, error)),
            _ => None,
        })
        .expect("a Failed event");
    assert!(failure.0);
    assert!(failure.1.contains("\"Notes\""), "{}", failure.1);
}

#[test]
fn a_detachable_body_on_another_thread_is_held_to_the_ports() {
    let mut net = field_of(unit(), |b, field| {
        let edge = TakeEdge(field).reaching(["Orders In"]);
        b.transition("Reach", (edge,), (field,), |mut unit: Net| {
            order_then_note(&mut unit);
            (unit,)
        });
    });
    let reach = net.transition_named("Reach").expect("Reach");
    let firing = net.launch(reach).expect("fireable");
    assert!(firing.is_pending(), "detachable");
    let completed = std::thread::spawn(move || firing.run())
        .join()
        .expect("the panic is caught in the body");
    let error = NetError::from(net.distribute(completed).expect_err("refused"));
    let NetError::FiringFailed(failed) = error else {
        panic!("a failed firing: {error:?}");
    };
    assert!(matches!(refusal(&failed), NetError::ClosedNet { .. }));
}

// ----- a body's knobs and firing by name ---------------------------------------

/// A knob a body might ask, under the act its refusal names.
type Knob = (&'static str, fn(&mut Net));

/// Every knob and firing by name a body might ask of a net it holds, each
/// dropping whatever it is told.
fn knobs() -> Vec<Knob> {
    #[cfg_attr(not(feature = "serde"), allow(unused_mut))]
    let mut knobs: Vec<Knob> = vec![
        ("set a parameter of", |net| {
            let _ = net.set_param(Speed(9));
        }),
        ("switch a transition of", |net| {
            let _ = net.disable(fire_of(net));
        }),
        ("switch a transition of", |net| {
            let _ = net.apply(Command::disable(fire_of(net)));
        }),
        ("set a parameter of", |net| {
            let _ = net.apply(Command::set_param(Speed(9)));
        }),
        ("set the tie-break of", |net| {
            net.set_tie_break(TieBreak::LastDeclared);
        }),
        // Since 0.4.0: the knobs with no `Result` have one that returns it.
        ("set the tie-break of", |net| {
            let _ = net.try_set_tie_break(TieBreak::LastDeclared);
        }),
        ("set the selector of", |net| {
            let _ = net.try_set_selector(|_, _| None);
        }),
        ("clear the selector of", |net| {
            let _ = net.try_clear_selector();
        }),
        ("set a stepping rule of", |net| {
            let _ = net.set_stepping(bay_of(net), Stepping::Passive);
        }),
        ("set the selector of", |net| net.set_selector(|_, _| None)),
        ("clear the selector of", |net| net.clear_selector()),
        ("fire a transition of", |net| {
            let _ = net.fire(fire_of(net));
        }),
        ("launch a transition of", |net| {
            if let Ok(firing) = net.launch(fire_of(net)) {
                let _ = net.distribute(firing.run());
            }
        }),
        ("step the nets inside", |net| {
            let _ = net.step_inside();
        }),
        ("press a control of", |net| {
            let _ = net.press("noop", Value::None);
        }),
        // Since 2026-11-28: what else a body leaves alone.
        ("drain the events of", |net| {
            let _ = net.drain_events().count();
        }),
        ("choose a transition of", |net| {
            let _ = net.choose();
        }),
        ("stage a step of", |net| {
            let _ = net.step_deep_staged();
        }),
        ("stage a step of", |net| {
            let _ = net.step_deep_staged_with(|_| {});
        }),
        ("switch off the failing transitions of", |net| {
            let _ = drivers::run(net, Mode::Sequential, Limits::default(), OnFailure::Disable);
        }),
        ("switch off the failing transitions of", |net| {
            let _ = drivers::run(net, Mode::Concurrent, Limits::default(), OnFailure::Disable);
        }),
        ("switch off the failing transitions of", |net| {
            for mode in [Mode::Sequential, Mode::Concurrent] {
                // A run let through would stop after its first tick.
                let stop = AtomicBool::new(false);
                let tick = Duration::from_millis(1);
                let _ =
                    drivers::run_indefinitely(net, mode, tick, &stop, OnFailure::Disable, |_| {
                        stop.store(true, Ordering::Relaxed);
                    });
            }
        }),
    ];
    #[cfg(feature = "serde")]
    knobs.push(("import a marking into", |net| {
        let _ = net.import_marking(&petritype::Marking::new());
    }));
    knobs
}

fn fire_of(net: &Net) -> TransitionId {
    net.transition_named("Fire").expect("Fire")
}

fn bay_of(net: &Net) -> PlaceId {
    net.place_named("Bay").expect("Bay")
}

/// A field whose `Reach` lends its body the unit by a mutate edge that
/// names `Orders In`, and asks `knob` of it.
fn reach_for(the_unit: Net, knob: fn(&mut Net)) -> Net {
    field_of(the_unit, |b, field| {
        let edge = MutateEdge(field).reaching(["Orders In"]);
        b.transition("Reach", (edge,), (), move |unit: &mut Net| knob(unit));
    })
}

#[test]
fn a_body_is_refused_every_knob_and_firing_by_name_on_every_net_it_holds() {
    // With a port and without, closed and declared open: the override opens
    // the places, not the controls.
    for (port, open) in [(true, false), (true, true), (false, false), (false, true)] {
        for (act, knob) in knobs() {
            let mut net = reach_for(unit_with(port, open), knob);
            let failed = net.step().expect_err(act).into_firing().unwrap();
            assert!(
                matches!(refusal(&failed), NetError::HeldByBody { act: said, .. } if *said == act),
                "{act}: {failed}"
            );
            let unit = front(&net);
            assert_eq!(unit.param::<Speed>(), Some(&Speed(1)), "{act}");
            assert!(
                unit.is_enabled(unit.transition_named("Fire").unwrap()),
                "{act}"
            );
            assert_eq!(unit.tie_break(), TieBreak::FirstDeclared, "{act}");
            assert_eq!(
                unit.stepping(unit.place_named("Bay").unwrap()),
                Some(Stepping::InnerFirst),
                "{act}"
            );
        }
    }
}

#[test]
fn a_body_is_refused_a_knob_on_a_net_that_marks_no_port_even_when_it_handles_the_error() {
    let mut net = reach_for(unit_with(false, false), |unit| {
        let refused = unit.set_param(Speed(7)).expect_err("held");
        assert!(matches!(refused, NetError::HeldByBody { .. }));
        for (act, refused) in [
            ("set the selector of", unit.try_set_selector(|_, _| None)),
            ("clear the selector of", unit.try_clear_selector()),
            (
                "set the tie-break of",
                unit.try_set_tie_break(TieBreak::RoundRobin),
            ),
        ] {
            assert!(
                matches!(refused, Err(NetError::HeldByBody { act: said, .. }) if said == act),
                "{act}: {refused:?}"
            );
        }
        assert_eq!(unit.tie_break(), TieBreak::FirstDeclared);
        let pressed = unit.press("noop", Value::None);
        assert!(
            matches!(&pressed, Err(petritype::PressError::Refused(message))
                if message.contains("press a control of")),
            "{pressed:?}"
        );
    });
    let failed = net
        .step()
        .expect_err("the firing fails with the first refusal")
        .into_firing()
        .unwrap();
    assert!(matches!(
        refusal(&failed),
        NetError::HeldByBody {
            act: "set a parameter of",
            ..
        }
    ));
    assert_eq!(front(&net).param::<Speed>(), Some(&Speed(1)));
}

#[test]
fn the_program_keeps_every_knob_and_firing_by_name_on_a_net_that_marks_no_port() {
    for open in [false, true] {
        let mut net = unit_seeded(false, open, 2);
        let fire = net.transition_named("Fire").expect("Fire");
        let bay = net.place_named("Bay").expect("Bay");
        net.set_param(Speed(2)).expect("a parameter");
        net.apply(Command::set_param(Speed(3)))
            .expect("a parameter");
        net.disable(fire).expect("a switch");
        net.enable(fire).expect("a switch");
        net.set_tie_break(TieBreak::RoundRobin);
        net.set_stepping(bay, Stepping::Passive)
            .expect("a stepping rule");
        net.set_selector(|_, fireable| fireable.first().copied());
        net.clear_selector();
        assert_eq!(net.press("noop", Value::None), Ok(Pressed::Applied));
        net.fire(fire).expect("firing by name");
        let firing = net.launch(fire).expect("launching by name");
        net.distribute(firing.run()).expect("and distributing");
        assert_eq!(net.step_inside().expect("nothing inside"), None);
        assert_eq!(net.fired_count(), 2);
        assert_eq!(net.param::<Speed>(), Some(&Speed(3)));
    }
}

/// A firing a body is refused landing in a net it holds comes back to it in
/// the `DistributeError` (since 0.4.0; until then it was dropped with its
/// body, and its transition stayed in flight for good), so the program can
/// land it once the body is done.
#[test]
fn a_firing_a_body_may_not_land_comes_back_to_be_landed_later() {
    use std::sync::{Arc, Mutex};
    let carried: Arc<Mutex<Option<Completed>>> = Arc::default();
    let inside = Arc::clone(&carried);
    let mut net = field_of(unit_seeded(true, false, 1), move |b, field| {
        b.transition("Reach", (MutateEdge(field),), (), move |unit: &mut Net| {
            let completed = inside.lock().unwrap().take().expect("a firing to land");
            let refused = unit.distribute(completed).expect_err("held");
            assert!(
                matches!(
                    refused.error(),
                    NetError::HeldByBody {
                        act: "distribute a firing of",
                        ..
                    }
                ),
                "{refused}"
            );
            *inside.lock().unwrap() = refused.into_completed();
        });
    });
    let completed = with_front(&mut net, |unit| {
        let fire = fire_of(unit);
        unit.launch(fire).expect("the program launches").run()
    });
    *carried.lock().unwrap() = Some(completed);

    let failed = net.step().expect_err("refused").into_firing().unwrap();
    assert!(matches!(
        refusal(&failed),
        NetError::HeldByBody {
            act: "distribute a firing of",
            ..
        }
    ));
    let completed = carried.lock().unwrap().take().expect("handed back");
    with_front(&mut net, |unit| {
        assert_eq!(unit.in_flight(), [fire_of(unit)], "still in flight");
        unit.distribute(completed).expect("the program lands it");
        assert!(unit.in_flight().is_empty(), "the body is home");
        assert_eq!(unit.fired_count(), 1);
    });
}

#[test]
fn a_body_steps_a_net_it_holds() {
    let mut net = reach_for(unit(), |unit| {
        order(unit);
        unit.step().expect("its own transition").expect("Fire");
        order(unit);
        unit.step_deep().expect("its own transition").expect("Fire");
    });
    net.step().expect("stepping is allowed");
    assert_eq!(front(&net).fired_count(), 2);
}

// ----- what else a body leaves alone on a net it holds (since 2026-11-28) -----

/// What a body might ask of a net it holds that it is refused with a
/// panic, under the act its refusal names: a drain, which has no `Result`
/// to refuse it with. It panics with the refusal. A choice did too until
/// 0.4.0, and is refused with an error now, as a staged step is.
fn asks_with_no_result() -> [Knob; 1] {
    [("drain the events of", |net| {
        let _ = net.drain_events().count();
    })]
}

/// A step a body is refused on a net it holds, under the act its refusal
/// names, saying whether it stepped anything.
type RefusedStep = (&'static str, fn(&mut Net) -> Result<bool, StepError>);

/// The steps a body is refused on a net it holds, and the choice that
/// would move the cursor with no firing: each returns
/// `StepError::HeldByBody` (since 0.4.0; until then each panicked).
fn steps_refused() -> [RefusedStep; 4] {
    [
        ("choose a transition of", |net| {
            net.choose().map(|chosen| chosen.is_some())
        }),
        ("stage a step of", |net| {
            net.step_deep_staged().map(|step| step.is_some())
        }),
        ("stage a step of", |net| {
            net.step_deep_staged_with(|_| panic!("no body runs"))
                .map(|step| step.is_some())
        }),
        ("step the nets inside", |net| {
            net.step_inside().map(|step| step.is_some())
        }),
    ]
}

#[test]
fn a_body_is_refused_a_choice_a_staged_step_or_a_step_inside_with_an_error() {
    for (act, step) in steps_refused() {
        let mut net = field_of(unit_seeded(true, false, 1), |b, field| {
            b.transition("Reach", (MutateEdge(field),), (), move |unit: &mut Net| {
                let refused = step(unit).expect_err(act);
                assert!(
                    matches!(&refused, StepError::HeldByBody { act: said, .. } if *said == act),
                    "{act}: {refused}"
                );
                assert!(refused.path().is_empty() && refused.firing().is_none());
                // The advice fits the step.
                let advice = match act {
                    "step the nets inside" => "leaves out the net's own turn",
                    "choose a transition of" => "moves only as the net fires",
                    _ => "a staged step leaves the net's choice",
                };
                let said = refused.to_string();
                assert!(said.contains(act) && said.contains(advice), "{said}");
                // `?` into a `NetError` gives the error the firing fails with.
                let passed_on = NetError::from(refused);
                assert!(
                    matches!(passed_on, NetError::HeldByBody { act: said, .. } if said == act),
                    "{act}: {passed_on}"
                );
                // The body goes on, and steps the net it holds.
                unit.step().expect("its own").expect("Fire");
            });
        });
        let failed = net.step().expect_err(act).into_firing().unwrap();
        assert!(
            matches!(refusal(&failed), NetError::HeldByBody { act: said, .. } if *said == act),
            "{act}: {failed}"
        );
        assert_eq!(
            front(&net).fired_count(),
            1,
            "{act}: the step after it stands"
        );
    }
}

#[test]
fn a_body_that_catches_the_panic_of_a_refused_drain_fails_all_the_same() {
    for (act, ask) in asks_with_no_result() {
        let mut net = field_of(unit_seeded(true, false, 1), |b, field| {
            b.transition("Reach", (MutateEdge(field),), (), move |unit: &mut Net| {
                let caught = catch_unwind(AssertUnwindSafe(|| ask(unit)));
                assert!(caught.is_err(), "{act}: refused with a panic");
                // The body goes on, and steps the net it holds.
                unit.step().expect("its own").expect("Fire");
            });
        });
        let failed = net.step().expect_err(act).into_firing().unwrap();
        assert!(
            matches!(refusal(&failed), NetError::HeldByBody { act: said, .. } if *said == act),
            "{act}: {failed}"
        );
        assert_eq!(
            front(&net).fired_count(),
            1,
            "{act}: the step after it stands"
        );
    }
}

/// A unit whose `Bay` holds a closed unit of its own.
fn unit_in_a_unit() -> Net {
    let mut b = NetBuilder::new();
    let orders = b.place::<String>("Orders In");
    let bay = b.place_nested::<Net>("Bay");
    b.port(orders);
    b.seed(bay, [unit_seeded(true, false, 1)]);
    b.build().expect("a unit with a unit inside")
}

#[test]
fn a_body_is_refused_the_same_on_a_net_inside_one_it_holds() {
    let steps: [Knob; 4] = [
        ("choose a transition of", |net| drop(net.choose())),
        ("stage a step of", |net| drop(net.step_deep_staged())),
        ("stage a step of", |net| {
            drop(net.step_deep_staged_with(|_| {}))
        }),
        ("step the nets inside", |net| drop(net.step_inside())),
    ];
    for (act, ask) in asks_with_no_result().into_iter().chain(steps) {
        let mut net = field_of(unit_in_a_unit(), |b, field| {
            b.transition("Reach", (MutateEdge(field),), (), move |held: &mut Net| {
                let bay = held.place_named("Bay").expect("Bay");
                let (token, _) = held.tokens(bay).next().expect("the inner unit");
                held.nested_scope(bay, token, |nets| ask(nets[0].1));
            });
        });
        let failed = net.step().expect_err(act).into_firing().unwrap();
        assert!(
            matches!(refusal(&failed), NetError::HeldByBody { act: said, .. } if *said == act),
            "{act}: {failed}"
        );
    }
}

/// A unit whose `Jam` fails on every order: `Orders In`, its port, seeded
/// with one.
fn jammed() -> Net {
    #[derive(Debug, thiserror::Error)]
    #[error("jammed")]
    struct Jammed;
    let mut b = NetBuilder::new();
    let orders = b.place::<String>("Orders In");
    b.port(orders);
    b.seed(orders, ["Order".to_owned()]);
    b.transition(
        "Jam",
        (TakeEdge(orders),),
        (),
        |_: String| -> Result<(), Jammed> { Err(Jammed) },
    );
    b.build().expect("a jammed unit")
}

#[test]
fn the_program_keeps_the_events_the_cursor_the_staged_steps_and_disable_on_failure() {
    // On a net no body holds, at the top level and lent from inside a
    // closed net, which shields it from writes and not from these.
    for shielded in [false, true] {
        let mut ours = unit_seeded(true, false, 3);
        let mut field = field_of(unit_seeded(true, false, 3), |_, _| {});
        let keep = |unit: &mut Net| {
            let fire = fire_of(unit);
            assert_eq!(unit.choose().unwrap(), Some(fire));
            let staged = unit.step_deep_staged().expect("nothing inside");
            assert_eq!(staged, Some(StagedStep::Chosen(fire)));
            let firing = unit.launch(fire).expect("launched");
            unit.distribute(firing.run()).expect("landed");
            let staged = unit.step_deep_staged_with(|_| {}).expect("nothing inside");
            assert_eq!(staged, Some(StagedStep::Chosen(fire)));
            let firing = unit.launch(fire).expect("launched");
            unit.distribute(firing.run()).expect("landed");
            assert!(unit.drain_events().count() > 0);
        };
        if shielded {
            with_front(&mut field, keep);
        } else {
            keep(&mut ours);
        }
        for mode in [Mode::Sequential, Mode::Concurrent] {
            let mut ours = jammed();
            let mut field = field_of(jammed(), |_, _| {});
            let disable = |unit: &mut Net| {
                let summary = drivers::run(unit, mode, Limits::default(), OnFailure::Disable)
                    .expect("the program's");
                assert_eq!(summary.failed_count, 1);
                assert!(!unit.is_enabled(unit.transition_named("Jam").unwrap()));
            };
            if shielded {
                with_front(&mut field, disable);
            } else {
                disable(&mut ours);
            }
        }
    }
}

#[test]
fn the_outer_log_receives_the_events_of_a_step_or_a_run_the_body_made() {
    let stepped: fn(&mut Net) = |unit| {
        order(unit);
        unit.step().expect("its own").expect("Fire");
    };
    let ran: fn(&mut Net) = |unit| {
        order(unit);
        let summary = drivers::run(unit, Mode::Concurrent, Limits::default(), OnFailure::Stop)
            .expect("its own");
        assert_eq!(summary.fired_count, 1);
    };
    for (body, concurrent) in [(stepped, false), (ran, true)] {
        let mut net = reach_for(unit(), body);
        net.step().expect("let through").expect("Reach");
        let fire = fire_of(front(&net));
        let inner: Vec<Event> = net
            .drain_events()
            .filter_map(|event| match event {
                Event::Nested { event, .. } => Some(*event),
                _ => None,
            })
            .collect();
        if concurrent {
            assert!(
                matches!(&inner[..], [
                    Event::Added { .. },
                    Event::Launched { transition: launched, .. },
                    Event::Distributed { transition: landed, .. },
                ] if *launched == fire && *landed == fire),
                "{inner:?}"
            );
        } else {
            assert!(
                matches!(&inner[..], [Event::Added { .. }, Event::Fired { transition, .. }]
                    if *transition == fire),
                "{inner:?}"
            );
        }
    }
}

/// A workshop a body drives: `Orders In`, its port, seeded with 1 to 6,
/// `Cut` from there to `Cut`, and `Sand` from there to `Done`, both
/// detachable, so that the concurrent driver runs their bodies on threads.
fn workshop() -> Net {
    let mut b = NetBuilder::new();
    let orders = b.place::<u32>("Orders In");
    let cut = b.place::<u32>("Cut");
    let done = b.place::<u32>("Done");
    b.port(orders);
    b.seed(orders, 1..=6);
    b.transition("Cut", (TakeEdge(orders),), (cut,), |n: u32| (n * 10,));
    b.transition("Sand", (TakeEdge(cut),), (done,), |n: u32| (n + 1,));
    b.build().expect("a workshop")
}

/// What a workshop's `Done` holds, in order, once a body of the field
/// around it has driven it in `mode`: lent by a mutate edge, or owned
/// through a take edge on a worker thread of the field's concurrent run.
fn driven(mode: Mode, owned: bool) -> Vec<u32> {
    let mut b = NetBuilder::new();
    let go = b.place::<u32>("Go");
    let field = b.place_nested::<Net>("Field");
    let parked = b.place_nested::<Net>("Parked");
    b.seed(go, [1]);
    b.seed(field, [workshop()]);
    // The field's own run leaves the workshop to the body.
    b.stepping(field, Stepping::Passive);
    b.stepping(parked, Stepping::Passive);
    let drive = move |unit: &mut Net| {
        let summary = drivers::run(unit, mode, Limits::default(), OnFailure::Stop).expect("driven");
        assert!(summary.quiesced);
        assert_eq!(summary.fired_count, 12);
    };
    if owned {
        b.transition(
            "Drive",
            (TakeEdge(go), TakeEdge(field)),
            (parked,),
            move |_: u32, mut unit: Net| {
                drive(&mut unit);
                (unit,)
            },
        );
    } else {
        b.transition(
            "Drive",
            (TakeEdge(go), MutateEdge(field)),
            (),
            move |_: u32, unit: &mut Net| drive(unit),
        );
    }
    let mut net = b.build().expect("a field");
    let summary = drivers::run(
        &mut net,
        Mode::Concurrent,
        Limits::default(),
        OnFailure::Stop,
    )
    .expect("the body drove the workshop");
    assert_eq!(summary.fired_count, 1);
    let at = if owned { parked } else { field };
    let (token, _) = net.tokens(at.id()).next().expect("the workshop");
    let workshop = net.token::<Net>(at.id(), token).expect("a net");
    let done = workshop.place_named("Done").expect("Done");
    let mut done: Vec<u32> = workshop
        .tokens(done)
        .filter_map(|(token, _)| workshop.token::<u32>(done, token).copied())
        .collect();
    done.sort_unstable();
    done
}

#[test]
fn a_body_drives_a_net_it_holds_in_either_mode_with_the_same_marking() {
    for owned in [false, true] {
        let sequential = driven(Mode::Sequential, owned);
        assert_eq!(sequential, [11, 21, 31, 41, 51, 61], "owned: {owned}");
        assert_eq!(
            driven(Mode::Concurrent, owned),
            sequential,
            "owned: {owned}"
        );
    }
}

#[test]
fn a_body_that_drives_a_net_it_holds_hears_of_its_failures_as_the_program_would() {
    for mode in [Mode::Sequential, Mode::Concurrent] {
        let mut net = field_of(jammed(), |b, field| {
            b.transition("Drive", (MutateEdge(field),), (), move |unit: &mut Net| {
                let refused = drivers::run(unit, mode, Limits::default(), OnFailure::Disable);
                assert!(
                    matches!(&refused, Err(DriverError::HeldByBody { .. })),
                    "{refused:?}"
                );
                let failed = drivers::run(unit, mode, Limits::default(), OnFailure::Stop);
                assert!(
                    matches!(&failed, Err(DriverError::FiringFailed { .. })),
                    "{failed:?}"
                );
                assert!(unit.in_flight().is_empty());
            });
        });
        let failed = net
            .step()
            .expect_err("the disable was refused")
            .into_firing()
            .unwrap();
        assert!(
            matches!(
                refusal(&failed),
                NetError::HeldByBody {
                    act: "switch off the failing transitions of",
                    ..
                }
            ),
            "{failed}"
        );
        let unit = front(&net);
        assert!(unit.is_enabled(unit.transition_named("Jam").unwrap()));
        assert_eq!(unit.count(unit.place_named("Orders In").unwrap()), 0);
    }
}

#[test]
fn the_mark_travels_down_through_the_scopes_and_lapses_when_the_firing_ends() {
    // The unit's bay holds a closed unit of its own.
    let mut outer_unit = NetBuilder::new();
    let orders = outer_unit.place::<String>("Orders In");
    let bay = outer_unit.place_nested::<Net>("Bay");
    outer_unit.port(orders);
    outer_unit.seed(bay, [unit()]);
    let outer_unit = outer_unit.build().expect("a unit with a unit inside");
    let mut net = reach_for(outer_unit, |held| {
        let bay = held.place_named("Bay").expect("Bay");
        let (token, _) = held.tokens(bay).next().expect("the inner unit");
        held.nested_scope(bay, token, |nets| {
            let _ = nets[0].1.set_param(Speed(9));
        });
    });
    let failed = net
        .step()
        .expect_err("two levels down, held all the same")
        .into_firing()
        .unwrap();
    assert!(matches!(
        refusal(&failed),
        NetError::HeldByBody {
            act: "set a parameter of",
            ..
        }
    ));

    // The firing has ended, so the program reaches the same net freely.
    with_front(&mut net, |held| {
        let bay = held.place_named("Bay").expect("Bay");
        let (token, _) = held.tokens(bay).next().expect("the inner unit");
        held.nested_scope(bay, token, |nets| nets[0].1.set_param(Speed(9)))
            .expect("a nested place")
    })
    .expect("the program's");
}

#[test]
fn a_body_holding_a_net_holds_its_subnet_too() {
    let mut held = NetBuilder::new();
    let lobby = held.place::<String>("Lobby");
    let level_place = held.subnet_place("Level", level());
    held.port(lobby);
    let held = held.build().expect("a net with a subnet");
    let level_id: PlaceId = level_place.id();
    let mut net = reach_for(held, |held| {
        let level = held.place_named("Level").expect("Level");
        let _ = held.subnet_scope(level, |level| {
            let _ = level.set_param(Speed(9));
        });
    });
    let failed = net.step().expect_err("held").into_firing().unwrap();
    assert!(matches!(refusal(&failed), NetError::HeldByBody { .. }));
    assert_eq!(
        front(&net).subnet(level_id).unwrap().param::<Speed>(),
        Some(&Speed(1))
    );
}

#[test]
fn a_body_that_puts_a_net_it_built_in_place_of_one_it_holds_has_made_a_new_net() {
    let mut net = reach_for(unit(), |held| {
        let mut fresh = unit();
        fresh
            .set_param(Speed(8))
            .expect("a new net carries no mark");
        *held = fresh;
        held.set_param(Speed(6)).expect("and it is the body's own");
    });
    let before = front(&net).id();
    net.step().expect("let through").expect("Reach");
    assert_eq!(front(&net).param::<Speed>(), Some(&Speed(6)));
    assert_ne!(front(&net).id(), before, "a new net, with an id of its own");
}

// ----- a body replaces the net of a token it was lent (since 2026-11-28) -----

/// A courier, a token that carries a net, its round.
#[derive(Debug)]
struct Courier {
    round: Net,
}

impl Nested for Courier {
    fn nets(&self) -> Vec<(&str, &Net)> {
        vec![("round", &self.round)]
    }
    fn nets_mut(&mut self) -> Vec<(&str, &mut Net)> {
        vec![("round", &mut self.round)]
    }
}

/// A round: `Inbox`, its port, started with `inbox`, and `File`, the
/// round's own move, from there to `Ledger`.
fn round(inbox: &[&str]) -> Net {
    let mut b = NetBuilder::new();
    let inbox_place = b.place::<String>("Inbox");
    let ledger = b.place::<String>("Ledger");
    b.port(inbox_place);
    b.seed(inbox_place, inbox.iter().map(|letter| (*letter).to_owned()));
    b.transition(
        "File",
        (TakeEdge(inbox_place),),
        (ledger,),
        |letter: String| (letter,),
    );
    b.build().expect("a round")
}

/// A closed depot whose `Restart` gives the courier it is lent a new
/// round, with a parcel in its inbox: a game that resets a character
/// swaps its whole net.
fn depot() -> Net {
    let mut b = NetBuilder::new();
    let couriers = b.place_nested::<Courier>("Couriers");
    let orders = b.place::<String>("Orders");
    let done = b.place::<String>("Done");
    b.port(orders);
    b.seed(couriers, [Courier { round: round(&[]) }]);
    b.seed(orders, ["Restart".to_owned()]);
    b.transition(
        "Restart",
        (MutateEdge(couriers), TakeEdge(orders)),
        (done,),
        |courier: &mut Courier, order: String| {
            courier.round = round(&["Parcel"]);
            (order,)
        },
    );
    b.build().expect("a depot")
}

/// The ids of the nets the depot's couriers carry.
fn rounds(depot: &Net) -> Vec<NetId> {
    let couriers = depot.place_named("Couriers").expect("Couriers");
    depot
        .tokens(couriers)
        .flat_map(|(token, _)| depot.nested(couriers, token).expect("nested"))
        .map(|(_, round)| round.id())
        .collect()
}

#[test]
fn a_net_a_body_puts_in_a_token_it_was_lent_is_told_as_a_new_net() {
    let mut net = depot();
    let couriers = net.place_named("Couriers").expect("Couriers");
    let (courier, _) = net.tokens(couriers).next().expect("a courier");
    let [old] = rounds(&net)[..] else {
        panic!("one round");
    };
    let restart = net.transition_named("Restart").expect("Restart");
    // Restart, then the new round's File, the round stepped inner first.
    let step = net.step_deep().expect("Restart");
    assert_eq!(
        step,
        Some(DeepStep::Here {
            transition: restart
        })
    );
    let step = net.step_deep().expect("File").expect("File");
    let [new] = rounds(&net)[..] else {
        panic!("one round");
    };
    assert_ne!(
        new, old,
        "the new net has an id of its own, and the old one is gone"
    );
    assert!(
        matches!(&step, DeepStep::Inside { nested_id, token, .. }
            if *nested_id == new && *token == courier),
        "{step:?}"
    );
    // The courier is the same token, told as changed, and every event of a
    // net it carries names the new net.
    let events: Vec<Event> = net.drain_events().collect();
    assert!(
        matches!(&events[0], Event::Mutated { place, token }
            if *place == couriers && *token == courier),
        "{events:?}"
    );
    let told: Vec<(NetId, &Event)> = events
        .iter()
        .filter_map(|event| match event {
            Event::Nested {
                nested_id, event, ..
            } => Some((*nested_id, &**event)),
            _ => None,
        })
        .collect();
    let carried = net.nested(couriers, courier).expect("nested")[0].1;
    let file = carried.transition_named("File").expect("File");
    assert!(
        matches!(&told[..], [(id, Event::Fired { transition, .. })]
            if *id == new && *transition == file),
        "{told:?}"
    );
    assert_eq!(strings(carried, "Ledger"), ["Parcel"]);
}

// ----- the nets inside a net a body holds (since 2026-11-28) -------------------

/// A post: `Inbox`, a port unless `port` is false, and `Ledger`, its own
/// place, starting with `ledger`; a `Speed` parameter. Declared open when
/// `open` is. With `serde` both places are serializable, for an import.
fn post_of(port: bool, open: bool, ledger: &[&str]) -> Net {
    let mut b = NetBuilder::new();
    let inbox = b.place::<String>("Inbox");
    let ledger_place = b.place::<String>("Ledger");
    #[cfg(feature = "serde")]
    {
        b.serializable(inbox);
        b.serializable(ledger_place);
    }
    if port {
        b.port(inbox);
    }
    if open {
        b.open();
    }
    b.seed(ledger_place, ledger.iter().map(|entry| (*entry).to_owned()));
    b.param(Speed(1));
    b.build().expect("a post")
}

/// A closed post.
fn post() -> Net {
    post_of(true, false, &[])
}

/// How a refusal names a net built without a template: by its id.
fn name(net: &Net) -> String {
    format!("{:?}", net.id())
}

/// Puts `"Letter"` at `place` of `net`.
fn put_at(net: &mut Net, place: &str) -> Result<petritype::TokenId, NetError> {
    let place = net.place_ref::<String>(place).expect("a place of strings");
    net.try_put(place, "Letter".to_owned())
}

/// Calls `f` with the net of the front token of `place`, through
/// `nested_scope`.
fn in_token<R>(net: &mut Net, place: &str, f: impl FnOnce(&mut Net) -> R) -> R {
    let place = net.place_named(place).expect("a place");
    let (token, _) = net.tokens(place).next().expect("a token");
    net.nested_scope(place, token, |nets| f(nets[0].1))
        .expect("a nested place")
}

/// Calls `f` with the subnet of `place`, through `subnet_scope`.
fn in_subnet<R>(net: &mut Net, place: &str, f: impl FnOnce(&mut Net) -> R) -> R {
    let place = net.place_named(place).expect("a place");
    net.subnet_scope(place, f).expect("a subnet place")
}

/// The net of the front token of `place`, to read.
fn token_net<'a>(net: &'a Net, place: &str) -> &'a Net {
    let place = net.place_named(place).expect("a place");
    let (token, _) = net.tokens(place).next().expect("a token");
    net.nested(place, token).expect("a nested place")[0].1
}

/// The subnet of `place`, to read.
fn subnet_of<'a>(net: &'a Net, place: &str) -> &'a Net {
    net.subnet(net.place_named(place).expect("a place"))
        .expect("a subnet place")
}

/// Which refusal an error was, and where.
fn why(error: &NetError) -> (ClosedRefusal, Option<String>, String) {
    match error {
        NetError::ClosedNet {
            why, place, net, ..
        } => (why.clone(), place.clone(), net.clone()),
        other => panic!("a ClosedNet refusal, not {other:?}"),
    }
}

/// An office around `unit`: `Letters` holding one letter, `Posts` holding
/// the unit (passive), `Done`, and `Use`, which takes the letter and lends
/// the body the unit through a mutate edge naming `reach`; the body does
/// `act` to it. Declared open when `open` is.
fn office(unit: Net, reach: &[&str], act: fn(&mut Net), open: bool) -> Net {
    let mut b = NetBuilder::new();
    let letters = b.place::<String>("Letters");
    let posts = b.place_nested::<Net>("Posts");
    let done = b.place::<String>("Done");
    b.seed(letters, ["Letter".to_owned()]);
    b.seed(posts, [unit]);
    b.stepping(posts, Stepping::Passive);
    if open {
        b.open();
    }
    let reach: Vec<String> = reach.iter().map(|name| (*name).to_owned()).collect();
    b.transition(
        "Use",
        (TakeEdge(letters), MutateEdge(posts).reaching(reach)),
        (done,),
        move |letter: String, unit: &mut Net| {
            act(unit);
            (letter,)
        },
    );
    b.build().expect("an office")
}

/// The refusal the one firing of `net` failed with.
fn failure(net: &mut Net) -> (ClosedRefusal, Option<String>, String) {
    let failed = net
        .step()
        .expect_err("the body is refused")
        .into_firing()
        .unwrap();
    why(refusal(&failed))
}

/// A unit holding `sack` in `Sacks` (passive), `Inbox` its port.
fn holding(sack: Net, open: bool) -> Net {
    let mut b = NetBuilder::new();
    let inbox = b.place::<String>("Inbox");
    let sacks = b.place_nested::<Net>("Sacks");
    b.port(inbox);
    b.seed(sacks, [sack]);
    b.stepping(sacks, Stepping::Passive);
    if open {
        b.open();
    }
    b.build().expect("a unit with a sack")
}

#[test]
fn a_closed_net_inside_the_net_a_body_holds_takes_no_token_from_it() {
    // An edge names ports of the net it hands over, so a write to the sack
    // inside it is refused as deeper, at its port and at its own place
    // alike, whatever the edge names, the unit closed or open.
    for unit_open in [false, true] {
        let sack = post();
        let sack_name = name(&sack);
        let unit = holding(sack, unit_open);
        let mut net = office(
            unit,
            &["Inbox"],
            |unit| {
                let _ = in_token(unit, "Sacks", |sack| put_at(sack, "Inbox"));
            },
            false,
        );
        assert_eq!(
            failure(&mut net),
            (ClosedRefusal::Deeper, Some("Inbox".to_owned()), sack_name)
        );
        let unit = token_net(&net, "Posts");
        assert_eq!(
            strings(token_net(unit, "Sacks"), "Inbox"),
            Vec::<String>::new()
        );

        let mut net = office(
            holding(post(), unit_open),
            &[],
            |unit| {
                let _ = in_token(unit, "Sacks", |sack| put_at(sack, "Ledger"));
            },
            false,
        );
        let failed = net.step().expect_err("deeper").into_firing().unwrap();
        let error = refusal(&failed);
        assert_eq!(why(error).0, ClosedRefusal::Deeper);
        let message = error.to_string();
        assert!(
            message.contains("inside the net the body holds")
                && !message.contains("Mark the place as a port"),
            "{message}"
        );

        // The program is refused too, by the nearest closed net around.
        let mut net = office(holding(post(), unit_open), &[], |_| {}, false);
        let office_name = name(&net);
        let unit_name = name(token_net(&net, "Posts"));
        let refused = in_token(&mut net, "Posts", |unit| {
            in_token(unit, "Sacks", |sack| put_at(sack, "Inbox"))
        })
        .expect_err("shielded");
        let around = if unit_open { office_name } else { unit_name };
        assert_eq!(
            why(&refused),
            (
                ClosedRefusal::Around { net: around },
                Some("Inbox".to_owned()),
                name(token_net(token_net(&net, "Posts"), "Sacks"))
            )
        );
    }

    // Every net around declared open: the body is refused as before, and
    // the program's write lands.
    let mut net = office(
        holding(post(), true),
        &["Inbox"],
        |unit| {
            let _ = in_token(unit, "Sacks", |sack| put_at(sack, "Inbox"));
        },
        true,
    );
    assert_eq!(failure(&mut net).0, ClosedRefusal::Deeper);
    in_token(&mut net, "Posts", |unit| {
        in_token(unit, "Sacks", |sack| put_at(sack, "Inbox"))
    })
    .expect("every net around is open");
    let sack = token_net(token_net(&net, "Posts"), "Sacks");
    assert_eq!(strings(sack, "Inbox"), ["Letter"]);
}

#[test]
fn an_open_net_inside_the_net_a_body_holds_is_shielded_by_that_net_when_it_is_closed() {
    // The sack is open, so its own rule lets a write land anywhere. The
    // unit the body holds is around it: closed, it shields the sack from
    // the body; declared open, it lets the write through. The office the
    // body belongs to is closed either way, and does not count: the body is
    // on the unit's side of it.
    for unit_open in [false, true] {
        let unit = holding(post_of(true, true, &[]), unit_open);
        let unit_name = name(&unit);
        let mut net = office(
            unit,
            &[],
            |unit| {
                let _ = in_token(unit, "Sacks", |sack| put_at(sack, "Ledger"));
            },
            false,
        );
        if unit_open {
            net.step().expect("lands").expect("Use");
            let sack = token_net(token_net(&net, "Posts"), "Sacks");
            assert_eq!(strings(sack, "Ledger"), ["Letter"]);
        } else {
            assert_eq!(
                failure(&mut net).0,
                ClosedRefusal::Around { net: unit_name }
            );
            let sack = token_net(token_net(&net, "Posts"), "Sacks");
            assert_eq!(strings(sack, "Ledger"), Vec::<String>::new());
        }
    }
}

/// A branch office: `Desk`, its port, and the subnet place `Branch`
/// holding `branch`, with a window onto its `Inbox`. Declared open when
/// `open` is.
fn branch_office(branch: Net, open: bool) -> Net {
    let mut b = NetBuilder::new();
    let desk = b.place::<String>("Desk");
    let level = b.subnet_place("Branch", branch);
    b.port_of::<String>(level, "Inbox");
    b.port(desk);
    if open {
        b.open();
    }
    b.build().expect("a branch office")
}

#[test]
fn a_subnet_scope_on_a_held_net_holds_the_subnet_for_the_body() {
    // The body reads the subnet; a closed subnet takes no token from it,
    // at its port either, and keeps its knobs.
    let mut net = office(
        branch_office(post(), false),
        &[],
        |unit| {
            let seen = in_subnet(unit, "Branch", |branch| strings(branch, "Inbox").len());
            assert_eq!(seen, 0, "reading is allowed");
        },
        false,
    );
    net.step().expect("reading").expect("Use");

    let mut net = office(
        branch_office(post(), false),
        &["Inbox"],
        |unit| {
            let _ = in_subnet(unit, "Branch", |branch| put_at(branch, "Inbox"));
        },
        false,
    );
    assert_eq!(
        failure(&mut net),
        (
            ClosedRefusal::Deeper,
            Some("Inbox".to_owned()),
            name(subnet_of(token_net(&net, "Posts"), "Branch"))
        )
    );
    let mut net = office(
        branch_office(post(), false),
        &[],
        |unit| {
            let _ = in_subnet(unit, "Branch", |branch| branch.set_param(Speed(9)));
        },
        false,
    );
    let failed = net.step().expect_err("a knob").into_firing().unwrap();
    assert!(matches!(refusal(&failed), NetError::HeldByBody { .. }));

    // The program's scope: refused inside the closed branch office, and
    // landing once it and the office around it are declared open.
    let mut net = office(branch_office(post(), false), &[], |_| {}, false);
    let unit_name = name(token_net(&net, "Posts"));
    let refused = in_token(&mut net, "Posts", |unit| {
        in_subnet(unit, "Branch", |branch| put_at(branch, "Inbox"))
    })
    .expect_err("shielded");
    assert_eq!(why(&refused).0, ClosedRefusal::Around { net: unit_name });
    let mut net = office(branch_office(post(), true), &[], |_| {}, true);
    in_token(&mut net, "Posts", |unit| {
        in_subnet(unit, "Branch", |branch| put_at(branch, "Inbox"))
    })
    .expect("every net around is open");
    let branch = subnet_of(token_net(&net, "Posts"), "Branch");
    assert_eq!(strings(branch, "Inbox"), ["Letter"]);
}

#[test]
fn a_window_through_a_held_net_reaches_no_closed_subnet() {
    let write = |unit: &mut Net| {
        let window = unit
            .port_ref::<String>(unit.place_named("Branch").unwrap(), "Inbox")
            .expect("a window");
        let _ = unit.try_put(window, "Letter".to_owned());
    };
    // A closed held net: the window is no port of it.
    let mut net = office(branch_office(post(), false), &[], write, false);
    assert_eq!(failure(&mut net).0, ClosedRefusal::NotAPort);
    // An open held net: the window leads into the closed post, a net inside
    // the one the body holds, named with its port.
    let branch = post();
    let branch_name = name(&branch);
    let mut net = office(branch_office(branch, true), &[], write, false);
    assert_eq!(
        failure(&mut net),
        (ClosedRefusal::Deeper, Some("Inbox".to_owned()), branch_name)
    );
    // The program at the same window: refused inside the closed office,
    // landing inside an open one.
    let office_name = name(&net);
    let refused = in_token(&mut net, "Posts", |unit| {
        let window = unit
            .port_ref::<String>(unit.place_named("Branch").unwrap(), "Inbox")
            .unwrap();
        unit.try_put(window, "From the program".to_owned())
    })
    .expect_err("shielded");
    assert_eq!(why(&refused).0, ClosedRefusal::Around { net: office_name });
    let mut net = office(branch_office(post(), true), &[], write, true);
    assert_eq!(
        failure(&mut net).0,
        ClosedRefusal::Deeper,
        "the body as before"
    );
    in_token(&mut net, "Posts", |unit| {
        let window = unit
            .port_ref::<String>(unit.place_named("Branch").unwrap(), "Inbox")
            .unwrap();
        unit.try_put(window, "From the program".to_owned())
    })
    .expect("every net around is open");
    // An open subnet of an open held net takes the body's write.
    let mut net = office(
        branch_office(post_of(true, true, &[]), true),
        &[],
        write,
        false,
    );
    net.step().expect("lands").expect("Use");
    let branch = subnet_of(token_net(&net, "Posts"), "Branch");
    assert_eq!(strings(branch, "Inbox"), ["Letter"]);
}

#[test]
fn a_net_that_marks_no_port_takes_no_token_from_a_body_whatever_its_edge_names() {
    let acts: [fn(&mut Net); 2] = [
        |unit| {
            let _ = put_at(unit, "Inbox");
        },
        |unit| {
            let ledger = unit.place_named("Ledger").unwrap();
            let (entry, _) = unit.tokens(ledger).next().unwrap();
            let _ = unit.remove(ledger, entry);
        },
    ];
    for act in acts {
        let mut net = office(
            post_of(false, false, &["Entry"]),
            &["Inbox", "Ledger"],
            act,
            false,
        );
        assert_eq!(failure(&mut net).0, ClosedRefusal::NotAPort);
        let unit = token_net(&net, "Posts");
        assert_eq!(strings(unit, "Inbox"), Vec::<String>::new());
        assert_eq!(strings(unit, "Ledger"), ["Entry"]);
    }
}

#[test]
fn an_open_net_takes_the_body_s_write_anywhere_and_keeps_its_knobs_from_it() {
    let mut net = office(
        post_of(true, true, &[]),
        &[],
        |unit| put_at(unit, "Ledger").map(drop).expect("open"),
        false,
    );
    net.step().expect("lands").expect("Use");
    assert_eq!(strings(token_net(&net, "Posts"), "Ledger"), ["Letter"]);
    let mut net = office(
        post_of(true, true, &[]),
        &[],
        |unit| {
            let _ = unit.set_param(Speed(9));
        },
        false,
    );
    let failed = net.step().expect_err("a knob").into_firing().unwrap();
    assert!(matches!(refusal(&failed), NetError::HeldByBody { .. }));
}

// ----- a closed net shields the nets inside it (since 2026-11-28) --------------

/// A sorting office: `Desk`, its port; `Sacks`, a port of nets (passive)
/// holding a post with `"Waiting"` in its inbox; and the subnet place
/// `Branch` holding another such post. Declared open when `open` is.
fn sorting_office(open: bool) -> Net {
    sorting_office_with(open, |_| {})
}

/// [`sorting_office`], with whatever `declare` adds.
fn sorting_office_with(open: bool, declare: impl FnOnce(&mut NetBuilder)) -> Net {
    let waiting = || {
        let mut b = NetBuilder::new();
        let inbox = b.place::<String>("Inbox");
        let ledger = b.place::<String>("Ledger");
        b.port(inbox);
        b.seed(inbox, ["Waiting".to_owned()]);
        b.param(Speed(1));
        #[cfg(feature = "serde")]
        {
            b.serializable(inbox);
            b.serializable(ledger);
        }
        let _ = ledger;
        b.build().expect("a post")
    };
    let mut b = NetBuilder::new();
    let desk = b.place::<String>("Desk");
    let sacks = b.place_nested::<Net>("Sacks");
    b.subnet_place("Branch", waiting());
    b.port(desk);
    b.port(sacks);
    b.stepping(sacks, Stepping::Passive);
    b.seed(sacks, [waiting()]);
    if open {
        b.open();
    }
    declare(&mut b);
    b.build().expect("a sorting office")
}

/// Each way the program reaches the nets inside a sorting office: the post
/// in a sack, and the post in the branch.
fn reaching_in<R>(office: &mut Net, f: impl Fn(&mut Net) -> R) -> [R; 2] {
    [
        in_token(office, "Sacks", &f),
        in_subnet(office, "Branch", &f),
    ]
}

#[test]
fn a_closed_net_shields_its_subnet_and_the_nets_its_tokens_carry_at_their_ports() {
    let mut office = sorting_office(false);
    let around = name(&office);
    let puts = reaching_in(&mut office, |post| {
        let refused = put_at(post, "Inbox").expect_err("shielded");
        (why(&refused), name(post))
    });
    let removes = reaching_in(&mut office, |post| {
        let inbox = post.place_named("Inbox").unwrap();
        let (waiting, _) = post.tokens(inbox).next().unwrap();
        let refused = post.remove(inbox, waiting).expect_err("shielded");
        assert!(matches!(
            refused,
            NetError::ClosedNet {
                act: "remove a token from",
                ..
            }
        ));
        (why(&refused), name(post))
    });
    for ((refusal, place, net), post) in puts.into_iter().chain(removes) {
        assert_eq!(
            refusal,
            ClosedRefusal::Around {
                net: around.clone()
            }
        );
        assert_eq!(place.as_deref(), Some("Inbox"));
        assert_eq!(net, post, "the net written is named, and the net around");
    }
    assert_eq!(strings(token_net(&office, "Sacks"), "Inbox"), ["Waiting"]);
    assert_eq!(strings(subnet_of(&office, "Branch"), "Inbox"), ["Waiting"]);
}

#[test]
fn a_command_on_a_net_inside_a_closed_net_is_refused_too() {
    let mut office = sorting_office(false);
    for command in [
        (|inbox| Command::put(inbox, "Letter".to_owned())) as fn(PlaceRef<String>) -> Command,
        |inbox| Command::set_tokens(inbox, ["Letter".to_owned()]),
    ] {
        let refused = reaching_in(&mut office, |post| {
            let inbox = place(post, "Inbox");
            post.apply(command(inbox))
        });
        for refused in refused {
            assert!(
                matches!(
                    refused,
                    Err(CommandError::Net(NetError::ClosedNet {
                        why: ClosedRefusal::Around { .. },
                        ..
                    }))
                ),
                "{refused:?}"
            );
        }
    }
    assert_eq!(strings(token_net(&office, "Sacks"), "Inbox"), ["Waiting"]);
}

#[test]
fn two_levels_down_every_net_around_must_be_declared_open() {
    // The top carries the middle net in a token, and the middle holds the
    // post in a subnet place. A refusal names the nearest closed net
    // around. A window on the middle is a write at the middle: a closed
    // middle refuses it by its own rule, and an open one as the nets
    // around it say.
    for (top_open, middle_open) in [(false, true), (true, false), (false, false), (true, true)] {
        let middle = branch_office(post(), middle_open);
        let middle_name = name(&middle);
        let mut b = NetBuilder::new();
        let floors = b.place_nested::<Net>("Floors");
        b.seed(floors, [middle]);
        b.stepping(floors, Stepping::Passive);
        if top_open {
            b.open();
        }
        let mut top = b.build().expect("a top");
        let top_name = name(&top);
        let down = in_token(&mut top, "Floors", |middle| {
            in_subnet(middle, "Branch", |bottom| put_at(bottom, "Inbox"))
        });
        let by_the_window = in_token(&mut top, "Floors", |middle| {
            let window = middle
                .port_ref::<String>(middle.place_named("Branch").unwrap(), "Inbox")
                .unwrap();
            middle.try_put(window, "By the window".to_owned())
        });
        let bottom = || subnet_of(token_net(&top, "Floors"), "Branch");
        match (top_open, middle_open) {
            (true, true) => {
                down.expect("every net around is open");
                by_the_window.expect("every net around is open");
                assert_eq!(strings(bottom(), "Inbox"), ["Letter", "By the window"]);
            }
            _ => {
                let nearest = if middle_open {
                    top_name.clone()
                } else {
                    middle_name
                };
                assert_eq!(
                    why(&down.expect_err("shielded")).0,
                    ClosedRefusal::Around { net: nearest }
                );
                let at_window = why(&by_the_window.expect_err("refused")).0;
                if middle_open {
                    assert_eq!(at_window, ClosedRefusal::Around { net: top_name });
                } else {
                    assert_eq!(at_window, ClosedRefusal::NotAPort);
                }
                assert_eq!(strings(bottom(), "Inbox"), Vec::<String>::new());
            }
        }
    }
}

#[test]
fn an_open_net_around_lets_the_write_land_at_the_port_and_the_inner_net_s_own_rule_still_refuses_its_own_place()
 {
    let mut office = sorting_office(true);
    let results = reaching_in(&mut office, |post| {
        put_at(post, "Inbox").expect("a port, inside an open net");
        let inbox = post.place_named("Inbox").unwrap();
        let (waiting, _) = post.tokens(inbox).next().unwrap();
        post.remove(inbox, waiting).expect("a port");
        let refused = put_at(post, "Ledger").expect_err("its own place");
        (why(&refused).0, refused.to_string())
    });
    for (refusal, message) in results {
        assert_eq!(refusal, ClosedRefusal::NotAPort);
        assert!(message.contains("declare the net open"), "{message}");
    }
    assert_eq!(strings(token_net(&office, "Sacks"), "Inbox"), ["Letter"]);
}

#[test]
fn inside_a_closed_net_the_inner_net_s_own_rule_is_judged_first() {
    let mut office = sorting_office(false);
    for refused in reaching_in(&mut office, |post| put_at(post, "Ledger")) {
        assert_eq!(
            why(&refused.expect_err("refused")).0,
            ClosedRefusal::NotAPort
        );
    }
}

#[test]
fn the_refusal_names_both_nets_and_says_how_to_put_it_right() {
    let mut office = sorting_office(false);
    let around = name(&office);
    let (refused, post) = in_token(&mut office, "Sacks", |post| {
        (put_at(post, "Inbox").expect_err("shielded"), name(post))
    });
    assert_eq!(
        refused.to_string(),
        format!(
            "cannot put a token into \"Inbox\" of net {post:?}: the net is inside net {around:?}, \
             which is closed, so from outside {around:?} nothing inside it is written. Put the \
             token at a port of {around:?} and let one of its transitions carry it in, or declare \
             {around:?} open"
        )
    );
}

#[test]
fn a_net_the_program_puts_into_a_closed_net_is_shielded_from_then_on() {
    // Rust's program cannot take a net back out of a place, as Python's
    // can: a removed token is dropped, and the net with it.
    let mut sack = post();
    put_at(&mut sack, "Inbox").expect("a port of a net inside nothing");
    let mut office = sorting_office(false);
    let sacks = office.place_ref::<Net>("Sacks").unwrap();
    office.put(sacks, sack);
    let sacks = sacks.id();
    let (token, _) = office.tokens(sacks).nth(1).expect("the new sack");
    let refused = office
        .nested_scope(sacks, token, |nets| put_at(nets[0].1, "Inbox"))
        .expect("a nested place")
        .expect_err("inside the office now");
    assert!(matches!(why(&refused).0, ClosedRefusal::Around { .. }));
}

#[test]
fn a_net_a_firing_moves_to_another_place_of_the_closed_net_is_still_inside_it() {
    let mut b = NetBuilder::new();
    let sacks = b.place_nested::<Net>("Sacks");
    let sent = b.place_nested::<Net>("Sent");
    b.seed(sacks, [post()]);
    b.stepping(sacks, Stepping::Passive);
    b.stepping(sent, Stepping::Passive);
    b.transition("Send", (TakeEdge(sacks),), (sent,), |sack: Net| (sack,));
    let mut office = b.build().expect("an office");
    office.step().expect("Send").expect("fired");
    let refused =
        in_token(&mut office, "Sent", |sack| put_at(sack, "Inbox")).expect_err("still inside");
    assert_eq!(
        why(&refused).0,
        ClosedRefusal::Around { net: name(&office) }
    );
}

#[test]
fn a_net_a_firing_moves_into_another_net_is_judged_by_that_net() {
    // An open depot locks the sack into its closed vault's bay, through the
    // window, and takes it out again: the program writes the sack before
    // and after, and not while it rests in the vault.
    let mut vault = NetBuilder::new();
    let bay = vault.place_nested::<Net>("Bay");
    vault.port(bay);
    vault.stepping(bay, Stepping::Passive);
    let vault = vault.build().expect("a vault");
    let vault_name = name(&vault);
    let mut b = NetBuilder::new();
    let sacks = b.place_nested::<Net>("Sacks");
    let level = b.subnet_place("Vault", vault);
    let window = b.port_of::<Net>(level, "Bay");
    let out = b.place_nested::<Net>("Out");
    b.seed(sacks, [post()]);
    b.transition("Lock", (TakeEdge(sacks),), (window,), |sack: Net| (sack,));
    b.transition("Unlock", (TakeEdge(window),), (out,), |sack: Net| (sack,));
    b.open();
    let mut depot = b.build().expect("an open depot");
    let lock = depot.transition_named("Lock").unwrap();
    let unlock = depot.transition_named("Unlock").unwrap();

    in_token(&mut depot, "Sacks", |sack| put_at(sack, "Inbox")).expect("before");
    depot.fire(lock).expect("Lock");
    let through_the_window = {
        let (token, _) = depot.tokens(window.id()).next().expect("in the bay");
        depot
            .nested_scope(window.id(), token, |nets| put_at(nets[0].1, "Inbox"))
            .expect("a nested place")
    };
    let through_the_vault = in_subnet(&mut depot, "Vault", |vault| {
        in_token(vault, "Bay", |sack| put_at(sack, "Inbox"))
    });
    for refused in [through_the_window, through_the_vault] {
        assert_eq!(
            why(&refused.expect_err("in the vault")).0,
            ClosedRefusal::Around {
                net: vault_name.clone()
            }
        );
    }
    depot.fire(unlock).expect("Unlock");
    in_token(&mut depot, "Out", |sack| put_at(sack, "Inbox")).expect("after");
    assert_eq!(
        strings(token_net(&depot, "Out"), "Inbox"),
        ["Letter", "Letter"]
    );
}

#[test]
fn a_body_whose_edge_names_the_port_writes_the_net_it_holds_inside_a_closed_net() {
    // The office is carried by a token of a closed top, and its `Use` fires
    // as a deep step of the top: the body is held to its edge's reach, not
    // to the nets around, while the program is refused at the same port.
    let office = office(
        post(),
        &["Inbox"],
        |unit| {
            put_at(unit, "Inbox").expect("named");
        },
        false,
    );
    let office_name = name(&office);
    let mut b = NetBuilder::new();
    let offices = b.place_nested::<Net>("Offices");
    b.seed(offices, [office]);
    let mut top = b.build().expect("a closed top");
    let refused = in_token(&mut top, "Offices", |office| {
        in_token(office, "Posts", |unit| put_at(unit, "Inbox"))
    })
    .expect_err("shielded");
    assert_eq!(why(&refused).0, ClosedRefusal::Around { net: office_name });
    let step = top.step_deep().expect("nothing fails").expect("Use fires");
    assert!(matches!(step, petritype::DeepStep::Inside { .. }));
    let office = token_net(&top, "Offices");
    assert_eq!(strings(token_net(office, "Posts"), "Inbox"), ["Letter"]);
    assert_eq!(strings(office, "Done"), ["Letter"]);
}

#[test]
fn the_engine_s_own_deposit_into_a_subnet_s_port_lands_in_a_deep_step_too() {
    let mut b = NetBuilder::new();
    let door = b.place::<String>("Door");
    let level = b.subnet_place("Branch", post());
    let window = b.port_of::<String>(level, "Inbox");
    b.seed(door, ["Letter".to_owned()]);
    b.transition("Enter", (TakeEdge(door),), (window,), |letter: String| {
        (letter,)
    });
    let world = b.build().expect("a closed world");
    let mut b = NetBuilder::new();
    let worlds = b.place_nested::<Net>("Worlds");
    b.seed(worlds, [world]);
    let mut top = b.build().expect("a closed top");
    top.step_deep()
        .expect("nothing fails")
        .expect("Enter fires");
    let world = token_net(&top, "Worlds");
    assert_eq!(strings(subnet_of(world, "Branch"), "Inbox"), ["Letter"]);
    assert_eq!(strings(world, "Door"), Vec::<String>::new());
}

#[test]
fn a_body_writes_the_ports_of_a_net_it_built_and_inside_it_only_if_it_built_it_open() {
    /// A builder: `Orders` holding one order, `Built` (passive), and
    /// `Build`, whose body builds a sorting office and writes it.
    fn builder(open: bool, reach: &'static str) -> Net {
        let mut b = NetBuilder::new();
        let orders = b.place::<String>("Orders");
        let built = b.place_nested::<Net>("Built");
        b.seed(orders, ["Letter".to_owned()]);
        b.stepping(built, Stepping::Passive);
        b.transition(
            "Build",
            (TakeEdge(orders),),
            (built,),
            move |order: String| -> Result<(Net,), NetError> {
                let mut office = sorting_office(open);
                let desk = place(&office, "Desk");
                office.try_put(desk, order.clone())?;
                let inside = |post: &mut Net| {
                    let inbox = place(post, "Inbox");
                    post.try_put(inbox, order.clone()).map(drop)
                };
                match reach {
                    "sack" => in_token(&mut office, "Sacks", inside)?,
                    _ => in_subnet(&mut office, "Branch", inside)?,
                }
                Ok((office,))
            },
        );
        b.build().expect("a builder")
    }

    for reach in ["sack", "branch"] {
        let mut net = builder(true, reach);
        net.step()
            .expect("its own office, built open")
            .expect("Build");
        let office = token_net(&net, "Built");
        assert_eq!(strings(office, "Desk"), ["Letter"]);
        // Once the firing has put the office into a place of the closed
        // builder, the program is refused inside it.
        let around = name(&net);
        let refused = in_token(&mut net, "Built", |office| {
            in_token(office, "Sacks", |post| put_at(post, "Inbox"))
        })
        .expect_err("inside the builder now");
        assert_eq!(why(&refused).0, ClosedRefusal::Around { net: around });

        let mut net = builder(false, reach);
        let failed = net
            .step()
            .expect_err("the office built closed")
            .into_firing()
            .unwrap();
        let (kind, place, _) = why(refusal(&failed));
        assert!(matches!(kind, ClosedRefusal::Around { .. }), "{reach}");
        assert_eq!(place.as_deref(), Some("Inbox"));
    }
}

#[test]
fn a_body_that_reaches_a_net_by_a_closure_is_outside_the_nets_around_it_as_the_program_is() {
    use std::sync::{Arc, Mutex};
    for open in [false, true] {
        let office = Arc::new(Mutex::new(sorting_office(open)));
        let held = Arc::clone(&office);
        let mut b = NetBuilder::new();
        let orders = b.place::<String>("Orders");
        let done = b.place::<String>("Done");
        b.seed(orders, ["Letter".to_owned()]);
        b.transition(
            "Meddle",
            (TakeEdge(orders),),
            (done,),
            move |order: String| -> Result<(String,), NetError> {
                let mut office = held.lock().unwrap();
                in_token(&mut office, "Sacks", |sack| put_at(sack, "Inbox"))?;
                Ok((order,))
            },
        )
        .restore_tokens_on_failure();
        let mut other = b.build().expect("another net");
        let result = other.step();
        let office = office.lock().unwrap();
        let sack = token_net(&office, "Sacks");
        if open {
            result.expect("an open office").expect("Meddle");
            assert_eq!(strings(sack, "Inbox"), ["Waiting", "Letter"]);
        } else {
            let failed = result.expect_err("a closed office").into_firing().unwrap();
            assert_eq!(
                why(refusal(&failed)).0,
                ClosedRefusal::Around { net: name(&office) }
            );
            assert_eq!(strings(sack, "Inbox"), ["Waiting"]);
        }
    }
}

#[test]
fn the_new_net_a_body_puts_in_a_token_it_was_lent_is_shielded_as_the_old_one_was() {
    let mut net = office(
        post(),
        &[],
        |unit| {
            *unit = post();
        },
        false,
    );
    net.step().expect("let through").expect("Use");
    let refused =
        in_token(&mut net, "Posts", |unit| put_at(unit, "Inbox")).expect_err("inside the office");
    assert_eq!(why(&refused).0, ClosedRefusal::Around { net: name(&net) });
}

#[test]
fn the_knobs_of_a_net_inside_a_closed_net_stay_with_the_program() {
    let mut office = sorting_office(false);
    reaching_in(&mut office, |post| {
        post.set_param(Speed(3)).expect("a parameter");
        post.set_tie_break(TieBreak::RoundRobin);
    });
    assert_eq!(
        token_net(&office, "Sacks").param::<Speed>(),
        Some(&Speed(3))
    );
    assert_eq!(
        subnet_of(&office, "Branch").param::<Speed>(),
        Some(&Speed(3))
    );
}

#[test]
fn a_control_of_a_net_reaches_the_nets_inside_it_by_their_own_rule() {
    // A handler is inside its net, so a scope it opens on its own net
    // shields nothing by that net: the post inside is judged by its own
    // rule, its port taking the letter and its ledger refusing it.
    let mut office = sorting_office_with(false, |b| {
        b.control(Control::button("file", "File", |office, _| {
            let results = reaching_in(office, |post| {
                (
                    put_at(post, "Inbox").map(drop),
                    put_at(post, "Ledger").map(drop),
                )
            });
            for (inbox, ledger) in results {
                inbox.expect("the post's own port");
                let refused = ledger.expect_err("its own place");
                assert_eq!(why(&refused).0, ClosedRefusal::NotAPort);
            }
            Ok(Pressed::Applied)
        }));
    });
    assert_eq!(office.press("file", Value::None), Ok(Pressed::Applied));
    assert_eq!(
        strings(token_net(&office, "Sacks"), "Inbox"),
        ["Waiting", "Letter"]
    );
    assert_eq!(
        strings(subnet_of(&office, "Branch"), "Inbox"),
        ["Waiting", "Letter"]
    );
}

#[cfg(feature = "serde")]
mod imports {
    use super::*;
    use petritype::serde_json::{self, json};
    use petritype::{Marking, Params, Templates};

    fn marking() -> Marking {
        serde_json::from_value(json!({"Inbox": ["Loaded"], "Ledger": ["Loaded"]})).unwrap()
    }

    /// An import writes every place of a net, so a closed net shields the
    /// nets inside it from one as from a put; the net around is loaded as a
    /// whole, or declared open. Its knobs stay the program's.
    #[test]
    fn an_import_into_a_net_inside_a_closed_net_is_refused_and_lands_inside_an_open_one() {
        let mut office = sorting_office(false);
        let around = name(&office);
        let refused = reaching_in(&mut office, |post| {
            post.set_param(Speed(3)).expect("a knob is the program's");
            post.import_marking(&marking())
        });
        for refused in refused {
            let Err(CommandError::Net(error)) = refused else {
                panic!("refused: {refused:?}");
            };
            assert!(
                matches!(&error, NetError::ClosedNet { place: None, act: "import a marking into", why: ClosedRefusal::Around { net }, .. }
                    if *net == around),
                "{error:?}"
            );
            let message = error.to_string();
            assert!(
                message.contains(&format!("into {around:?} as a whole")),
                "{message}"
            );
        }
        assert_eq!(
            strings(token_net(&office, "Sacks"), "Ledger"),
            Vec::<String>::new()
        );
        assert_eq!(
            strings(subnet_of(&office, "Branch"), "Ledger"),
            Vec::<String>::new()
        );

        let mut office = sorting_office(true);
        for loaded in reaching_in(&mut office, |post| post.import_marking(&marking())) {
            loaded.expect("inside an open net");
        }
        assert_eq!(strings(token_net(&office, "Sacks"), "Ledger"), ["Loaded"]);
        assert_eq!(strings(subnet_of(&office, "Branch"), "Ledger"), ["Loaded"]);
    }

    #[test]
    fn a_body_that_does_not_hold_the_net_is_refused_the_import_as_the_program_is() {
        use std::sync::{Arc, Mutex};
        let office = Arc::new(Mutex::new(sorting_office(false)));
        let held = Arc::clone(&office);
        let mut b = NetBuilder::new();
        let orders = b.place::<String>("Orders");
        let done = b.place::<String>("Done");
        b.seed(orders, ["Letter".to_owned()]);
        b.transition(
            "Meddle",
            (TakeEdge(orders),),
            (done,),
            move |order: String| -> Result<(String,), CommandError> {
                let mut office = held.lock().unwrap();
                in_token(&mut office, "Sacks", |sack| sack.import_marking(&marking()))?;
                Ok((order,))
            },
        )
        .restore_tokens_on_failure();
        let mut other = b.build().expect("another net");
        let failed = other.step().expect_err("refused").into_firing().unwrap();
        let error = failed
            .error
            .downcast_ref::<CommandError>()
            .expect("its error");
        assert!(
            matches!(
                error,
                CommandError::Net(NetError::ClosedNet {
                    place: None,
                    why: ClosedRefusal::Around { .. },
                    ..
                })
            ),
            "{error:?}"
        );
        let office = office.lock().unwrap();
        assert_eq!(
            strings(token_net(&office, "Sacks"), "Ledger"),
            Vec::<String>::new()
        );
    }

    /// A body imports into no net inside the one it holds, open or closed,
    /// through either scope: it holds that net too.
    #[test]
    fn a_body_imports_no_marking_into_a_net_inside_the_one_it_holds() {
        let acts: [fn(&mut Net); 2] = [
            |unit| {
                let _ = in_token(unit, "Sacks", |post| post.import_marking(&marking()));
            },
            |unit| {
                let _ = in_subnet(unit, "Branch", |post| post.import_marking(&marking()));
            },
        ];
        for act in acts {
            let mut net = office(sorting_office(true), &[], act, false);
            let failed = net.step().expect_err("held").into_firing().unwrap();
            assert!(
                matches!(
                    refusal(&failed),
                    NetError::HeldByBody {
                        act: "import a marking into",
                        ..
                    }
                ),
                "{failed}"
            );
            let unit = token_net(&net, "Posts");
            assert_eq!(
                strings(token_net(unit, "Sacks"), "Ledger"),
                Vec::<String>::new()
            );
            assert_eq!(
                strings(subnet_of(unit, "Branch"), "Ledger"),
                Vec::<String>::new()
            );
        }
    }

    /// A fork is a new net, which rests in no place: a shield does not go
    /// with it.
    #[test]
    fn a_fork_of_a_shielded_net_is_shielded_by_nothing() {
        let mut templates = Templates::new();
        templates.register("Test Post", |_: &Params| {
            Ok::<_, petritype::BuildError>(post())
        });
        let mut b = NetBuilder::new();
        let sacks = b.place_nested::<Net>("Sacks");
        b.seed(
            sacks,
            [templates.build("Test Post", &Params::new()).unwrap()],
        );
        b.stepping(sacks, Stepping::Passive);
        let mut office = b.build().expect("a closed office");
        let mut fork = in_token(&mut office, "Sacks", |sack| {
            let refused = put_at(sack, "Inbox").expect_err("shielded");
            assert!(matches!(why(&refused).0, ClosedRefusal::Around { .. }));
            sack.fork().expect("a fork")
        });
        put_at(&mut fork, "Inbox").expect("a port of a net inside nothing");
        assert_eq!(strings(&fork, "Inbox"), ["Letter"]);
        assert_eq!(
            strings(token_net(&office, "Sacks"), "Inbox"),
            Vec::<String>::new()
        );
    }
}
