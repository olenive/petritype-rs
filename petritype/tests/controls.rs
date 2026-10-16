//! Integration tests for net-declared controls and token styles (DESIGN §18,
//! 2026-10-16, "net-declared controls"; the `control` and `style` modules).
//!
//! A net declares, as data, how its tokens look and which buttons it wants;
//! a press runs code written beside the net, through the net's own API, so
//! that everything it does is an ordinary event. What is tested here: a
//! style colours the tokens of the place that declared it and no other, the
//! trait and the closure say the same, the place style is shape; each
//! control constructor, each kind's value check, an unknown name, a handler
//! that refuses, a handler that reads the net before it changes it, and the
//! events a press leaves in the log.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use petritype::control::{Action, Control, ControlKind, Node, Pressed, Value};
use petritype::style::{Fill, PlaceStyle, Rgb, Style, TokenStyle};
use petritype::{BuildError, Event, Net, NetBuilder, PlaceRef, PressError, TakeEdge, TokenId};

// ----- styles ------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum Lamp {
    Red,
    Green,
    Off,
}

impl TokenStyle for Lamp {
    fn style(&self) -> Style {
        match self {
            Lamp::Red => Style::default().with_colour(Rgb(0xff, 0x63, 0x47)),
            Lamp::Green => Style::default().with_colour(Rgb(0x32, 0xcd, 0x32)),
            Lamp::Off => Style::default(),
        }
    }
}

/// The first token of a place.
fn first<T>(net: &Net, place: PlaceRef<T>) -> TokenId {
    net.tokens(place.id()).next().expect("a token").0
}

#[test]
fn a_styled_place_colours_its_tokens() {
    let mut b = NetBuilder::new();
    let lamps = b.place::<Lamp>("Lamps");
    b.styled(lamps);
    b.seed(lamps, [Lamp::Red, Lamp::Green, Lamp::Off]);
    let net = b.build().unwrap();

    let colours: Vec<Option<String>> = net
        .tokens(lamps.id())
        .map(|(token, _)| {
            net.style(lamps.id(), token)
                .expect("a styled place gives every token a style")
                .colour
                .map(|rgb| rgb.to_string())
        })
        .collect();
    assert_eq!(
        colours,
        [Some("#ff6347".to_owned()), Some("#32cd32".to_owned()), None],
        "a token whose style has no colour has none"
    );
}

#[test]
fn an_unstyled_place_gives_none() {
    let mut b = NetBuilder::new();
    let lamps = b.place::<Lamp>("Lamps");
    b.seed(lamps, [Lamp::Red]);
    let styled = b.place::<Lamp>("Styled");
    b.styled(styled);
    let net = b.build().unwrap();

    let red = first(&net, lamps);
    assert_eq!(
        net.style(lamps.id(), red),
        None,
        "Lamp is TokenStyle, but the place did not opt in"
    );
    assert_eq!(
        net.style(styled.id(), red),
        None,
        "no such token in that place"
    );
}

#[test]
fn the_trait_and_the_closure_say_the_same() {
    let mut b = NetBuilder::new();
    let by_trait = b.place::<Lamp>("By trait");
    let by_closure = b.place::<Lamp>("By closure");
    b.styled(by_trait);
    b.style(by_closure, |lamp: &Lamp| lamp.style());
    for place in [by_trait, by_closure] {
        b.seed(place, [Lamp::Green, Lamp::Off]);
    }
    let net = b.build().unwrap();

    let styles = |place: PlaceRef<Lamp>| -> Vec<Option<Style>> {
        net.tokens(place.id())
            .map(|(token, _)| net.style(place.id(), token))
            .collect()
    };
    assert_eq!(styles(by_trait), styles(by_closure));
    assert_eq!(
        styles(by_trait)[0].as_ref().unwrap().colour,
        Some(Rgb(0x32, 0xcd, 0x32))
    );
}

#[test]
fn a_style_follows_the_token_as_it_changes() {
    let mut b = NetBuilder::new();
    let lamp = b.place::<Lamp>("Lamp");
    let out = b.place::<Lamp>("Out");
    b.styled(lamp);
    b.styled(out);
    b.seed(lamp, [Lamp::Red]);
    b.transition(
        "Switch",
        (TakeEdge(lamp),),
        (out,),
        |_: Lamp| (Lamp::Green,),
    );
    let mut net = b.build().unwrap();

    net.step().unwrap();
    let green = first(&net, out);
    assert_eq!(
        net.style(out.id(), green).unwrap().colour,
        Some(Rgb(0x32, 0xcd, 0x32))
    );
}

#[test]
fn the_topology_carries_the_place_style() {
    let mut b = NetBuilder::new();
    let light = b.place::<Lamp>("Light");
    let plain = b.place::<u32>("Plain");
    b.place_style(light, PlaceStyle::fill_token());
    let net = b.build().unwrap();

    let styles: Vec<_> = net.places().map(|(_, info)| info.style).collect();
    assert_eq!(styles, [Some(PlaceStyle::fill_token()), None]);
    assert_eq!(styles[0].unwrap().fill, Fill::Token);
    let topology = net.topology();
    assert_eq!(
        topology.places[light.id().index() as usize].style,
        Some(PlaceStyle::fill_token())
    );
    assert_eq!(topology.places[plain.id().index() as usize].style, None);
    let snapshot = net.snapshot().unwrap();
    assert_eq!(snapshot.topology(), topology, "a snapshot carries it too");
}

#[cfg(feature = "serde")]
#[test]
fn a_place_style_serialises_as_fill_token() {
    let mut b = NetBuilder::new();
    let light = b.place::<Lamp>("Light");
    let _plain = b.place::<u32>("Plain");
    b.place_style(light, PlaceStyle::fill_token());
    let net = b.build().unwrap();

    let json = petritype::serde_json::to_value(net.topology()).unwrap();
    assert_eq!(
        json["places"][0]["style"],
        petritype::serde_json::json!({"fill": "token"})
    );
    assert!(
        json["places"][1].get("style").is_none(),
        "written only when there is one"
    );
}

// ----- controls ----------------------------------------------------------------

/// A counter with one control of every kind and every action.
struct Panel {
    net: Net,
    inbox: PlaceRef<String>,
    out: PlaceRef<String>,
    /// What the last press of a kind's control was handed.
    seen: Arc<std::sync::Mutex<Vec<Value>>>,
}

fn panel() -> Panel {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut b = NetBuilder::new();
    let inbox = b.place::<String>("Inbox");
    let out = b.place::<String>("Out");
    let pass = b
        .transition("Pass", (TakeEdge(inbox),), (out,), |s: String| (s,))
        .id();
    let record = |seen: &Arc<std::sync::Mutex<Vec<Value>>>| {
        let seen = Arc::clone(seen);
        move |_: &mut Net, value: Value| {
            seen.lock().unwrap().push(value);
            Ok(Pressed::Applied)
        }
    };
    b.control(Control::button("button", "Button", record(&seen)))
        .control(Control::toggle("toggle", "Toggle", false, record(&seen)))
        .control(Control::slider(
            "slider",
            "Slider",
            0.0,
            10.0,
            0.5,
            2.5,
            record(&seen),
        ))
        .control(Control::select(
            "select",
            "Select",
            ["low", "high"],
            "low",
            record(&seen),
        ))
        .control(Control::text("text", "Text", Some("hint"), record(&seen)))
        .control(Control::inject(
            "inject",
            "Inject",
            inbox,
            |value| match value {
                Value::Text(text) => text,
                _ => "nothing".to_owned(),
            },
        ))
        .control(Control::fire("fire", "Fire", pass))
        .control(Control::enable("enable", "Enable", pass));
    Panel {
        net: b.build().unwrap(),
        inbox,
        out,
        seen,
    }
}

#[test]
fn every_constructor_declares_its_kind_node_action_and_initial() {
    let panel = panel();
    let net = &panel.net;
    let pass = net.transition_named("Pass").unwrap();
    let rows: Vec<_> = net
        .controls()
        .iter()
        .map(|c| {
            (
                c.name.as_str(),
                c.label.as_str(),
                c.kind.clone(),
                c.node,
                c.action,
                c.initial.clone(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("button", "Button", ControlKind::Button, None, None, None),
            (
                "toggle",
                "Toggle",
                ControlKind::Toggle,
                None,
                None,
                Some(Value::Bool(false))
            ),
            (
                "slider",
                "Slider",
                ControlKind::Slider {
                    start: 0.0,
                    stop: 10.0,
                    step: 0.5
                },
                None,
                None,
                Some(Value::Number(2.5)),
            ),
            (
                "select",
                "Select",
                ControlKind::Select {
                    options: vec!["low".into(), "high".into()]
                },
                None,
                None,
                Some(Value::Text("low".into())),
            ),
            (
                "text",
                "Text",
                ControlKind::Text {
                    placeholder: Some("hint".into())
                },
                None,
                None,
                None,
            ),
            (
                "inject",
                "Inject",
                ControlKind::Button,
                Some(Node::Place(panel.inbox.id())),
                Some(Action::Inject),
                None,
            ),
            (
                "fire",
                "Fire",
                ControlKind::Button,
                Some(Node::Transition(pass)),
                Some(Action::Fire),
                None
            ),
            (
                "enable",
                "Enable",
                ControlKind::Toggle,
                Some(Node::Transition(pass)),
                Some(Action::Enable),
                Some(Value::Bool(true)),
            ),
        ]
    );
}

#[test]
fn each_kind_takes_its_own_value_and_nothing_else() {
    let mut panel = panel();
    let net = &mut panel.net;
    let accepted = [
        ("button", Value::None),
        ("toggle", Value::Bool(true)),
        ("slider", Value::Number(10.0)),
        ("select", Value::Text("high".into())),
        ("text", Value::Text(String::new())),
    ];
    for (name, value) in accepted.clone() {
        assert_eq!(net.press(name, value), Ok(Pressed::Applied), "{name}");
    }
    assert_eq!(
        *panel.seen.lock().unwrap(),
        accepted.map(|(_, value)| value),
        "each handler is handed the value pressed"
    );

    let refused = [
        ("button", Value::Bool(true)),
        ("toggle", Value::Text("on".into())),
        ("slider", Value::Number(10.5)),
        ("slider", Value::Number(-0.5)),
        ("slider", Value::Text("5".into())),
        ("select", Value::Text("medium".into())),
        ("select", Value::Number(1.0)),
        ("text", Value::Number(7.0)),
        ("text", Value::None),
        ("inject", Value::Text("a button".into())),
        ("enable", Value::None),
    ];
    for (name, value) in refused {
        let error = net.press(name, value.clone()).unwrap_err();
        assert!(
            matches!(&error, PressError::WrongKind { control, .. } if control == name),
            "{name} with {value:?}: {error}"
        );
    }
    assert_eq!(
        panel.seen.lock().unwrap().len(),
        5,
        "no handler ran for a wrong value"
    );
    assert_eq!(
        net.drain_events().count(),
        0,
        "these handlers record and touch nothing"
    );
}

#[test]
fn an_unknown_control_is_an_error_and_changes_nothing() {
    let mut panel = panel();
    let error = panel.net.press("nope", Value::None).unwrap_err();
    assert_eq!(
        error,
        PressError::UnknownControl {
            name: "nope".into()
        }
    );
    assert_eq!(error.to_string(), "no control named \"nope\"");
    assert_eq!(panel.net.state_counter(), 0);
}

#[test]
fn a_duplicate_control_name_does_not_build() {
    let mut b = NetBuilder::new();
    let place = b.place::<u32>("Place");
    b.control(Control::inject("same", "One", place, |_| 1));
    b.control(Control::inject("same", "Two", place, |_| 2));
    match b.build() {
        Err(BuildError::DuplicateControlName { name }) => assert_eq!(name, "same"),
        other => panic!("expected DuplicateControlName, got {other:?}"),
    }
}

#[test]
fn inject_puts_what_make_returns() {
    let mut panel = panel();
    let net = &mut panel.net;
    assert_eq!(net.press("inject", Value::None), Ok(Pressed::Applied));
    let token = first(net, panel.inbox);
    assert_eq!(
        net.token::<String>(panel.inbox.id(), token).unwrap(),
        "nothing"
    );
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Added {
            place: panel.inbox.id(),
            token
        }],
        "an ordinary Added"
    );
}

#[test]
fn an_inject_given_another_kind_takes_that_kind_s_value() {
    let mut b = NetBuilder::new();
    let names = b.place::<String>("Names");
    b.control(
        Control::inject("name", "Name", names, |value| match value {
            Value::Text(text) => text,
            _ => unreachable!("a text control is handed text"),
        })
        .with_kind(ControlKind::Text {
            placeholder: Some("name".into()),
        }),
    );
    let mut net = b.build().unwrap();
    assert!(matches!(
        net.press("name", Value::None),
        Err(PressError::WrongKind { .. })
    ));
    net.press("name", Value::Text("Ada".into())).unwrap();
    assert_eq!(
        net.token::<String>(names.id(), first(&net, names)).unwrap(),
        "Ada"
    );
}

#[test]
fn fire_fires_or_says_why_not() {
    let mut panel = panel();
    let net = &mut panel.net;
    let pass = net.transition_named("Pass").unwrap();
    assert_eq!(
        net.press("fire", Value::None),
        Ok(Pressed::Nothing("Pass is not fireable".into()))
    );
    assert_eq!(
        net.state_counter(),
        0,
        "a refused fire leaves the net untouched"
    );

    net.press("inject", Value::None).unwrap();
    let car = first(net, panel.inbox);
    net.drain_events().count();
    assert_eq!(net.press("fire", Value::None), Ok(Pressed::Applied));
    let passed = first(net, panel.out);
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Fired {
            transition: pass,
            consumed: vec![(panel.inbox.id(), car)],
            produced: vec![(panel.out.id(), passed)],
        }],
        "an ordinary Fired, as Net::fire gives"
    );
}

#[test]
fn a_fire_whose_body_fails_is_refused_with_the_failure() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [1]);
    let fail = b
        .transition(
            "Fail",
            (TakeEdge(inbox),),
            (out,),
            |_: u32| -> Result<(u32,), std::io::Error> { Err(std::io::Error::other("no")) },
        )
        .id();
    b.control(Control::fire("fail", "Fail", fail));
    let mut net = b.build().unwrap();
    match net.press("fail", Value::None) {
        Err(PressError::Refused(message)) => assert!(message.contains("no"), "{message}"),
        other => panic!("expected Refused, got {other:?}"),
    }
    assert!(
        net.drain_events()
            .any(|event| matches!(event, Event::Failed { .. })),
        "the failure is in the log as for any firing"
    );
}

#[test]
fn enable_switches_the_transition() {
    let mut panel = panel();
    let net = &mut panel.net;
    let pass = net.transition_named("Pass").unwrap();
    assert_eq!(
        net.press("enable", Value::Bool(false)),
        Ok(Pressed::Applied)
    );
    assert!(!net.is_enabled(pass));
    assert_eq!(net.press("enable", Value::Bool(true)), Ok(Pressed::Applied));
    assert!(net.is_enabled(pass));
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [
            Event::Toggled {
                transition: pass,
                enabled: false
            },
            Event::Toggled {
                transition: pass,
                enabled: true
            },
        ]
    );
}

#[test]
fn a_handler_that_refuses_is_refused() {
    let mut b = NetBuilder::new();
    b.control(Control::button("no", "No", |_, _| {
        Err("not today".to_owned())
    }));
    let mut net = b.build().unwrap();
    let error = net.press("no", Value::None).unwrap_err();
    assert_eq!(error, PressError::Refused("not today".into()));
    assert_eq!(error.to_string(), "not today");
}

#[test]
fn a_handler_reads_the_net_and_changes_it() {
    // A text box that names a car after the cars there are when it is
    // empty, as the traffic light's `car` does.
    let mut b = NetBuilder::new();
    let waiting = b.place::<String>("Waiting");
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    b.control(Control::text(
        "car",
        "+ Car",
        Some("name"),
        move |net, value| {
            counted.fetch_add(1, Ordering::Relaxed);
            let name = match value {
                Value::Text(name) if !name.is_empty() => name,
                _ => format!("Car {}", net.count(waiting.id()) + 1),
            };
            net.put(waiting, name);
            Ok(Pressed::Applied)
        },
    ));
    let mut net = b.build().unwrap();

    for value in ["", "", "Taxi", ""] {
        net.press("car", Value::Text(value.into())).unwrap();
    }
    let cars: Vec<&String> = net
        .tokens(waiting.id())
        .map(|(token, _)| net.token::<String>(waiting.id(), token).unwrap())
        .collect();
    assert_eq!(cars, ["Car 1", "Car 2", "Taxi", "Car 4"]);
    assert_eq!(calls.load(Ordering::Relaxed), 4);
    assert_eq!(net.state_counter(), 4, "four Added, one per press");
}

#[test]
fn an_inject_into_another_net_s_place_is_refused() {
    let mut other = NetBuilder::new();
    let elsewhere = other.place::<u32>("Elsewhere");
    let mut b = NetBuilder::new();
    let _here = b.place::<u32>("Here");
    b.control(Control::inject("stray", "Stray", elsewhere, |_| 1));
    let mut net = b.build().unwrap();
    assert!(matches!(
        net.press("stray", Value::None),
        Err(PressError::Refused(_))
    ));
    assert_eq!(net.state_counter(), 0);
}

#[test]
fn controls_are_debug_without_their_handler() {
    let panel = panel();
    let text = format!("{:?}", panel.net.controls()[0]);
    assert!(text.starts_with("Control { name: \"button\""), "{text}");
    assert!(format!("{:?}", panel.net).contains("controls: [\"button\""));
}
