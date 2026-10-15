//! A struct of edges passed as the inputs without the tuple around it. It
//! is one input position like any other, so it goes in the tuple:
//! `(PricingEdges { .. },)`. (Accepting it alone would take a blanket impl
//! that makes every wrong input tuple ambiguous, and its message wrong;
//! DESIGN §4.4.)

use petritype::{Inputs, NetBuilder, SurveyEdge, TakeEdge};

#[derive(Debug, Clone)]
struct Order(u32);

#[derive(Debug, Clone)]
struct Promo(u32);

#[derive(Inputs)]
struct Pricing {
    order: Order,
    promos: Vec<Promo>,
}

fn price(p: Pricing) -> (Order,) {
    (Order(p.order.0 - p.promos.len() as u32),)
}

fn main() {
    let mut b = NetBuilder::new();
    let orders = b.place::<Order>("Orders");
    let promos = b.place::<Promo>("Promotions");
    let priced = b.place::<Order>("Priced");

    b.transition(
        "Price Order",
        PricingEdges { order: TakeEdge(orders), promos: SurveyEdge(promos) },
        (priced,),
        price,
    );
}
