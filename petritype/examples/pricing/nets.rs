//! The nets that wire the pricing library, each reading its inputs and
//! sending its outputs its own way, with the library unchanged.
//!
//! The net side of `dev-docs/BINDING_BY_NAME.md` §2: where the places are,
//! and which kind of edge fills each field of [`Pricing`] and takes each
//! field of [`Priced`](super::library::Priced). The library names
//! neither.

use petritype::{
    BuildError, Filter, Net, NetBuilder, Param, PlaceRef, SnapshotEdge, SurveyEdge, TakeEdge,
    TakeWhereEdge, TransitionBody, TransitionId,
};

use super::library::{
    Item, Note, Order, PricedEdges, Prices, Pricing, PricingEdges, Promo, Stamp, price,
};

/// A shop: orders priced as they come in, with whatever promotions happen
/// to be running and the price list as a parameter; gift orders the same,
/// with a stamp.
pub struct Shop {
    pub net: Net,
    /// Orders waiting to be priced.
    pub orders: PlaceRef<Order>,
    /// Gift orders waiting to be priced and stamped.
    pub gifts: PlaceRef<Order>,
    /// One stamp per gift order.
    pub stamps: PlaceRef<Stamp>,
    /// Promotions running right now, usually none; pricing reads them and
    /// leaves them running.
    pub promos: PlaceRef<Promo>,
    /// One per order priced.
    pub priced: PlaceRef<Order>,
    /// One per order that had something taken off, or was stamped.
    pub notes: PlaceRef<Note>,
    /// Items ordered that the price list does not carry.
    pub restock: PlaceRef<Item>,
    /// `Price Order`.
    pub price_order: TransitionId,
    /// `Price and Stamp`.
    pub price_and_stamp: TransitionId,
}

/// The shop, empty, with this price list.
///
/// `Price Order` takes the front order, surveys the promotions (every one,
/// zero or more, none consumed) and reads the prices off the net's
/// parameter. `Price and Stamp` is the mixed form: the struct of edges is
/// one input position beside a plain edge, and the body takes the struct
/// and the stamp. Both write to the same places through one struct of
/// outputs, which is a value like any other.
pub fn build_shop(prices: Prices) -> Result<Shop, BuildError> {
    let mut b = NetBuilder::new();
    let orders = b.place::<Order>("Orders");
    let gifts = b.place::<Order>("Gift Orders");
    let stamps = b.place::<Stamp>("Stamps");
    let promos = b.place::<Promo>("Promotions");
    let priced = b.place::<Order>("Priced");
    let notes = b.place::<Note>("Notes");
    let restock = b.place::<Item>("Restock");
    b.param(prices);

    let outputs = PricedEdges {
        order: priced,
        note: notes,
        restock,
    };
    let price_order = b
        .transition(
            "Price Order",
            (PricingEdges {
                order: TakeEdge(orders),
                promos: SurveyEdge(promos),
                prices: Param::new(),
            },),
            outputs,
            price,
        )
        .id();
    let price_and_stamp = b
        .transition(
            "Price and Stamp",
            (
                PricingEdges {
                    order: TakeEdge(gifts),
                    promos: SurveyEdge(promos),
                    prices: Param::new(),
                },
                TakeEdge(stamps),
            ),
            outputs,
            |p: Pricing, stamp: Stamp| price(p).stamped(stamp),
        )
        .id();

    Ok(Shop {
        net: b.build()?,
        orders,
        gifts,
        stamps,
        promos,
        priced,
        notes,
        restock,
        price_order,
        price_and_stamp,
    })
}

/// The wholesale net's inputs: an order chosen by its tier, the promotions
/// surveyed, the prices a token on a place. A type like any other, so the
/// wiring can be named, built once and handed to more than one body.
pub type WholesaleInputs =
    PricingEdges<TakeWhereEdge<Order>, SurveyEdge<Promo>, SnapshotEdge<Prices>>;

/// The wholesale net's outputs: places of its own.
pub type WholesaleOutputs = PricedEdges<PlaceRef<Order>, PlaceRef<Note>, PlaceRef<Item>>;

/// A wholesaler: trade orders only, the promotions read and left running,
/// the price list a token on a place, and every output somewhere else.
pub struct Wholesale {
    pub net: Net,
    /// Orders of every tier; only trade orders are priced here.
    pub orders: PlaceRef<Order>,
    /// Promotions running right now; every trade order priced reads them
    /// and leaves them running.
    pub promos: PlaceRef<Promo>,
    /// The price list, one token, read and left in place.
    pub price_list: PlaceRef<Prices>,
    /// One per trade order priced.
    pub invoiced: PlaceRef<Order>,
    /// One per trade order that had something taken off.
    pub audit: PlaceRef<Note>,
    /// Items ordered that the price list does not carry.
    pub warehouse: PlaceRef<Item>,
    /// `Price Trade Order`.
    pub price_trade_order: TransitionId,
}

/// The wholesaler, with the library's [`price`] as the body.
pub fn build_wholesale(prices: Prices) -> Result<Wholesale, BuildError> {
    build_wholesale_with(prices, price)
}

/// The wholesaler, with any body that takes a [`Pricing`] and returns a
/// [`Priced`](super::library::Priced) (or a `Result` of one): the wiring
/// does not depend on the
/// function, which is the point. `Price Trade Order` restores what it took
/// if its body fails: the order; the promotions it only read, and they
/// never left their place.
pub fn build_wholesale_with<B, M>(prices: Prices, body: B) -> Result<Wholesale, BuildError>
where
    B: TransitionBody<(WholesaleInputs,), WholesaleOutputs, M>,
{
    let mut b = NetBuilder::new();
    let orders = b.place::<Order>("Orders");
    let promos = b.place::<Promo>("Promotions");
    let price_list = b.place::<Prices>("Price List");
    let invoiced = b.place::<Order>("Invoiced");
    let audit = b.place::<Note>("Audit");
    let warehouse = b.place::<Item>("Warehouse");
    b.keys(orders, ["tier"], |order: &Order| {
        [order.tier.as_str().into()]
    });
    b.seed(price_list, [prices]);

    let inputs: WholesaleInputs = PricingEdges {
        order: TakeWhereEdge(orders, Filter::new().key_in("tier", ["trade"])),
        promos: SurveyEdge(promos),
        prices: SnapshotEdge(price_list),
    };
    let price_trade_order = b
        .transition(
            "Price Trade Order",
            (inputs,),
            PricedEdges {
                order: invoiced,
                note: audit,
                restock: warehouse,
            },
            body,
        )
        .restore_tokens_on_failure()
        .id();

    Ok(Wholesale {
        net: b.build()?,
        orders,
        promos,
        price_list,
        invoiced,
        audit,
        warehouse,
        price_trade_order,
    })
}
