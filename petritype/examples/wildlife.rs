//! Wildlife: species across habitats, and the keys to ask for them by
//! (DESIGN §8.3).
//!
//! The example of the token-keys proposal
//! (`Cyberpunk2020-Simulation/dev-docs/TOKEN_QUERYING_AND_FILTERING.md` §2),
//! built as the conformance net `wildlife` of the shared protocol
//! (`PROTOCOL.md` §11), which lives in [`net`] so that a runner can carry the
//! same file. Places are habitats — `Forest`, `Wetland`, `Savanna`, `Reef`,
//! `Tundra` — and `Migrating`; a token is one species occurring in one
//! habitat, and every place declares the keys `species`, `class` and `diet`.
//!
//! `main` asks the proposal's five questions on the seed, then again after
//! one migration (`season` set to `"Autumn"`, `Migrate Forest Wetland`,
//! `Settle`) and one spread (`Spread Savanna Reef`), and lists the classes,
//! which is what a viewer would make its tabs of. The tests below hold the
//! proposal's §6 answers.
//!
//! `Migrate` is the "before keyed edges" form: it takes every species out of
//! `Forest` and puts back all but the oldest bird, so every one of them
//! leaves and returns under a new id. The test that says so is the one keyed
//! edges will change.

// A crate root's `mod` looks beside it, so the path is spelled out: the net
// lives in `wildlife/net.rs`, the file a runner's example can include too.
#[path = "wildlife/net.rs"]
mod net;

use std::error::Error;

use petritype::{Filter, KeyValue, Net, PlaceId};

use net::{Habitat, Season, Wildlife, build_wildlife};

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

/// What a query found, written as the proposal's §6 table writes it:
/// `"Forest: Robin, Fox; Wetland: Robin, Heron"`, places in place order and
/// species in token order; empty when nothing matched.
fn answer(net: &Net, filter: &Filter) -> String {
    let found = net
        .query(filter)
        .expect("every place a question names is in the net");
    let mut places: Vec<(PlaceId, Vec<String>)> = Vec::new();
    for found in found {
        let species = found
            .keys
            .get("species")
            .map_or_else(String::new, KeyValue::to_string);
        match places.last_mut() {
            Some((place, names)) if *place == found.place => names.push(species),
            _ => places.push((found.place, vec![species])),
        }
    }
    places
        .into_iter()
        .map(|(place, names)| format!("{}: {}", place_name(net, place), names.join(", ")))
        .collect::<Vec<_>>()
        .join("; ")
}

/// A list of key values as `Bird, Mammal, Reptile, Fish`.
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

    // Autumn: the oldest bird of Forest sets off, and settles in Wetland.
    wildlife.net.set_param(Season("Autumn".to_owned()))?;
    wildlife.net.fire(wildlife.migrate)?;
    let on_the_move = Filter::new().in_places([wildlife.migrating.id()]);
    println!(
        "\nMigrating after Migrate Forest Wetland: {}",
        answer(&wildlife.net, &on_the_move)
    );
    wildlife.net.fire(wildlife.settle)?;
    // The zebra, above the threshold, spreads to Reef.
    wildlife.net.fire(wildlife.spread)?;

    println!("\nAfter one migration and one spread:");
    ask(&wildlife);

    // A decline halves a population and leaves the keys as they were.
    let forest = wildlife.at(Habitat::Forest);
    let (fox, _) = wildlife
        .net
        .tokens(forest.id())
        .next()
        .ok_or("Forest is empty")?;
    let before = wildlife.net.keys(forest.id(), fox);
    wildlife.net.fire(wildlife.decline)?;
    println!(
        "\nDecline Forest: {}; keys unchanged: {}",
        wildlife.net.label(forest.id(), fox).unwrap_or_default(),
        wildlife.net.keys(forest.id(), fox) == before,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use net::{Class, Diet, KEY_NAMES, Species, SpreadThreshold};
    use petritype::serde_json::{self, json};
    use petritype::{Event, JsonCommand, NetError, TokenId};

    /// The proposal's §6 table: the five answers on the seed.
    const ON_THE_SEED: [&str; 5] = [
        "Forest: Robin; Wetland: Robin",
        "Forest: Robin; Wetland: Robin, Heron",
        "Forest: Robin, Fox; Wetland: Robin, Heron; Savanna: Zebra; Tundra: Fox, Arctic Hare",
        "Forest: Robin; Wetland: Robin",
        "Forest: Fox; Tundra: Fox, Arctic Hare",
    ];

    /// The five answers after a migration has settled and the zebra has
    /// spread: the Forest robin is in Wetland, behind the two already there,
    /// and a second zebra token is in Reef.
    const AFTER_MIGRATION_AND_SPREAD: [&str; 5] = [
        "Wetland: Robin, Robin",
        "Wetland: Robin, Heron, Robin",
        "Forest: Fox; Wetland: Robin, Heron, Robin; Savanna: Zebra; Reef: Zebra; \
         Tundra: Fox, Arctic Hare",
        "Wetland: Robin, Robin",
        "Forest: Fox; Tundra: Fox, Arctic Hare",
    ];

    fn answers(wildlife: &Wildlife) -> Vec<String> {
        questions(wildlife)
            .iter()
            .map(|(_, filter)| answer(&wildlife.net, filter))
            .collect()
    }

    /// The net after `season` is set to `"Autumn"` and the Forest robin has
    /// migrated and settled.
    fn migrated() -> Wildlife {
        let mut wildlife = build_wildlife().expect("the net builds");
        wildlife
            .net
            .set_param(Season("Autumn".to_owned()))
            .expect("a parameter of the net");
        wildlife
            .net
            .fire(wildlife.migrate)
            .expect("a bird in Forest");
        wildlife.net.fire(wildlife.settle).expect("one on the move");
        wildlife
    }

    fn species(net: &Net, place: PlaceId, token: TokenId) -> &Species {
        net.token::<Species>(place, token).expect("a species")
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
                "Settle",
                "Spread Savanna Reef",
                "Decline Forest",
            ],
        );
        assert!(net.transitions().all(|(_, info)| info.enabled));
        assert_eq!(net.param::<Season>(), Some(&Season("Summer".to_owned())));
        assert_eq!(net.param::<SpreadThreshold>(), Some(&SpreadThreshold(1000)));
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
                seed("Robin", "Bird", "Insectivore", 80),
                seed("Heron", "Bird", "Carnivore", 40),
                seed("Zebra", "Mammal", "Herbivore", 5000),
                seed("Gecko", "Reptile", "Insectivore", 200),
                seed("Clownfish", "Fish", "Omnivore", 300),
                seed("Fox", "Mammal", "Omnivore", 30),
                seed("Arctic Hare", "Mammal", "Herbivore", 90),
            ],
        );

        // Spread and Decline can fire from the start; Migrate waits for
        // autumn (as PROTOCOL's transcript 06 has it).
        assert_eq!(net.fireable(), [wildlife.spread, wildlife.decline]);
        let mut wildlife = wildlife;
        wildlife.net.set_param(Season("Autumn".to_owned())).unwrap();
        assert_eq!(
            wildlife.net.fireable(),
            [wildlife.migrate, wildlife.spread, wildlife.decline]
        );

        // The topology says which keys each place derives.
        let topology = serde_json::to_value(wildlife.net.topology()).unwrap();
        assert_eq!(
            topology["places"][0]["key_names"],
            json!(["species", "class", "diet"])
        );
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
        assert_eq!(seen, 9, "nine tokens, seven species, five habitats");
    }

    #[test]
    fn the_five_questions_on_the_seed() {
        let wildlife = build_wildlife().expect("the net builds");
        assert_eq!(answers(&wildlife), ON_THE_SEED);
    }

    #[test]
    fn after_a_migration_the_forest_robin_is_in_wetland() {
        let wildlife = migrated();
        let birds = wildlife
            .net
            .query(&Filter::new().key_in("class", ["Bird"]))
            .unwrap();
        assert_eq!(birds.len(), 3);
        let wetland = wildlife.at(Habitat::Wetland).id();
        assert!(birds.iter().all(|found| found.place == wetland));
        let settled = birds.last().expect("three birds");
        let robin = species(&wildlife.net, settled.place, settled.token);
        assert_eq!(
            (robin.name.as_str(), robin.population, &robin.destination),
            ("Robin", 120, &None),
            "the Forest robin, its destination cleared"
        );
        assert_eq!(answers(&wildlife)[1], "Wetland: Robin, Heron, Robin");
        assert_eq!(answers(&wildlife)[0], "Wetland: Robin, Robin");
    }

    #[test]
    fn after_a_migration_and_a_spread_the_five_questions_again() {
        let mut wildlife = migrated();
        wildlife
            .net
            .fire(wildlife.spread)
            .expect("the zebra spreads");
        assert_eq!(answers(&wildlife), AFTER_MIGRATION_AND_SPREAD);
        assert_eq!(
            answer(&wildlife.net, &Filter::new().key_in("species", ["Zebra"])),
            "Savanna: Zebra; Reef: Zebra"
        );
        let populations: Vec<u32> = wildlife
            .net
            .query(&Filter::new().key_in("species", ["Zebra"]))
            .unwrap()
            .iter()
            .map(|found| species(&wildlife.net, found.place, found.token).population)
            .collect();
        assert_eq!(
            populations,
            [2500, 2500],
            "halved, the copy taking the rest"
        );
    }

    /// `Migrate` before keyed edges: its one `Fired` consumes both species
    /// in Forest and produces both again, the fox back in Forest under a new
    /// id. This is what a keyed edge will take away: one consumed, one
    /// produced, the fox untouched.
    #[test]
    fn migrate_takes_everything_and_puts_the_rest_back() {
        let mut wildlife = build_wildlife().expect("the net builds");
        let forest = wildlife.at(Habitat::Forest).id();
        let seeded: Vec<TokenId> = wildlife.net.tokens(forest).map(|(id, _)| id).collect();
        let [robin, fox] = seeded[..] else {
            panic!("two species in Forest")
        };
        wildlife.net.set_param(Season("Autumn".to_owned())).unwrap();
        wildlife.net.drain_events().for_each(drop);

        wildlife.net.fire(wildlife.migrate).unwrap();
        let events: Vec<Event> = wildlife.net.drain_events().collect();
        let [
            Event::Fired {
                consumed, produced, ..
            },
        ] = &events[..]
        else {
            panic!("one firing: {events:?}")
        };
        assert_eq!(consumed, &[(forest, robin), (forest, fox)], "all of Forest");
        let [(fox_back, new_fox), (on_the_move, _)] = produced[..] else {
            panic!("two produced: {produced:?}")
        };
        assert_eq!((fox_back, on_the_move), (forest, wildlife.migrating.id()));
        assert_ne!(new_fox, fox, "the fox came back under a new id");
        assert_eq!(species(&wildlife.net, forest, new_fox).name, "Fox");

        wildlife.net.fire(wildlife.settle).unwrap();
        let events: Vec<Event> = wildlife.net.drain_events().collect();
        assert!(
            matches!(&events[..], [Event::Fired { consumed, produced, .. }]
                if consumed.len() == 1
                    && produced.len() == 1
                    && produced[0].0 == wildlife.at(Habitat::Wetland).id()),
            "{events:?}"
        );
    }

    #[test]
    fn distinct_values_in_order_of_first_appearance() {
        let wildlife = build_wildlife().expect("the net builds");
        let net = &wildlife.net;
        assert_eq!(
            net.distinct_values("class", None),
            [Class::Bird, Class::Mammal, Class::Reptile, Class::Fish].map(KeyValue::from)
        );
        let tundra = [wildlife.at(Habitat::Tundra).id()];
        assert_eq!(
            net.distinct_values("diet", Some(&tundra)),
            [Diet::Omnivore, Diet::Herbivore].map(KeyValue::from)
        );
        assert!(net.distinct_values("colour", None).is_empty());
    }

    #[test]
    fn a_decline_keeps_the_keys_and_a_rename_changes_them() {
        let mut wildlife = build_wildlife().expect("the net builds");
        let forest = wildlife.at(Habitat::Forest);
        let (robin, _) = wildlife.net.tokens(forest.id()).next().unwrap();
        let before = wildlife.net.keys(forest.id(), robin).unwrap();

        wildlife.net.fire(wildlife.decline).unwrap();
        assert_eq!(species(&wildlife.net, forest.id(), robin).population, 60);
        assert_eq!(
            wildlife.net.keys(forest.id(), robin).unwrap(),
            before,
            "population is not a key"
        );
        assert_eq!(answers(&wildlife), ON_THE_SEED);

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
            "Tundra: Fox, Arctic Hare",
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
            "Forest: Fox",
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
        wildlife.net.set_param(Season("Autumn".to_owned())).unwrap();
        assert_eq!(answer(&wildlife.net, &class_is_season), "");

        // A season named like a class, to show that the clause reads it.
        wildlife.net.set_param(Season("Bird".to_owned())).unwrap();
        assert_eq!(
            answer(&wildlife.net, &class_is_season),
            "Forest: Robin; Wetland: Robin, Heron"
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
            "Forest: Owl; Wetland: Heron"
        );
    }
}
