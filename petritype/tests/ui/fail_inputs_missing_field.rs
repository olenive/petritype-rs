//! A struct of edges with a field left out: the ordinary error, which names
//! the field. `pass_inputs_every_field.rs` is the same wiring complete.

use petritype::{Inputs, NetBuilder, TakeEdge};

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
    let _promos = b.place::<Promo>("Promotions");
    let priced = b.place::<Order>("Priced");

    b.transition(
        "Price Order",
        (PricingEdges { order: TakeEdge(orders) },),
        (priced,),
        price,
    );
}
