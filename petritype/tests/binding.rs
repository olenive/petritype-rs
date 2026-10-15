//! Integration tests for binding by name (DESIGN §4.4, §4.5;
//! `dev-docs/BINDING_BY_NAME.md`).
//!
//! A struct with one field per input derives `Inputs`, and the struct of
//! edges it generates is one input position: the body takes the struct, and
//! the net sees the fields' edges, in field order, as if they had been
//! written in the tuple one by one. A struct a body returns derives
//! `Outputs`, and the struct of outputs it generates is the whole of the
//! transition's outputs: each field's tokens go to the place wired to its
//! name. What is tested here is that promise — the order, the kinds, the
//! borrows, the restore, the build checks, the read API, the return forms —
//! and the by-name part of it: the fields are matched by name, whatever
//! order a literal writes them in. The acceptance test is the till of
//! `dev-docs/TRANSITION_ARITY.md` §2, written by name: fourteen inputs and
//! thirteen outputs, past the tuples' cap, with that document's §6 checks.

use std::collections::BTreeMap;

use petritype::{
    BuildError, DrainEdge, EdgeInfo, EdgeKind, Event, Filter, Inputs, MutateEdge, MutateWhereEdge,
    Net, NetBuilder, NetError, OutputDistribution, Outputs, Param, PlaceRef, Routed, SnapshotEdge,
    SurveyEdge, TakeAllEdge, TakeEdge, TakeWhereEdge, TransitionId,
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

// ----- outputs by name -------------------------------------------------------

/// A sale: every shape an output field can have.
#[derive(Debug, Clone, PartialEq, Outputs)]
struct Sale {
    total: u32,
    discount: Option<String>,
    lines: Vec<String>,
}

/// A struct that derives both `Routed` and `Outputs`: routed whole where
/// the outputs are a tuple, bound by name where they are its own struct of
/// outputs.
#[derive(Debug, Clone, PartialEq, Routed, Outputs)]
struct Receipt {
    total: u32,
    customer: String,
}

/// The output edges of one transition, as `(place, slot)`.
fn output_slots(net: &Net, transition: TransitionId) -> Vec<(petritype::PlaceId, usize)> {
    net.edges()
        .filter_map(|edge| match edge {
            EdgeInfo::Output {
                transition: from,
                place,
                slot,
            } if from == transition => Some((place, slot)),
            _ => None,
        })
        .collect()
}

/// The fields of a returned struct go to the places wired to their names, a
/// field of `T` one token, of `Option<T>` zero or one, of `Vec<T>` each;
/// the output edges are the fields', in field order whatever the literal
/// says, and the form reads back as positional, slots by field (D4).
#[test]
fn each_field_goes_to_the_place_wired_to_its_name() {
    let mut b = NetBuilder::new();
    let baskets = b.place::<Vec<String>>("Baskets");
    let totals = b.place::<u32>("Totals");
    let discounts = b.place::<String>("Discounts");
    let lines = b.place::<String>("Lines");
    b.seed(baskets, [vec![s("tea"), s("cake")], vec![s("tea")]]);
    let sell = b
        .transition(
            "Sell",
            (TakeEdge(baskets),),
            SaleEdges {
                lines,
                total: totals,
                discount: discounts,
            },
            |basket: Vec<String>| Sale {
                total: 100 * basket.len() as u32,
                discount: (basket.len() > 1).then(|| s("two for one")),
                lines: basket,
            },
        )
        .id();
    let mut net = b.build().unwrap();

    let (_, info) = net.transitions().next().unwrap();
    assert_eq!(info.outputs, [totals.id(), discounts.id(), lines.id()]);
    assert!(info.positional);
    assert_eq!(
        output_slots(&net, sell),
        [(totals.id(), 0), (discounts.id(), 1), (lines.id(), 2)]
    );

    net.step().unwrap();
    net.step().unwrap();
    assert_eq!(values(&net, totals), [200, 100]);
    assert_eq!(values(&net, discounts), ["two for one"], "zero or one");
    assert_eq!(values(&net, lines), ["tea", "cake", "tea"], "each");
}

/// `Result<Sale, E>` wraps the named form as it wraps the others: `Ok` is
/// distributed by name, `Err` fails the firing and writes nothing.
#[test]
fn a_result_of_a_named_return_is_failure_or_outputs() {
    let mut b = NetBuilder::new();
    let baskets = b.place::<Vec<String>>("Baskets");
    let totals = b.place::<u32>("Totals");
    let discounts = b.place::<String>("Discounts");
    let lines = b.place::<String>("Lines");
    b.seed(baskets, [vec![s("tea")], vec![]]);
    b.transition(
        "Sell",
        (TakeEdge(baskets),),
        SaleEdges {
            total: totals,
            discount: discounts,
            lines,
        },
        |basket: Vec<String>| -> Result<Sale, std::fmt::Error> {
            if basket.is_empty() {
                return Err(std::fmt::Error);
            }
            Ok(Sale {
                total: 1,
                discount: None,
                lines: basket,
            })
        },
    )
    .restore_tokens_on_failure();
    let mut net = b.build().unwrap();
    let (_, info) = net.transitions().next().unwrap();
    assert!(info.positional, "a `Result` reads back as what it wraps");

    net.step().unwrap();
    assert_eq!(values(&net, lines), ["tea"]);
    assert!(matches!(
        net.step(),
        Err(ref failure) if failure.restored
    ));
    assert_eq!(net.count(totals.id()), 1, "the failure wrote nothing");
    assert_eq!(
        values(&net, baskets),
        [Vec::<String>::new()],
        "and restored"
    );
}

/// An output distribution function may return a named struct: the body
/// keeps its own return type, and the function decides the form.
#[test]
fn an_output_distribution_function_may_return_by_name() {
    let mut b = NetBuilder::new();
    let baskets = b.place::<Vec<String>>("Baskets");
    let totals = b.place::<u32>("Totals");
    let discounts = b.place::<String>("Discounts");
    let lines = b.place::<String>("Lines");
    b.seed(baskets, [vec![s("tea"), s("cake")]]);
    let count = |basket: Vec<String>| basket.len() as u32;
    b.transition(
        "Count",
        (TakeEdge(baskets),),
        SaleEdges {
            total: totals,
            discount: discounts,
            lines,
        },
        count.output_distribution(|n: u32| Sale {
            total: n,
            discount: None,
            lines: vec![],
        }),
    );
    let mut net = b.build().unwrap();
    net.step().unwrap();
    assert_eq!(values(&net, totals), [2]);
}

/// A struct that derives both `Routed` and `Outputs` is never ambiguous:
/// returned to a tuple of outputs it is routed whole, as a leaf, to the
/// place of its own type; returned to its own struct of outputs, its fields
/// are bound by name.
#[test]
fn a_struct_that_is_both_routed_and_named_is_read_by_its_outputs() {
    let mut b = NetBuilder::new();
    let tills = b.place::<String>("Tills");
    let counters = b.place::<String>("Counters");
    let receipts = b.place::<Receipt>("Receipts");
    let totals = b.place::<u32>("Totals");
    let customers = b.place::<String>("Customers");
    b.seed(tills, [s("Ada")]);
    b.seed(counters, [s("Bo")]);
    let receipt = |customer: String| Receipt { total: 7, customer };
    let routed = b
        .transition("Till", (TakeEdge(tills),), (receipts,), receipt)
        .id();
    let named = b
        .transition(
            "Counter",
            (TakeEdge(counters),),
            ReceiptEdges {
                total: totals,
                customer: customers,
            },
            receipt,
        )
        .id();
    let mut net = b.build().unwrap();

    let positional = |id| {
        net.transitions()
            .find(|(transition, _)| *transition == id)
            .unwrap()
            .1
            .positional
    };
    assert!(!positional(routed), "routed by type");
    assert!(positional(named), "bound by name");

    net.fire(routed).unwrap();
    net.fire(named).unwrap();
    assert_eq!(
        values(&net, receipts),
        [Receipt {
            total: 7,
            customer: s("Ada")
        }]
    );
    assert_eq!(values(&net, totals), [7]);
    assert_eq!(values(&net, customers), ["Bo"]);
}

// ----- the till, by name -------------------------------------------------

/// The till's price list: cents per item.
#[derive(Debug, Clone)]
struct Prices(BTreeMap<String, u32>);

/// `Serve`'s inputs, fourteen of them: the order, the till's marker, the
/// price list, and eleven shelves, each a list of item names, possibly
/// empty (`TRANSITION_ARITY.md` §2).
#[derive(Inputs)]
struct Till {
    order: String,
    open: String,
    prices: Prices,
    shelf_1: Vec<String>,
    shelf_2: Vec<String>,
    shelf_3: Vec<String>,
    shelf_4: Vec<String>,
    shelf_5: Vec<String>,
    shelf_6: Vec<String>,
    shelf_7: Vec<String>,
    shelf_8: Vec<String>,
    shelf_9: Vec<String>,
    shelf_10: Vec<String>,
    shelf_11: Vec<String>,
}

/// `Serve`'s outputs, thirteen of them: the receipt, a restock request per
/// shelf the order emptied, and the customer served.
#[derive(Outputs)]
struct Served {
    receipt: String,
    restock_1: Option<String>,
    restock_2: Option<String>,
    restock_3: Option<String>,
    restock_4: Option<String>,
    restock_5: Option<String>,
    restock_6: Option<String>,
    restock_7: Option<String>,
    restock_8: Option<String>,
    restock_9: Option<String>,
    restock_10: Option<String>,
    restock_11: Option<String>,
    served: String,
}

/// The products, one per shelf, in shelf order.
const PRODUCTS: [&str; 11] = [
    "milk", "bread", "eggs", "tea", "jam", "rice", "oats", "salt", "soap", "figs", "honey",
];

/// `Serve`: prices `"Ada: milk, bread"` from the shelves, and asks for a
/// restock of every shelf the order takes the last item of. The shelves are
/// surveyed, so the order is priced from what they hold, which the body
/// does not change.
fn serve(t: Till) -> Served {
    let shelves = [
        t.shelf_1, t.shelf_2, t.shelf_3, t.shelf_4, t.shelf_5, t.shelf_6, t.shelf_7, t.shelf_8,
        t.shelf_9, t.shelf_10, t.shelf_11,
    ];
    let (customer, items) = t.order.split_once(": ").expect("customer: items");
    let wanted: Vec<&str> = items.split(", ").collect();
    let mut restock: [Option<String>; 11] = Default::default();
    let mut total = 0;
    for (shelf, held) in shelves.iter().enumerate() {
        let taken = wanted
            .iter()
            .filter(|item| held.iter().any(|on| on == *item))
            .count();
        total += taken as u32 * t.prices.0.get(PRODUCTS[shelf]).copied().unwrap_or(0);
        if taken > 0 && taken >= held.len() {
            restock[shelf] = Some(PRODUCTS[shelf].to_owned());
        }
    }
    let [r1, r2, r3, r4, r5, r6, r7, r8, r9, r10, r11] = restock;
    Served {
        receipt: format!("{customer} ({}): {total}", t.open),
        restock_1: r1,
        restock_2: r2,
        restock_3: r3,
        restock_4: r4,
        restock_5: r5,
        restock_6: r6,
        restock_7: r7,
        restock_8: r8,
        restock_9: r9,
        restock_10: r10,
        restock_11: r11,
        served: customer.to_owned(),
    }
}

/// The till's places, and `Serve`.
struct TillNet {
    net: Net,
    orders: PlaceRef<String>,
    open: PlaceRef<String>,
    shelves: [PlaceRef<String>; 11],
    receipts: PlaceRef<String>,
    restocks: [PlaceRef<String>; 11],
    served: PlaceRef<String>,
    serve: TransitionId,
}

/// The till of `TRANSITION_ARITY.md` §2, by name; with `jammed`, `Serve`
/// fails and restores what it took.
fn till(jammed: bool) -> TillNet {
    let mut b = NetBuilder::new();
    let orders = b.place::<String>("Orders");
    let open = b.place::<String>("Till Open");
    let shelves: [PlaceRef<String>; 11] =
        std::array::from_fn(|i| b.place::<String>(&format!("Shelf {}", i + 1)));
    let receipts = b.place::<String>("Receipts");
    let restocks: [PlaceRef<String>; 11] =
        std::array::from_fn(|i| b.place::<String>(&format!("Restock {}", i + 1)));
    let served = b.place::<String>("Served");
    b.param(Prices(
        PRODUCTS
            .iter()
            .enumerate()
            .map(|(i, product)| ((*product).to_owned(), 100 + i as u32))
            .collect(),
    ));
    b.seed(open, [s("open")]);

    let [s1, s2, s3, s4, s5, s6, s7, s8, s9, s10, s11] = shelves;
    let [r1, r2, r3, r4, r5, r6, r7, r8, r9, r10, r11] = restocks;
    let inputs = TillEdges {
        order: TakeEdge(orders),
        open: SnapshotEdge(open),
        prices: Param::new(),
        shelf_1: SurveyEdge(s1),
        shelf_2: SurveyEdge(s2),
        shelf_3: SurveyEdge(s3),
        shelf_4: SurveyEdge(s4),
        shelf_5: SurveyEdge(s5),
        shelf_6: SurveyEdge(s6),
        shelf_7: SurveyEdge(s7),
        shelf_8: SurveyEdge(s8),
        shelf_9: SurveyEdge(s9),
        shelf_10: SurveyEdge(s10),
        shelf_11: SurveyEdge(s11),
    };
    let outputs = ServedEdges {
        receipt: receipts,
        restock_1: r1,
        restock_2: r2,
        restock_3: r3,
        restock_4: r4,
        restock_5: r5,
        restock_6: r6,
        restock_7: r7,
        restock_8: r8,
        restock_9: r9,
        restock_10: r10,
        restock_11: r11,
        served,
    };
    let serve = if jammed {
        b.transition(
            "Serve",
            (inputs,),
            outputs,
            |_: Till| -> Result<Served, std::fmt::Error> { Err(std::fmt::Error) },
        )
        .restore_tokens_on_failure()
        .id()
    } else {
        b.transition("Serve", (inputs,), outputs, serve).id()
    };
    b.assert_no_source_transitions();
    TillNet {
        net: b.build().expect("the till builds, its one take counted"),
        orders,
        open,
        shelves,
        receipts,
        restocks,
        served,
        serve,
    }
}

/// Every shelf seeded with two of its product.
fn stock(till: &mut TillNet) {
    for (shelf, product) in till.shelves.iter().zip(PRODUCTS) {
        till.net.extend(*shelf, [s(product), s(product)]);
    }
}

/// The till by name, with `TRANSITION_ARITY.md` §2's script and §6's
/// checks: fourteen inputs, thirteen outputs, one firing per order, a
/// restock only for an emptied shelf, nothing without an order.
#[test]
fn the_till_serves_by_name_with_fourteen_inputs_and_thirteen_outputs() {
    let mut till = till(false);

    // The shape: thirteen input edges and the parameter, fourteen inputs in
    // all; thirteen output edges; nothing to say there was a struct.
    let (_, info) = till.net.transitions().next().unwrap();
    assert_eq!(info.inputs.len(), 13);
    assert_eq!(
        info.params.len(),
        1,
        "the price list is the fourteenth input"
    );
    assert_eq!(info.inputs[0], (till.orders.id(), EdgeKind::Take));
    assert_eq!(info.inputs[1], (till.open.id(), EdgeKind::Snapshot));
    assert!(
        info.inputs[2..]
            .iter()
            .zip(till.shelves)
            .all(|(edge, shelf)| *edge == (shelf.id(), EdgeKind::Survey)),
        "the shelves in field order"
    );
    assert_eq!(info.outputs.len(), 13);
    assert_eq!(info.outputs[0], till.receipts.id());
    assert_eq!(info.outputs[12], till.served.id());
    assert_eq!(till.net.edges().count(), 26);
    let topology = till.net.topology();
    assert_eq!(topology.transitions[0].inputs.len(), 13);
    assert_eq!(topology.transitions[0].outputs.len(), 13);

    // With `Orders` empty, `Serve` is not fireable however full the shelves.
    stock(&mut till);
    assert!(!till.net.is_fireable(till.serve));
    assert_eq!(till.net.step().unwrap(), None);

    // Ada takes milk and bread from shelves of two: one receipt, one
    // served, no restock.
    till.net.put(till.orders, s("Ada: milk, bread"));
    assert_eq!(till.net.step().unwrap(), Some(till.serve));
    assert_eq!(values(&till.net, till.receipts), ["Ada (open): 201"]);
    assert_eq!(values(&till.net, till.served), ["Ada"]);
    assert!(till.restocks.iter().all(|r| till.net.count(r.id()) == 0));
    assert!(
        till.shelves
            .iter()
            .all(|shelf| till.net.count(shelf.id()) == 2),
        "the shelves are surveyed, not taken"
    );

    // A shelf of one, and an order that takes it: that shelf's restock
    // place gets one token, the others none.
    till.net.set_tokens(till.shelves[2], [s("eggs")]);
    till.net.put(till.orders, s("Bo: eggs"));
    till.net.step().unwrap();
    assert_eq!(values(&till.net, till.restocks[2]), ["eggs"]);
    assert_eq!(
        till.restocks
            .iter()
            .map(|r| till.net.count(r.id()))
            .sum::<usize>(),
        1
    );
    assert_eq!(values(&till.net, till.served), ["Ada", "Bo"]);
    assert!(!till.net.is_fireable(till.serve), "no order left");
}

/// A failing `Serve` restores the order, which it took, and nothing else:
/// the surveys and the snapshot consumed nothing.
#[test]
fn a_failing_till_restores_the_order_and_nothing_else() {
    let mut till = till(true);
    stock(&mut till);
    let order = till.net.put(till.orders, s("Ada: milk"));
    till.net.drain_events().for_each(drop);

    let failure = till.net.fire(till.serve).expect_err("the till jams");
    assert!(matches!(failure, NetError::FiringFailed(ref f) if f.restored));
    let events: Vec<Event> = till.net.drain_events().collect();
    assert!(
        matches!(&events[..], [Event::Failed { consumed, restored: true, .. }]
            if consumed == &[(till.orders.id(), order)]),
        "{events:?}"
    );
    assert_eq!(
        till.net.tokens(till.orders.id()).next().unwrap().0,
        order,
        "the order back, its id kept"
    );
    assert!(
        till.shelves
            .iter()
            .all(|shelf| till.net.count(shelf.id()) == 2)
    );
}

/// A struct of one input behaves like the input: the same edge, the same
/// firing.
#[test]
fn a_struct_of_one_input_behaves_like_the_input() {
    #[derive(Inputs)]
    struct One {
        value: u32,
    }

    let declare = |by_name: bool| {
        let mut b = NetBuilder::new();
        let inbox = b.place::<u32>("Inbox");
        let out = b.place::<u32>("Out");
        b.seed(inbox, [20, 1]);
        if by_name {
            b.transition(
                "Double",
                (OneEdges {
                    value: TakeEdge(inbox),
                },),
                (out,),
                |one: One| (one.value * 2,),
            );
        } else {
            b.transition("Double", (TakeEdge(inbox),), (out,), |value: u32| {
                (value * 2,)
            });
        }
        let mut net = b.build().unwrap();
        while net.step().unwrap().is_some() {}
        let (_, info) = net.transitions().next().unwrap();
        (info.inputs.to_vec(), values(&net, out))
    };
    assert_eq!(declare(true), declare(false));
}
