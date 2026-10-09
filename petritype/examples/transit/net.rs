//! The transit net: a city whose vehicles carry nets of their own.
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
//! Two guards differ from the proposal, as §11 records: nobody sits or
//! stands while an announcement stands, and the announcement is cleared only
//! once nobody is aboard at all. Built the proposal's way, a cabin re-seats
//! the passengers at its door the moment the announcement clears, and never
//! holds them there; so where the proposal's §6 has the announcement
//! cleared after step 4, here it stands until the vehicle is empty.

use std::error::Error;
use std::fmt;

use petritype::drivers::{self, DriverError, Limits, Mode, OnFailure, RunSummary};
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
    /// A vehicle standing still with nobody aboard, its cabin built from
    /// its kind's template with the template's defaults.
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

/// Inner parameter `stopped`: the vehicle stands at a station. Passengers
/// head for the door when it is true and an arrival has been announced.
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

/// A place of passengers (or announcements), saved with the vehicle.
fn place(b: &mut NetBuilder, name: &str) -> PlaceRef<String> {
    let place = b.place::<String>(name);
    b.serializable(place);
    place
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

fn stopped(net: &Net) -> bool {
    net.param::<Stopped>().is_some_and(|stopped| stopped.0)
}

fn seat_count(net: &Net) -> usize {
    net.param::<SeatCount>().map_or(0, |seats| seats.0)
}

/// `Cabin`, for a bus or a tram: `Door`, `Seats`, `Standing`,
/// `Announcements`; parameters `seats` (2) and `capacity` (6).
///
/// "Go To Door" is two transitions, one from `Seats` and one from
/// `Standing`: a transition needs a token in every input place, so one
/// transition cannot take from whichever of two places has someone in it.
pub fn cabin(params: &Params) -> Result<Net, BoxedError> {
    let seats = number(params, "seats", 2)?;
    let capacity = number(params, "capacity", 6)?;
    let mut b = NetBuilder::new();
    let door = place(&mut b, "Door");
    let seated = place(&mut b, "Seats");
    let standing = place(&mut b, "Standing");
    let announcements = place(&mut b, "Announcements");
    parameters(&mut b, Some(seats), capacity);

    let quiet = move |net: &Net| net.count(announcements.id()) == 0;
    b.transition("Sit", (TakeEdge(door),), (seated,), |who: String| (who,))
        .guard(move |net| net.count(seated.id()) < seat_count(net) && quiet(net));
    b.transition(
        "Stand",
        (TakeEdge(door),),
        (standing,),
        |who: String| (who,),
    )
    .guard(move |net| net.count(seated.id()) >= seat_count(net) && quiet(net));
    let leave = move |net: &Net| stopped(net) && net.count(announcements.id()) > 0;
    b.transition("Go To Door", (TakeEdge(seated),), (door,), |who: String| {
        (who,)
    })
    .guard(leave);
    b.transition(
        "Go To Door From Standing",
        (TakeEdge(standing),),
        (door,),
        |who: String| (who,),
    )
    .guard(leave);
    let empty = [door, seated, standing];
    b.transition(
        "Clear Announcement",
        (TakeEdge(announcements),),
        (),
        |_announcement: String| {},
    )
    .guard(move |net| empty.iter().all(|place| net.count(place.id()) == 0));
    Ok(b.build()?)
}

/// `Deck`, for a ferry: `Door`, `Lower Deck`, `Upper Deck`,
/// `Announcements`; parameter `capacity` (40). Passengers go below, may go
/// up, and come back to the door from either deck once the ferry has
/// stopped and announced.
pub fn deck(params: &Params) -> Result<Net, BoxedError> {
    let capacity = number(params, "capacity", 40)?;
    let mut b = NetBuilder::new();
    let door = place(&mut b, "Door");
    let lower = place(&mut b, "Lower Deck");
    let upper = place(&mut b, "Upper Deck");
    let announcements = place(&mut b, "Announcements");
    parameters(&mut b, None, capacity);

    b.transition(
        "Go Below",
        (TakeEdge(door),),
        (lower,),
        |who: String| (who,),
    )
    .guard(move |net| net.count(announcements.id()) == 0);
    b.transition("Go Up", (TakeEdge(lower),), (upper,), |who: String| (who,));
    let leave = move |net: &Net| stopped(net) && net.count(announcements.id()) > 0;
    b.transition("Go To Door", (TakeEdge(lower),), (door,), |who: String| {
        (who,)
    })
    .guard(leave);
    b.transition(
        "Go To Door From Upper Deck",
        (TakeEdge(upper),),
        (door,),
        |who: String| (who,),
    )
    .guard(leave);
    let empty = [door, lower, upper];
    b.transition(
        "Clear Announcement",
        (TakeEdge(announcements),),
        (),
        |_announcement: String| {},
    )
    .guard(move |net| empty.iter().all(|place| net.count(place.id()) == 0));
    Ok(b.build()?)
}

/// `Pod`, for a shuttle: `Door`, `Seats` and `Announcements`, nobody
/// standing; parameters `seats` (4) and `capacity` (as many as the seats).
pub fn pod(params: &Params) -> Result<Net, BoxedError> {
    let seats = number(params, "seats", 4)?;
    let capacity = number(params, "capacity", seats)?;
    let mut b = NetBuilder::new();
    let door = place(&mut b, "Door");
    let seated = place(&mut b, "Seats");
    let announcements = place(&mut b, "Announcements");
    parameters(&mut b, Some(seats), capacity);

    b.transition("Sit", (TakeEdge(door),), (seated,), |who: String| (who,))
        .guard(move |net| {
            net.count(seated.id()) < seat_count(net) && net.count(announcements.id()) == 0
        });
    b.transition("Go To Door", (TakeEdge(seated),), (door,), |who: String| {
        (who,)
    })
    .guard(move |net| stopped(net) && net.count(announcements.id()) > 0);
    let empty = [door, seated];
    b.transition(
        "Clear Announcement",
        (TakeEdge(announcements),),
        (),
        |_announcement: String| {},
    )
    .guard(move |net| empty.iter().all(|place| net.count(place.id()) == 0));
    Ok(b.build()?)
}

/// How many passengers are aboard: everyone in every place but
/// `Announcements`, whatever the template.
pub fn aboard(cabin: &Net) -> usize {
    cabin
        .places()
        .filter(|(_, info)| info.name != "Announcements")
        .map(|(_, info)| info.count)
        .sum()
}

// ----- the city -------------------------------------------------------------

/// Net parameter `destination`: where the next `Depart` sends its vehicle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Destination(pub Location);

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

/// The city as PROTOCOL §11 seeds it: one bus at `Depot`, one ferry at
/// `Harbour`, and Ada, Bram and Cato waiting on `Platform@Depot`, every
/// transition switched on.
pub fn build_city() -> Result<City, BoxedError> {
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
    for place in stations.places.into_iter().chain([in_transit]) {
        b.serializable(place);
    }
    for place in platforms.places {
        b.serializable(place);
    }
    b.param(Destination(Location::Central));
    b.serializable_param::<Destination>("destination");

    b.seed(
        stations.at(Location::Depot),
        [Vehicle::new("Bus 1", Kind::Bus, &templates)?],
    );
    b.seed(
        stations.at(Location::Harbour),
        [Vehicle::new("Ferry 1", Kind::Ferry, &templates)?],
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
                vehicle.cabin.put(door, passenger);
                Ok(())
            },
        )
        // Room aboard: everyone in the cabin, door included, below capacity.
        .guard(move |net| {
            front_cabin(net, station).is_some_and(|cabin| {
                aboard(cabin) < cabin.param::<Capacity>().map_or(0, |capacity| capacity.0)
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
                let Some((token, _)) = vehicle.cabin.tokens(door.id()).next() else {
                    return Ok((None,));
                };
                let passenger = vehicle.cabin.token::<String>(door.id(), token).cloned();
                vehicle.cabin.remove(door.id(), token)?;
                Ok((passenger,))
            },
        )
        // Someone at the door to step off.
        .guard(move |net| {
            front_cabin(net, station).is_some_and(|cabin| {
                cabin
                    .place_named("Door")
                    .is_some_and(|door| cabin.count(door) > 0)
            })
        })
        .id()
    });
    let depart = Location::ALL.map(|location| {
        b.transition(
            format!("Depart {location}"),
            (TakeEdge(stations.at(location)), Param::<Destination>::new()),
            (in_transit,),
            |mut vehicle: Vehicle, to: Destination| -> Result<(Vehicle,), TransitError> {
                vehicle.cabin.set_param(Stopped(false))?;
                vehicle.destination = Some(to.0);
                Ok((vehicle,))
            },
        )
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
                vehicle.cabin.set_param(Stopped(true))?;
                let announcements = vehicle.place("Announcements")?;
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
        board,
        alight,
        depart,
        arrive,
    })
}

impl City {
    /// Switches every transition of the city off, so that it fires only on
    /// [`order`](City::order), and drops what that logged: it is part of
    /// setting the city up for the proposal's script, which fires the
    /// city's transitions by hand and runs only the vehicles to quiet.
    pub fn hold_back(&mut self) {
        let transitions: Vec<TransitionId> = self.net.transitions().map(|(id, _)| id).collect();
        for transition in transitions {
            self.net
                .disable(transition)
                .expect("a transition of the city");
        }
        self.net.drain_events().for_each(drop);
    }

    /// Carries out one order: switches the transition on, fires it, and
    /// switches it off again.
    pub fn order(&mut self, transition: TransitionId) -> Result<(), NetError> {
        self.net.enable(transition)?;
        let fired = self.net.fire(transition);
        self.net.disable(transition)?;
        fired
    }

    /// Runs to quiet: with the city held back, this is the vehicles' inner
    /// nets, in place order and token order.
    pub fn run_to_quiet(&mut self) -> Result<RunSummary, DriverError> {
        drivers::run(
            &mut self.net,
            Mode::Sequential,
            Limits::default(),
            OnFailure::Stop,
        )
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
}
