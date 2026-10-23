//! Integration tests for the eighth slice: the net read back as edges, and
//! the two opt-in checks `build()` makes.
//!
//! `edges()` is the wiring one edge at a time, which is what a diagram draws.
//! `assert_acyclic` and `assert_no_source_transitions` are the questions the
//! type system cannot ask: does the token flow loop, and does every firing
//! take something out of the net. Both are opt-in because a feedback loop and
//! a generator are perfectly good nets. Together they are a static proof
//! that the net quiesces, and the tests below are also the record of what
//! each one means by an edge.

use petritype::{
    BuildError, EdgeInfo, EdgeKind, MutateEdge, Net, NetBuilder, SnapshotEdge, TakeAllEdge,
    TakeEdge,
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

/// The DESIGN §5 net, plus a `Rework` transition that takes heavy parcels and
/// sends them back to `Inbox`: the cycle `Weigh -> Heavy -> Rework -> Inbox`.
///
/// The handles stay inside: every test below asks the built net or the error
/// for names, which is what the check reports in.
fn reworking_net() -> NetBuilder {
    let mut b = NetBuilder::new();
    let inbox = b.place::<Parcel>("Inbox");
    let heavy = b.place::<Parcel>("Heavy");
    let light = b.place::<Parcel>("Light");
    let count = b.place::<u32>("Count");
    let config = b.place::<Config>("Config");

    b.transition(
        "Weigh",
        (TakeAllEdge(inbox), SnapshotEdge(config)),
        (heavy, light, count),
        |parcels: Vec<Parcel>, cfg: Config| {
            let (h, l): (Vec<_>, Vec<_>) =
                parcels.into_iter().partition(|p| p.weight > cfg.max_weight);
            let n = (h.len() + l.len()) as u32;
            (h, l, Some(n))
        },
    );
    b.transition("Rework", (TakeEdge(heavy),), (inbox,), |p: Parcel| {
        (Parcel {
            id: p.id,
            weight: p.weight / 2.0,
        },)
    });
    b
}

/// The names of a net's edges, with the direction and the kind or slot spelled
/// out, so that a failing assertion reads as the diagram it describes.
fn described(net: &Net) -> Vec<String> {
    let name = |id| {
        net.places()
            .find(|(place, _)| *place == id)
            .map(|(_, info)| info.name.to_owned())
            .expect("the edge names a place of this net")
    };
    let transition_name = |id| {
        net.transitions()
            .find(|(transition, _)| *transition == id)
            .map(|(_, info)| info.name.to_owned())
            .expect("the edge names a transition of this net")
    };
    net.edges()
        .map(|edge| match edge {
            EdgeInfo::Input {
                place,
                transition,
                kind,
                ..
            } => format!(
                "{} -{kind:?}-> {}",
                name(place),
                transition_name(transition)
            ),
            EdgeInfo::Output {
                transition,
                place,
                slot,
            } => format!("{} -{slot}-> {}", transition_name(transition), name(place)),
            _ => unreachable!("EdgeInfo has two variants"),
        })
        .collect()
}

/// The DESIGN §5 net, edge by edge: inputs before outputs, both in
/// declaration order, with the kind of each input and the slot of each output.
#[test]
fn the_weigh_net_reads_back_as_its_edges() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<Parcel>("Inbox");
    let heavy = b.place::<Parcel>("Heavy");
    let light = b.place::<Parcel>("Light");
    let count = b.place::<u32>("Count");
    let config = b.place::<Config>("Config");

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
    let net = b.build().unwrap();

    assert_eq!(
        net.edges().collect::<Vec<_>>(),
        [
            EdgeInfo::Input {
                place: inbox.id(),
                transition: weigh,
                kind: EdgeKind::TakeAll,
                filter: None,
            },
            EdgeInfo::Input {
                place: config.id(),
                transition: weigh,
                kind: EdgeKind::Snapshot,
                filter: None,
            },
            EdgeInfo::Output {
                transition: weigh,
                place: heavy.id(),
                slot: 0,
            },
            EdgeInfo::Output {
                transition: weigh,
                place: light.id(),
                slot: 1,
            },
            EdgeInfo::Output {
                transition: weigh,
                place: count.id(),
                slot: 2,
            },
        ],
    );
}

/// Places are nodes, not edges: a net with nowhere for a token to go has none.
#[test]
fn a_net_with_no_transitions_has_no_edges() {
    assert_eq!(NetBuilder::new().build().unwrap().edges().count(), 0);

    let mut b = NetBuilder::new();
    let _ = b.place::<Parcel>("Inbox");
    let _ = b.place::<Parcel>("Heavy");
    let net = b.build().unwrap();

    assert_eq!(net.edges().count(), 0);
    assert_eq!(net.places().count(), 2, "the places are still there");
}

/// The cycle is reported as node names, from the node the walk closed on round
/// to itself, so the first and the last name are the same one.
#[test]
fn a_loop_in_the_token_flow_is_refused() {
    let mut b = reworking_net();
    b.assert_acyclic();

    let error = b.build().expect_err("Rework feeds Weigh's input place");
    assert!(
        matches!(error, BuildError::Cycle { ref path } if path == &["Weigh", "Heavy", "Rework", "Inbox", "Weigh"]),
        "unexpected error: {error:?}",
    );
    assert_eq!(
        error.to_string(),
        "the net's token flow has a cycle: Weigh -> Heavy -> Rework -> Inbox -> Weigh",
    );
}

/// The same net without the assertion: a feedback loop is a net, not a
/// mistake, so nothing but asking complains about one.
#[test]
fn the_same_loop_builds_without_the_assertion() {
    let net = reworking_net().build().unwrap();

    assert_eq!(
        described(&net),
        [
            "Inbox -TakeAll-> Weigh",
            "Config -Snapshot-> Weigh",
            "Weigh -0-> Heavy",
            "Weigh -1-> Light",
            "Weigh -2-> Count",
            "Heavy -Take-> Rework",
            "Rework -0-> Inbox",
        ],
    );
}

/// A loop closed by a read edge is not a cycle: a snapshot and a mutation
/// move no tokens, so nothing goes round.
#[test]
fn a_loop_through_a_read_edge_only_is_not_a_cycle() {
    for read_edge_is_a_snapshot in [true, false] {
        let mut b = NetBuilder::new();
        b.assert_acyclic();
        let inbox = b.place::<Parcel>("Inbox");
        let heavy = b.place::<Parcel>("Heavy");

        b.transition("Weigh", (TakeEdge(inbox),), (heavy,), |p: Parcel| (p,));
        // Rework reads Heavy and writes Inbox, which would close the loop if
        // reading moved anything.
        if read_edge_is_a_snapshot {
            b.transition("Rework", (SnapshotEdge(heavy),), (inbox,), |p: Parcel| (p,));
        } else {
            b.transition(
                "Rework",
                (MutateEdge(heavy),),
                (inbox,),
                |p: &mut Parcel| {
                    p.weight /= 2.0;
                    (Parcel {
                        id: p.id,
                        weight: p.weight,
                    },)
                },
            );
        }

        let net = b
            .build()
            .expect("a read edge carries no token, so it closes no cycle");
        assert_eq!(net.edges().count(), 4, "the edges are all still drawn");
    }
}

/// The shortest cycle there is: one transition that puts a token back where it
/// took it from. Two nodes, so three names in the path.
#[test]
fn a_transition_that_feeds_its_own_input_place_is_a_cycle() {
    let mut b = NetBuilder::new();
    b.assert_acyclic();
    let inbox = b.place::<u32>("Inbox");
    b.transition("Loop", (TakeEdge(inbox),), (inbox,), |n: u32| (n + 1,));

    let error = b
        .build()
        .expect_err("Loop writes to the place it takes from");
    assert!(
        matches!(error, BuildError::Cycle { ref path } if path == &["Loop", "Inbox", "Loop"]),
        "unexpected error: {error:?}",
    );
}

/// A transition with no inputs takes nothing out of the net, so firing it
/// leaves it as fireable as it was.
#[test]
fn a_transition_with_no_inputs_is_a_source() {
    let mut b = NetBuilder::new();
    b.assert_no_source_transitions();
    let out = b.place::<u32>("Out");
    b.transition("Spawn", (), (out,), || (1_u32,));

    let error = b.build().expect_err("Spawn consumes nothing");
    assert!(
        matches!(error, BuildError::SourceTransition { ref transition } if transition == "Spawn"),
        "unexpected error: {error:?}",
    );
}

/// The same generator without the assertion: a net that produces tokens from
/// nothing is a net, and is bounded while it runs instead.
#[test]
fn a_source_builds_without_the_assertion() {
    let mut b = NetBuilder::new();
    let out = b.place::<u32>("Out");
    b.transition("Spawn", (), (out,), || (1_u32,));
    let mut net = b.build().unwrap();

    let spawn = net.transition_named("Spawn").unwrap();
    assert_eq!(net.fireable(), [spawn], "and it fires with nothing to read");
    assert_eq!(net.step().unwrap(), Some(spawn));
    assert_eq!(net.fireable(), [spawn], "for ever");
}

/// Reading is not consuming: a transition whose every input is a read edge is
/// a source like one with no inputs at all.
#[test]
fn a_transition_that_only_reads_is_a_source() {
    let mut b = NetBuilder::new();
    b.assert_no_source_transitions();
    let config = b.place::<Config>("Config");
    let count = b.place::<u32>("Count");
    let log = b.place::<u32>("Log");
    b.transition(
        "Watch",
        (SnapshotEdge(config), MutateEdge(count)),
        (log,),
        |cfg: Config, seen: &mut u32| {
            *seen += 1;
            (*seen + cfg.max_weight as u32,)
        },
    );

    let error = b.build().expect_err("Watch consumes nothing");
    assert!(
        matches!(error, BuildError::SourceTransition { ref transition } if transition == "Watch"),
        "unexpected error: {error:?}",
    );
}

/// The pair, on a net that satisfies both: every transition consumes, and the
/// token flow runs one way. Such a net quiesces, which the test also shows.
#[test]
fn a_consuming_dag_passes_both_assertions() {
    let mut b = NetBuilder::new();
    // Both on one line: each returns `&mut Self`.
    b.assert_acyclic().assert_no_source_transitions();
    let inbox = b.place::<Parcel>("Inbox");
    let heavy = b.place::<Parcel>("Heavy");
    let light = b.place::<Parcel>("Light");
    let config = b.place::<Config>("Config");
    let shipped = b.place::<u32>("Shipped");
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

    b.transition(
        "Weigh",
        (TakeEdge(inbox), SnapshotEdge(config)),
        (heavy, light),
        |p: Parcel, cfg: Config| {
            let heavy = p.weight > cfg.max_weight;
            (heavy.then_some(p.clone()), (!heavy).then_some(p))
        },
    );
    b.transition("Ship", (TakeEdge(heavy),), (shipped,), |p: Parcel| (p.id,));

    let mut net = b.build().expect("acyclic, and every transition consumes");
    while net.step().expect("no body in this net fails").is_some() {}

    assert_eq!(net.count(inbox.id()), 0);
    assert_eq!(net.count(light.id()), 1);
    assert_eq!(net.count(shipped.id()), 1);
    assert_eq!(
        net.count(config.id()),
        1,
        "the snapshot left the config where it was",
    );
}

/// Two ways to the same node are not a loop: the walk has to tell a node it
/// has finished with from one it is still standing on.
#[test]
fn a_diamond_is_not_a_cycle() {
    let mut b = NetBuilder::new();
    b.assert_acyclic().assert_no_source_transitions();
    let inbox = b.place::<u32>("Inbox");
    let left = b.place::<u32>("Left");
    let right = b.place::<u32>("Right");
    let out = b.place::<u32>("Out");

    b.transition("Split", (TakeEdge(inbox),), (left, right), |n: u32| (n, n));
    // Join is reached from Split twice over, once through each branch.
    b.transition(
        "Join",
        (TakeEdge(left), TakeEdge(right)),
        (out,),
        |l: u32, r: u32| (l + r,),
    );

    let net = b
        .build()
        .expect("two paths to Join are two edges, not a loop");
    assert_eq!(net.edges().count(), 6);
}

/// The checks that always run come first: a net with two mistakes is told
/// about the one the type system was closest to catching.
#[test]
fn a_duplicate_name_is_reported_before_a_cycle() {
    let mut b = NetBuilder::new();
    b.assert_acyclic().assert_no_source_transitions();
    let inbox = b.place::<u32>("Inbox");
    let _twice = b.place::<u32>("Inbox");
    b.transition("Loop", (TakeEdge(inbox),), (inbox,), |n: u32| (n + 1,));

    let error = b.build().expect_err("the net has both problems");
    assert!(
        matches!(error, BuildError::DuplicatePlaceName { ref name } if name == "Inbox"),
        "unexpected error: {error:?}",
    );
}
