//! The wildlife net: species across habitats, with keys to ask for them by.
//!
//! This is the conformance net `wildlife` of the shared protocol
//! (`../petritype/PROTOCOL.md` §11), the example of the token-keys proposal
//! (`Cyberpunk2020-Simulation/dev-docs/TOKEN_QUERYING_AND_FILTERING.md` §2),
//! in its second edition (2026-10-13): places, keys, transitions, guards,
//! parameters, seed and JSON exactly as the two runtimes agreed them, so that
//! a transcript recorded against the Python runner can replay against this
//! one. It depends on `petritype`, `serde` and `serde_json` only, so that a
//! runner's example binary can include this file as it is.
//!
//! A place is a habitat, and a token is one species occurring in one
//! habitat: the same species in two habitats is two tokens with the same
//! `name`. A token carries its own category, `class`, so "every bird" is a
//! question about keys the tokens carry, never something worked out from
//! names. Every place declares the same three keys: `species` (the name),
//! `class` and `diet`.
//!
//! Every transition but `Settle` reads its habitat through one keyed edge,
//! which picks the species it is about whoever stands in front of it, and
//! every guard reads that same species (the oldest the edge's filter
//! accepts, [`oldest`]), never the front token. Birds migrate to `Wetland`
//! from `Forest` and from `Tundra` in autumn (`TakeWhereEdge`, filter
//! `class in {Bird}`); the egret of `Savanna`, above `spread_threshold`,
//! spreads to `Wetland`, keeping half and sending a copy with the rest (a
//! `MutateWhereEdge` with an output); the deer of `Forest` thin while there
//! are more than ten, and the lemmings of `Tundra` crash while there are
//! more than a hundred (`MutateWhereEdge`, no output). A mutate edge removes
//! nothing, so a decline is a halving with a floor and never takes a
//! species to zero. Left alone, in autumn, the net fires nineteen times and
//! stops.
//!
//! The first edition (2026-10-09) spread whatever stood first in `Savanna`
//! (the zebra, onto the reef) and halved whatever stood first in `Forest`
//! (the robin, down to nothing), and waited for autumn with summer the
//! default.

use std::error::Error;
use std::fmt;

use petritype::{
    BuildError, Distribution, Filter, KeyValue, MutateWhereEdge, Net, NetBuilder, PlaceRef,
    TakeEdge, TakeWhereEdge, TransitionId,
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

/// Net parameter `season` (`"Autumn"`): birds migrate in `"Autumn"`, and
/// in any other season stay where they are. A string, as the protocol's
/// parameter is, not an enum of four.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Season(pub String);

/// Net parameter `spread_threshold` (`200`): a population above it spreads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpreadThreshold(pub u32);

/// What a body can find wrong.
#[derive(Debug)]
pub enum WildlifeError {
    /// A species settling with nowhere to go.
    NowhereToGo { species: String },
    /// A species settling in a habitat that is not one.
    UnknownHabitat { species: String, habitat: String },
}

impl fmt::Display for WildlifeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
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
    /// `Migrate Forest Wetland`: the oldest bird of `Forest` sets off.
    pub migrate_forest: TransitionId,
    /// `Migrate Tundra Wetland`: the oldest bird of `Tundra` sets off.
    pub migrate_tundra: TransitionId,
    /// `Settle`: the species at the front of `Migrating` arrives.
    pub settle: TransitionId,
    /// `Spread Savanna Wetland`: the egret of `Savanna` spreads to
    /// `Wetland`.
    pub spread: TransitionId,
    /// `Decline Forest`: the deer of `Forest` thin.
    pub decline_forest: TransitionId,
    /// `Decline Tundra`: the lemmings of `Tundra` crash.
    pub decline_tundra: TransitionId,
}

impl Wildlife {
    /// The place of one habitat.
    pub fn at(&self, habitat: Habitat) -> PlaceRef<Species> {
        self.habitats[habitat as usize]
    }
}

/// The species of `place` a keyed edge with `filter` would take or be
/// handed now: the oldest the filter accepts, wherever it stands
/// ([`Net::first_match`]). What every guard of this net reads.
pub fn oldest<'n>(net: &'n Net, place: PlaceRef<Species>, filter: &Filter) -> Option<&'n Species> {
    let token = net.first_match(place.id(), filter)?;
    net.token::<Species>(place.id(), token)
}

/// `class in {Bird}`: what migrates, and what spreads.
pub fn birds() -> Filter {
    Filter::new().key_in("class", [Class::Bird])
}

/// `diet in {Herbivore}`: what thins in `Forest`.
pub fn herbivores() -> Filter {
    Filter::new().key_in("diet", [Diet::Herbivore])
}

/// `species in {Lemming}`: what crashes in `Tundra`.
pub fn lemmings() -> Filter {
    Filter::new().key_in("species", ["Lemming"])
}

fn season(net: &Net) -> &str {
    net.param::<Season>().map_or("", |season| season.0.as_str())
}

fn spread_threshold(net: &Net) -> u32 {
    net.param::<SpreadThreshold>()
        .map_or(u32::MAX, |threshold| threshold.0)
}

/// `Migrate <from> Wetland`: one keyed edge taking the oldest bird of
/// `from`, whoever stands in front of it, into `Migrating` with `Wetland`
/// for a destination. Fireable only while `from` holds a bird; the guard
/// waits for autumn, and `season` is a parameter, not a key, so not a
/// clause.
fn migrate(
    b: &mut NetBuilder,
    from: Habitat,
    place: PlaceRef<Species>,
    migrating: PlaceRef<Species>,
) -> TransitionId {
    b.transition(
        format!("Migrate {from} {}", Habitat::Wetland),
        (TakeWhereEdge(place, birds()),),
        (migrating,),
        |mut bird: Species| {
            bird.destination = Some(Habitat::Wetland.to_string());
            (bird,)
        },
    )
    .guard(|net| season(net) == "Autumn")
    .id()
}

/// `Decline <habitat>`: one keyed mutate edge halving the oldest species of
/// `place` that `filter` accepts, while there are more than `floor` of it,
/// so that none ever reaches zero.
fn decline(
    b: &mut NetBuilder,
    habitat: Habitat,
    place: PlaceRef<Species>,
    filter: Filter,
    floor: u32,
) -> TransitionId {
    let guarded = filter.clone();
    b.transition(
        format!("Decline {habitat}"),
        (MutateWhereEdge(place, filter),),
        (),
        |species: &mut Species| species.population /= 2,
    )
    .guard(move |net| {
        oldest(net, place, &guarded).is_some_and(|species| species.population > floor)
    })
    .id()
}

/// The net as seeded: twenty species-in-a-habitat in five habitats, nothing
/// migrating, `season` `"Autumn"`, `spread_threshold` 200, every transition
/// switched on.
pub fn build_wildlife() -> Result<Wildlife, BuildError> {
    let mut b = NetBuilder::new();
    let habitats = Habitat::ALL.map(|habitat| b.place::<Species>(habitat.to_string()));
    let migrating = b.place::<Species>("Migrating");
    for place in habitats.into_iter().chain([migrating]) {
        b.serializable(place);
        b.keys(place, KEY_NAMES, keys_of);
    }
    b.param(Season("Autumn".to_owned()));
    b.serializable_param::<Season>("season");
    b.param(SpreadThreshold(200));
    b.serializable_param::<SpreadThreshold>("spread_threshold");

    let [forest, wetland, savanna, reef, tundra] = habitats;
    use {Class::*, Diet::*};
    b.seed(
        forest,
        [
            Species::new("Robin", Bird, Insectivore, 120),
            Species::new("Fox", Mammal, Omnivore, 60),
            Species::new("Deer", Mammal, Herbivore, 300),
            Species::new("Woodpecker", Bird, Insectivore, 45),
        ],
    );
    b.seed(
        wetland,
        [
            Species::new("Robin", Bird, Insectivore, 80),
            Species::new("Heron", Bird, Carnivore, 40),
            Species::new("Pike", Fish, Carnivore, 150),
            Species::new("Otter", Mammal, Carnivore, 25),
        ],
    );
    b.seed(
        savanna,
        [
            Species::new("Zebra", Mammal, Herbivore, 5000),
            Species::new("Gecko", Reptile, Insectivore, 200),
            Species::new("Lion", Mammal, Carnivore, 40),
            Species::new("Egret", Bird, Insectivore, 1500),
        ],
    );
    b.seed(
        reef,
        [
            Species::new("Clownfish", Fish, Omnivore, 300),
            Species::new("Parrotfish", Fish, Herbivore, 400),
            Species::new("Reef Shark", Fish, Carnivore, 20),
            Species::new("Sea Turtle", Reptile, Herbivore, 80),
        ],
    );
    b.seed(
        tundra,
        [
            Species::new("Fox", Mammal, Omnivore, 30),
            Species::new("Arctic Hare", Mammal, Herbivore, 90),
            Species::new("Lemming", Mammal, Herbivore, 2000),
            Species::new("Snowy Owl", Bird, Carnivore, 15),
        ],
    );

    let migrate_forest = migrate(&mut b, Habitat::Forest, forest, migrating);
    let migrate_tundra = migrate(&mut b, Habitat::Tundra, tundra, migrating);

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

    // Cattle egrets follow the herds and roost in wetlands: the oldest bird
    // of Savanna, whoever stands in front of it, keeps half its population,
    // rounded down, and a copy with the rest lands in Wetland. The egret
    // keeps its id and its place.
    let spreading = birds();
    let spread = b
        .transition(
            "Spread Savanna Wetland",
            (MutateWhereEdge(savanna, birds()),),
            (wetland,),
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
            oldest(net, savanna, &spreading)
                .is_some_and(|species| species.population > spread_threshold(net))
        })
        .id();

    let decline_forest = decline(&mut b, Habitat::Forest, forest, herbivores(), 10);
    let decline_tundra = decline(&mut b, Habitat::Tundra, tundra, lemmings(), 100);

    Ok(Wildlife {
        net: b.build()?,
        habitats,
        migrating,
        migrate_forest,
        migrate_tundra,
        settle,
        spread,
        decline_forest,
        decline_tundra,
    })
}
