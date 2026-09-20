//! Read arcs in action: a scaler whose factor is read, not consumed.
//!
//! A port of the Python `examples/interactive/read_arc_scaler.py`. That one
//! is a marimo notebook with buttons and a refresh clock; this is the net and
//! its behaviour, driven from `main` instead of from a UI.
//!
//! `Scale` **consumes** an item from `Inbox`, **reads** two places without
//! consuming them, and writes `item × multiplier` to `Outbox`:
//!
//! - `Multiplier` through a [`SnapshotEdge`] — a read-only factor, cloned
//!   when the firing starts, so the transition cannot disturb it. Change it
//!   between runs and later items scale differently.
//! - `Processed` through a [`MutateEdge`] — a running tally the body
//!   increments *in place*, which the net reports as an
//!   [`Event::Mutated`] per firing rather than as a new token.
//!
//! Both places stay non-empty throughout, which is what a read arc needs:
//! all four edge kinds require their place to hold a token before the
//! transition is fireable, including the two that consume nothing.

use petritype::drivers::{self, Limits, Mode, OnFailure};
use petritype::{BuildError, Event, MutateEdge, Net, NetBuilder, PlaceRef, SnapshotEdge, TakeEdge};

/// The running total of items scaled, borrowed in place by every firing.
#[derive(Debug, Clone, PartialEq)]
struct Tally {
    n: u32,
}

/// The net and the four places the example reads and writes.
struct Scaler {
    net: Net,
    inbox: PlaceRef<i64>,
    multiplier: PlaceRef<i64>,
    processed: PlaceRef<Tally>,
    outbox: PlaceRef<i64>,
}

/// `Inbox -> Scale -> Outbox`, with `Multiplier` read and `Processed` bumped.
fn build() -> Result<Scaler, BuildError> {
    let mut b = NetBuilder::new();
    let inbox = b.place::<i64>("Inbox");
    let multiplier = b.place::<i64>("Multiplier");
    let processed = b.place::<Tally>("Processed");
    let outbox = b.place::<i64>("Outbox");
    b.seed(multiplier, [2]);
    b.seed(processed, [Tally { n: 0 }]);

    b.transition(
        "Scale",
        (
            TakeEdge(inbox),
            SnapshotEdge(multiplier),
            MutateEdge(processed),
        ),
        (outbox,),
        |item: i64, factor: i64, tally: &mut Tally| {
            tally.n += 1;
            (item * factor,)
        },
    );

    Ok(Scaler {
        net: b.build()?,
        inbox,
        multiplier,
        processed,
        outbox,
    })
}

/// Runs to quiescence and reports how many tokens were mutated in place.
fn drain_to_quiescence(scaler: &mut Scaler) -> usize {
    drivers::run(
        &mut scaler.net,
        Mode::Sequential,
        Limits::default(),
        OnFailure::Stop,
    )
    .expect("nothing fails");
    let processed = scaler.processed.id();
    scaler
        .net
        .drain_events()
        .filter(|event| matches!(event, Event::Mutated { place, .. } if *place == processed))
        .count()
}

/// Reads a place back as plain values, oldest first.
fn values<T: Clone + Send + Sync + 'static>(net: &Net, place: PlaceRef<T>) -> Vec<T> {
    net.tokens(place.id())
        .filter_map(|(_, value)| value.downcast_ref::<T>())
        .cloned()
        .collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut scaler = build()?;
    println!("{}", petritype_dot::dot(&scaler.net));

    // Three items at the starting factor.
    scaler.net.extend(scaler.inbox, [1, 2, 3]);
    let first = drain_to_quiescence(&mut scaler);
    println!(
        "×2:  {:?} ({first} tokens mutated)",
        values(&scaler.net, scaler.outbox)
    );

    // The ➕ button: replace the factor, then send two more items through.
    scaler.net.set_tokens(scaler.multiplier, [10]);
    scaler.net.extend(scaler.inbox, [4, 5]);
    let second = drain_to_quiescence(&mut scaler);
    println!(
        "×10: {:?} ({second} tokens mutated)",
        values(&scaler.net, scaler.outbox)
    );

    println!("processed: {:?}", values(&scaler.net, scaler.processed));
    println!(
        "multiplier still there: {:?}",
        values(&scaler.net, scaler.multiplier)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Tally, build, drain_to_quiescence, values};

    /// The whole session of the notebook: three items, a bigger factor, two
    /// more items. Returns the scaler and the mutation counts of both runs.
    fn run() -> (super::Scaler, usize, usize) {
        let mut scaler = build().expect("the net builds");
        scaler.net.extend(scaler.inbox, [1, 2, 3]);
        let first = drain_to_quiescence(&mut scaler);
        scaler.net.set_tokens(scaler.multiplier, [10]);
        scaler.net.extend(scaler.inbox, [4, 5]);
        let second = drain_to_quiescence(&mut scaler);
        (scaler, first, second)
    }

    #[test]
    fn later_items_use_the_factor_that_was_there_when_they_fired() {
        let (scaler, _, _) = run();

        assert_eq!(values(&scaler.net, scaler.outbox), [2, 4, 6, 40, 50]);
    }

    #[test]
    fn the_read_places_are_never_consumed() {
        let (scaler, _, _) = run();

        assert_eq!(values(&scaler.net, scaler.multiplier), [10]);
        assert_eq!(scaler.net.count(scaler.processed.id()), 1);
        assert_eq!(scaler.net.count(scaler.inbox.id()), 0);
    }

    #[test]
    fn every_firing_bumps_the_tally_in_place_and_says_so() {
        let (scaler, first, second) = run();

        assert_eq!((first, second), (3, 2), "one Mutated event per firing");
        assert_eq!(values(&scaler.net, scaler.processed), [Tally { n: 5 }]);
    }

    #[test]
    fn the_two_read_arcs_are_drawn_differently_from_the_consuming_one() {
        let scaler = build().expect("the net builds");
        let text = petritype_dot::dot(&scaler.net);

        assert!(
            text.contains("  p0 -> t0 [color=black, penwidth=1];"),
            "Inbox is taken"
        );
        assert!(
            text.contains(
                "  p1 -> t0 [style=dashed, color=dimgray, arrowhead=normal, penwidth=1];"
            ),
            "Multiplier is read",
        );
        assert!(
            text.contains(
                "  p2 -> t0 [dir=both, arrowhead=dot, arrowtail=dot, color=dimgray, penwidth=2];"
            ),
            "Processed is read and written",
        );
    }
}
