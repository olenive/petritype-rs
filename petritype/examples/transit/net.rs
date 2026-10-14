//! The transit net: a city whose vehicles carry nets of their own, and
//! which runs its journey on its own.
//!
//! This is the conformance net `transit` of the shared protocol
//! (`../petritype/PROTOCOL.md` §11), which the nets-as-tokens proposal's
//! example (`Cyberpunk2020-Simulation/dev-docs/NETS_AS_TOKENS.md` §2)
//! became: places, transitions, guards, parameters, seed and JSON exactly as
//! §11 fixes them, so that a transcript recorded against the Python runner
//! replays against this one. It depends on `petritype`, `serde` and
//! `serde_json` only, so that a runner's example binary can include this
//! file as it is.
//!
//! **A vehicle's state is where its doors are.** Every template has two
//! control places, `Doors Open` and `Doors Closed`, holding one token per
//! door, the door's name: `Front` on a bus, a tram or a shuttle, `Bow` and
//! `Stern` on a ferry, every door open as built. With the announcement they
//! make the vehicle's phase: a door open and no announcement standing is
//! boarding, a door open and an announcement standing is alighting, every
//! door closed is under way. The city's guards count them, and transitions
//! on both sides move the doors from one place to the other: `Board` wants
//! a door open and no announcement; the cabin closes its doors one at a
//! time once everyone aboard has settled (`Close Doors`); `Depart` takes
//! only a vehicle with no door open; `Arrive` opens every door and puts the
//! announcement up in its own body; `Alight` wants the announcement; and
//! the cabin takes it down once everyone has gone (`Clear Announcement`),
//! which leaves it boarding. So the journey runs under plain
//! [`Net::step_deep`], with nothing switched off and no transition picked
//! by name: the city fires whatever it can, and a cabin gets its turn when
//! the city can fire nothing, which is exactly when a vehicle is waiting on
//! what happens inside it. Vehicles go round a fixed loop, `Depot`,
//! `Central`, `Harbour`, `Airport` and back ([`Location::next`]), unless
//! the net parameter `destination` names a station; passengers who step off
//! go to `Arrived`, never back to a platform, or they would board again
//! where they got off. An empty vehicle closes up and moves on, so once
//! everyone has arrived the vehicles go on circulating.
//!
//! The net was built this way on 2026-10-10. Before, the city held back its
//! own transitions under a script that fired them by hand; stepped with
//! nothing held back, it boarded and alighted the same passengers for ever
//! and never gave the cabins a turn (DESIGN §6.6, and §18 for the story).
//! The doors were one token holding a word at first, the same day; as two
//! places they show the phase on a drawing of the cabin, and a vehicle with
//! more doors has more tokens.

use std::error::Error;
use std::fmt;

use petritype::{
    Distribution, MutateEdge, Nested, Net, NetBuilder, NetError, Param, Params, PlaceRef, TakeEdge,
    TemplateError, Templates, TokenId, TransitionId,
};
use serde::{Deserialize, Serialize};

/// An error of any kind that can cross a thread: what a template's factory
/// and [`build_city`] return.
pub type BoxedError = Box<dyn Error + Send + Sync>;

/// The four stations. A destination is written as the station's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Location {
    Depot,
    Central,
    Harbour,
    Airport,
}

impl Location {
    pub const ALL: [Location; 4] = [
        Location::Depot,
        Location::Central,
        Location::Harbour,
        Location::Airport,
    ];

    /// The next station on the loop every vehicle follows: `Depot`,
    /// `Central`, `Harbour`, `Airport`, and back to `Depot`.
    pub fn next(self) -> Location {
        match self {
            Location::Depot => Location::Central,
            Location::Central => Location::Harbour,
            Location::Harbour => Location::Airport,
            Location::Airport => Location::Depot,
        }
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// What a vehicle is, which decides the template its cabin is built from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Bus,
    Tram,
    Ferry,
    Shuttle,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Bus, Kind::Tram, Kind::Ferry, Kind::Shuttle];

    /// The name of the template its cabin is built from.
    pub fn template(self) -> &'static str {
        match self {
            Kind::Bus | Kind::Tram => "Cabin",
            Kind::Ferry => "Deck",
            Kind::Shuttle => "Pod",
        }
    }

    /// Its cabin's doors, by name, in the order its template builds them.
    pub fn doors(self) -> &'static [&'static str] {
        match self {
            Kind::Bus | Kind::Tram | Kind::Shuttle => FRONT_DOOR,
            Kind::Ferry => BOW_AND_STERN,
        }
    }
}

/// A vehicle token: what the city knows about it, and the net inside it.
///
/// Its JSON form is `{"name", "kind", "destination", "cabin"}`, the cabin
/// in the saved form of a [`Net`] (`{"template", "marking", "params",
/// "enabled"}`), which is how a saved city carries what is inside its
/// vehicles.
#[derive(Serialize, Deserialize)]
pub struct Vehicle {
    pub name: String,
    pub kind: Kind,
    /// Where it is going: set by `Depart`, cleared by `Arrive`, `None`
    /// whenever it stands at a station.
    pub destination: Option<Location>,
    /// The one inner net, called `cabin` whatever the template.
    pub cabin: Net,
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

/// One inner net, `cabin`, and a fork that rebuilds it from its template:
/// what lets the city be snapshotted.
impl Nested for Vehicle {
    fn nets(&self) -> Vec<(&str, &Net)> {
        vec![("cabin", &self.cabin)]
    }

    fn nets_mut(&mut self) -> Vec<(&str, &mut Net)> {
        vec![("cabin", &mut self.cabin)]
    }

    fn fork(&self) -> Result<Self, NetError> {
        Ok(Vehicle {
            name: self.name.clone(),
            kind: self.kind,
            destination: self.destination,
            cabin: self.cabin.fork()?,
        })
    }
}

impl Vehicle {
    /// A vehicle standing still with nobody aboard and every door open, so
    /// boarding, its cabin built from its kind's template with the
    /// template's defaults.
    pub fn new(name: &str, kind: Kind, templates: &Templates) -> Result<Vehicle, TemplateError> {
        Ok(Vehicle {
            name: name.to_owned(),
            kind,
            destination: None,
            cabin: templates.build(kind.template(), &Params::new())?,
        })
    }

    /// A place of the cabin by name, typed: a body that reaches inside knows
    /// the templates' place names, not their handles.
    pub fn place(&self, name: &'static str) -> Result<PlaceRef<String>, TransitError> {
        self.cabin
            .place_ref::<String>(name)
            .ok_or_else(|| TransitError::NoPlace {
                vehicle: self.name.clone(),
                place: name,
            })
    }
}

// ----- the inner nets -------------------------------------------------------

/// The doors of a `Cabin` and of a `Pod`: one, at the front.
pub const FRONT_DOOR: &[&str] = &["Front"];
/// The doors of a `Deck`: the bow's, then the stern's.
pub const BOW_AND_STERN: &[&str] = &["Bow", "Stern"];

/// Inner parameter `stopped`: the vehicle stands at a station. `Depart`
/// sets it false and `Arrive` true again, for a viewer to show; no guard
/// reads it, since where the doors are says more.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stopped(pub bool);

/// Inner parameter `seats`: how many seats there are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeatCount(pub usize);

/// Inner parameter `capacity`: how many passengers fit aboard, door
/// included.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capacity(pub usize);

/// The three templates, `Cabin`, `Deck` and `Pod`: what builds a cabin
/// again from a saved vehicle.
pub fn templates() -> Templates {
    let mut templates = Templates::new();
    templates
        .register("Cabin", cabin)
        .register("Deck", deck)
        .register("Pod", pod);
    templates
}

/// A whole number from a template's parameters, or its default when the
/// parameter is not given.
fn number(params: &Params, name: &str, default: usize) -> Result<usize, String> {
    match params.get(name) {
        None => Ok(default),
        Some(value) => value
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| {
                format!("template parameter {name:?} must be a whole number, not {value}")
            }),
    }
}

/// A place of passengers (or of announcements, or of doors), saved with
/// the vehicle.
fn place(b: &mut NetBuilder, name: &str) -> PlaceRef<String> {
    let place = b.place::<String>(name);
    b.serializable(place);
    place
}

/// The places a template's phase is read from: `Announcements`, and the
/// doors, `Doors Open` and `Doors Closed`, one token per door, the door's
/// name. A door open and no announcement standing is boarding; a door open
/// and an announcement standing is alighting; every door closed is under
/// way.
#[derive(Clone, Copy)]
struct Control {
    announcements: PlaceRef<String>,
    open: PlaceRef<String>,
    closed: PlaceRef<String>,
}

impl Control {
    /// The three places, in that order, after the template's passenger
    /// places, built with every one of `doors` open: a vehicle starts at a
    /// station, boarding.
    fn declare(b: &mut NetBuilder, doors: &[&str]) -> Control {
        let announcements = place(b, "Announcements");
        let open = place(b, "Doors Open");
        let closed = place(b, "Doors Closed");
        b.seed(open, doors.iter().map(|door| door.to_string()));
        Control {
            announcements,
            open,
            closed,
        }
    }

    /// A door open and no announcement standing.
    fn boarding(self, net: &Net) -> bool {
        net.count(self.open.id()) > 0 && net.count(self.announcements.id()) == 0
    }

    /// An announcement standing, which it only does while the doors are
    /// open: `Arrive` opens them as it puts one up, and `Close Doors`
    /// refuses while one stands.
    fn alighting(self, net: &Net) -> bool {
        net.count(self.announcements.id()) > 0
    }
}

/// The parameters of a template, each saved under its wire name: `stopped`
/// (starting true: a vehicle starts at a station), `seats` where the
/// template has seats, and `capacity`.
fn parameters(b: &mut NetBuilder, seats: Option<usize>, capacity: usize) {
    b.param(Stopped(true));
    b.serializable_param::<Stopped>("stopped");
    if let Some(seats) = seats {
        b.param(SeatCount(seats));
        b.serializable_param::<SeatCount>("seats");
    }
    b.param(Capacity(capacity));
    b.serializable_param::<Capacity>("capacity");
}

fn seat_count(net: &Net) -> usize {
    net.param::<SeatCount>().map_or(0, |seats| seats.0)
}

/// `Close Doors` and `Clear Announcement`, declared last in every template.
/// `Close Doors` moves one door from `Doors Open` to `Doors Closed`, its
/// name passing through, once nobody is left at the `Door` and no
/// announcement stands, so it fires once per open door; an empty vehicle
/// closes too, and moves on. `Clear Announcement` takes the announcement
/// down once nobody is left in any of `aboard`, which leaves the vehicle
/// boarding, its doors still open.
fn door_transitions(
    b: &mut NetBuilder,
    door: PlaceRef<String>,
    control: Control,
    aboard: Vec<PlaceRef<String>>,
) {
    b.transition(
        "Close Doors",
        (TakeEdge(control.open),),
        (control.closed,),
        |name: String| (name,),
    )
    .guard(move |net| net.count(door.id()) == 0 && net.count(control.announcements.id()) == 0);
    b.transition(
        "Clear Announcement",
        (TakeEdge(control.announcements),),
        (),
        |_announcement: String| {},
    )
    .guard(move |net| aboard.iter().all(|place| net.count(place.id()) == 0));
}

/// `Cabin`, for a bus or a tram: `Door`, `Seats`, `Standing`,
/// `Announcements`, `Doors Open`, `Doors Closed`, one door, `Front`;
/// parameters `seats` (2) and `capacity` (6).
///
/// Passengers who come in at the `Door` sit while there are seats and stand
/// once there are none, while the vehicle is boarding; while an
/// announcement stands, everyone goes back to the `Door`. "Go To Door" is two
/// transitions, one from `Seats` and one from `Standing`: a transition needs
/// a token in every input place, so one transition cannot take from
/// whichever of two places has someone in it.
pub fn cabin(params: &Params) -> Result<Net, BoxedError> {
    let seats = number(params, "seats", 2)?;
    let capacity = number(params, "capacity", 6)?;
    let mut b = NetBuilder::new();
    let door = place(&mut b, "Door");
    let seated = place(&mut b, "Seats");
    let standing = place(&mut b, "Standing");
    let control = Control::declare(&mut b, FRONT_DOOR);
    parameters(&mut b, Some(seats), capacity);

    let boarding = move |net: &Net| control.boarding(net);
    let alighting = move |net: &Net| control.alighting(net);
    b.transition("Sit", (TakeEdge(door),), (seated,), |who: String| (who,))
        .guard(move |net| net.count(seated.id()) < seat_count(net) && boarding(net));
    b.transition(
        "Stand",
        (TakeEdge(door),),
        (standing,),
        |who: String| (who,),
    )
    .guard(move |net| net.count(seated.id()) >= seat_count(net) && boarding(net));
    b.transition("Go To Door", (TakeEdge(seated),), (door,), |who: String| {
        (who,)
    })
    .guard(alighting);
    b.transition(
        "Go To Door From Standing",
        (TakeEdge(standing),),
        (door,),
        |who: String| (who,),
    )
    .guard(alighting);
    door_transitions(&mut b, door, control, vec![door, seated, standing]);
    Ok(b.build()?)
}

/// `Deck`, for a ferry: `Door`, `Lower Deck`, `Upper Deck`,
/// `Announcements`, `Doors Open`, `Doors Closed`, two doors, `Bow` and
/// `Stern`; parameter `capacity` (40). Passengers go below while the ferry
/// is boarding, may go up whenever, and come back to the door from either
/// deck while an announcement stands.
pub fn deck(params: &Params) -> Result<Net, BoxedError> {
    let capacity = number(params, "capacity", 40)?;
    let mut b = NetBuilder::new();
    let door = place(&mut b, "Door");
    let lower = place(&mut b, "Lower Deck");
    let upper = place(&mut b, "Upper Deck");
    let control = Control::declare(&mut b, BOW_AND_STERN);
    parameters(&mut b, None, capacity);

    let alighting = move |net: &Net| control.alighting(net);
    b.transition(
        "Go Below",
        (TakeEdge(door),),
        (lower,),
        |who: String| (who,),
    )
    .guard(move |net| control.boarding(net));
    b.transition("Go Up", (TakeEdge(lower),), (upper,), |who: String| (who,));
    b.transition("Go To Door", (TakeEdge(lower),), (door,), |who: String| {
        (who,)
    })
    .guard(alighting);
    b.transition(
        "Go To Door From Upper Deck",
        (TakeEdge(upper),),
        (door,),
        |who: String| (who,),
    )
    .guard(alighting);
    door_transitions(&mut b, door, control, vec![door, lower, upper]);
    Ok(b.build()?)
}

/// `Pod`, for a shuttle: `Door`, `Seats`, `Announcements`, `Doors Open`
/// and `Doors Closed`, one door, `Front`, and nobody standing; parameters
/// `seats` (4) and `capacity` (as many as the seats).
pub fn pod(params: &Params) -> Result<Net, BoxedError> {
    let seats = number(params, "seats", 4)?;
    let capacity = number(params, "capacity", seats)?;
    let mut b = NetBuilder::new();
    let door = place(&mut b, "Door");
    let seated = place(&mut b, "Seats");
    let control = Control::declare(&mut b, FRONT_DOOR);
    parameters(&mut b, Some(seats), capacity);

    b.transition("Sit", (TakeEdge(door),), (seated,), |who: String| (who,))
        .guard(move |net| net.count(seated.id()) < seat_count(net) && control.boarding(net));
    b.transition("Go To Door", (TakeEdge(seated),), (door,), |who: String| {
        (who,)
    })
    .guard(move |net| control.alighting(net));
    door_transitions(&mut b, door, control, vec![door, seated]);
    Ok(b.build()?)
}

/// The places of every template that hold no passengers: the
/// announcements and the doors.
pub const CONTROL_PLACES: [&str; 3] = ["Announcements", "Doors Open", "Doors Closed"];

/// How many passengers are aboard: every token in every place but
/// [`CONTROL_PLACES`], whatever the template.
pub fn aboard(cabin: &Net) -> usize {
    cabin
        .places()
        .filter(|(_, info)| !CONTROL_PLACES.contains(&info.name))
        .map(|(_, info)| info.count)
        .sum()
}

/// Whether a place of a cabin, named as the city's guards name it, holds a
/// token; `None` for a template without it.
pub fn occupied(cabin: &Net, place: &str) -> Option<bool> {
    cabin.place_named(place).map(|place| cabin.count(place) > 0)
}

// ----- the city -------------------------------------------------------------

/// Net parameter `destination`: an override of the loop. `None` (`null` on
/// the wire), as built, sends every vehicle on to the next station
/// ([`Location::next`]); a station sends every departure there until it is
/// set back to `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Destination(pub Option<Location>);

/// Who waits where when the city is built, oldest first: seven passengers
/// on three platforms.
pub const WAITING: [(Location, &[&str]); 3] = [
    (Location::Depot, &["Ada", "Bram", "Cato"]),
    (Location::Central, &["Dara", "Eve"]),
    (Location::Harbour, &["Finn", "Gwen"]),
];

/// What a body that reaches inside a vehicle can find wrong.
#[derive(Debug)]
pub enum TransitError {
    /// The vehicle's cabin has no such place: a template without it.
    NoPlace {
        vehicle: String,
        place: &'static str,
    },
    /// A vehicle in transit with nowhere to go.
    NoDestination { vehicle: String },
    /// The cabin refused a call.
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

impl Error for TransitError {}

impl From<NetError> for TransitError {
    fn from(error: NetError) -> Self {
        TransitError::Net(error)
    }
}

/// One place per station, of one kind of token.
#[derive(Debug)]
pub struct ByLocation<T> {
    pub places: [PlaceRef<T>; 4],
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
    pub fn at(&self, location: Location) -> PlaceRef<T> {
        self.places[location as usize]
    }
}

/// The city: its net, its places and its transitions.
pub struct City {
    pub net: Net,
    pub stations: ByLocation<Vehicle>,
    pub platforms: ByLocation<String>,
    pub in_transit: PlaceRef<Vehicle>,
    pub arrived: PlaceRef<String>,
    pub board: [TransitionId; 4],
    pub alight: [TransitionId; 4],
    pub depart: [TransitionId; 4],
    pub arrive: TransitionId,
}

/// The front vehicle at a station or in transit: the one a `MutateEdge` on
/// that place is handed, its cabin read through [`Net::nested`] as a guard
/// sees it.
pub fn front_cabin(net: &Net, place: PlaceRef<Vehicle>) -> Option<&Net> {
    let (token, _) = net.tokens(place.id()).next()?;
    let mut nets = net.nested(place.id(), token)?;
    (!nets.is_empty()).then(|| nets.remove(0).1)
}

/// The city as PROTOCOL §11 seeds it: one bus at `Depot` and one ferry at
/// `Harbour`, each with nobody aboard and every door open, the
/// passengers of [`WAITING`] on their platforms, `destination` unset and
/// every transition switched on.
pub fn build_city() -> Result<City, BoxedError> {
    build_city_with(None)
}

/// [`build_city`] with the net parameter `destination` set as built: the
/// runner's `transit` entry takes it as a load parameter, as the Python
/// factory takes it as a keyword (PROTOCOL.md §11).
pub fn build_city_with(destination: Option<Location>) -> Result<City, BoxedError> {
    let templates = templates();
    let mut b = NetBuilder::new();
    b.templates(&templates);
    let stations = ByLocation {
        places: Location::ALL.map(|location| b.place_nested::<Vehicle>(location.to_string())),
    };
    let platforms = ByLocation {
        places: Location::ALL.map(|location| b.place::<String>(format!("Platform@{location}"))),
    };
    let in_transit = b.place_nested::<Vehicle>("In Transit");
    let arrived = b.place::<String>("Arrived");
    for place in stations.places.into_iter().chain([in_transit]) {
        b.serializable(place);
    }
    for place in platforms.places.into_iter().chain([arrived]) {
        b.serializable(place);
    }
    b.param(Destination(destination));
    b.serializable_param::<Destination>("destination");

    b.seed(
        stations.at(Location::Depot),
        [Vehicle::new("Bus 1", Kind::Bus, &templates)?],
    );
    b.seed(
        stations.at(Location::Harbour),
        [Vehicle::new("Ferry 1", Kind::Ferry, &templates)?],
    );
    for (location, passengers) in WAITING {
        b.seed(
            platforms.at(location),
            passengers.iter().map(|name| name.to_string()),
        );
    }

    let board = Location::ALL.map(|location| {
        let station = stations.at(location);
        b.transition(
            format!("Board {location}"),
            (TakeEdge(platforms.at(location)), MutateEdge(station)),
            (),
            |passenger: String, vehicle: &mut Vehicle| -> Result<(), TransitError> {
                let door = vehicle.place("Door")?;
                vehicle.cabin.put(door, passenger);
                Ok(())
            },
        )
        // Room aboard (everyone in the cabin, door included, below
        // capacity), and boarding: a door open, no announcement standing.
        .guard(move |net| {
            front_cabin(net, station).is_some_and(|cabin| {
                aboard(cabin) < cabin.param::<Capacity>().map_or(0, |capacity| capacity.0)
                    && occupied(cabin, "Doors Open") == Some(true)
                    && occupied(cabin, "Announcements") == Some(false)
            })
        })
        .id()
    });
    let alight = Location::ALL.map(|location| {
        let station = stations.at(location);
        b.transition(
            format!("Alight {location}"),
            (MutateEdge(station),),
            (arrived,),
            |vehicle: &mut Vehicle| -> Result<(Option<String>,), TransitError> {
                let door = vehicle.place("Door")?;
                let Some((token, _)) = vehicle.cabin.tokens(door.id()).next() else {
                    return Ok((None,));
                };
                let passenger = vehicle.cabin.token::<String>(door.id(), token).cloned();
                vehicle.cabin.remove(door.id(), token)?;
                Ok((passenger,))
            },
        )
        // Someone at the door to step off, and an announcement standing.
        .guard(move |net| {
            front_cabin(net, station).is_some_and(|cabin| {
                occupied(cabin, "Door") == Some(true)
                    && occupied(cabin, "Announcements") == Some(true)
            })
        })
        .id()
    });
    let depart = Location::ALL.map(|location| {
        let station = stations.at(location);
        b.transition(
            format!("Depart {location}"),
            (TakeEdge(station), Param::<Destination>::new()),
            (in_transit,),
            move |mut vehicle: Vehicle, to: Destination| -> Result<(Vehicle,), TransitError> {
                vehicle.cabin.set_param(Stopped(false))?;
                vehicle.destination = Some(to.0.unwrap_or(location.next()));
                Ok((vehicle,))
            },
        )
        // Every door closed: everyone who boarded has settled.
        .guard(move |net| {
            front_cabin(net, station)
                .is_some_and(|cabin| occupied(cabin, "Doors Open") == Some(false))
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
                let announcements = vehicle.place("Announcements")?;
                let open = vehicle.place("Doors Open")?;
                let closed = vehicle.place("Doors Closed")?;
                vehicle.cabin.set_param(Stopped(true))?;
                // The doors open here, in the city's own firing: opened by a
                // transition inside, they would leave `Depart` fireable
                // until the cabin had its turn, and the city goes first.
                // Every closed door, in order, taken out and put back open.
                let doors: Vec<String> = vehicle
                    .cabin
                    .tokens(closed.id())
                    .filter_map(|(_, door)| door.downcast_ref::<String>().cloned())
                    .collect();
                vehicle.cabin.set_tokens(closed, std::iter::empty());
                vehicle.cabin.extend(open, doors);
                vehicle
                    .cabin
                    .put(announcements, format!("Arrived at {destination}"));
                Ok(Distribution::new().to(stations.at(destination), vehicle))
            },
        )
        .id();

    Ok(City {
        net: b.build()?,
        stations,
        platforms,
        in_transit,
        arrived,
        board,
        alight,
        depart,
        arrive,
    })
}

impl City {
    /// How many passengers the city is built with: everyone in [`WAITING`].
    pub fn passengers() -> usize {
        WAITING.iter().map(|(_, names)| names.len()).sum()
    }

    /// Whether everyone the city was built with is in `Arrived`.
    pub fn all_arrived(&self) -> bool {
        self.net.count(self.arrived.id()) >= City::passengers()
    }

    /// The cabin of the front vehicle at a station or in transit.
    pub fn inside(&self, place: PlaceRef<Vehicle>) -> Option<&Net> {
        front_cabin(&self.net, place)
    }

    /// The front vehicle at a station or in transit, typed.
    pub fn vehicle(&self, place: PlaceRef<Vehicle>) -> Option<(TokenId, &Vehicle)> {
        let (token, _) = self.net.tokens(place.id()).next()?;
        Some((token, self.net.token::<Vehicle>(place.id(), token)?))
    }

    /// Every vehicle in the city, station by station and then in transit,
    /// oldest first, with the place it is in.
    pub fn vehicles(&self) -> impl Iterator<Item = (PlaceRef<Vehicle>, TokenId, &Vehicle)> {
        self.stations
            .places
            .into_iter()
            .chain([self.in_transit])
            .flat_map(move |place| {
                self.net
                    .tokens(place.id())
                    .filter_map(move |(token, value)| {
                        Some((place, token, value.downcast_ref::<Vehicle>()?))
                    })
            })
    }
}
