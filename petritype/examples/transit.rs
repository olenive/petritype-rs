//! Transit: vehicles that carry Petri nets of their own (DESIGN §6.6), and
//! a city that runs their journey on its own.
//!
//! The example of the nets-as-tokens proposal
//! (`Cyberpunk2020-Simulation/dev-docs/NETS_AS_TOKENS.md` §2), built as the
//! conformance net `transit` of the shared protocol (`PROTOCOL.md` §11),
//! which lives in [`net`] so that a runner can carry the same file. The
//! outer net, `City`, has four stations — `Depot`, `Central`, `Harbour`,
//! `Airport` — each holding the vehicles standing there, a
//! `Platform@<station>` of passengers waiting at each, `In Transit` for the
//! vehicles between them, and `Arrived` for the passengers who have stepped
//! off. A vehicle is a token, and it carries an *inner* net, its `cabin`,
//! whose shape depends on its kind: built from the template `Cabin` for a
//! bus or a tram, `Deck` for a ferry, `Pod` for a shuttle. The city has one
//! `Depot` place whatever is inside the vehicles in it.
//!
//! Inside a vehicle, passengers come in at the `Door` and sit or stand, and
//! go back to the door when it has arrived somewhere. Nothing inside a
//! vehicle touches the city. The city reaches in only through the vehicle
//! token it holds: `Board <station>` and `Alight <station>` put a passenger
//! into the cabin's `Door` and take one out with a [`MutateEdge`] body;
//! `Depart <station>` and `Arrive` move the vehicle and set the cabin's
//! parameter `stopped`, and `Arrive` puts an announcement inside and opens
//! the doors. The city's guards read the cabin through [`Net::nested`].
//!
//! **Step and watch.** A vehicle's state is where its doors are: each door is
//! a token, its name, in the cabin's `Doors Open` or `Doors Closed`, which
//! the city's guards count and transitions on both sides move from one to the
//! other ([`net`] has the cycle). So nothing here switches a transition off
//! or fires one by name: `main` calls [`Net::step_deep`], which fires the
//! city's own transition when it has one and otherwise the first one inside a
//! vehicle, until all seven passengers are in `Arrived`, and prints each
//! firing under its path as it goes. That is all a viewer's Step does, and
//! the net is built so that it is enough. On the way it saves the city's
//! marking as JSON text — the vehicles' cabins in it, each as the template
//! that built it with its marking, parameters and switches — the first time a
//! vehicle is between stations, loads it into a freshly built city, steps
//! that one to the end too, and compares the two.
//!
//! [`MutateEdge`]: petritype::MutateEdge

// A crate root's `mod` looks beside it, so the path is spelled out: the net
// lives in `transit/net.rs`, the file a runner's example includes too.
#[path = "transit/net.rs"]
mod net;

use petritype::serde_json;
use petritype::{DeepStep, Marking, Net, PlaceId, PlaceRef, TransitionId};

use net::{BoxedError, City, Kind, Location, Vehicle, WAITING, build_city, templates};

/// Firings enough for everyone to arrive, with room to spare: from the seed
/// it takes 45.
const BUDGET: usize = 200;

/// The passengers in a place, by name, oldest first.
fn names(net: &Net, place: &str) -> Vec<String> {
    net.place_named(place)
        .map(|place| {
            net.tokens(place)
                .filter_map(|(_, value)| value.downcast_ref::<String>().cloned())
                .collect()
        })
        .unwrap_or_default()
}

/// The name of a transition of `net`.
fn transition_name(net: &Net, transition: TransitionId) -> String {
    net.transitions()
        .find(|(id, _)| *id == transition)
        .map_or_else(
            || format!("{transition:?}"),
            |(_, info)| info.name.to_owned(),
        )
}

/// The name of a place of `net`.
fn place_name(net: &Net, place: PlaceId) -> String {
    net.places()
        .find(|(id, _)| *id == place)
        .map_or_else(|| format!("{place:?}"), |(_, info)| info.name.to_owned())
}

/// A firing as [`Net::step_deep`] reports it: the city's own transition by
/// name, or one inside a vehicle under its path, `"Depot / #0 / cabin:
/// Sit"`. Read straight after the step, while the vehicle is still where it
/// fired.
fn firing(net: &Net, step: &DeepStep) -> String {
    match step {
        DeepStep::Here { transition } => transition_name(net, *transition),
        DeepStep::Inside {
            place,
            token,
            net: name,
            step,
            ..
        } => {
            let inside = net
                .nested(*place, *token)
                .and_then(|nets| nets.into_iter().find(|(n, _)| *n == name.as_str()))
                .map_or_else(
                    || format!("{:?}", step.transition()),
                    |(_, inner)| firing(inner, step),
                );
            format!("{} / #{token} / {name}: {inside}", place_name(net, *place))
        }
    }
}

/// What one of the city's own firings did, read straight after it: who
/// boarded or stepped off which vehicle, where a vehicle left for, and
/// where one arrived. `None` for a firing inside a vehicle, whose path says
/// enough.
fn what(city: &City, step: &DeepStep) -> Option<String> {
    let DeepStep::Here { transition } = step else {
        return None;
    };
    let newest = |place: PlaceRef<Vehicle>| {
        let (token, _) = city.net.tokens(place.id()).last()?;
        Some((token, place, city.net.token::<Vehicle>(place.id(), token)?))
    };
    if let Some(at) = Location::ALL
        .into_iter()
        .find(|at| city.board[*at as usize] == *transition)
    {
        // The newest at the door of the front vehicle there.
        let (_, vehicle) = city.vehicle(city.stations.at(at))?;
        let who = names(city.inside(city.stations.at(at))?, "Door").pop()?;
        return Some(format!("{who} boards {}", vehicle.name));
    }
    if let Some(at) = Location::ALL
        .into_iter()
        .find(|at| city.alight[*at as usize] == *transition)
    {
        let (_, vehicle) = city.vehicle(city.stations.at(at))?;
        let who = names(&city.net, "Arrived").pop()?;
        return Some(format!("{who} steps off {}", vehicle.name));
    }
    if city.depart.contains(transition) {
        let (_, _, vehicle) = newest(city.in_transit)?;
        return Some(format!(
            "{} leaves for {}",
            vehicle.name, vehicle.destination?
        ));
    }
    if city.arrive == *transition {
        // Token ids are never reused, so the one just put down is the
        // newest of the vehicles at the back of a station.
        let (_, place, vehicle) = city
            .stations
            .places
            .into_iter()
            .filter_map(newest)
            .max_by_key(|(token, _, _)| *token)?;
        return Some(format!(
            "{} at {}",
            vehicle.name,
            place_name(&city.net, place.id())
        ));
    }
    None
}

/// Steps the city with [`Net::step_deep`] until everyone has arrived,
/// handing each firing to `each`, with its number, as it is made; returns
/// how many it took. An error if a body fails, if the city stops with
/// someone still to arrive, or if [`BUDGET`] firings are not enough.
fn journey(
    city: &mut City,
    mut each: impl FnMut(&City, usize, &DeepStep) -> Result<(), BoxedError>,
) -> Result<usize, BoxedError> {
    let mut fired = 0;
    while !city.all_arrived() {
        if fired == BUDGET {
            return Err(format!(
                "{:?} arrived in {BUDGET} firings, not everyone",
                names(&city.net, "Arrived")
            )
            .into());
        }
        let step = city
            .net
            .step_deep()?
            .ok_or("the city stopped with passengers still to arrive")?;
        fired += 1;
        each(city, fired, &step)?;
    }
    Ok(fired)
}

/// The city's marking as JSON text, every vehicle's cabin inside it.
fn save(city: &City) -> Result<String, BoxedError> {
    Ok(serde_json::to_string(&city.net.export_marking()?)?)
}

/// A freshly built city with a saved marking loaded into it. The import's
/// one `Resynced` is dropped with the rest of the set-up.
fn load(text: &str) -> Result<City, BoxedError> {
    let mut city = build_city()?;
    let marking: Marking = serde_json::from_str(text)?;
    city.net.import_marking(&marking)?;
    city.net.drain_events().for_each(drop);
    Ok(city)
}

fn main() -> Result<(), BoxedError> {
    let mut city = build_city()?;
    println!("{}", petritype_dot::dot(&city.net));
    let templates = templates();
    for kind in Kind::ALL {
        let vehicle = Vehicle::new(&format!("{kind:?}"), kind, &templates)?;
        let places: Vec<&str> = vehicle.cabin.places().map(|(_, info)| info.name).collect();
        println!(
            "a {kind:?} carries a {}: {places:?}, its doors {:?}",
            kind.template(),
            kind.doors()
        );
    }
    for (location, _) in WAITING {
        let waiting: Vec<&String> = city
            .net
            .tokens(city.platforms.at(location).id())
            .filter_map(|(_, value)| value.downcast_ref::<String>())
            .collect();
        println!("waiting at {location}: {waiting:?}");
    }

    println!("\nthe journey, one step_deep at a time:");
    let mut saved = None;
    let fired = journey(&mut city, |city, n, step| {
        match what(city, step) {
            Some(what) => println!("  {n:>3}  {}: {what}", firing(&city.net, step)),
            None => println!("  {n:>3}  {}", firing(&city.net, step)),
        }
        if saved.is_none() && city.net.count(city.in_transit.id()) > 0 {
            saved = Some((n, save(city)?));
        }
        Ok(())
    })?;
    println!(
        "\nall {} arrived in {fired} firings: {:?}",
        City::passengers(),
        names(&city.net, "Arrived")
    );
    for (place, _, vehicle) in city.vehicles() {
        println!(
            "{} is at {}, doors open {:?}, closed {:?}",
            vehicle.name,
            place_name(&city.net, place.id()),
            names(&vehicle.cabin, "Doors Open"),
            names(&vehicle.cabin, "Doors Closed"),
        );
    }
    println!(
        "the city, as a save writes it:\n{}",
        serde_json::to_string_pretty(&city.net.export_marking()?)?
    );

    let (at, text) = saved.ok_or("no vehicle ever left a station")?;
    let mut loaded = load(&text)?;
    let (_, vehicle) = loaded.vehicle(loaded.in_transit).ok_or("in transit")?;
    println!(
        "\nsaved after firing {at} ({} bytes), with {} in transit as the save wrote it:\n{}",
        text.len(),
        vehicle.name,
        serde_json::to_string_pretty(vehicle)?
    );
    let more = journey(&mut loaded, |_, _, _| Ok(()))?;
    println!(
        "loaded into a fresh city and stepped on, everyone arrived {more} firings later \
         ({at} + {more} = {}), the same marking as the straight run, inside the vehicles \
         too: {}",
        at + more,
        loaded.net.export_marking()? == city.net.export_marking()?,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
    use net::{CONTROL_PLACES, Capacity, Destination, SeatCount, Stopped};
    use petritype::drivers::{self, Limits, Mode, OnFailure};
    use petritype::{Event, Params, TieBreak, TokenId};
    use serde_json::{Value, json};

    /// A firing named for a test: the city's own transition, or one inside
    /// a vehicle after the vehicle's name, `"Bus 1: Sit"`.
    fn by_vehicle(city: &City, step: &DeepStep) -> String {
        match step {
            DeepStep::Here { transition } => transition_name(&city.net, *transition),
            DeepStep::Inside {
                place, token, step, ..
            } => {
                let vehicle = city
                    .net
                    .token::<Vehicle>(*place, *token)
                    .expect("something fired inside a vehicle");
                format!(
                    "{}: {}",
                    vehicle.name,
                    transition_name(&vehicle.cabin, step.transition())
                )
            }
        }
    }

    /// `firings` more firings of `step_deep`, named by [`by_vehicle`].
    fn step(city: &mut City, firings: usize) -> Vec<String> {
        (0..firings)
            .map(|_| {
                let step = city
                    .net
                    .step_deep()
                    .expect("no body fails")
                    .expect("something to fire");
                by_vehicle(city, &step)
            })
            .collect()
    }

    /// The city after `firings` firings of `step_deep` from the seed, and
    /// those firings.
    fn after(firings: usize) -> (City, Vec<String>) {
        let mut city = build_city().expect("the city builds");
        let story = step(&mut city, firings);
        (city, story)
    }

    /// The bus's token id at a place, and its cabin's id.
    fn bus_at(city: &City, place: PlaceRef<Vehicle>) -> (TokenId, petritype::NetId) {
        let (token, bus) = city.vehicle(place).expect("the bus is there");
        assert_eq!(bus.name, "Bus 1");
        (token, bus.cabin.id())
    }

    /// A vehicle's phase, read as the city's guards read it: a door open
    /// and no announcement standing is boarding, a door open and an
    /// announcement standing alighting, every door closed under way.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    enum Phase {
        Boarding,
        Alighting,
        UnderWay,
    }

    fn phase(cabin: &Net) -> Phase {
        match (
            names(cabin, "Doors Open").is_empty(),
            names(cabin, "Announcements").is_empty(),
        ) {
            (true, _) => Phase::UnderWay,
            (false, true) => Phase::Boarding,
            (false, false) => Phase::Alighting,
        }
    }

    /// A cabin's doors, open and closed, by name.
    fn doors(cabin: &Net) -> (Vec<String>, Vec<String>) {
        (names(cabin, "Doors Open"), names(cabin, "Doors Closed"))
    }

    /// Door names as a cabin holds them.
    fn owned(doors: &[&str]) -> Vec<String> {
        doors.iter().map(|door| door.to_string()).collect()
    }

    /// Every passenger the city is built with.
    fn everyone() -> BTreeSet<String> {
        WAITING
            .iter()
            .flat_map(|(_, names)| names.iter().map(|name| name.to_string()))
            .collect()
    }

    /// Where each passenger is: a platform, `Arrived`, or a place inside a
    /// vehicle, `"Bus 1 Seats"`. Everyone, once each, is the promise.
    fn whereabouts(city: &City) -> BTreeMap<String, Vec<String>> {
        let mut found: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (_, info) in city.net.places().filter(|(_, info)| !info.nested) {
            for who in names(&city.net, info.name) {
                found.entry(who).or_default().push(info.name.to_owned());
            }
        }
        for (_, _, vehicle) in city.vehicles() {
            for (_, info) in vehicle.cabin.places() {
                if CONTROL_PLACES.contains(&info.name) {
                    continue;
                }
                for who in names(&vehicle.cabin, info.name) {
                    let place = format!("{} {}", vehicle.name, info.name);
                    found.entry(who).or_default().push(place);
                }
            }
        }
        found
    }

    #[test]
    fn the_city_is_the_conformance_net_of_protocol_section_11() {
        let city = build_city().expect("the city builds");
        let places: Vec<(&str, bool, bool)> = city
            .net
            .places()
            .map(|(_, info)| (info.name, info.nested, info.serializable))
            .collect();
        assert_eq!(
            places,
            [
                ("Depot", true, true),
                ("Central", true, true),
                ("Harbour", true, true),
                ("Airport", true, true),
                ("Platform@Depot", false, true),
                ("Platform@Central", false, true),
                ("Platform@Harbour", false, true),
                ("Platform@Airport", false, true),
                ("In Transit", true, true),
                ("Arrived", false, true),
            ],
        );
        let transitions: Vec<&str> = city.net.transitions().map(|(_, info)| info.name).collect();
        assert_eq!(
            transitions,
            [
                "Board Depot",
                "Board Central",
                "Board Harbour",
                "Board Airport",
                "Alight Depot",
                "Alight Central",
                "Alight Harbour",
                "Alight Airport",
                "Depart Depot",
                "Depart Central",
                "Depart Harbour",
                "Depart Airport",
                "Arrive",
            ],
        );
        assert!(
            city.net.transitions().all(|(_, info)| info.enabled),
            "every transition starts switched on"
        );
        assert_eq!(city.net.param::<Destination>(), Some(&Destination(None)));
        assert_eq!(
            city.net.param_name(std::any::type_name::<Destination>()),
            Some("destination")
        );

        // The seed, as a save writes it: two vehicles, nobody aboard, every
        // door open, and seven passengers on three platforms.
        let empty = |template: &str, marking: Value, params: Value, enabled: Value| {
            json!({"template": template, "marking": marking, "params": params,
                   "enabled": enabled})
        };
        let switches = |names: &[&str]| {
            Value::Object(names.iter().map(|n| (n.to_string(), json!(true))).collect())
        };
        let bus = empty(
            "Cabin",
            json!({"Door": [], "Seats": [], "Standing": [], "Announcements": [],
                   "Doors Open": ["Front"], "Doors Closed": []}),
            json!({"stopped": true, "seats": 2, "capacity": 6}),
            switches(&[
                "Sit",
                "Stand",
                "Go To Door",
                "Go To Door From Standing",
                "Close Doors",
                "Clear Announcement",
            ]),
        );
        let ferry = empty(
            "Deck",
            json!({"Door": [], "Lower Deck": [], "Upper Deck": [], "Announcements": [],
                   "Doors Open": ["Bow", "Stern"], "Doors Closed": []}),
            json!({"stopped": true, "capacity": 40}),
            switches(&[
                "Go Below",
                "Go Up",
                "Go To Door",
                "Go To Door From Upper Deck",
                "Close Doors",
                "Clear Announcement",
            ]),
        );
        assert_eq!(
            serde_json::to_value(city.net.export_marking().expect("saves")).expect("JSON"),
            json!({
                "Depot": [{"name": "Bus 1", "kind": "Bus", "destination": null, "cabin": bus}],
                "Central": [],
                "Harbour": [{"name": "Ferry 1", "kind": "Ferry", "destination": null,
                             "cabin": ferry}],
                "Airport": [],
                "Platform@Depot": ["Ada", "Bram", "Cato"],
                "Platform@Central": ["Dara", "Eve"],
                "Platform@Harbour": ["Finn", "Gwen"],
                "Platform@Airport": [],
                "In Transit": [],
                "Arrived": [],
            }),
        );

        // The third template, which nothing in the seed is built from.
        let pod = templates()
            .build("Pod", &Params::new())
            .expect("a pod builds");
        assert_eq!(
            serde_json::to_value(&pod).expect("a pod saves"),
            empty(
                "Pod",
                json!({"Door": [], "Seats": [], "Announcements": [],
                       "Doors Open": ["Front"], "Doors Closed": []}),
                json!({"stopped": true, "seats": 4, "capacity": 4}),
                switches(&["Sit", "Go To Door", "Close Doors", "Clear Announcement"]),
            ),
        );
        let order = |net: &Net| -> (Vec<String>, Vec<String>) {
            (
                net.places().map(|(_, info)| info.name.to_owned()).collect(),
                net.transitions()
                    .map(|(_, info)| info.name.to_owned())
                    .collect(),
            )
        };
        let (places, transitions) = order(&pod);
        assert_eq!(
            places,
            [
                "Door",
                "Seats",
                "Announcements",
                "Doors Open",
                "Doors Closed"
            ]
        );
        assert_eq!(
            transitions,
            ["Sit", "Go To Door", "Close Doors", "Clear Announcement"]
        );
        let cabin = city.inside(city.stations.at(Location::Depot)).expect("bus");
        let (places, transitions) = order(cabin);
        assert_eq!(
            places,
            [
                "Door",
                "Seats",
                "Standing",
                "Announcements",
                "Doors Open",
                "Doors Closed"
            ]
        );
        assert_eq!(
            transitions,
            [
                "Sit",
                "Stand",
                "Go To Door",
                "Go To Door From Standing",
                "Close Doors",
                "Clear Announcement",
            ],
        );
        let deck = city
            .inside(city.stations.at(Location::Harbour))
            .expect("ferry");
        let (places, transitions) = order(deck);
        assert_eq!(
            places,
            [
                "Door",
                "Lower Deck",
                "Upper Deck",
                "Announcements",
                "Doors Open",
                "Doors Closed"
            ]
        );
        assert_eq!(
            transitions,
            [
                "Go Below",
                "Go Up",
                "Go To Door",
                "Go To Door From Upper Deck",
                "Close Doors",
                "Clear Announcement",
            ],
        );
    }

    #[test]
    fn the_journey_runs_on_its_own_under_plain_step_deep() {
        let mut city = build_city().expect("the city builds");
        let mut story = Vec::new();
        journey(&mut city, |city, _, step| {
            story.push(by_vehicle(city, step));
            Ok(())
        })
        .expect("everyone arrives within the budget");
        assert_eq!(
            story,
            [
                "Board Depot",
                "Board Depot",
                "Board Depot",
                "Board Harbour",
                "Board Harbour",
                // Nothing left for the city: the bus seats its passengers
                // and, once nobody is at the door, closes it.
                "Bus 1: Sit",
                "Bus 1: Sit",
                "Bus 1: Stand",
                "Bus 1: Close Doors",
                "Depart Depot",
                // At Central, its door opened by `Arrive` itself.
                "Arrive",
                "Bus 1: Go To Door",
                "Alight Central",
                "Bus 1: Go To Door",
                "Alight Central",
                "Bus 1: Go To Door From Standing",
                "Alight Central",
                "Bus 1: Clear Announcement",
                "Board Central",
                "Board Central",
                "Bus 1: Sit",
                "Bus 1: Sit",
                "Bus 1: Close Doors",
                "Depart Central",
                // At Harbour, behind the ferry, whose passengers boarded at
                // the start and whose deck has its turn now: two doors,
                // closed one at a time.
                "Arrive",
                "Ferry 1: Go Below",
                "Ferry 1: Go Below",
                "Ferry 1: Go Up",
                "Ferry 1: Go Up",
                "Ferry 1: Close Doors",
                "Ferry 1: Close Doors",
                "Depart Harbour",
                // At the Airport; the bus, at Harbour, has its turn.
                "Arrive",
                "Bus 1: Go To Door",
                "Alight Harbour",
                "Bus 1: Go To Door",
                "Alight Harbour",
                "Bus 1: Clear Announcement",
                // Empty, it closes up and moves on, behind the ferry.
                "Bus 1: Close Doors",
                "Depart Harbour",
                "Arrive",
                "Ferry 1: Go To Door From Upper Deck",
                "Alight Airport",
                "Ferry 1: Go To Door From Upper Deck",
                "Alight Airport",
            ],
        );
    }

    #[test]
    fn all_seven_arrive_and_nobody_steps_off_where_they_got_on() {
        let mut city = build_city().expect("the city builds");
        let mut boarded_at: BTreeMap<String, Location> = BTreeMap::new();
        let mut arrived: Vec<String> = Vec::new();
        let fired = journey(&mut city, |city, n, step| {
            let found = whereabouts(city);
            assert_eq!(
                found.keys().cloned().collect::<BTreeSet<_>>(),
                everyone(),
                "after firing {n}"
            );
            for (who, places) in &found {
                assert_eq!(places.len(), 1, "after firing {n}, {who} is in {places:?}");
            }
            let now = names(&city.net, "Arrived");
            assert!(now.starts_with(&arrived), "nobody leaves Arrived");
            if let DeepStep::Here { transition } = step {
                for location in Location::ALL {
                    if *transition == city.board[location as usize] {
                        // The newest at the door of the front vehicle there.
                        let cabin = city.inside(city.stations.at(location)).expect("boarded");
                        let who = names(cabin, "Door").pop().expect("at the door");
                        assert_eq!(boarded_at.insert(who, location), None, "boards once");
                    }
                    if *transition == city.alight[location as usize] {
                        let who = now.last().expect("stepped off into Arrived");
                        assert_ne!(
                            boarded_at.get(who),
                            Some(&location),
                            "{who} stepped off where they got on"
                        );
                    }
                }
            }
            arrived = now;
            Ok(())
        })
        .expect("everyone arrives within the budget");
        assert_eq!(fired, 45);
        assert_eq!(arrived.iter().cloned().collect::<BTreeSet<_>>(), everyone());
        assert_eq!(
            arrived,
            ["Ada", "Bram", "Cato", "Dara", "Eve", "Finn", "Gwen"],
            "in the order they stepped off"
        );
        assert_eq!(boarded_at.len(), 7);
    }

    #[test]
    fn every_door_is_open_or_closed_at_every_step_and_closed_in_transit() {
        let mut city = build_city().expect("the city builds");
        let mut seen = BTreeSet::new();
        let mut half_closed = false;
        for n in 1..=BUDGET {
            city.net
                .step_deep()
                .expect("no body fails")
                .expect("the vehicles go round for ever");
            for (place, _, vehicle) in city.vehicles() {
                let cabin = &vehicle.cabin;
                let (open, closed) = doors(cabin);
                let mut every = [open.clone(), closed.clone()].concat();
                every.sort();
                let mut built = owned(vehicle.kind.doors());
                built.sort();
                assert_eq!(
                    every, built,
                    "{} after firing {n}: each door once, open or closed",
                    vehicle.name
                );
                let phase = phase(cabin);
                if place.id() == city.in_transit.id() {
                    assert_eq!(
                        phase,
                        Phase::UnderWay,
                        "{} travels with every door closed",
                        vehicle.name
                    );
                }
                if phase == Phase::UnderWay {
                    assert!(names(cabin, "Announcements").is_empty(), "no door to it");
                }
                half_closed |= !open.is_empty() && !closed.is_empty();
                seen.insert(phase);
            }
        }
        assert_eq!(seen.len(), 3, "every phase came round");
        assert!(half_closed, "the ferry closes its doors one at a time");
        assert!(city.all_arrived());
    }

    #[test]
    fn everyone_arrives_whichever_way_ties_are_broken() {
        let mut city = build_city().expect("the city builds");
        city.net.set_tie_break(TieBreak::LastDeclared);
        let vehicles: Vec<_> = city
            .vehicles()
            .map(|(place, token, _)| (place.id(), token))
            .collect();
        for (place, token) in vehicles {
            city.net
                .nested_scope(place, token, |nets| {
                    nets[0].1.set_tie_break(TieBreak::LastDeclared);
                })
                .expect("a nested place");
        }
        let mut story = Vec::new();
        journey(&mut city, |city, _, step| {
            story.push(by_vehicle(city, step));
            Ok(())
        })
        .expect("everyone arrives within the budget");
        assert_eq!(
            &story[..2],
            ["Board Harbour", "Board Harbour"],
            "the other way"
        );
        assert!(city.all_arrived());
    }

    #[test]
    fn a_departure_with_the_override_set_goes_where_it_says() {
        let mut city = build_city().expect("the city builds");
        city.net
            .set_param(Destination(Some(Location::Airport)))
            .expect("the city has a destination");
        assert_eq!(
            &step(&mut city, 10)[8..],
            ["Bus 1: Close Doors", "Depart Depot"]
        );
        let (_, bus) = city.vehicle(city.in_transit).expect("in transit");
        assert_eq!(bus.destination, Some(Location::Airport), "not Central");
        assert_eq!(step(&mut city, 1), ["Arrive"]);
        let cabin = city
            .inside(city.stations.at(Location::Airport))
            .expect("the bus is at the Airport");
        assert_eq!(names(cabin, "Announcements"), ["Arrived at Airport"]);
        assert_eq!(cabin.param::<Stopped>(), Some(&Stopped(true)));
        assert_eq!(doors(cabin), (owned(&["Front"]), vec![]), "opened");
        assert_eq!(phase(cabin), Phase::Alighting);
        assert_eq!(city.net.count(city.stations.at(Location::Central).id()), 0);

        // Set back, the next departure from the Airport follows the loop.
        city.net.set_param(Destination(None)).expect("set back");
        let depart = city.depart[Location::Airport as usize];
        for _ in 0..BUDGET {
            let step = city.net.step_deep().expect("fires").expect("something");
            if matches!(step, DeepStep::Here { transition } if transition == depart) {
                let (_, bus) = city.vehicle(city.in_transit).expect("in transit");
                assert_eq!(bus.destination, Some(Location::Depot));
                assert_eq!(names(&city.net, "Arrived"), ["Ada", "Bram", "Cato"]);
                return;
            }
        }
        panic!("the bus never left the Airport");
    }

    #[test]
    fn saved_mid_journey_and_loaded_the_city_carries_on_to_the_same_end() {
        let (mut straight, story) = after(10);
        assert_eq!(story.last().map(String::as_str), Some("Depart Depot"));
        let text = save(&straight).expect("every place is serializable");
        let mut loaded = load(&text).expect("the saved city loads");
        assert_eq!(
            loaded.net.export_marking().expect("saves"),
            straight.net.export_marking().expect("saves"),
            "what was saved is what was loaded",
        );

        let rest = journey(&mut straight, |_, _, _| Ok(())).expect("everyone arrives");
        let more = journey(&mut loaded, |_, _, _| Ok(())).expect("in the loaded city too");
        assert_eq!((10 + rest, more), (45, rest), "in as many firings");
        // By place name, token order and value, inside the vehicles too.
        let end = save(&straight).expect("saves");
        assert_eq!(save(&loaded).expect("saves"), end);

        // And the end, saved and loaded, is itself.
        let again = load(&end).expect("loads");
        assert_eq!(save(&again).expect("saves"), end);
    }

    #[test]
    fn a_loaded_cabin_is_a_new_net_with_its_parameters_and_switches() {
        let (mut city, _) = after(10);
        // A switch inside, thrown, to see it come back: what a save keeps,
        // not how the journey runs.
        let (token, _) = city.vehicle(city.in_transit).expect("the bus");
        city.net
            .nested_scope(city.in_transit.id(), token, |nets| {
                let cabin = &mut *nets[0].1;
                let clear = cabin
                    .transition_named("Clear Announcement")
                    .expect("a cabin transition");
                cabin.disable(clear).expect("disables");
            })
            .expect("a nested place");
        let text = save(&city).expect("saves");
        let loaded = load(&text).expect("loads");

        let before = city.inside(city.in_transit).expect("the bus");
        let after = loaded.inside(loaded.in_transit).expect("the bus");
        assert_ne!(after.id(), before.id(), "a restored cabin is a new net");
        assert_eq!(after.template(), Some("Cabin"));
        assert_eq!(after.param::<Stopped>(), Some(&Stopped(false)));
        assert_eq!(after.param::<SeatCount>(), Some(&SeatCount(2)));
        assert_eq!(after.param::<Capacity>(), Some(&Capacity(6)));
        assert_eq!(doors(after), (vec![], owned(&["Front"])), "closed");
        for (transition, info) in before.transitions() {
            let same = after.transition_named(info.name).expect("same template");
            assert_eq!(after.is_enabled(same), before.is_enabled(transition));
        }
        let clear = after.transition_named("Clear Announcement").expect("there");
        assert!(!after.is_enabled(clear));
        assert_eq!(
            loaded
                .vehicle(loaded.in_transit)
                .map(|(_, bus)| bus.destination),
            Some(Some(Location::Central)),
        );
        assert_eq!(after.state_counter(), 0, "restoring is setup");
    }

    #[test]
    fn the_bus_saves_in_the_shape_of_protocol_section_12_5() {
        let (city, _) = after(9);
        let (_, bus) = city
            .vehicle(city.stations.at(Location::Depot))
            .expect("the bus");
        assert_eq!(
            serde_json::to_value(bus).expect("the bus serialises"),
            json!({"name": "Bus 1", "kind": "Bus", "destination": null,
                   "cabin": {"template": "Cabin",
                             "marking": {"Door": [], "Seats": ["Ada", "Bram"], "Standing": ["Cato"],
                                         "Announcements": [], "Doors Open": [],
                                         "Doors Closed": ["Front"]},
                             "params": {"stopped": true, "seats": 2, "capacity": 6},
                             "enabled": {"Sit": true, "Stand": true, "Go To Door": true,
                                         "Go To Door From Standing": true,
                                         "Close Doors": true, "Clear Announcement": true}}}),
        );
    }

    #[test]
    fn a_snapshot_of_the_city_forks_every_vehicle() {
        // Both vehicles at Harbour, the ferry in front.
        let (city, _) = after(25);
        let harbour = city.stations.at(Location::Harbour);
        assert_eq!(city.net.count(harbour.id()), 2);
        let snapshot = city.net.snapshot().expect("the vehicles can be forked");
        let mut seen = 0;
        for (place, token, _) in city.vehicles() {
            let live = city.net.nested(place.id(), token).expect("nested")[0].1;
            let copy = snapshot.nested(place.id(), token).expect("in the snapshot")[0].1;
            assert_eq!(
                copy.export_marking().expect("saves"),
                live.export_marking().expect("saves"),
            );
            assert_ne!(copy.id(), live.id(), "a fork is a new net");
            seen += 1;
        }
        assert_eq!(seen, 2, "the bus and the ferry");
        assert_eq!(
            snapshot.count(city.arrived.id()),
            3,
            "the rest of the city is cloned as ever"
        );
        let (ferry, _) = city.vehicle(harbour).expect("the ferry");
        let cabin_id = |snapshot: &petritype::Snapshot| {
            snapshot
                .nested(harbour.id(), ferry)
                .map(|nets| nets[0].1.id())
        };
        assert_eq!(
            cabin_id(&snapshot.clone()),
            cabin_id(&snapshot),
            "a clone of a snapshot shares its forks",
        );
    }

    #[test]
    fn the_bus_is_followed_by_its_nested_id_across_token_ids() {
        let mut city = build_city().expect("the city builds");
        let depot = city.stations.at(Location::Depot);
        let (first_token, nested_id) = bus_at(&city, depot);
        // From the seed to the bus's arrival at Central.
        assert_eq!(
            step(&mut city, 11).last().map(String::as_str),
            Some("Arrive")
        );
        let log: Vec<Event> = city.net.drain_events().collect();

        // Everything that happened inside the bus, by where the bus was.
        let mut stops: Vec<(String, TokenId)> = Vec::new();
        for event in &log {
            if let Event::Nested {
                place,
                token,
                nested_id: id,
                ..
            } = event
                && *id == nested_id
            {
                let stop = (place_name(&city.net, *place), *token);
                if stops.last() != Some(&stop) {
                    stops.push(stop);
                }
            }
        }
        let (last_token, still) = bus_at(&city, city.stations.at(Location::Central));
        assert_eq!(still, nested_id, "the cabin never changed");
        assert_eq!(stops.len(), 3, "{stops:?}");
        assert_eq!(stops[0], ("Depot".to_owned(), first_token));
        assert_eq!(stops[1].0, "In Transit");
        assert_eq!(stops[2], ("Central".to_owned(), last_token));
        assert!(
            first_token != stops[1].1 && stops[1].1 != last_token,
            "a new token id at every move"
        );
        // The ferry's boarding is in the log too, under its own id.
        let (_, ferry) = city
            .vehicle(city.stations.at(Location::Harbour))
            .expect("the ferry");
        assert!(log.iter().any(
            |event| matches!(event, Event::Nested { nested_id: id, .. } if *id == ferry.cabin.id())
        ));
    }

    #[test]
    fn board_is_refused_when_the_bus_is_full() {
        let mut city = build_city().expect("the city builds");
        let platform = city.platforms.at(Location::Depot);
        // Six fit aboard a bus; three are waiting already.
        city.net
            .extend(platform, ["Dee", "Eli", "Fay", "Gus"].map(String::from));
        let story = step(&mut city, 9);
        assert_eq!(story[..6], ["Board Depot"; 6]);
        assert_eq!(story[6..], ["Board Harbour", "Board Harbour", "Bus 1: Sit"]);
        assert!(
            !city.net.is_fireable(city.board[Location::Depot as usize]),
            "the bus is full"
        );
        assert_eq!(names(&city.net, "Platform@Depot"), ["Gus"]);
    }

    #[test]
    fn every_template_closes_every_door_once_settled_and_once_emptied() {
        let templates = templates();
        for kind in [Kind::Bus, Kind::Ferry, Kind::Shuttle] {
            let template = kind.template();
            let every = owned(kind.doors());
            let mut net = templates.build(template, &Params::new()).expect("builds");
            assert_eq!(
                doors(&net),
                (every.clone(), vec![]),
                "{template}: built open"
            );
            let run = |net: &mut Net| {
                let summary =
                    drivers::run(net, Mode::Sequential, Limits::default(), OnFailure::Stop)
                        .expect("no body fails");
                assert!(summary.quiesced, "{template}");
            };
            let door = net.place_ref::<String>("Door").expect("a door");
            net.extend(door, ["Ada", "Bram", "Cato"].map(String::from));
            run(&mut net);
            assert_eq!(
                doors(&net),
                (vec![], every.clone()),
                "{template}: settled, every door closed, in order"
            );
            assert_eq!(net.count(door.id()), 0);
            assert_eq!(net::aboard(&net), 3);

            // An arrival, as `Arrive` makes one: every door opened, in
            // order, and the announcement put up.
            let open = net.place_ref::<String>("Doors Open").expect("there");
            let closed = net.place_ref::<String>("Doors Closed").expect("there");
            let shut = names(&net, "Doors Closed");
            net.set_tokens(closed, std::iter::empty());
            net.extend(open, shut);
            let announcements = net.place_ref::<String>("Announcements").expect("there");
            net.put(announcements, "Arrived at Central".to_owned());
            run(&mut net);
            assert_eq!(names(&net, "Door").len(), 3, "{template}: all at the door");
            assert_eq!(
                doors(&net),
                (every.clone(), vec![]),
                "open while anyone is aboard"
            );

            // Everyone steps off, as `Alight` takes them.
            let gone: Vec<TokenId> = net.tokens(door.id()).map(|(token, _)| token).collect();
            for token in gone {
                net.remove(door.id(), token).expect("removes");
            }
            run(&mut net);
            assert_eq!(names(&net, "Announcements"), Vec::<String>::new());
            assert_eq!(
                doors(&net),
                (vec![], every),
                "{template}: the announcement down, and empty, closed again"
            );
        }
    }

    #[test]
    fn every_kind_has_a_cabin_of_its_own_template() {
        let templates = templates();
        let tram = Vehicle::new("Tram 1", Kind::Tram, &templates).expect("a cabin");
        let ferry = Vehicle::new("Ferry 2", Kind::Ferry, &templates).expect("a deck");
        let shuttle = Vehicle::new("Shuttle 1", Kind::Shuttle, &templates).expect("a pod");
        use petritype::Nested;
        for vehicle in [&tram, &ferry, &shuttle] {
            assert_eq!(vehicle.nets()[0].0, "cabin", "one name whatever the kind");
            assert_eq!(phase(&vehicle.cabin), Phase::Boarding, "built boarding");
            assert_eq!(
                doors(&vehicle.cabin),
                (owned(vehicle.kind.doors()), vec![]),
                "every door open"
            );
        }
        assert_eq!(Kind::Tram.doors(), ["Front"]);
        assert_eq!(Kind::Ferry.doors(), ["Bow", "Stern"]);
        assert_eq!(Kind::Shuttle.doors(), ["Front"]);
        assert_eq!(tram.cabin.template(), Some("Cabin"));
        assert_eq!(ferry.cabin.template(), Some("Deck"));
        assert_eq!(shuttle.cabin.template(), Some("Pod"));
        assert_eq!(tram.cabin.param::<SeatCount>(), Some(&SeatCount(2)));
        assert_eq!(shuttle.cabin.param::<Capacity>(), Some(&Capacity(4)));
        assert!(ferry.cabin.place_named("Upper Deck").is_some());
        assert!(ferry.cabin.param::<SeatCount>().is_none());
        assert!(shuttle.cabin.place_named("Standing").is_none());
    }

    #[test]
    fn the_loop_and_its_override() {
        let mut at = Location::Depot;
        let mut route = Vec::new();
        for _ in 0..4 {
            at = at.next();
            route.push(at);
        }
        use Location::*;
        assert_eq!(route, [Central, Harbour, Airport, Depot]);
        assert_eq!(
            serde_json::to_value(Destination(None)).expect("JSON"),
            Value::Null
        );
        assert_eq!(
            serde_json::from_value::<Destination>(json!("Harbour")).expect("a station"),
            Destination(Some(Harbour))
        );
        assert!(
            serde_json::from_value::<Destination>(json!(3)).is_err(),
            "a number is no destination"
        );
    }
}
