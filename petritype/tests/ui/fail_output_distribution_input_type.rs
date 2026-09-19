//! An output distribution function whose parameter is not what the body
//! returns.
//!
//! The two halves are composed before `transition()` sees them, so the
//! mismatch is between `make_a_ball`'s return type and the closure's
//! parameter type, and it is reported where the body is declared.
//!
//! Every type in the declaration is this crate's own, so that the expected
//! message does not depend on how rustc chooses to spell a standard-library
//! path — which it does differently once the `serde` feature brings more
//! paths into scope.

use petritype::{Distribution, NetBuilder, OutputDistribution, TakeEdge};

#[derive(Debug, Clone)]
struct Paint(u8);

#[derive(Debug, Clone)]
struct Ball(u8);

fn make_a_ball(paint: Paint) -> Ball {
    Ball(paint.0)
}

fn main() {
    let mut b = NetBuilder::new();
    let paint = b.place::<Paint>("Paint");
    let red = b.place::<Ball>("Red");
    let blue = b.place::<Ball>("Blue");

    // `make_a_ball` returns a `Ball`, so the distribution function is handed
    // a `Ball`, not a `Paint`.
    b.transition(
        "Make a Ball",
        (TakeEdge(paint),),
        (red, blue),
        make_a_ball.output_distribution(move |paint: Paint| {
            Distribution::new().to(red, Ball(paint.0))
        }),
    );
}
