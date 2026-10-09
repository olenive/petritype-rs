//! Integration tests for the twentieth slice: the runner's half of token
//! keys (PROTOCOL.md §13, DESIGN §19.3.12).
//!
//! The runner is tested in its own crate (`petritype-runner/tests`, with the
//! shared transcript `06_keys_query_filter.json`). What the core gained for
//! it is [`Net::query_ids`]: the tokens a filter keeps, as `query` finds
//! them, but as ids alone, so that a runner projecting the net through a
//! client's filter can count, page and remember them without rendering a
//! label or a JSON value it will not write. `tests/slice18.rs` checks it
//! against `query` after every kind of change; these are its own promises.

use petritype::{Filter, KeyValue, Net, NetBuilder, NetError, PlaceId, PlaceRef, TokenId};

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
struct Species {
    name: String,
    class: String,
    legs: i64,
}

fn species(name: &str, class: &str, legs: i64) -> Species {
    Species {
        name: name.to_owned(),
        class: class.to_owned(),
        legs,
    }
}

/// `Forest` and `Wetland` declare keys, `Notes` holds strings and declares
/// none.
struct Habitats {
    net: Net,
    forest: PlaceRef<Species>,
    wetland: PlaceRef<Species>,
    notes: PlaceRef<String>,
}

fn habitats() -> Habitats {
    let mut b = NetBuilder::new();
    let forest = b.place::<Species>("Forest");
    let wetland = b.place::<Species>("Wetland");
    let notes = b.place::<String>("Notes");
    for place in [forest, wetland] {
        b.keys(place, ["species", "class", "legs"], |s: &Species| {
            [
                s.name.as_str().into(),
                s.class.as_str().into(),
                s.legs.into(),
            ]
        });
    }
    b.seed(
        forest,
        [
            species("Robin", "Bird", 2),
            species("Fox", "Mammal", 4),
            species("Owl", "Bird", 2),
        ],
    );
    b.seed(
        wetland,
        [species("Heron", "Bird", 2), species("Frog", "Amphibian", 4)],
    );
    b.seed(notes, ["counted in spring".to_owned()]);
    Habitats {
        net: b.build().unwrap(),
        forest,
        wetland,
        notes,
    }
}

/// What `query` finds, as `(place, token)` pairs.
fn queried(net: &Net, filter: &Filter) -> Vec<(PlaceId, TokenId)> {
    net.query(filter)
        .unwrap()
        .iter()
        .map(|found| (found.place, found.token))
        .collect()
}

#[test]
fn query_ids_are_the_tokens_query_finds_in_its_order() {
    let Habitats {
        net,
        forest,
        wetland,
        notes,
    } = habitats();
    for filter in [
        Filter::new(),
        Filter::new().key_in("class", ["Bird"]),
        Filter::new().key_in("legs", [4]),
        Filter::new()
            .key_in("class", ["Bird", "Mammal"])
            .key_in("legs", [2]),
        Filter::new().key_in("colour", ["Red"]),
        Filter::new().in_places([wetland.id(), notes.id()]),
        Filter::new().with_prefix("W"),
        Filter::new()
            .key_in("class", ["Bird"])
            .in_places([forest.id()]),
    ] {
        assert_eq!(
            net.query_ids(&filter).unwrap(),
            queried(&net, &filter),
            "{filter:?}"
        );
    }
    let birds = net
        .query_ids(&Filter::new().key_in("class", ["Bird"]))
        .unwrap();
    let ids = |place: PlaceRef<Species>| -> Vec<TokenId> {
        net.tokens(place.id()).map(|(token, _)| token).collect()
    };
    let (forest_ids, wetland_ids) = (ids(forest), ids(wetland));
    assert_eq!(
        birds,
        [
            (forest.id(), forest_ids[0]),
            (forest.id(), forest_ids[2]),
            (wetland.id(), wetland_ids[0]),
        ],
        "place order, then token order"
    );
    assert_eq!(
        net.query_ids(&Filter::new()).unwrap().len(),
        6,
        "no clauses: every token, the keyless place's too"
    );
}

#[test]
fn query_ids_follow_a_change_of_keys() {
    let Habitats {
        mut net, forest, ..
    } = habitats();
    let robin = net.tokens(forest.id()).next().unwrap().0;
    let mammals = Filter::new().key_in("class", ["Mammal"]);
    assert_eq!(net.query_ids(&mammals).unwrap().len(), 1);
    net.replace(forest, robin, species("Robin", "Mammal", 2))
        .unwrap();
    assert_eq!(
        net.query_ids(&mammals).unwrap(),
        [
            (forest.id(), robin),
            (forest.id(), net.tokens(forest.id()).nth(1).unwrap().0)
        ]
    );
    net.remove(forest.id(), robin).unwrap();
    assert_eq!(net.query_ids(&mammals).unwrap().len(), 1);
    assert_eq!(net.query_ids(&mammals).unwrap(), queried(&net, &mammals));
}

#[test]
fn a_snapshot_gives_the_same_ids() {
    let Habitats { net, wetland, .. } = habitats();
    let snapshot = net.snapshot().expect("every place clones");
    for filter in [
        Filter::new(),
        Filter::new().key_in("legs", [2, 4]),
        Filter::new()
            .key_in("species", ["Frog"])
            .in_places([wetland.id()]),
        Filter::new().key_in("legs", [KeyValue::Str("2".into())]),
    ] {
        assert_eq!(
            snapshot.query_ids(&filter).unwrap(),
            net.query_ids(&filter).unwrap(),
            "{filter:?}"
        );
    }
}

#[test]
fn query_ids_name_their_places_or_fail() {
    let Habitats { net, forest, .. } = habitats();
    let nowhere = PlaceId::new(9);
    let filter = Filter::new().in_places([forest.id(), nowhere]);
    let error = net.query_ids(&filter).expect_err("no such place");
    assert!(matches!(error, NetError::NoSuchPlace { place } if place == nowhere));
    let snapshot = net.snapshot().unwrap();
    assert!(matches!(
        snapshot.query_ids(&filter),
        Err(NetError::NoSuchPlace { place }) if place == nowhere
    ));
}

#[cfg(feature = "serde")]
#[test]
fn a_clause_on_a_parameter_reads_it_now() {
    #[derive(Clone, serde::Serialize, serde::Deserialize)]
    struct Wanted(String);

    let mut b = NetBuilder::new();
    let forest = b.place::<Species>("Forest");
    b.keys(forest, ["class"], |s: &Species| [s.class.as_str().into()]);
    b.seed(
        forest,
        [species("Robin", "Bird", 2), species("Fox", "Mammal", 4)],
    );
    b.param(Wanted("Bird".to_owned()));
    b.serializable_param::<Wanted>("wanted");
    let mut net = b.build().unwrap();
    let fox = net.tokens(forest.id()).nth(1).unwrap().0;
    let wanted = Filter::new().key_is_param("class", "wanted");
    assert_eq!(net.query_ids(&wanted).unwrap().len(), 1);
    net.set_param(Wanted("Mammal".to_owned())).unwrap();
    assert_eq!(net.query_ids(&wanted).unwrap(), [(forest.id(), fox)]);
}
