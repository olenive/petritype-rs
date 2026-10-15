//! Pricing: one library function wired into two nets by field name (DESIGN
//! §4.4, §4.5).
//!
//! The example of `dev-docs/BINDING_BY_NAME.md` §2. [`library`] is a
//! pricing library: [`price`](library::price) takes a
//! [`Pricing`](library::Pricing) and returns a [`Priced`](library::Priced),
//! structs with one field per input and per output that name no place and
//! no kind of edge. `#[derive(Inputs)]` and `#[derive(Outputs)]` on them
//! generate `PricingEdges` and `PricedEdges`, the same fields as structs of
//! edges, and [`nets`] wires the library twice:
//!
//! - a shop, which takes each order, surveys the promotions running and
//!   reads the price list off a parameter, and prices gift orders the same
//!   way with a stamp beside the struct (the mixed form);
//! - a wholesaler, which takes trade orders only, through a filter, uses
//!   each promotion up, reads the price list off a place, and sends what
//!   comes out to places of its own.
//!
//! `main` runs both and prints what came out. The tests hold what the
//! proposal's §6 asks of them: what is priced and noted, the promotions
//! drained and restored, the mixed form, and that the net sees each struct
//! as plain edges in field order.

// A crate root's `mod` looks beside it, so the paths are spelled out.
#[path = "pricing/library.rs"]
mod library;
#[path = "pricing/nets.rs"]
mod nets;

use std::error::Error;

use petritype::{Net, PlaceRef};

use library::{Order, Prices, Promo, Stamp};
use nets::{build_shop, build_wholesale};

/// The price list both nets use.
fn prices() -> Prices {
    Prices::of(&[("tea", 400), ("cake", 350), ("scone", 250)])
}

/// The values in a place, in queue order.
fn values<T: Clone + 'static>(net: &Net, place: PlaceRef<T>) -> Vec<T> {
    net.tokens(place.id())
        .filter_map(|(token, _)| net.token::<T>(place.id(), token).cloned())
        .collect()
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut shop = build_shop(prices())?;
    println!("{}", petritype_dot::dot(&shop.net));

    shop.net
        .put(shop.orders, Order::retail("Ada", &["tea", "cake"]));
    shop.net.fire(shop.price_order)?;
    shop.net.extend(
        shop.promos,
        [Promo::new("Spring sale", 10), Promo::new("Loyalty", 5)],
    );
    shop.net
        .put(shop.orders, Order::retail("Bo", &["tea", "crumpet"]));
    shop.net.fire(shop.price_order)?;
    shop.net.put(shop.gifts, Order::retail("Cy", &["scone"]));
    shop.net
        .put(shop.stamps, Stamp(String::from("Happy birthday, Cy")));
    shop.net.fire(shop.price_and_stamp)?;

    println!("The shop:");
    for order in values(&shop.net, shop.priced) {
        println!("  priced: {} {:?} cents", order.customer, order.total);
    }
    for note in values(&shop.net, shop.notes) {
        println!("  note: {}", note.0);
    }
    for item in values(&shop.net, shop.restock) {
        println!("  restock: {}", item.0);
    }

    let mut wholesale = build_wholesale(prices())?;
    wholesale.net.extend(
        wholesale.orders,
        [
            Order::retail("Dee", &["tea"]),
            Order::trade("Tea Rooms Ltd", &["tea", "cake", "cake", "crumpet"]),
        ],
    );
    wholesale
        .net
        .put(wholesale.promos, Promo::new("Trade fair", 20));
    wholesale.net.fire(wholesale.price_trade_order)?;

    println!("The wholesaler:");
    for order in values(&wholesale.net, wholesale.invoiced) {
        println!("  invoiced: {} {:?} cents", order.customer, order.total);
    }
    for note in values(&wholesale.net, wholesale.audit) {
        println!("  audit: {}", note.0);
    }
    for item in values(&wholesale.net, wholesale.warehouse) {
        println!("  to the warehouse: {}", item.0);
    }
    println!(
        "  left waiting: {}; promotions left: {}; price lists: {}",
        values(&wholesale.net, wholesale.orders)
            .iter()
            .map(|order| order.customer.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        wholesale.net.count(wholesale.promos.id()),
        wholesale.net.count(wholesale.price_list.id())
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use library::{Item, Note, Priced, Pricing};
    use nets::build_wholesale_with;
    use petritype::{EdgeInfo, EdgeKind, NetError};

    /// A body that always fails, for the wholesaler's restore.
    #[derive(Debug)]
    struct Jammed;

    impl std::fmt::Display for Jammed {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("the invoice printer jammed")
        }
    }

    impl Error for Jammed {}

    fn note(text: &str) -> Note {
        Note(text.to_owned())
    }

    // ----- the shop ---------------------------------------------------------

    /// An order with promotions running comes out priced, less their
    /// percentages, with a note; the promotions keep running.
    #[test]
    fn the_shop_prices_an_order_with_the_promotions_running() {
        let mut shop = build_shop(prices()).expect("the shop builds");
        shop.net.extend(
            shop.promos,
            [Promo::new("Spring sale", 10), Promo::new("Loyalty", 5)],
        );
        shop.net
            .put(shop.orders, Order::retail("Ada", &["tea", "cake"]));
        assert_eq!(shop.net.step().unwrap(), Some(shop.price_order));

        let priced = values(&shop.net, shop.priced);
        assert_eq!(priced.len(), 1);
        assert_eq!(priced[0].total, Some(750 * 85 / 100));
        assert_eq!(
            values(&shop.net, shop.notes),
            [note("15% off Ada's order (Spring sale, Loyalty)")]
        );
        assert!(values(&shop.net, shop.restock).is_empty());
        assert_eq!(shop.net.count(shop.promos.id()), 2, "surveyed, not used up");
    }

    /// With no promotion running the shop still sells, at full price and
    /// with no note; an item the list does not carry goes to be restocked.
    #[test]
    fn the_shop_sells_without_promotions_and_restocks_what_it_lacks() {
        let mut shop = build_shop(prices()).expect("the shop builds");
        shop.net
            .put(shop.orders, Order::retail("Bo", &["scone", "crumpet"]));
        assert_eq!(shop.net.step().unwrap(), Some(shop.price_order));
        assert_eq!(values(&shop.net, shop.priced)[0].total, Some(250));
        assert!(values(&shop.net, shop.notes).is_empty());
        assert_eq!(
            values(&shop.net, shop.restock),
            [Item(String::from("crumpet"))]
        );
        assert_eq!(shop.net.step().unwrap(), None, "no order left");
    }

    /// The mixed form: the struct of edges beside a plain edge, the body
    /// taking both. It fires when both a gift order and a stamp are there.
    #[test]
    fn the_mixed_form_prices_and_stamps_a_gift() {
        let mut shop = build_shop(prices()).expect("the shop builds");
        shop.net.put(shop.gifts, Order::retail("Cy", &["scone"]));
        assert!(!shop.net.is_fireable(shop.price_and_stamp), "no stamp yet");
        shop.net
            .put(shop.stamps, Stamp(String::from("Happy birthday")));
        assert_eq!(shop.net.step().unwrap(), Some(shop.price_and_stamp));
        assert_eq!(values(&shop.net, shop.priced)[0].customer, "Cy");
        assert_eq!(values(&shop.net, shop.notes), [note("Happy birthday")]);
        assert_eq!(shop.net.count(shop.stamps.id()), 0);
    }

    /// Each struct is one position to the body and nothing to the net: the
    /// inputs are edges in field order, the parameter beside them, the
    /// outputs edges in field order, and a named return reads back as
    /// positional, its output edges' slots running in field order.
    #[test]
    fn the_net_sees_the_fields_as_edges_in_field_order() {
        let shop = build_shop(prices()).expect("the shop builds");
        let info = |id| {
            shop.net
                .transitions()
                .find(|(transition, _)| *transition == id)
                .unwrap()
                .1
        };
        let price_order = info(shop.price_order);
        assert_eq!(
            price_order.inputs,
            [
                (shop.orders.id(), EdgeKind::Take),
                (shop.promos.id(), EdgeKind::Survey)
            ]
        );
        assert_eq!(price_order.params.len(), 1);
        assert!(price_order.params[0].ends_with("Prices"));
        assert_eq!(
            price_order.outputs,
            [shop.priced.id(), shop.notes.id(), shop.restock.id()]
        );
        assert!(price_order.positional, "a named return is positional (D4)");
        assert_eq!(
            info(shop.price_and_stamp).inputs,
            [
                (shop.gifts.id(), EdgeKind::Take),
                (shop.promos.id(), EdgeKind::Survey),
                (shop.stamps.id(), EdgeKind::Take),
            ]
        );

        let slots: Vec<(petritype::PlaceId, usize)> = shop
            .net
            .edges()
            .filter_map(|edge| match edge {
                EdgeInfo::Output {
                    transition,
                    place,
                    slot,
                    ..
                } if transition == shop.price_order => Some((place, slot)),
                _ => None,
            })
            .collect();
        assert_eq!(
            slots,
            [
                (shop.priced.id(), 0),
                (shop.notes.id(), 1),
                (shop.restock.id(), 2)
            ]
        );
        let topology = shop.net.topology();
        assert!(topology.transitions.iter().all(|t| t.positional));
    }

    // ----- the wholesaler -----------------------------------------------

    /// The same function, wired differently: a trade order is chosen by its
    /// tier past a retail one, every promotion is used up by it, the price
    /// list stays on its place, and the outputs land in the wholesaler's
    /// own places.
    #[test]
    fn the_wholesaler_prices_trade_orders_and_uses_the_promotions_up() {
        let mut wholesale = build_wholesale(prices()).expect("the wholesaler builds");
        wholesale.net.extend(
            wholesale.orders,
            [
                Order::retail("Dee", &["tea"]),
                Order::trade("Tea Rooms Ltd", &["tea", "cake", "cake"]),
            ],
        );
        wholesale.net.extend(
            wholesale.promos,
            [Promo::new("Trade fair", 20), Promo::new("Bulk", 5)],
        );
        assert_eq!(
            wholesale.net.step().unwrap(),
            Some(wholesale.price_trade_order)
        );

        let invoiced = values(&wholesale.net, wholesale.invoiced);
        assert_eq!(invoiced.len(), 1);
        assert_eq!(invoiced[0].customer, "Tea Rooms Ltd");
        assert_eq!(invoiced[0].total, Some(1100 * 75 / 100));
        assert_eq!(
            values(&wholesale.net, wholesale.audit),
            [note("25% off Tea Rooms Ltd's order (Trade fair, Bulk)")]
        );
        assert_eq!(wholesale.net.count(wholesale.promos.id()), 0, "drained");
        assert_eq!(wholesale.net.count(wholesale.price_list.id()), 1);
        assert_eq!(
            values(&wholesale.net, wholesale.orders)[0].customer,
            "Dee",
            "the retail order stays where it was"
        );
        assert_eq!(wholesale.net.step().unwrap(), None, "no trade order left");

        // With no promotion left, the next trade order is priced in full,
        // and what the list does not carry goes to the warehouse.
        wholesale.net.put(
            wholesale.orders,
            Order::trade("Cafe", &["scone", "crumpet"]),
        );
        wholesale.net.step().unwrap();
        assert_eq!(
            values(&wholesale.net, wholesale.invoiced)[1].total,
            Some(250)
        );
        assert_eq!(values(&wholesale.net, wholesale.audit).len(), 1);
        assert_eq!(
            values(&wholesale.net, wholesale.warehouse),
            [Item(String::from("crumpet"))]
        );
    }

    /// When the body fails, `restore_tokens_on_failure` puts back what the
    /// struct's edges consumed: the trade order chosen by the filter and
    /// every promotion the drain took, with their ids.
    #[test]
    fn a_failing_wholesale_body_restores_the_order_and_the_drained_promotions() {
        let mut wholesale =
            build_wholesale_with(prices(), |_: Pricing| -> Result<Priced, Jammed> {
                Err(Jammed)
            })
            .expect("the wholesaler builds");
        wholesale
            .net
            .put(wholesale.orders, Order::trade("Tea Rooms Ltd", &["tea"]));
        let promos = wholesale.net.extend(
            wholesale.promos,
            [Promo::new("Trade fair", 20), Promo::new("Bulk", 5)],
        );
        let orders: Vec<_> = wholesale
            .net
            .tokens(wholesale.orders.id())
            .map(|(id, _)| id)
            .collect();

        let failure = wholesale
            .net
            .fire(wholesale.price_trade_order)
            .expect_err("the body fails");
        assert!(matches!(failure, NetError::FiringFailed(ref f) if f.restored));
        let back = |place: petritype::PlaceId| -> Vec<_> {
            wholesale.net.tokens(place).map(|(id, _)| id).collect()
        };
        assert_eq!(
            back(wholesale.promos.id()),
            promos,
            "every promotion back, ids kept"
        );
        assert_eq!(back(wholesale.orders.id()), orders);
        assert_eq!(wholesale.net.count(wholesale.invoiced.id()), 0);
    }
}
