//! The shop net: orders priced with whatever promotions are running.
//!
//! This is the conformance net `shop` of the shared protocol
//! (`../petritype/PROTOCOL.md` §11), the survey half of the
//! edges-on-empty-places proposal's example (`dev-docs/EDGES_ON_EMPTY_PLACES.md`
//! §2): places, transitions, edges and outputs exactly as the two runtimes
//! agreed them, so that a transcript recorded against the Python runner can
//! replay against this one. It depends on `petritype` only, so that a
//! runner's example binary can include this file as it is.
//!
//! Every place holds strings and starts empty; every place is serializable,
//! so that a host can `inject` into any of them. `Price Order` takes the
//! front of `Orders` and *surveys* `Promotions` — every promotion running,
//! zero or more, none consumed — and writes the order to `Priced` and one
//! `"{promotion} on {order}"` per promotion to `Discounts Applied`.
//!
//! A shop with no promotion running still sells things: the survey does not
//! hold `Price Order` back on an empty `Promotions` ([`SurveyEdge`] does not
//! require presence), and without it the transition would need an "empty
//! twin", guarded on the place being empty. `Price Order` waits only for
//! what it takes, an order. The drain edge, which takes every token rather
//! than reading it, has an example of its own: the traffic light
//! (`examples/traffic_light/net.rs`).

use petritype::{BuildError, Net, NetBuilder, PlaceRef, SurveyEdge, TakeEdge, TransitionId};

/// The net, its places and its transition.
pub struct Shop {
    pub net: Net,
    /// Orders waiting to be priced.
    pub orders: PlaceRef<String>,
    /// Promotions running right now, usually none.
    pub promotions: PlaceRef<String>,
    /// One per order priced.
    pub priced: PlaceRef<String>,
    /// One per promotion applied to an order.
    pub discounts_applied: PlaceRef<String>,
    /// `Price Order`: the front order, priced with every promotion running.
    pub price_order: TransitionId,
}

/// `Price Order`'s body: the order, for `Priced`, and one discount per
/// promotion running, for `Discounts Applied` — none when none is running.
pub fn apply_promotions(order: String, promotions: Vec<String>) -> (String, Vec<String>) {
    let applied = promotions
        .iter()
        .map(|promotion| format!("{promotion} on {order}"))
        .collect();
    (order, applied)
}

/// The shop as §11 seeds it: empty everywhere, its transition switched on,
/// no parameters.
pub fn build_shop() -> Result<Shop, BuildError> {
    let mut b = NetBuilder::new();
    let orders = b.place::<String>("Orders");
    let promotions = b.place::<String>("Promotions");
    let priced = b.place::<String>("Priced");
    let discounts_applied = b.place::<String>("Discounts Applied");
    // Every place takes a token injected over the wire: the two the
    // transition reads, and the two it writes.
    for place in [orders, promotions, priced, discounts_applied] {
        b.serializable(place);
    }

    let price_order = b
        .transition(
            "Price Order",
            (TakeEdge(orders), SurveyEdge(promotions)),
            (priced, discounts_applied),
            apply_promotions,
        )
        .id();

    Ok(Shop {
        net: b.build()?,
        orders,
        promotions,
        priced,
        discounts_applied,
        price_order,
    })
}
