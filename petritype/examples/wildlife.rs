//! Wildlife: species across habitats, and the keys to ask for them by
//! (DESIGN §8.3).
//!
//! The example of the token-keys proposal
//! (`Cyberpunk2020-Simulation/dev-docs/TOKEN_QUERYING_AND_FILTERING.md` §2),
//! built as the conformance net `wildlife` of the shared protocol
//! (`PROTOCOL.md` §11), which lives in [`net`] so that a runner can carry the
//! same file. Places are habitats (`Forest`, `Wetland`, `Savanna`, `Reef`,
//! `Tundra`) and `Migrating`; a token is one species occurring in one
//! habitat, and every place declares the keys `species`, `class` and `diet`.
//!
//! `main` asks the proposal's five questions on the seed, then again after
//! one migration (`Migrate Forest Wetland`, then `Settle`: it is autumn) and
//! one spread (`Spread Savanna Wetland`, the egret), lists the classes,
//! which is what a viewer would make its tabs of, shows the declines halving
//! the deer and the lemmings, and on a fresh net shows the migrations
//! waiting for autumn and runs it until nothing is fireable. The tests below
//! hold the answers, and the plain run's nineteen firings.
//!
//! Every transition but `Settle` reads its habitat through a keyed edge
//! (DESIGN §8.3, slice 19): `TakeWhereEdge(forest, class in {Bird})` takes
//! the oldest bird and leaves every other species where it is, under the id
//! it had, and a `MutateWhereEdge` hands its body the species it is about
//! wherever it stands, which is what its guard reads too. Before keyed edges
//! `Migrate` took the whole of `Forest` and put back all but the bird, every
//! one of them under a new id (the game's pattern P2); the tests below hold
//! the difference: one token consumed, one produced, the fox untouched.

// A crate root's `mod` looks beside it, so the path is spelled out: the net
// lives in `wildlife/net.rs`, the file a runner's example can include too.
#[path = "wildlife/net.rs"]
mod net;

use std::error::Error;

use petritype::{Filter, KeyValue, Net, PlaceId, TransitionId};

use net::{Habitat, Season, Species, Wildlife, build_wildlife, herbivores, lemmings};

/// The five questions of the proposal's §2, the places the last one names
/// taken from the net.
fn questions(wildlife: &Wildlife) -> [(&'static str, Filter); 5] {
    [
        (
            "species in {Robin}",
            Filter::new().key_in("species", ["Robin"]),
        ),
        ("class in {Bird}", Filter::new().key_in("class", ["Bird"])),
        (
            "class in {Bird, Mammal}",
            Filter::new().key_in("class", ["Bird", "Mammal"]),
        ),
        (
            "class in {Bird} and diet in {Insectivore}",
            Filter::new()
                .key_in("class", ["Bird"])
                .key_in("diet", ["Insectivore"]),
        ),
        (
            "class in {Mammal} in {Forest, Tundra}",
            Filter::new().key_in("class", ["Mammal"]).in_places([
                wildlife.at(Habitat::Forest).id(),
                wildlife.at(Habitat::Tundra).id(),
            ]),
        ),
    ]
}

/// The name of a place of `net`.
fn place_name(net: &Net, place: PlaceId) -> String {
    net.places()
        .find(|(id, _)| *id == place)
        .map_or_else(|| format!("{place:?}"), |(_, info)| info.name.to_owned())
}

/// The name of a transition of `net`.
fn transition_name(net: &Net, transition: TransitionId) -> &str {
    net.transitions()
        .find(|(id, _)| *id == transition)
        .map_or("", |(_, info)| info.name)
}

/// Places in place order, each with what `entry` writes of its tokens in
/// token order: `"Forest: Robin, Fox; Wetland: Robin, Heron"`. Empty places
/// are left out.
fn by_place(net: &Net, found: impl IntoIterator<Item = (PlaceId, String)>) -> String {
    let mut places: Vec<(PlaceId, Vec<String>)> = Vec::new();
    for (place, entry) in found {
        match places.last_mut() {
            Some((at, entries)) if *at == place => entries.push(entry),
            _ => places.push((place, vec![entry])),
        }
    }
    places
        .into_iter()
        .map(|(place, entries)| format!("{}: {}", place_name(net, place), entries.join(", ")))
        .collect::<Vec<_>>()
        .join("; ")
}

/// What a query found, written as the proposal's §6 table writes it:
/// `"Forest: Robin, Fox; Wetland: Robin, Heron"`, places in place order and
/// species in token order; empty when nothing matched.
fn answer(net: &Net, filter: &Filter) -> String {
    let found = net
        .query(filter)
        .expect("every place a question names is in the net");
    by_place(
        net,
        found.into_iter().map(|found| {
            let species = found
                .keys
                .get("species")
                .map_or_else(String::new, KeyValue::to_string);
            (found.place, species)
        }),
    )
}

/// Every species with its population, place by place:
/// `"Forest: Fox 60, Deer 9; ..."`.
fn census(net: &Net) -> String {
    by_place(
        net,
        net.places()
            .flat_map(|(place, _)| net.tokens(place).map(move |(token, _)| (place, token)))
            .filter_map(|(place, token)| {
                let species = net.token::<Species>(place, token)?;
                Some((place, format!("{} {}", species.name, species.population)))
            }),
    )
}

/// Steps `net` with the default selector until nothing is fireable, and
/// returns what fired, in order.
fn run_to_the_end(net: &mut Net) -> Result<Vec<TransitionId>, Box<dyn Error>> {
    let mut fired = Vec::new();
    while let Some(transition) = net.step()? {
        fired.push(transition);
    }
    Ok(fired)
}

/// A firing sequence as runs of the same transition:
/// `"Settle ×3, Spread Savanna Wetland"`.
fn runs(net: &Net, fired: &[TransitionId]) -> String {
    let mut runs: Vec<(TransitionId, usize)> = Vec::new();
    for &transition in fired {
        match runs.last_mut() {
            Some((last, count)) if *last == transition => *count += 1,
            _ => runs.push((transition, 1)),
        }
    }
    runs.into_iter()
        .map(|(transition, count)| match count {
            1 => transition_name(net, transition).to_owned(),
            _ => format!("{} ×{count}", transition_name(net, transition)),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// A list of key values as `Bird, Mammal, Fish, Reptile`.
fn list(values: &[KeyValue]) -> String {
    values
        .iter()
        .map(KeyValue::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Prints the five questions with what the net answers now.
fn ask(wildlife: &Wildlife) {
    for (question, filter) in questions(wildlife) {
        println!("  {question}: {}", answer(&wildlife.net, &filter));
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut wildlife = build_wildlife()?;
    println!("{}", petritype_dot::dot(&wildlife.net));

    println!("On the seed:");
    ask(&wildlife);
    println!(
        "  distinct class: {}",
        list(&wildlife.net.distinct_values("class", None))
    );

    // Autumn, the default: the oldest bird of Forest sets off, and settles
    // in Wetland.
    wildlife.net.fire(wildlife.migrate_forest)?;
    let on_the_move = Filter::new().in_places([wildlife.migrating.id()]);
    println!(
        "\nMigrating after Migrate Forest Wetland: {}",
        answer(&wildlife.net, &on_the_move)
    );
    wildlife.net.fire(wildlife.settle)?;
    // The egret, above the threshold, spreads to Wetland; the zebra in front
    // of it is not what the edge is about, and stays as it is.
    wildlife.net.fire(wildlife.spread)?;

    println!("\nAfter one migration and one spread:");
    ask(&wildlife);

    // A decline halves the species its filter names, wherever it stands,
    // and leaves the keys as they were.
    println!();
    for (decline, habitat, filter) in [
        (wildlife.decline_forest, Habitat::Forest, herbivores()),
        (wildlife.decline_tundra, Habitat::Tundra, lemmings()),
    ] {
        let place = wildlife.at(habitat).id();
        let token = wildlife
            .net
            .first_match(place, &filter)
            .ok_or("nothing to decline")?;
        let before = wildlife.net.keys(place, token);
        wildlife.net.fire(decline)?;
        println!(
            "{}: {}; keys unchanged: {}",
            transition_name(&wildlife.net, decline),
            wildlife.net.label(place, token).unwrap_or_default(),
            wildlife.net.keys(place, token) == before,
        );
    }

    // On a fresh net: in summer the birds stay, and in autumn, the default,
    // they go; left alone, the net runs until nothing is fireable.
    let mut fresh = build_wildlife()?;
    let migrations = [fresh.migrate_forest, fresh.migrate_tundra];
    let fireable = |net: &Net| {
        let names: Vec<&str> = migrations
            .into_iter()
            .filter(|&migration| net.is_fireable(migration))
            .map(|migration| transition_name(net, migration))
            .collect();
        if names.is_empty() {
            "none".to_owned()
        } else {
            names.join(", ")
        }
    };
    fresh.net.set_param(Season("Summer".to_owned()))?;
    println!("\nMigrations fireable in summer: {}", fireable(&fresh.net));
    fresh.net.set_param(Season("Autumn".to_owned()))?;
    println!("Migrations fireable in autumn: {}", fireable(&fresh.net));
    let fired = run_to_the_end(&mut fresh.net)?;
    println!(
        "\nA plain run from the seed, {} firings: {}",
        fired.len(),
        runs(&fresh.net, &fired)
    );
    println!("Then: {}", census(&fresh.net));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use net::{Class, Diet, KEY_NAMES, SpreadThreshold, birds};
    use petritype::serde_json::{self, json};
    use petritype::{Event, JsonCommand, NetError, TokenId};

    /// The proposal's five questions on the seed.
    const ON_THE_SEED: [&str; 5] = [
        "Forest: Robin; Wetland: Robin",
        "Forest: Robin, Woodpecker; Wetland: Robin, Heron; Savanna: Egret; Tundra: Snowy Owl",
        "Forest: Robin, Fox, Deer, Woodpecker; Wetland: Robin, Heron, Otter; \
         Savanna: Zebra, Lion, Egret; Tundra: Fox, Arctic Hare, Lemming, Snowy Owl",
        "Forest: Robin, Woodpecker; Wetland: Robin; Savanna: Egret",
        "Forest: Fox, Deer; Tundra: Fox, Arctic Hare, Lemming",
    ];

    /// The five answers after a migration has settled and the egret has
    /// spread: the Forest robin is in Wetland, behind the four already
    /// there, and a second egret token is behind it.
    const AFTER_MIGRATION_AND_SPREAD: [&str; 5] = [
        "Wetland: Robin, Robin",
        "Forest: Woodpecker; Wetland: Robin, Heron, Robin, Egret; Savanna: Egret; \
         Tundra: Snowy Owl",
        "Forest: Fox, Deer, Woodpecker; Wetland: Robin, Heron, Otter, Robin, Egret; \
         Savanna: Zebra, Lion, Egret; Tundra: Fox, Arctic Hare, Lemming, Snowy Owl",
        "Forest: Woodpecker; Wetland: Robin, Robin, Egret; Savanna: Egret",
        "Forest: Fox, Deer; Tundra: Fox, Arctic Hare, Lemming",
    ];

    fn answers(wildlife: &Wildlife) -> Vec<String> {
        questions(wildlife)
            .iter()
            .map(|(_, filter)| answer(&wildlife.net, filter))
            .collect()
    }

    /// The net after the Forest robin has migrated and settled (it is
    /// autumn from the start).
    fn migrated() -> Wildlife {
        let mut wildlife = build_wildlife().expect("the net builds");
        wildlife
            .net
            .fire(wildlife.migrate_forest)
            .expect("a bird in Forest");
        wildlife.net.fire(wildlife.settle).expect("one on the move");
        wildlife
    }

    fn species(net: &Net, place: PlaceId, token: TokenId) -> &Species {
        net.token::<Species>(place, token).expect("a species")
    }

    /// The token ids of a place, in token order.
    fn ids(net: &Net, place: PlaceId) -> Vec<TokenId> {
        net.tokens(place).map(|(id, _)| id).collect()
    }

    #[test]
    fn the_net_is_the_conformance_net_of_protocol_section_11() {
        let wildlife = build_wildlife().expect("the net builds");
        let net = &wildlife.net;
        let places: Vec<(&str, bool, &[&str])> = net
            .places()
            .map(|(_, info)| (info.name, info.serializable, info.key_names))
            .collect();
        let keyed: &[&str] = &KEY_NAMES;
        assert_eq!(
            places,
            [
                ("Forest", true, keyed),
                ("Wetland", true, keyed),
                ("Savanna", true, keyed),
                ("Reef", true, keyed),
                ("Tundra", true, keyed),
                ("Migrating", true, keyed),
            ],
        );
        let transitions: Vec<&str> = net.transitions().map(|(_, info)| info.name).collect();
        assert_eq!(
            transitions,
            [
                "Migrate Forest Wetland",
                "Migrate Tundra Wetland",
                "Settle",
                "Spread Savanna Wetland",
                "Decline Forest",
                "Decline Tundra",
            ],
        );
        assert!(net.transitions().all(|(_, info)| info.enabled));
        assert_eq!(net.param::<Season>(), Some(&Season("Autumn".to_owned())));
        assert_eq!(net.param::<SpreadThreshold>(), Some(&SpreadThreshold(200)));
        assert_eq!(
            net.param_name(std::any::type_name::<Season>()),
            Some("season")
        );
        assert_eq!(
            net.param_name(std::any::type_name::<SpreadThreshold>()),
            Some("spread_threshold")
        );

        // The seed, in place order then token order, with its JSON.
        let seeded: Vec<serde_json::Value> = net
            .places()
            .flat_map(|(place, _)| net.tokens(place).map(move |(token, _)| (place, token)))
            .map(|(place, token)| net.json(place, token).expect("serializable"))
            .collect();
        let seed = |name: &str, class: &str, diet: &str, population: u32| {
            json!({"name": name, "class": class, "diet": diet,
                   "population": population, "destination": null})
        };
        assert_eq!(
            seeded,
            [
                seed("Robin", "Bird", "Insectivore", 120),
                seed("Fox", "Mammal", "Omnivore", 60),
                seed("Deer", "Mammal", "Herbivore", 300),
                seed("Woodpecker", "Bird", "Insectivore", 45),
                seed("Robin", "Bird", "Insectivore", 80),
                seed("Heron", "Bird", "Carnivore", 40),
                seed("Pike", "Fish", "Carnivore", 150),
                seed("Otter", "Mammal", "Carnivore", 25),
                seed("Zebra", "Mammal", "Herbivore", 5000),
                seed("Gecko", "Reptile", "Insectivore", 200),
                seed("Lion", "Mammal", "Carnivore", 40),
                seed("Egret", "Bird", "Insectivore", 1500),
                seed("Clownfish", "Fish", "Omnivore", 300),
                seed("Parrotfish", "Fish", "Herbivore", 400),
                seed("Reef Shark", "Fish", "Carnivore", 20),
                seed("Sea Turtle", "Reptile", "Herbivore", 80),
                seed("Fox", "Mammal", "Omnivore", 30),
                seed("Arctic Hare", "Mammal", "Herbivore", 90),
                seed("Lemming", "Mammal", "Herbivore", 2000),
                seed("Snowy Owl", "Bird", "Carnivore", 15),
            ],
        );

        // It is autumn: everything but Settle can fire from the start.
        assert_eq!(
            net.fireable(),
            [
                wildlife.migrate_forest,
                wildlife.migrate_tundra,
                wildlife.spread,
                wildlife.decline_forest,
                wildlife.decline_tundra,
            ]
        );

        // The topology says which keys each place derives, and which
        // transitions read their place through a keyed edge with which
        // filter, in the core's id form (a runner names the places).
        let topology = serde_json::to_value(wildlife.net.topology()).unwrap();
        assert_eq!(
            topology["places"][0]["key_names"],
            json!(["species", "class", "diet"])
        );
        let edges = topology["edges"].as_array().unwrap();
        let input = |transition: u32| {
            edges
                .iter()
                .find(|edge| edge["edge"] == "input" && edge["transition"] == transition)
                .unwrap()
                .clone()
        };
        let outputs = |transition: u32| -> Vec<serde_json::Value> {
            edges
                .iter()
                .filter(|edge| edge["edge"] == "output" && edge["transition"] == transition)
                .map(|edge| edge["place"].clone())
                .collect()
        };
        let clause = |key: &str, value: &str| json!({"clauses": [{"key": key, "values": [value]}]});
        assert_eq!(
            input(0),
            json!({"edge": "input", "place": 0, "transition": 0, "kind": "take_where",
                   "filter": clause("class", "Bird")})
        );
        assert_eq!(outputs(0), [json!(5)], "into Migrating alone");
        assert_eq!(
            input(1),
            json!({"edge": "input", "place": 4, "transition": 1, "kind": "take_where",
                   "filter": clause("class", "Bird")})
        );
        assert_eq!(outputs(1), [json!(5)], "into Migrating alone");
        assert_eq!(input(2)["kind"], "take", "Settle takes the front");
        assert_eq!(
            input(3),
            json!({"edge": "input", "place": 2, "transition": 3, "kind": "mutate_where",
                   "filter": clause("class", "Bird")})
        );
        assert_eq!(outputs(3), [json!(1)], "the copy goes to Wetland");
        assert_eq!(
            input(4),
            json!({"edge": "input", "place": 0, "transition": 4, "kind": "mutate_where",
                   "filter": clause("diet", "Herbivore")})
        );
        assert!(outputs(4).is_empty());
        assert_eq!(
            input(5),
            json!({"edge": "input", "place": 4, "transition": 5, "kind": "mutate_where",
                   "filter": clause("species", "Lemming")})
        );
        assert!(outputs(5).is_empty());
    }

    #[test]
    fn every_token_reports_its_three_keys() {
        let wildlife = build_wildlife().expect("the net builds");
        let net = &wildlife.net;
        let mut seen = 0;
        for (place, info) in net.places() {
            assert_eq!(info.key_names, KEY_NAMES, "{}", info.name);
            for (token, _) in net.tokens(place) {
                let keys = net.keys(place, token).expect("every place declares keys");
                let species = species(net, place, token);
                assert_eq!(
                    keys.iter().collect::<Vec<_>>(),
                    [
                        ("species", &KeyValue::from(species.name.as_str())),
                        ("class", &KeyValue::from(species.class)),
                        ("diet", &KeyValue::from(species.diet)),
                    ],
                );
                seen += 1;
            }
        }
        assert_eq!(seen, 20, "twenty tokens, four to a habitat");
    }

    #[test]
    fn the_five_questions_on_the_seed() {
        let wildlife = build_wildlife().expect("the net builds");
        assert_eq!(answers(&wildlife), ON_THE_SEED);
    }

    #[test]
    fn after_a_migration_the_forest_robin_is_in_wetland() {
        let wildlife = migrated();
        let wetland = wildlife.at(Habitat::Wetland).id();
        let robins = wildlife
            .net
            .query(&Filter::new().key_in("species", ["Robin"]))
            .unwrap();
        assert_eq!(robins.len(), 2);
        assert!(robins.iter().all(|found| found.place == wetland));
        let settled = robins.last().expect("two robins");
        let robin = species(&wildlife.net, settled.place, settled.token);
        assert_eq!(
            (robin.name.as_str(), robin.population, &robin.destination),
            ("Robin", 120, &None),
            "the Forest robin, its destination cleared"
        );
        assert_eq!(answers(&wildlife)[0], "Wetland: Robin, Robin");
        assert_eq!(
            answers(&wildlife)[1],
            "Forest: Woodpecker; Wetland: Robin, Heron, Robin; Savanna: Egret; Tundra: Snowy Owl"
        );
    }

    #[test]
    fn after_a_migration_and_a_spread_the_five_questions_again() {
        let mut wildlife = migrated();
        wildlife
            .net
            .fire(wildlife.spread)
            .expect("the egret spreads");
        assert_eq!(answers(&wildlife), AFTER_MIGRATION_AND_SPREAD);
        assert_eq!(
            answer(&wildlife.net, &Filter::new().key_in("species", ["Egret"])),
            "Wetland: Egret; Savanna: Egret"
        );
        let populations: Vec<u32> = wildlife
            .net
            .query(&Filter::new().key_in("species", ["Egret"]))
            .unwrap()
            .iter()
            .map(|found| species(&wildlife.net, found.place, found.token).population)
            .collect();
        assert_eq!(populations, [750, 750], "halved, the copy taking the rest");
    }

    /// The spread is about the egret, whoever stands in front of it: the
    /// zebra, first in `Savanna` and far above the threshold, is neither
    /// read nor touched. One firing: the egret mutated where it stands, under
    /// its id, and one token produced, the copy in `Wetland`; nothing
    /// consumed. An odd population leaves the copy the larger half.
    #[test]
    fn the_egret_spreads_and_the_zebra_stays() {
        let mut wildlife = build_wildlife().expect("the net builds");
        let (savanna, wetland) = (
            wildlife.at(Habitat::Savanna).id(),
            wildlife.at(Habitat::Wetland).id(),
        );
        let seeded = ids(&wildlife.net, savanna);
        let [zebra, _, _, egret] = seeded[..] else {
            panic!("four species in Savanna")
        };
        assert_eq!(wildlife.net.first_match(savanna, &birds()), Some(egret));
        wildlife.net.drain_events().for_each(drop);

        wildlife.net.fire(wildlife.spread).unwrap();
        let events: Vec<Event> = wildlife.net.drain_events().collect();
        let [
            Event::Mutated { place, token },
            Event::Fired {
                consumed, produced, ..
            },
        ] = &events[..]
        else {
            panic!("the egret mutated, then the firing: {events:?}")
        };
        assert_eq!((*place, *token), (savanna, egret));
        assert!(consumed.is_empty(), "a mutate edge consumes nothing");
        let [(into, copy)] = produced[..] else {
            panic!("one copy: {produced:?}")
        };
        assert_eq!(into, wetland);
        assert_eq!(ids(&wildlife.net, savanna), seeded, "nobody moved");
        assert_eq!(species(&wildlife.net, savanna, egret).population, 750);
        assert_eq!(species(&wildlife.net, wetland, copy).name, "Egret");
        assert_eq!(species(&wildlife.net, wetland, copy).population, 750);
        assert_eq!(species(&wildlife.net, savanna, zebra).population, 5000);

        wildlife.net.fire(wildlife.spread).unwrap();
        wildlife.net.fire(wildlife.spread).unwrap();
        let egrets: Vec<u32> = wildlife
            .net
            .query(&Filter::new().key_in("species", ["Egret"]))
            .unwrap()
            .iter()
            .map(|found| species(&wildlife.net, found.place, found.token).population)
            .collect();
        assert_eq!(
            egrets,
            [750, 375, 188, 187],
            "Wetland's copies, then Savanna's egret"
        );
        assert!(
            !wildlife.net.is_fireable(wildlife.spread),
            "187 is not above 200"
        );
        wildlife
            .net
            .set_param(SpreadThreshold(100))
            .expect("a parameter of the net");
        assert!(
            wildlife.net.is_fireable(wildlife.spread),
            "but it is above 100"
        );
    }

    /// The declines halve the species their filters name, wherever it
    /// stands, down to a floor and never to zero: the deer thin while there
    /// are more than ten, the lemmings crash while there are more than a
    /// hundred. The robin in front of the deer is not touched.
    #[test]
    fn the_declines_halve_down_to_a_floor() {
        let mut wildlife = build_wildlife().expect("the net builds");
        let (forest, tundra) = (
            wildlife.at(Habitat::Forest).id(),
            wildlife.at(Habitat::Tundra).id(),
        );
        let deer = wildlife.net.first_match(forest, &herbivores()).unwrap();
        let lemming = wildlife.net.first_match(tundra, &lemmings()).unwrap();
        let (robin, _) = wildlife.net.tokens(forest).next().unwrap();
        let population = |net: &Net, place, token| species(net, place, token).population;

        let mut thinned = Vec::new();
        while wildlife.net.is_fireable(wildlife.decline_forest) {
            wildlife.net.fire(wildlife.decline_forest).unwrap();
            thinned.push(population(&wildlife.net, forest, deer));
        }
        assert_eq!(thinned, [150, 75, 37, 18, 9]);
        assert_eq!(population(&wildlife.net, forest, robin), 120);

        let mut crashed = Vec::new();
        while wildlife.net.is_fireable(wildlife.decline_tundra) {
            wildlife.net.fire(wildlife.decline_tundra).unwrap();
            crashed.push(population(&wildlife.net, tundra, lemming));
        }
        assert_eq!(crashed, [1000, 500, 250, 125, 62]);
    }

    /// The firings of a plain run, by the default selector: the first
    /// fireable transition declared fires, so each runs out before the next.
    fn plain_run(wildlife: &Wildlife) -> Vec<TransitionId> {
        let mut sequence = vec![wildlife.migrate_forest; 2];
        sequence.push(wildlife.migrate_tundra);
        sequence.extend([wildlife.settle; 3]);
        sequence.extend([wildlife.spread; 3]);
        sequence.extend([wildlife.decline_forest; 5]);
        sequence.extend([wildlife.decline_tundra; 5]);
        sequence
    }

    /// Left alone in autumn the net fires nineteen times and stops: three
    /// birds migrate and settle in Wetland, the egret spreads three times,
    /// the deer thin five times and the lemmings crash five times. The zebra
    /// never leaves the savanna and no population reaches zero.
    #[test]
    fn a_plain_run_fires_nineteen_times_and_stops() {
        let mut wildlife = build_wildlife().expect("the net builds");
        let fired = run_to_the_end(&mut wildlife.net).unwrap();
        assert_eq!(fired, plain_run(&wildlife));
        assert_eq!(fired.len(), 19);
        assert_eq!(
            census(&wildlife.net),
            "Forest: Fox 60, Deer 9; \
             Wetland: Robin 80, Heron 40, Pike 150, Otter 25, Robin 120, Woodpecker 45, \
             Snowy Owl 15, Egret 750, Egret 375, Egret 188; \
             Savanna: Zebra 5000, Gecko 200, Lion 40, Egret 187; \
             Reef: Clownfish 300, Parrotfish 400, Reef Shark 20, Sea Turtle 80; \
             Tundra: Fox 30, Arctic Hare 90, Lemming 62"
        );
        assert!(wildlife.net.fireable().is_empty());
    }

    /// In summer the birds stay: the migrations leave `fireable`, and the
    /// rest of the run is the same without them. Set back to autumn, they
    /// return.
    #[test]
    fn in_summer_the_birds_stay() {
        let mut wildlife = build_wildlife().expect("the net builds");
        wildlife
            .net
            .set_param(Season("Summer".to_owned()))
            .expect("a parameter of the net");
        assert_eq!(
            wildlife.net.fireable(),
            [
                wildlife.spread,
                wildlife.decline_forest,
                wildlife.decline_tundra
            ]
        );
        let fired = run_to_the_end(&mut wildlife.net).unwrap();
        let without_migrations: Vec<TransitionId> = plain_run(&wildlife)
            .into_iter()
            .filter(|&t| {
                ![
                    wildlife.migrate_forest,
                    wildlife.migrate_tundra,
                    wildlife.settle,
                ]
                .contains(&t)
            })
            .collect();
        assert_eq!(fired, without_migrations);
        assert_eq!(wildlife.net.count(wildlife.migrating.id()), 0);

        wildlife.net.set_param(Season("Autumn".to_owned())).unwrap();
        assert_eq!(
            wildlife.net.fireable(),
            [wildlife.migrate_forest, wildlife.migrate_tundra]
        );
    }

    /// The five answers once the Forest robin has set off and not yet
    /// settled: it is in `Migrating`, the last place.
    const ON_THE_MOVE: [&str; 5] = [
        "Wetland: Robin; Migrating: Robin",
        "Forest: Woodpecker; Wetland: Robin, Heron; Savanna: Egret; Tundra: Snowy Owl; \
         Migrating: Robin",
        "Forest: Fox, Deer, Woodpecker; Wetland: Robin, Heron, Otter; \
         Savanna: Zebra, Lion, Egret; Tundra: Fox, Arctic Hare, Lemming, Snowy Owl; \
         Migrating: Robin",
        "Forest: Woodpecker; Wetland: Robin; Savanna: Egret; Migrating: Robin",
        "Forest: Fox, Deer; Tundra: Fox, Arctic Hare, Lemming",
    ];

    /// The proposal's R5, done when: `Migrate`, one keyed edge, fires on a
    /// `Forest` holding a robin in front of a fox, a deer and a woodpecker,
    /// and its one `Fired` names exactly one consumed token and one
    /// produced; the others keep their ids and their places, and the index
    /// still answers the five questions. Then `Settle` is one more of each,
    /// which is the proposal's §6 "only events": one out of Forest, one into
    /// Migrating, one out of it, one into Wetland.
    #[test]
    fn migrate_takes_the_bird_and_leaves_the_fox_alone() {
        let mut wildlife = build_wildlife().expect("the net builds");
        let forest = wildlife.at(Habitat::Forest).id();
        let seeded = ids(&wildlife.net, forest);
        let [robin, fox, deer, woodpecker] = seeded[..] else {
            panic!("four species in Forest")
        };
        wildlife.net.drain_events().for_each(drop);

        wildlife.net.fire(wildlife.migrate_forest).unwrap();
        let events: Vec<Event> = wildlife.net.drain_events().collect();
        let [
            Event::Fired {
                consumed, produced, ..
            },
        ] = &events[..]
        else {
            panic!("one firing and nothing else: {events:?}")
        };
        assert_eq!(consumed, &[(forest, robin)], "the robin alone");
        let [(on_the_move, migrant)] = produced[..] else {
            panic!("one produced: {produced:?}")
        };
        assert_eq!(on_the_move, wildlife.migrating.id());
        assert_eq!(
            species(&wildlife.net, on_the_move, migrant).destination,
            Some("Wetland".to_owned())
        );
        assert_eq!(
            ids(&wildlife.net, forest),
            [fox, deer, woodpecker],
            "the rest stayed, under their own ids"
        );
        assert_eq!(answers(&wildlife), ON_THE_MOVE);
        let snapshot = wildlife.net.snapshot().unwrap();
        for (question, filter) in questions(&wildlife) {
            assert_eq!(
                wildlife.net.query(&filter).unwrap(),
                snapshot.query(&filter).unwrap(),
                "the index against a scan: {question}"
            );
        }

        wildlife.net.fire(wildlife.settle).unwrap();
        let events: Vec<Event> = wildlife.net.drain_events().collect();
        assert!(
            matches!(&events[..], [Event::Fired { consumed, produced, .. }]
                if consumed == &[(on_the_move, migrant)]
                    && produced.len() == 1
                    && produced[0].0 == wildlife.at(Habitat::Wetland).id()),
            "{events:?}"
        );
        assert_eq!(ids(&wildlife.net, forest), [fox, deer, woodpecker]);
    }

    /// The bird need not be in front: the woodpecker stands behind the fox
    /// and the deer, and `Migrate` takes it from the back; then, with a
    /// gecko and an owl injected behind them, the owl, as PROTOCOL's
    /// transcript 06 has it. With no bird left, it is not fireable, autumn
    /// or not.
    #[test]
    fn migrate_takes_the_oldest_bird_wherever_it_stands() {
        let mut wildlife = migrated();
        let forest = wildlife.at(Habitat::Forest);
        let [fox, deer, woodpecker] = ids(&wildlife.net, forest.id())[..] else {
            panic!("three species left in Forest")
        };
        assert_eq!(
            wildlife.net.first_match(forest.id(), &birds()),
            Some(woodpecker),
            "the token the edge would take"
        );
        wildlife.net.fire(wildlife.migrate_forest).unwrap();
        assert_eq!(ids(&wildlife.net, forest.id()), [fox, deer]);
        assert!(
            !wildlife.net.is_fireable(wildlife.migrate_forest),
            "no bird left in Forest"
        );

        let gecko = wildlife.net.put(
            forest,
            Species::new("Gecko", Class::Reptile, Diet::Insectivore, 5),
        );
        let owl = wildlife
            .net
            .put(forest, Species::new("Owl", Class::Bird, Diet::Carnivore, 5));
        assert_eq!(wildlife.net.first_match(forest.id(), &birds()), Some(owl));
        wildlife.net.drain_events().for_each(drop);

        wildlife.net.fire(wildlife.migrate_forest).unwrap();
        let events: Vec<Event> = wildlife.net.drain_events().collect();
        assert!(
            matches!(&events[..], [Event::Fired { consumed, produced, .. }]
                if consumed == &[(forest.id(), owl)] && produced.len() == 1),
            "{events:?}"
        );
        assert_eq!(ids(&wildlife.net, forest.id()), [fox, deer, gecko]);
    }

    #[test]
    fn distinct_values_in_order_of_first_appearance() {
        let wildlife = build_wildlife().expect("the net builds");
        let net = &wildlife.net;
        assert_eq!(
            net.distinct_values("class", None),
            [Class::Bird, Class::Mammal, Class::Fish, Class::Reptile].map(KeyValue::from)
        );
        let tundra = [wildlife.at(Habitat::Tundra).id()];
        assert_eq!(
            net.distinct_values("diet", Some(&tundra)),
            [Diet::Omnivore, Diet::Herbivore, Diet::Carnivore].map(KeyValue::from)
        );
        assert!(net.distinct_values("colour", None).is_empty());
    }

    #[test]
    fn a_decline_keeps_the_keys_and_a_rename_changes_them() {
        let mut wildlife = build_wildlife().expect("the net builds");
        let forest = wildlife.at(Habitat::Forest);
        let deer = wildlife
            .net
            .first_match(forest.id(), &herbivores())
            .unwrap();
        let before = wildlife.net.keys(forest.id(), deer).unwrap();

        wildlife.net.fire(wildlife.decline_forest).unwrap();
        assert_eq!(species(&wildlife.net, forest.id(), deer).population, 150);
        assert_eq!(
            wildlife.net.keys(forest.id(), deer).unwrap(),
            before,
            "population is not a key"
        );
        assert_eq!(answers(&wildlife), ON_THE_SEED);

        let (robin, _) = wildlife.net.tokens(forest.id()).next().unwrap();
        let before = wildlife.net.keys(forest.id(), robin).unwrap();
        let mut renamed = species(&wildlife.net, forest.id(), robin).clone();
        renamed.name = "European Robin".to_owned();
        wildlife.net.replace(forest, robin, renamed).unwrap();
        let after = wildlife.net.keys(forest.id(), robin).unwrap();
        assert_eq!(
            after.get("species"),
            Some(&KeyValue::from("European Robin"))
        );
        assert_eq!(after.get("class"), before.get("class"));
        assert_eq!(
            answer(&wildlife.net, &Filter::new().key_in("species", ["Robin"])),
            "Wetland: Robin"
        );
        assert_eq!(
            answer(
                &wildlife.net,
                &Filter::new().key_in("species", ["European Robin"])
            ),
            "Forest: European Robin"
        );
        assert_eq!(
            wildlife.net.distinct_values("species", None)[0],
            KeyValue::from("European Robin")
        );
    }

    #[test]
    fn a_filter_restricted_by_place_and_by_prefix() {
        let wildlife = build_wildlife().expect("the net builds");
        let net = &wildlife.net;
        let (forest, tundra) = (
            wildlife.at(Habitat::Forest).id(),
            wildlife.at(Habitat::Tundra).id(),
        );
        assert_eq!(
            answer(net, &Filter::new().with_prefix("T")),
            "Tundra: Fox, Arctic Hare, Lemming, Snowy Owl",
            "no clauses: every token of the places covered"
        );
        assert_eq!(
            answer(
                net,
                &Filter::new().key_in("diet", ["Omnivore"]).with_prefix("T")
            ),
            "Tundra: Fox"
        );
        assert_eq!(
            answer(
                net,
                &Filter::new()
                    .key_in("class", ["Mammal"])
                    .in_places([tundra, forest])
                    .with_prefix("F")
            ),
            "Forest: Fox, Deer",
            "both restrictions, and place order whatever order they are named in"
        );
        assert_eq!(answer(net, &Filter::new().with_prefix("Migrat")), "");
        assert_eq!(answer(net, &Filter::new().with_prefix("Transition")), "");
        let nowhere = PlaceId::new(99);
        assert!(matches!(
            net.query(&Filter::new().in_places([nowhere])),
            Err(NetError::NoSuchPlace { place }) if place == nowhere
        ));
    }

    #[test]
    fn a_clause_on_a_parameter_reads_its_current_value() {
        let mut wildlife = build_wildlife().expect("the net builds");
        let class_is_season = Filter::new().key_is_param("class", "season");
        assert_eq!(answer(&wildlife.net, &class_is_season), "", "Autumn");

        // A season named like a class, to show that the clause reads it.
        wildlife.net.set_param(Season("Bird".to_owned())).unwrap();
        assert_eq!(
            answer(&wildlife.net, &class_is_season),
            "Forest: Robin, Woodpecker; Wetland: Robin, Heron; Savanna: Egret; Tundra: Snowy Owl"
        );
        let forest = wildlife.at(Habitat::Forest).id();
        let (robin, _) = wildlife.net.tokens(forest).next().unwrap();
        assert!(wildlife.net.matches(&class_is_season, forest, robin));

        // A number is never a class, and a name the net has no parameter
        // under accepts nothing.
        for param in ["spread_threshold", "weather"] {
            let filter = Filter::new().key_is_param("class", param);
            assert_eq!(answer(&wildlife.net, &filter), "", "{param}");
        }
    }

    #[test]
    fn a_snapshot_answers_the_same() {
        let mut wildlife = migrated();
        wildlife.net.fire(wildlife.spread).unwrap();
        let snapshot = wildlife.net.snapshot().expect("every place clones");
        for (question, filter) in questions(&wildlife) {
            assert_eq!(
                snapshot.query(&filter).unwrap(),
                wildlife.net.query(&filter).unwrap(),
                "{question}"
            );
        }
        for key in ["species", "class", "diet"] {
            assert_eq!(
                snapshot.distinct_values(key, None),
                wildlife.net.distinct_values(key, None)
            );
        }
        let wetland = wildlife.at(Habitat::Wetland).id();
        let (token, _) = wildlife.net.tokens(wetland).next().unwrap();
        assert_eq!(
            snapshot.keys(wetland, token),
            wildlife.net.keys(wetland, token)
        );
    }

    /// A species injected as JSON, as the protocol's `inject` sends it,
    /// without a destination, is indexed like any other.
    #[test]
    fn an_injected_species_is_found() {
        let mut wildlife = build_wildlife().expect("the net builds");
        wildlife
            .net
            .apply_json(JsonCommand::Extend {
                place: "Forest".to_owned(),
                tokens: vec![json!({"name": "Owl", "class": "Bird",
                                    "diet": "Carnivore", "population": 5})],
            })
            .expect("a species");
        assert_eq!(
            answer(
                &wildlife.net,
                &Filter::new()
                    .key_in("class", ["Bird"])
                    .key_in("diet", ["Carnivore"])
            ),
            "Forest: Owl; Wetland: Heron; Tundra: Snowy Owl"
        );
    }
}
