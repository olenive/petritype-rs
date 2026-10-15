//! Integration tests for binding by name (DESIGN §4.4;
//! `dev-docs/BINDING_BY_NAME.md`).
//!
//! A struct with one field per input derives `Inputs`, and the struct of
//! edges it generates is one input position: the body takes the struct, and
//! the net sees the fields' edges, in field order, as if they had been
//! written in the tuple one by one. What is tested here is that promise —
//! the order, the kinds, the borrows, the restore, the build checks, the
//! read API — and the by-name part of it: the fields are matched by name,
//! whatever order the literal writes them in.

use petritype::{
    BuildError, DrainEdge, EdgeInfo, EdgeKind, Filter, Inputs, MutateEdge, MutateWhereEdge, Net,
    NetBuilder, NetError, Param, PlaceRef, SnapshotEdge, SurveyEdge, TakeAllEdge, TakeEdge,
    TakeWhereEdge, TransitionId,
};

/// A transfer between two accounts of one type: the two fields a tuple
/// could swap without the compiler noticing.
#[derive(Inputs)]
struct Transfer {
    from: u32,
    to: u32,
    note: String,
}

/// Every owned shape a field can have, each from more than one kind.
#[derive(Inputs)]
struct Shapes {
    one: u32,
    every: Vec<u32>,
    copy: String,
    chosen: (String, u32),
    rate: Rate,
}

#[derive(Debug, Clone, PartialEq)]
struct Rate(u32);

/// Fields that borrow their tokens where they lie, beside an owned one.
#[derive(Inputs)]
struct Tally<'a> {
    vote: String,
    total: &'a mut u32,
    leader: &'a mut (String, u32),
}

/// A wiring renamed by the attribute.
#[derive(Inputs)]
#[binding(edges = "Wiring")]
struct Renamed {
    value: u32,
}

/// The values in a place, in queue order.
fn values<T: Clone + 'static>(net: &Net, place: PlaceRef<T>) -> Vec<T> {
    net.tokens(place.id())
        .filter_map(|(token, _)| net.token::<T>(place.id(), token).cloned())
        .collect()
}

fn s(text: &str) -> String {
    text.to_owned()
}

/// The info of one transition.
fn inputs_of(net: &Net, transition: TransitionId) -> Vec<(petritype::PlaceId, EdgeKind)> {
    net.transitions()
        .find(|(id, _)| *id == transition)
        .expect("the transition exists")
        .1
        .inputs
        .to_vec()
}

// ----- by name --------------------------------------------------------------

/// A literal may name the fields in any order: each edge goes to the field
/// it names, and the net lists the edges in the order the fields are
/// declared, which is the order of the edges whatever the literal says.
#[test]
fn fields_are_bound_by_name_and_listed_in_declaration_order() {
    let mut b = NetBuilder::new();
    let checking = b.place::<u32>("Checking");
    let savings = b.place::<u32>("Savings");
    let notes = b.place::<String>("Notes");
    let log = b.place::<String>("Log");
    b.seed(checking, [100]);
    b.seed(savings, [5]);
    b.seed(notes, [s("rent")]);
    let transfer = b
        .transition(
            "Transfer",
            (TransferEdges {
                note: TakeEdge(notes),
                to: TakeEdge(savings),
                from: TakeEdge(checking),
            },),
            (log,),
            |t: Transfer| (format!("{} from {} to {}", t.note, t.from, t.to),),
        )
        .id();
    let mut net = b.build().unwrap();

    assert_eq!(
        inputs_of(&net, transfer),
        [
            (checking.id(), EdgeKind::Take),
            (savings.id(), EdgeKind::Take),
            (notes.id(), EdgeKind::Take),
        ],
        "declaration order, not the literal's"
    );
    net.step().unwrap();
    assert_eq!(values(&net, log), ["rent from 100 to 5"]);
}

/// Each field takes any kind that reads its shape, a parameter included,
/// and the body is handed what each kind hands over.
#[test]
fn every_kind_fills_a_field_of_its_shape() {
    let mut b = NetBuilder::new();
    let ones = b.place::<u32>("Ones");
    let many = b.place::<u32>("Many");
    let names = b.place::<String>("Names");
    let pairs = b.place::<(String, u32)>("Pairs");
    let out = b.place::<String>("Out");
    b.keys(pairs, ["name"], |(name, _): &(String, u32)| {
        [name.as_str().into()]
    });
    b.param(Rate(3));
    b.seed(ones, [7]);
    b.seed(many, [1, 2]);
    b.seed(names, [s("snap")]);
    b.seed(pairs, [(s("a"), 1), (s("b"), 2)]);
    let describe = |x: Shapes| {
        (format!(
            "{} {:?} {} {:?} {}",
            x.one, x.every, x.copy, x.chosen, x.rate.0
        ),)
    };
    let first = b
        .transition(
            "Take",
            (ShapesEdges {
                one: TakeEdge(ones),
                every: TakeAllEdge(many),
                copy: SnapshotEdge(names),
                chosen: TakeWhereEdge(pairs, Filter::new().key_in("name", ["b"])),
                rate: Param::new(),
            },),
            (out,),
            describe,
        )
        .id();
    let mut net = b.build().unwrap();
    assert_eq!(
        inputs_of(&net, first),
        [
            (ones.id(), EdgeKind::Take),
            (many.id(), EdgeKind::TakeAll),
            (names.id(), EdgeKind::Snapshot),
            (pairs.id(), EdgeKind::TakeWhere),
        ]
    );
    net.step().unwrap();
    assert_eq!(values(&net, out), [r#"7 [1, 2] snap ("b", 2) 3"#]);
    assert_eq!(values(&net, pairs), [(s("a"), 1)], "the filter chose b");

    // The same struct, read without consuming: a survey and a drain fill
    // the `Vec`, possibly empty.
    let mut b = NetBuilder::new();
    let ones = b.place::<u32>("Ones");
    let many = b.place::<u32>("Many");
    let names = b.place::<String>("Names");
    let pairs = b.place::<(String, u32)>("Pairs");
    let out = b.place::<String>("Out");
    b.param(Rate(1));
    b.seed(ones, [7]);
    b.seed(names, [s("snap")]);
    b.seed(pairs, [(s("a"), 1)]);
    b.transition(
        "Look",
        (ShapesEdges {
            one: TakeEdge(ones),
            every: SurveyEdge(many),
            copy: SnapshotEdge(names),
            chosen: SnapshotEdge(pairs),
            rate: Param::new(),
        },),
        (out,),
        describe,
    );
    let mut net = b.build().unwrap();
    net.step().unwrap();
    assert_eq!(values(&net, out), [r#"7 [] snap ("a", 1) 1"#]);
}

/// A field of type `&'a mut T` borrows its token where it lies, as a
/// mutating edge always does: the token keeps its id, the transition runs
/// inline, and a keyed one is chosen by its filter.
#[test]
fn a_mutable_field_borrows_its_token_in_place() {
    let mut b = NetBuilder::new();
    let votes = b.place::<String>("Votes");
    let total = b.place::<u32>("Total");
    let candidates = b.place::<(String, u32)>("Candidates");
    let counted = b.place::<String>("Counted");
    b.keys(candidates, ["name"], |(name, _): &(String, u32)| {
        [name.as_str().into()]
    });
    b.seed(votes, [s("Ada"), s("Ada")]);
    b.seed(total, [0]);
    b.seed(candidates, [(s("Bo"), 0), (s("Ada"), 0)]);
    b.transition(
        "Count",
        (TallyEdges {
            vote: TakeEdge(votes),
            total: MutateEdge(total),
            leader: MutateWhereEdge(candidates, Filter::new().key_in("name", ["Ada"])),
        },),
        (counted,),
        |t: Tally<'_>| {
            *t.total += 1;
            t.leader.1 += 1;
            (t.vote,)
        },
    );
    let mut net = b.build().unwrap();
    let total_id = net.tokens(total.id()).next().unwrap().0;
    let (_, info) = net.transitions().next().unwrap();
    assert!(
        !info.detachable,
        "a borrowing field ties the firing to the net"
    );

    net.step().unwrap();
    net.step().unwrap();
    assert_eq!(values(&net, total), [2]);
    assert_eq!(
        net.tokens(total.id()).next().unwrap().0,
        total_id,
        "same id"
    );
    assert_eq!(values(&net, candidates), [(s("Bo"), 0), (s("Ada"), 2)]);
    assert_eq!(values(&net, counted), ["Ada", "Ada"]);
}

/// A struct of edges stands in the tuple beside other positions, a plain
/// edge and a parameter, and its edges stand where it does.
#[test]
fn a_struct_of_edges_is_one_position_among_others() {
    let mut b = NetBuilder::new();
    let stamps = b.place::<u32>("Stamps");
    let checking = b.place::<u32>("Checking");
    let savings = b.place::<u32>("Savings");
    let notes = b.place::<String>("Notes");
    let log = b.place::<String>("Log");
    b.param(Rate(2));
    b.seed(stamps, [9]);
    b.seed(checking, [10]);
    b.seed(savings, [20]);
    b.seed(notes, [s("gift")]);
    let both = b
        .transition(
            "Stamped Transfer",
            (
                TakeEdge(stamps),
                TransferEdges {
                    from: TakeEdge(checking),
                    to: TakeEdge(savings),
                    note: TakeEdge(notes),
                },
                Param::<Rate>::new(),
            ),
            (log,),
            |stamp: u32, t: Transfer, rate: Rate| {
                (format!(
                    "#{stamp} {} x{}: {}",
                    t.note,
                    rate.0,
                    t.from + t.to
                ),)
            },
        )
        .id();
    let mut net = b.build().unwrap();
    assert_eq!(
        inputs_of(&net, both),
        [
            (stamps.id(), EdgeKind::Take),
            (checking.id(), EdgeKind::Take),
            (savings.id(), EdgeKind::Take),
            (notes.id(), EdgeKind::Take),
        ]
    );
    net.step().unwrap();
    assert_eq!(values(&net, log), ["#9 gift x2: 30"]);
}

/// The wiring is a value: `Copy` when its inputs are, so one wiring serves
/// two transitions, and printable.
#[test]
fn a_wiring_is_a_value_that_can_be_reused() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let a = b.place::<u32>("A");
    let c = b.place::<u32>("C");
    let wiring = Wiring {
        value: TakeEdge(inbox),
    };
    b.transition("First", (wiring,), (a,), |r: Renamed| (r.value,));
    b.transition("Second", (wiring,), (c,), |r: Renamed| (r.value + 1,));
    assert!(format!("{wiring:?}").starts_with("Wiring { value: TakeEdge("));
    let net = b.build().unwrap();
    assert_eq!(net.transitions().count(), 2);
}

// ----- what the engine does with the edges ---------------------------------

/// A failing body that restores its inputs puts back what the struct's
/// edges consumed, a drained place included, at the front with their ids;
/// what they only read was never taken.
#[test]
fn a_failing_body_restores_what_the_fields_consumed() {
    let mut b = NetBuilder::new();
    let ones = b.place::<u32>("Ones");
    let many = b.place::<u32>("Many");
    let names = b.place::<String>("Names");
    let pairs = b.place::<(String, u32)>("Pairs");
    let out = b.place::<String>("Out");
    b.param(Rate(1));
    b.seed(ones, [7]);
    b.seed(many, [1, 2]);
    b.seed(names, [s("snap")]);
    b.seed(pairs, [(s("a"), 1)]);
    let jam = b
        .transition(
            "Jam",
            (ShapesEdges {
                one: TakeEdge(ones),
                every: DrainEdge(many),
                copy: SnapshotEdge(names),
                chosen: SnapshotEdge(pairs),
                rate: Param::new(),
            },),
            (out,),
            |_: Shapes| -> Result<(String,), std::fmt::Error> { Err(std::fmt::Error) },
        )
        .restore_tokens_on_failure()
        .id();
    let mut net = b.build().unwrap();
    let before: Vec<_> = [ones, many]
        .iter()
        .flat_map(|place| net.tokens(place.id()).map(|(id, _)| id).collect::<Vec<_>>())
        .collect();

    let failure = net.fire(jam).expect_err("the body fails");
    assert!(matches!(failure, NetError::FiringFailed(ref f) if f.restored));
    let after: Vec<_> = [ones, many]
        .iter()
        .flat_map(|place| net.tokens(place.id()).map(|(id, _)| id).collect::<Vec<_>>())
        .collect();
    assert_eq!(after, before, "the take and the drain put back, ids kept");
    assert_eq!(values(&net, many), [1, 2]);
    assert_eq!(net.count(names.id()), 1);
}

/// One edge per place per transition counts the edges inside a struct as
/// any others, within it and against the rest of the tuple.
#[test]
fn a_place_read_twice_through_a_struct_is_a_duplicate() {
    let mut b = NetBuilder::new();
    let accounts = b.place::<u32>("Accounts");
    let notes = b.place::<String>("Notes");
    b.transition(
        "Self Transfer",
        (TransferEdges {
            from: TakeEdge(accounts),
            to: SnapshotEdge(accounts),
            note: TakeEdge(notes),
        },),
        (),
        |_: Transfer| (),
    );
    assert!(matches!(
        b.build(),
        Err(BuildError::DuplicateInputPlace { ref transition, ref place })
            if transition == "Self Transfer" && place == "Accounts"
    ));

    let mut b = NetBuilder::new();
    let checking = b.place::<u32>("Checking");
    let savings = b.place::<u32>("Savings");
    let notes = b.place::<String>("Notes");
    b.transition(
        "Twice",
        (
            TransferEdges {
                from: TakeEdge(checking),
                to: TakeEdge(savings),
                note: TakeEdge(notes),
            },
            SnapshotEdge(notes),
        ),
        (),
        |_: Transfer, _: String| (),
    );
    assert!(matches!(
        b.build(),
        Err(BuildError::DuplicateInputPlace { ref place, .. }) if place == "Notes"
    ));
}

/// `assert_no_source_transitions` counts the edges inside a struct that
/// always take a token, and only those: a struct read through a snapshot,
/// a drain and a survey makes its transition a source.
#[test]
fn the_source_rule_counts_only_the_always_consuming_edges_inside_a_struct() {
    let declare = |take: bool| {
        let mut b = NetBuilder::new();
        let checking = b.place::<u32>("Checking");
        let savings = b.place::<u32>("Savings");
        let notes = b.place::<String>("Notes");
        let ones = b.place::<u32>("Ones");
        let many = b.place::<u32>("Many");
        let pairs = b.place::<(String, u32)>("Pairs");
        b.param(Rate(1));
        if take {
            b.transition(
                "Read",
                (TransferEdges {
                    from: SnapshotEdge(checking),
                    to: TakeEdge(savings),
                    note: SnapshotEdge(notes),
                },),
                (),
                |_: Transfer| (),
            );
        } else {
            b.transition(
                "Read",
                (ShapesEdges {
                    one: SnapshotEdge(ones),
                    every: DrainEdge(many),
                    copy: SnapshotEdge(notes),
                    chosen: SnapshotEdge(pairs),
                    rate: Param::new(),
                },),
                (),
                |_: Shapes| (),
            );
        }
        b.assert_no_source_transitions();
        b.build().map(drop)
    };
    assert!(declare(true).is_ok(), "the take inside the struct counts");
    assert!(matches!(
        declare(false),
        Err(BuildError::SourceTransition { ref transition }) if transition == "Read"
    ));
}

/// The read API, the edge list and the serialised topology carry the
/// struct's edges as plain edges, in field order, with nothing to say they
/// came from a struct.
#[test]
fn the_read_api_and_the_topology_see_plain_edges() {
    let mut b = NetBuilder::new();
    let votes = b.place::<String>("Votes");
    let total = b.place::<u32>("Total");
    let candidates = b.place::<(String, u32)>("Candidates");
    let counted = b.place::<String>("Counted");
    b.keys(candidates, ["name"], |(name, _): &(String, u32)| {
        [name.as_str().into()]
    });
    let ada = Filter::new().key_in("name", ["Ada"]);
    b.transition(
        "Count",
        (TallyEdges {
            vote: TakeEdge(votes),
            total: MutateEdge(total),
            leader: MutateWhereEdge(candidates, ada.clone()),
        },),
        (counted,),
        |t: Tally<'_>| (t.vote,),
    );
    let net = b.build().unwrap();
    let edges: Vec<EdgeInfo> = net.edges().collect();
    assert_eq!(edges.len(), 4, "three input edges and one output");
    assert!(
        matches!(
            &edges[..3],
            [
                EdgeInfo::Input { place: p0, kind: EdgeKind::Take, filter: None, .. },
                EdgeInfo::Input { place: p1, kind: EdgeKind::Mutate, filter: None, .. },
                EdgeInfo::Input { place: p2, kind: EdgeKind::MutateWhere, filter: Some(f), .. },
            ] if *p0 == votes.id() && *p1 == total.id() && *p2 == candidates.id() && *f == ada
        ),
        "{edges:?}"
    );
    let topology = net.topology();
    assert_eq!(topology.transitions[0].inputs.len(), 3);
}

/// With `serde`, the topology a runner puts on the wire names the kinds in
/// field order, as it would for a tuple of the same edges.
#[cfg(feature = "serde")]
#[test]
fn the_serialised_topology_names_the_kinds_in_field_order() {
    let mut b = NetBuilder::new();
    let checking = b.place::<u32>("Checking");
    let savings = b.place::<u32>("Savings");
    let notes = b.place::<String>("Notes");
    b.transition(
        "Transfer",
        (TransferEdges {
            note: TakeEdge(notes),
            to: SnapshotEdge(savings),
            from: TakeEdge(checking),
        },),
        (),
        |_: Transfer| (),
    );
    let net = b.build().unwrap();
    let topology = petritype::serde_json::to_value(net.topology()).unwrap();
    let kinds: Vec<&str> = topology["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|edge| edge["kind"].as_str())
        .collect();
    assert_eq!(kinds, ["take", "snapshot", "take"]);
}
