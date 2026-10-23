//! Traffic light: a drain edge, which takes every token of a place that may
//! be empty, and a snapshot edge, the plain read (DESIGN §4.4).
//!
//! The drain half of the edges-on-empty-places example, split from the shop
//! on 2026-10-16, built as the conformance net `traffic-light` of the shared
//! protocol (`PROTOCOL.md` §11), which lives in [`net`] so that a runner can
//! carry the same file. Cars wait at a light that cycles red, green and
//! amber, and all go when it turns green. The light is one place, `Light`,
//! and its one token is the colour. Two transitions: `Go` moves the cars,
//! `Change` changes the light.
//!
//! `Go` reads `Waiting` through a [`DrainEdge`]: every car waiting, all
//! consumed in one firing. It reads `Light` through a [`SnapshotEdge`], the
//! simpler kind of read: the colour is cloned, not taken, and the light
//! keeps its token. A drain accepts an empty place, so `Go`'s guard asks for
//! someone waiting as well as for green: without it `Go`, declared first,
//! would fire at every green taking nothing and the light would never
//! change. `Change`, unguarded, takes the light and writes the next colour
//! back, and is always fireable.
//!
//! The net also declares, as data a viewer draws without knowing it, how
//! the light looks (its token a [`Colour`](net::Colour) drawn as the lamp
//! it lights, its place filled with that colour) and two controls, `car`
//! (`+ Car`, a text box) and `change` (`Change`, a button), which a press
//! runs with code written beside the net.
//!
//! `main` runs the script of the shared transcript `08_traffic_light.json`
//! (the light changes to green and to amber with nobody waiting, two cars
//! arrive at amber and wait through red, both go in one firing when it
//! turns green, a third arrives at green and goes straight through, the
//! light changes to amber, and then the controls are pressed: `change` turns
//! it red, `car` with nothing typed adds `Car 4`, and with `Taxi` a taxi)
//! and prints `Light`, `Waiting` and `Passed` after each step, checking what
//! fired and what is fireable as the transcript does, then the controls as a
//! legend lists them and the light's colour. The light never stops: `Change`
//! is always fireable. The tests below hold the table, the net's shape as
//! §11 has it, its controls and its colours, what the drain does to the
//! event log, what `Go`'s guard holds back and what it would let through
//! without it, and that the light goes round by itself.
//!
//! [`DrainEdge`]: petritype::DrainEdge
//! [`SnapshotEdge`]: petritype::SnapshotEdge

// A crate root's `mod` looks beside it, so the path is spelled out: the net
// lives in `traffic_light/net.rs`, the file a runner's example can include
// too.
#[path = "traffic_light/net.rs"]
mod net;

use std::error::Error;

use petritype::control::{ControlKind, Node, Value};
use petritype::{Net, PlaceRef, TransitionId};

use net::{Colour, TrafficLight, build_traffic_light, colour};

/// One row of the table: what happened, and `Light`, `Waiting` and `Passed`
/// after it.
type Row = (&'static str, [Vec<String>; 3]);

/// The strings in a place, in queue order.
fn strings(net: &Net, place: PlaceRef<String>) -> Vec<String> {
    net.tokens(place.id())
        .filter_map(|(token, _)| net.token::<String>(place.id(), token).cloned())
        .collect()
}

/// The colours in `Light`, in queue order, as their names: one, always.
fn colours(net: &Net, place: PlaceRef<Colour>) -> Vec<String> {
    net.tokens(place.id())
        .filter_map(|(token, _)| net.token::<Colour>(place.id(), token))
        .map(Colour::to_string)
        .collect()
}

/// The three places as they stand.
fn row(after: &'static str, light: &TrafficLight) -> Row {
    (
        after,
        [
            colours(&light.net, light.light),
            strings(&light.net, light.waiting),
            strings(&light.net, light.passed),
        ],
    )
}

/// Transitions' names, in the order given.
fn names<'a>(net: &'a Net, transitions: &[TransitionId]) -> Vec<&'a str> {
    transitions
        .iter()
        .filter_map(|id| {
            net.transitions()
                .find(|(transition, _)| transition == id)
                .map(|(_, info)| info.name)
        })
        .collect()
}

/// Checks that what is fireable now is `expected`, in declaration order.
fn fireable(light: &TrafficLight, expected: &[TransitionId]) -> Result<(), Box<dyn Error>> {
    let now = light.net.fireable();
    if now == expected {
        Ok(())
    } else {
        Err(format!(
            "expected {:?} fireable, got {:?}",
            names(&light.net, expected),
            names(&light.net, &now)
        )
        .into())
    }
}

/// Steps the net once with the default selector, checks that `expected` is
/// what fired, and that `then` is fireable after it.
fn step(
    light: &mut TrafficLight,
    expected: TransitionId,
    then: &[TransitionId],
) -> Result<(), Box<dyn Error>> {
    match light.net.step()? {
        Some(fired) if fired == expected => fireable(light, then),
        other => Err(format!("expected {expected:?} to fire, got {other:?}").into()),
    }
}

/// Puts cars into `Waiting`, as the wire's `inject` does, and checks that
/// `then` is fireable after it.
fn arrive(
    light: &mut TrafficLight,
    cars: &[&str],
    then: &[TransitionId],
) -> Result<(), Box<dyn Error>> {
    light
        .net
        .extend(light.waiting, cars.iter().map(|&car| car.to_owned()));
    fireable(light, then)
}

/// The transcript's script on a fresh light, one row per step and per
/// arrival.
fn script(light: &mut TrafficLight) -> Result<Vec<Row>, Box<dyn Error>> {
    let (go, change) = (light.go, light.change);
    let mut rows = Vec::new();

    // Green with nobody waiting: `Go` is held by its guard, and the light
    // changes on.
    step(light, change, &[change])?;
    rows.push(row("Change", light));
    step(light, change, &[change])?;
    rows.push(row("Change", light));

    // At amber and at red the cars wait: `Go` is held by its guard.
    arrive(light, &["Car 1", "Car 2"], &[change])?;
    rows.push(row("Car 1 and Car 2 arrive", light));
    step(light, change, &[change])?;
    rows.push(row("Change", light));

    // Green with cars waiting: both are fireable, and `Go`, the first
    // declared, fires, draining both.
    step(light, change, &[go, change])?;
    rows.push(row("Change", light));
    step(light, go, &[change])?;
    rows.push(row("Go (two waiting)", light));

    arrive(light, &["Car 3"], &[go, change])?;
    rows.push(row("Car 3 arrives", light));
    step(light, go, &[change])?;
    rows.push(row("Go (one waiting)", light));
    step(light, change, &[change])?;
    rows.push(row("Change", light));

    // The net's own controls, as a viewer presses them: `change` fires
    // `Change`, and `car` adds a car, named `Car N` when nothing is typed.
    press(light, "change", Value::None, &[change])?;
    rows.push(row("press change", light));
    press(light, "car", Value::Text(String::new()), &[change])?;
    rows.push(row("press car \"\"", light));
    press(light, "car", Value::Text("Taxi".to_owned()), &[change])?;
    rows.push(row("press car \"Taxi\"", light));
    Ok(rows)
}

/// Presses a control of the net with `value`, checks that it applied
/// something, and that `then` is fireable after it.
fn press(
    light: &mut TrafficLight,
    control: &str,
    value: Value,
    then: &[TransitionId],
) -> Result<(), Box<dyn Error>> {
    match light.net.press(control, value)? {
        petritype::control::Pressed::Applied => fireable(light, then),
        other => Err(format!("expected {control} to apply, got {other:?}").into()),
    }
}

/// A node by name, as a legend would show it.
fn node_name(net: &Net, node: Option<Node>) -> String {
    match node {
        Some(Node::Place(place)) => net
            .places()
            .find(|(id, _)| *id == place)
            .map_or_else(String::new, |(_, info)| info.name.to_owned()),
        Some(Node::Transition(transition)) => names(net, &[transition]).concat(),
        None => String::new(),
    }
}

/// A list as the table writes it: `[Car 1, Car 2]`.
fn list(items: &[String]) -> String {
    format!("[{}]", items.join(", "))
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut light = build_traffic_light()?;
    // Left to right, like the road: the cars flowing from `Waiting` to
    // `Passed`, the light's loop beside them.
    let road = petritype_dot::Style {
        rankdir: petritype_dot::RankDir::LR,
        ..petritype_dot::Style::default()
    };
    println!("{}", petritype_dot::dot_with(&light.net, &road));

    println!("| After | Light | Waiting | Passed |");
    println!("|---|---|---|---|");
    for (after, [colour, waiting, passed]) in script(&mut light)? {
        println!(
            "| {after} | {} | {} | {} |",
            list(&colour),
            list(&waiting),
            list(&passed)
        );
    }
    let net = &light.net;
    println!("\nControls, as a viewer's legend lists them:");
    for control in net.controls() {
        let hint = match &control.kind {
            ControlKind::Text {
                placeholder: Some(hint),
            } => format!(", hint {hint:?}"),
            _ => String::new(),
        };
        let widget = match control.kind.name() {
            "text" => "text box",
            other => other,
        };
        println!(
            "- {} ({}): a {widget}{hint}, acting on {}",
            control.name,
            control.label,
            node_name(net, control.node),
        );
    }
    if let Err(refused) = light.net.press("car", Value::Number(7.0)) {
        println!("A number for car is refused: {refused}");
    }
    let net = &light.net;
    let lamp = colour(net, light.light).expect("the light holds its colour");
    let (token, _) = net.tokens(light.light.id()).next().expect("one token");
    let hex = net
        .style(light.light.id(), token)
        .and_then(|style| style.colour)
        .map_or_else(|| "nothing".to_owned(), |rgb| rgb.to_string());
    let filled = net
        .places()
        .any(|(place, info)| place == light.light.id() && info.style.is_some());
    println!(
        "The light is {lamp}, drawn {hex}{}.",
        if filled {
            ", and `Light` is filled with it"
        } else {
            ""
        }
    );
    println!(
        "Never quiescent: fireable now {:?}",
        names(net, &net.fireable())
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use net::{car_name, next_colour};
    use petritype::control::{Action, Pressed};
    use petritype::serde_json::{self, json};
    use petritype::style::{PlaceStyle, Rgb, TokenStyle};
    use petritype::{
        BuildError, DrainEdge, EdgeKind, Event, JsonCommand, NetBuilder, PressError, SnapshotEdge,
        TakeEdge,
    };

    /// The table: after each step, each arrival and each press, `Light`,
    /// `Waiting` and `Passed`.
    const TABLE: [(&str, [&[&str]; 3]); 12] = [
        ("Change", [&["green"], &[], &[]]),
        ("Change", [&["amber"], &[], &[]]),
        (
            "Car 1 and Car 2 arrive",
            [&["amber"], &["Car 1", "Car 2"], &[]],
        ),
        ("Change", [&["red"], &["Car 1", "Car 2"], &[]]),
        ("Change", [&["green"], &["Car 1", "Car 2"], &[]]),
        ("Go (two waiting)", [&["green"], &[], &["Car 1", "Car 2"]]),
        (
            "Car 3 arrives",
            [&["green"], &["Car 3"], &["Car 1", "Car 2"]],
        ),
        (
            "Go (one waiting)",
            [&["green"], &[], &["Car 1", "Car 2", "Car 3"]],
        ),
        ("Change", [&["amber"], &[], &["Car 1", "Car 2", "Car 3"]]),
        (
            "press change",
            [&["red"], &[], &["Car 1", "Car 2", "Car 3"]],
        ),
        (
            "press car \"\"",
            [&["red"], &["Car 4"], &["Car 1", "Car 2", "Car 3"]],
        ),
        (
            "press car \"Taxi\"",
            [&["red"], &["Car 4", "Taxi"], &["Car 1", "Car 2", "Car 3"]],
        ),
    ];

    /// The ids of the tokens in `Light`, in queue order: one, always.
    fn lamps(light: &TrafficLight) -> Vec<petritype::TokenId> {
        light
            .net
            .tokens(light.light.id())
            .map(|(id, _)| id)
            .collect()
    }

    /// The light set to `colour`, as a host's `set_tokens` would.
    fn set_colour(light: &mut TrafficLight, colour: Colour) {
        light.net.set_tokens(light.light, [colour]);
    }

    /// The colour the light's token is drawn in, as `#rrggbb`.
    fn drawn(light: &TrafficLight) -> Option<String> {
        let (token, _) = light.net.tokens(light.light.id()).next()?;
        let style = light.net.style(light.light.id(), token)?;
        style.colour.map(|rgb| rgb.to_string())
    }

    #[test]
    fn the_net_is_the_conformance_net_of_protocol_section_11() {
        let light = build_traffic_light().expect("the net builds");
        let net = &light.net;
        let places: Vec<(&str, bool, usize, Option<PlaceStyle>)> = net
            .places()
            .map(|(place, info)| (info.name, info.serializable, net.count(place), info.style))
            .collect();
        assert_eq!(
            places,
            [
                ("Light", true, 1, Some(PlaceStyle::fill_token())),
                ("Waiting", true, 0, None),
                ("Passed", true, 0, None),
            ]
        );
        assert_eq!(colours(net, light.light), ["red"]);

        let transitions: Vec<(&str, &[_], &[_], bool)> = net
            .transitions()
            .map(|(_, info)| (info.name, info.inputs, info.outputs, info.enabled))
            .collect();
        assert_eq!(
            transitions,
            [
                (
                    "Go",
                    &[
                        (light.waiting.id(), EdgeKind::Drain),
                        (light.light.id(), EdgeKind::Snapshot)
                    ][..],
                    &[light.passed.id()][..],
                    true
                ),
                (
                    "Change",
                    &[(light.light.id(), EdgeKind::Take)][..],
                    &[light.light.id()][..],
                    true
                ),
            ]
        );
        assert!(
            net.transitions()
                .all(|(_, info)| info.positional && info.params.is_empty() && info.detachable),
            "positional, no parameters, nothing borrowed"
        );
        assert_eq!(net.fireable(), [light.change], "red, nobody waiting");

        // The kinds on the core's own serialised edges, as a runner names
        // them on the wire (PROTOCOL.md §5.1), and the light's style.
        let topology = serde_json::to_value(net.topology()).unwrap();
        let kinds: Vec<&str> = topology["edges"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|edge| edge["kind"].as_str())
            .collect();
        assert_eq!(kinds, ["drain", "snapshot", "take"]);
        assert_eq!(topology["places"][0]["style"], json!({"fill": "token"}));
        assert!(topology["places"][1].get("style").is_none());
    }

    /// The two controls, in order, as §11 has them: `car`, a text box on
    /// `Waiting` that injects, hinted `name`; `change`, a button that fires
    /// `Change`.
    #[test]
    fn the_net_declares_its_two_controls() {
        let light = build_traffic_light().expect("the net builds");
        let controls: Vec<_> = light
            .net
            .controls()
            .iter()
            .map(|c| {
                (
                    c.name.as_str(),
                    c.label.as_str(),
                    c.kind.clone(),
                    c.node,
                    c.action,
                )
            })
            .collect();
        assert_eq!(
            controls,
            [
                (
                    "car",
                    "+ Car",
                    ControlKind::Text {
                        placeholder: Some("name".to_owned())
                    },
                    Some(Node::Place(light.waiting.id())),
                    Some(Action::Inject),
                ),
                (
                    "change",
                    "Change",
                    ControlKind::Button,
                    Some(Node::Transition(light.change)),
                    Some(Action::Fire),
                ),
            ]
        );
        assert!(light.net.controls().iter().all(|c| c.initial.is_none()));
    }

    /// The light's token is drawn as the lamp it lights, whatever the
    /// colour, and follows it round.
    #[test]
    fn the_light_is_drawn_in_its_lamp_s_colour() {
        assert_eq!(
            [Colour::Red, Colour::Green, Colour::Amber].map(|c| c.style().colour),
            [
                Some(Rgb(0xff, 0x63, 0x47)),
                Some(Rgb(0x32, 0xcd, 0x32)),
                Some(Rgb(0xff, 0xa5, 0x00)),
            ]
        );
        let mut light = build_traffic_light().expect("the net builds");
        let mut seen = vec![drawn(&light)];
        for _ in 0..3 {
            light.net.step().unwrap();
            seen.push(drawn(&light));
        }
        assert_eq!(
            seen,
            ["#ff6347", "#32cd32", "#ffa500", "#ff6347"].map(|hex| Some(hex.to_owned()))
        );
        let (car, _) = {
            light.net.put(light.waiting, "Car 1".to_owned());
            light.net.tokens(light.waiting.id()).next().unwrap()
        };
        assert_eq!(
            light.net.style(light.waiting.id(), car),
            None,
            "cars have no style"
        );
    }

    /// A colour's JSON is the plain lower-case string, so the wire is as it
    /// was when the token was a string, and an `inject` into `Light` of
    /// `"green"` is a green light.
    #[test]
    fn a_colour_is_its_name_in_json() {
        for (colour, name) in [
            (Colour::Red, "red"),
            (Colour::Green, "green"),
            (Colour::Amber, "amber"),
        ] {
            assert_eq!(serde_json::to_value(colour).unwrap(), json!(name));
            assert_eq!(
                serde_json::from_value::<Colour>(json!(name)).unwrap(),
                colour
            );
            assert_eq!(colour.to_string(), name);
            assert_eq!(
                format!("{colour:?}"),
                format!("{name:?}"),
                "the label as a string's"
            );
        }
        assert!(serde_json::from_value::<Colour>(json!("blue")).is_err());

        let mut light = build_traffic_light().expect("the net builds");
        light
            .net
            .apply_json(JsonCommand::SetTokens {
                place: "Light".to_owned(),
                tokens: vec![json!("green")],
            })
            .expect("Light takes a colour by name");
        assert_eq!(colour(&light.net, light.light), Some(Colour::Green));
        let (token, _) = light.net.tokens(light.light.id()).next().unwrap();
        assert_eq!(
            light.net.json(light.light.id(), token),
            Some(json!("green"))
        );
    }

    #[test]
    fn the_script_gives_the_table() {
        let mut light = build_traffic_light().expect("the net builds");
        let rows = script(&mut light).expect("every step fires what it should");
        assert_eq!(rows.len(), TABLE.len());
        for ((after, columns), (expected_after, expected)) in rows.iter().zip(TABLE) {
            assert_eq!(*after, expected_after);
            assert_eq!(*columns, expected, "after {after}");
        }
        assert_eq!(light.net.fired_count(), 8, "seven steps and one press");
        assert_eq!(colour(&light.net, light.light), Some(Colour::Red));
        assert_eq!(light.net.fireable(), [light.change], "never quiescent");
    }

    /// `+ Car` names an untyped car one more than the cars there are,
    /// waiting and passed, and a typed one as typed; each press is an
    /// ordinary `Added` in `Waiting`.
    #[test]
    fn car_names_a_car_from_the_marking_or_as_typed() {
        let mut light = build_traffic_light().expect("the net builds");
        assert_eq!(
            car_name(&light.net, light.waiting, light.passed, ""),
            "Car 1"
        );
        for typed in ["", "", "Taxi"] {
            assert_eq!(
                light.net.press("car", Value::Text(typed.to_owned())),
                Ok(Pressed::Applied)
            );
        }
        assert_eq!(
            strings(&light.net, light.waiting),
            ["Car 1", "Car 2", "Taxi"]
        );
        let events: Vec<Event> = light.net.drain_events().collect();
        assert_eq!(events.len(), 3);
        assert!(
            events.iter().all(
                |event| matches!(event, Event::Added { place, .. } if *place == light.waiting.id())
            ),
            "{events:?}"
        );

        // Passed counts too: the three went at green and Bus waits, so the
        // next is Car 5.
        set_colour(&mut light, Colour::Green);
        assert_eq!(light.net.step().unwrap(), Some(light.go));
        light
            .net
            .press("car", Value::Text("Bus".to_owned()))
            .unwrap();
        light.net.press("car", Value::Text(String::new())).unwrap();
        assert_eq!(strings(&light.net, light.waiting), ["Bus", "Car 5"]);
    }

    /// `change` fires `Change`, an ordinary `Fired`; `car` takes text and
    /// nothing else, and the net stays as it was when it is refused.
    #[test]
    fn change_fires_change_and_car_takes_only_text() {
        let mut light = build_traffic_light().expect("the net builds");
        light.net.drain_events().for_each(drop);
        assert_eq!(light.net.press("change", Value::None), Ok(Pressed::Applied));
        assert_eq!(colour(&light.net, light.light), Some(Colour::Green));
        let events: Vec<Event> = light.net.drain_events().collect();
        assert!(
            matches!(&events[..], [Event::Fired { transition, .. }] if *transition == light.change),
            "{events:?}"
        );

        for value in [Value::Number(7.0), Value::None, Value::Bool(true)] {
            assert!(
                matches!(
                    light.net.press("car", value.clone()),
                    Err(PressError::WrongKind { .. })
                ),
                "{value:?}"
            );
        }
        assert!(matches!(
            light.net.press("change", Value::Bool(true)),
            Err(PressError::WrongKind { .. })
        ));
        assert!(matches!(
            light.net.press("nope", Value::None),
            Err(PressError::UnknownControl { .. })
        ));
        assert_eq!(light.net.drain_events().count(), 0, "nothing happened");
    }

    /// At green with cars waiting both transitions are fireable, and `Go`,
    /// the first declared, fires: its `consumed` lists the cars, oldest
    /// first, and nothing else, and they come out in `Passed` in that order,
    /// one token each, in one firing.
    #[test]
    fn at_green_with_cars_waiting_both_are_fireable_and_go_fires() {
        let mut light = build_traffic_light().expect("the net builds");
        assert_eq!(light.net.step().unwrap(), Some(light.change));
        assert_eq!(colour(&light.net, light.light), Some(Colour::Green));
        let cars = light
            .net
            .extend(light.waiting, ["Car 1".to_owned(), "Car 2".to_owned()]);
        assert_eq!(light.net.fireable(), [light.go, light.change]);
        light.net.drain_events().for_each(drop);

        assert_eq!(light.net.step().unwrap(), Some(light.go));
        let events: Vec<Event> = light.net.drain_events().collect();
        let expected = [(light.waiting.id(), cars[0]), (light.waiting.id(), cars[1])];
        assert!(
            matches!(&events[..], [Event::Fired { transition, consumed, produced }]
                if *transition == light.go
                    && consumed == &expected
                    && produced.len() == 2
                    && produced.iter().all(|(place, _)| *place == light.passed.id())),
            "the cars, oldest first, and not the light: {events:?}"
        );
        assert_eq!(light.net.count(light.waiting.id()), 0, "everyone went");
        assert_eq!(strings(&light.net, light.passed), ["Car 1", "Car 2"]);
        assert_eq!(light.net.fireable(), [light.change]);
    }

    /// A car waiting is not enough for `Go`: the light must be green. And
    /// green is not enough: someone must be waiting. `Light` always holds
    /// the colour, so the read is satisfied at every colour, and the drain
    /// on an empty place too; the guard is what holds `Go` back, and
    /// `Change` is all that is left to fire.
    #[test]
    fn go_is_held_at_red_and_amber_and_with_nobody_waiting() {
        for colour in [Colour::Red, Colour::Amber] {
            let mut light = build_traffic_light().expect("the net builds");
            set_colour(&mut light, colour);
            light.net.put(light.waiting, "Car 1".to_owned());
            assert_eq!(light.net.count(light.light.id()), 1, "a light to read");
            assert!(
                !light.net.is_fireable(light.go),
                "a car waiting at {colour}"
            );
            assert_eq!(light.net.fireable(), [light.change], "at {colour}");
            assert_eq!(light.net.step().unwrap(), Some(light.change));
            assert_eq!(strings(&light.net, light.waiting), ["Car 1"], "it waits");
        }

        let mut light = build_traffic_light().expect("the net builds");
        set_colour(&mut light, Colour::Green);
        assert!(!light.net.is_fireable(light.go), "green, nobody waiting");
        assert_eq!(light.net.fireable(), [light.change]);
        assert_eq!(light.net.step().unwrap(), Some(light.change));
        assert_eq!(colour(&light.net, light.light), Some(Colour::Amber));
    }

    /// What `Go`'s guard prevents. Without it `Go` is fireable at green with
    /// nobody waiting, ahead of `Change`, and fires taking nothing, the
    /// drain's zero case: no token consumed, none produced, the light read
    /// and left as it was. Left to step, it fires again and again, and the
    /// light sticks on green. `assert_no_source_transitions` names it: its
    /// only consuming edge is the drain, which may take nothing.
    #[test]
    fn without_its_guard_go_fires_at_green_with_nobody_waiting_and_takes_nothing() {
        let declare = |b: &mut NetBuilder| {
            let light = b.place::<Colour>("Light");
            let waiting = b.place::<String>("Waiting");
            let passed = b.place::<String>("Passed");
            b.seed(light, [Colour::Green]);
            let go = b
                .transition(
                    "Go",
                    (DrainEdge(waiting), SnapshotEdge(light)),
                    (passed,),
                    net::go,
                )
                .id();
            let change = b
                .transition("Change", (TakeEdge(light),), (light,), net::change)
                .id();
            (light, waiting, passed, go, change)
        };

        let mut b = NetBuilder::new();
        let (light, waiting, passed, go, change) = declare(&mut b);
        let mut net = b.build().expect("the unguarded light builds");
        let lamp: Vec<_> = net.tokens(light.id()).map(|(id, _)| id).collect();
        assert_eq!(net.count(waiting.id()), 0, "nobody waiting");
        assert_eq!(net.fireable(), [go, change], "and still fireable");
        net.drain_events().for_each(drop);

        assert_eq!(net.step().unwrap(), Some(go));
        let events: Vec<Event> = net.drain_events().collect();
        assert!(
            matches!(&events[..], [Event::Fired { transition, consumed, produced }]
                if *transition == go && consumed.is_empty() && produced.is_empty()),
            "took nothing, gave nothing: {events:?}"
        );
        assert_eq!(net.count(passed.id()), 0);
        for _ in 0..5 {
            assert_eq!(net.step().unwrap(), Some(go), "the light sticks on green");
        }
        let still: Vec<_> = net.tokens(light.id()).map(|(id, _)| id).collect();
        assert_eq!(still, lamp, "read, never taken");

        let mut b = NetBuilder::new();
        b.assert_no_source_transitions();
        declare(&mut b);
        let error = b.build().unwrap_err();
        assert!(
            matches!(error, BuildError::SourceTransition { ref transition } if transition == "Go"),
            "{error:?}"
        );
    }

    /// `Go` reads the light and does not take it: the light's token keeps
    /// its id through every `Go`, and only `Change` replaces it.
    #[test]
    fn go_leaves_the_lights_token_unchanged() {
        let mut light = build_traffic_light().expect("the net builds");
        assert_eq!(light.net.step().unwrap(), Some(light.change));
        let lamp = lamps(&light);
        assert_eq!(lamp.len(), 1);

        for cars in [&["Car 1", "Car 2"][..], &["Car 3"][..]] {
            light
                .net
                .extend(light.waiting, cars.iter().map(|&car| car.to_owned()));
            assert_eq!(light.net.step().unwrap(), Some(light.go));
            assert_eq!(lamps(&light), lamp, "read, not taken");
            assert_eq!(colour(&light.net, light.light), Some(Colour::Green));
        }

        assert_eq!(light.net.step().unwrap(), Some(light.change));
        assert_ne!(lamps(&light), lamp, "Change takes it and writes a new one");
    }

    /// Left to itself the light goes round for ever, green, amber, red, by
    /// `Change` alone, and `Light` holds exactly one token after every
    /// firing.
    #[test]
    fn the_light_goes_round_by_itself() {
        let mut light = build_traffic_light().expect("the net builds");
        let round = [Colour::Green, Colour::Amber, Colour::Red];
        for after in round.iter().cycle().take(30) {
            assert_eq!(light.net.step().unwrap(), Some(light.change));
            assert_eq!(light.net.count(light.light.id()), 1);
            assert_eq!(colour(&light.net, light.light), Some(*after));
        }
        assert_eq!(light.net.fired_count(), 30);
        assert_eq!(
            [Colour::Red, Colour::Green, Colour::Amber].map(next_colour),
            round
        );
    }

    /// The wire's `inject` is a JSON `extend` of one string into a place,
    /// which every place takes; the script driven that way ends as the table
    /// does before the presses.
    #[test]
    fn the_script_through_json_commands() {
        let mut light = build_traffic_light().expect("the net builds");
        let inject = |light: &mut TrafficLight, token: &str| {
            light
                .net
                .apply_json(JsonCommand::Extend {
                    place: "Waiting".to_owned(),
                    tokens: vec![json!(token)],
                })
                .expect("every place is serializable");
        };
        light.net.step().unwrap();
        light.net.step().unwrap();
        inject(&mut light, "Car 1");
        inject(&mut light, "Car 2");
        assert_eq!(light.net.fireable(), [light.change]);
        light.net.step().unwrap();
        light.net.step().unwrap();
        assert_eq!(light.net.fireable(), [light.go, light.change]);
        light.net.step().unwrap();
        inject(&mut light, "Car 3");
        light.net.step().unwrap();
        light.net.step().unwrap();
        let (after, columns) = row("end", &light);
        assert_eq!(after, "end");
        assert_eq!(columns, TABLE[8].1);
    }
}
