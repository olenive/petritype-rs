//! Shop: a survey edge, which reads a place that may be empty (DESIGN §4.4).
//!
//! The survey half of the edges-on-empty-places proposal's example
//! (`dev-docs/EDGES_ON_EMPTY_PLACES.md` §2), built as the conformance net
//! `shop` of the shared protocol (`PROTOCOL.md` §11), which lives in [`net`]
//! so that a runner can carry the same file. A shop prices orders as they
//! come in, applying whatever promotions happen to be running; a shop with
//! no promotion running still sells things.
//!
//! `Price Order` reads `Promotions` through a [`SurveyEdge`]: every token,
//! zero or more, cloned, none consumed. The edge does not hold its
//! transition back on an empty place, which is the point: without it the
//! transition needs an "empty twin" guarded on the place being empty, and a
//! transition reading *k* such places needs 2^*k* of them. The promotions
//! are read and left running, never used up. The other edge an empty place
//! satisfies, the drain, which takes every token, is the traffic light's
//! (`examples/traffic_light.rs`).
//!
//! `main` runs the script of the shared transcript
//! `07_empty_place_edges.json` — an order priced with no promotion running,
//! two promotions started, an order priced with both, then nothing left to
//! fire — and prints the marking after each step, then shows that `Price
//! Order` waits for an order however many promotions are running. The tests
//! below hold the table, the net's shape as §11 has it, and what the survey
//! does to the event log.
//!
//! [`SurveyEdge`]: petritype::SurveyEdge

// A crate root's `mod` looks beside it, so the path is spelled out: the net
// lives in `shop/net.rs`, the file a runner's example can include too.
#[path = "shop/net.rs"]
mod net;

use std::error::Error;

use petritype::{Net, PlaceRef, TransitionId};

use net::{Shop, build_shop};

/// The columns of the table, after the step's name.
const COLUMNS: [&str; 3] = ["Priced", "Discounts Applied", "Promotions"];

/// One row of the table: the step's name, and `Priced`, `Discounts Applied`
/// and `Promotions` after it.
type Row = (&'static str, [Vec<String>; 3]);

/// The strings in a place, in queue order.
fn strings(net: &Net, place: PlaceRef<String>) -> Vec<String> {
    net.tokens(place.id())
        .filter_map(|(token, _)| net.token::<String>(place.id(), token).cloned())
        .collect()
}

/// The three columns as they stand.
fn columns(shop: &Shop) -> [Vec<String>; 3] {
    [
        strings(&shop.net, shop.priced),
        strings(&shop.net, shop.discounts_applied),
        strings(&shop.net, shop.promotions),
    ]
}

/// Steps the net once with the default selector and checks that `expected`
/// is what fired.
fn step(shop: &mut Shop, expected: TransitionId) -> Result<(), Box<dyn Error>> {
    match shop.net.step()? {
        Some(fired) if fired == expected => Ok(()),
        other => Err(format!("expected {expected:?} to fire, got {other:?}").into()),
    }
}

/// The transcript's script on a fresh shop, one row per step that fires.
fn script(shop: &mut Shop) -> Result<Vec<Row>, Box<dyn Error>> {
    let mut rows = Vec::new();

    shop.net.put(shop.orders, "Order 1".to_owned());
    step(shop, shop.price_order)?;
    rows.push(("Price Order (no promotions)", columns(shop)));

    shop.net.extend(
        shop.promotions,
        ["10% off".to_owned(), "Free shipping".to_owned()],
    );
    if !shop.net.fireable().is_empty() {
        return Err("promotions alone made something fireable".into());
    }
    shop.net.put(shop.orders, "Order 2".to_owned());
    step(shop, shop.price_order)?;
    rows.push(("Price Order (two running)", columns(shop)));

    if let Some(fired) = shop.net.step()? {
        return Err(format!("nothing should fire, {fired:?} did").into());
    }
    Ok(rows)
}

/// A list as the table writes it: `[Order 1, Order 2]`.
fn list(items: &[String]) -> String {
    format!("[{}]", items.join(", "))
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut shop = build_shop()?;
    println!("{}", petritype_dot::dot(&shop.net));

    println!("| After | {} |", COLUMNS.join(" | "));
    println!("|---|---|---|---|");
    for (after, columns) in script(&mut shop)? {
        let cells: Vec<String> = columns.iter().map(|column| list(column)).collect();
        println!("| {after} | {} |", cells.join(" | "));
    }
    println!(
        "\nQuiescent, the promotions still running: {}",
        shop.net.fireable().is_empty() && shop.net.count(shop.promotions.id()) == 2
    );

    // However many promotions are running, nothing is priced without an
    // order: the survey edge never holds `Price Order` back, and never
    // makes it fireable either.
    let mut fresh = build_shop()?;
    for promotion in ["10% off", "Free shipping", "Two for one"] {
        fresh.net.put(fresh.promotions, promotion.to_owned());
    }
    println!(
        "With {} promotions running and no order, Price Order is fireable: {}",
        fresh.net.count(fresh.promotions.id()),
        fresh.net.is_fireable(fresh.price_order)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use petritype::serde_json::{self, json};
    use petritype::{EdgeKind, Event, JsonCommand};

    /// The table: after each step that fires, `Priced`, `Discounts Applied`
    /// and `Promotions`.
    const TABLE: [(&str, [&[&str]; 3]); 2] = [
        ("Price Order (no promotions)", [&["Order 1"], &[], &[]]),
        (
            "Price Order (two running)",
            [
                &["Order 1", "Order 2"],
                &["10% off on Order 2", "Free shipping on Order 2"],
                &["10% off", "Free shipping"],
            ],
        ),
    ];

    #[test]
    fn the_net_is_the_conformance_net_of_protocol_section_11() {
        let shop = build_shop().expect("the net builds");
        let net = &shop.net;
        let places: Vec<(&str, bool)> = net
            .places()
            .map(|(_, info)| (info.name, info.serializable))
            .collect();
        assert_eq!(
            places,
            [
                ("Orders", true),
                ("Promotions", true),
                ("Priced", true),
                ("Discounts Applied", true),
            ]
        );
        assert!(
            net.places().all(|(place, _)| net.count(place) == 0),
            "empty everywhere"
        );

        let transitions: Vec<(&str, &[_], usize, bool)> = net
            .transitions()
            .map(|(_, info)| (info.name, info.inputs, info.outputs.len(), info.enabled))
            .collect();
        assert_eq!(
            transitions,
            [(
                "Price Order",
                &[
                    (shop.orders.id(), EdgeKind::Take),
                    (shop.promotions.id(), EdgeKind::Survey)
                ][..],
                2,
                true
            )]
        );
        let (_, info) = net.transitions().next().unwrap();
        assert_eq!(
            info.outputs,
            [shop.priced.id(), shop.discounts_applied.id()]
        );
        assert!(info.positional);
        assert!(info.params.is_empty());
        assert!(net.fireable().is_empty(), "nothing to take");

        // The kinds on the core's own serialised edges, as a runner names
        // them on the wire (PROTOCOL.md §5.1).
        let topology = serde_json::to_value(net.topology()).unwrap();
        let kinds: Vec<&str> = topology["edges"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|edge| edge["kind"].as_str())
            .collect();
        assert_eq!(kinds, ["take", "survey"]);
    }

    #[test]
    fn the_script_gives_the_table() {
        let mut shop = build_shop().expect("the net builds");
        let rows = script(&mut shop).expect("every step fires what it should");
        assert_eq!(rows.len(), TABLE.len());
        for ((after, columns), (expected_after, expected)) in rows.iter().zip(TABLE) {
            assert_eq!(*after, expected_after);
            assert_eq!(*columns, expected, "after {after}");
        }
        assert_eq!(shop.net.count(shop.orders.id()), 0, "every order priced");
        assert_eq!(shop.net.fired_count(), 2);
    }

    /// `Price Order` is absent from `fireable()` while `Orders` is empty,
    /// whatever `Promotions` holds, and present once an order is there,
    /// whatever `Promotions` holds.
    #[test]
    fn price_order_waits_for_an_order_and_nothing_else() {
        let mut shop = build_shop().expect("the net builds");
        for promotion in ["10% off", "Free shipping"] {
            shop.net.put(shop.promotions, promotion.to_owned());
            assert!(!shop.net.fireable().contains(&shop.price_order));
        }
        shop.net.put(shop.orders, "Order 1".to_owned());
        assert_eq!(shop.net.fireable(), [shop.price_order]);

        let mut empty = build_shop().expect("the net builds");
        empty.net.put(empty.orders, "Order 1".to_owned());
        assert_eq!(empty.net.fireable(), [empty.price_order], "no promotions");
    }

    /// A survey leaves the promotions where they are, under their ids, and
    /// the firing consumes the order alone.
    #[test]
    fn the_events_of_a_survey() {
        let mut shop = build_shop().expect("the net builds");
        let running = shop.net.extend(
            shop.promotions,
            ["10% off".to_owned(), "Free shipping".to_owned()],
        );
        let order = shop.net.put(shop.orders, "Order 1".to_owned());
        shop.net.drain_events().for_each(drop);

        shop.net.step().unwrap();
        let events: Vec<Event> = shop.net.drain_events().collect();
        assert!(
            matches!(&events[..], [Event::Fired { consumed, produced, .. }]
                if consumed == &[(shop.orders.id(), order)] && produced.len() == 3),
            "{events:?}"
        );
        let still: Vec<_> = shop
            .net
            .tokens(shop.promotions.id())
            .map(|(id, _)| id)
            .collect();
        assert_eq!(still, running);
    }

    /// The wire's `inject` is a JSON `extend` of one string into a place,
    /// which every place takes; the script driven that way ends as the table
    /// does.
    #[test]
    fn the_script_through_json_commands() {
        let mut shop = build_shop().expect("the net builds");
        let inject = |shop: &mut Shop, place: &str, token: &str| {
            shop.net
                .apply_json(JsonCommand::Extend {
                    place: place.to_owned(),
                    tokens: vec![json!(token)],
                })
                .expect("every place is serializable");
        };
        inject(&mut shop, "Orders", "Order 1");
        shop.net.step().unwrap();
        inject(&mut shop, "Promotions", "10% off");
        inject(&mut shop, "Promotions", "Free shipping");
        assert!(shop.net.fireable().is_empty());
        inject(&mut shop, "Orders", "Order 2");
        shop.net.step().unwrap();
        assert_eq!(columns(&shop), TABLE[1].1);
        assert_eq!(shop.net.step().unwrap(), None);
    }
}
