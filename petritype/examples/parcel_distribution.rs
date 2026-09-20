//! A parcel distribution centre: receive, sort, stage, dispatch.
//!
//! A port of the Python `examples/toy/parcel_distribution.py`. That one is a
//! perpetual marimo simulation whose arrivals and truck departures are paced
//! by wall-clock guards and whose left-behind parcels are random; this one
//! keeps the shape of the net and makes it finite and deterministic, because
//! an example that is also a test has to say what it expects.
//!
//! What it demonstrates: [`TakeAllEdge`] for a truck that loads everything on
//! the platform at once, a [guard](petritype::TransitionBuilder::guard) that
//! holds the truck until it is worth dispatching, and an output edge that
//! points back at an input place — the parcels left behind for the next
//! truck.

use petritype::drivers::{self, Limits, Mode, OnFailure};
use petritype::{BuildError, Net, NetBuilder, PlaceRef, TakeAllEdge, TakeEdge};

/// A parcel, and how many trucks have already gone without it.
#[derive(Debug, Clone, PartialEq)]
struct Parcel {
    id: u32,
    deferrals: u32,
}

/// The net and the handles the example needs to read it back.
struct Line {
    net: Net,
    receiving: PlaceRef<Parcel>,
    sorting: PlaceRef<Parcel>,
    staging: PlaceRef<Parcel>,
    dispatched: PlaceRef<Parcel>,
}

/// `Receiving -> Sort -> Sorting -> Stage -> Staging Area -> Dispatch Truck`,
/// seeded with eight parcels.
fn build() -> Result<Line, BuildError> {
    let mut b = NetBuilder::new();
    let receiving = b.place::<Parcel>("Receiving");
    let sorting = b.place::<Parcel>("Sorting");
    let staging = b.place::<Parcel>("Staging Area");
    let dispatched = b.place::<Parcel>("Dispatched");
    b.seed(receiving, (1..=8).map(|id| Parcel { id, deferrals: 0 }));

    b.transition("Sort", (TakeEdge(receiving),), (sorting,), |p: Parcel| (p,));
    b.transition("Stage", (TakeEdge(sorting),), (staging,), |p: Parcel| (p,));
    b.transition(
        "Dispatch Truck",
        (TakeAllEdge(staging),),
        // Slot 0 leaves on the truck, slot 1 waits for the next one.
        (dispatched, staging),
        |parcels: Vec<Parcel>| {
            let (mut gone, mut left) = (Vec::new(), Vec::new());
            for mut parcel in parcels {
                // Every fourth parcel misses its first truck: the Python
                // example rolled a die here, and a test cannot.
                if parcel.id % 4 == 0 && parcel.deferrals == 0 {
                    parcel.deferrals += 1;
                    left.push(parcel);
                } else {
                    gone.push(parcel);
                }
            }
            (gone, left)
        },
    )
    .guard(move |net: &Net| {
        // A truck waits for a load, unless nothing more is coming — which is
        // this net's answer to the Python original's five-second timer.
        net.count(staging.id()) >= 2
            || (net.count(receiving.id()) == 0 && net.count(sorting.id()) == 0)
    });

    Ok(Line {
        net: b.build()?,
        receiving,
        sorting,
        staging,
        dispatched,
    })
}

/// The parcel ids held by a place, oldest first.
fn ids(net: &Net, place: PlaceRef<Parcel>) -> Vec<u32> {
    net.tokens(place.id())
        .filter_map(|(_, value)| value.downcast_ref::<Parcel>())
        .map(|parcel| parcel.id)
        .collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut line = build()?;
    println!("{}", petritype_dot::dot(&line.net));

    let summary = drivers::run(
        &mut line.net,
        Mode::Sequential,
        Limits::default(),
        OnFailure::Stop,
    )?;

    println!(
        "dispatched, in order: {:?}",
        ids(&line.net, line.dispatched)
    );
    println!("still on the platform: {:?}", ids(&line.net, line.staging));
    println!(
        "still upstream: {:?} receiving, {:?} sorting",
        ids(&line.net, line.receiving),
        ids(&line.net, line.sorting),
    );
    println!(
        "{} firings, quiesced: {}",
        summary.fired_count, summary.quiesced
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{build, ids};
    use petritype::drivers::{self, Limits, Mode, OnFailure};

    /// Builds the net and runs it to quiescence.
    fn run() -> super::Line {
        let mut line = build().expect("the net builds");
        let summary = drivers::run(
            &mut line.net,
            Mode::Sequential,
            Limits::default(),
            OnFailure::Stop,
        )
        .expect("nothing fails");
        assert!(summary.quiesced, "the line empties itself");
        // Eight sorted, eight staged, and two trucks.
        assert_eq!(summary.fired_count, 18);
        line
    }

    #[test]
    fn every_parcel_is_dispatched_and_the_line_empties() {
        let line = run();

        assert!(ids(&line.net, line.receiving).is_empty());
        assert!(ids(&line.net, line.sorting).is_empty());
        assert!(ids(&line.net, line.staging).is_empty());
        assert_eq!(ids(&line.net, line.dispatched).len(), 8);
    }

    #[test]
    fn the_deferred_parcels_go_on_the_second_truck() {
        let line = run();

        // The first truck took everything but 4 and 8; the second took those.
        assert_eq!(ids(&line.net, line.dispatched), [1, 2, 3, 5, 6, 7, 4, 8]);
        let deferred: Vec<u32> = line
            .net
            .tokens(line.dispatched.id())
            .filter_map(|(_, value)| value.downcast_ref::<super::Parcel>())
            .filter(|parcel| parcel.deferrals == 1)
            .map(|parcel| parcel.id)
            .collect();
        assert_eq!(deferred, [4, 8]);
    }

    #[test]
    fn the_truck_loads_the_whole_platform_at_once() {
        let line = build().expect("the net builds");
        let text = petritype_dot::dot(&line.net);

        // A `TakeAllEdge` is drawn with a `*`, and the truck writes back to
        // the place it drains.
        assert_eq!(text.matches(r#"label="*""#).count(), 1);
        assert!(text.contains(r#"label="0""#), "dispatched is slot 0");
        assert!(text.contains(r#"label="1""#), "left behind is slot 1");
    }
}
