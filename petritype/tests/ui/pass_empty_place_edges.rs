//! The two edges that accept an empty place hand the body every token there
//! is, possibly none: a `DrainEdge` the tokens it took, a `SurveyEdge` clones
//! of them, both as a `Vec<T>`.

use petritype::{DrainEdge, NetBuilder, SurveyEdge, TakeEdge};

fn main() {
    let mut b = NetBuilder::new();
    let orders = b.place::<String>("Orders");
    let promotions = b.place::<String>("Promotions");
    let notes = b.place::<u32>("Notes");
    let priced = b.place::<String>("Priced");

    b.transition(
        "Price Order",
        (TakeEdge(orders), SurveyEdge(promotions), DrainEdge(notes)),
        (priced,),
        |order: String, running: Vec<String>, taken: Vec<u32>| {
            (format!("{order} with {} and {}", running.len(), taken.len()),)
        },
    );

    let mut net = b.build().unwrap();
    net.put(orders, String::from("Order 1"));
    assert_eq!(net.step().unwrap().map(|t| t.index()), Some(0));
}
