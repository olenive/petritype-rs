//! A struct of outputs inside a tuple of outputs: it is the whole of the
//! outputs, never one position among them.

use petritype::{NetBuilder, Outputs, TakeEdge};

#[derive(Debug, Clone)]
struct Order(u32);

#[derive(Debug, Clone)]
struct Note(String);

#[derive(Debug, Clone)]
struct Log(String);

#[derive(Outputs)]
struct Priced {
    order: Order,
    note: Option<Note>,
}

fn main() {
    let mut b = NetBuilder::new();
    let orders = b.place::<Order>("Orders");
    let priced = b.place::<Order>("Priced");
    let notes = b.place::<Note>("Notes");
    let log = b.place::<Log>("Log");

    b.transition(
        "Price Order",
        (TakeEdge(orders),),
        (PricedEdges { order: priced, note: notes }, log),
        |order: Order| (Priced { order, note: None }, Log(String::from("priced"))),
    );
}
