//! A field of a struct of inputs wired to an input of the wrong shape: a
//! `TakeEdge` hands over one token, and the field wants every promotion.

use petritype::{Inputs, NetBuilder, Param, TakeEdge};

#[derive(Debug, Clone)]
struct Order(u32);

#[derive(Debug, Clone)]
struct Promo(u32);

#[derive(Debug, Clone)]
struct Prices(u32);

#[derive(Inputs)]
struct Pricing {
    order: Order,
    promos: Vec<Promo>,
    prices: Prices,
}

fn price(p: Pricing) -> (Order,) {
    let off: u32 = p.promos.iter().map(|promo| promo.0).sum();
    (Order(p.order.0 * p.prices.0 - off),)
}

fn main() {
    let mut b = NetBuilder::new();
    let orders = b.place::<Order>("Orders");
    let promos = b.place::<Promo>("Promotions");
    let priced = b.place::<Order>("Priced");
    b.param(Prices(2));

    b.transition(
        "Price Order",
        (PricingEdges { order: TakeEdge(orders), promos: TakeEdge(promos), prices: Param::new() },),
        (priced,),
        price,
    );
}
