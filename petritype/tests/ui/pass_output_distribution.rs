//! The DESIGN §4.5 coloured-balls shape, with the two corrections the
//! implementation made to the sketch there: `output_distribution` wraps the
//! *body* rather than the `TransitionBuilder` (by the time `transition()`
//! returns, the body's return type has been erased, and the whole point is
//! that the distribution function's return type decides the output form), and
//! a `Distribution` is built from its one constructor,
//! `Distribution::new().to(place, token)`.

use petritype::{Distribution, NetBuilder, OutputDistribution, Routed, TakeEdge};

#[derive(Debug, Clone)]
struct Rubber(u32);

#[derive(Debug, Clone)]
struct Paint(&'static str);

#[derive(Debug, Clone)]
struct Ball {
    color: String,
}

#[derive(Debug, Clone, Routed)]
struct Offcut(u32);

/// The domain function, unchanged by the net around it: no `PlaceRef` in
/// sight, and nothing to rewrite when the net is rewired.
fn make_a_ball(rubber: Rubber, paint: Paint) -> Ball {
    let _ = rubber;
    Ball {
        color: String::from(paint.0),
    }
}

fn main() {
    let mut b = NetBuilder::new();
    let rubber = b.place::<Rubber>("Rubber");
    let paint = b.place::<Paint>("Paint");
    let red = b.place::<Ball>("Red");
    let blue = b.place::<Ball>("Blue");
    let other = b.place::<Ball>("Other");
    let offcuts = b.place::<Offcut>("Offcuts");

    b.transition(
        "Make a Ball",
        (TakeEdge(rubber), TakeEdge(paint)),
        (red, blue, other),
        make_a_ball.output_distribution(move |ball: Ball| {
            let target = match ball.color.as_str() {
                "red" => red,
                "blue" => blue,
                _ => other,
            };
            Distribution::new().to(target, ball)
        }),
    );

    // A body may also build the `Distribution` itself, when it is written
    // beside the net anyway.
    b.transition(
        "Make another Ball",
        (TakeEdge(rubber), TakeEdge(paint)),
        (red, blue, other),
        move |_: Rubber, paint: Paint| {
            let ball = Ball {
                color: String::from(paint.0),
            };
            Distribution::new().to(if paint.0 == "red" { red } else { other }, ball)
        },
    );

    // The distribution function may return any output form, not only a
    // `Distribution`: a positional tuple ...
    b.transition(
        "Trim",
        (TakeEdge(rubber),),
        (offcuts,),
        (|rubber: Rubber| rubber.0).output_distribution(|size: u32| (Offcut(size),)),
    );

    // ... or a value routed by its type.
    b.transition(
        "Trim again",
        (TakeEdge(rubber),),
        (offcuts,),
        (|rubber: Rubber| rubber.0).output_distribution(|size: u32| Offcut(size)),
    );

    // A fallible body composes too: the function takes what is inside the
    // `Ok`, and an `Err` never reaches it.
    b.transition(
        "Trim carefully",
        (TakeEdge(rubber),),
        (offcuts,),
        (|rubber: Rubber| -> Result<u32, std::num::TryFromIntError> {
            u32::try_from(i64::from(rubber.0))
        })
        .output_distribution(move |size: u32| Distribution::new().to(offcuts, Offcut(size))),
    );

    let net = b.build().unwrap();
    assert_eq!(net.transitions().count(), 5);
}
