//! The smallest net there is: `Input -> Double -> Output`.
//!
//! A port of the Python `examples/quickstart.py`, which is the README's
//! quickstart made executable. Three tokens go in, one transition doubles
//! each of them, and [`drivers::run`] fires until there is nothing left to
//! fire. The assertions deliberately do not sort: the order tokens come out
//! in is part of what the example documents.

use petritype::drivers::{self, Limits, Mode, OnFailure};
use petritype::{BuildError, Net, NetBuilder, PlaceRef, TakeEdge};

/// The net: one place in, one place out, one transition between them.
fn build() -> Result<(Net, PlaceRef<i64>, PlaceRef<i64>), BuildError> {
    let mut b = NetBuilder::new();
    let input = b.place::<i64>("Input");
    let output = b.place::<i64>("Output");
    b.seed(input, [1, 2, 3]);
    b.transition("Double", (TakeEdge(input),), (output,), |x: i64| (x * 2,));
    Ok((b.build()?, input, output))
}

/// Reads a place back as plain values, oldest first.
fn values(net: &Net, place: PlaceRef<i64>) -> Vec<i64> {
    net.tokens(place.id())
        .filter_map(|(_, value)| value.downcast_ref::<i64>().copied())
        .collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (mut net, input, output) = build()?;
    println!("{}", petritype_dot::dot(&net));

    let summary = drivers::run(
        &mut net,
        Mode::Sequential,
        Limits::default(),
        OnFailure::Stop,
    )?;

    println!("Input:  {:?}", values(&net, input));
    println!("Output: {:?}", values(&net, output));
    println!(
        "fired {} times, quiesced: {}",
        summary.fired_count, summary.quiesced
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{build, values};
    use petritype::drivers::{self, Limits, Mode, OnFailure};

    #[test]
    fn three_tokens_are_doubled_in_order() {
        let (mut net, input, output) = build().expect("the net builds");
        assert_eq!(values(&net, input), [1, 2, 3]);

        let summary = drivers::run(
            &mut net,
            Mode::Sequential,
            Limits::default(),
            OnFailure::Stop,
        )
        .expect("nothing fails");

        assert_eq!(values(&net, output), [2, 4, 6]);
        assert!(values(&net, input).is_empty());
        assert_eq!(summary.fired_count, 3);
        assert!(summary.quiesced);
    }

    #[test]
    fn the_net_draws_as_two_places_and_one_transition() {
        let (net, _input, _output) = build().expect("the net builds");
        let text = petritype_dot::dot(&net);

        assert!(text.contains(r#"label="Input\n1\n2\n3""#));
        assert!(text.contains(r#"label="Double""#));
        assert_eq!(text.matches(" -> ").count(), 2);
    }
}
