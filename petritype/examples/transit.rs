//! Transit: vehicles that carry Petri nets of their own (DESIGN §6.6).
//!
//! The example of the nets-as-tokens proposal
//! (`Cyberpunk2020-Simulation/dev-docs/NETS_AS_TOKENS.md` §2). The outer net,
//! `City`, has four stations — `Depot`, `Central`, `Harbour`, `Airport` —
//! each holding the vehicles standing there, a `Platform@<station>` of
//! passengers waiting at each, and `In Transit` for the vehicles between
//! them. A vehicle is a token, and it carries an *inner* net whose shape
//! depends on its kind: a `Cabin` for a bus or a tram, a `Deck` for a ferry,
//! a `Pod` for a shuttle. The city has one `Depot` place whatever is inside
//! the vehicles in it.
//!
//! Inside a vehicle, passengers come in at the `Door`, sit or stand while it
//! is under way, and go back to the door once it has stopped and announced
//! the station. Nothing inside a vehicle touches the city. The city reaches
//! in only through the vehicle token it holds: `Board <station>` and
//! `Alight <station>` put a passenger into the inner `Door` and take one out
//! with a [`MutateEdge`] body; `Depart <station>` and `Arrive` move the
//! vehicle and set the inner parameter `Stopped`, and `Arrive` puts an
//! announcement inside. A guard on `Board` reads the inner marking through
//! [`Net::nested`] to see whether there is room.
//!
//! **The city fires only on orders.** Its transitions start switched off,
//! and an order switches one on for a single [`Net::fire`] and off again
//! ([`City::order`]); [`drivers::run`] then has nothing to fire in the city
//! itself and runs the inner nets of the vehicles, which is what "run to
//! quiet" means in the script below. That keeps the city's transitions as
//! the proposal writes them — no extra input for orders — and keeps the
//! city honestly quiescent between orders, so
//! [`Net::is_quiescent_deep`] answers what the script asks of it. (A
//! selector that declines everything would also stop the driver firing the
//! city's own transitions, but `Depart` and `Arrive` would still be fireable,
//! and the city would never be quiescent.)
//!
//! The acceptance script, steps 1 to 5 and 7 of the proposal, is `main` and
//! the tests below. Step 6, saving and loading the city, is phase 2.

use std::fmt;

use petritype::drivers::{self, DriverError, Limits, Mode, OnFailure, RunSummary};
use petritype::{
    BuildError, Distribution, Event, MutateEdge, Nested, Net, NetBuilder, NetError, Param, PlaceId,
    PlaceRef, TakeEdge, TokenId, TransitionId,
};

/// The four stations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Location {
    Depot,
    Central,
    Harbour,
    Airport,
}

impl Location {
    const ALL: [Location; 4] = [
        Location::Depot,
        Location::Central,
        Location::Harbour,
        Location::Airport,
    ];
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// What a vehicle is, which decides the shape of the net inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Bus,
    Tram,
    Ferry,
    Shuttle,
}

impl Kind {
    const ALL: [Kind; 4] = [Kind::Bus, Kind::Tram, Kind::Ferry, Kind::Shuttle];

    /// The template the inner net is built from, which is also its name in
    /// the city's log.
    fn template(self) -> &'static str {
        match self {
            Kind::Bus | Kind::Tram => "cabin",
            Kind::Ferry => "deck",
            Kind::Shuttle => "pod",
        }
    }
}

/// A vehicle token: what the city knows about it, and the net inside it.
struct Vehicle {
    name: String,
    kind: Kind,
    destination: Option<Location>,
    inner: Net,
}

/// The inner net is left out of the label: a diagram of the city shows the
/// vehicle, and [`Net::nested`] is how to look inside.
impl fmt::Debug for Vehicle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Vehicle")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("destination", &self.destination)
            .finish_non_exhaustive()
    }
}

/// One inner net, named after its template.
impl Nested for Vehicle {
    fn nets(&self) -> Vec<(&str, &Net)> {
        vec![(self.kind.template(), &self.inner)]
    }

    fn nets_mut(&mut self) -> Vec<(&str, &mut Net)> {
        vec![(self.kind.template(), &mut self.inner)]
    }
}

impl Vehicle {
    /// A vehicle standing still, with nobody aboard.
    fn new(name: &str, kind: Kind) -> Result<Vehicle, BuildError> {
        let inner = match kind {
            Kind::Bus => cabin(2, 6)?,
            Kind::Tram => cabin(8, 30)?,
            Kind::Ferry => deck(40)?,
            Kind::Shuttle => pod(4)?,
        };
        Ok(Vehicle {
            name: name.to_owned(),
            kind,
            destination: None,
            inner,
        })
    }

    /// A place of the inner net by name, typed: the body that reaches inside
    /// knows the templates' place names, not their handles.
    fn place(&self, name: &'static str) -> Result<PlaceRef<String>, TransitError> {
        self.inner
            .place_ref::<String>(name)
            .ok_or_else(|| TransitError::NoPlace {
                vehicle: self.name.clone(),
                place: name,
            })
    }
}

// ----- the inner nets -------------------------------------------------------

/// Inner parameter: the vehicle has stopped at a station and announced it.
/// Passengers take their places while it is false and head for the door
/// while it is true.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stopped(bool);

/// Inner parameter: how many seats there are.
#[derive(Debug, Clone, Copy)]
struct SeatCount(usize);

/// Inner parameter: how many passengers fit aboard, door included.
#[derive(Debug, Clone, Copy)]
struct Capacity(usize);

fn stopped(net: &Net) -> bool {
    net.param::<Stopped>().is_some_and(|stopped| stopped.0)
}

fn seat_count(net: &Net) -> usize {
    net.param::<SeatCount>().map_or(0, |seats| seats.0)
}

/// `Cabin`, for a bus or a tram: `Door`, `Seats`, `Standing`,
/// `Announcements`.
///
/// "Go To Door" is two transitions, one from `Seats` and one from
/// `Standing`: a transition needs a token in every input place, so one
/// transition cannot take from whichever of two places has someone in it.
fn cabin(seats: usize, capacity: usize) -> Result<Net, BuildError> {
    let mut b = NetBuilder::new();
    let door = b.place::<String>("Door");
    let seated = b.place::<String>("Seats");
    let standing = b.place::<String>("Standing");
    let announcements = b.place::<String>("Announcements");
    b.param(Stopped(false));
    b.param(SeatCount(seats));
    b.param(Capacity(capacity));

    b.transition("Sit", (TakeEdge(door),), (seated,), |who: String| (who,))
        .guard(move |net| !stopped(net) && net.count(seated.id()) < seat_count(net));
    b.transition(
        "Stand",
        (TakeEdge(door),),
        (standing,),
        |who: String| (who,),
    )
    .guard(move |net| !stopped(net) && net.count(seated.id()) >= seat_count(net));
    let leave = move |net: &Net| stopped(net) && net.count(announcements.id()) > 0;
    b.transition(
        "Go To Door From Seats",
        (TakeEdge(seated),),
        (door,),
        |who: String| (who,),
    )
    .guard(leave);
    b.transition(
        "Go To Door From Standing",
        (TakeEdge(standing),),
        (door,),
        |who: String| (who,),
    )
    .guard(leave);
    b.transition(
        "Clear Announcement",
        (TakeEdge(announcements),),
        (),
        |_announcement: String| {},
    )
    .guard(move |net| net.count(seated.id()) == 0 && net.count(standing.id()) == 0);
    b.build()
}

/// `Deck`, for a ferry: `Door`, `Lower Deck`, `Upper Deck`,
/// `Announcements`. Passengers go below, then up, and back to the door from
/// either deck once the ferry has stopped.
fn deck(capacity: usize) -> Result<Net, BuildError> {
    let mut b = NetBuilder::new();
    let door = b.place::<String>("Door");
    let lower = b.place::<String>("Lower Deck");
    let upper = b.place::<String>("Upper Deck");
    let announcements = b.place::<String>("Announcements");
    b.param(Stopped(false));
    b.param(Capacity(capacity));

    b.transition(
        "Go Below",
        (TakeEdge(door),),
        (lower,),
        |who: String| (who,),
    )
    .guard(|net| !stopped(net));
    b.transition("Go Up", (TakeEdge(lower),), (upper,), |who: String| (who,))
        .guard(|net| !stopped(net));
    let leave = move |net: &Net| stopped(net) && net.count(announcements.id()) > 0;
    b.transition(
        "Go To Door From Lower Deck",
        (TakeEdge(lower),),
        (door,),
        |who: String| (who,),
    )
    .guard(leave);
    b.transition(
        "Go To Door From Upper Deck",
        (TakeEdge(upper),),
        (door,),
        |who: String| (who,),
    )
    .guard(leave);
    b.transition(
        "Clear Announcement",
        (TakeEdge(announcements),),
        (),
        |_announcement: String| {},
    )
    .guard(move |net| net.count(lower.id()) == 0 && net.count(upper.id()) == 0);
    b.build()
}

/// `Pod`, for a shuttle: `Door` and `Seats`, nobody standing and no
/// announcements — everyone heads for the door once it stops.
fn pod(seats: usize) -> Result<Net, BuildError> {
    let mut b = NetBuilder::new();
    let door = b.place::<String>("Door");
    let seated = b.place::<String>("Seats");
    b.param(Stopped(false));
    b.param(SeatCount(seats));
    b.param(Capacity(seats));

    b.transition("Sit", (TakeEdge(door),), (seated,), |who: String| (who,))
        .guard(move |net| !stopped(net) && net.count(seated.id()) < seat_count(net));
    b.transition("Go To Door", (TakeEdge(seated),), (door,), |who: String| {
        (who,)
    })
    .guard(stopped);
    b.build()
}

/// How many passengers are aboard: everyone in every place but
/// `Announcements`, whatever the template.
fn aboard(inner: &Net) -> usize {
    inner
        .places()
        .filter(|(_, info)| info.name != "Announcements")
        .map(|(_, info)| info.count)
        .sum()
}

// ----- the city -------------------------------------------------------------

/// Outer parameter: where the next `Depart` sends its vehicle.
#[derive(Debug, Clone, Copy)]
struct NextStop(Location);

/// What a body that reaches inside a vehicle can find wrong.
#[derive(Debug)]
enum TransitError {
    /// The vehicle's inner net has no such place: a template without it.
    NoPlace {
        vehicle: String,
        place: &'static str,
    },
    /// A vehicle in transit with nowhere to go.
    NoDestination { vehicle: String },
    /// The inner net refused a call.
    Net(NetError),
}

impl fmt::Display for TransitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransitError::NoPlace { vehicle, place } => {
                write!(f, "{vehicle} has no place {place:?} inside")
            }
            TransitError::NoDestination { vehicle } => {
                write!(f, "{vehicle} is in transit with no destination")
            }
            TransitError::Net(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for TransitError {}

impl From<NetError> for TransitError {
    fn from(error: NetError) -> Self {
        TransitError::Net(error)
    }
}

/// One place per station, of one kind of token.
#[derive(Debug)]
struct ByLocation<T> {
    places: [PlaceRef<T>; 4],
}

/// Written out: a derive would ask `T: Copy`, and a handle is `Copy` whatever
/// its token is.
impl<T> Clone for ByLocation<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for ByLocation<T> {}

impl<T> ByLocation<T> {
    fn at(&self, location: Location) -> PlaceRef<T> {
        self.places[location as usize]
    }
}

/// The city: its net, its places and its transitions.
struct City {
    net: Net,
    stations: ByLocation<Vehicle>,
    platforms: ByLocation<String>,
    in_transit: PlaceRef<Vehicle>,
    board: [TransitionId; 4],
    alight: [TransitionId; 4],
    depart: [TransitionId; 4],
    arrive: TransitionId,
}

/// The front vehicle standing at a station: the one a `MutateEdge` on that
/// place is handed, read through [`Net::nested`] as a guard sees it.
fn front_vehicle(net: &Net, station: PlaceRef<Vehicle>) -> Option<&Net> {
    let (token, _) = net.tokens(station.id()).next()?;
    let mut nets = net.nested(station.id(), token)?;
    (!nets.is_empty()).then(|| nets.remove(0).1)
}

/// Step 1 of the script: the city with one bus at `Depot`, one ferry at
/// `Harbour`, and Ada, Bram and Cato waiting on `Platform@Depot`.
fn build_city() -> Result<City, BuildError> {
    let mut b = NetBuilder::new();
    let stations = ByLocation {
        places: Location::ALL.map(|location| b.place_nested::<Vehicle>(location.to_string())),
    };
    let platforms = ByLocation {
        places: Location::ALL.map(|location| b.place::<String>(format!("Platform@{location}"))),
    };
    let in_transit = b.place_nested::<Vehicle>("In Transit");
    b.param(NextStop(Location::Central));

    b.seed(
        stations.at(Location::Depot),
        [Vehicle::new("Bus 1", Kind::Bus)?],
    );
    b.seed(
        stations.at(Location::Harbour),
        [Vehicle::new("Ferry 1", Kind::Ferry)?],
    );
    b.seed(
        platforms.at(Location::Depot),
        ["Ada", "Bram", "Cato"].map(String::from),
    );

    let board = Location::ALL.map(|location| {
        let station = stations.at(location);
        b.transition(
            format!("Board {location}"),
            (TakeEdge(platforms.at(location)), MutateEdge(station)),
            (),
            |passenger: String, vehicle: &mut Vehicle| -> Result<(), TransitError> {
                let door = vehicle.place("Door")?;
                vehicle.inner.put(door, passenger);
                Ok(())
            },
        )
        // Room aboard: door, seats, standing and the rest, below capacity.
        .guard(move |net| {
            front_vehicle(net, station).is_some_and(|inner| {
                aboard(inner) < inner.param::<Capacity>().map_or(0, |capacity| capacity.0)
            })
        })
        .id()
    });
    let alight = Location::ALL.map(|location| {
        let station = stations.at(location);
        b.transition(
            format!("Alight {location}"),
            (MutateEdge(station),),
            (platforms.at(location),),
            |vehicle: &mut Vehicle| -> Result<(Option<String>,), TransitError> {
                let door = vehicle.place("Door")?;
                let Some((token, _)) = vehicle.inner.tokens(door.id()).next() else {
                    return Ok((None,));
                };
                let passenger = vehicle.inner.token::<String>(door.id(), token).cloned();
                vehicle.inner.remove(door.id(), token)?;
                Ok((passenger,))
            },
        )
        // Someone at the door to step off.
        .guard(move |net| {
            front_vehicle(net, station).is_some_and(|inner| {
                inner
                    .place_named("Door")
                    .is_some_and(|door| inner.count(door) > 0)
            })
        })
        .id()
    });
    let depart = Location::ALL.map(|location| {
        b.transition(
            format!("Depart {location}"),
            (TakeEdge(stations.at(location)), Param::<NextStop>::new()),
            (in_transit,),
            |mut vehicle: Vehicle, next: NextStop| -> Result<(Vehicle,), TransitError> {
                vehicle.inner.set_param(Stopped(false))?;
                vehicle.destination = Some(next.0);
                Ok((vehicle,))
            },
        )
        .guard(move |net| {
            net.param::<NextStop>()
                .is_some_and(|next| next.0 != location)
        })
        .id()
    });
    let arrive = b
        .transition(
            "Arrive",
            (TakeEdge(in_transit),),
            (
                stations.at(Location::Depot),
                stations.at(Location::Central),
                stations.at(Location::Harbour),
                stations.at(Location::Airport),
            ),
            move |mut vehicle: Vehicle| -> Result<Distribution, TransitError> {
                let destination =
                    vehicle
                        .destination
                        .take()
                        .ok_or_else(|| TransitError::NoDestination {
                            vehicle: vehicle.name.clone(),
                        })?;
                vehicle.inner.set_param(Stopped(true))?;
                // A pod has nowhere to announce anything.
                if let Some(announcements) = vehicle.inner.place_ref::<String>("Announcements") {
                    vehicle
                        .inner
                        .put(announcements, format!("Arrived at {destination}"));
                }
                Ok(Distribution::new().to(stations.at(destination), vehicle))
            },
        )
        .id();

    let mut net = b.build()?;
    // Everything in the city waits for an order. Setting the switches is
    // part of setting the city up, so its log starts after them.
    let transitions: Vec<TransitionId> = net.transitions().map(|(id, _)| id).collect();
    for transition in transitions {
        net.disable(transition).expect("a transition of the city");
    }
    net.drain_events().for_each(drop);

    Ok(City {
        net,
        stations,
        platforms,
        in_transit,
        board,
        alight,
        depart,
        arrive,
    })
}

impl City {
    /// Carries out one order: switches the transition on, fires it, and
    /// switches it off again.
    fn order(&mut self, transition: TransitionId) -> Result<(), NetError> {
        self.net.enable(transition)?;
        let fired = self.net.fire(transition);
        self.net.disable(transition)?;
        fired
    }

    /// Runs to quiet: nothing in the city is switched on, so this is the
    /// vehicles' inner nets, in place order and token order.
    fn run_to_quiet(&mut self) -> Result<RunSummary, DriverError> {
        drivers::run(
            &mut self.net,
            Mode::Sequential,
            Limits::default(),
            OnFailure::Stop,
        )
    }

    /// The inner net of the front vehicle at a station or in transit.
    fn inside(&self, place: PlaceRef<Vehicle>) -> Option<&Net> {
        front_vehicle(&self.net, place)
    }

    /// The front vehicle at a station or in transit, typed.
    fn vehicle(&self, place: PlaceRef<Vehicle>) -> Option<(TokenId, &Vehicle)> {
        let (token, _) = self.net.tokens(place.id()).next()?;
        Some((token, self.net.token::<Vehicle>(place.id(), token)?))
    }
}

/// The passengers in a place of the city, by name, oldest first.
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
    let template = cabin(2, 6).expect("the cabin template builds");
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

/// The script, steps 2 to 5. Each returns the events it logged.
mod script {
    use super::*;

    /// Step 2: `Board Depot` three times, then run to quiet.
    pub(super) fn board(city: &mut City) -> Result<RunSummary, Box<dyn std::error::Error>> {
        for _ in 0..3 {
            city.order(city.board[Location::Depot as usize])?;
        }
        Ok(city.run_to_quiet()?)
    }

    /// Step 3: `Depart Depot` for `Central`.
    pub(super) fn depart(city: &mut City) -> Result<(), Box<dyn std::error::Error>> {
        city.net.set_param(NextStop(Location::Central))?;
        city.order(city.depart[Location::Depot as usize])?;
        Ok(())
    }

    /// Step 4: `Arrive`, then run to quiet.
    pub(super) fn arrive(city: &mut City) -> Result<RunSummary, Box<dyn std::error::Error>> {
        city.order(city.arrive)?;
        Ok(city.run_to_quiet()?)
    }

    /// Step 5: `Alight Central` twice.
    pub(super) fn alight(city: &mut City) -> Result<(), Box<dyn std::error::Error>> {
        for _ in 0..2 {
            city.order(city.alight[Location::Central as usize])?;
        }
        Ok(())
    }

    // Step 6, saving the city after step 3 and step 5 and carrying on from
    // the loaded copies, is phase 2: it needs inner nets rebuilt from
    // templates and markings imported by name.
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut city = build_city()?;
    println!("{}", petritype_dot::dot(&city.net));
    for kind in Kind::ALL {
        let vehicle = Vehicle::new(&format!("{kind:?}"), kind)?;
        let places: Vec<&str> = vehicle.inner.places().map(|(_, info)| info.name).collect();
        println!("a {kind:?} carries a {}: {places:?}", kind.template());
    }
    println!("waiting at Depot: {:?}\n", waiting(&city, Location::Depot));
    let mut log = Vec::new();

    let summary = script::board(&mut city)?;
    log.extend(city.net.drain_events());
    let cabin = city
        .inside(city.stations.at(Location::Depot))
        .expect("the bus");
    println!(
        "after boarding ({} inner firings): seats {:?}, standing {:?}, door {:?}",
        summary.fired_count,
        names(cabin, "Seats"),
        names(cabin, "Standing"),
        names(cabin, "Door"),
    );

    script::depart(&mut city)?;
    log.extend(city.net.drain_events());
    let (_, bus) = city.vehicle(city.in_transit).expect("the bus");
    println!(
        "in transit to {:?}; quiescent all the way down: {}",
        bus.destination,
        city.net.is_quiescent_deep()
    );

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

    println!("\nthe firings, with paths:");
    for firing in firings(&city.net, &log) {
        println!("  {firing}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The city after the first `step` steps of the script (1 to 5), and
    /// everything it logged on the way.
    fn after(step: u32) -> (City, Vec<Event>) {
        let mut city = build_city().expect("the city builds");
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

    /// The bus's token id at a station, and its inner net's id.
    fn bus_at(city: &City, place: PlaceRef<Vehicle>) -> (TokenId, petritype::NetId) {
        let (token, bus) = city.vehicle(place).expect("the bus is there");
        assert_eq!(bus.name, "Bus 1");
        (token, bus.inner.id())
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
        assert_eq!(
            city.vehicle(city.stations.at(Location::Harbour))
                .map(|(_, ferry)| ferry.kind),
            Some(Kind::Ferry),
        );
        assert_eq!(names(&city.net, "Platform@Depot"), ["Ada", "Bram", "Cato"]);
        assert!(
            city.net.fireable().is_empty(),
            "every order is switched off"
        );
        assert!(city.net.is_quiescent_deep());
        let info = city.net.places().next().expect("Depot").1;
        assert!(info.nested && !info.passive);
    }

    #[test]
    fn step_2_three_board_and_the_bus_sorts_them_out_inside() {
        let mut city = build_city().expect("the city builds");
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
        assert!(city.net.is_quiescent_deep(), "Stopped is false");
    }

    #[test]
    fn step_4_the_bus_arrives_and_everyone_goes_to_the_door() {
        let (city, log) = after(4);

        let central = city.stations.at(Location::Central);
        assert_eq!(city.net.count(central.id()), 1);
        assert_eq!(city.net.count(city.in_transit.id()), 0);
        let cabin = city.inside(central).expect("the bus");
        assert_eq!(names(cabin, "Door"), ["Ada", "Bram", "Cato"]);
        assert!(names(cabin, "Announcements").is_empty());
        assert!(city.net.is_quiescent_deep());

        // `Clear Announcement` only once all three had gone to the door.
        let inner: Vec<String> = firings(&city.net, &log)
            .into_iter()
            .filter_map(|firing| firing.split_once(": ").map(|(_, name)| name.to_owned()))
            .filter(|name| name.starts_with("Go To Door") || name == "Clear Announcement")
            .collect();
        assert_eq!(
            inner,
            [
                "Go To Door From Seats",
                "Go To Door From Seats",
                "Go To Door From Standing",
                "Clear Announcement",
            ],
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
                format!("{central}: Go To Door From Seats"),
                format!("{central}: Go To Door From Seats"),
                format!("{central}: Go To Door From Standing"),
                format!("{central}: Clear Announcement"),
                "Alight Central".to_owned(),
                "Alight Central".to_owned(),
            ],
        );
    }

    #[test]
    fn step_7_the_bus_is_followed_by_its_nested_id_across_token_ids() {
        let mut city = build_city().expect("the city builds");
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
        assert_eq!(still, nested_id, "the inner net never changed");
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
        let mut city = build_city().expect("the city builds");
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
    fn every_kind_has_an_inside_of_its_own() {
        let tram = Vehicle::new("Tram 1", Kind::Tram).expect("a cabin");
        let ferry = Vehicle::new("Ferry 2", Kind::Ferry).expect("a deck");
        let shuttle = Vehicle::new("Shuttle 1", Kind::Shuttle).expect("a pod");
        assert_eq!(tram.nets()[0].0, "cabin");
        assert_eq!(ferry.nets()[0].0, "deck");
        assert_eq!(shuttle.nets()[0].0, "pod");
        assert_eq!(
            tram.inner.param::<SeatCount>().map(|seats| seats.0),
            Some(8)
        );
        assert!(ferry.inner.place_named("Upper Deck").is_some());
        assert!(shuttle.inner.place_named("Standing").is_none());
    }
}
