//! Look a key up in a cache, and fall back to the database on a miss.
//!
//! A port of the Python `examples/caching/hypothetical_caching.py` and the
//! net that `examples/caching/execution_subgraph.py` builds around it.
//!
//! The Python `CacheOperations.retrieve_key_value_pair` returns
//! `Union[DBKey, DBKeyValuePair]` and lets reflection route the answer. The
//! Rust form of that is a [`#[derive(Routed)]`](petritype::Routed) enum: a
//! [`Hit`] goes to one place and a [`Miss`] to another, resolved from the
//! types when the transition is declared, so both routes are wired before
//! anything runs. The other two Python `kwargs` become the two Rust forms of
//! "a value a transition needs that is not a token": the database never
//! changes, so it is a [`Param`]; the cache does, so it is a place, read with
//! a [`SnapshotEdge`] and written through a [`MutateEdge`].

use std::collections::BTreeMap;

use petritype::drivers::{self, Limits, Mode, OnFailure};
use petritype::{
    BuildError, MutateEdge, Net, NetBuilder, Param, PlaceRef, Routed, SnapshotEdge, TakeEdge,
};

/// A key to look up.
#[derive(Debug, Clone, PartialEq)]
struct Key(String);

/// The key was in the cache, with the value that was there.
#[derive(Debug, Clone, PartialEq)]
struct Hit {
    key: String,
    value: String,
}

/// The key was not in the cache.
#[derive(Debug, Clone, PartialEq)]
struct Miss {
    key: String,
}

/// The value came back from the database and has not been cached yet.
#[derive(Debug, Clone, PartialEq)]
struct Fetched {
    key: String,
    value: String,
}

/// The answer, however it was arrived at.
#[derive(Debug, Clone, PartialEq)]
struct Resolved {
    key: String,
    value: String,
}

/// What `CheckCache` returns: the union the Python version routed by
/// reflection, written as the two places it can go to.
#[derive(Routed)]
enum Lookup {
    Hit(Hit),
    Miss(Miss),
}

/// The cache: a token of its own, because it changes as the net runs.
#[derive(Debug, Clone, PartialEq)]
struct Cache(BTreeMap<String, String>);

/// The database: a parameter, because it does not.
#[derive(Clone)]
struct Database(BTreeMap<String, String>);

/// The net and the handles the example reads back.
struct Lookups {
    net: Net,
    cache: PlaceRef<Cache>,
    resolved: PlaceRef<Resolved>,
}

/// A map from `[(key, value)]`, so the fixtures read as data.
fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

/// The caching net, seeded with three keys, one of which is already cached,
/// and cached *stale*, so that a hit is visibly not a fetch.
fn build() -> Result<Lookups, BuildError> {
    let mut b = NetBuilder::new();
    let key_input = b.place::<Key>("KeyInput");
    let cache = b.place::<Cache>("Cache");
    let cached_value_found = b.place::<Hit>("CachedValueFound");
    let key_for_db_retrieval = b.place::<Miss>("KeyForDBRetrieval");
    let db_value_retrieved = b.place::<Fetched>("DBValueRetrieved");
    let resolved = b.place::<Resolved>("CacheRetrievalOutput");

    b.seed(key_input, ["a_0", "c_0", "e_0"].map(|k| Key(k.to_owned())));
    b.seed(cache, [Cache(map(&[("c_0", "C_1")]))]);
    b.param(Database(map(&[
        ("a_0", "A_10"),
        ("b_0", "B_11"),
        ("c_0", "C_12"),
        ("e_0", "E_13"),
    ])));

    b.transition(
        "CheckCache",
        (TakeEdge(key_input), SnapshotEdge(cache)),
        (cached_value_found, key_for_db_retrieval),
        |key: Key, cache: Cache| match cache.0.get(&key.0) {
            Some(value) => Lookup::Hit(Hit {
                key: key.0,
                value: value.clone(),
            }),
            None => Lookup::Miss(Miss { key: key.0 }),
        },
    );
    b.transition(
        "RetrieveFromDB",
        (TakeEdge(key_for_db_retrieval), Param::<Database>::new()),
        (db_value_retrieved,),
        |miss: Miss, db: Database| {
            let value = db.0.get(&miss.key).cloned().unwrap_or_default();
            (Fetched {
                key: miss.key,
                value,
            },)
        },
    );
    b.transition(
        "CacheKeyValuePair",
        (TakeEdge(db_value_retrieved), MutateEdge(cache)),
        (resolved,),
        |fetched: Fetched, cache: &mut Cache| {
            cache.0.insert(fetched.key.clone(), fetched.value.clone());
            (Resolved {
                key: fetched.key,
                value: fetched.value,
            },)
        },
    );
    b.transition(
        "UseCachedValue",
        (TakeEdge(cached_value_found),),
        (resolved,),
        |hit: Hit| {
            (Resolved {
                key: hit.key,
                value: hit.value,
            },)
        },
    );

    Ok(Lookups {
        net: b.build()?,
        cache,
        resolved,
    })
}

/// The resolved pairs, sorted, so the assertions do not depend on which route
/// finished first.
fn answers(net: &Net, resolved: PlaceRef<Resolved>) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = net
        .tokens(resolved.id())
        .filter_map(|(_, value)| value.downcast_ref::<Resolved>())
        .map(|r| (r.key.clone(), r.value.clone()))
        .collect();
    found.sort();
    found
}

/// The cache as it stands.
fn cached(net: &Net, cache: PlaceRef<Cache>) -> BTreeMap<String, String> {
    net.tokens(cache.id())
        .filter_map(|(_, value)| value.downcast_ref::<Cache>())
        .map(|cache| cache.0.clone())
        .next()
        .unwrap_or_default()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut lookups = build()?;
    drivers::run(
        &mut lookups.net,
        Mode::Sequential,
        Limits::default(),
        OnFailure::Stop,
    )?;

    for (key, value) in answers(&lookups.net, lookups.resolved) {
        println!("{key} -> {value}");
    }
    println!(
        "cache afterwards: {:?}",
        cached(&lookups.net, lookups.cache)
    );
    println!("{}", petritype_dot::dot(&lookups.net));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{answers, build, cached, map};
    use petritype::drivers::{self, Limits, Mode, OnFailure};

    /// Builds the net and runs it to quiescence.
    fn run() -> super::Lookups {
        let mut lookups = build().expect("the net builds");
        let summary = drivers::run(
            &mut lookups.net,
            Mode::Sequential,
            Limits::default(),
            OnFailure::Stop,
        )
        .expect("nothing fails");
        assert!(summary.quiesced);
        // Three cache checks, two fetches, two caches, one hit used.
        assert_eq!(summary.fired_count, 8);
        lookups
    }

    #[test]
    fn both_routes_are_taken() {
        let lookups = run();

        // `c_0` was cached stale, so a hit gives `C_1` where a fetch would
        // have given the database's `C_12`: that is the hit route. The other
        // two hold database values, which is the miss route.
        assert_eq!(
            answers(&lookups.net, lookups.resolved),
            [
                (String::from("a_0"), String::from("A_10")),
                (String::from("c_0"), String::from("C_1")),
                (String::from("e_0"), String::from("E_13")),
            ],
        );
    }

    #[test]
    fn the_misses_are_cached_on_the_way_through() {
        let lookups = run();

        assert_eq!(
            cached(&lookups.net, lookups.cache),
            map(&[("a_0", "A_10"), ("c_0", "C_1"), ("e_0", "E_13")]),
        );
    }

    #[test]
    fn the_cache_is_read_and_written_by_two_different_edges() {
        let lookups = build().expect("the net builds");
        let text = petritype_dot::dot(&lookups.net);

        assert_eq!(
            text.matches("style=dashed, color=dimgray, arrowhead=normal")
                .count(),
            1,
            "CheckCache reads the cache without consuming it",
        );
        assert_eq!(
            text.matches("dir=both, arrowhead=dot, arrowtail=dot")
                .count(),
            1,
            "CacheKeyValuePair writes it in place",
        );
        assert!(
            text.contains(r#"label="RetrieveFromDB\n[Database]""#),
            "the parameter is named on the box, never printed",
        );
    }
}
