//! Make balls out of rubber and paint, and sort them into bins by colour.
//!
//! A port of the Python
//! `examples/toy/distribution_function/01_coloured_balls.py`.
//!
//! What it demonstrates: the third output form. Three output places all hold
//! a `Ball`, so a type cannot say which one a ball belongs in and neither can
//! a slot position — only the value can. A [`Distribution`] is the list of
//! `(place, token)` pairs the firing names itself, and
//! [`output_distribution`](petritype::OutputDistribution::output_distribution)
//! composes the function that builds one *after* the domain function, so
//! `make_a_ball` below never hears about a place.

use petritype::drivers::{self, Limits, Mode, OnFailure};
use petritype::{
    BuildError, Distribution, Net, NetBuilder, OutputDistribution, PlaceRef, TakeEdge,
};

/// Raw material: how much rubber this lump is.
#[derive(Debug, Clone, PartialEq)]
struct Rubber {
    quantity: u32,
}

/// Raw material: what colour this tin is.
#[derive(Debug, Clone, PartialEq)]
struct Paint {
    color: String,
}

/// The product.
#[derive(Debug, Clone, PartialEq)]
struct Ball {
    color: String,
    size: u32,
}

/// The net and the three bins the example reads back.
struct Factory {
    net: Net,
    rubber: PlaceRef<Rubber>,
    red: PlaceRef<Ball>,
    blue: PlaceRef<Ball>,
    other: PlaceRef<Ball>,
}

/// The domain function: one lump, one tin, one ball. No `PlaceRef` in sight.
fn make_a_ball(rubber: Rubber, paint: Paint) -> Ball {
    Ball {
        color: paint.color,
        size: rubber.quantity * 10,
    }
}

/// Ten lumps of rubber, nine tins of paint, and three bins to sort into.
fn build() -> Result<Factory, BuildError> {
    let mut b = NetBuilder::new();
    let rubber = b.place::<Rubber>("Input Rubber");
    let paint = b.place::<Paint>("Input Paint");
    let red = b.place::<Ball>("Red Balls");
    let blue = b.place::<Ball>("Blue Balls");
    let other = b.place::<Ball>("Other Balls");

    b.seed(rubber, (1..=10).map(|quantity| Rubber { quantity }));
    b.seed(
        paint,
        [
            "red", "blue", "green", "yellow", "red", "blue", "green", "yellow", "red",
        ]
        .map(|color| Paint {
            color: color.to_owned(),
        }),
    );

    b.transition(
        "Make a Ball",
        (TakeEdge(rubber), TakeEdge(paint)),
        (red, blue, other),
        make_a_ball.output_distribution(move |ball: Ball| {
            let bin = match ball.color.as_str() {
                "red" => red,
                "blue" => blue,
                _ => other,
            };
            Distribution::new().to(bin, ball)
        }),
    );

    Ok(Factory {
        net: b.build()?,
        rubber,
        red,
        blue,
        other,
    })
}

/// The sizes of the balls in a bin, oldest first.
fn sizes(net: &Net, bin: PlaceRef<Ball>) -> Vec<u32> {
    net.tokens(bin.id())
        .filter_map(|(_, value)| value.downcast_ref::<Ball>())
        .map(|ball| ball.size)
        .collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut factory = build()?;
    let summary = drivers::run(
        &mut factory.net,
        Mode::Sequential,
        Limits::default(),
        OnFailure::Stop,
    )?;

    println!("{} balls made", summary.fired_count);
    println!("red:   {:?}", sizes(&factory.net, factory.red));
    println!("blue:  {:?}", sizes(&factory.net, factory.blue));
    println!("other: {:?}", sizes(&factory.net, factory.other));
    println!(
        "rubber left over: {}",
        factory.net.count(factory.rubber.id())
    );
    println!("{}", petritype_dot::dot(&factory.net));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Paint, Rubber, build, make_a_ball, sizes};
    use petritype::drivers::{self, Limits, Mode, OnFailure};

    /// Builds the factory and runs it until the paint runs out.
    fn run() -> super::Factory {
        let mut factory = build().expect("the net builds");
        let summary = drivers::run(
            &mut factory.net,
            Mode::Sequential,
            Limits::default(),
            OnFailure::Stop,
        )
        .expect("nothing fails");
        assert!(summary.quiesced);
        // Nine tins of paint, so nine balls: the tenth lump of rubber waits.
        assert_eq!(summary.fired_count, 9);
        factory
    }

    #[test]
    fn each_colour_lands_in_its_own_bin() {
        let factory = run();

        assert_eq!(sizes(&factory.net, factory.red), [10, 50, 90]);
        assert_eq!(sizes(&factory.net, factory.blue), [20, 60]);
        assert_eq!(sizes(&factory.net, factory.other), [30, 40, 70, 80]);
    }

    #[test]
    fn the_unpaired_rubber_is_left_where_it_was() {
        let factory = run();

        assert_eq!(factory.net.count(factory.rubber.id()), 1);
    }

    #[test]
    fn the_domain_function_knows_nothing_about_bins() {
        let ball = make_a_ball(
            Rubber { quantity: 7 },
            Paint {
                color: String::from("green"),
            },
        );

        assert_eq!(ball.size, 70);
        assert_eq!(ball.color, "green");
    }

    #[test]
    fn all_three_bins_are_drawn_even_though_only_one_is_used_per_firing() {
        let factory = build().expect("the net builds");
        let text = petritype_dot::dot(&factory.net);

        assert_eq!(text.matches("t0 -> ").count(), 3);
        for bin in ["Red Balls", "Blue Balls", "Other Balls"] {
            assert!(text.contains(&format!(r#"label="{bin}""#)));
        }
    }
}
