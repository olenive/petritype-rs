//! Integration tests for several input edges of one transition on one place
//! (DESIGN §4.4, since 0.4.0, the owner's decision D6), ported from Python
//! petritype's `tests/test_edges_sharing_a_place.py` where the two
//! libraries agree.
//!
//! Take and take-where edges share a place, each taking a token of its own:
//! the transition is fireable only while the place holds a token for every
//! edge, the edges choose in declaration order, each the first token no edge
//! before it chose, and the choice is greedy, as Python's planning is. Every
//! other kind beside another edge on its place is refused when the net is
//! built, and so is a bound edge or a source on a shared place. A firing
//! takes all its tokens or none, a failed one that restores puts each back
//! at the front of the place it came from, the first edge's first, and the
//! events, the read API and the topology name each edge. Two output edges of
//! one transition to one place are refused at build.
//!
//! Python allows more beside a take on one place (a snapshot, a drain, a
//! list-mode take); the Rust core refuses those, as the owner decided.

use std::num::NonZeroUsize;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use petritype::drivers::{self, Limits, Mode, OnFailure};
use petritype::{
    BuildError, ClosedRefusal, Distribution, DrainEdge, EdgeInfo, EdgeKind, Event, Filter,
    FiringFailed, Inputs, KeyValue, MutateEdge, MutateWhereEdge, Net, NetBuilder, NetError,
    Outputs, PlaceRef, ReadAllEdge, ReadEdge, ReadWhereEdge, Routed, SnapshotEdge, SurveyEdge,
    TakeAllEdge, TakeEdge, TakeWhereEdge, TokenId, TransitionId,
};

#[derive(Debug)]
struct Broken;

impl std::fmt::Display for Broken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("broken")
    }
}

impl std::error::Error for Broken {}

/// The values a place holds, in queue order.
fn values<T: Clone + 'static>(net: &Net, place: PlaceRef<T>) -> Vec<T> {
    net.tokens(place.id())
        .map(|(id, _)| net.token::<T>(place.id(), id).unwrap().clone())
        .collect()
}

/// The ids a place holds, in queue order.
fn ids<T>(net: &Net, place: PlaceRef<T>) -> Vec<TokenId> {
    net.tokens(place.id()).map(|(id, _)| id).collect()
}

/// The tokens a query finds, in the order it finds them.
fn found(net: &Net, filter: &Filter) -> Vec<TokenId> {
    net.query(filter)
        .expect("every place exists")
        .into_iter()
        .map(|found| found.token)
        .collect()
}

/// The failure a firing ended in.
fn failure(error: NetError) -> FiringFailed {
    match error {
        NetError::FiringFailed(failed) => failed,
        other => panic!("a failed firing, not {other:?}"),
    }
}

// ----- two oxygen radicals ----------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
struct Radical(u32);

#[derive(Debug, Clone, PartialEq)]
struct Molecule(u32, u32);

/// `Radicals`, a port, and `Bond`, which takes two of them through two take
/// edges and makes a molecule.
fn oxygen() -> (Net, PlaceRef<Radical>, PlaceRef<Molecule>, TransitionId) {
    let mut b = NetBuilder::new();
    let radicals = b.place::<Radical>("Radicals");
    let molecules = b.place::<Molecule>("Molecules");
    b.port(radicals);
    let bond = b
        .transition(
            "Bond",
            (TakeEdge(radicals), TakeEdge(radicals)),
            (molecules,),
            |a: Radical, c: Radical| (Molecule(a.0, c.0),),
        )
        .id();
    (
        b.build().expect("two takes share a place"),
        radicals,
        molecules,
        bond,
    )
}

#[test]
fn two_oxygen_radicals_make_one_molecule_and_one_radical_waits() {
    let (mut net, radicals, molecules, bond) = oxygen();
    let first = net.put(radicals, Radical(1));
    assert!(!net.is_fireable(bond), "one radical for two edges");
    assert_eq!(net.step().unwrap(), None, "it waits");
    assert_eq!(ids(&net, radicals), [first], "and takes nothing");
    assert!(matches!(net.fire(bond), Err(NetError::NotFireable { .. })));

    let second = net.put(radicals, Radical(2));
    assert_eq!(net.fireable(), [bond]);
    net.drain_events().for_each(drop);
    assert_eq!(net.step().unwrap(), Some(bond));
    assert_eq!(
        values(&net, molecules),
        [Molecule(1, 2)],
        "the first edge takes the front token, the second the one after"
    );
    let produced = ids(&net, molecules)[0];
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Fired {
            transition: bond,
            consumed: vec![(radicals.id(), first), (radicals.id(), second)],
            produced: vec![(molecules.id(), produced)],
        }],
        "each edge's token, in edge order"
    );

    for n in 3..=5 {
        net.put(radicals, Radical(n));
    }
    assert_eq!(net.step().unwrap(), Some(bond));
    assert_eq!(net.step().unwrap(), None, "the fifth radical waits");
    assert_eq!(values(&net, molecules), [Molecule(1, 2), Molecule(3, 4)]);
    assert_eq!(values(&net, radicals), [Radical(5)]);
}

/// The read API, the edge list and the topology carry each edge, and the
/// staged path reports both tokens at launch.
#[test]
fn each_edge_is_named_in_the_read_api_the_topology_and_the_events() {
    let (mut net, radicals, molecules, bond) = oxygen();
    let (_, info) = net.transitions().next().unwrap();
    assert_eq!(
        info.inputs,
        [
            (radicals.id(), EdgeKind::Take),
            (radicals.id(), EdgeKind::Take)
        ]
    );
    let inputs: Vec<EdgeInfo> = net
        .edges()
        .filter(|edge| matches!(edge, EdgeInfo::Input { .. }))
        .collect();
    assert_eq!(inputs.len(), 2, "two edges, two arrows");
    assert!(inputs.iter().all(|edge| matches!(
        edge,
        EdgeInfo::Input { place, transition, kind: EdgeKind::Take, .. }
            if *place == radicals.id() && *transition == bond
    )));
    assert_eq!(net.topology().edges, net.edges().collect::<Vec<_>>());

    let first = net.put(radicals, Radical(1));
    let second = net.put(radicals, Radical(2));
    net.drain_events().for_each(drop);
    let firing = net.launch(bond).unwrap();
    assert_eq!(net.count(radicals.id()), 0, "both out at launch");
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Launched {
            transition: bond,
            consumed: vec![(radicals.id(), first), (radicals.id(), second)],
        }]
    );
    net.distribute(firing.run()).unwrap();
    assert_eq!(values(&net, molecules), [Molecule(1, 2)]);
}

/// Beside a mutating edge on a place of its own, which makes the firing
/// inline, two takes on one place take their two tokens as on a worker.
#[test]
fn takes_on_a_shared_place_beside_a_mutate_edge_fire_inline() {
    let mut b = NetBuilder::new();
    let radicals = b.place::<Radical>("Radicals");
    let molecules = b.place::<Molecule>("Molecules");
    let count = b.place::<u32>("Count");
    b.seed(radicals, (1..=3).map(Radical));
    b.seed(count, [0]);
    b.transition(
        "Bond",
        (TakeEdge(radicals), MutateEdge(count), TakeEdge(radicals)),
        (molecules,),
        |a: Radical, n: &mut u32, c: Radical| {
            *n += 1;
            (Molecule(a.0, c.0),)
        },
    );
    let mut net = b.build().unwrap();
    net.step().unwrap().expect("Bond");
    assert_eq!(net.step().unwrap(), None, "one radical left");
    assert_eq!(values(&net, molecules), [Molecule(1, 2)]);
    assert_eq!(values(&net, radicals), [Radical(3)]);
    assert_eq!(values(&net, count), [1]);
}

/// A struct of edges with two fields on one place: its edges are flat, in
/// field order, like any others.
#[test]
fn a_struct_of_edges_may_read_one_place_through_two_fields() {
    #[derive(Inputs)]
    struct Bonding {
        left: Radical,
        right: Radical,
    }
    let mut b = NetBuilder::new();
    let radicals = b.place::<Radical>("Radicals");
    let molecules = b.place::<Molecule>("Molecules");
    b.seed(radicals, [Radical(1), Radical(2)]);
    b.transition(
        "Bond",
        (BondingEdges {
            right: TakeEdge(radicals),
            left: TakeEdge(radicals),
        },),
        (molecules,),
        |pair: Bonding| (Molecule(pair.left.0, pair.right.0),),
    );
    let mut net = b.build().unwrap();
    net.step().unwrap().expect("Bond");
    assert_eq!(values(&net, molecules), [Molecule(1, 2)], "in field order");
}

// ----- a ballroom ---------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
struct Dancer {
    name: &'static str,
    role: &'static str,
}

fn lady(name: &'static str) -> Dancer {
    Dancer { name, role: "lady" }
}

fn gentleman(name: &'static str) -> Dancer {
    Dancer {
        name,
        role: "gentleman",
    }
}

fn role(role: &str) -> Filter {
    Filter::new().key_in("role", [role])
}

/// Either role: a filter that accepts every dancer.
fn anyone() -> Filter {
    Filter::new().key_in("role", ["lady", "gentleman"])
}

struct Ballroom {
    net: Net,
    hall: PlaceRef<Dancer>,
    floor: PlaceRef<(Dancer, Dancer)>,
    dance: TransitionId,
}

/// `Ballroom`, a port keyed by role and seeded with `seed`, and `Dance`,
/// which takes a couple through two take-where edges, `first` and then
/// `second`, onto `Floor`.
fn ballroom(seed: &[Dancer], first: Filter, second: Filter) -> Ballroom {
    let mut b = NetBuilder::new();
    let hall = b.place::<Dancer>("Ballroom");
    let floor = b.place::<(Dancer, Dancer)>("Floor");
    b.port(hall);
    b.keys(hall, ["role"], |d: &Dancer| [KeyValue::from(d.role)]);
    b.seed(hall, seed.iter().cloned());
    let dance = b
        .transition(
            "Dance",
            (TakeWhereEdge(hall, first), TakeWhereEdge(hall, second)),
            (floor,),
            |a: Dancer, c: Dancer| ((a, c),),
        )
        .id();
    Ballroom {
        net: b.build().expect("two take-wheres share a place"),
        hall,
        floor,
        dance,
    }
}

#[test]
fn a_lady_and_a_gentleman_are_taken_from_one_ballroom() {
    let mut r = ballroom(
        &[lady("Ada"), lady("Cleo")],
        role("lady"),
        role("gentleman"),
    );
    assert!(!r.net.is_fireable(r.dance), "two ladies and no gentleman");
    r.net.put(r.hall, gentleman("Ben"));
    assert!(r.net.is_fireable(r.dance), "one of each");
    r.net.step().unwrap().expect("Dance");
    assert_eq!(values(&r.net, r.floor), [(lady("Ada"), gentleman("Ben"))]);
    assert_eq!(
        values(&r.net, r.hall),
        [lady("Cleo")],
        "the rest keep their places"
    );
    assert_eq!(found(&r.net, &role("lady")), ids(&r.net, r.hall));
    assert!(found(&r.net, &role("gentleman")).is_empty());

    let r = ballroom(
        &[gentleman("Ben"), gentleman("Dan")],
        role("lady"),
        role("gentleman"),
    );
    assert!(!r.net.is_fireable(r.dance), "two gentlemen and no lady");
}

#[test]
fn two_edges_never_take_the_same_token() {
    let mut r = ballroom(&[lady("Ada")], anyone(), anyone());
    assert!(
        !r.net.is_fireable(r.dance),
        "one dancer that both filters accept"
    );
    r.net.put(r.hall, gentleman("Ben"));
    r.net.step().unwrap().expect("Dance");
    assert_eq!(values(&r.net, r.floor), [(lady("Ada"), gentleman("Ben"))]);
}

/// The first edge declared takes the front token, the next the one after;
/// a take-where takes the first token its filter accepts that no earlier
/// edge took.
#[test]
fn each_edge_takes_the_first_token_no_edge_before_it_took() {
    fn parity(even: bool) -> Filter {
        Filter::new().key_in("parity", [if even { "even" } else { "odd" }])
    }
    /// A builder with `Numbers`, seeded 1 to 6 and keyed by parity, and
    /// `Taken`.
    fn numbers() -> (NetBuilder, PlaceRef<u32>, PlaceRef<Vec<u32>>) {
        let mut b = NetBuilder::new();
        let numbers = b.place::<u32>("Numbers");
        let taken = b.place::<Vec<u32>>("Taken");
        b.keys(numbers, ["parity"], |n: &u32| {
            [KeyValue::from(if n.is_multiple_of(2) {
                "even"
            } else {
                "odd"
            })]
        });
        b.seed(numbers, 1..=6);
        (b, numbers, taken)
    }
    let three = |a: u32, c: u32, d: u32| (vec![a, c, d],);

    let (mut b, p, taken) = numbers();
    let edges = (TakeEdge(p), TakeWhereEdge(p, parity(false)), TakeEdge(p));
    b.transition("Take", edges, (taken,), three);
    let mut net = b.build().unwrap();
    net.step().unwrap().expect("Take");
    assert_eq!(values(&net, taken), [vec![1, 3, 2]]);
    assert_eq!(values(&net, p), [4, 5, 6]);

    let (mut b, p, taken) = numbers();
    let edges = (
        TakeWhereEdge(p, parity(true)),
        TakeEdge(p),
        TakeWhereEdge(p, parity(true)),
    );
    b.transition("Take", edges, (taken,), three);
    let mut net = b.build().unwrap();
    net.step().unwrap().expect("Take");
    assert_eq!(values(&net, taken), [vec![2, 1, 4]]);
    assert_eq!(values(&net, p), [3, 5, 6]);
}

/// Python's planning is greedy in declaration order: an edge never gives
/// its token up for a later edge, so when the first takes the only token
/// the second could also have used the transition is not fireable, though
/// the other assignment would have fired it. Rust does the same.
#[test]
fn the_choice_is_greedy_in_declaration_order_as_python_s_planning_is() {
    let couple = [gentleman("Ben"), lady("Ada")];
    let r = ballroom(&couple, anyone(), role("gentleman"));
    assert!(
        !r.net.is_fireable(r.dance),
        "the first filter takes Ben, the only gentleman"
    );
    let mut r = ballroom(&couple, role("gentleman"), anyone());
    r.net.step().unwrap().expect("the narrower filter first");
    assert_eq!(values(&r.net, r.floor), [(gentleman("Ben"), lady("Ada"))]);

    // A plain take is greedy too: it takes the front token whatever a
    // later filter wanted.
    for take_first in [true, false] {
        let mut b = NetBuilder::new();
        let hall = b.place::<Dancer>("Ballroom");
        let floor = b.place::<(Dancer, Dancer)>("Floor");
        b.keys(hall, ["role"], |d: &Dancer| [KeyValue::from(d.role)]);
        b.seed(hall, couple.iter().cloned());
        let dance = if take_first {
            b.transition(
                "Dance",
                (TakeEdge(hall), TakeWhereEdge(hall, role("gentleman"))),
                (floor,),
                |a: Dancer, c: Dancer| ((a, c),),
            )
            .id()
        } else {
            b.transition(
                "Dance",
                (TakeWhereEdge(hall, role("gentleman")), TakeEdge(hall)),
                (floor,),
                |a: Dancer, c: Dancer| ((a, c),),
            )
            .id()
        };
        let net = b.build().unwrap();
        assert_eq!(
            net.is_fireable(dance),
            !take_first,
            "take first: {take_first}"
        );
    }
}

// ----- what is refused --------------------------------------------------------------

/// Declares a transition over `P`.
type Declare = fn(&mut NetBuilder, PlaceRef<u32>);

/// A transition over `P`, a place of numbers keyed by `n`, declared by
/// `declare`; the build's answer.
fn built(declare: Declare) -> Result<Net, BuildError> {
    let mut b = NetBuilder::new();
    let p = b.place::<u32>("P");
    b.keys(p, ["n"], |n: &u32| [KeyValue::from(i64::from(*n))]);
    declare(&mut b, p);
    b.build()
}

fn refused_as(declare: Declare) -> EdgeKind {
    match built(declare) {
        Err(BuildError::EdgeCannotSharePlace {
            transition,
            place,
            kind,
        }) => {
            assert_eq!((transition.as_str(), place.as_str()), ("T", "P"));
            kind
        }
        other => panic!("a refusal of the shared place, not {other:?}"),
    }
}

#[test]
fn every_other_kind_beside_another_edge_on_its_place_is_refused_at_build() {
    let beside_a_take: [(EdgeKind, Declare); 9] = [
        (EdgeKind::TakeAll, |b, p| {
            b.transition(
                "T",
                (TakeEdge(p), TakeAllEdge(p)),
                (),
                |_: u32, _: Vec<u32>| {},
            );
        }),
        (EdgeKind::Drain, |b, p| {
            b.transition(
                "T",
                (TakeEdge(p), DrainEdge(p)),
                (),
                |_: u32, _: Vec<u32>| {},
            );
        }),
        (EdgeKind::Snapshot, |b, p| {
            b.transition("T", (TakeEdge(p), SnapshotEdge(p)), (), |_: u32, _: u32| {});
        }),
        (EdgeKind::Survey, |b, p| {
            b.transition(
                "T",
                (TakeEdge(p), SurveyEdge(p)),
                (),
                |_: u32, _: Vec<u32>| {},
            );
        }),
        (EdgeKind::Mutate, |b, p| {
            b.transition(
                "T",
                (TakeEdge(p), MutateEdge(p)),
                (),
                |_: u32, _: &mut u32| {},
            );
        }),
        (EdgeKind::MutateWhere, |b, p| {
            let edge = MutateWhereEdge(p, Filter::new());
            b.transition("T", (TakeEdge(p), edge), (), |_: u32, _: &mut u32| {});
        }),
        (EdgeKind::Read, |b, p| {
            b.transition("T", (TakeEdge(p), ReadEdge(p)), (), |_: u32, _: &u32| {});
        }),
        (EdgeKind::ReadWhere, |b, p| {
            let edge = ReadWhereEdge(p, Filter::new());
            b.transition("T", (TakeEdge(p), edge), (), |_: u32, _: &u32| {});
        }),
        (EdgeKind::ReadAll, |b, p| {
            b.transition(
                "T",
                (TakeEdge(p), ReadAllEdge(p)),
                (),
                |_: u32, _: Vec<&u32>| {},
            );
        }),
    ];
    for (kind, declare) in beside_a_take {
        assert_eq!(refused_as(declare), kind);
    }

    // Whichever comes first, and two of one kind, and beside a take-where.
    assert_eq!(
        refused_as(|b, p| {
            b.transition("T", (SnapshotEdge(p), TakeEdge(p)), (), |_: u32, _: u32| {});
        }),
        EdgeKind::Snapshot
    );
    assert_eq!(
        refused_as(|b, p| {
            b.transition(
                "T",
                (MutateEdge(p), MutateEdge(p)),
                (),
                |_: &mut u32, _: &mut u32| {},
            );
        }),
        EdgeKind::Mutate
    );
    assert_eq!(
        refused_as(|b, p| {
            b.transition("T", (ReadEdge(p), ReadEdge(p)), (), |_: &u32, _: &u32| {});
        }),
        EdgeKind::Read
    );
    assert_eq!(
        refused_as(|b, p| {
            let edges = (TakeWhereEdge(p, Filter::new()), TakeEdge(p), DrainEdge(p));
            b.transition("T", edges, (), |_: u32, _: u32, _: Vec<u32>| {});
        }),
        EdgeKind::Drain
    );

    let error = built(|b, p| {
        b.transition("T", (TakeEdge(p), SnapshotEdge(p)), (), |_: u32, _: u32| {});
    })
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "transition \"T\" reads place \"P\" through a SnapshotEdge and another input edge; only \
         TakeEdge and TakeWhereEdge share a place, each taking a token of its own"
    );

    // Two edges on two places, and three takes on one, build.
    built(|b, p| {
        let q = b.place::<u32>("Q");
        b.transition(
            "T",
            (TakeEdge(p), MutateEdge(q)),
            (),
            |_: u32, _: &mut u32| {},
        );
    })
    .unwrap();
    built(|b, p| {
        let edges = (TakeEdge(p), TakeWhereEdge(p, Filter::new()), TakeEdge(p));
        b.transition("T", edges, (), |_: u32, _: u32, _: u32| {});
    })
    .unwrap();
}

// ----- edges that pair tokens ----------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
struct Order(&'static str);

#[derive(Debug, Clone, PartialEq)]
struct Unit(&'static str);

/// `Orders` keyed by the unit each names, `Units` keyed by name, `Stamps`,
/// and a transition `T` that `declare` adds over them.
fn depot(
    declare: impl FnOnce(&mut NetBuilder, PlaceRef<Order>, PlaceRef<Unit>, PlaceRef<u32>),
) -> Result<Net, BuildError> {
    let mut b = NetBuilder::new();
    let orders = b.place::<Order>("Orders");
    let units = b.place::<Unit>("Units");
    let stamps = b.place::<u32>("Stamps");
    b.keys(orders, ["unit"], |o: &Order| [KeyValue::from(o.0)]);
    b.keys(units, ["name"], |u: &Unit| [KeyValue::from(u.0)]);
    b.seed(orders, [Order("Ben"), Order("Ada")]);
    b.seed(units, [Unit("Ada"), Unit("Ben")]);
    b.seed(stamps, [1, 2, 3]);
    declare(&mut b, orders, units, stamps);
    b.build()
}

fn named_by(orders: PlaceRef<Order>) -> Filter {
    Filter::new().key_is_input("name", orders, "unit")
}

/// A bound edge and its source each read a place of their own, as Python
/// petritype asks: a second edge on either place is refused, since it could
/// take the token the pair was found with. A pairing beside takes that
/// share another place fires.
#[test]
fn a_bound_edge_or_its_source_on_a_shared_place_is_refused() {
    let shared = |result: Result<Net, BuildError>| match result {
        Err(BuildError::BoundPlaceShared { transition, place }) => {
            assert_eq!(transition, "T");
            place
        }
        other => panic!("a refusal of the pair's shared place, not {other:?}"),
    };
    let place = shared(depot(|b, orders, units, _| {
        b.transition(
            "T",
            (
                TakeEdge(orders),
                TakeEdge(orders),
                TakeWhereEdge(units, named_by(orders)),
            ),
            (),
            |_: Order, _: Order, _: Unit| {},
        );
    }));
    assert_eq!(place, "Orders", "the source's place");
    let place = shared(depot(|b, orders, units, _| {
        b.transition(
            "T",
            (
                TakeEdge(orders),
                TakeWhereEdge(units, named_by(orders)),
                TakeEdge(units),
            ),
            (),
            |_: Order, _: Unit, _: Unit| {},
        );
    }));
    assert_eq!(place, "Units", "the bound edge's place");
    let error = depot(|b, orders, units, _| {
        b.transition(
            "T",
            (
                TakeEdge(orders),
                TakeWhereEdge(units, Filter::new()),
                TakeWhereEdge(units, named_by(orders)),
            ),
            (),
            |_: Order, _: Unit, _: Unit| {},
        );
    })
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "transition \"T\" reads place \"Units\" through more than one input edge, and a token \
         there chooses or is chosen by another input"
    );

    let mut net = depot(|b, orders, units, stamps| {
        let done = b.place::<(Order, Unit, u32, u32)>("Done");
        b.transition(
            "T",
            (
                TakeEdge(orders),
                TakeWhereEdge(units, named_by(orders)),
                TakeEdge(stamps),
                TakeEdge(stamps),
            ),
            (done,),
            |o: Order, u: Unit, a: u32, c: u32| ((o, u, a, c),),
        );
    })
    .expect("the pair reads places of its own");
    net.step().unwrap().expect("T");
    let done = net.place_ref::<(Order, Unit, u32, u32)>("Done").unwrap();
    assert_eq!(values(&net, done), [(Order("Ben"), Unit("Ben"), 1, 2)]);
}

// ----- failure ------------------------------------------------------------------------

/// A failed firing that restores puts each token back at the front of the
/// place it came from, with its id, the first edge's first, as Python
/// does: `[1, 2, 3]` again after two takes.
#[test]
fn a_failed_firing_puts_the_tokens_back_in_edge_order_with_their_ids() {
    let mut b = NetBuilder::new();
    let p = b.place::<u32>("P");
    b.seed(p, [1, 2, 3]);
    let boom = b
        .transition(
            "Boom",
            (TakeEdge(p), TakeEdge(p)),
            (),
            |_: u32, _: u32| -> Result<(), Broken> { Err(Broken) },
        )
        .restore_tokens_on_failure()
        .id();
    let mut net = b.build().unwrap();
    let before = ids(&net, p);
    net.drain_events().for_each(drop);
    let failed = failure(net.fire(boom).unwrap_err());
    assert!(failed.restored);
    assert_eq!(ids(&net, p), before, "every token back, ids kept");
    assert_eq!(values(&net, p), [1, 2, 3]);
    assert!(matches!(
        net.drain_events().next_back(),
        Some(Event::Failed { consumed, restored: true, .. })
            if consumed == [(p.id(), before[0]), (p.id(), before[1])]
    ));
}

/// Two take-wheres that took from the middle and the front put their
/// tokens back at the front in edge order, and the key index follows: each
/// query finds what the place holds, and the next firing takes the same
/// couple again.
#[test]
fn a_failed_keyed_firing_restores_the_couple_and_the_index_follows() {
    let fail = Arc::new(AtomicBool::new(true));
    let failing = Arc::clone(&fail);
    let mut b = NetBuilder::new();
    let hall = b.place::<Dancer>("Ballroom");
    let floor = b.place::<(Dancer, Dancer)>("Floor");
    b.keys(hall, ["role"], |d: &Dancer| [KeyValue::from(d.role)]);
    b.seed(hall, [lady("Ada"), gentleman("Ben"), lady("Cleo")]);
    let dance = b
        .transition(
            "Dance",
            (
                TakeWhereEdge(hall, role("gentleman")),
                TakeWhereEdge(hall, role("lady")),
            ),
            (floor,),
            move |a: Dancer, c: Dancer| -> Result<((Dancer, Dancer),), Broken> {
                if failing.load(Ordering::SeqCst) {
                    return Err(Broken);
                }
                Ok(((a, c),))
            },
        )
        .restore_tokens_on_failure()
        .id();
    let mut net = b.build().unwrap();
    let [ada, ben, cleo] = ids(&net, hall)[..] else {
        panic!("three dancers")
    };
    assert!(failure(net.fire(dance).unwrap_err()).restored);
    assert_eq!(
        ids(&net, hall),
        [ben, ada, cleo],
        "the first edge's token at the front, then the second's"
    );
    assert_eq!(found(&net, &role("lady")), [ada, cleo]);
    assert_eq!(found(&net, &role("gentleman")), [ben]);
    assert_eq!(found(&net, &Filter::new()), [ben, ada, cleo]);

    fail.store(false, Ordering::SeqCst);
    net.fire(dance).unwrap();
    assert_eq!(values(&net, floor), [(gentleman("Ben"), lady("Ada"))]);
    assert_eq!(found(&net, &role("lady")), [cleo]);
    assert!(found(&net, &role("gentleman")).is_empty());
}

/// A firing takes all its tokens or none: a key extractor that panics on
/// the second token, before anything moves, leaves both where they were,
/// with their ids and their index entries, and nothing in flight.
#[test]
fn a_panic_before_the_second_token_is_taken_leaves_both_in_place() {
    let armed = Arc::new(AtomicBool::new(false));
    let breaks = Arc::clone(&armed);
    let mut b = NetBuilder::new();
    let p = b.place::<u32>("P");
    let out = b.place::<Vec<u32>>("Out");
    b.keys(p, ["n"], move |n: &u32| {
        assert!(
            !(breaks.load(Ordering::SeqCst) && *n == 2),
            "the extractor broke on 2"
        );
        [KeyValue::from(i64::from(*n))]
    });
    b.seed(p, [1, 2]);
    let pair = b
        .transition(
            "Pair",
            (TakeEdge(p), TakeEdge(p)),
            (out,),
            |a: u32, c: u32| (vec![a, c],),
        )
        .id();
    let mut net = b.build().unwrap();
    let before = ids(&net, p);
    net.drain_events().for_each(drop);

    armed.store(true, Ordering::SeqCst);
    let caught = catch_unwind(AssertUnwindSafe(|| net.fire(pair)));
    assert!(caught.is_err(), "the extractor's panic comes out");
    armed.store(false, Ordering::SeqCst);
    assert_eq!(ids(&net, p), before, "neither token moved");
    assert!(!net.is_in_flight(pair));
    assert_eq!(net.drain_events().count(), 0, "no firing was told");
    for n in [1_i64, 2] {
        assert_eq!(found(&net, &Filter::new().key_in("n", [n])).len(), 1);
    }

    net.fire(pair).unwrap();
    assert_eq!(values(&net, out), [vec![1, 2]]);
}

// ----- the concurrent driver ----------------------------------------------------------

/// Every launch of a sweep takes tokens of its own: each radical ends up in
/// exactly one molecule or as one stray, whichever transition launched
/// first, in either mode.
#[test]
fn the_concurrent_driver_takes_distinct_tokens_for_every_launch() {
    let modes = [
        Mode::Sequential,
        Mode::Concurrent,
        Mode::ConcurrentWith {
            workers: NonZeroUsize::new(2).unwrap(),
        },
    ];
    for mode in modes {
        let mut b = NetBuilder::new();
        let radicals = b.place::<Radical>("Radicals");
        let molecules = b.place::<Molecule>("Molecules");
        let strays = b.place::<Radical>("Strays");
        b.seed(radicals, (1..=9).map(Radical));
        b.transition(
            "Bond",
            (TakeEdge(radicals), TakeEdge(radicals)),
            (molecules,),
            |a: Radical, c: Radical| {
                std::thread::sleep(std::time::Duration::from_millis(2));
                (Molecule(a.0, c.0),)
            },
        );
        b.transition("Stray", (TakeEdge(radicals),), (strays,), |a: Radical| (a,));
        let mut net = b.build().unwrap();
        let summary = drivers::run(&mut net, mode, Limits::default(), OnFailure::Stop).unwrap();
        assert!(summary.quiesced, "{mode:?}");
        assert_eq!(net.count(radicals.id()), 0, "{mode:?}");
        let mut seen: Vec<u32> = values(&net, molecules)
            .into_iter()
            .flat_map(|Molecule(a, c)| {
                assert!(a < c, "{mode:?}: each takes the front two left");
                [a, c]
            })
            .chain(values(&net, strays).into_iter().map(|Radical(n)| n))
            .collect();
        seen.sort_unstable();
        assert_eq!(seen, (1..=9).collect::<Vec<_>>(), "{mode:?}: each once");
    }
}

// ----- a subnet's port ----------------------------------------------------------------

/// A reactor: `Radicals`, its port, keyed by spin (even numbers up, odd
/// down) and seeded with `seed`.
fn reactor(seed: impl IntoIterator<Item = u32>) -> Net {
    let mut b = NetBuilder::new();
    let radicals = b.place::<Radical>("Radicals");
    b.port(radicals);
    b.keys(radicals, ["spin"], |r: &Radical| {
        [KeyValue::from(if r.0.is_multiple_of(2) {
            "up"
        } else {
            "down"
        })]
    });
    b.seed(radicals, seed.into_iter().map(Radical));
    b.build().expect("a reactor")
}

fn spin(spin: &str) -> Filter {
    Filter::new().key_in("spin", [spin])
}

/// The port of a subnet place is a place like any other here: two takes
/// share it, two take-wheres choose through the subnet's index, and a
/// failed firing puts both back into the port with their ids.
#[test]
fn a_subnet_s_port_may_be_the_shared_place() {
    let fail = Arc::new(AtomicBool::new(false));
    let failing = Arc::clone(&fail);
    let mut b = NetBuilder::new();
    let held = b.subnet_place("Reactor", reactor(1..=6));
    let port = b.port_of::<Radical>(held, "Radicals");
    let molecules = b.place::<Molecule>("Molecules");
    let bond = b
        .transition(
            "Bond",
            (TakeEdge(port), TakeEdge(port)),
            (molecules,),
            |a: Radical, c: Radical| (Molecule(a.0, c.0),),
        )
        .id();
    let pair = b
        .transition(
            "Pair Spins",
            (
                TakeWhereEdge(port, spin("up")),
                TakeWhereEdge(port, spin("down")),
            ),
            (molecules,),
            move |a: Radical, c: Radical| -> Result<(Molecule,), Broken> {
                if failing.load(Ordering::SeqCst) {
                    return Err(Broken);
                }
                Ok((Molecule(a.0, c.0),))
            },
        )
        .restore_tokens_on_failure()
        .id();
    let mut net = b.build().unwrap();
    for transition in [bond, pair] {
        net.disable(transition).unwrap();
    }
    fn reactor_of(net: &Net, held: petritype::SubnetRef) -> &Net {
        net.subnet(held.id()).expect("the reactor")
    }

    net.enable(bond).unwrap();
    net.fire(bond).unwrap();
    net.disable(bond).unwrap();
    assert_eq!(values(&net, molecules), [Molecule(1, 2)]);
    assert_eq!(values(&net, port), [3, 4, 5, 6].map(Radical));

    net.enable(pair).unwrap();
    fail.store(true, Ordering::SeqCst);
    let before = ids(&net, port);
    assert!(failure(net.fire(pair).unwrap_err()).restored);
    assert_eq!(
        ids(&net, port),
        [before[1], before[0], before[2], before[3]],
        "the up radical, then the down one, at the front of the port"
    );
    let inside = reactor_of(&net, held);
    let radicals = inside.place_named("Radicals").unwrap();
    assert_eq!(
        inside
            .tokens(radicals)
            .map(|(id, _)| id)
            .collect::<Vec<_>>(),
        ids(&net, port),
        "one queue, whichever net reads it"
    );
    assert_eq!(found(inside, &spin("up")), [before[1], before[3]]);
    assert_eq!(found(inside, &spin("down")), [before[0], before[2]]);

    fail.store(false, Ordering::SeqCst);
    net.fire(pair).unwrap();
    assert_eq!(values(&net, molecules), [Molecule(1, 2), Molecule(4, 3)]);
    let inside = reactor_of(&net, held);
    assert_eq!(found(inside, &spin("up")), [before[3]]);
    assert_eq!(found(inside, &spin("down")), [before[2]]);
}

// ----- the ports each edge reaches ------------------------------------------------------

/// A unit, a net a token carries: `Orders In` and `Shots Out`, both ports.
fn unit() -> Net {
    let mut b = NetBuilder::new();
    let orders = b.place::<String>("Orders In");
    let shots = b.place::<String>("Shots Out");
    b.port(orders);
    b.port(shots);
    b.build().expect("a unit")
}

/// A field of two units, and `Arm`, which takes both through two take
/// edges, the first reaching `Orders In` and the second `Shots Out`, and
/// writes `first` into the first unit and `second` into the second.
fn armoury(first: &'static str, second: &'static str) -> Net {
    let mut b = NetBuilder::new();
    let field = b.place_nested::<Net>("Field");
    let armed = b.place_nested::<Net>("Armed");
    b.seed(field, [unit(), unit()]);
    b.transition(
        "Arm",
        (
            TakeEdge(field).reaching(["Orders In"]),
            TakeEdge(field).reaching(["Shots Out"]),
        ),
        (armed,),
        move |mut a: Net, mut c: Net| -> Result<(Vec<Net>,), NetError> {
            for (unit, port) in [(&mut a, first), (&mut c, second)] {
                let place = unit.place_ref::<String>(port).expect("a port");
                unit.try_put(place, "Go".to_owned())?;
            }
            Ok((vec![a, c],))
        },
    );
    b.build().expect("an armoury")
}

/// Each edge's reach is its own: the body writes each unit at the port its
/// own edge names, and is refused at the port the other edge names.
#[test]
fn each_edge_on_a_shared_place_reaches_its_own_ports() {
    let mut net = armoury("Orders In", "Shots Out");
    net.step().unwrap().expect("Arm");
    let armed = net.place_named("Armed").unwrap();
    assert_eq!(net.count(armed), 2);

    for (first, second) in [("Shots Out", "Shots Out"), ("Orders In", "Orders In")] {
        let mut net = armoury(first, second);
        let failed = net.step().unwrap_err().into_firing().unwrap();
        match failed.error.downcast_ref::<NetError>() {
            Some(NetError::ClosedNet {
                why: ClosedRefusal::Undeclared,
                place,
                ..
            }) => {
                let refused = if first == "Shots Out" { first } else { second };
                assert_eq!(place.as_deref(), Some(refused), "{first}, {second}");
            }
            other => panic!("refused at a port its edge does not name, not {other:?}"),
        }
    }
}

// ----- two output edges to one place -------------------------------------------------

fn duplicate_output(result: Result<Net, BuildError>) -> (String, String) {
    match result {
        Err(BuildError::DuplicateOutputPlace { transition, place }) => (transition, place),
        other => panic!("a refusal of the second output edge, not {other:?}"),
    }
}

/// Two output edges from one transition to one place are refused at build,
/// whichever way the body returns (since 0.4.0; until then a positional,
/// a named or a distributing body built, and two slots landed in one
/// place). One slot of `Vec<T>` puts two tokens there.
#[test]
fn two_output_edges_to_one_place_are_refused_at_build() {
    #[derive(Outputs)]
    struct Split {
        left: u32,
        right: u32,
    }
    #[derive(Debug, Clone, PartialEq, Routed)]
    struct Leaf(u32);

    let net = |declare: fn(&mut NetBuilder, PlaceRef<u32>, PlaceRef<u32>)| {
        let mut b = NetBuilder::new();
        let inbox = b.place::<u32>("Inbox");
        let out = b.place::<u32>("Out");
        declare(&mut b, inbox, out);
        b.build()
    };
    let by_position = net(|b, inbox, out| {
        b.transition("T", (TakeEdge(inbox),), (out, out), |n: u32| (n, n + 1));
    });
    assert_eq!(duplicate_output(by_position), ("T".into(), "Out".into()));
    let by_name = net(|b, inbox, out| {
        b.transition(
            "T",
            (TakeEdge(inbox),),
            SplitEdges {
                left: out,
                right: out,
            },
            |n: u32| Split {
                left: n,
                right: n + 1,
            },
        );
    });
    assert_eq!(duplicate_output(by_name), ("T".into(), "Out".into()));
    let distributed = net(|b, inbox, out| {
        b.transition("T", (TakeEdge(inbox),), (out, out), move |n: u32| {
            Distribution::new().to(out, n)
        });
    });
    assert_eq!(duplicate_output(distributed), ("T".into(), "Out".into()));

    // A routed body: the second edge is refused before the type it routes
    // is found ambiguous.
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let leaves = b.place::<Leaf>("Leaves");
    b.transition("T", (TakeEdge(inbox),), (leaves, leaves), |n: u32| Leaf(n));
    assert_eq!(duplicate_output(b.build()), ("T".into(), "Leaves".into()));

    // A port, through its window, the same.
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let held = b.subnet_place("Reactor", reactor([]));
    let port = b.port_of::<Radical>(held, "Radicals");
    b.transition("T", (TakeEdge(inbox),), (port, port), |n: u32| {
        (Radical(n), Radical(n))
    });
    assert_eq!(duplicate_output(b.build()), ("T".into(), "Radicals".into()));

    let error = net(|b, inbox, out| {
        b.transition("T", (TakeEdge(inbox),), (out, out), |n: u32| (n, n));
    })
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "transition \"T\" has two output edges to place \"Out\"; a transition has at most one \
         output edge to a place. To put two tokens there, return them as a Vec in one slot"
    );

    // The way to put two tokens in one place: one slot, a `Vec`. And two
    // transitions to one place, or one transition to two, stay allowed.
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let other = b.place::<u32>("Other");
    let out = b.place::<u32>("Out");
    let also = b.place::<u32>("Also");
    b.seed(inbox, [1]);
    b.seed(other, [5]);
    b.transition("T", (TakeEdge(inbox),), (out, also), |n: u32| {
        (vec![n, n + 1], n + 2)
    });
    b.transition("U", (TakeEdge(other),), (out,), |n: u32| (n,));
    let mut net = b.build().unwrap();
    drivers::run(
        &mut net,
        Mode::Sequential,
        Limits::default(),
        OnFailure::Stop,
    )
    .unwrap();
    assert_eq!(values(&net, out), [1, 2, 5]);
    assert_eq!(values(&net, also), [3]);
}
