//! A struct of edges with every field wired: the body takes the struct the
//! wiring was derived from, and the net sees one edge per field, in field
//! order. `fail_inputs_missing_field.rs` leaves one out.

use petritype::{EdgeKind, Inputs, NetBuilder, SurveyEdge, TakeEdge};

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
        (PricingEdges { order: TakeEdge(orders), promos: SurveyEdge(promos) },),
        (priced,),
        price,
    );
    let net = b.build().unwrap();
    let (_, info) = net.transitions().next().unwrap();
    assert_eq!(
        info.inputs,
        [(orders.id(), EdgeKind::Take), (promos.id(), EdgeKind::Survey)]
    );
}
