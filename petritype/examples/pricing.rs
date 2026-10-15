//! Pricing: one library function wired into a net by field name (DESIGN
//! §4.4).
//!
//! The example of `dev-docs/BINDING_BY_NAME.md` §2. [`library`] is a
//! pricing library: [`price`](library::price) takes a
//! [`Pricing`](library::Pricing), a struct with one field per input that
//! names no place and no kind of edge. `#[derive(Inputs)]` on it generates
//! `PricingEdges`, the same fields as a struct of edges, and [`nets`] wires
//! the library into a shop by filling each field with whatever reads its
//! type: the order taken, the promotions surveyed, the prices a parameter.
//!
//! `main` prices two orders, one with promotions running and one without,
//! and prints what came out. The tests hold what the proposal's §6 asks of
//! the shop: what is priced and noted, and that the net sees the struct's
//! fields as plain edges, in field order.

// A crate root's `mod` looks beside it, so the paths are spelled out.
#[path = "pricing/library.rs"]
mod library;
#[path = "pricing/nets.rs"]
mod nets;

use std::error::Error;

use petritype::{Net, PlaceRef};

use library::{Order, Prices, Promo};
use nets::build_shop;

/// The shop's price list.
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

    for order in values(&shop.net, shop.priced) {
        println!("priced: {} {:?} cents", order.customer, order.total);
    }
    for note in values(&shop.net, shop.notes) {
        println!("note: {}", note.0);
    }
    for item in values(&shop.net, shop.restock) {
        println!("restock: {}", item.0);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use library::{Item, Note};
    use petritype::{EdgeInfo, EdgeKind};

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
            [Note(String::from(
                "15% off Ada's order (Spring sale, Loyalty)"
            ))]
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

    /// The struct of edges is one position to the body and nothing to the
    /// net: two edges in field order, the parameter beside them, and no
    /// trace of the struct anywhere a reader looks.
    #[test]
    fn the_net_sees_the_fields_as_edges_in_field_order() {
        let shop = build_shop(prices()).expect("the shop builds");
        let (_, info) = shop.net.transitions().next().unwrap();
        assert_eq!(info.name, "Price Order");
        assert_eq!(
            info.inputs,
            [
                (shop.orders.id(), EdgeKind::Take),
                (shop.promos.id(), EdgeKind::Survey)
            ]
        );
        assert_eq!(info.params.len(), 1);
        assert!(info.params[0].ends_with("Prices"));
        assert_eq!(
            info.outputs,
            [shop.priced.id(), shop.notes.id(), shop.restock.id()]
        );
        let inputs: Vec<EdgeInfo> = shop
            .net
            .edges()
            .filter(|edge| matches!(edge, EdgeInfo::Input { .. }))
            .collect();
        assert_eq!(inputs.len(), 2);
        let topology = shop.net.topology();
        assert_eq!(topology.transitions[0].inputs.len(), 2);
    }
}
