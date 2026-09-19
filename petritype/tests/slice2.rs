//! Integration tests for the second slice: edge kinds, bodies over tuples,
//! `transition()`, the transition read API, and the net id stamped into every
//! place handle.
//!
//! Nothing here fires: a transition is *declared* in this slice, and the
//! checks are the ones the compiler and `build()` can make before anything
//! runs.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use petritype::{
    BuildError, EdgeKind, MutateEdge, NetBuilder, PlaceRef, SnapshotEdge, TakeAllEdge, TakeEdge,
    TransitionId,
};

#[derive(Debug, Clone, PartialEq)]
struct Parcel {
    id: u32,
    weight: f32,
}

#[derive(Debug, Clone)]
struct Config {
    max_weight: f32,
}

/// A token type that is `Clone` but lives in a place that gave cloning up, so
/// that a snapshot of it is a build error rather than a compile error.
#[derive(Debug, Clone)]
struct Connection {
    host: String,
}

#[derive(Debug)]
struct Refused;

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("refused")
    }
}

impl std::error::Error for Refused {}

fn hash_of<T: Hash>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// The example from DESIGN §5, built and read back.
#[test]
fn the_weigh_example_is_wired_as_declared() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<Parcel>("Inbox");
    let heavy = b.place::<Parcel>("Heavy");
    let light = b.place::<Parcel>("Light");
    let count = b.place::<u32>("Count");
    let config = b.place::<Config>("Config");
    b.seed(
        inbox,
        [
            Parcel { id: 1, weight: 3.0 },
            Parcel {
                id: 2,
                weight: 30.0,
            },
        ],
    );
    b.seed(config, [Config { max_weight: 10.0 }]);

    let weigh = b
        .transition(
            "Weigh",
            (TakeAllEdge(inbox), SnapshotEdge(config)),
            (heavy, light, count),
            |parcels: Vec<Parcel>, cfg: Config| {
                let (h, l): (Vec<_>, Vec<_>) =
                    parcels.into_iter().partition(|p| p.weight > cfg.max_weight);
                let n = (h.len() + l.len()) as u32;
                (h, l, Some(n))
            },
        )
        .id();

    let net = b
        .build()
        .expect("distinct names, no snapshot of a no-clone place");

    let transitions: Vec<_> = net.transitions().collect();
    assert_eq!(transitions.len(), 1);
    let (id, info) = transitions[0];
    assert_eq!(id, weigh);
    assert_eq!(info.name, "Weigh");
    assert_eq!(
        info.inputs,
        [
            (inbox.id(), EdgeKind::TakeAll),
            (config.id(), EdgeKind::Snapshot),
        ],
        "input edges keep declaration order, which is the body's parameter order"
    );
    assert_eq!(
        info.outputs,
        [heavy.id(), light.id(), count.id()],
        "output places keep declaration order, which is the returned tuple's order"
    );
    assert!(info.enabled);
    assert_eq!(net.transition_named("Weigh"), Some(weigh));
    assert_eq!(net.transition_named("Nowhere"), None);
}

#[test]
fn transition_ids_are_handed_out_in_declaration_order() {
    let mut b = NetBuilder::new();
    let numbers = b.place::<u32>("Numbers");

    let first = b.transition("First", (), (numbers,), || (1_u32,)).id();
    let second = b.transition("Second", (), (numbers,), || (2_u32,)).id();
    let third = b.transition("Third", (), (numbers,), || (3_u32,)).id();
    assert_eq!([first, second, third], [first, second, third]);

    let net = b.build().unwrap();
    let ids: Vec<TransitionId> = net.transitions().map(|(id, _)| id).collect();
    let names: Vec<&str> = net.transitions().map(|(_, info)| info.name).collect();
    assert_eq!(ids, [first, second, third]);
    assert_eq!(names, ["First", "Second", "Third"]);
    assert_ne!(first, second);
}

/// One body parameter per edge kind, each with the type that kind produces.
#[test]
fn every_edge_kind_is_accepted_with_its_own_parameter_type() {
    let mut b = NetBuilder::new();
    let one = b.place::<u32>("One");
    let many = b.place::<u32>("Many");
    let read = b.place::<Config>("Read");
    let live = b.place::<u32>("Live");

    b.transition(
        "Every kind",
        (
            TakeEdge(one),
            TakeAllEdge(many),
            SnapshotEdge(read),
            MutateEdge(live),
        ),
        (),
        |taken: u32, batch: Vec<u32>, cfg: Config, live: &mut u32| {
            *live += taken + batch.iter().sum::<u32>() + cfg.max_weight as u32;
        },
    );

    let net = b.build().unwrap();
    let (_, info) = net.transitions().next().unwrap();
    let kinds: Vec<EdgeKind> = info.inputs.iter().map(|(_, kind)| *kind).collect();
    assert_eq!(
        kinds,
        [
            EdgeKind::Take,
            EdgeKind::TakeAll,
            EdgeKind::Snapshot,
            EdgeKind::Mutate
        ]
    );
    assert!(kinds[0].consumes() && kinds[1].consumes());
    assert!(!kinds[2].consumes() && !kinds[3].consumes());
}

#[test]
fn a_body_may_have_no_inputs_or_twelve() {
    let mut b = NetBuilder::new();
    let p = b.place::<u32>("Numbers");
    // Twelve *distinct* places: a transition reads one place through one
    // edge, so twelve inputs need twelve places.
    let (p0, p1, p2, p3) = (
        b.place::<u32>("P0"),
        b.place::<u32>("P1"),
        b.place::<u32>("P2"),
        b.place::<u32>("P3"),
    );
    let (p4, p5, p6, p7) = (
        b.place::<u32>("P4"),
        b.place::<u32>("P5"),
        b.place::<u32>("P6"),
        b.place::<u32>("P7"),
    );
    let (p8, p9, p10, p11) = (
        b.place::<u32>("P8"),
        b.place::<u32>("P9"),
        b.place::<u32>("P10"),
        b.place::<u32>("P11"),
    );

    b.transition("None", (), (p,), || (0_u32,));
    b.transition(
        "Twelve",
        (
            TakeEdge(p0),
            TakeEdge(p1),
            TakeEdge(p2),
            TakeEdge(p3),
            TakeEdge(p4),
            TakeEdge(p5),
            TakeEdge(p6),
            TakeEdge(p7),
            TakeEdge(p8),
            TakeEdge(p9),
            TakeEdge(p10),
            TakeEdge(p11),
        ),
        (p,),
        #[allow(clippy::too_many_arguments)]
        |a: u32,
         c: u32,
         d: u32,
         e: u32,
         f: u32,
         g: u32,
         h: u32,
         i: u32,
         j: u32,
         k: u32,
         l: u32,
         m: u32| { (a + c + d + e + f + g + h + i + j + k + l + m,) },
    );

    let net = b.build().unwrap();
    let arities: Vec<usize> = net
        .transitions()
        .map(|(_, info)| info.inputs.len())
        .collect();
    assert_eq!(arities, [0, 12]);
}

#[test]
fn a_body_may_have_no_outputs_or_twelve() {
    let mut b = NetBuilder::new();
    let p = b.place::<u32>("Numbers");
    let sink = b.place::<u32>("Sink");

    // Zero outputs: the body must return `()`, never a discarded value.
    b.transition("Sink it", (TakeEdge(p),), (), |_n: u32| {});
    b.transition(
        "Fan out",
        (TakeEdge(p),),
        (
            sink, sink, sink, sink, sink, sink, sink, sink, sink, sink, sink, sink,
        ),
        |n: u32| (n, n, n, n, n, n, n, n, n, n, n, n),
    );

    let net = b.build().unwrap();
    let widths: Vec<usize> = net
        .transitions()
        .map(|(_, info)| info.outputs.len())
        .collect();
    assert_eq!(widths, [0, 12]);
}

/// A slot is one token, zero or one, or many — decided by the place's type.
#[test]
fn a_slot_may_be_one_token_maybe_one_or_many() {
    let mut b = NetBuilder::new();
    let numbers = b.place::<u32>("Numbers");
    let maybe = b.place::<Option<u32>>("Maybe");
    let batches = b.place::<Vec<u32>>("Batches");

    b.transition("Slots", (), (numbers, numbers, numbers), || {
        (1_u32, None::<u32>, vec![2_u32, 3])
    });
    // A place whose type is itself an `Option` or a `Vec` takes such a value
    // as *one* token: the place's type is what the slot is matched against.
    b.transition("Containers", (), (maybe, batches), || {
        (Some(1_u32), vec![2_u32, 3])
    });
    // Many tokens into a `Vec<u32>` place therefore need a `Vec<Vec<u32>>`.
    b.transition("Many batches", (), (batches,), || {
        (vec![vec![1_u32], vec![2]],)
    });

    let net = b.build().unwrap();
    assert_eq!(net.transitions().count(), 3);
}

#[test]
fn a_body_may_return_a_result() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<Parcel>("Inbox");
    let count = b.place::<u32>("Count");

    b.transition(
        "Fallible",
        (TakeEdge(inbox),),
        (count,),
        |parcel: Parcel| -> Result<(u32,), Refused> {
            if parcel.weight < 0.0 {
                Err(Refused)
            } else {
                Ok((parcel.id,))
            }
        },
    );
    // A `Result` with no outputs at all is still a `Result`.
    b.transition("Nothing or else", (), (), || -> Result<(), Refused> {
        Ok(())
    });

    let net = b.build().unwrap();
    assert_eq!(net.transitions().count(), 2);
}

/// Bodies are `FnMut` and must be `Send + Sync`, so private state is an
/// `Arc`, a `Mutex` or an atomic — and a captured counter is allowed to
/// change between firings.
#[test]
fn a_body_may_capture_state() {
    let fired = Arc::new(AtomicU32::new(0));
    let seen = Arc::new(std::sync::Mutex::new(Vec::<u32>::new()));

    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");

    let counter = Arc::clone(&fired);
    let log = Arc::clone(&seen);
    let mut owned_state = 0_u32;
    b.transition("Counting", (TakeEdge(inbox),), (out,), move |n: u32| {
        counter.fetch_add(1, Ordering::Relaxed);
        log.lock().expect("not poisoned").push(n);
        owned_state += n;
        (owned_state,)
    });

    let net = b.build().unwrap();
    assert_eq!(net.transitions().count(), 1);
    // Nothing has fired, so the captured state is untouched.
    assert_eq!(fired.load(Ordering::Relaxed), 0);
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn duplicate_transition_names_are_a_build_error() {
    let mut b = NetBuilder::new();
    let p = b.place::<u32>("Numbers");
    b.transition("Same", (TakeEdge(p),), (), |_n: u32| {});
    b.transition("Same", (TakeEdge(p),), (), |_n: u32| {});

    match b.build() {
        Err(BuildError::DuplicateTransitionName { name }) => assert_eq!(name, "Same"),
        other => panic!("expected a duplicate transition name error, got {other:?}"),
    }
}

/// The type is `Clone`; the *place* is what gave cloning up, which is why
/// this is caught at `build()` rather than by the compiler.
#[test]
fn a_snapshot_of_a_no_clone_place_is_a_build_error() {
    let mut b = NetBuilder::new();
    let connections = b.place_no_clone::<Connection>("Connections");
    let names = b.place::<String>("Names");
    b.transition(
        "Peek",
        (SnapshotEdge(connections),),
        (names,),
        |c: Connection| (c.host,),
    );

    match b.build() {
        Err(BuildError::SnapshotOfNoClonePlace { transition, place }) => {
            assert_eq!(transition, "Peek");
            assert_eq!(place, "Connections");
        }
        other => panic!("expected a snapshot-of-no-clone error, got {other:?}"),
    }
}

/// A take edge consumes rather than clones, so it is fine on such a place.
#[test]
fn a_no_clone_place_can_still_be_taken_from() {
    let mut b = NetBuilder::new();
    let connections = b.place_no_clone::<Connection>("Connections");
    let names = b.place::<String>("Names");
    b.transition(
        "Use it up",
        (TakeEdge(connections),),
        (names,),
        |c: Connection| (c.host,),
    );

    let net = b.build().expect("taking needs no cloner");
    assert_eq!(net.transitions().count(), 1);
}

/// Declaring a transition is setup, like seeding: no events, no ticks.
#[test]
fn declaring_transitions_leaves_the_counters_at_zero() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [1, 2]);
    b.transition("Copy", (TakeEdge(inbox),), (out,), |n: u32| (n,));
    let mut net = b.build().unwrap();

    assert_eq!(net.state_counter(), 0);
    assert_eq!(
        net.topology_counter(),
        0,
        "the wiring is where the net starts"
    );
    assert_eq!(net.fired_count(), 0);
    assert_eq!(net.drain_events().count(), 0);
}

#[test]
#[should_panic(expected = "but this is net")]
fn a_place_handle_from_another_net_cannot_be_an_edge() {
    let mut other = NetBuilder::new();
    let elsewhere = other.place::<u32>("Elsewhere");

    let mut b = NetBuilder::new();
    let here = b.place::<u32>("Here");
    b.transition("Wrong net", (TakeEdge(elsewhere),), (here,), |n: u32| (n,));
}

#[test]
#[should_panic(expected = "but this is net")]
fn a_place_handle_from_another_net_cannot_be_an_output() {
    let mut other = NetBuilder::new();
    let elsewhere = other.place::<u32>("Elsewhere");

    let mut b = NetBuilder::new();
    let here = b.place::<u32>("Here");
    b.transition("Wrong net", (TakeEdge(here),), (elsewhere,), |n: u32| (n,));
}

/// Two nets number their places from zero, so the index alone is not an
/// identity: the stamp is what tells two handles apart.
#[test]
fn handles_with_the_same_index_in_different_nets_differ() {
    let mut a = NetBuilder::new();
    let here: PlaceRef<u32> = a.place::<u32>("First");

    let mut b = NetBuilder::new();
    let there: PlaceRef<u32> = b.place::<u32>("First");

    assert_eq!(here.id(), there.id(), "same index");
    assert_ne!(here.net(), there.net(), "different net");
    assert_ne!(here, there);
    assert_ne!(hash_of(&here), hash_of(&there));
}

#[test]
fn every_net_has_its_own_id() {
    let first = NetBuilder::new().build().unwrap();
    let second = NetBuilder::new().build().unwrap();
    assert_ne!(first.id(), second.id());

    let mut b = NetBuilder::new();
    let p = b.place::<u32>("Numbers");
    let net = b.build().unwrap();
    assert_eq!(p.net(), net.id(), "handles carry the id of their own net");
}
