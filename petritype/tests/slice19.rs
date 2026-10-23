//! Integration tests for the nineteenth slice: keyed input edges (DESIGN
//! §8.3, "Keyed input edges").
//!
//! A `TakeWhereEdge` takes, and a `MutateWhereEdge` hands over, the first
//! token in queue order whose keys a filter accepts, leaving every other
//! token where it is with its id. What is tested here is that promise
//! (fireability, the token chosen, the events, the restore, the index kept
//! right) and the edges' place in everything else that reads edges: the
//! build checks, the read API and its serialised form, detachability, the
//! two opt-in checks and the drivers. The wildlife example
//! (`examples/wildlife.rs`) holds the proposal's own acceptance test.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use petritype::drivers::{self, Limits, Mode, OnFailure};
use petritype::{
    BuildError, EdgeInfo, EdgeKind, Event, Filter, MutateWhereEdge, Net, NetBuilder, NetError,
    PlaceRef, SnapshotEdge, TakeEdge, TakeWhereEdge, TokenId, TransitionId,
};

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
struct Animal {
    name: String,
    class: String,
    legs: i64,
}

fn animal(name: &str, class: &str, legs: i64) -> Animal {
    Animal {
        name: name.to_owned(),
        class: class.to_owned(),
        legs,
    }
}

#[derive(Debug)]
struct Broken;

impl std::fmt::Display for Broken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("dropped it")
    }
}

impl std::error::Error for Broken {}

/// `name`, `class` and `legs`: a key of each kind of value but booleans.
fn declare_keys(b: &mut NetBuilder, place: PlaceRef<Animal>) {
    b.keys(place, ["name", "class", "legs"], |a: &Animal| {
        [
            a.name.as_str().into(),
            a.class.as_str().into(),
            a.legs.into(),
        ]
    });
}

fn birds() -> Filter {
    Filter::new().key_in("class", ["Bird"])
}

fn mammals() -> Filter {
    Filter::new().key_in("class", ["Mammal"])
}

/// A barn with keys and a yard without, the barn seeded with these animals.
struct Farm {
    b: NetBuilder,
    barn: PlaceRef<Animal>,
    yard: PlaceRef<Animal>,
}

fn farm(seed: &[(&str, &str, i64)]) -> Farm {
    let mut b = NetBuilder::new();
    let barn = b.place::<Animal>("Barn");
    let yard = b.place::<Animal>("Yard");
    declare_keys(&mut b, barn);
    b.seed(
        barn,
        seed.iter()
            .map(|&(name, class, legs)| animal(name, class, legs)),
    );
    Farm { b, barn, yard }
}

/// The ids of a place's tokens, in queue order.
fn ids(net: &Net, place: PlaceRef<Animal>) -> Vec<TokenId> {
    net.tokens(place.id()).map(|(id, _)| id).collect()
}

/// The names of a place's animals, in queue order.
fn names(net: &Net, place: PlaceRef<Animal>) -> Vec<String> {
    net.tokens(place.id())
        .map(|(id, _)| net.token::<Animal>(place.id(), id).unwrap().name.clone())
        .collect()
}

/// The id of the animal of this name in a place.
fn id_of(net: &Net, place: PlaceRef<Animal>, name: &str) -> TokenId {
    net.tokens(place.id())
        .find(|(id, _)| net.token::<Animal>(place.id(), *id).unwrap().name == name)
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("no {name} in {place:?}"))
}

/// The net's indexed answers against a snapshot's scanned ones, for the
/// questions a keyed edge asks and a few more.
fn index_agrees(net: &Net) {
    let snapshot = net.snapshot().expect("every place clones");
    let mut filters = vec![birds(), mammals(), Filter::new()];
    for key in ["name", "class", "legs"] {
        for value in net.distinct_values(key, None) {
            filters.push(Filter::new().key_in(key, [value]));
        }
    }
    for filter in &filters {
        assert_eq!(
            net.query(filter).unwrap(),
            snapshot.query(filter).unwrap(),
            "{filter:?}"
        );
    }
    for key in ["name", "class", "legs"] {
        assert_eq!(
            net.distinct_values(key, None),
            snapshot.distinct_values(key, None),
            "{key}"
        );
    }
}

// ----- fireability --------------------------------------------------------

#[test]
fn a_keyed_edge_is_fireable_only_while_its_place_holds_a_match() {
    let Farm { mut b, barn, yard } = farm(&[("Cow", "Mammal", 4)]);
    let pick = b
        .transition(
            "Pick",
            (TakeWhereEdge(barn, birds()),),
            (yard,),
            |a: Animal| (a,),
        )
        .id();
    let mut net = b.build().unwrap();

    assert_eq!(net.count(barn.id()), 1, "not empty");
    assert!(!net.is_fireable(pick), "but no bird");
    assert!(net.fireable().is_empty());
    assert!(matches!(
        net.fire(pick),
        Err(NetError::NotFireable { transition, .. }) if transition == pick
    ));
    assert!(matches!(
        net.launch(pick),
        Err(NetError::NotFireable { .. })
    ));
    assert_eq!(net.step().unwrap(), None);

    net.put(barn, animal("Hen", "Bird", 2));
    assert_eq!(net.fireable(), [pick]);
    assert_eq!(net.step().unwrap(), Some(pick));
    assert!(!net.is_fireable(pick), "the one bird is gone");
    assert_eq!(names(&net, barn), ["Cow"]);
    assert_eq!(names(&net, yard), ["Hen"]);
}

/// The guard is asked after the filter, and only when it has a token to
/// look at; it can read that token with `first_match`.
#[test]
fn the_guard_is_asked_after_the_filter_and_can_read_the_chosen_token() {
    let Farm { mut b, barn, yard } = farm(&[("Cow", "Mammal", 4), ("Emu", "Bird", 2)]);
    let asked = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&asked);
    let pick = b
        .transition(
            "Pick",
            (TakeWhereEdge(barn, birds()),),
            (yard,),
            |a: Animal| (a,),
        )
        .guard(move |net| {
            counter.fetch_add(1, Ordering::SeqCst);
            // Only a bird with a short name may go.
            let chosen = net
                .first_match(barn.id(), &birds())
                .expect("asked only when a bird is there");
            net.token::<Animal>(barn.id(), chosen).unwrap().name.len() <= 3
        })
        .id();
    let mut net = b.build().unwrap();

    assert!(net.is_fireable(pick));
    assert_eq!(asked.load(Ordering::SeqCst), 1);
    net.fire(pick).unwrap();
    let asked_then = asked.load(Ordering::SeqCst);
    assert!(!net.is_fireable(pick));
    assert_eq!(
        asked.load(Ordering::SeqCst),
        asked_then,
        "no bird, so the guard is not asked"
    );

    net.put(barn, animal("Ostrich", "Bird", 2));
    assert!(!net.is_fireable(pick), "a bird, but the guard says no");
    assert!(asked.load(Ordering::SeqCst) > asked_then);
}

// ----- the token chosen, and the events -------------------------------------

#[test]
fn the_oldest_match_is_taken_not_the_front_and_nothing_else_moves() {
    let Farm { mut b, barn, yard } = farm(&[
        ("Cow", "Mammal", 4),
        ("Hen", "Bird", 2),
        ("Pig", "Mammal", 4),
        ("Duck", "Bird", 2),
    ]);
    let pick = b
        .transition(
            "Pick",
            (TakeWhereEdge(barn, birds()),),
            (yard,),
            |a: Animal| (a,),
        )
        .id();
    let mut net = b.build().unwrap();
    let [cow, hen, pig, duck] = ids(&net, barn)[..] else {
        panic!("four animals")
    };
    assert_eq!(net.first_match(barn.id(), &birds()), Some(hen));

    net.fire(pick).unwrap();
    let events: Vec<Event> = net.drain_events().collect();
    let [
        Event::Fired {
            consumed, produced, ..
        },
    ] = &events[..]
    else {
        panic!("one Fired and nothing else, no Removed or Added: {events:?}")
    };
    assert_eq!(consumed, &[(barn.id(), hen)]);
    assert_eq!(produced.len(), 1);
    assert_eq!(
        ids(&net, barn),
        [cow, pig, duck],
        "the rest keep ids and order"
    );
    index_agrees(&net);

    // The staged path names the same one token at launch.
    let firing = net.launch(pick).unwrap();
    assert!(firing.is_pending(), "a taking edge is detachable");
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Launched {
            transition: pick,
            consumed: vec![(barn.id(), duck)],
        }]
    );
    net.distribute(firing.run()).unwrap();
    assert_eq!(ids(&net, barn), [cow, pig]);
    assert_eq!(names(&net, yard), ["Hen", "Duck"]);
    assert!(!net.is_fireable(pick));
    index_agrees(&net);
}

/// A filter with no clauses accepts every token, so the edge takes the
/// front one, as a `TakeEdge` would; a clause on a key the place does not
/// declare accepts none, so the edge never fires (and is no build error:
/// the place has keys).
#[test]
fn no_clauses_take_the_front_and_an_unknown_key_takes_nothing() {
    let Farm { mut b, barn, yard } = farm(&[("Cow", "Mammal", 4), ("Hen", "Bird", 2)]);
    let any = b
        .transition(
            "Any",
            (TakeWhereEdge(barn, Filter::new()),),
            (yard,),
            |a: Animal| (a,),
        )
        .id();
    let colour = b
        .transition(
            "Brown",
            (TakeWhereEdge(
                barn,
                Filter::new().key_in("colour", ["Brown"]),
            ),),
            (yard,),
            |a: Animal| (a,),
        )
        .id();
    let mut net = b.build().unwrap();
    assert!(!net.is_fireable(colour));
    net.fire(any).unwrap();
    assert_eq!(names(&net, yard), ["Cow"]);
    assert!(!net.is_fireable(colour));
}

/// An edge reads its one place: the filter's `places` and `prefix` are not
/// consulted, by the edge or by `first_match`.
#[test]
fn an_edge_ignores_its_filters_places_and_prefix() {
    let Farm { mut b, barn, yard } = farm(&[("Hen", "Bird", 2)]);
    let elsewhere = birds().in_places([yard.id()]).with_prefix("Nowhere");
    let pick = b
        .transition(
            "Pick",
            (TakeWhereEdge(barn, elsewhere.clone()),),
            (yard,),
            |a: Animal| (a,),
        )
        .id();
    let mut net = b.build().unwrap();
    let hen = ids(&net, barn)[0];
    assert_eq!(net.first_match(barn.id(), &elsewhere), Some(hen));
    assert!(
        net.query(&elsewhere).unwrap().is_empty(),
        "a query does consult them"
    );
    net.fire(pick).unwrap();
    assert_eq!(names(&net, yard), ["Hen"]);
}

// ----- mutating ---------------------------------------------------------------

#[test]
fn a_mutate_where_edge_changes_the_chosen_token_where_it_stands() {
    let Farm { mut b, barn, .. } = farm(&[
        ("Hen", "Bird", 2),
        ("Cow", "Mammal", 4),
        ("Pig", "Mammal", 4),
    ]);
    let four_legged = mammals().key_in("legs", [4]);
    let trip = b
        .transition(
            "Trip",
            (MutateWhereEdge(barn, four_legged.clone()),),
            (),
            |a: &mut Animal| a.legs -= 1,
        )
        .id();
    let mut net = b.build().unwrap();
    let [hen, cow, pig] = ids(&net, barn)[..] else {
        panic!("three animals")
    };
    let info = net.transitions().next().unwrap().1;
    assert!(
        !info.detachable,
        "a mutating edge ties the firing to the net"
    );

    let firing = net.launch(trip).unwrap();
    assert!(!firing.is_pending(), "run inline at launch");
    net.distribute(firing.run()).unwrap();
    let events: Vec<Event> = net.drain_events().collect();
    assert_eq!(
        events,
        [
            Event::Launched {
                transition: trip,
                consumed: vec![],
            },
            Event::Mutated {
                place: barn.id(),
                token: cow,
            },
            Event::Distributed {
                transition: trip,
                produced: vec![],
            },
        ]
    );
    assert_eq!(ids(&net, barn), [hen, cow, pig], "nothing moved");
    assert_eq!(net.token::<Animal>(barn.id(), cow).unwrap().legs, 3);
    assert_eq!(net.token::<Animal>(barn.id(), pig).unwrap().legs, 4);

    // Re-indexed: the cow has three legs now, so it is no longer a match,
    // and the pig is.
    let three: Vec<TokenId> = net
        .query(&Filter::new().key_in("legs", [3]))
        .unwrap()
        .iter()
        .map(|found| found.token)
        .collect();
    assert_eq!(three, [cow]);
    assert_eq!(net.first_match(barn.id(), &four_legged), Some(pig));
    index_agrees(&net);

    net.fire(trip).unwrap();
    assert_eq!(net.token::<Animal>(barn.id(), pig).unwrap().legs, 3);
    assert!(!net.is_fireable(trip), "no four-legged mammal left");
    index_agrees(&net);
}

/// A keyed edge and a plain one in one transition, with a parameter between
/// them: each argument goes where the tuple put it.
#[test]
fn keyed_and_plain_edges_mix_in_one_transition() {
    #[derive(Clone)]
    struct Bonus(i64);

    let Farm { mut b, barn, yard } = farm(&[("Cow", "Mammal", 4), ("Hen", "Bird", 2)]);
    let feed = b.place::<i64>("Feed");
    b.seed(feed, [10, 20]);
    b.param(Bonus(1));
    b.transition(
        "Feed the bird",
        (
            TakeEdge(feed),
            petritype::Param::<Bonus>::new(),
            MutateWhereEdge(barn, birds()),
        ),
        (yard,),
        |grain: i64, bonus: Bonus, bird: &mut Animal| {
            bird.legs += grain + bonus.0;
            (bird.clone(),)
        },
    );
    let mut net = b.build().unwrap();
    net.step().unwrap();
    let hen = id_of(&net, barn, "Hen");
    assert_eq!(net.token::<Animal>(barn.id(), hen).unwrap().legs, 13);
    assert_eq!(
        net.token::<Animal>(barn.id(), ids(&net, barn)[0])
            .unwrap()
            .legs,
        4
    );
    assert_eq!(net.count(feed.id()), 1);
    index_agrees(&net);
}

// ----- failure ------------------------------------------------------------------

#[test]
fn a_restored_token_goes_back_to_the_front_with_its_id() {
    let Farm { mut b, barn, yard } = farm(&[("Cow", "Mammal", 4), ("Hen", "Bird", 2)]);
    let drop = b
        .transition(
            "Drop",
            (TakeWhereEdge(barn, birds()),),
            (yard,),
            |_: Animal| -> Result<(Animal,), Broken> { Err(Broken) },
        )
        .restore_tokens_on_failure()
        .id();
    let mut net = b.build().unwrap();
    let [cow, hen] = ids(&net, barn)[..] else {
        panic!("two animals")
    };

    let failure = net.fire(drop).expect_err("the body fails");
    assert!(matches!(failure, NetError::FiringFailed(ref f) if f.restored));
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Failed {
            transition: drop,
            consumed: vec![(barn.id(), hen)],
            restored: true,
            error: String::from("dropped it"),
        }]
    );
    assert_eq!(
        ids(&net, barn),
        [hen, cow],
        "the id is kept, the position is not"
    );
    let all: Vec<TokenId> = net
        .query(&Filter::new())
        .unwrap()
        .iter()
        .map(|found| found.token)
        .collect();
    assert_eq!(all, [hen, cow], "a query reads the place in queue order");
    index_agrees(&net);

    // The next firing takes the same token again.
    net.fire(drop).expect_err("still failing");
    let events: Vec<Event> = net.drain_events().collect();
    assert!(
        matches!(&events[..], [Event::Failed { consumed, .. }] if consumed == &[(barn.id(), hen)]),
        "{events:?}"
    );
    assert_eq!(ids(&net, barn), [hen, cow]);
    index_agrees(&net);
}

// ----- parameters ---------------------------------------------------------------

/// A `{key, param}` clause on an edge reads the net's parameter each time
/// fireability is asked.
#[cfg(feature = "serde")]
#[test]
fn a_param_clause_on_an_edge_follows_set_param() {
    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    struct Wanted(String);

    let Farm { mut b, barn, yard } = farm(&[
        ("Cow", "Mammal", 4),
        ("Hen", "Bird", 2),
        ("Pig", "Mammal", 4),
    ]);
    b.param(Wanted("Bird".to_owned()));
    b.serializable_param::<Wanted>("wanted");
    let fetch = b
        .transition(
            "Fetch",
            (TakeWhereEdge(
                barn,
                Filter::new().key_is_param("class", "wanted"),
            ),),
            (yard,),
            |a: Animal| (a,),
        )
        .id();
    let mut net = b.build().unwrap();

    net.fire(fetch).unwrap();
    assert_eq!(names(&net, yard), ["Hen"]);
    assert!(!net.is_fireable(fetch), "no bird left");

    net.set_param(Wanted("Mammal".to_owned())).unwrap();
    assert!(net.is_fireable(fetch));
    net.fire(fetch).unwrap();
    assert_eq!(names(&net, yard), ["Hen", "Cow"]);

    net.set_param(Wanted("Fish".to_owned())).unwrap();
    assert!(!net.is_fireable(fetch));
}

/// Without `serde` no parameter has a wire name, so a parameter clause
/// accepts nothing and the edge never fires.
#[cfg(not(feature = "serde"))]
#[test]
fn without_serde_a_param_clause_on_an_edge_accepts_nothing() {
    let Farm { mut b, barn, yard } = farm(&[("Hen", "Bird", 2)]);
    let fetch = b
        .transition(
            "Fetch",
            (TakeWhereEdge(
                barn,
                Filter::new().key_is_param("class", "wanted"),
            ),),
            (yard,),
            |a: Animal| (a,),
        )
        .id();
    let net = b.build().unwrap();
    assert!(!net.is_fireable(fetch));
}

// ----- build checks -------------------------------------------------------------

#[test]
fn a_keyed_edge_on_a_place_without_keys_is_refused() {
    let Farm { mut b, yard, barn } = farm(&[]);
    b.transition(
        "Pick",
        (TakeWhereEdge(yard, birds()),),
        (barn,),
        |a: Animal| (a,),
    );
    let error = b.build().expect_err("the yard declares no keys");
    assert!(
        matches!(
            &error,
            BuildError::KeyedEdgeOnUnkeyedPlace { transition, place }
                if transition == "Pick" && place == "Yard"
        ),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "transition \"Pick\" reads place \"Yard\" through a keyed edge, but the place declares \
         no keys"
    );

    let Farm { mut b, yard, .. } = farm(&[]);
    b.transition(
        "Trip",
        (MutateWhereEdge(yard, Filter::new()),),
        (),
        |_: &mut Animal| {},
    );
    assert!(matches!(
        b.build(),
        Err(BuildError::KeyedEdgeOnUnkeyedPlace { place, .. }) if place == "Yard"
    ));
}

#[test]
fn a_keyed_edge_reads_its_place_like_any_other_edge() {
    let Farm { mut b, barn, yard } = farm(&[]);
    b.transition(
        "Twice",
        (TakeWhereEdge(barn, birds()), SnapshotEdge(barn)),
        (yard,),
        |a: Animal, _: Animal| (a,),
    );
    assert!(matches!(
        b.build(),
        Err(BuildError::DuplicateInputPlace { transition, place })
            if transition == "Twice" && place == "Barn"
    ));
}

/// A `TakeWhereEdge` consumes, so a restoring transition needs its place to
/// clone; a `MutateWhereEdge` consumes nothing, so it does not.
#[test]
fn a_restoring_keyed_take_needs_a_place_that_clones() {
    let mut b = NetBuilder::new();
    let pen = b.place_no_clone::<Animal>("Pen");
    let yard = b.place_no_clone::<Animal>("Yard");
    declare_keys(&mut b, pen);
    b.transition(
        "Pick",
        (TakeWhereEdge(pen, birds()),),
        (yard,),
        |a: Animal| (a,),
    )
    .restore_tokens_on_failure();
    assert!(matches!(
        b.build(),
        Err(BuildError::RestoreOfNoClonePlace { transition, place })
            if transition == "Pick" && place == "Pen"
    ));

    let mut b = NetBuilder::new();
    let pen = b.place_no_clone::<Animal>("Pen");
    declare_keys(&mut b, pen);
    b.transition(
        "Trip",
        (MutateWhereEdge(pen, birds()),),
        (),
        |a: &mut Animal| a.legs -= 1,
    )
    .restore_tokens_on_failure();
    b.build().expect("nothing to restore, nothing to clone");
}

/// The cycle walk counts a `TakeWhereEdge` as consuming and leaves a
/// `MutateWhereEdge` out, as it does `TakeEdge` and `MutateEdge`; and so
/// does the source check.
#[test]
fn the_opt_in_checks_see_a_keyed_take_as_consuming() {
    let Farm { mut b, barn, .. } = farm(&[]);
    b.assert_acyclic();
    b.transition(
        "Shuffle",
        (TakeWhereEdge(barn, birds()),),
        (barn,),
        |a: Animal| (a,),
    );
    let error = b.build().expect_err("a bird goes round");
    assert_eq!(
        error.to_string(),
        "the net's token flow has a cycle: Shuffle -> Barn -> Shuffle"
    );

    let Farm { mut b, barn, yard } = farm(&[]);
    b.assert_acyclic().assert_no_source_transitions();
    b.transition(
        "Pick",
        (TakeWhereEdge(barn, birds()),),
        (yard,),
        |a: Animal| (a,),
    );
    b.build().expect("acyclic, and Pick consumes");

    let Farm { mut b, barn, .. } = farm(&[]);
    b.assert_acyclic();
    b.transition(
        "Groom",
        (MutateWhereEdge(barn, birds()),),
        (barn,),
        |a: &mut Animal| (a.clone(),),
    );
    b.build()
        .expect("a mutating edge moves no token, so this is no cycle");

    let Farm { mut b, barn, .. } = farm(&[]);
    b.assert_no_source_transitions();
    b.transition(
        "Groom",
        (MutateWhereEdge(barn, birds()),),
        (),
        |a: &mut Animal| a.legs += 0,
    );
    assert!(matches!(
        b.build(),
        Err(BuildError::SourceTransition { transition }) if transition == "Groom"
    ));
}

// ----- the read API -------------------------------------------------------------

/// `TransitionInfo::filters`, `EdgeInfo::Input::filter` and the topology
/// carry a keyed edge's filter as declared, and nothing for the others.
#[test]
fn the_read_api_carries_the_filter() {
    let Farm { mut b, barn, yard } = farm(&[]);
    let feed = b.place::<i64>("Feed");
    let pick = b
        .transition(
            "Pick",
            (TakeWhereEdge(barn, birds()), TakeEdge(feed)),
            (yard,),
            |a: Animal, _: i64| (a,),
        )
        .id();
    let trip = b
        .transition(
            "Trip",
            (MutateWhereEdge(barn, mammals()),),
            (),
            |a: &mut Animal| a.legs -= 1,
        )
        .id();
    let net = b.build().unwrap();

    let infos: Vec<_> = net.transitions().collect();
    assert_eq!(
        infos[0].1.inputs,
        [
            (barn.id(), EdgeKind::TakeWhere),
            (feed.id(), EdgeKind::Take)
        ]
    );
    assert_eq!(infos[0].1.filters, [Some(birds()), None]);
    assert!(infos[0].1.detachable);
    assert_eq!(infos[1].1.inputs, [(barn.id(), EdgeKind::MutateWhere)]);
    assert_eq!(infos[1].1.filters, [Some(mammals())]);
    assert!(!infos[1].1.detachable);

    let expected = vec![
        EdgeInfo::Input {
            place: barn.id(),
            transition: pick,
            kind: EdgeKind::TakeWhere,
            filter: Some(birds()),
        },
        EdgeInfo::Input {
            place: feed.id(),
            transition: pick,
            kind: EdgeKind::Take,
            filter: None,
        },
        EdgeInfo::Output {
            transition: pick,
            place: yard.id(),
            slot: 0,
        },
        EdgeInfo::Input {
            place: barn.id(),
            transition: trip,
            kind: EdgeKind::MutateWhere,
            filter: Some(mammals()),
        },
    ];
    assert_eq!(net.edges().collect::<Vec<_>>(), expected);
    assert_eq!(
        net.snapshot().unwrap().edges().collect::<Vec<_>>(),
        expected
    );
    let topology = net.topology();
    assert_eq!(topology.edges, expected);
    assert_eq!(topology.transitions[0].filters, [Some(birds()), None]);
    assert_eq!(topology.transitions[1].filters, [Some(mammals())]);
    assert_eq!(net.snapshot().unwrap().topology(), topology);

    assert!(EdgeKind::TakeWhere.consumes() && EdgeKind::TakeWhere.is_keyed());
    assert!(!EdgeKind::MutateWhere.consumes() && EdgeKind::MutateWhere.is_keyed());
    assert!(!EdgeKind::Take.is_keyed());
}

#[cfg(feature = "serde")]
mod json {
    use super::*;
    use petritype::serde_json::{self, json};
    use petritype::{Clause, KeyValue};

    #[test]
    fn the_keyed_kinds_serialise_by_the_protocols_names() {
        assert_eq!(
            [EdgeKind::TakeWhere, EdgeKind::MutateWhere]
                .map(|kind| serde_json::to_value(kind).unwrap()),
            [json!("take_where"), json!("mutate_where")]
        );
    }

    /// A keyed edge's filter is on its edge, in the protocol's shape with
    /// the core's place ids; no other edge has the field, and a transition
    /// without a keyed edge writes no `filters`.
    #[test]
    fn edges_and_the_topology_serialise_the_filter() {
        let Farm { mut b, barn, yard } = farm(&[]);
        b.transition(
            "Pick",
            (TakeWhereEdge(barn, birds()),),
            (yard,),
            |a: Animal| (a,),
        );
        b.transition(
            "Trip",
            (MutateWhereEdge(
                barn,
                Filter::new()
                    .key_is_param("class", "wanted")
                    .in_places([yard.id()])
                    .with_prefix("Ba"),
            ),),
            (),
            |a: &mut Animal| a.legs -= 1,
        );
        b.transition("Move", (TakeEdge(yard),), (barn,), |a: Animal| (a,));
        let net = b.build().unwrap();

        let topology = serde_json::to_value(net.topology()).unwrap();
        assert_eq!(
            topology["edges"],
            json!([
                {"edge": "input", "place": 0, "transition": 0, "kind": "take_where",
                 "filter": {"clauses": [{"key": "class", "values": ["Bird"]}]}},
                {"edge": "output", "transition": 0, "place": 1, "slot": 0},
                {"edge": "input", "place": 0, "transition": 1, "kind": "mutate_where",
                 "filter": {"clauses": [{"key": "class", "param": "wanted"}],
                            "places": [1], "prefix": "Ba"}},
                {"edge": "input", "place": 1, "transition": 2, "kind": "take"},
                {"edge": "output", "transition": 2, "place": 0, "slot": 0},
            ])
        );
        assert_eq!(
            topology["transitions"][0]["filters"],
            json!([{"clauses": [{"key": "class", "values": ["Bird"]}]}])
        );
        assert!(
            topology["transitions"][2].get("filters").is_none(),
            "no keyed input, no field: {}",
            topology["transitions"][2]
        );
    }

    /// The filter reads back from its protocol shape, a clause with neither
    /// `values` nor `param` is no clause, and `clauses` may be left out.
    #[test]
    fn a_filter_reads_back_from_its_json() {
        let filter = Filter::new()
            .key_in("class", ["Bird"])
            .key_in("legs", [KeyValue::Int(2), KeyValue::Bool(true)])
            .key_is_param("name", "wanted")
            .in_places([petritype::PlaceId::new(3)])
            .with_prefix("B");
        let value = serde_json::to_value(&filter).unwrap();
        assert_eq!(
            value,
            json!({"clauses": [{"key": "class", "values": ["Bird"]},
                               {"key": "legs", "values": [2, true]},
                               {"key": "name", "param": "wanted"}],
                   "places": [3], "prefix": "B"})
        );
        assert_eq!(serde_json::from_value::<Filter>(value).unwrap(), filter);

        assert_eq!(
            serde_json::from_value::<Filter>(json!({})).unwrap(),
            Filter::new()
        );
        assert!(
            serde_json::from_value::<Filter>(json!({"clauses": [{"key": "class"}]})).is_err(),
            "a clause must say what it accepts"
        );
        assert!(matches!(
            serde_json::from_value::<Clause>(json!({"key": "class", "param": "season"})),
            Ok(Clause::IsParam { .. })
        ));
    }
}

// ----- drivers and nesting ------------------------------------------------------

/// Under the concurrent driver two keyed edges wanting the same bird do not
/// both get it: the first launch takes it, and the second finds nothing.
#[test]
fn two_keyed_edges_racing_for_one_bird_under_the_concurrent_driver() {
    let Farm { mut b, barn, yard } = farm(&[("Cow", "Mammal", 4), ("Hen", "Bird", 2)]);
    let coop = b.place::<Animal>("Coop");
    b.transition(
        "To yard",
        (TakeWhereEdge(barn, birds()),),
        (yard,),
        |a: Animal| (a,),
    );
    b.transition(
        "To coop",
        (TakeWhereEdge(barn, birds()),),
        (coop,),
        |a: Animal| (a,),
    );
    let mut net = b.build().unwrap();
    let cow = ids(&net, barn)[0];

    let summary = drivers::run(
        &mut net,
        Mode::Concurrent,
        Limits::default(),
        OnFailure::Stop,
    )
    .unwrap();
    assert!(summary.quiesced);
    assert_eq!(summary.fired_count, 1);
    assert_eq!(net.count(yard.id()) + net.count(coop.id()), 1);
    assert_eq!(ids(&net, barn), [cow], "the cow never moved");
    index_agrees(&net);
}

/// The sequential driver drains every match and nothing else.
#[test]
fn a_run_takes_every_match_and_leaves_the_rest_with_their_ids() {
    let Farm { mut b, barn, yard } = farm(&[
        ("Hen", "Bird", 2),
        ("Cow", "Mammal", 4),
        ("Owl", "Bird", 2),
        ("Pig", "Mammal", 4),
        ("Emu", "Bird", 2),
    ]);
    b.transition(
        "Pick",
        (TakeWhereEdge(barn, birds()),),
        (yard,),
        |a: Animal| (a,),
    );
    let mut net = b.build().unwrap();
    let before = ids(&net, barn);
    let mammal_ids = vec![before[1], before[3]];
    net.drain_events().for_each(drop);

    let summary = drivers::run(
        &mut net,
        Mode::Sequential,
        Limits::default(),
        OnFailure::Stop,
    )
    .unwrap();
    assert_eq!(summary.fired_count, 3);
    assert_eq!(ids(&net, barn), mammal_ids);
    assert_eq!(names(&net, yard), ["Hen", "Owl", "Emu"]);
    let events: Vec<Event> = net.drain_events().collect();
    assert!(
        events
            .iter()
            .all(|event| matches!(event, Event::Fired { consumed, .. } if consumed.len() == 1)),
        "three firings of one token each, nothing else: {events:?}"
    );
    index_agrees(&net);
}

/// A keyed edge works on a nested place, whose keys may read the token's
/// inner net: the one with work left is taken, and its inner net goes with
/// it, while the other stays where it is.
#[test]
fn a_keyed_edge_on_a_nested_place() {
    let mut inner = NetBuilder::new();
    let todo = inner.place::<u32>("Todo");
    let busy = {
        let mut net = inner.build().unwrap();
        net.put(todo, 1);
        net
    };
    let idle = {
        let mut inner = NetBuilder::new();
        inner.place::<u32>("Todo");
        inner.build().unwrap()
    };
    let busy_id = busy.id();

    let mut b = NetBuilder::new();
    let desks = b.place_nested::<Net>("Desks");
    let taken = b.place_nested::<Net>("Taken");
    b.keys(desks, ["busy"], move |net: &Net| {
        [(net.count(todo.id()) > 0).into()]
    });
    b.seed(desks, [idle, busy]);
    let take: TransitionId = b
        .transition(
            "Take a busy desk",
            (TakeWhereEdge(desks, Filter::new().key_in("busy", [true])),),
            (taken,),
            |net: Net| (net,),
        )
        .id();
    let mut net = b.build().unwrap();
    let [idle_token, _] = net.tokens(desks.id()).map(|(id, _)| id).collect::<Vec<_>>()[..] else {
        panic!("two desks")
    };

    net.fire(take).unwrap();
    let (moved, _) = net.tokens(taken.id()).next().unwrap();
    let inner = net.nested(taken.id(), moved).unwrap();
    assert_eq!(inner[0].1.id(), busy_id, "the busy desk's own net went");
    assert_eq!(
        net.tokens(desks.id()).map(|(id, _)| id).collect::<Vec<_>>(),
        [idle_token]
    );
    assert!(!net.is_fireable(take));
}
