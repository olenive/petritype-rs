//! The nets that wire the pricing library, each reading its inputs its own
//! way, with the library unchanged.
//!
//! The net side of `dev-docs/BINDING_BY_NAME.md` §2: where the places are,
//! and which kind of edge fills each field of [`Pricing`].

use petritype::{BuildError, Net, NetBuilder, Param, PlaceRef, SurveyEdge, TakeEdge, TransitionId};

use super::library::{Item, Note, Order, Prices, PricingEdges, Promo, price};

/// A shop: orders priced as they come in, with whatever promotions happen
/// to be running and the price list as a parameter.
pub struct Shop {
    pub net: Net,
    /// Orders waiting to be priced.
    pub orders: PlaceRef<Order>,
    /// Promotions running right now, usually none; pricing reads them and
    /// leaves them running.
    pub promos: PlaceRef<Promo>,
    /// One per order priced.
    pub priced: PlaceRef<Order>,
    /// One per order that had something taken off.
    pub notes: PlaceRef<Note>,
    /// Items ordered that the price list does not carry.
    pub restock: PlaceRef<Item>,
    /// `Price Order`.
    pub price_order: TransitionId,
}

/// The shop, empty, with this price list.
///
/// `Price Order` takes the front order, surveys the promotions (every one,
/// zero or more, none consumed) and reads the prices off the net's
/// parameter.
pub fn build_shop(prices: Prices) -> Result<Shop, BuildError> {
    let mut b = NetBuilder::new();
    let orders = b.place::<Order>("Orders");
    let promos = b.place::<Promo>("Promotions");
    let priced = b.place::<Order>("Priced");
    let notes = b.place::<Note>("Notes");
    let restock = b.place::<Item>("Restock");
    b.param(prices);

    let price_order = b
        .transition(
            "Price Order",
            (PricingEdges {
                order: TakeEdge(orders),
                promos: SurveyEdge(promos),
                prices: Param::new(),
            },),
            (priced, notes, restock),
            price,
        )
        .id();

    Ok(Shop {
        net: b.build()?,
        orders,
        promos,
        priced,
        notes,
        restock,
        price_order,
    })
}
