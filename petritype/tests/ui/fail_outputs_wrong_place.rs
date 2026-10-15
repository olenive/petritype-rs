//! A field of a returned struct wired to a place of another type: a `Note`
//! cannot go to a place of orders.

use petritype::{NetBuilder, Outputs, TakeEdge};

#[derive(Debug, Clone)]
struct Order(u32);

#[derive(Debug, Clone)]
struct Note(String);

#[derive(Outputs)]
struct Priced {
    order: Order,
    note: Option<Note>,
}

fn price(order: Order) -> Priced {
    let note = (order.0 > 100).then(|| Note(String::from("large order")));
    Priced { order, note }
}

fn main() {
    let mut b = NetBuilder::new();
    let orders = b.place::<Order>("Orders");
    let priced = b.place::<Order>("Priced");
    let held = b.place::<Order>("Held");

    b.transition(
        "Price Order",
        (TakeEdge(orders),),
        PricedEdges { order: priced, note: held },
        price,
    );
}
