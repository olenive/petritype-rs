//! Integration tests for closed nets (DESIGN §6.9, since 2026-11-23): a net
//! that marks a port takes tokens from outside it only at its ports, unless
//! it declares itself open, and a body that holds a net marking a port is
//! refused its knobs and firing by name.
//!
//! One test, or a few, per line of the rule: who is outside (a body by each
//! edge kind that holds a token's net, the program by each method,
//! `nested_scope`, `subnet_scope`, an erased `Command`), a net two levels
//! down, an open net, a net with no port, and each thing that stays
//! allowed (reading, stepping, the whole-state acts, the program's knobs
//! and firing by name, a net's own control, its seeds and its own
//! firings). The runner's `inject` is tested in `petritype-runner`.

use std::panic::{AssertUnwindSafe, catch_unwind};

use petritype::control::{Control, Pressed, Value};
use petritype::{
    Command, CommandError, DrainEdge, Event, Filter, FiringFailed, KeyValue, MutateEdge,
    MutateWhereEdge, Net, NetBuilder, NetError, PlaceId, PlaceRef, ReadEdge, Stepping, TakeAllEdge,
    TakeEdge, TakeWhereEdge, TieBreak, TransitionId,
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
fn a_net_that_marks_a_port_is_closed_and_one_that_marks_none_is_open() {
    let closed = unit();
    assert!(closed.is_closed());
    assert!(!closed.is_open(), "it declares nothing");

    let plain = unit_with(false, false);
    assert!(!plain.is_closed(), "no port, so open as every net was");
    assert!(!plain.is_open(), "open by default, not by declaration");

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
                if place == "Notes" && *said == act),
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
fn a_net_that_marks_no_port_and_an_open_net_take_tokens_anywhere() {
    for mut net in [unit_with(false, false), unit_with(true, true)] {
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
    assert_eq!(net.press("noop", Value::None), Ok(Pressed::Applied));
    order(&mut net);
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

    let mut templates = Templates::new();
    templates.register("Post", |_: &Params| {
        let mut b = NetBuilder::new();
        let inbox = b.place::<String>("Inbox");
        let ledger = b.place::<String>("Ledger");
        b.serializable(inbox);
        b.serializable(ledger);
        b.port(inbox);
        b.build()
    });
    let mut post = templates.build("Post", &Params::new()).expect("a post");
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
    let mut net = field_of(unit(), |_, _| {});
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
    // Open: the world marks no port of its own, and the level's lobby
    // takes the token.
    let mut b = NetBuilder::new();
    let level_place = b.subnet_place("Level", level());
    let door = b.port_of::<String>(level_place, "Lobby");
    let mut world = b.build().expect("an open world");
    world.put(door, "Ada".to_owned());
    assert_eq!(world.count(door.id()), 1);

    // Closed: the world marks a port of its own, and its window, which can
    // never be one, refuses a write from outside. Its seed and its own
    // firing's output at the window are inside, and land.
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
        matches!(&refused, NetError::ClosedNet { place, .. } if place == "Lobby"),
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
    // The middle net marks no port, so it is open; the unit inside it is
    // closed.
    let mut middle = NetBuilder::new();
    let bay = middle.place_nested::<Net>("Bay");
    middle.seed(bay, [unit()]);
    let middle = middle.build().expect("a middle net");
    let mut net = field_of(middle, |_, _| {});
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
fn fire_once(net: &mut Net) -> Result<Option<TransitionId>, FiringFailed> {
    net.step()
}

/// Asserts that a firing failed with a [`NetError::ClosedNet`] at `Notes`.
fn failed_closed(result: Result<Option<TransitionId>, FiringFailed>) -> FiringFailed {
    let failed = result.expect_err("refused");
    assert!(
        matches!(refusal(&failed), NetError::ClosedNet { place, .. } if place == "Notes"),
        "{failed}"
    );
    failed
}

#[test]
fn a_mutate_edge_body_writes_at_a_port_and_fails_at_a_private_place() {
    for (write, lands) in [(order as fn(&mut Net), true), (order_then_note, false)] {
        let mut net = field_of(unit(), |b, field| {
            b.transition("Reach", (MutateEdge(field),), (), move |unit: &mut Net| {
                write(unit)
            });
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
                (MutateWhereEdge(field, the_unit()),),
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
                (TakeEdge(field),),
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
                (TakeWhereEdge(field, the_unit()),),
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
                    b.transition("Reach", (DrainEdge(field),), (field,), body);
                } else {
                    b.transition("Reach", (TakeAllEdge(field),), (field,), body);
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
            b.transition("Reach", (MutateEdge(field),), (), move |unit: &mut Net| {
                write(unit)
            });
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
        b.transition("Reach", (TakeEdge(field),), (field,), |mut unit: Net| {
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
    let error = net.distribute(completed).expect_err("refused");
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

/// A field whose `Reach` lends its body the unit by a mutate edge and asks
/// `knob` of it.
fn reach_for(the_unit: Net, knob: fn(&mut Net)) -> Net {
    field_of(the_unit, |b, field| {
        b.transition("Reach", (MutateEdge(field),), (), move |unit: &mut Net| {
            knob(unit)
        });
    })
}

#[test]
fn a_body_is_refused_every_knob_and_firing_by_name_on_a_net_that_marks_a_port() {
    // Closed, and declared open: the override opens the places, not the
    // controls.
    for (port, open) in [(true, false), (true, true)] {
        for (act, knob) in knobs() {
            let mut net = reach_for(unit_with(port, open), knob);
            let failed = net.step().expect_err(act);
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
fn a_body_keeps_every_knob_on_a_net_that_marks_no_port_in_stage_one() {
    for (act, knob) in knobs() {
        let mut net = reach_for(unit_with(false, false), knob);
        net.step()
            .unwrap_or_else(|failed| panic!("{act}: {failed}"))
            .expect("Reach");
    }
    let mut net = reach_for(unit_with(false, false), |unit| {
        unit.set_param(Speed(7)).expect("allowed in stage one");
    });
    net.step().expect("Reach");
    assert_eq!(front(&net).param::<Speed>(), Some(&Speed(7)));
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
    let failed = net.step().expect_err("two levels down, held all the same");
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
    let failed = net.step().expect_err("held");
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
    net.step().expect("let through").expect("Reach");
    assert_eq!(front(&net).param::<Speed>(), Some(&Speed(6)));
}
