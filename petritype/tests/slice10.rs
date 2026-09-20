//! Integration tests for the tenth slice: `Command`, `Net::apply`, and the
//! JSON form of both.
//!
//! A command is the typed input API with the types forgotten, for a sender
//! that holds no `&mut Net` — the far side of a channel from a UI thread, or
//! a decoder. So most of what is tested here is sameness: the same marking
//! and the same events as the typed method of the same name. The rest is what
//! only the erased route can get wrong — a token of the wrong type, an id
//! from another net — and the promise that getting it wrong changes nothing
//! and says nothing.

use std::any::Any;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use petritype::drivers::{self, Mode, OnFailure};
use petritype::{
    Command, CommandError, Event, Net, NetBuilder, NetError, PlaceId, PlaceRef, TakeEdge, TokenId,
    TransitionId,
};

/// Reads a place back as plain values, oldest first.
fn values<T: Clone + Send + Sync + 'static>(net: &Net, place: PlaceId) -> Vec<T> {
    net.tokens(place)
        .map(|(_, value)| {
            value
                .downcast_ref::<T>()
                .expect("the place holds this type")
                .clone()
        })
        .collect()
}

fn ids(net: &Net, place: PlaceId) -> Vec<TokenId> {
    net.tokens(place).map(|(id, _)| id).collect()
}

/// `Inbox -> Double -> Out`: one place to put tokens in, one transition to
/// switch on and off.
fn net_with_a_transition() -> (Net, PlaceRef<u32>, PlaceRef<u32>, TransitionId) {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    let double = b
        .transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,))
        .id();
    let net = b.build().expect("a well-formed net");
    (net, inbox, out, double)
}

/// A net with one `u32` place and nothing else.
fn small_net() -> (Net, PlaceRef<u32>) {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let net = b.build().expect("a well-formed net");
    (net, inbox)
}

/// A second net with more places and transitions than the ones above, purely
/// as a source of ids that are out of range there. The inner index of a
/// `PlaceId` is private, so an out-of-range id has to be borrowed from a
/// bigger net rather than made up.
fn bigger_net() -> (Net, PlaceRef<u32>, TransitionId) {
    let mut b = NetBuilder::new();
    let first = b.place::<u32>("First");
    let second = b.place::<u32>("Second");
    let third = b.place::<u32>("Third");
    b.transition("One", (TakeEdge(first),), (second,), |n: u32| (n,));
    let two = b
        .transition("Two", (TakeEdge(second),), (third,), |n: u32| (n,))
        .id();
    let net = b.build().expect("a well-formed net");
    (net, third, two)
}

// ----- the typed constructors ---------------------------------------------

#[test]
fn put_appends_one_token_with_one_added_event() {
    let (mut net, inbox) = small_net();

    net.apply(Command::put(inbox, 7)).expect("a u32 place");

    assert_eq!(values::<u32>(&net, inbox.id()), [7]);
    assert_eq!(net.state_counter(), 1, "one event, one tick");
    let token = ids(&net, inbox.id())[0];
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Added {
            place: inbox.id(),
            token
        }],
    );
}

#[test]
fn extend_appends_in_order_with_one_event_per_token() {
    let (mut net, inbox) = small_net();

    net.apply(Command::extend(inbox, [1, 2, 3]))
        .expect("a u32 place");

    assert_eq!(values::<u32>(&net, inbox.id()), [1, 2, 3]);
    let tokens = ids(&net, inbox.id());
    let events: Vec<Event> = net.drain_events().collect();
    assert_eq!(
        events,
        tokens
            .iter()
            .map(|&token| Event::Added {
                place: inbox.id(),
                token
            })
            .collect::<Vec<_>>(),
    );
    assert_eq!(net.state_counter(), 3);
}

#[test]
fn set_tokens_replaces_everything_and_an_empty_batch_clears() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    b.seed(inbox, [1, 2]);
    let mut net = b.build().expect("a well-formed net");
    let seeded = ids(&net, inbox.id());

    net.apply(Command::set_tokens(inbox, [9]))
        .expect("a u32 place");

    assert_eq!(values::<u32>(&net, inbox.id()), [9]);
    let fresh = ids(&net, inbox.id())[0];
    assert!(!seeded.contains(&fresh), "new tokens, new ids");
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [
            Event::Removed {
                place: inbox.id(),
                token: seeded[0]
            },
            Event::Removed {
                place: inbox.id(),
                token: seeded[1]
            },
            Event::Added {
                place: inbox.id(),
                token: fresh
            },
        ],
    );

    // An empty batch is how a command clears a place.
    net.apply(Command::set_tokens(inbox, Vec::new()))
        .expect("a u32 place");
    assert_eq!(net.count(inbox.id()), 0);
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Removed {
            place: inbox.id(),
            token: fresh
        }],
    );
}

#[test]
fn remove_takes_one_token_by_id_and_refuses_an_unknown_one() {
    let (mut net, inbox) = small_net();
    net.apply(Command::extend(inbox, [1, 2]))
        .expect("a u32 place");
    let tokens = ids(&net, inbox.id());
    let _ = net.drain_events();

    net.apply(Command::remove(inbox.id(), tokens[0]))
        .expect("a token that is there");

    assert_eq!(values::<u32>(&net, inbox.id()), [2]);
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Removed {
            place: inbox.id(),
            token: tokens[0]
        }],
    );

    // The retired id is not there any more.
    let counter = net.state_counter();
    let rejected = net.apply(Command::remove(inbox.id(), tokens[0]));
    assert!(matches!(
        rejected,
        Err(CommandError::Net(NetError::NoSuchToken { .. })),
    ));
    assert_eq!(values::<u32>(&net, inbox.id()), [2], "untouched");
    assert_eq!(net.state_counter(), counter, "and silent");
    assert_eq!(net.drain_events().count(), 0);
}

#[test]
fn replace_keeps_the_id_and_refuses_the_wrong_type() {
    let (mut net, inbox) = small_net();
    net.apply(Command::extend(inbox, [1, 2]))
        .expect("a u32 place");
    let tokens = ids(&net, inbox.id());
    let _ = net.drain_events();

    net.apply(Command::replace(inbox, tokens[0], 42))
        .expect("a token that is there");

    assert_eq!(values::<u32>(&net, inbox.id()), [42, 2]);
    assert_eq!(ids(&net, inbox.id()), tokens, "the same tokens as before");
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Mutated {
            place: inbox.id(),
            token: tokens[0]
        }],
    );

    // Only an erased sender can get this wrong: the typed constructor above
    // would not compile with a `&str`.
    let counter = net.state_counter();
    let value: Box<dyn Any + Send + Sync> = Box::new("nope");
    let rejected = net.apply(Command::Replace {
        place: inbox.id(),
        token: tokens[0],
        value,
    });
    match rejected {
        Err(CommandError::Net(NetError::TypeMismatch {
            place,
            expected,
            found,
        })) => {
            assert_eq!(place, inbox.id());
            assert_eq!(expected, "u32");
            assert_eq!(found, "<erased>", "a dyn Any knows its TypeId, not a name");
        }
        other => panic!("expected a type mismatch, got {other:?}"),
    }
    assert_eq!(values::<u32>(&net, inbox.id()), [42, 2], "untouched");
    assert_eq!(net.state_counter(), counter, "and silent");
}

#[test]
fn enable_and_disable_work_the_switch() {
    let (mut net, inbox, _out, double) = net_with_a_transition();
    net.put(inbox, 21);
    let _ = net.drain_events();

    net.apply(Command::disable(double)).expect("a transition");
    assert!(!net.is_enabled(double));
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Toggled {
            transition: double,
            enabled: false
        }],
    );

    net.apply(Command::enable(double)).expect("a transition");
    assert!(net.is_enabled(double));
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Toggled {
            transition: double,
            enabled: true
        }],
    );

    // An id from a net with more transitions is out of range here.
    let (_bigger, _place, foreign) = bigger_net();
    let counter = net.state_counter();
    assert!(matches!(
        net.apply(Command::enable(foreign)),
        Err(CommandError::Net(NetError::NoSuchTransition { .. })),
    ));
    assert_eq!(net.state_counter(), counter);
    assert_eq!(net.drain_events().count(), 0);
}

// ----- what only the erased route can get wrong ---------------------------

#[test]
fn a_batch_with_one_bad_token_adds_none_of_them_and_emits_nothing() {
    let (mut net, inbox) = small_net();
    net.apply(Command::put(inbox, 1)).expect("a u32 place");
    let _ = net.drain_events();
    let counter = net.state_counter();

    // The second of three is a `&str`: what an untyped sender can build and
    // the typed constructors cannot.
    let tokens: Vec<Box<dyn Any + Send + Sync>> =
        vec![Box::new(10_u32), Box::new("two"), Box::new(30_u32)];
    let rejected = net.apply(Command::Extend {
        place: inbox.id(),
        tokens,
    });

    assert!(matches!(
        rejected,
        Err(CommandError::Net(NetError::TypeMismatch { .. })),
    ));
    assert_eq!(
        values::<u32>(&net, inbox.id()),
        [1],
        "not even the token before the bad one",
    );
    assert_eq!(net.state_counter(), counter);
    assert_eq!(net.drain_events().count(), 0);
}

#[test]
fn a_rejected_set_tokens_does_not_clear_the_place_on_its_way_out() {
    let (mut net, inbox) = small_net();
    net.apply(Command::extend(inbox, [1, 2]))
        .expect("a u32 place");
    let tokens = ids(&net, inbox.id());
    let _ = net.drain_events();
    let counter = net.state_counter();

    let batch: Vec<Box<dyn Any + Send + Sync>> = vec![Box::new(String::from("nope"))];
    let rejected = net.apply(Command::SetTokens {
        place: inbox.id(),
        tokens: batch,
    });

    assert!(matches!(
        rejected,
        Err(CommandError::Net(NetError::TypeMismatch { .. })),
    ));
    assert_eq!(values::<u32>(&net, inbox.id()), [1, 2]);
    assert_eq!(ids(&net, inbox.id()), tokens);
    assert_eq!(net.state_counter(), counter);
    assert_eq!(net.drain_events().count(), 0);
}

#[test]
fn a_place_id_this_net_does_not_have_is_rejected() {
    let (mut net, inbox) = small_net();
    let (mut bigger, foreign, _transition) = bigger_net();
    let foreign_token = bigger.put(foreign, 1);

    let counter = net.state_counter();
    assert!(matches!(
        net.apply(Command::put(foreign, 7)),
        Err(CommandError::Net(NetError::NoSuchPlace { .. })),
    ));
    assert!(matches!(
        net.apply(Command::remove(foreign.id(), foreign_token)),
        Err(CommandError::Net(NetError::NoSuchPlace { .. })),
    ));

    assert_eq!(net.count(inbox.id()), 0);
    assert_eq!(net.state_counter(), counter);
    assert_eq!(net.drain_events().count(), 0);
}

#[test]
fn a_command_sent_to_the_wrong_net_lands_by_index_the_type_check_is_the_guard() {
    // The accepted consequence of carrying no `NetId`: a `Command` is data on
    // a wire, where the stamp would be meaningless, so `apply` can only ask
    // whether the types agree — and here they do.
    let (_net_a, inbox_a) = small_net();
    let (mut net_b, inbox_b) = small_net();

    net_b
        .apply(Command::put(inbox_a, 7))
        .expect("place #0 of net B also holds u32");
    assert_eq!(values::<u32>(&net_b, inbox_b.id()), [7]);

    // Where the types differ, the one runtime check catches it.
    let mut c = NetBuilder::new();
    let notes = c.place::<String>("Notes");
    let mut net_c = c.build().expect("a well-formed net");
    assert!(matches!(
        net_c.apply(Command::put(inbox_a, 7)),
        Err(CommandError::Net(NetError::TypeMismatch { .. })),
    ));
    assert_eq!(net_c.count(notes.id()), 0);
}

// ----- sameness -----------------------------------------------------------

#[test]
fn the_erased_route_and_the_typed_methods_agree() {
    let (mut typed, typed_inbox) = small_net();
    let (mut applied, applied_inbox) = small_net();

    typed.put(typed_inbox, 1);
    typed.extend(typed_inbox, [2, 3]);
    typed.set_tokens(typed_inbox, [4, 5]);
    let victim = ids(&typed, typed_inbox.id())[0];
    typed.replace(typed_inbox, victim, 6).expect("it is there");
    typed.remove(typed_inbox.id(), victim).expect("it is there");

    applied
        .apply(Command::put(applied_inbox, 1))
        .expect("a u32 place");
    applied
        .apply(Command::extend(applied_inbox, [2, 3]))
        .expect("a u32 place");
    applied
        .apply(Command::set_tokens(applied_inbox, [4, 5]))
        .expect("a u32 place");
    let same_victim = ids(&applied, applied_inbox.id())[0];
    assert_eq!(same_victim, victim, "ids are minted in the same order");
    applied
        .apply(Command::replace(applied_inbox, same_victim, 6))
        .expect("it is there");
    applied
        .apply(Command::remove(applied_inbox.id(), same_victim))
        .expect("it is there");

    assert_eq!(
        values::<u32>(&typed, typed_inbox.id()),
        values::<u32>(&applied, applied_inbox.id()),
    );
    assert_eq!(
        ids(&typed, typed_inbox.id()),
        ids(&applied, applied_inbox.id()),
    );
    assert_eq!(typed.state_counter(), applied.state_counter());
    assert_eq!(
        typed.drain_events().collect::<Vec<_>>(),
        applied.drain_events().collect::<Vec<_>>(),
    );
}

#[test]
fn a_command_crosses_a_thread_and_debugs_without_its_contents() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Command>();

    let (mut net, inbox, _out, double) = net_with_a_transition();
    let token = net.put(inbox, 1);

    assert_eq!(
        format!("{:?}", Command::extend(inbox, [1, 2, 3])),
        "Extend { place: PlaceId(0), tokens: 3 }",
    );
    assert_eq!(
        format!("{:?}", Command::put(inbox, 7)),
        "Extend { place: PlaceId(0), tokens: 1 }",
        "a put is an extend of one",
    );
    assert_eq!(
        format!("{:?}", Command::set_tokens(inbox, Vec::new())),
        "SetTokens { place: PlaceId(0), tokens: 0 }",
    );
    assert_eq!(
        format!("{:?}", Command::remove(inbox.id(), token)),
        format!("Remove {{ place: PlaceId(0), token: {token:?} }}"),
    );
    assert_eq!(
        format!("{:?}", Command::replace(inbox, token, 99_u32)),
        format!("Replace {{ place: PlaceId(0), token: {token:?}, .. }}"),
        "the new value is erased, so it is not printed",
    );
    assert_eq!(
        format!("{:?}", Command::disable(double)),
        "Disable { transition: TransitionId(0) }",
    );

    // And it really does travel.
    let sent = thread::spawn(move || Command::put(inbox, 7))
        .join()
        .expect("no panic");
    assert_eq!(
        format!("{sent:?}"),
        "Extend { place: PlaceId(0), tokens: 1 }",
    );
}

#[test]
fn a_driver_applies_commands_from_its_each_tick_hook() {
    let (mut net, inbox, out, _double) = net_with_a_transition();
    let (sender, receiver) = mpsc::channel::<Command>();

    let feeder = thread::spawn(move || {
        for n in [1_u32, 2, 3] {
            sender
                .send(Command::put(inbox, n))
                .expect("the driver holds the receiver");
        }
    });

    let stop = AtomicBool::new(false);
    let started = Instant::now();
    let summary = drivers::run_indefinitely(
        &mut net,
        Mode::Sequential,
        Duration::from_millis(1),
        &stop,
        OnFailure::Stop,
        |net| {
            // DESIGN §9: channels are the caller's concern, and this hook is
            // where a driver drains one.
            for command in receiver.try_iter() {
                net.apply(command).expect("a u32 command for a u32 place");
            }
            if net.count(out.id()) == 3 || started.elapsed() > Duration::from_secs(10) {
                stop.store(true, Ordering::Relaxed);
            }
        },
    )
    .expect("nothing fails");

    feeder.join().expect("no panic");
    assert_eq!(summary.fired_count, 3, "one firing per command");
    assert_eq!(values::<u32>(&net, out.id()), [2, 4, 6]);
    assert_eq!(net.count(inbox.id()), 0);
}

// ----- the JSON form ------------------------------------------------------

#[cfg(feature = "serde")]
mod json {
    use super::*;

    use petritype::JsonCommand;
    use petritype::serde_json::{self, Value};

    /// `Counts` is serialisable, `Notes` deliberately is not.
    fn json_net() -> (Net, PlaceRef<u32>, PlaceRef<String>, TransitionId) {
        let mut b = NetBuilder::new();
        let counts = b.place::<u32>("Counts");
        let notes = b.place::<String>("Notes");
        let seen = b.place::<u32>("Seen");
        b.serializable(counts);
        let tally = b
            .transition("Tally", (TakeEdge(counts),), (seen,), |n: u32| (n,))
            .id();
        let net = b.build().expect("a well-formed net");
        (net, counts, notes, tally)
    }

    /// A `TokenId` with no net to get one from: the only public way to make
    /// one, which is itself part of what this slice added.
    fn token_id(id: u64) -> TokenId {
        serde_json::from_str::<TokenId>(&id.to_string()).expect("a bare number")
    }

    #[test]
    fn the_wire_shape_is_a_tagged_object() {
        let extend = JsonCommand::Extend {
            place: String::from("Inbox"),
            tokens: vec![Value::from(1), Value::from(2), Value::from(3)],
        };
        let text = serde_json::to_string(&extend).expect("plain data");
        assert_eq!(
            text,
            r#"{"command":"extend","place":"Inbox","tokens":[1,2,3]}"#,
        );
        assert_eq!(
            serde_json::from_str::<JsonCommand>(&text).expect("it round-trips"),
            extend,
        );

        let disable = JsonCommand::Disable {
            transition: String::from("Double"),
        };
        let text = serde_json::to_string(&disable).expect("plain data");
        assert_eq!(text, r#"{"command":"disable","transition":"Double"}"#);
        assert_eq!(
            serde_json::from_str::<JsonCommand>(&text).expect("it round-trips"),
            disable,
        );

        // Multi-word variants are `snake_case`, and a `TokenId` is a bare
        // number rather than a wrapper.
        assert_eq!(
            serde_json::to_string(&JsonCommand::SetTokens {
                place: String::from("Inbox"),
                tokens: Vec::new(),
            })
            .expect("plain data"),
            r#"{"command":"set_tokens","place":"Inbox","tokens":[]}"#,
        );
        assert_eq!(
            serde_json::to_string(&JsonCommand::Replace {
                place: String::from("Inbox"),
                token: token_id(4),
                value: Value::from(9),
            })
            .expect("plain data"),
            r#"{"command":"replace","place":"Inbox","token":4,"value":9}"#,
        );
    }

    #[test]
    fn apply_json_lands_tokens_in_a_serializable_place() {
        let (mut net, counts, _notes, _tally) = json_net();

        let command: JsonCommand =
            serde_json::from_str(r#"{"command":"extend","place":"Counts","tokens":[1,2,3]}"#)
                .expect("the documented shape");
        net.apply_json(command).expect("Counts is serializable");

        assert_eq!(values::<u32>(&net, counts.id()), [1, 2, 3]);
        let tokens = ids(&net, counts.id());
        assert_eq!(
            net.drain_events().collect::<Vec<_>>(),
            tokens
                .iter()
                .map(|&token| Event::Added {
                    place: counts.id(),
                    token
                })
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            net.json(counts.id(), tokens[2]).expect("serializable"),
            Value::from(3),
        );
    }

    #[test]
    fn a_place_that_is_not_serializable_has_no_decoder() {
        let (mut net, _counts, notes, _tally) = json_net();

        let rejected = Command::from_json(
            &net,
            JsonCommand::Extend {
                place: String::from("Notes"),
                tokens: vec![Value::from("hello")],
            },
        );
        match rejected {
            Err(CommandError::NotSerializable { place }) => assert_eq!(place, notes.id()),
            other => panic!("expected NotSerializable, got {other:?}"),
        }

        // And nothing reached the net.
        assert_eq!(net.count(notes.id()), 0);
        assert_eq!(net.state_counter(), 0);
        assert_eq!(net.drain_events().count(), 0);
    }

    #[test]
    fn unknown_names_are_rejected_before_the_net_is_asked() {
        let (net, _counts, _notes, _tally) = json_net();

        let rejected = Command::from_json(
            &net,
            JsonCommand::Extend {
                place: String::from("Nowhere"),
                tokens: vec![Value::from(1)],
            },
        );
        match rejected {
            Err(CommandError::NoSuchPlaceName { name }) => assert_eq!(name, "Nowhere"),
            other => panic!("expected NoSuchPlaceName, got {other:?}"),
        }

        let rejected = Command::from_json(
            &net,
            JsonCommand::Disable {
                transition: String::from("Nothing"),
            },
        );
        match rejected {
            Err(CommandError::NoSuchTransitionName { name }) => assert_eq!(name, "Nothing"),
            other => panic!("expected NoSuchTransitionName, got {other:?}"),
        }
    }

    #[test]
    fn a_token_that_does_not_parse_is_rejected_and_nothing_is_applied() {
        let (mut net, counts, _notes, _tally) = json_net();

        let rejected = net.apply_json(JsonCommand::Extend {
            place: String::from("Counts"),
            tokens: vec![Value::from(1), Value::from("seven")],
        });
        match rejected {
            Err(CommandError::InvalidToken {
                place, expected, ..
            }) => {
                assert_eq!(place, counts.id());
                assert_eq!(expected, "u32");
            }
            other => panic!("expected InvalidToken, got {other:?}"),
        }

        assert_eq!(net.count(counts.id()), 0, "not even the token before it");
        assert_eq!(net.state_counter(), 0);
        assert_eq!(net.drain_events().count(), 0);
    }

    #[test]
    fn to_json_is_the_way_back_out() {
        let (net, counts, notes, tally) = json_net();

        assert_eq!(
            Command::extend(counts, [1, 2])
                .to_json(&net)
                .expect("Counts is serializable"),
            JsonCommand::Extend {
                place: String::from("Counts"),
                tokens: vec![Value::from(1), Value::from(2)],
            },
        );
        assert_eq!(
            Command::disable(tally).to_json(&net).expect("a transition"),
            JsonCommand::Disable {
                transition: String::from("Tally"),
            },
        );

        // A place with no encoder cannot be put on a wire either.
        let rejected = Command::put(notes, String::from("hi")).to_json(&net);
        match rejected {
            Err(CommandError::NotSerializable { place }) => assert_eq!(place, notes.id()),
            other => panic!("expected NotSerializable, got {other:?}"),
        }
    }

    #[test]
    fn replace_through_json_keeps_the_id() {
        let (mut net, counts, _notes, _tally) = json_net();
        net.apply_json(JsonCommand::Extend {
            place: String::from("Counts"),
            tokens: vec![Value::from(1)],
        })
        .expect("Counts is serializable");
        let token = ids(&net, counts.id())[0];
        let _ = net.drain_events();

        net.apply_json(JsonCommand::Replace {
            place: String::from("Counts"),
            token,
            value: Value::from(99),
        })
        .expect("the token is there");

        assert_eq!(ids(&net, counts.id()), [token], "the same token");
        assert_eq!(
            net.drain_events().collect::<Vec<_>>(),
            [Event::Mutated {
                place: counts.id(),
                token
            }],
        );
        assert_eq!(
            net.json(counts.id(), token).expect("serializable"),
            Value::from(99),
        );
    }

    #[test]
    fn ids_are_bare_numbers_on_a_wire() {
        let (net, counts, notes, tally) = json_net();

        assert_eq!(
            serde_json::to_string(&counts.id()).expect("a newtype over u32"),
            "0",
        );
        assert_eq!(
            serde_json::to_string(&tally).expect("a newtype over u32"),
            "0",
        );
        assert_eq!(
            serde_json::to_string(&token_id(12)).expect("a newtype over u64"),
            "12",
        );
        assert_eq!(
            serde_json::from_str::<PlaceId>("1").expect("a bare number"),
            notes.id(),
        );
        assert_eq!(
            serde_json::from_str::<TransitionId>("0").expect("a bare number"),
            net.transition_named("Tally").expect("it is there"),
        );
    }
}
