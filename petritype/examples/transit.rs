//! Transit: vehicles that carry Petri nets of their own (DESIGN §6.6).
//!
//! The example of the nets-as-tokens proposal
//! (`Cyberpunk2020-Simulation/dev-docs/NETS_AS_TOKENS.md` §2), built as the
//! conformance net `transit` of the shared protocol (`PROTOCOL.md` §11),
//! which lives in [`net`] so that a runner can carry the same file. The
//! outer net, `City`, has four stations — `Depot`, `Central`, `Harbour`,
//! `Airport` — each holding the vehicles standing there, a
//! `Platform@<station>` of passengers waiting at each, and `In Transit` for
//! the vehicles between them. A vehicle is a token, and it carries an
//! *inner* net, its `cabin`, whose shape depends on its kind: built from the
//! template `Cabin` for a bus or a tram, `Deck` for a ferry, `Pod` for a
//! shuttle. The city has one `Depot` place whatever is inside the vehicles
//! in it.
//!
//! Inside a vehicle, passengers come in at the `Door` and sit or stand; once
//! it has stopped and announced a station they go back to the door, and the
//! announcement is cleared when nobody is left aboard. Nothing inside a
//! vehicle touches the city. The city reaches in only through the vehicle
//! token it holds: `Board <station>` and `Alight <station>` put a passenger
//! into the cabin's `Door` and take one out with a [`MutateEdge`] body;
//! `Depart <station>` and `Arrive` move the vehicle and set the cabin's
//! parameter `stopped`, and `Arrive` puts an announcement inside. A guard on
//! `Board` reads the cabin's marking through [`Net::nested`] to see whether
//! there is room.
//!
//! **The script fires the city only on orders.** The city is built with
//! every transition switched on, as the protocol's conformance net is; the
//! script holds them all back first ([`City::hold_back`]), and an order
//! switches one on for a single [`Net::fire`] and off again
//! ([`City::order`]). [`drivers::run`] then has nothing to fire in the city
//! itself and runs the vehicles' cabins, which is what "run to quiet" means
//! in the script below, and the city is honestly quiescent between orders,
//! so [`Net::is_quiescent_deep`] answers what the script asks of it.
//!
//! The acceptance script, steps 1 to 7 of the proposal, is `main` and the
//! tests below. Where the proposal's §6 has the announcement cleared after
//! step 4, it is not: the protocol's guards (see [`net`]) clear it only once
//! the vehicle is empty, which the Python track adopted after finding that
//! the proposal's guards re-seat the passengers at the door. Step 6 saves
//! the city's marking as JSON text — the vehicles' cabins in it, each as the
//! template that built it with its marking, parameters and switches — loads
//! it into a freshly built city, and carries on.
//!
//! [`MutateEdge`]: petritype::MutateEdge
//! [`drivers::run`]: petritype::drivers::run

// A crate root's `mod` looks beside it, so the path is spelled out: the net
// lives in `transit/net.rs`, the file a runner's example includes too.
#[path = "transit/net.rs"]
mod net;

use petritype::serde_json;
use petritype::{Event, Marking, Net, Params, PlaceId, TransitionId};

use net::{BoxedError, City, Kind, Location, Vehicle, build_city, cabin, templates};

/// The passengers on a platform, by name, oldest first.
fn waiting(city: &City, location: Location) -> Vec<String> {
    city.net
        .tokens(city.platforms.at(location).id())
        .filter_map(|(_, value)| value.downcast_ref::<String>().cloned())
        .collect()
}

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

/// The firings in a stretch of the city's log, in order, each as
/// `"<transition>"` for the city's own or `"<place> / #<token> / <net>:
/// <transition>"` for one inside a vehicle.
///
/// Inner transitions are named from a fresh cabin: every net built from one
/// template numbers its transitions the same way.
fn firings(city: &Net, events: &[Event]) -> Vec<String> {
    let template = cabin(&Params::new()).expect("the cabin template builds");
    events
        .iter()
        .filter_map(|event| match event {
            Event::Fired { transition, .. } => Some(transition_name(city, *transition)),
            Event::Nested {
                place,
                token,
                net,
                event,
                ..
            } => match &**event {
                Event::Fired { transition, .. } => Some(format!(
                    "{} / #{token} / {net}: {}",
                    place_name(city, *place),
                    transition_name(&template, *transition),
                )),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// The script, steps 1 to 6.
mod script {
    use super::*;
    use petritype::drivers::RunSummary;

    /// Step 1: the city as built, held back so that it fires only on
    /// orders.
    pub(super) fn city() -> Result<City, BoxedError> {
        let mut city = build_city()?;
        city.hold_back();
        Ok(city)
    }

    /// Step 2: `Board Depot` three times, then run to quiet.
    pub(super) fn board(city: &mut City) -> Result<RunSummary, BoxedError> {
        for _ in 0..3 {
            city.order(city.board[Location::Depot as usize])?;
        }
        Ok(city.run_to_quiet()?)
    }

    /// Step 3: `Depart Depot` for `Central`.
    pub(super) fn depart(city: &mut City) -> Result<(), BoxedError> {
        city.net.set_param(net::Destination(Location::Central))?;
        city.order(city.depart[Location::Depot as usize])?;
        Ok(())
    }

    /// Step 4: `Arrive`, then run to quiet.
    pub(super) fn arrive(city: &mut City) -> Result<RunSummary, BoxedError> {
        city.order(city.arrive)?;
        Ok(city.run_to_quiet()?)
    }

    /// Step 5: `Alight Central` twice.
    pub(super) fn alight(city: &mut City) -> Result<(), BoxedError> {
        for _ in 0..2 {
            city.order(city.alight[Location::Central as usize])?;
        }
        Ok(())
    }

    /// Step 6, the first half: the city's marking as JSON text, every
    /// vehicle's cabin inside it.
    pub(super) fn save(city: &City) -> Result<String, BoxedError> {
        Ok(serde_json::to_string(&city.net.export_marking()?)?)
    }

    /// Step 6, the second half: a freshly built city, held back as the
    /// script's is, with a saved marking loaded into it. The import's one
    /// `Resynced` is dropped with the rest of the set-up.
    pub(super) fn load(text: &str) -> Result<City, BoxedError> {
        let mut city = city()?;
        let marking: Marking = serde_json::from_str(text)?;
        city.net.import_marking(&marking)?;
        city.net.drain_events().for_each(drop);
        Ok(city)
    }
}

fn main() -> Result<(), BoxedError> {
    let mut city = script::city()?;
    println!("{}", petritype_dot::dot(&city.net));
    let templates = templates();
    for kind in Kind::ALL {
        let vehicle = Vehicle::new(&format!("{kind:?}"), kind, &templates)?;
        let places: Vec<&str> = vehicle.cabin.places().map(|(_, info)| info.name).collect();
        println!("a {kind:?} carries a {}: {places:?}", kind.template());
    }
    println!("waiting at Depot: {:?}\n", waiting(&city, Location::Depot));
    let mut log = Vec::new();

    let summary = script::board(&mut city)?;
    log.extend(city.net.drain_events());
    let depot = city.stations.at(Location::Depot);
    let cabin = city.inside(depot).expect("the bus");
    println!(
        "after boarding ({} inner firings): seats {:?}, standing {:?}, door {:?}",
        summary.fired_count,
        names(cabin, "Seats"),
        names(cabin, "Standing"),
        names(cabin, "Door"),
    );
    let (_, bus) = city.vehicle(depot).expect("the bus");
    println!(
        "the bus, as a saved city writes it:\n{}",
        serde_json::to_string_pretty(bus)?
    );

    script::depart(&mut city)?;
    log.extend(city.net.drain_events());
    let (_, bus) = city.vehicle(city.in_transit).expect("the bus");
    println!(
        "in transit to {:?}; quiescent all the way down: {}",
        bus.destination,
        city.net.is_quiescent_deep()
    );
    let saved = script::save(&city)?;

    script::arrive(&mut city)?;
    log.extend(city.net.drain_events());
    let cabin = city
        .inside(city.stations.at(Location::Central))
        .expect("the bus");
    println!(
        "at Central: door {:?}, announcements {:?}",
        names(cabin, "Door"),
        names(cabin, "Announcements"),
    );

    script::alight(&mut city)?;
    log.extend(city.net.drain_events());
    let cabin = city
        .inside(city.stations.at(Location::Central))
        .expect("the bus");
    println!(
        "alighted: platform {:?}, still at the door {:?}",
        waiting(&city, Location::Central),
        names(cabin, "Door"),
    );

    let mut loaded = script::load(&saved)?;
    script::arrive(&mut loaded)?;
    script::alight(&mut loaded)?;
    println!(
        "\nsaved in transit ({} bytes), loaded into a fresh city and carried on: \
         the same marking as the straight run, inside the vehicles too: {}",
        saved.len(),
        loaded.net.export_marking()? == city.net.export_marking()?,
    );

    println!("\nthe firings, with paths:");
    for firing in firings(&city.net, &log) {
        println!("  {firing}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use net::{Capacity, Destination, SeatCount, Stopped};
    use petritype::{NetError, PlaceRef, TokenId};
    use serde_json::json;

    /// The city after the first `step` steps of the script (1 to 5), and
    /// everything it logged on the way.
    fn after(step: u32) -> (City, Vec<Event>) {
        let mut city = script::city().expect("the city builds");
        let mut log = Vec::new();
        if step >= 2 {
            script::board(&mut city).expect("boarding works");
            log.extend(city.net.drain_events());
        }
        if step >= 3 {
            script::depart(&mut city).expect("the bus departs");
            log.extend(city.net.drain_events());
        }
        if step >= 4 {
            script::arrive(&mut city).expect("the bus arrives");
            log.extend(city.net.drain_events());
        }
        if step >= 5 {
            script::alight(&mut city).expect("two passengers alight");
            log.extend(city.net.drain_events());
        }
        (city, log)
    }

    /// The bus's token id at a station, and its cabin's id.
    fn bus_at(city: &City, place: PlaceRef<Vehicle>) -> (TokenId, petritype::NetId) {
        let (token, bus) = city.vehicle(place).expect("the bus is there");
        assert_eq!(bus.name, "Bus 1");
        (token, bus.cabin.id())
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
        assert_eq!(
            city.net.param::<Destination>(),
            Some(&Destination(Location::Central))
        );
        let ferry = city
            .inside(city.stations.at(Location::Harbour))
            .expect("the ferry");
        assert_eq!(ferry.template(), Some("Deck"));
        let cabin = cabin(&Params::new()).expect("a cabin");
        let inner: Vec<&str> = cabin.transitions().map(|(_, info)| info.name).collect();
        assert_eq!(
            inner,
            [
                "Sit",
                "Stand",
                "Go To Door",
                "Go To Door From Standing",
                "Clear Announcement",
            ],
        );
    }

    #[test]
    fn step_1_the_city_is_built_and_waits_for_orders() {
        let (city, _) = after(1);

        let depot = city.stations.at(Location::Depot);
        let nets = city
            .net
            .nested(depot.id(), bus_at(&city, depot).0)
            .expect("Depot is a nested place");
        assert_eq!(nets.len(), 1);
        assert_eq!(nets[0].0, "cabin");
        assert_eq!(nets[0].1.template(), Some("Cabin"));
        assert_eq!(
            city.vehicle(city.stations.at(Location::Harbour))
                .map(|(_, ferry)| ferry.kind),
            Some(Kind::Ferry),
        );
        assert_eq!(names(&city.net, "Platform@Depot"), ["Ada", "Bram", "Cato"]);
        assert!(city.net.fireable().is_empty(), "every order is held back");
        assert!(city.net.is_quiescent_deep());
        let info = city.net.places().next().expect("Depot").1;
        assert!(info.nested && !info.passive);
    }

    #[test]
    fn step_2_three_board_and_the_bus_sorts_them_out_inside() {
        let mut city = script::city().expect("the city builds");
        let summary = script::board(&mut city).expect("boarding works");
        let log: Vec<Event> = city.net.drain_events().collect();
        assert!(summary.quiesced);
        assert_eq!(summary.fired_count, 3, "the inner firings are counted");

        let depot = city.stations.at(Location::Depot);
        let (bus, _) = bus_at(&city, depot);
        assert_eq!(city.net.count(city.platforms.at(Location::Depot).id()), 0);
        let cabin = city.inside(depot).expect("the bus");
        assert_eq!(names(cabin, "Seats"), ["Ada", "Bram"]);
        assert_eq!(names(cabin, "Standing"), ["Cato"]);
        assert!(names(cabin, "Door").is_empty());

        let inner: Vec<String> = firings(&city.net, &log)
            .into_iter()
            .filter(|firing| firing.contains(" / "))
            .collect();
        let path = format!("Depot / #{bus} / cabin");
        assert_eq!(
            inner,
            [
                format!("{path}: Sit"),
                format!("{path}: Sit"),
                format!("{path}: Stand"),
            ],
        );
    }

    #[test]
    fn step_3_the_bus_departs_and_nothing_inside_can_move() {
        let (city, _) = after(3);

        assert_eq!(city.net.count(city.stations.at(Location::Depot).id()), 0);
        let (_, bus) = city
            .vehicle(city.in_transit)
            .expect("the bus is in transit");
        assert_eq!(bus.destination, Some(Location::Central));
        let cabin = city.inside(city.in_transit).expect("the bus");
        assert_eq!(cabin.param::<Stopped>(), Some(&Stopped(false)));
        assert_eq!(names(cabin, "Seats"), ["Ada", "Bram"], "unchanged");
        assert_eq!(names(cabin, "Standing"), ["Cato"]);
        assert!(city.net.is_quiescent_deep(), "nothing to announce yet");
    }

    #[test]
    fn step_4_the_bus_arrives_and_everyone_goes_to_the_door() {
        let (city, log) = after(4);

        let central = city.stations.at(Location::Central);
        assert_eq!(city.net.count(central.id()), 1);
        assert_eq!(city.net.count(city.in_transit.id()), 0);
        let cabin = city.inside(central).expect("the bus");
        assert_eq!(names(cabin, "Door"), ["Ada", "Bram", "Cato"]);
        // PROTOCOL §11: the announcement stands while anyone is aboard, the
        // door included, so that nobody sits down again.
        assert_eq!(names(cabin, "Announcements"), ["Arrived at Central"]);
        assert!(city.net.is_quiescent_deep());

        let inner: Vec<String> = firings(&city.net, &log)
            .into_iter()
            .filter_map(|firing| firing.split_once(": ").map(|(_, name)| name.to_owned()))
            .filter(|name| name.starts_with("Go To Door") || name == "Clear Announcement")
            .collect();
        assert_eq!(
            inner,
            ["Go To Door", "Go To Door", "Go To Door From Standing"],
        );
    }

    #[test]
    fn step_5_two_alight_and_one_stays_at_the_door() {
        let (city, _) = after(5);

        assert_eq!(names(&city.net, "Platform@Central"), ["Ada", "Bram"]);
        let cabin = city
            .inside(city.stations.at(Location::Central))
            .expect("the bus");
        assert_eq!(names(cabin, "Door"), ["Cato"]);
        assert_eq!(names(cabin, "Announcements"), ["Arrived at Central"]);
    }

    #[test]
    fn step_6_saved_in_transit_and_loaded_the_city_carries_on_to_the_same_end() {
        let (in_transit, _) = after(3);
        let text = script::save(&in_transit).expect("every place is serializable");
        let (straight, _) = after(5);

        let mut loaded = script::load(&text).expect("the saved city loads");
        assert_eq!(
            loaded.net.export_marking().expect("saves"),
            in_transit.net.export_marking().expect("saves"),
            "what was saved is what was loaded",
        );
        script::arrive(&mut loaded).expect("the loaded bus arrives");
        script::alight(&mut loaded).expect("two passengers alight");

        // By place name, token order and value, inside the vehicles too.
        assert_eq!(
            script::save(&loaded).expect("saves"),
            script::save(&straight).expect("saves"),
        );
        assert_eq!(names(&loaded.net, "Platform@Central"), ["Ada", "Bram"]);
        let cabin = loaded
            .inside(loaded.stations.at(Location::Central))
            .expect("the bus");
        assert_eq!(names(cabin, "Door"), ["Cato"]);
    }

    #[test]
    fn step_6_saved_at_the_end_and_loaded_the_city_is_the_same() {
        let (straight, _) = after(5);
        let text = script::save(&straight).expect("every place is serializable");
        let loaded = script::load(&text).expect("the saved city loads");
        assert_eq!(
            loaded.net.export_marking().expect("saves"),
            straight.net.export_marking().expect("saves"),
        );
        assert_eq!(script::save(&loaded).expect("saves"), text);
    }

    #[test]
    fn step_6_a_loaded_cabin_is_a_new_net_with_its_parameters_and_switches() {
        let (mut city, _) = after(3);
        // A switch inside, thrown, to see it come back.
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
        let text = script::save(&city).expect("saves");
        let loaded = script::load(&text).expect("loads");

        let before = city.inside(city.in_transit).expect("the bus");
        let after = loaded.inside(loaded.in_transit).expect("the bus");
        assert_ne!(after.id(), before.id(), "a restored cabin is a new net");
        assert_eq!(after.template(), Some("Cabin"));
        assert_eq!(after.param::<Stopped>(), Some(&Stopped(false)));
        assert_eq!(after.param::<SeatCount>(), Some(&SeatCount(2)));
        assert_eq!(after.param::<Capacity>(), Some(&Capacity(6)));
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
        let (city, _) = after(2);
        let (_, bus) = city
            .vehicle(city.stations.at(Location::Depot))
            .expect("the bus");
        assert_eq!(
            serde_json::to_value(bus).expect("the bus serialises"),
            json!({"name": "Bus 1", "kind": "Bus", "destination": null,
                   "cabin": {"template": "Cabin",
                             "marking": {"Door": [], "Seats": ["Ada", "Bram"], "Standing": ["Cato"],
                                         "Announcements": []},
                             "params": {"stopped": true, "seats": 2, "capacity": 6},
                             "enabled": {"Sit": true, "Stand": true, "Go To Door": true,
                                         "Go To Door From Standing": true,
                                         "Clear Announcement": true}}}),
        );
    }

    #[test]
    fn a_snapshot_of_the_city_forks_every_vehicle() {
        let (city, _) = after(5);
        let snapshot = city.net.snapshot().expect("the vehicles can be forked");
        let mut seen = 0;
        for place in city.stations.places.into_iter().chain([city.in_transit]) {
            for (token, _) in city.net.tokens(place.id()) {
                let live = city.net.nested(place.id(), token).expect("nested")[0].1;
                let copy = snapshot.nested(place.id(), token).expect("in the snapshot")[0].1;
                assert_eq!(
                    copy.export_marking().expect("saves"),
                    live.export_marking().expect("saves"),
                );
                assert_ne!(copy.id(), live.id(), "a fork is a new net");
                seen += 1;
            }
        }
        assert_eq!(seen, 2, "the bus and the ferry");
        assert_eq!(
            snapshot.count(city.platforms.at(Location::Central).id()),
            2,
            "the rest of the city is cloned as ever"
        );
        let central = city.stations.at(Location::Central);
        let (bus, _) = bus_at(&city, central);
        let cabin_id = |snapshot: &petritype::Snapshot| {
            snapshot
                .nested(central.id(), bus)
                .map(|nets| nets[0].1.id())
        };
        assert_eq!(
            cabin_id(&snapshot.clone()),
            cabin_id(&snapshot),
            "a clone of a snapshot shares its forks",
        );
    }

    #[test]
    fn step_7_the_log_tells_the_story_with_paths() {
        let (city, log) = after(5);
        let (bus_at_central, _) = bus_at(&city, city.stations.at(Location::Central));
        // The bus's first id: the one it had at Depot, read off the first
        // thing that happened inside it.
        let bus_at_depot = log
            .iter()
            .find_map(|event| match event {
                Event::Nested { token, .. } => Some(*token),
                _ => None,
            })
            .expect("something happened inside the bus");

        let depot = format!("Depot / #{bus_at_depot} / cabin");
        let central = format!("Central / #{bus_at_central} / cabin");
        assert_eq!(
            firings(&city.net, &log),
            [
                "Board Depot".to_owned(),
                "Board Depot".to_owned(),
                "Board Depot".to_owned(),
                format!("{depot}: Sit"),
                format!("{depot}: Sit"),
                format!("{depot}: Stand"),
                "Depart Depot".to_owned(),
                "Arrive".to_owned(),
                format!("{central}: Go To Door"),
                format!("{central}: Go To Door"),
                format!("{central}: Go To Door From Standing"),
                "Alight Central".to_owned(),
                "Alight Central".to_owned(),
            ],
        );
    }

    #[test]
    fn step_7_the_bus_is_followed_by_its_nested_id_across_token_ids() {
        let mut city = script::city().expect("the city builds");
        let depot = city.stations.at(Location::Depot);
        let (first_token, nested_id) = bus_at(&city, depot);

        let mut log = Vec::new();
        script::board(&mut city).expect("boarding works");
        script::depart(&mut city).expect("the bus departs");
        script::arrive(&mut city).expect("the bus arrives");
        script::alight(&mut city).expect("two passengers alight");
        log.extend(city.net.drain_events());

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
        // Nothing inside any other vehicle happened.
        assert!(log.iter().all(|event| match event {
            Event::Nested { nested_id: id, .. } => *id == nested_id,
            _ => true,
        }));
    }

    #[test]
    fn board_is_refused_when_the_bus_is_full() {
        let mut city = script::city().expect("the city builds");
        let platform = city.platforms.at(Location::Depot);
        // Six fit aboard a bus; three are waiting already.
        city.net
            .extend(platform, ["Dee", "Eli", "Fay", "Gus"].map(String::from));
        for _ in 0..6 {
            city.order(city.board[Location::Depot as usize])
                .expect("there is room");
        }
        let refused = city
            .order(city.board[Location::Depot as usize])
            .expect_err("the bus is full");
        assert!(matches!(refused, NetError::NotFireable { .. }), "{refused}");
        assert_eq!(names(&city.net, "Platform@Depot"), ["Gus"]);
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
        }
        assert_eq!(tram.cabin.template(), Some("Cabin"));
        assert_eq!(ferry.cabin.template(), Some("Deck"));
        assert_eq!(shuttle.cabin.template(), Some("Pod"));
        assert_eq!(tram.cabin.param::<SeatCount>(), Some(&SeatCount(2)));
        assert_eq!(shuttle.cabin.param::<Capacity>(), Some(&Capacity(4)));
        assert!(ferry.cabin.place_named("Upper Deck").is_some());
        assert!(ferry.cabin.param::<SeatCount>().is_none());
        assert!(shuttle.cabin.place_named("Standing").is_none());
    }
}
