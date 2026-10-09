//! The wildlife net: species across habitats, with keys to ask for them by.
//!
//! This is the conformance net `wildlife` of the shared protocol
//! (`../petritype/PROTOCOL.md` §11), the example of the token-keys proposal
//! (`Cyberpunk2020-Simulation/dev-docs/TOKEN_QUERYING_AND_FILTERING.md` §2):
//! places, keys, transitions, guards, parameters, seed and JSON exactly as
//! §11 fixes them, so that a transcript recorded against the Python runner
//! can replay against this one. It depends on `petritype`, `serde` and
//! `serde_json` only, so that a runner's example binary can include this
//! file as it is.
//!
//! A place is a habitat, and a token is one species occurring in one
//! habitat: the same species in two habitats is two tokens with the same
//! `name`. A token carries its own category, `class`, so "every bird" is a
//! question about keys the tokens carry, never something worked out from
//! names. Every place declares the same three keys: `species` (the name),
//! `class` and `diet`.
//!
//! Two transitions differ from the proposal's §2, as the Python track's "As
//! built" records and §11 fixes. `Decline` halves a population through a
//! mutate edge and never removes the token: a mutate edge removes nothing,
//! and "at zero the token is removed" would need a consuming edge with a
//! conditional put-back, which is the pattern keyed edges exist to avoid; its
//! guard stops it at zero instead. And `Spread` leaves the original with half
//! its population, rounded down, and gives the copy the rest.
//!
//! `Migrate Forest Wetland` is written the way a net has to before keyed
//! edges: it takes *every* species in `Forest`, keeps the oldest bird, and
//! puts the rest back (the game's pattern P2). Each of them leaves and comes
//! back under a new id, which is what a keyed edge will remove.

use std::error::Error;
use std::fmt;

use petritype::{
    BuildError, Distribution, KeyValue, MutateEdge, Net, NetBuilder, PlaceRef, TakeAllEdge,
    TakeEdge, TransitionId,
};
use serde::{Deserialize, Serialize};

/// What a species is, carried by the token itself. Written as its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Class {
    Bird,
    Mammal,
    Reptile,
    Fish,
}

/// What a species eats. Written as its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Diet {
    Herbivore,
    Carnivore,
    Insectivore,
    Omnivore,
}

/// A class as a key value: its name, as its JSON form writes it.
impl From<Class> for KeyValue {
    fn from(class: Class) -> Self {
        KeyValue::Str(format!("{class:?}"))
    }
}

/// A diet as a key value: its name, as its JSON form writes it.
impl From<Diet> for KeyValue {
    fn from(diet: Diet) -> Self {
        KeyValue::Str(format!("{diet:?}"))
    }
}

/// One species occurring in one habitat.
///
/// Its JSON form is `{"name", "class", "diet", "population",
/// "destination"}`, `destination` a habitat's name while the species is
/// migrating and `null` otherwise (it may be left out when a token is
/// injected).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Species {
    pub name: String,
    pub class: Class,
    pub diet: Diet,
    pub population: u32,
    #[serde(default)]
    pub destination: Option<String>,
}

impl Species {
    /// A species settled where it is, going nowhere.
    pub fn new(name: &str, class: Class, diet: Diet, population: u32) -> Species {
        Species {
            name: name.to_owned(),
            class,
            diet,
            population,
            destination: None,
        }
    }
}

/// The keys every place declares, in this order.
pub const KEY_NAMES: [&str; 3] = ["species", "class", "diet"];

/// A species' keys: its name, its class and its diet.
pub fn keys_of(species: &Species) -> [KeyValue; 3] {
    [
        species.name.as_str().into(),
        species.class.into(),
        species.diet.into(),
    ]
}

/// The five habitats, in place order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Habitat {
    Forest,
    Wetland,
    Savanna,
    Reef,
    Tundra,
}

impl Habitat {
    pub const ALL: [Habitat; 5] = [
        Habitat::Forest,
        Habitat::Wetland,
        Habitat::Savanna,
        Habitat::Reef,
        Habitat::Tundra,
    ];

    /// The habitat with this name, as a destination names it.
    pub fn named(name: &str) -> Option<Habitat> {
        Habitat::ALL
            .into_iter()
            .find(|habitat| habitat.to_string() == name)
    }
}

impl fmt::Display for Habitat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// Net parameter `season` (`"Summer"`): birds migrate in `"Autumn"`. A
/// string, as the protocol's parameter is, not an enum of four.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Season(pub String);

/// Net parameter `spread_threshold` (`1000`): a population above it spreads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpreadThreshold(pub u32);

/// What a body can find wrong.
#[derive(Debug)]
pub enum WildlifeError {
    /// `Migrate` fired on a habitat with no bird in it, which its guard
    /// rules out.
    NothingToMigrate,
    /// A species settling with nowhere to go.
    NowhereToGo { species: String },
    /// A species settling in a habitat that is not one.
    UnknownHabitat { species: String, habitat: String },
}

impl fmt::Display for WildlifeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WildlifeError::NothingToMigrate => f.write_str("no bird to migrate"),
            WildlifeError::NowhereToGo { species } => {
                write!(f, "{species} is migrating with no destination")
            }
            WildlifeError::UnknownHabitat { species, habitat } => {
                write!(
                    f,
                    "{species} is migrating to {habitat:?}, which is no habitat"
                )
            }
        }
    }
}

impl Error for WildlifeError {}

/// The net, its places and its transitions.
pub struct Wildlife {
    pub net: Net,
    pub habitats: [PlaceRef<Species>; 5],
    pub migrating: PlaceRef<Species>,
    pub migrate: TransitionId,
    pub settle: TransitionId,
    pub spread: TransitionId,
    pub decline: TransitionId,
}

impl Wildlife {
    /// The place of one habitat.
    pub fn at(&self, habitat: Habitat) -> PlaceRef<Species> {
        self.habitats[habitat as usize]
    }
}

/// The front species of a place: the one a `MutateEdge` on it is handed.
pub fn front(net: &Net, place: PlaceRef<Species>) -> Option<&Species> {
    let (token, _) = net.tokens(place.id()).next()?;
    net.token::<Species>(place.id(), token)
}

fn season(net: &Net) -> &str {
    net.param::<Season>().map_or("", |season| season.0.as_str())
}

fn spread_threshold(net: &Net) -> u32 {
    net.param::<SpreadThreshold>()
        .map_or(u32::MAX, |threshold| threshold.0)
}

/// The net as PROTOCOL §11 seeds it: nine species in five habitats, nothing
/// migrating, `season` `"Summer"`, `spread_threshold` 1000, every
/// transition switched on.
pub fn build_wildlife() -> Result<Wildlife, BuildError> {
    let mut b = NetBuilder::new();
    let habitats = Habitat::ALL.map(|habitat| b.place::<Species>(habitat.to_string()));
    let migrating = b.place::<Species>("Migrating");
    for place in habitats.into_iter().chain([migrating]) {
        b.serializable(place);
        b.keys(place, KEY_NAMES, keys_of);
    }
    b.param(Season("Summer".to_owned()));
    b.serializable_param::<Season>("season");
    b.param(SpreadThreshold(1000));
    b.serializable_param::<SpreadThreshold>("spread_threshold");

    let [forest, wetland, savanna, reef, tundra] = habitats;
    use {Class::*, Diet::*};
    b.seed(
        forest,
        [
            Species::new("Robin", Bird, Insectivore, 120),
            Species::new("Fox", Mammal, Omnivore, 60),
        ],
    );
    b.seed(
        wetland,
        [
            Species::new("Robin", Bird, Insectivore, 80),
            Species::new("Heron", Bird, Carnivore, 40),
        ],
    );
    b.seed(
        savanna,
        [
            Species::new("Zebra", Mammal, Herbivore, 5000),
            Species::new("Gecko", Reptile, Insectivore, 200),
        ],
    );
    b.seed(reef, [Species::new("Clownfish", Fish, Omnivore, 300)]);
    b.seed(
        tundra,
        [
            Species::new("Fox", Mammal, Omnivore, 30),
            Species::new("Arctic Hare", Mammal, Herbivore, 90),
        ],
    );

    // phase 2: one TakeWhere edge
    //
    // Until then, the game's pattern P2: every species leaves Forest, the
    // oldest bird goes on to Migrating (slot 1), and the rest go back to
    // Forest (slot 0), each under a new id. The guard asks the key index
    // whether Forest holds a bird at all.
    let migrate = b
        .transition(
            "Migrate Forest Wetland",
            (TakeAllEdge(forest),),
            (forest, migrating),
            |mut all: Vec<Species>| -> Result<(Vec<Species>, Species), WildlifeError> {
                let oldest = all
                    .iter()
                    .position(|species| species.class == Class::Bird)
                    .ok_or(WildlifeError::NothingToMigrate)?;
                let mut bird = all.remove(oldest);
                bird.destination = Some(Habitat::Wetland.to_string());
                Ok((all, bird))
            },
        )
        .guard(move |net| {
            season(net) == "Autumn"
                && net
                    .distinct_values("class", Some(&[forest.id()][..]))
                    .contains(&Class::Bird.into())
        })
        .id();

    let settle = b
        .transition(
            "Settle",
            (TakeEdge(migrating),),
            (forest, wetland, savanna, reef, tundra),
            move |mut species: Species| -> Result<Distribution, WildlifeError> {
                let destination =
                    species
                        .destination
                        .take()
                        .ok_or_else(|| WildlifeError::NowhereToGo {
                            species: species.name.clone(),
                        })?;
                let habitat =
                    Habitat::named(&destination).ok_or_else(|| WildlifeError::UnknownHabitat {
                        species: species.name.clone(),
                        habitat: destination,
                    })?;
                Ok(Distribution::new().to(habitats[habitat as usize], species))
            },
        )
        .id();

    let spread = b
        .transition(
            "Spread Savanna Reef",
            (MutateEdge(savanna),),
            (reef,),
            |species: &mut Species| {
                let kept = species.population / 2;
                let moved = species.population - kept;
                species.population = kept;
                (Species {
                    population: moved,
                    ..species.clone()
                },)
            },
        )
        .guard(move |net| {
            front(net, savanna).is_some_and(|species| species.population > spread_threshold(net))
        })
        .id();

    let decline = b
        .transition(
            "Decline Forest",
            (MutateEdge(forest),),
            (),
            |species: &mut Species| species.population /= 2,
        )
        .guard(move |net| front(net, forest).is_some_and(|species| species.population > 0))
        .id();

    Ok(Wildlife {
        net: b.build()?,
        habitats,
        migrating,
        migrate,
        settle,
        spread,
        decline,
    })
}
