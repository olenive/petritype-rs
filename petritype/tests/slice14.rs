//! Integration tests for the fourteenth slice: `PlaceInfo::serializable`,
//! the owned `Topology`, `Snapshot::edges`, and `Serialize` on `Event`,
//! `EdgeInfo` and `EdgeKind`.
//!
//! All of it is additive, for a reader that lives on another thread — a
//! runner serving a net to a viewer, or the Bevy visualiser — and needs the
//! net's *shape* without a borrow, and its events as JSON (DESIGN §19.2). A
//! **topology** is that shape: places, transitions and edges in the core's
//! ids, with no tokens and no switches, so most of what is tested here is
//! that it agrees with the read API item for item, that `Net` and `Snapshot`
//! give the same one, and that state does not leak into it.

use petritype::{
    Command, EdgeInfo, EdgeKind, MutateEdge, Net, NetBuilder, Param, SnapshotEdge, TakeAllEdge,
    TakeEdge, Topology,
};

#[derive(Clone, Debug)]
struct Scale(u32);

/// `Inbox -Take-> Weigh -> Heavy`, reading `Config` through a snapshot edge
/// and a `Scale` parameter, and `Heavy -TakeAll-> Batch -> Batches`, with
/// `Tally` mutated by `Batch`. One of every edge kind, two transitions, a
/// parameter and an undetachable transition.
fn mixed_net() -> Net {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let config = b.place::<u32>("Config");
    let heavy = b.place::<u32>("Heavy");
    let batches = b.place::<Vec<u32>>("Batches");
    let tally = b.place::<u64>("Tally");
    b.seed(inbox, [3, 30]);
    b.seed(config, [10]);
    b.seed(tally, [0]);
    b.param(Scale(1));
    b.transition(
        "Weigh",
        (TakeEdge(inbox), SnapshotEdge(config), Param::<Scale>::new()),
        (heavy,),
        |n: u32, max: u32, scale: Scale| (Some(n * scale.0).filter(|w| *w > max),),
    );
    b.transition(
        "Batch",
        (TakeAllEdge(heavy), MutateEdge(tally)),
        (batches,),
        |all: Vec<u32>, tally: &mut u64| {
            *tally += all.len() as u64;
            (all,)
        },
    );
    b.build().expect("the net is well formed")
}

#[test]
fn an_ordinary_place_is_not_serializable() {
    let net = mixed_net();
    assert!(net.places().all(|(_, info)| !info.serializable));
    assert!(net.topology().places.iter().all(|p| !p.serializable));
}

#[cfg(feature = "serde")]
#[test]
fn a_place_marked_serializable_says_so() {
    let mut b = NetBuilder::new();
    let counts = b.place::<u32>("Counts");
    let notes = b.place::<String>("Notes");
    b.serializable(counts);
    let net = b.build().unwrap();

    let flags: Vec<_> = net.places().map(|(_, info)| info.serializable).collect();
    assert_eq!(flags, [true, false]);

    let topology = net.topology();
    assert_eq!(topology.places[0].id, counts.id());
    assert!(topology.places[0].serializable);
    assert_eq!(topology.places[1].id, notes.id());
    assert!(!topology.places[1].serializable, "Notes was not marked");
}

#[test]
fn a_topology_agrees_with_the_read_api_item_for_item() {
    let net = mixed_net();
    let topology = net.topology();

    assert_eq!(topology.places.len(), net.places().count());
    for (place, (id, info)) in topology.places.iter().zip(net.places()) {
        assert_eq!(place.id, id);
        assert_eq!(place.name, info.name);
        assert_eq!(place.type_name, info.type_name);
        assert_eq!(place.serializable, info.serializable);
    }

    assert_eq!(topology.transitions.len(), net.transitions().count());
    for (transition, (id, info)) in topology.transitions.iter().zip(net.transitions()) {
        assert_eq!(transition.id, id);
        assert_eq!(transition.name, info.name);
        assert_eq!(transition.inputs, info.inputs);
        assert_eq!(transition.params, info.params);
        assert_eq!(transition.outputs, info.outputs);
        assert_eq!(transition.detachable, info.detachable);
    }

    assert_eq!(topology.edges, net.edges().collect::<Vec<_>>());
    assert_eq!(topology.topology_counter, net.topology_counter());

    // Spot checks, so that agreement is not agreement on nothing.
    assert_eq!(topology.transitions[0].params.len(), 1);
    assert_eq!(
        topology.transitions[0].inputs[1].1,
        EdgeKind::Snapshot,
        "Config is read through a snapshot edge"
    );
    assert!(topology.transitions[0].detachable);
    assert!(!topology.transitions[1].detachable, "Batch mutates Tally");
    assert_eq!(topology.edges.len(), 6);
}

#[test]
fn index_is_id() {
    let topology = mixed_net().topology();
    for (index, place) in topology.places.iter().enumerate() {
        assert_eq!(place.id.to_string(), index.to_string());
    }
    for (index, transition) in topology.transitions.iter().enumerate() {
        assert_eq!(transition.id.to_string(), index.to_string());
    }
}

#[test]
fn tokens_and_firings_do_not_change_the_topology() {
    let mut net = mixed_net();
    let before = net.topology();

    let inbox = net.place_named("Inbox").unwrap();
    net.apply(Command::Extend {
        place: inbox,
        tokens: vec![Box::new(99_u32)],
    })
    .expect("Inbox holds u32");
    assert_eq!(net.count(inbox), 3);
    assert_eq!(net.topology(), before, "a token is state");

    net.step().unwrap();
    let batch = net.transition_named("Batch").unwrap();
    net.disable(batch).unwrap();
    assert_eq!(net.topology(), before, "a firing and a switch are state");
}

#[test]
fn a_snapshot_has_the_same_edges_and_topology_as_its_net() {
    let mut net = mixed_net();
    net.step().unwrap();
    let snapshot = net.snapshot().unwrap();

    assert_eq!(
        snapshot.edges().collect::<Vec<_>>(),
        net.edges().collect::<Vec<_>>()
    );
    assert_eq!(snapshot.topology(), net.topology());
}

#[test]
fn a_topology_can_be_held_anywhere() {
    fn assert_owned<T: Send + Sync + Clone + 'static>() {}
    assert_owned::<Topology>();

    let topology = mixed_net().topology();
    let elsewhere = std::thread::spawn({
        let topology = topology.clone();
        move || topology.places.len()
    })
    .join()
    .unwrap();
    assert_eq!(elsewhere, topology.places.len());
}

#[test]
fn edges_are_inputs_then_outputs_transition_by_transition() {
    let net = mixed_net();
    let place = |name| net.place_named(name).unwrap();
    let weigh = net.transition_named("Weigh").unwrap();
    let batch = net.transition_named("Batch").unwrap();
    let expected: Vec<EdgeInfo> = vec![
        EdgeInfo::Input {
            place: place("Inbox"),
            transition: weigh,
            kind: EdgeKind::Take,
            filter: None,
        },
        EdgeInfo::Input {
            place: place("Config"),
            transition: weigh,
            kind: EdgeKind::Snapshot,
            filter: None,
        },
        EdgeInfo::Output {
            transition: weigh,
            place: place("Heavy"),
            slot: 0,
        },
        EdgeInfo::Input {
            place: place("Heavy"),
            transition: batch,
            kind: EdgeKind::TakeAll,
            filter: None,
        },
        EdgeInfo::Input {
            place: place("Tally"),
            transition: batch,
            kind: EdgeKind::Mutate,
            filter: None,
        },
        EdgeInfo::Output {
            transition: batch,
            place: place("Batches"),
            slot: 0,
        },
    ];
    assert_eq!(net.edges().collect::<Vec<_>>(), expected);
    assert_eq!(
        net.snapshot().unwrap().edges().collect::<Vec<_>>(),
        expected
    );
}

#[cfg(feature = "serde")]
mod json {
    use super::*;
    use petritype::Event;
    use petritype::serde_json::{self, json};

    #[test]
    fn edge_kinds_are_snake_case_strings() {
        let kinds = [
            EdgeKind::Take,
            EdgeKind::TakeAll,
            EdgeKind::Snapshot,
            EdgeKind::Mutate,
        ]
        .map(|kind| serde_json::to_value(kind).unwrap());
        assert_eq!(
            kinds,
            [
                json!("take"),
                json!("take_all"),
                json!("snapshot"),
                json!("mutate")
            ]
        );
    }

    #[test]
    fn edges_are_tagged_by_direction() {
        let net = mixed_net();
        let edges: Vec<_> = net.edges().collect();

        // Inbox is place 0, Weigh transition 0, Heavy place 2.
        assert_eq!(
            serde_json::to_value(&edges[0]).unwrap(),
            json!({"edge": "input", "place": 0, "transition": 0, "kind": "take"}),
        );
        assert_eq!(
            serde_json::to_value(&edges[2]).unwrap(),
            json!({"edge": "output", "transition": 0, "place": 2, "slot": 0}),
        );
    }

    #[test]
    fn a_fired_event_is_tagged_and_speaks_in_bare_ids() {
        let mut b = NetBuilder::new();
        let inbox = b.place::<u32>("Inbox");
        let out = b.place::<u32>("Out");
        b.seed(inbox, [21]);
        b.transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,));
        let mut net = b.build().unwrap();

        let taken = net.tokens(inbox.id()).next().unwrap().0;
        net.step().unwrap();
        let produced = net.tokens(out.id()).next().unwrap().0;
        let events: Vec<Event> = net.drain_events().collect();
        assert_eq!(events.len(), 1);

        let value = serde_json::to_value(&events[0]).unwrap();
        assert_eq!(
            value,
            json!({
                "event": "fired",
                "transition": 0,
                "consumed": [[0, taken]],
                "produced": [[1, produced]],
            }),
        );
        // Bare numbers, not wrappers: `[[0, 0]]` and `[[1, 1]]`.
        assert_eq!(
            value["consumed"][0][1].as_u64(),
            Some(taken.to_string().parse().unwrap())
        );
        assert!(value["produced"][0][1].is_u64());
    }

    #[test]
    fn a_param_change_carries_only_the_type_name() {
        let mut net = mixed_net();
        net.set_param(Scale(2)).unwrap();
        let event = net.drain_events().next().unwrap();
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            json!({"event": "param_changed", "type_name": std::any::type_name::<Scale>()}),
        );
    }

    #[test]
    fn a_topology_serialises_with_ids_and_string_kinds() {
        let mut b = NetBuilder::new();
        let inbox = b.place::<u32>("Inbox");
        let out = b.place::<u32>("Out");
        b.serializable(inbox);
        b.transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,));
        let net = b.build().unwrap();

        assert_eq!(
            serde_json::to_value(net.topology()).unwrap(),
            json!({
                "places": [
                    {"id": 0, "name": "Inbox", "type_name": "u32", "serializable": true},
                    {"id": 1, "name": "Out", "type_name": "u32", "serializable": false},
                ],
                "transitions": [{
                    "id": 0,
                    "name": "Double",
                    "inputs": [[0, "take"]],
                    "params": [],
                    "outputs": [1],
                    "detachable": true,
                }],
                "edges": [
                    {"edge": "input", "place": 0, "transition": 0, "kind": "take"},
                    {"edge": "output", "transition": 0, "place": 1, "slot": 0},
                ],
                "topology_counter": 0,
            }),
        );
    }
}
