//! Traffic light: a drain edge, which takes every token of a place that may
//! be empty, and a snapshot edge, the plain read (DESIGN §4.4).
//!
//! The drain half of the edges-on-empty-places example, split from the shop
//! on 2026-10-16, built as the conformance net `traffic-light` of the shared
//! protocol (`PROTOCOL.md` §11), which lives in [`net`] so that a runner can
//! carry the same file. Cars wait at a light that cycles red, green and
//! amber, and all go when it turns green. The light is one place, `Light`,
//! and its one token is the colour.
//!
//! `Turn Green` reads `Waiting` through a [`DrainEdge`]: every car, zero or
//! more, all consumed. The edge does not hold its transition back on an
//! empty place, so the light turns green whether or not anyone is waiting,
//! and everyone who waited at red goes the moment it does; without the
//! drain the light would need an "empty twin" of `Turn Green`, guarded on
//! nobody waiting. `Change` moves the colour on from green and from amber.
//! `Pass on Green` reads `Light` through a [`SnapshotEdge`], the simpler
//! kind of read: the colour is cloned, not taken. The light is never empty,
//! so the read alone would let a car through at any colour; a guard on
//! green is what holds it back at amber and red.
//!
//! `main` runs the script of the shared transcript `08_traffic_light.json`
//! — the light turns green with nobody waiting and changes to amber, two
//! cars arrive at amber and wait through red, both go when it turns green, a
//! third arrives at green and goes straight through, and the light changes
//! to amber — and prints `Light`, `Waiting` and `Passed` after each step,
//! checking what fired and what is fireable as the transcript does. The
//! light never stops: some light transition is always fireable. The tests
//! below hold the table, the net's shape as §11 has it, what the drain does
//! to the event log, that a car goes on green only, and that the light goes
//! round by itself.
//!
//! [`DrainEdge`]: petritype::DrainEdge
//! [`SnapshotEdge`]: petritype::SnapshotEdge

// A crate root's `mod` looks beside it, so the path is spelled out: the net
// lives in `traffic_light/net.rs`, the file a runner's example can include
// too.
#[path = "traffic_light/net.rs"]
mod net;

use std::error::Error;

use petritype::{Net, PlaceRef, TransitionId};

use net::{TrafficLight, build_traffic_light};

/// One row of the table: what happened, and `Light`, `Waiting` and `Passed`
/// after it.
type Row = (&'static str, [Vec<String>; 3]);

/// The strings in a place, in queue order.
fn strings(net: &Net, place: PlaceRef<String>) -> Vec<String> {
    net.tokens(place.id())
        .filter_map(|(token, _)| net.token::<String>(place.id(), token).cloned())
        .collect()
}

/// The three places as they stand.
fn row(after: &'static str, light: &TrafficLight) -> Row {
    (
        after,
        [
            strings(&light.net, light.light),
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
    let (turn_green, pass_on_green, change) = (light.turn_green, light.pass_on_green, light.change);
    let mut rows = Vec::new();

    step(light, turn_green, &[change])?;
    rows.push(row("Turn Green (nobody waiting)", light));
    step(light, change, &[change])?;
    rows.push(row("Change", light));

    // At amber the cars wait: `Pass on Green` and `Turn Green` are held by
    // their guards.
    arrive(light, &["Car 1", "Car 2"], &[change])?;
    rows.push(row("Car 1 and Car 2 arrive", light));
    step(light, change, &[turn_green])?;
    rows.push(row("Change", light));
    step(light, turn_green, &[change])?;
    rows.push(row("Turn Green (two waiting)", light));

    // At green a car goes before the light changes: first declared.
    arrive(light, &["Car 3"], &[pass_on_green, change])?;
    rows.push(row("Car 3 arrives", light));
    step(light, pass_on_green, &[change])?;
    rows.push(row("Pass on Green", light));
    step(light, change, &[change])?;
    rows.push(row("Change", light));
    Ok(rows)
}

/// A list as the table writes it: `[Car 1, Car 2]`.
fn list(items: &[String]) -> String {
    format!("[{}]", items.join(", "))
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut light = build_traffic_light()?;
    // Left to right, like the road: the cars flowing from `Waiting` to
    // `Passed`, the light's loops beside them.
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
    println!(
        "\nNever quiescent: fireable now {:?}",
        names(&light.net, &light.net.fireable())
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use net::{colour, next_colour};
    use petritype::serde_json::{self, json};
    use petritype::{EdgeKind, Event, JsonCommand};

    /// The table: after each step and each arrival, `Light`, `Waiting` and
    /// `Passed`.
    const TABLE: [(&str, [&[&str]; 3]); 8] = [
        ("Turn Green (nobody waiting)", [&["green"], &[], &[]]),
        ("Change", [&["amber"], &[], &[]]),
        (
            "Car 1 and Car 2 arrive",
            [&["amber"], &["Car 1", "Car 2"], &[]],
        ),
        ("Change", [&["red"], &["Car 1", "Car 2"], &[]]),
        (
            "Turn Green (two waiting)",
            [&["green"], &[], &["Car 1", "Car 2"]],
        ),
        (
            "Car 3 arrives",
            [&["green"], &["Car 3"], &["Car 1", "Car 2"]],
        ),
        (
            "Pass on Green",
            [&["green"], &[], &["Car 1", "Car 2", "Car 3"]],
        ),
        ("Change", [&["amber"], &[], &["Car 1", "Car 2", "Car 3"]]),
    ];

    /// The ids of the tokens in `Light`, in queue order: one, always.
    fn lamps(light: &TrafficLight) -> Vec<petritype::TokenId> {
        light
            .net
            .tokens(light.light.id())
            .map(|(id, _)| id)
            .collect()
    }

    #[test]
    fn the_net_is_the_conformance_net_of_protocol_section_11() {
        let light = build_traffic_light().expect("the net builds");
        let net = &light.net;
        let places: Vec<(&str, bool, usize)> = net
            .places()
            .map(|(place, info)| (info.name, info.serializable, net.count(place)))
            .collect();
        assert_eq!(
            places,
            [
                ("Light", true, 1),
                ("Waiting", true, 0),
                ("Passed", true, 0),
            ]
        );
        assert_eq!(strings(net, light.light), ["red"]);

        let transitions: Vec<(&str, &[_], &[_], bool)> = net
            .transitions()
            .map(|(_, info)| (info.name, info.inputs, info.outputs, info.enabled))
            .collect();
        assert_eq!(
            transitions,
            [
                (
                    "Turn Green",
                    &[
                        (light.light.id(), EdgeKind::Take),
                        (light.waiting.id(), EdgeKind::Drain)
                    ][..],
                    &[light.light.id(), light.passed.id()][..],
                    true
                ),
                (
                    "Pass on Green",
                    &[
                        (light.waiting.id(), EdgeKind::Take),
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
        assert_eq!(net.fireable(), [light.turn_green], "red, nobody waiting");

        // The kinds on the core's own serialised edges, as a runner names
        // them on the wire (PROTOCOL.md §5.1).
        let topology = serde_json::to_value(net.topology()).unwrap();
        let kinds: Vec<&str> = topology["edges"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|edge| edge["kind"].as_str())
            .collect();
        assert_eq!(kinds, ["take", "drain", "take", "snapshot", "take"]);
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
        assert_eq!(light.net.fired_count(), 6);
        assert_eq!(strings(&light.net, light.light), ["amber"]);
        assert_eq!(light.net.count(light.waiting.id()), 0);
        assert_eq!(light.net.fireable(), [light.change], "never quiescent");
    }

    /// `Turn Green` fires with nobody waiting, consuming the light alone,
    /// and with cars waiting at red drains them all: its `consumed` lists the
    /// light, then the cars, oldest first, and they come out in `Passed` in
    /// that order. The light comes back to `Light` green, a new token.
    #[test]
    fn turn_green_fires_with_nobody_waiting_and_drains_everyone_waiting_at_red() {
        let mut light = build_traffic_light().expect("the net builds");
        let lamp = lamps(&light)[0];
        light.net.drain_events().for_each(drop);

        assert_eq!(light.net.step().unwrap(), Some(light.turn_green));
        let events: Vec<Event> = light.net.drain_events().collect();
        assert!(
            matches!(&events[..], [Event::Fired { transition, consumed, produced }]
                if *transition == light.turn_green
                    && consumed == &[(light.light.id(), lamp)]
                    && produced.len() == 1
                    && produced[0].0 == light.light.id()
                    && produced[0].1 != lamp),
            "nobody waiting, and still fired: {events:?}"
        );
        assert_eq!(strings(&light.net, light.light), ["green"]);

        assert_eq!(light.net.step().unwrap(), Some(light.change));
        let cars = light
            .net
            .extend(light.waiting, ["Car 1".to_owned(), "Car 2".to_owned()]);
        assert_eq!(light.net.step().unwrap(), Some(light.change));
        assert_eq!(strings(&light.net, light.light), ["red"]);
        let lamp = lamps(&light)[0];
        light.net.drain_events().for_each(drop);

        assert_eq!(light.net.step().unwrap(), Some(light.turn_green));
        let events: Vec<Event> = light.net.drain_events().collect();
        let expected = [
            (light.light.id(), lamp),
            (light.waiting.id(), cars[0]),
            (light.waiting.id(), cars[1]),
        ];
        assert!(
            matches!(&events[..], [Event::Fired { consumed, produced, .. }]
                if consumed == &expected && produced.len() == 3),
            "the light, then the cars oldest first: {events:?}"
        );
        assert_eq!(light.net.count(light.waiting.id()), 0, "everyone went");
        assert_eq!(strings(&light.net, light.passed), ["Car 1", "Car 2"]);
        assert_eq!(strings(&light.net, light.light), ["green"]);
    }

    /// A car waiting is enough for `Pass on Green` only while the light is
    /// green. `Light` always holds the colour, so the read is satisfied at
    /// every colour and the guard is what holds the car at amber and red. At
    /// green the car goes before the light changes, the first declared; the
    /// light is read there, not taken, and stays green under the same id.
    #[test]
    fn pass_on_green_is_held_by_its_guard_and_goes_before_change_at_green() {
        for colour in ["red", "green", "amber"] {
            let mut light = build_traffic_light().expect("the net builds");
            light.net.set_tokens(light.light, [colour.to_owned()]);
            light.net.put(light.waiting, "Car 1".to_owned());
            assert_eq!(light.net.count(light.light.id()), 1, "a light to read");
            assert_eq!(
                light.net.is_fireable(light.pass_on_green),
                colour == "green",
                "a car waiting at {colour}"
            );
            let expected = match colour {
                "red" => vec![light.turn_green],
                "green" => vec![light.pass_on_green, light.change],
                _ => vec![light.change],
            };
            assert_eq!(light.net.fireable(), expected, "at {colour}");
        }

        let mut light = build_traffic_light().expect("the net builds");
        assert_eq!(light.net.step().unwrap(), Some(light.turn_green));
        assert!(
            !light.net.is_fireable(light.pass_on_green),
            "nobody waiting"
        );
        let lamp = lamps(&light)[0];
        let car = light.net.put(light.waiting, "Car 3".to_owned());
        assert_eq!(light.net.fireable(), [light.pass_on_green, light.change]);
        light.net.drain_events().for_each(drop);
        assert_eq!(light.net.step().unwrap(), Some(light.pass_on_green));
        let events: Vec<Event> = light.net.drain_events().collect();
        assert!(
            matches!(&events[..], [Event::Fired { consumed, .. }]
                if consumed == &[(light.waiting.id(), car)]),
            "the car alone: {events:?}"
        );
        assert_eq!(lamps(&light), [lamp], "the light read, not taken");
        assert_eq!(colour(&light.net, light.light), Some("green"));
    }

    /// Left to itself the light goes round for ever, green, amber, red, by
    /// `Turn Green` and two `Change`s, and `Light` holds exactly one token
    /// after every firing.
    #[test]
    fn the_light_goes_round_by_itself() {
        let mut light = build_traffic_light().expect("the net builds");
        let cycle = [
            (light.turn_green, "green"),
            (light.change, "amber"),
            (light.change, "red"),
        ];
        for (expected, after) in cycle.iter().cycle().take(30) {
            assert_eq!(light.net.step().unwrap(), Some(*expected));
            assert_eq!(light.net.count(light.light.id()), 1);
            assert_eq!(colour(&light.net, light.light), Some(*after));
        }
        assert_eq!(light.net.fired_count(), 30);
        assert_eq!(
            ["red", "green", "amber"].map(next_colour),
            ["green", "amber", "red"]
        );
    }

    /// The wire's `inject` is a JSON `extend` of one string into a place,
    /// which every place takes; the script driven that way ends as the table
    /// does.
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
        inject(&mut light, "Car 3");
        light.net.step().unwrap();
        light.net.step().unwrap();
        let (after, columns) = row("end", &light);
        assert_eq!(after, "end");
        assert_eq!(columns, TABLE[7].1);
    }
}
