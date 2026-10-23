//! The pricing library: a domain function and the structs it takes and
//! returns, which name no place and no kind of edge.
//!
//! This is the library side of `dev-docs/BINDING_BY_NAME.md` §2. A library
//! like this is written once and wired into many nets, each reading the
//! inputs its own way and sending the outputs to places of its own;
//! `#[derive(Inputs)]` and `#[derive(Outputs)]` are what let a net do that
//! by field name rather than by position, so that no net depends on the
//! order the fields happen to be declared in, and two of one type cannot be
//! swapped by accident.

use std::collections::BTreeMap;

use petritype::{Inputs, Outputs};

/// An item on an order, by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item(pub String);

/// A customer's order: who, at which trade tier, what, and (once priced)
/// what it came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Order {
    /// Who ordered.
    pub customer: String,
    /// `"retail"` or `"trade"`: the wholesale net prices trade orders only.
    pub tier: String,
    /// What they ordered.
    pub items: Vec<Item>,
    /// What the order came to, in cents, once priced.
    pub total: Option<u32>,
}

impl Order {
    /// A retail order, not yet priced.
    pub fn retail(customer: &str, items: &[&str]) -> Self {
        Order::new(customer, "retail", items)
    }

    /// A trade order, not yet priced.
    pub fn trade(customer: &str, items: &[&str]) -> Self {
        Order::new(customer, "trade", items)
    }

    fn new(customer: &str, tier: &str, items: &[&str]) -> Self {
        Order {
            customer: customer.to_owned(),
            tier: tier.to_owned(),
            items: items.iter().map(|&item| Item(item.to_owned())).collect(),
            total: None,
        }
    }

    /// This order with its total.
    pub fn priced(self, total: u32) -> Self {
        Order {
            total: Some(total),
            ..self
        }
    }
}

/// A running promotion: a percentage off every order it applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Promo {
    /// What the shop calls it.
    pub name: String,
    /// How much it takes off, in percent.
    pub percent: u32,
}

impl Promo {
    /// A promotion of that many percent.
    pub fn new(name: &str, percent: u32) -> Self {
        Promo {
            name: name.to_owned(),
            percent,
        }
    }
}

/// The price list: cents per item, by name.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Prices(pub BTreeMap<String, u32>);

impl Prices {
    /// A list of `(item, cents)`.
    pub fn of(list: &[(&str, u32)]) -> Self {
        Prices(
            list.iter()
                .map(|&(item, cents)| (item.to_owned(), cents))
                .collect(),
        )
    }
}

/// What was taken off an order, for whoever keeps the books.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note(pub String);

/// A message stamped on an order, a gift's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp(pub String);

/// What pricing an order takes. A net decides where each comes from: the
/// order taken or chosen by a filter, the promotions surveyed, the prices a
/// parameter or a token on a place.
#[derive(Inputs)]
pub struct Pricing {
    /// The order to price.
    pub order: Order,
    /// The promotions that apply to it, possibly none.
    pub promos: Vec<Promo>,
    /// The price of everything the shop sells.
    pub prices: Prices,
}

/// What pricing an order gives. A net decides where each goes.
#[derive(Outputs)]
pub struct Priced {
    /// The order, with its total: one token.
    pub order: Order,
    /// What was taken off it, if anything: zero tokens or one.
    pub note: Option<Note>,
    /// What it asked for that the list does not carry: any number.
    pub restock: Vec<Item>,
}

impl Priced {
    /// The same, with a stamp's message added to the note.
    pub fn stamped(self, stamp: Stamp) -> Self {
        let note = match self.note {
            Some(Note(note)) => format!("{note}; {}", stamp.0),
            None => stamp.0,
        };
        Priced {
            note: Some(Note(note)),
            ..self
        }
    }
}

/// Prices an order: every item on the list, less every promotion's
/// percentage (all of it at most); a note when something was taken off; and
/// the items the list does not carry, to be restocked, which the order is
/// not charged for.
pub fn price(p: Pricing) -> Priced {
    let percent = p
        .promos
        .iter()
        .map(|promo| promo.percent)
        .sum::<u32>()
        .min(100);
    let (stocked, restock): (Vec<&Item>, Vec<&Item>) = p
        .order
        .items
        .iter()
        .partition(|item| p.prices.0.contains_key(&item.0));
    let full: u32 = stocked.iter().map(|item| p.prices.0[&item.0]).sum();
    let restock = restock.into_iter().cloned().collect();
    let note = (percent > 0).then(|| {
        let names: Vec<&str> = p.promos.iter().map(|promo| promo.name.as_str()).collect();
        Note(format!(
            "{percent}% off {}'s order ({})",
            p.order.customer,
            names.join(", ")
        ))
    });
    Priced {
        order: p.order.priced(full * (100 - percent) / 100),
        note,
        restock,
    }
}
