//! Integration tests for the flat tuple cap at sixteen (DESIGN §5;
//! `dev-docs/BINDING_BY_NAME.md` R1).
//!
//! Until 2026-10-15 a transition's input and output tuples stopped at twelve.
//! The acceptance test is the till of `dev-docs/TRANSITION_ARITY.md` §2,
//! written flat: one transition, `Serve`, past the old cap on both sides,
//! with fourteen inputs (an order taken, the till-open marker snapshotted,
//! the price list as a parameter, eleven shelves surveyed) and thirteen
//! outputs (a receipt, eleven restock requests, the customer served), and
//! the checks of that document's §6. Then the cap itself: sixteen in and
//! sixteen out, fired once.

use std::any::type_name;

use petritype::{
    EdgeInfo, EdgeKind, Event, Net, NetBuilder, NetError, Param, PlaceRef, SnapshotEdge,
    SurveyEdge, TakeEdge, TokenId, TransitionId,
};

/// A customer's order: who, and the item names wanted.
#[derive(Debug, Clone, PartialEq)]
struct Order {
    customer: String,
    items: Vec<String>,
}

/// What the till prints for one order.
#[derive(Debug, Clone, PartialEq)]
struct Receipt {
    customer: String,
    total: u32,
}

/// The price list, in pence: a parameter of the net, not a place.
#[derive(Clone)]
struct Prices(Vec<(&'static str, u32)>);

impl Prices {
    fn of(&self, item: &str) -> u32 {
        self.0
            .iter()
            .find(|(name, _)| *name == item)
            .map_or(0, |(_, pence)| *pence)
    }
}

/// An order for something no shelf holds.
#[derive(Debug)]
struct OutOfStock(String);

impl std::fmt::Display for OutOfStock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no shelf holds {}", self.0)
    }
}

impl std::error::Error for OutOfStock {}

/// The item each shelf stocks, `Shelf 1` to `Shelf 11` in order.
const ITEMS: [&str; 11] = [
    "milk", "bread", "eggs", "butter", "cheese", "apples", "rice", "tea", "coffee", "jam", "honey",
];

/// What `Serve` returns, by position: the receipt, one restock request per
/// shelf (an item name for a shelf this order emptied, else nothing), and
/// the customer served. Thirteen positions.
type Served = (
    Receipt,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
);

/// The body of `Serve`, one parameter per input position in declaration
/// order. The shelves are surveyed, so it works out which of them the order
/// *would* empty from the clones it was handed; the shelves themselves are
/// left as they were. An item no shelf holds fails the firing.
#[allow(clippy::too_many_arguments)]
fn serve(
    order: Order,
    _open: String,
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
) -> Result<Served, OutOfStock> {
    let mut shelves = [
        shelf_1, shelf_2, shelf_3, shelf_4, shelf_5, shelf_6, shelf_7, shelf_8, shelf_9, shelf_10,
        shelf_11,
    ];
    let mut emptied: [Option<String>; 11] = Default::default();
    let mut total = 0;
    for item in &order.items {
        let Some(shelf) = shelves.iter().position(|shelf| shelf.contains(item)) else {
            return Err(OutOfStock(item.clone()));
        };
        let at = shelves[shelf].iter().position(|on| on == item).unwrap();
        shelves[shelf].remove(at);
        if shelves[shelf].is_empty() {
            emptied[shelf] = Some(item.clone());
        }
        total += prices.of(item);
    }
    let [r1, r2, r3, r4, r5, r6, r7, r8, r9, r10, r11] = emptied;
    let receipt = Receipt {
        customer: order.customer.clone(),
        total,
    };
    Ok((
        receipt,
        r1,
        r2,
        r3,
        r4,
        r5,
        r6,
        r7,
        r8,
        r9,
        r10,
        r11,
        order.customer,
    ))
}

/// The till's places and its one transition.
struct Till {
    orders: PlaceRef<Order>,
    till_open: PlaceRef<String>,
    shelves: [PlaceRef<String>; 11],
    receipts: PlaceRef<Receipt>,
    restock: [PlaceRef<String>; 11],
    served: PlaceRef<String>,
    serve: TransitionId,
}

/// The till of `TRANSITION_ARITY.md` §2, built: the till open, every other
/// place empty, and `Serve` restoring what it took when it fails.
fn till() -> (Net, Till) {
    let mut b = NetBuilder::new();
    let orders = b.place::<Order>("Orders");
    let till_open = b.place::<String>("Till Open");
    let shelves: [PlaceRef<String>; 11] =
        std::array::from_fn(|i| b.place(format!("Shelf {}", i + 1)));
    let receipts = b.place::<Receipt>("Receipts");
    let restock: [PlaceRef<String>; 11] =
        std::array::from_fn(|i| b.place(format!("Restock {}", i + 1)));
    let served = b.place::<String>("Served");
    b.param(Prices(vec![
        ("milk", 120),
        ("bread", 250),
        ("eggs", 300),
        ("rice", 180),
    ]));
    b.seed(till_open, [s("open")]);

    let [s1, s2, s3, s4, s5, s6, s7, s8, s9, s10, s11] = shelves;
    let [r1, r2, r3, r4, r5, r6, r7, r8, r9, r10, r11] = restock;
    let transition = b
        .transition(
            "Serve",
            (
                TakeEdge(orders),
                SnapshotEdge(till_open),
                Param::<Prices>::new(),
                SurveyEdge(s1),
                SurveyEdge(s2),
                SurveyEdge(s3),
                SurveyEdge(s4),
                SurveyEdge(s5),
                SurveyEdge(s6),
                SurveyEdge(s7),
                SurveyEdge(s8),
                SurveyEdge(s9),
                SurveyEdge(s10),
                SurveyEdge(s11),
            ),
            (
                receipts, r1, r2, r3, r4, r5, r6, r7, r8, r9, r10, r11, served,
            ),
            serve,
        )
        .restore_tokens_on_failure()
        .id();
    // One edge takes, so `Serve` is no source.
    b.assert_no_source_transitions();
    let net = b.build().expect("the till builds");
    let till = Till {
        orders,
        till_open,
        shelves,
        receipts,
        restock,
        served,
        serve: transition,
    };
    (net, till)
}

fn s(text: &str) -> String {
    text.to_owned()
}

fn order(customer: &str, items: &[&str]) -> Order {
    Order {
        customer: s(customer),
        items: items.iter().copied().map(s).collect(),
    }
}

/// Every shelf stocked with `count` of its item.
fn stock(net: &mut Net, till: &Till, count: usize) {
    for (shelf, item) in till.shelves.into_iter().zip(ITEMS) {
        net.set_tokens(shelf, vec![s(item); count]);
    }
}

/// The tokens of a place, cloned, in queue order.
fn tokens<T: Clone + 'static>(net: &Net, place: PlaceRef<T>) -> Vec<T> {
    net.tokens(place.id())
        .map(|(id, _)| net.token::<T>(place.id(), id).unwrap().clone())
        .collect()
}

/// The ids of a place's tokens, in queue order.
fn ids<T>(net: &Net, place: PlaceRef<T>) -> Vec<TokenId> {
    net.tokens(place.id()).map(|(id, _)| id).collect()
}

/// How many tokens each restock place holds, `Restock 1` first.
fn restock_counts(net: &Net, till: &Till) -> Vec<usize> {
    till.restock
        .iter()
        .map(|place| net.count(place.id()))
        .collect()
}

// ----- the till ----------------------------------------------------------------

/// Every shelf holds two, and Ada orders milk and bread: one receipt priced
/// from the parameter, one `Served`, no restock, and the shelves untouched.
#[test]
fn one_order_makes_one_receipt_and_no_restock() {
    let (mut net, till) = till();
    stock(&mut net, &till, 2);
    let shelves_before: Vec<Vec<TokenId>> =
        till.shelves.iter().map(|&shelf| ids(&net, shelf)).collect();
    net.put(till.orders, order("Ada", &["milk", "bread"]));

    assert_eq!(net.step().unwrap(), Some(till.serve));
    assert_eq!(
        tokens(&net, till.receipts),
        [Receipt {
            customer: s("Ada"),
            total: 120 + 250
        }]
    );
    assert_eq!(tokens(&net, till.served), ["Ada"]);
    assert_eq!(restock_counts(&net, &till), [0; 11], "no shelf emptied");
    assert_eq!(net.count(till.orders.id()), 0, "the order taken");
    assert_eq!(tokens(&net, till.till_open), ["open"], "the marker read");
    let shelves_after: Vec<Vec<TokenId>> =
        till.shelves.iter().map(|&shelf| ids(&net, shelf)).collect();
    assert_eq!(shelves_after, shelves_before, "surveyed, not touched");

    assert_eq!(net.step().unwrap(), None, "quiescent");
}

/// One shelf holds a single item and the order takes it: that shelf's
/// restock place, and no other, gets the item's name.
#[test]
fn a_shelf_the_order_empties_is_restocked_and_no_other() {
    let (mut net, till) = till();
    stock(&mut net, &till, 2);
    net.set_tokens(till.shelves[6], [s("rice")]);
    net.put(till.orders, order("Bo", &["rice", "eggs"]));

    assert_eq!(net.step().unwrap(), Some(till.serve));
    let mut expected = [0; 11];
    expected[6] = 1;
    assert_eq!(restock_counts(&net, &till), expected);
    assert_eq!(tokens(&net, till.restock[6]), ["rice"]);
    assert_eq!(
        tokens(&net, till.receipts),
        [Receipt {
            customer: s("Bo"),
            total: 180 + 300
        }]
    );
    assert_eq!(tokens(&net, till.served), ["Bo"]);
}

/// With `Orders` empty `Serve` is not fireable, however full the shelves;
/// an order makes it fireable, and the snapshot of the till-open marker
/// still holds it back when the till is closed.
#[test]
fn serve_waits_for_an_order_however_full_the_shelves() {
    let (mut net, till) = till();
    assert!(!net.is_fireable(till.serve), "empty shelves, no order");
    for count in [1, 5, 50] {
        stock(&mut net, &till, count);
        assert!(!net.is_fireable(till.serve), "{count} on every shelf");
        assert!(net.fireable().is_empty());
    }
    assert_eq!(net.step().unwrap(), None);

    net.put(till.orders, order("Ada", &["milk"]));
    assert_eq!(net.fireable(), [till.serve]);
    stock(&mut net, &till, 0);
    assert_eq!(net.fireable(), [till.serve], "empty shelves are fine");

    net.set_tokens(till.till_open, Vec::<String>::new());
    assert!(!net.is_fireable(till.serve), "the till is closed");
}

/// An order for something no shelf holds fails the body. `Serve` restores
/// what it took, which is the order and nothing else: the marker was
/// snapshotted and the shelves surveyed, so neither lost a token, and no
/// output place gained one.
#[test]
fn a_failing_serve_restores_the_order_and_nothing_else() {
    let (mut net, till) = till();
    stock(&mut net, &till, 2);
    let marker = ids(&net, till.till_open);
    let shelves_before: Vec<Vec<TokenId>> =
        till.shelves.iter().map(|&shelf| ids(&net, shelf)).collect();
    let wanted = net.put(till.orders, order("Cy", &["milk", "caviar"]));
    net.drain_events().for_each(drop);

    let failure = net.fire(till.serve).expect_err("out of stock");
    assert!(matches!(failure, NetError::FiringFailed(ref f) if f.restored));
    let events: Vec<Event> = net.drain_events().collect();
    assert!(
        matches!(&events[..], [Event::Failed { transition, consumed, restored: true, error }]
            if *transition == till.serve
                && consumed == &[(till.orders.id(), wanted)]
                && error == "no shelf holds caviar"),
        "{events:?}"
    );

    assert_eq!(ids(&net, till.orders), [wanted], "the order is back");
    assert_eq!(
        tokens(&net, till.orders),
        [order("Cy", &["milk", "caviar"])]
    );
    assert_eq!(ids(&net, till.till_open), marker);
    let shelves_after: Vec<Vec<TokenId>> =
        till.shelves.iter().map(|&shelf| ids(&net, shelf)).collect();
    assert_eq!(shelves_after, shelves_before);
    assert_eq!(net.count(till.receipts.id()), 0);
    assert_eq!(net.count(till.served.id()), 0);
    assert_eq!(restock_counts(&net, &till), [0; 11]);
}

/// The topology carries every input and output of `Serve` in declaration
/// order. Of its fourteen input positions thirteen are edges — the take,
/// the snapshot and the eleven surveys — and one is the price list, which
/// is an input of the transition but not an edge of the net (DESIGN §10),
/// so it is listed under `params` instead. Its thirteen outputs are thirteen
/// output edges, slots 0 to 12, and the body is positional.
#[test]
fn the_topology_lists_every_input_and_output_in_order() {
    let (net, till) = till();
    let topology = net.topology();
    let serve = &topology.transitions[0];
    assert_eq!(serve.id, till.serve);
    assert_eq!(serve.name, "Serve");

    let mut inputs = vec![
        (till.orders.id(), EdgeKind::Take),
        (till.till_open.id(), EdgeKind::Snapshot),
    ];
    inputs.extend(
        till.shelves
            .iter()
            .map(|shelf| (shelf.id(), EdgeKind::Survey)),
    );
    assert_eq!(serve.inputs, inputs);
    assert_eq!(serve.params, [type_name::<Prices>()]);
    assert_eq!(serve.inputs.len() + serve.params.len(), 14);
    assert_eq!(serve.filters, vec![None; 13]);

    let mut outputs = vec![till.receipts.id()];
    outputs.extend(till.restock.iter().map(|place| place.id()));
    outputs.push(till.served.id());
    assert_eq!(serve.outputs, outputs);
    assert!(serve.positional);

    let input_edges: Vec<_> = inputs
        .iter()
        .map(|&(place, kind)| EdgeInfo::Input {
            place,
            transition: till.serve,
            kind,
            filter: None,
        })
        .collect();
    let output_edges: Vec<_> = outputs
        .iter()
        .enumerate()
        .map(|(slot, &place)| EdgeInfo::Output {
            transition: till.serve,
            place,
            slot,
        })
        .collect();
    assert_eq!(input_edges.len(), 13);
    assert_eq!(output_edges.len(), 13);
    let expected: Vec<EdgeInfo> = input_edges.into_iter().chain(output_edges).collect();
    assert_eq!(topology.edges, expected);
    assert_eq!(net.edges().collect::<Vec<_>>(), expected);
}

// ----- the cap ------------------------------------------------------------------

/// Sixteen take edges in, sixteen places out, and a body that returns a
/// sixteen-tuple by position, reversed, so that every position past the old
/// cap has to land where it was declared.
#[test]
fn sixteen_in_and_sixteen_out_fire_once() {
    let mut b = NetBuilder::new();
    let ins: [PlaceRef<u32>; 16] = std::array::from_fn(|i| b.place(format!("In {i}")));
    let outs: [PlaceRef<u32>; 16] = std::array::from_fn(|i| b.place(format!("Out {i}")));
    for (n, place) in (0..).zip(ins) {
        b.seed(place, [n]);
    }
    let sixteen = b
        .transition(
            "Sixteen",
            (
                TakeEdge(ins[0]),
                TakeEdge(ins[1]),
                TakeEdge(ins[2]),
                TakeEdge(ins[3]),
                TakeEdge(ins[4]),
                TakeEdge(ins[5]),
                TakeEdge(ins[6]),
                TakeEdge(ins[7]),
                TakeEdge(ins[8]),
                TakeEdge(ins[9]),
                TakeEdge(ins[10]),
                TakeEdge(ins[11]),
                TakeEdge(ins[12]),
                TakeEdge(ins[13]),
                TakeEdge(ins[14]),
                TakeEdge(ins[15]),
            ),
            (
                outs[0], outs[1], outs[2], outs[3], outs[4], outs[5], outs[6], outs[7], outs[8],
                outs[9], outs[10], outs[11], outs[12], outs[13], outs[14], outs[15],
            ),
            |a0: u32,
             a1: u32,
             a2: u32,
             a3: u32,
             a4: u32,
             a5: u32,
             a6: u32,
             a7: u32,
             a8: u32,
             a9: u32,
             a10: u32,
             a11: u32,
             a12: u32,
             a13: u32,
             a14: u32,
             a15: u32| {
                (
                    a15, a14, a13, a12, a11, a10, a9, a8, a7, a6, a5, a4, a3, a2, a1, a0,
                )
            },
        )
        .id();
    let mut net = b.build().unwrap();

    let info = net.transitions().next().unwrap().1;
    assert_eq!((info.inputs.len(), info.outputs.len()), (16, 16));
    assert!(info.positional);

    assert_eq!(net.fireable(), [sixteen]);
    net.fire(sixteen).unwrap();
    for place in ins {
        assert_eq!(net.count(place.id()), 0, "every input taken");
    }
    let landed: Vec<Vec<u32>> = outs.iter().map(|&place| tokens(&net, place)).collect();
    let expected: Vec<Vec<u32>> = (0..16).rev().map(|n| vec![n]).collect();
    assert_eq!(landed, expected, "Out i holds what In 15 - i held");
    assert!(net.fireable().is_empty());
}
