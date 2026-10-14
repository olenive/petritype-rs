//! Shop: input edges that accept an empty place (DESIGN §4.4).
//!
//! The example of the edges-on-empty-places proposal
//! (`dev-docs/EDGES_ON_EMPTY_PLACES.md` §2), built as the conformance net
//! `shop` of the shared protocol (`PROTOCOL.md` §11), which lives in [`net`]
//! so that a runner can carry the same file. A shop prices orders as they
//! come in, applying whatever promotions happen to be running, and ends
//! every promotion when the day closes; a shop with no promotion running
//! still sells things.
//!
//! `Price Order` reads `Promotions` through a [`SurveyEdge`]: every token,
//! zero or more, cloned, none consumed. `Close Day` reads it through a
//! [`DrainEdge`]: every token, zero or more, all consumed. Neither edge
//! holds its transition back on an empty place, which is the point: without
//! them each transition needs an "empty twin" guarded on the place being
//! empty, and a transition reading *k* such places needs 2^*k* of them.
//!
//! `main` runs the proposal's §2 script — seed, step, seed, step — and
//! prints its §6 table after each step, then shows that `Price Order` waits
//! for an order however many promotions are running. The tests below hold
//! the table, the net's shape as §11 has it, and what the two edges do to
//! the event log.
//!
//! [`SurveyEdge`]: petritype::SurveyEdge
//! [`DrainEdge`]: petritype::DrainEdge

// A crate root's `mod` looks beside it, so the path is spelled out: the net
// lives in `shop/net.rs`, the file a runner's example can include too.
#[path = "shop/net.rs"]
mod net;

use std::error::Error;

use petritype::{Net, PlaceRef, TransitionId};

use net::{Shop, build_shop};

/// The columns of the proposal's §6 table, after the step's name.
const COLUMNS: [&str; 4] = ["Priced", "Discounts Applied", "Expired", "Promotions"];

/// One row of the table: the step's name, and `Priced`, `Discounts
/// Applied`, `Expired` and `Promotions` after it.
type Row = (&'static str, [Vec<String>; 4]);

/// The strings in a place, in queue order.
fn strings(net: &Net, place: PlaceRef<String>) -> Vec<String> {
    net.tokens(place.id())
        .filter_map(|(token, _)| net.token::<String>(place.id(), token).cloned())
        .collect()
}

/// The four columns as they stand.
fn columns(shop: &Shop) -> [Vec<String>; 4] {
    [
        strings(&shop.net, shop.priced),
        strings(&shop.net, shop.discounts_applied),
        strings(&shop.net, shop.expired),
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

/// The proposal's §2 script on a fresh shop, one row per step.
fn script(shop: &mut Shop) -> Result<Vec<Row>, Box<dyn Error>> {
    let mut rows = Vec::new();

    shop.net.put(shop.orders, "Order 1".to_owned());
    step(shop, shop.price_order)?;
    rows.push(("Price Order (no promotions)", columns(shop)));

    shop.net.extend(
        shop.promotions,
        ["10% off".to_owned(), "Free shipping".to_owned()],
    );
    shop.net.put(shop.orders, "Order 2".to_owned());
    step(shop, shop.price_order)?;
    rows.push(("Price Order (two running)", columns(shop)));

    shop.net.put(shop.day_end, "close".to_owned());
    step(shop, shop.close_day)?;
    rows.push(("Close Day", columns(shop)));

    shop.net.put(shop.day_end, "close".to_owned());
    step(shop, shop.close_day)?;
    rows.push(("Close Day (none running)", columns(shop)));

    Ok(rows)
}

/// A list as the proposal's table writes it: `[Order 1, Order 2]`.
fn list(items: &[String]) -> String {
    format!("[{}]", items.join(", "))
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut shop = build_shop()?;
    println!("{}", petritype_dot::dot(&shop.net));

    println!("| After | {} |", COLUMNS.join(" | "));
    println!("|---|---|---|---|---|");
    for (after, columns) in script(&mut shop)? {
        let cells: Vec<String> = columns.iter().map(|column| list(column)).collect();
        println!("| {after} | {} |", cells.join(" | "));
    }
    println!(
        "\nQuiescent: {}",
        shop.net.fireable().is_empty() && shop.net.step()?.is_none()
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

    /// The proposal's §6 table.
    const TABLE: [(&str, [&[&str]; 4]); 4] = [
        ("Price Order (no promotions)", [&["Order 1"], &[], &[], &[]]),
        (
            "Price Order (two running)",
            [
                &["Order 1", "Order 2"],
                &["10% off on Order 2", "Free shipping on Order 2"],
                &[],
                &["10% off", "Free shipping"],
            ],
        ),
        (
            "Close Day",
            [
                &["Order 1", "Order 2"],
                &["10% off on Order 2", "Free shipping on Order 2"],
                &["10% off expired", "Free shipping expired"],
                &[],
            ],
        ),
        (
            "Close Day (none running)",
            [
                &["Order 1", "Order 2"],
                &["10% off on Order 2", "Free shipping on Order 2"],
                &["10% off expired", "Free shipping expired"],
                &[],
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
                ("Day End", true),
                ("Priced", true),
                ("Discounts Applied", true),
                ("Expired", true),
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
            [
                (
                    "Price Order",
                    &[
                        (shop.orders.id(), EdgeKind::Take),
                        (shop.promotions.id(), EdgeKind::Survey)
                    ][..],
                    2,
                    true
                ),
                (
                    "Close Day",
                    &[
                        (shop.day_end.id(), EdgeKind::Take),
                        (shop.promotions.id(), EdgeKind::Drain)
                    ][..],
                    1,
                    true
                ),
            ]
        );
        let info = |transition: TransitionId| {
            net.transitions()
                .find(|(id, _)| *id == transition)
                .unwrap()
                .1
        };
        assert_eq!(
            info(shop.price_order).outputs,
            [shop.priced.id(), shop.discounts_applied.id()]
        );
        assert_eq!(info(shop.close_day).outputs, [shop.expired.id()]);
        assert!(info(shop.price_order).positional && info(shop.close_day).positional);
        assert!(info(shop.price_order).params.is_empty());
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
        assert_eq!(kinds, ["take", "survey", "take", "drain"]);
    }

    #[test]
    fn the_script_gives_the_proposals_table() {
        let mut shop = build_shop().expect("the net builds");
        let rows = script(&mut shop).expect("every step fires what it should");
        assert_eq!(rows.len(), TABLE.len());
        for ((after, columns), (expected_after, expected)) in rows.iter().zip(TABLE) {
            assert_eq!(*after, expected_after);
            assert_eq!(*columns, expected, "after {after}");
        }
        assert_eq!(shop.net.count(shop.day_end.id()), 0, "both markers spent");
        assert_eq!(shop.net.step().unwrap(), None, "then quiescent");
        assert_eq!(shop.net.fired_count(), 4);
    }

    /// `Price Order` is absent from `fireable()` while `Orders` is empty,
    /// whatever `Promotions` holds; `Close Day` likewise waits for its
    /// marker, not for a promotion.
    #[test]
    fn each_transition_waits_for_what_it_takes_and_nothing_else() {
        let mut shop = build_shop().expect("the net builds");
        for promotion in ["10% off", "Free shipping"] {
            shop.net.put(shop.promotions, promotion.to_owned());
            assert!(!shop.net.fireable().contains(&shop.price_order));
            assert!(!shop.net.fireable().contains(&shop.close_day));
        }
        shop.net.put(shop.orders, "Order 1".to_owned());
        assert_eq!(shop.net.fireable(), [shop.price_order]);

        let mut empty = build_shop().expect("the net builds");
        empty.net.put(empty.day_end, "close".to_owned());
        assert_eq!(empty.net.fireable(), [empty.close_day], "no promotions");
    }

    /// A survey leaves the promotions where they are, under their ids, and
    /// the firing consumes the order alone; a drain consumes every one, and
    /// the firing says so.
    #[test]
    fn the_events_of_a_survey_and_a_drain() {
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

        let marker = shop.net.put(shop.day_end, "close".to_owned());
        shop.net.drain_events().for_each(drop);
        shop.net.step().unwrap();
        let events: Vec<Event> = shop.net.drain_events().collect();
        assert!(
            matches!(&events[..], [Event::Fired { consumed, .. }] if consumed == &[
                (shop.day_end.id(), marker),
                (shop.promotions.id(), running[0]),
                (shop.promotions.id(), running[1]),
            ]),
            "{events:?}"
        );
    }

    /// The wire's `inject` is a JSON `extend` of one string into a place,
    /// which every place takes; the script driven that way gives the same
    /// table.
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
        inject(&mut shop, "Orders", "Order 2");
        shop.net.step().unwrap();
        inject(&mut shop, "Day End", "close");
        shop.net.step().unwrap();
        inject(&mut shop, "Day End", "close");
        shop.net.step().unwrap();
        assert_eq!(columns(&shop), TABLE[3].1);
        assert_eq!(shop.net.step().unwrap(), None);
    }
}
