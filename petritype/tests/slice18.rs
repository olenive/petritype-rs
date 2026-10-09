//! Integration tests for the eighteenth slice: token keys, queries and the
//! index (DESIGN §8.3).
//!
//! A place declares keys with `NetBuilder::keys`, and the net answers
//! `keys`, `query` and `distinct_values` from them. The answers from the
//! net's index must be the answers a scan of the marking gives, after every
//! kind of change a net goes through, so most of what is tested here is
//! that agreement: each change is followed by the same questions asked of the
//! net (through the index), of a snapshot (which scans) and of the public
//! read API by hand. The wildlife example (`examples/wildlife.rs`) holds the
//! proposal's own questions and answers.

use petritype::drivers::{self, Limits, Mode, OnFailure};
use petritype::{
    BuildError, Command, Filter, KeyValue, Keys, MutateEdge, Net, NetBuilder, NetError, PlaceId,
    PlaceRef, TakeAllEdge, TakeEdge, TokenId,
};

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
struct Parcel {
    kind: String,
    fragile: bool,
    weight: i64,
}

fn parcel(kind: &str, fragile: bool, weight: i64) -> Parcel {
    Parcel {
        kind: kind.to_owned(),
        fragile,
        weight,
    }
}

#[derive(Debug)]
struct Broken;

impl std::fmt::Display for Broken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("dropped")
    }
}

impl std::error::Error for Broken {}

/// A key of each of the three kinds of value.
fn declare_keys(b: &mut NetBuilder, place: PlaceRef<Parcel>) {
    b.keys(place, ["kind", "fragile", "weight"], |p: &Parcel| {
        [p.kind.as_str().into(), p.fragile.into(), p.weight.into()]
    });
}

/// `Inbox` and `Shelf` declare keys; `Bin` does not. `Shelve` takes one,
/// `Clear` takes all, `Weigh` mutates (changing a key), `Drop` fails and
/// restores; every transition is switched off until a test fires it.
struct Depot {
    net: Net,
    inbox: PlaceRef<Parcel>,
    shelf: PlaceRef<Parcel>,
    bin: PlaceRef<Parcel>,
}

fn depot() -> Depot {
    let mut b = NetBuilder::new();
    let inbox = b.place::<Parcel>("Inbox");
    let shelf = b.place::<Parcel>("Shelf");
    let bin = b.place::<Parcel>("Bin");
    declare_keys(&mut b, inbox);
    declare_keys(&mut b, shelf);
    #[cfg(feature = "serde")]
    for place in [inbox, shelf, bin] {
        b.serializable(place);
    }
    b.seed(
        inbox,
        [
            parcel("Book", false, 2),
            parcel("Vase", true, 3),
            parcel("Book", false, 1),
        ],
    );
    b.seed(shelf, [parcel("Lamp", true, 5)]);
    b.seed(bin, [parcel("Book", false, 9)]);
    b.transition("Shelve", (TakeEdge(inbox),), (shelf,), |p: Parcel| (p,));
    b.transition(
        "Clear",
        (TakeAllEdge(shelf),),
        (bin,),
        |all: Vec<Parcel>| (all,),
    );
    b.transition("Weigh", (MutateEdge(shelf),), (), |p: &mut Parcel| {
        p.weight += 10;
    });
    b.transition(
        "Drop",
        (TakeEdge(inbox),),
        (bin,),
        |_: Parcel| -> Result<(Parcel,), Broken> { Err(Broken) },
    )
    .restore_tokens_on_failure();
    let mut net = b.build().expect("the depot builds");
    let all: Vec<_> = net.transitions().map(|(id, _)| id).collect();
    for transition in all {
        net.disable(transition).unwrap();
    }
    Depot {
        net,
        inbox,
        shelf,
        bin,
    }
}

fn fire(net: &mut Net, name: &str) -> Result<(), NetError> {
    let transition = net
        .transition_named(name)
        .expect("a transition of the depot");
    net.enable(transition)?;
    let fired = net.fire(transition);
    net.disable(transition)?;
    fired
}

/// What a filter lets through, worked out by hand from the read API: every
/// token of every place it covers, kept if its keys pass.
fn scanned(net: &Net, filter: &Filter) -> Vec<(PlaceId, TokenId)> {
    net.places()
        .filter(|(place, info)| {
            filter
                .places
                .as_ref()
                .is_none_or(|places| places.contains(place))
                && filter
                    .prefix
                    .as_deref()
                    .is_none_or(|prefix| info.name.starts_with(prefix))
        })
        .flat_map(|(place, _)| net.tokens(place).map(move |(token, _)| (place, token)))
        .filter(|&(place, token)| filter.matches(&net.keys(place, token).unwrap_or_default()))
        .collect()
}

/// The distinct values of a key, worked out by hand.
fn distinct_by_hand(net: &Net, key: &str) -> Vec<KeyValue> {
    let mut seen = Vec::new();
    for (place, _) in net.places() {
        for (token, _) in net.tokens(place) {
            if let Some(value) = net
                .keys(place, token)
                .and_then(|keys| keys.get(key).cloned())
                && !seen.contains(&value)
            {
                seen.push(value);
            }
        }
    }
    seen
}

/// The net's answers (from its index) against a snapshot's (a scan) and
/// against the read API by hand, for a spread of filters and every key.
fn check(net: &Net) {
    let snapshot = net.snapshot().expect("every place clones");
    let places: Vec<PlaceId> = net.places().map(|(id, _)| id).collect();
    let mut filters = vec![
        Filter::new(),
        Filter::new().with_prefix("S"),
        Filter::new().key_in("kind", ["Nothing"]),
        Filter::new().key_in("colour", ["Red"]),
        Filter::new().key_in("fragile", [true]),
        Filter::new().key_in("weight", [KeyValue::Int(2), KeyValue::Str("1".into())]),
        Filter::new()
            .key_in("kind", ["Book", "Vase", "Book"])
            .key_in("fragile", [false]),
        Filter::new().in_places([places[2], places[0]]),
    ];
    for key in ["kind", "fragile", "weight"] {
        for value in distinct_by_hand(net, key) {
            filters.push(Filter::new().key_in(key, [value.clone()]));
            filters.push(
                Filter::new()
                    .key_in(key, [value])
                    .in_places([places[1]])
                    .with_prefix("Sh"),
            );
        }
    }
    for filter in &filters {
        let indexed = net.query(filter).unwrap();
        let found: Vec<(PlaceId, TokenId)> = indexed
            .iter()
            .map(|found| (found.place, found.token))
            .collect();
        assert_eq!(found, scanned(net, filter), "{filter:?}");
        assert_eq!(indexed, snapshot.query(filter).unwrap(), "{filter:?}");
        // The same tokens as ids alone (slice 20), from the index and by a scan.
        assert_eq!(net.query_ids(filter).unwrap(), found, "{filter:?}");
        assert_eq!(snapshot.query_ids(filter).unwrap(), found, "{filter:?}");
        for found in &indexed {
            // Empty for a place without keys, where `keys` says `None`.
            assert_eq!(
                found.keys,
                net.keys(found.place, found.token).unwrap_or_default()
            );
            assert_eq!(
                Some(&found.label),
                net.label(found.place, found.token).as_ref()
            );
            assert!(net.matches(filter, found.place, found.token));
        }
    }
    for key in ["kind", "fragile", "weight", "colour"] {
        assert_eq!(net.distinct_values(key, None), distinct_by_hand(net, key));
        assert_eq!(
            net.distinct_values(key, None),
            snapshot.distinct_values(key, None)
        );
        for place in &places {
            let only = [*place];
            assert_eq!(
                net.distinct_values(key, Some(&only)),
                snapshot.distinct_values(key, Some(&only)),
                "{key} in {place:?}"
            );
        }
    }
}

fn kinds(net: &Net, filter: &Filter) -> Vec<String> {
    net.query(filter)
        .unwrap()
        .iter()
        .map(|found| {
            found
                .keys
                .get("kind")
                .map_or("-".into(), ToString::to_string)
        })
        .collect()
}

// ----- the index after every kind of change --------------------------------------

#[test]
fn the_index_after_every_kind_of_external_change() {
    let Depot {
        mut net,
        inbox,
        shelf,
        bin,
    } = depot();
    check(&net);

    let clock = net.put(inbox, parcel("Clock", true, 4));
    check(&net);
    net.extend(shelf, [parcel("Rug", false, 7), parcel("Clock", true, 4)]);
    check(&net);
    net.replace(inbox, clock, parcel("Watch", true, 1)).unwrap();
    check(&net);
    assert_eq!(
        kinds(&net, &Filter::new().key_in("kind", ["Watch"])),
        ["Watch"]
    );
    net.remove(inbox.id(), clock).unwrap();
    check(&net);
    assert!(kinds(&net, &Filter::new().key_in("kind", ["Watch"])).is_empty());
    net.set_tokens(shelf, [parcel("Desk", false, 30)]);
    check(&net);
    net.set_tokens(bin, []);
    check(&net);
    net.apply(Command::put(shelf, parcel("Book", true, 2)))
        .unwrap();
    check(&net);
    net.apply(Command::set_tokens(inbox, [parcel("Vase", true, 3)]))
        .unwrap();
    check(&net);
}

#[test]
fn the_index_after_every_kind_of_firing() {
    let Depot { mut net, shelf, .. } = depot();

    fire(&mut net, "Shelve").unwrap();
    check(&net);
    assert_eq!(
        kinds(&net, &Filter::new().in_places([shelf.id()])),
        ["Lamp", "Book"]
    );

    // A mutation changes a key, and the index follows it.
    fire(&mut net, "Weigh").unwrap();
    check(&net);
    assert_eq!(kinds(&net, &Filter::new().key_in("weight", [15])), ["Lamp"]);
    assert!(kinds(&net, &Filter::new().key_in("weight", [5])).is_empty());

    fire(&mut net, "Clear").unwrap();
    check(&net);
    assert_eq!(net.count(shelf.id()), 0);

    // A failure that restores puts the token back, keys and all.
    let error = fire(&mut net, "Drop").expect_err("the body fails");
    assert!(matches!(error, NetError::FiringFailed(ref failed) if failed.restored));
    check(&net);

    // The staged path, and the drivers' concurrent loop, which is made of it.
    let shelve = net.transition_named("Shelve").unwrap();
    net.enable(shelve).unwrap();
    let firing = net.launch(shelve).unwrap();
    check(&net);
    net.distribute(firing.run()).unwrap();
    check(&net);
    drivers::run(
        &mut net,
        Mode::Concurrent,
        Limits::default(),
        OnFailure::Stop,
    )
    .unwrap();
    check(&net);
    assert!(kinds(&net, &Filter::new().with_prefix("Inbox")).is_empty());
}

#[cfg(feature = "serde")]
#[test]
fn an_import_rebuilds_the_index() {
    use petritype::Marking;
    use petritype::serde_json::{self, json};

    let Depot { mut net, .. } = depot();
    fire(&mut net, "Shelve").unwrap();
    let saved = net.export_marking().unwrap();
    let marking: Marking = serde_json::from_value(json!({
        "Inbox": [{"kind": "Kite", "fragile": true, "weight": 1}],
        "Shelf": [{"kind": "Book", "fragile": false, "weight": 2}],
    }))
    .unwrap();
    net.import_marking(&marking).unwrap();
    check(&net);
    assert_eq!(
        net.distinct_values("kind", None),
        [KeyValue::from("Kite"), KeyValue::from("Book")]
    );
    assert!(kinds(&net, &Filter::new().key_in("kind", ["Lamp"])).is_empty());

    net.import_marking(&saved).unwrap();
    check(&net);
    assert_eq!(
        kinds(&net, &Filter::new().key_in("kind", ["Lamp"])),
        ["Lamp"]
    );
}

#[test]
fn a_snapshot_keeps_its_own_answers() {
    let Depot { mut net, shelf, .. } = depot();
    let (lamp, _) = net.tokens(shelf.id()).next().unwrap();
    let snapshot = net.snapshot().unwrap();
    fire(&mut net, "Weigh").unwrap();

    let then = snapshot
        .keys(shelf.id(), lamp)
        .expect("Shelf declares keys");
    let now = net.keys(shelf.id(), lamp).unwrap();
    assert_eq!(then.get("weight"), Some(&KeyValue::Int(5)));
    assert_eq!(now.get("weight"), Some(&KeyValue::Int(15)));
    assert_eq!(
        snapshot
            .query(&Filter::new().key_in("weight", [5]))
            .unwrap()
            .len(),
        1
    );
    assert!(snapshot.keys(shelf.id(), TokenId::new(999)).is_none());
}

// ----- answers ---------------------------------------------------------------

#[test]
fn distinct_values_come_in_order_of_first_appearance() {
    let Depot { net, inbox, .. } = depot();
    assert_eq!(
        net.distinct_values("kind", None),
        [KeyValue::from("Book"), "Vase".into(), "Lamp".into()],
        "Inbox first, then Shelf; Bin declares no keys"
    );
    assert_eq!(
        net.distinct_values("fragile", None),
        [KeyValue::Bool(false), KeyValue::Bool(true)]
    );
    assert_eq!(
        net.distinct_values("weight", Some(&[inbox.id()])),
        [KeyValue::Int(2), KeyValue::Int(3), KeyValue::Int(1)]
    );
    assert!(net.distinct_values("colour", None).is_empty());
    assert!(
        net.distinct_values("kind", Some(&[PlaceId::new(99)]))
            .is_empty(),
        "a place that is not there contributes nothing"
    );
}

#[test]
fn a_place_without_keys_matches_no_clause_and_every_empty_filter() {
    let Depot { net, bin, .. } = depot();
    let (book, _) = net.tokens(bin.id()).next().unwrap();
    assert_eq!(net.keys(bin.id(), book), None, "Bin declares no keys");
    let info = net.places().nth(2).unwrap().1;
    assert!(info.key_names.is_empty());

    let in_the_bin = Filter::new().in_places([bin.id()]);
    let found = net.query(&in_the_bin).unwrap();
    assert_eq!(found.len(), 1, "no clauses: the token is kept");
    assert!(found[0].keys.is_empty());
    assert!(net.matches(&in_the_bin, bin.id(), book));

    let books = in_the_bin.key_in("kind", ["Book"]);
    assert!(net.query(&books).unwrap().is_empty(), "a clause: never");
    assert!(!net.matches(&books, bin.id(), book));

    let all = net.query(&Filter::new()).unwrap();
    assert_eq!(all.len(), 5, "the empty filter keeps every token");
}

#[test]
fn a_query_names_its_places_or_fails() {
    let Depot { net, inbox, .. } = depot();
    let nowhere = PlaceId::new(42);
    let error = net
        .query(&Filter::new().in_places([inbox.id(), nowhere]))
        .expect_err("no such place");
    assert!(matches!(error, NetError::NoSuchPlace { place } if place == nowhere));
    assert!(!net.matches(&Filter::new(), nowhere, TokenId::new(0)));
    assert!(!net.matches(&Filter::new(), inbox.id(), TokenId::new(999)));
}

#[test]
fn matches_follow_place_order_then_token_order() {
    let Depot {
        mut net,
        inbox,
        shelf,
        ..
    } = depot();
    net.put(shelf, parcel("Book", true, 8));
    net.put(inbox, parcel("Book", true, 6));
    let found = net
        .query(
            &Filter::new()
                .key_in("kind", ["Book"])
                .key_in("fragile", [true]),
        )
        .unwrap();
    let weights: Vec<(PlaceId, Option<&KeyValue>)> = found
        .iter()
        .map(|found| (found.place, found.keys.get("weight")))
        .collect();
    assert_eq!(
        weights,
        [
            (inbox.id(), Some(&KeyValue::Int(6))),
            (shelf.id(), Some(&KeyValue::Int(8)))
        ]
    );
}

// ----- declaring keys ----------------------------------------------------------

#[test]
fn a_name_declared_twice_is_refused() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<Parcel>("Inbox");
    b.keys(inbox, ["kind", "weight", "kind"], |p: &Parcel| {
        [p.kind.as_str().into(), p.weight.into(), p.fragile.into()]
    });
    let error = b.build().expect_err("kind twice");
    assert!(
        matches!(&error, BuildError::DuplicateKeyName { place, name } if place == "Inbox" && name == "kind"),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "place \"Inbox\" declares the key \"kind\" more than once"
    );
}

#[test]
fn keys_declared_twice_on_one_place_are_refused() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<Parcel>("Inbox");
    declare_keys(&mut b, inbox);
    b.keys(inbox, ["kind"], |p: &Parcel| [p.kind.as_str().into()]);
    let error = b.build().expect_err("two declarations");
    assert!(
        matches!(&error, BuildError::KeysAlreadyDeclared { place } if place == "Inbox"),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "place \"Inbox\" declares its keys more than once"
    );
}

#[test]
fn key_names_are_in_the_read_api_and_the_topology() {
    let Depot { net, .. } = depot();
    let names: Vec<&[&str]> = net.places().map(|(_, info)| info.key_names).collect();
    let keyed: &[&str] = &["kind", "fragile", "weight"];
    assert_eq!(names, [keyed, keyed, &[]]);
    let topology = net.topology();
    assert_eq!(topology.places[0].key_names, keyed);
    assert!(topology.places[2].key_names.is_empty());
    assert_eq!(net.snapshot().unwrap().topology(), topology);
}

#[cfg(feature = "serde")]
#[test]
fn key_names_are_written_only_where_there_are_some() {
    use petritype::serde_json::{self, json};

    let Depot { net, .. } = depot();
    let topology = serde_json::to_value(net.topology()).unwrap();
    assert_eq!(
        topology["places"][0]["key_names"],
        json!(["kind", "fragile", "weight"])
    );
    assert!(
        topology["places"][2].get("key_names").is_none(),
        "a place without keys reads as it always did"
    );
}

// ----- values and keys -----------------------------------------------------------

#[test]
fn values_compare_by_kind_and_value() {
    assert_ne!(KeyValue::Int(1), KeyValue::Str("1".into()));
    assert_ne!(KeyValue::Str("1".into()), KeyValue::Bool(true));
    assert_ne!(KeyValue::Int(1), KeyValue::Bool(true));
    assert_eq!(KeyValue::from(1), KeyValue::Int(1));
    assert_eq!(KeyValue::from("a"), KeyValue::from(String::from("a")));
    assert_eq!(KeyValue::from(false), KeyValue::Bool(false));
    assert_eq!(
        [KeyValue::Int(1), "1".into(), true.into()].map(|value| value.to_string()),
        ["1", "1", "true"]
    );

    // A filter on the integer finds the integer and nothing else.
    let mut b = NetBuilder::new();
    let values = b.place::<KeyValue>("Values");
    b.keys(values, ["value"], |value: &KeyValue| [value.clone()]);
    b.seed(values, [KeyValue::Int(1), "1".into(), true.into()]);
    let net = b.build().unwrap();
    for value in [KeyValue::Int(1), "1".into(), true.into()] {
        let found = net
            .query(&Filter::new().key_in("value", [value.clone()]))
            .unwrap();
        assert_eq!(found.len(), 1, "{value:?}");
        assert_eq!(found[0].keys.get("value"), Some(&value));
    }
}

#[cfg(feature = "serde")]
#[test]
fn a_value_is_a_bare_json_string_integer_or_boolean() {
    use petritype::serde_json::{self, json};

    let values: Vec<KeyValue> = serde_json::from_value(json!([1, "1", true])).unwrap();
    assert_eq!(values, [KeyValue::Int(1), "1".into(), true.into()]);
    assert_eq!(
        serde_json::to_value(&values).unwrap(),
        json!([1, "1", true])
    );
    assert!(serde_json::from_value::<KeyValue>(json!(1.5)).is_err());
    assert!(serde_json::from_value::<KeyValue>(json!(null)).is_err());

    let keys: Keys = [("kind", KeyValue::from("Book")), ("weight", 2.into())]
        .into_iter()
        .collect();
    assert_eq!(
        serde_json::to_string(&keys).unwrap(),
        r#"{"kind":"Book","weight":2}"#,
        "an object, in declaration order"
    );
}

#[test]
fn keys_read_by_name_and_in_order() {
    let Depot { net, inbox, .. } = depot();
    let (book, _) = net.tokens(inbox.id()).next().unwrap();
    let keys = net.keys(inbox.id(), book).unwrap();
    assert_eq!(keys.get("kind"), Some(&KeyValue::from("Book")));
    assert_eq!(keys.get("fragile"), Some(&KeyValue::Bool(false)));
    assert_eq!(keys.get("weight"), Some(&KeyValue::Int(2)));
    assert_eq!(keys.get("colour"), None);
    assert_eq!(keys.len(), 3);
    assert!(!keys.is_empty());
    assert_eq!(
        keys.names().collect::<Vec<_>>(),
        ["kind", "fragile", "weight"]
    );
    assert_eq!(
        keys.iter().map(|(name, _)| name).collect::<Vec<_>>(),
        ["kind", "fragile", "weight"]
    );
    assert!(Keys::default().is_empty());
    assert_eq!(net.keys(inbox.id(), TokenId::new(999)), None);
    assert_eq!(net.keys(PlaceId::new(42), book), None);
}

/// Without the `serde` feature there are no wire names, so a clause on a
/// parameter accepts nothing; with it, it reads the parameter by its name.
#[test]
fn a_clause_on_a_parameter() {
    #[derive(Clone)]
    #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
    struct Wanted(String);

    let mut b = NetBuilder::new();
    let inbox = b.place::<Parcel>("Inbox");
    declare_keys(&mut b, inbox);
    b.seed(inbox, [parcel("Book", false, 2), parcel("Vase", true, 3)]);
    b.param(Wanted("Vase".into()));
    #[cfg(feature = "serde")]
    b.serializable_param::<Wanted>("wanted");
    let mut net = b.build().unwrap();

    let wanted = Filter::new().key_is_param("kind", "wanted");
    let expected: &[&str] = if cfg!(feature = "serde") {
        &["Vase"]
    } else {
        &[]
    };
    assert_eq!(kinds(&net, &wanted), expected);
    net.set_param(Wanted("Book".into())).unwrap();
    assert_eq!(
        net.param::<Wanted>().map(|wanted| wanted.0.as_str()),
        Some("Book")
    );
    let expected: &[&str] = if cfg!(feature = "serde") {
        &["Book"]
    } else {
        &[]
    };
    assert_eq!(kinds(&net, &wanted), expected);
    let (book, _) = net.tokens(inbox.id()).next().unwrap();
    assert_eq!(
        net.matches(&wanted, inbox.id(), book),
        cfg!(feature = "serde")
    );
    assert!(
        !wanted.matches(&net.keys(inbox.id(), book).unwrap()),
        "a filter alone knows no parameters"
    );
}
