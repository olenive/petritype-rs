//! Traffic light: a drain edge, which takes every token of a place that may
//! be empty, and a snapshot edge, the plain read (DESIGN §4.4).
//!
//! The drain half of the edges-on-empty-places example, split from the shop
//! on 2026-10-16, built as the conformance net `traffic-light` of the shared
//! protocol (`PROTOCOL.md` §11), which lives in [`net`] so that a runner can
//! carry the same file. Cars wait at a light that cycles red, green and
//! amber, and all go when it turns green.
//!
//! `Turn Green` reads `Waiting` through a [`DrainEdge`]: every car, zero or
//! more, all consumed. The edge does not hold its transition back on an
//! empty place, so the light turns green whether or not anyone is waiting,
//! and everyone who waited at red goes the moment it does; without the
//! drain the light would need an "empty twin" of `Turn Green`, guarded on
//! nobody waiting. `Pass on Green` reads `Green` through a
//! [`SnapshotEdge`], the simpler kind of read: the light is cloned, not
//! taken, and since a snapshot needs its place non-empty, a car goes past
//! only while the light is green.
//!
//! `main` runs the script of the shared transcript `08_traffic_light.json`
//! — the light turns green with nobody waiting and on to amber, two cars
//! arrive at amber and wait through red, both go when it turns green, a
//! third arrives at green and goes straight through, and the light turns
//! amber — and prints the light and the cars after each step. The light
//! never stops: some light transition is always fireable. The tests below
//! hold the table, the net's shape as §11 has it, what the drain does to the
//! event log, and that a car goes on green only.
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

/// One row of the table: what happened, the light's colour after it, and
/// `Waiting` and `Passed`.
type Row = (&'static str, String, [Vec<String>; 2]);

/// The strings in a place, in queue order.
fn strings(net: &Net, place: PlaceRef<String>) -> Vec<String> {
    net.tokens(place.id())
        .filter_map(|(token, _)| net.token::<String>(place.id(), token).cloned())
        .collect()
}

/// The light's colour: the name of the place the light stands in (more than
/// one, joined, or `none`, would be a broken net).
fn colour(light: &TrafficLight) -> String {
    let lit: Vec<&str> = [
        ("Red", light.red),
        ("Green", light.green),
        ("Amber", light.amber),
    ]
    .into_iter()
    .filter(|(_, place)| light.net.count(place.id()) > 0)
    .map(|(name, _)| name)
    .collect();
    if lit.is_empty() {
        "none".to_owned()
    } else {
        lit.join(" and ")
    }
}

/// The light and the two columns as they stand.
fn row(after: &'static str, light: &TrafficLight) -> Row {
    (
        after,
        colour(light),
        [
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

/// Steps the net once with the default selector and checks that `expected`
/// is what fired.
fn step(light: &mut TrafficLight, expected: TransitionId) -> Result<(), Box<dyn Error>> {
    match light.net.step()? {
        Some(fired) if fired == expected => Ok(()),
        other => Err(format!("expected {expected:?} to fire, got {other:?}").into()),
    }
}

/// Puts cars into `Waiting`, as the wire's `inject` does.
fn arrive(light: &mut TrafficLight, cars: &[&str]) {
    light
        .net
        .extend(light.waiting, cars.iter().map(|&car| car.to_owned()));
}

/// The transcript's script on a fresh light, one row per step and per
/// arrival.
fn script(light: &mut TrafficLight) -> Result<Vec<Row>, Box<dyn Error>> {
    let mut rows = Vec::new();

    step(light, light.turn_green)?;
    rows.push(row("Turn Green (nobody waiting)", light));
    step(light, light.turn_amber)?;
    rows.push(row("Turn Amber", light));

    arrive(light, &["Car 1", "Car 2"]);
    if light.net.fireable() != [light.turn_red] {
        return Err("at amber only Turn Red should be fireable".into());
    }
    rows.push(row("Car 1 and Car 2 arrive", light));
    step(light, light.turn_red)?;
    rows.push(row("Turn Red", light));
    step(light, light.turn_green)?;
    rows.push(row("Turn Green (two waiting)", light));

    arrive(light, &["Car 3"]);
    rows.push(row("Car 3 arrives", light));
    step(light, light.pass_on_green)?;
    rows.push(row("Pass on Green", light));
    step(light, light.turn_amber)?;
    rows.push(row("Turn Amber", light));
    Ok(rows)
}

/// A list as the table writes it: `[Car 1, Car 2]`.
fn list(items: &[String]) -> String {
    format!("[{}]", items.join(", "))
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut light = build_traffic_light()?;
    // Left to right, like the road: the light's loop along the top, the cars
    // flowing from `Waiting` to `Passed` below it.
    let road = petritype_dot::Style {
        rankdir: petritype_dot::RankDir::LR,
        ..petritype_dot::Style::default()
    };
    println!("{}", petritype_dot::dot_with(&light.net, &road));

    println!("| After | Light | Waiting | Passed |");
    println!("|---|---|---|---|");
    for (after, colour, [waiting, passed]) in script(&mut light)? {
        println!(
            "| {after} | {colour} | {} | {} |",
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
    use petritype::serde_json::{self, json};
    use petritype::{EdgeKind, Event, JsonCommand};

    /// The table: after each step and each arrival, the light, `Waiting` and
    /// `Passed`.
    const TABLE: [(&str, &str, [&[&str]; 2]); 8] = [
        ("Turn Green (nobody waiting)", "Green", [&[], &[]]),
        ("Turn Amber", "Amber", [&[], &[]]),
        (
            "Car 1 and Car 2 arrive",
            "Amber",
            [&["Car 1", "Car 2"], &[]],
        ),
        ("Turn Red", "Red", [&["Car 1", "Car 2"], &[]]),
        (
            "Turn Green (two waiting)",
            "Green",
            [&[], &["Car 1", "Car 2"]],
        ),
        ("Car 3 arrives", "Green", [&["Car 3"], &["Car 1", "Car 2"]]),
        (
            "Pass on Green",
            "Green",
            [&[], &["Car 1", "Car 2", "Car 3"]],
        ),
        ("Turn Amber", "Amber", [&[], &["Car 1", "Car 2", "Car 3"]]),
    ];

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
                ("Red", true, 1),
                ("Green", true, 0),
                ("Amber", true, 0),
                ("Waiting", true, 0),
                ("Passed", true, 0),
            ]
        );
        assert_eq!(strings(net, light.red), ["light"]);

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
                        (light.red.id(), EdgeKind::Take),
                        (light.waiting.id(), EdgeKind::Drain)
                    ][..],
                    &[light.green.id(), light.passed.id()][..],
                    true
                ),
                (
                    "Pass on Green",
                    &[
                        (light.waiting.id(), EdgeKind::Take),
                        (light.green.id(), EdgeKind::Snapshot)
                    ][..],
                    &[light.passed.id()][..],
                    true
                ),
                (
                    "Turn Amber",
                    &[(light.green.id(), EdgeKind::Take)][..],
                    &[light.amber.id()][..],
                    true
                ),
                (
                    "Turn Red",
                    &[(light.amber.id(), EdgeKind::Take)][..],
                    &[light.red.id()][..],
                    true
                ),
            ]
        );
        assert!(
            net.transitions()
                .all(|(_, info)| info.positional && info.params.is_empty() && info.detachable),
            "positional, no parameters, nothing borrowed"
        );
        assert_eq!(net.fireable(), [light.turn_green], "nobody waiting");

        // The kinds on the core's own serialised edges, as a runner names
        // them on the wire (PROTOCOL.md §5.1).
        let topology = serde_json::to_value(net.topology()).unwrap();
        let kinds: Vec<&str> = topology["edges"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|edge| edge["kind"].as_str())
            .collect();
        assert_eq!(kinds, ["take", "drain", "take", "snapshot", "take", "take"]);
    }

    #[test]
    fn the_script_gives_the_table() {
        let mut light = build_traffic_light().expect("the net builds");
        let rows = script(&mut light).expect("every step fires what it should");
        assert_eq!(rows.len(), TABLE.len());
        for ((after, colour, columns), (expected_after, expected_colour, expected)) in
            rows.iter().zip(TABLE)
        {
            assert_eq!(*after, expected_after);
            assert_eq!(colour, expected_colour, "after {after}");
            assert_eq!(*columns, expected, "after {after}");
        }
        assert_eq!(light.net.fired_count(), 6);
        assert_eq!(strings(&light.net, light.amber), ["light"]);
        for place in [light.red, light.green, light.waiting] {
            assert_eq!(light.net.count(place.id()), 0);
        }
        assert_eq!(light.net.fireable(), [light.turn_red], "never quiescent");
    }

    /// `Turn Green` fires with nobody waiting, consuming the light alone,
    /// and with cars waiting at red drains them all: its `consumed` lists the
    /// light, then the cars, oldest first, and they come out in `Passed` in
    /// that order.
    #[test]
    fn turn_green_fires_with_nobody_waiting_and_drains_everyone_waiting_at_red() {
        let mut light = build_traffic_light().expect("the net builds");
        let lamp = light.net.tokens(light.red.id()).next().unwrap().0;
        light.net.drain_events().for_each(drop);

        assert_eq!(light.net.step().unwrap(), Some(light.turn_green));
        let events: Vec<Event> = light.net.drain_events().collect();
        assert!(
            matches!(&events[..], [Event::Fired { transition, consumed, produced }]
                if *transition == light.turn_green
                    && consumed == &[(light.red.id(), lamp)]
                    && produced.len() == 1
                    && produced[0].0 == light.green.id()),
            "nobody waiting, and still fired: {events:?}"
        );

        assert_eq!(light.net.step().unwrap(), Some(light.turn_amber));
        let cars = light
            .net
            .extend(light.waiting, ["Car 1".to_owned(), "Car 2".to_owned()]);
        assert_eq!(light.net.step().unwrap(), Some(light.turn_red));
        let lamp = light.net.tokens(light.red.id()).next().unwrap().0;
        light.net.drain_events().for_each(drop);

        assert_eq!(light.net.step().unwrap(), Some(light.turn_green));
        let events: Vec<Event> = light.net.drain_events().collect();
        let expected = [
            (light.red.id(), lamp),
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
    }

    /// A car waiting is enough for `Pass on Green` only while the light is
    /// green; the light is read there, not taken, and stays green under the
    /// same id.
    #[test]
    fn pass_on_green_fires_only_while_the_light_is_green() {
        for colour in ["Red", "Green", "Amber"] {
            let mut light = build_traffic_light().expect("the net builds");
            let place = match colour {
                "Red" => light.red,
                "Green" => light.green,
                _ => light.amber,
            };
            light.net.set_tokens(light.red, Vec::<String>::new());
            light.net.put(place, "light".to_owned());
            light.net.put(light.waiting, "Car 1".to_owned());
            assert_eq!(
                light.net.is_fireable(light.pass_on_green),
                colour == "Green",
                "a car waiting at {colour}"
            );
        }

        let mut light = build_traffic_light().expect("the net builds");
        assert_eq!(light.net.step().unwrap(), Some(light.turn_green));
        assert!(
            !light.net.is_fireable(light.pass_on_green),
            "nobody waiting"
        );
        let lamp = light.net.tokens(light.green.id()).next().unwrap().0;
        let car = light.net.put(light.waiting, "Car 3".to_owned());
        assert_eq!(
            light.net.fireable(),
            [light.pass_on_green, light.turn_amber]
        );
        light.net.drain_events().for_each(drop);
        assert_eq!(light.net.step().unwrap(), Some(light.pass_on_green));
        let events: Vec<Event> = light.net.drain_events().collect();
        assert!(
            matches!(&events[..], [Event::Fired { consumed, .. }]
                if consumed == &[(light.waiting.id(), car)]),
            "the car alone: {events:?}"
        );
        let still: Vec<_> = light
            .net
            .tokens(light.green.id())
            .map(|(id, _)| id)
            .collect();
        assert_eq!(still, [lamp], "the light read, not taken");
    }

    /// Left to itself the light goes round for ever, green, amber, red, and
    /// stands in exactly one place after every firing.
    #[test]
    fn the_light_never_rests() {
        let mut light = build_traffic_light().expect("the net builds");
        let cycle = [light.turn_green, light.turn_amber, light.turn_red];
        for expected in cycle.iter().cycle().take(30) {
            assert_eq!(light.net.step().unwrap(), Some(*expected));
            let lit: usize = [light.red, light.green, light.amber]
                .iter()
                .map(|place| light.net.count(place.id()))
                .sum();
            assert_eq!(lit, 1);
        }
        assert_eq!(colour(&light), "Red");
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
        assert_eq!(light.net.fireable(), [light.turn_red]);
        light.net.step().unwrap();
        light.net.step().unwrap();
        inject(&mut light, "Car 3");
        light.net.step().unwrap();
        light.net.step().unwrap();
        let (after, colour, columns) = row("end", &light);
        assert_eq!((after, colour.as_str()), ("end", TABLE[7].1));
        assert_eq!(columns, TABLE[7].2);
    }
}
