//! The shop net: orders priced with whatever promotions are running, and
//! every promotion ended when the day closes.
//!
//! This is the conformance net `shop` of the shared protocol
//! (`../petritype/PROTOCOL.md` §11), the example of the edges-on-empty-places
//! proposal (`dev-docs/EDGES_ON_EMPTY_PLACES.md` §2): places, transitions,
//! edges and outputs exactly as the two runtimes agreed them, so that a
//! transcript recorded against the Python runner can replay against this
//! one. It depends on `petritype` only, so that a runner's example binary
//! can include this file as it is.
//!
//! Every place holds strings and starts empty; every place is serializable,
//! so that a host can `inject` into any of them. `Price Order` takes the
//! front of `Orders` and *surveys* `Promotions` — every promotion running,
//! zero or more, none consumed — and writes the order to `Priced` and one
//! `"{promotion} on {order}"` per promotion to `Discounts Applied`. `Close
//! Day` takes the marker at the front of `Day End` and *drains*
//! `Promotions` — every promotion, zero or more, all consumed — and writes
//! one `"{promotion} expired"` per promotion to `Expired`.
//!
//! A shop with no promotion running still sells things: neither edge holds
//! its transition back on an empty `Promotions` ([`SurveyEdge`] and
//! [`DrainEdge`] do not require presence), and without them each transition
//! would need an "empty twin", guarded on the place being empty. The
//! transitions wait only for what they take: an order, or the day's end.

use petritype::{
    BuildError, DrainEdge, Net, NetBuilder, PlaceRef, SurveyEdge, TakeEdge, TransitionId,
};

/// The net, its places and its transitions.
pub struct Shop {
    pub net: Net,
    /// Orders waiting to be priced.
    pub orders: PlaceRef<String>,
    /// Promotions running right now, usually none.
    pub promotions: PlaceRef<String>,
    /// One marker token when the shop closes.
    pub day_end: PlaceRef<String>,
    /// One per order priced.
    pub priced: PlaceRef<String>,
    /// One per promotion applied to an order.
    pub discounts_applied: PlaceRef<String>,
    /// One per promotion that ended at the close of the day.
    pub expired: PlaceRef<String>,
    /// `Price Order`: the front order, priced with every promotion running.
    pub price_order: TransitionId,
    /// `Close Day`: the day's end, which ends every promotion running.
    pub close_day: TransitionId,
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

/// `Close Day`'s body: the marker is spent, and every promotion that was
/// running has expired — none, on a day without one.
pub fn end_promotions(_close: String, promotions: Vec<String>) -> (Vec<String>,) {
    (promotions
        .into_iter()
        .map(|promotion| format!("{promotion} expired"))
        .collect(),)
}

/// The shop as §11 seeds it: empty everywhere, both transitions switched on,
/// no parameters.
pub fn build_shop() -> Result<Shop, BuildError> {
    let mut b = NetBuilder::new();
    let orders = b.place::<String>("Orders");
    let promotions = b.place::<String>("Promotions");
    let day_end = b.place::<String>("Day End");
    let priced = b.place::<String>("Priced");
    let discounts_applied = b.place::<String>("Discounts Applied");
    let expired = b.place::<String>("Expired");
    // Every place takes a token injected over the wire: the three the
    // transitions read, and the three they write.
    for place in [orders, promotions, day_end] {
        b.serializable(place);
    }
    for place in [priced, discounts_applied, expired] {
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
    let close_day = b
        .transition(
            "Close Day",
            (TakeEdge(day_end), DrainEdge(promotions)),
            (expired,),
            end_promotions,
        )
        .id();

    Ok(Shop {
        net: b.build()?,
        orders,
        promotions,
        day_end,
        priced,
        discounts_applied,
        expired,
        price_order,
        close_day,
    })
}
