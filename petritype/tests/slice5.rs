//! Integration tests for the fifth slice: the `Distribution` output form.
//!
//! The question this slice answers is "which of these places, *this* time?".
//! Neither order nor type can say, because the places that compete are the
//! same type and the answer is in the data, so the author names the place and
//! the engine checks the name — before it writes anything.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use petritype::{
    Distribution, DistributionError, Event, Net, NetBuilder, NetError, OutputDistribution, PlaceId,
    Routed, TakeAllEdge, TakeEdge, TokenId,
};

/// The DESIGN §14 coloured balls: three output places of one type, and only
/// the ball's colour to choose between them.
#[derive(Debug, Clone, PartialEq)]
struct Ball {
    color: String,
}

impl Ball {
    fn new(color: &str) -> Ball {
        Ball {
            color: String::from(color),
        }
    }
}

/// The domain function, which knows nothing about places. Every test that
/// composes a distribution function after a body uses this one, unchanged.
fn make_a_ball(color: String) -> Ball {
    Ball { color }
}

/// A payload type for the routed form, to show that a distribution function
/// may return any output form and not only a `Distribution`.
#[derive(Debug, Clone, PartialEq, Routed)]
struct Receipt(u32);

/// A body's failure, erased into [`petritype::FiringFailed`] by the engine.
#[derive(Debug)]
struct Refused;

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("refused")
    }
}

impl std::error::Error for Refused {}

/// Reads a place back as plain values, oldest first.
fn values<T: Clone + Send + Sync + 'static>(net: &Net, place: PlaceId) -> Vec<T> {
    net.tokens(place)
        .map(|(_, value)| {
            value
                .downcast_ref::<T>()
                .expect("the place holds this type")
                .clone()
        })
        .collect()
}

/// Every token of a place as the `(place, token)` pairs an event carries.
fn pairs(net: &Net, place: PlaceId) -> Vec<(PlaceId, TokenId)> {
    net.tokens(place).map(|(id, _)| (place, id)).collect()
}

/// The `Fired` event of the firing just made.
fn fired(net: &mut Net) -> Event {
    net.drain_events()
        .find(|event| matches!(event, Event::Fired { .. }))
        .expect("one firing, one Fired event")
}

/// The colours a place holds, oldest first.
fn colors(net: &Net, place: PlaceId) -> Vec<String> {
    values::<Ball>(net, place)
        .into_iter()
        .map(|ball| ball.color)
        .collect()
}

#[test]
fn a_body_may_name_the_place_each_token_goes_to() {
    let mut b = NetBuilder::new();
    let paint = b.place::<String>("Paint");
    let red = b.place::<Ball>("Red");
    let blue = b.place::<Ball>("Blue");
    let other = b.place::<Ball>("Other");
    b.seed(
        paint,
        [
            String::from("blue"),
            String::from("green"),
            String::from("red"),
        ],
    );
    let make = b
        .transition(
            "Make a Ball",
            (TakeEdge(paint),),
            (red, blue, other),
            move |color: String| {
                let ball = make_a_ball(color);
                let target = match ball.color.as_str() {
                    "red" => red,
                    "blue" => blue,
                    _ => other,
                };
                Distribution::new().to(target, ball)
            },
        )
        .id();
    let mut net = b.build().expect("a distribution needs no routing table");

    for _ in 0..3 {
        assert_eq!(net.step().expect("this body cannot fail"), Some(make));
    }
    assert_eq!(colors(&net, red.id()), ["red"]);
    assert_eq!(colors(&net, blue.id()), ["blue"]);
    assert_eq!(colors(&net, other.id()), ["green"]);
    assert_eq!(net.fired_count(), 3);
}

#[test]
fn an_unchanged_domain_function_is_routed_by_the_function_after_it() {
    let mut b = NetBuilder::new();
    let paint = b.place::<String>("Paint");
    let red = b.place::<Ball>("Red");
    let blue = b.place::<Ball>("Blue");
    let other = b.place::<Ball>("Other");
    b.seed(paint, [String::from("blue")]);
    // `make_a_ball` is the same function as in the test above, taken by name:
    // the routing decision lives here, beside the net, where the handles are.
    b.transition(
        "Make a Ball",
        (TakeEdge(paint),),
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
    let mut net = b.build().expect("a distribution needs no routing table");

    net.step().expect("this body cannot fail");
    assert_eq!(colors(&net, blue.id()), ["blue"]);
    assert_eq!(net.count(red.id()), 0);
    assert_eq!(net.count(other.id()), 0);
}

#[test]
fn a_distribution_function_may_return_a_positional_tuple() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let small = b.place::<u32>("Small");
    let large = b.place::<u32>("Large");
    b.seed(inbox, [2, 40]);
    b.transition(
        "Sort",
        (TakeEdge(inbox),),
        (small, large),
        (|n: u32| n * 2).output_distribution(|doubled: u32| {
            // Two slots, one per output place, as any positional body would.
            if doubled < 10 {
                (Some(doubled), None)
            } else {
                (None, Some(doubled))
            }
        }),
    );
    let mut net = b.build().expect("a positional form needs no routing table");

    net.step().expect("this body cannot fail");
    net.step().expect("this body cannot fail");
    assert_eq!(values::<u32>(&net, small.id()), [4]);
    assert_eq!(values::<u32>(&net, large.id()), [80]);
}

#[test]
fn a_distribution_function_may_return_a_routed_value() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let receipts = b.place::<Receipt>("Receipts");
    b.seed(inbox, [7]);
    b.transition(
        "Receipt",
        (TakeEdge(inbox),),
        (receipts,),
        (|n: u32| n + 1).output_distribution(|n: u32| Receipt(n)),
    );
    let mut net = b.build().expect("one output place for the payload type");

    net.step().expect("this body cannot fail");
    assert_eq!(values::<Receipt>(&net, receipts.id()), [Receipt(8)]);
}

#[test]
fn a_fallible_body_distributes_its_ok_and_fails_on_its_err() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);

    let mut b = NetBuilder::new();
    let paint = b.place::<String>("Paint");
    let red = b.place::<Ball>("Red");
    let blue = b.place::<Ball>("Blue");
    b.seed(paint, [String::from("red"), String::from("chartreuse")]);
    let make = b
        .transition(
            "Make a Ball",
            (TakeEdge(paint),),
            (red, blue),
            (|color: String| -> Result<Ball, Refused> {
                match color.as_str() {
                    "red" | "blue" => Ok(make_a_ball(color)),
                    _ => Err(Refused),
                }
            })
            .output_distribution(move |ball: Ball| {
                counted.fetch_add(1, Ordering::Relaxed);
                let target = if ball.color == "red" { red } else { blue };
                Distribution::new().to(target, ball)
            }),
        )
        .id();
    let mut net = b.build().expect("a distribution needs no routing table");

    assert_eq!(net.step().expect("the first colour is known"), Some(make));
    assert_eq!(colors(&net, red.id()), ["red"]);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(matches!(fired(&mut net), Event::Fired { .. }));

    let consumed = pairs(&net, paint.id());
    let failure = net.step().expect_err("the second colour is refused");
    assert_eq!(failure.transition, make);
    assert_eq!(failure.error.to_string(), "refused");
    assert_eq!(
        calls.load(Ordering::Relaxed),
        1,
        "an Err never reaches the output distribution function"
    );
    assert_eq!(net.fired_count(), 1, "a failed firing is not a firing");
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Failed {
            transition: make,
            lost: consumed,
            restored: false,
            error: String::from("refused"),
        }]
    );

    // The net is consistent: the body is back in its slot, so a further step
    // works — there is simply nothing left to fire.
    assert_eq!(net.step().expect("nothing is fireable"), None);
}

#[test]
fn push_fans_one_batch_out_over_several_places() {
    let mut b = NetBuilder::new();
    let paint = b.place::<String>("Paint");
    let red = b.place::<Ball>("Red");
    let blue = b.place::<Ball>("Blue");
    let other = b.place::<Ball>("Other");
    b.seed(
        paint,
        ["red", "blue", "red", "teal"].map(String::from).to_vec(),
    );
    b.transition(
        "Make balls",
        (TakeAllEdge(paint),),
        (red, blue, other),
        move |colors: Vec<String>| {
            // How many pairs there are is known only now, which is what
            // `push` is for.
            let mut distribution = Distribution::new();
            for color in colors {
                let ball = make_a_ball(color);
                let target = match ball.color.as_str() {
                    "red" => red,
                    "blue" => blue,
                    _ => other,
                };
                distribution.push(target, ball);
            }
            distribution
        },
    );
    let mut net = b.build().expect("a distribution needs no routing table");

    net.step().expect("this body cannot fail");
    assert_eq!(colors(&net, red.id()), ["red", "red"]);
    assert_eq!(colors(&net, blue.id()), ["blue"]);
    assert_eq!(colors(&net, other.id()), ["teal"]);
}

#[test]
fn several_tokens_for_one_place_land_in_order_with_fresh_ids() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [3]);
    b.transition("Spread", (TakeEdge(inbox),), (out,), move |n: u32| {
        let mut distribution = Distribution::new();
        for step in 0..n {
            distribution.push(out, step);
        }
        distribution
    });
    let mut net = b.build().expect("a distribution needs no routing table");

    let consumed: Vec<TokenId> = net.tokens(inbox.id()).map(|(id, _)| id).collect();
    net.step().expect("this body cannot fail");

    assert_eq!(values::<u32>(&net, out.id()), [0, 1, 2]);
    let landed: Vec<TokenId> = net.tokens(out.id()).map(|(id, _)| id).collect();
    let mut sorted = landed.clone();
    sorted.sort();
    assert_eq!(landed, sorted, "ids are minted in the order tokens land");
    assert!(
        landed.iter().all(|id| !consumed.contains(id)),
        "a produced token never reuses a consumed id"
    );
}

#[test]
fn an_empty_distribution_fires_and_produces_nothing() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [1]);
    let swallow = b
        .transition("Swallow", (TakeEdge(inbox),), (out,), |_: u32| {
            Distribution::new()
        })
        .id();
    let mut net = b.build().expect("a distribution needs no routing table");

    let consumed = pairs(&net, inbox.id());
    assert_eq!(
        net.step().expect("producing nothing is not a failure"),
        Some(swallow)
    );
    assert_eq!(net.count(out.id()), 0);
    assert_eq!(net.fired_count(), 1);
    assert_eq!(
        fired(&mut net),
        Event::Fired {
            transition: swallow,
            consumed,
            produced: Vec::new(),
        }
    );
}

#[test]
fn fired_names_the_place_each_distributed_token_landed_in() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let red = b.place::<Ball>("Red");
    let blue = b.place::<Ball>("Blue");
    b.seed(inbox, [1]);
    let make = b
        .transition("Make", (TakeEdge(inbox),), (red, blue), move |_: u32| {
            Distribution::new()
                .to(blue, Ball::new("blue"))
                .to(red, Ball::new("red"))
                .to(blue, Ball::new("navy"))
        })
        .id();
    let mut net = b.build().expect("a distribution needs no routing table");

    let consumed = pairs(&net, inbox.id());
    net.step().expect("this body cannot fail");

    // The order is the order the pairs were added in, across places.
    let landed: Vec<(PlaceId, TokenId)> = {
        let mut blues = pairs(&net, blue.id()).into_iter();
        let reds = pairs(&net, red.id());
        vec![
            blues.next().expect("the first blue"),
            reds[0],
            blues.next().expect("the second blue"),
        ]
    };
    assert_eq!(
        fired(&mut net),
        Event::Fired {
            transition: make,
            consumed,
            produced: landed,
        }
    );
}

#[test]
fn a_place_that_is_not_an_output_fails_the_whole_firing() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let red = b.place::<Ball>("Red");
    // Declared in the net, but not an output of the transition below.
    let elsewhere = b.place::<Ball>("Elsewhere");
    b.seed(inbox, [1, 2]);
    let make = b
        .transition("Make", (TakeEdge(inbox),), (red,), move |_: u32| {
            // The good pair comes first on purpose: if resolution did not
            // happen before the writes, this ball would already be in Red.
            Distribution::new()
                .to(red, Ball::new("red"))
                .to(elsewhere, Ball::new("stray"))
        })
        .id();
    let mut net = b.build().expect("a distribution needs no routing table");

    let consumed = pairs(&net, inbox.id())[..1].to_vec();
    let failure = net.step().expect_err("one pair names a place it may not");
    assert_eq!(failure.transition, make);
    assert!(!failure.restored);
    assert!(matches!(
        failure.error.downcast_ref::<DistributionError>(),
        Some(DistributionError::NotAnOutput { transition, place })
            if *transition == make && *place == elsewhere.id()
    ));

    assert_eq!(net.count(red.id()), 0, "the valid pair was not written");
    assert_eq!(net.count(elsewhere.id()), 0);
    assert_eq!(net.fired_count(), 0, "a failed firing is not a firing");
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Failed {
            transition: make,
            lost: consumed,
            restored: false,
            error: format!(
                "transition {make:?} distributed a token to place {:?}, \
                 which is not one of its outputs",
                elsewhere.id()
            ),
        }]
    );

    // The net is consistent afterwards: the second token is still there and
    // the transition fires again (and fails again, which is the body's doing,
    // not the engine's).
    assert_eq!(net.count(inbox.id()), 1);
    assert!(net.is_fireable(make));
    assert!(net.step().is_err());
    assert_eq!(net.count(inbox.id()), 0);
}

#[test]
fn a_handle_from_another_net_fails_the_firing_too() {
    let mut other = NetBuilder::new();
    let foreign = other.place::<Ball>("Red");
    let _ = other.build().expect("a net of one place");

    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    // Place #0 of this net, exactly as `foreign` is place #0 of the other:
    // only the stamp tells them apart.
    let red = b.place::<Ball>("Red");
    b.seed(inbox, [1]);
    let make = b
        .transition("Make", (TakeEdge(inbox),), (red,), move |_: u32| {
            Distribution::new().to(foreign, Ball::new("red"))
        })
        .id();
    let mut net = b.build().expect("a distribution needs no routing table");

    match net.fire(make) {
        Err(NetError::FiringFailed(failure)) => {
            assert!(matches!(
                failure.error.downcast_ref::<DistributionError>(),
                Some(DistributionError::NotAnOutput { place, .. }) if *place == foreign.id()
            ));
        }
        other => panic!("expected a firing failure, got {other:?}"),
    }
    assert_eq!(net.count(red.id()), 0, "nothing was written to this net");
    assert_eq!(net.fired_count(), 0);
    assert!(matches!(
        net.drain_events().next(),
        Some(Event::Failed { .. })
    ));
}

#[test]
fn a_distribution_may_name_the_same_place_twice_and_repeat_a_type() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let red = b.place::<Ball>("Red");
    let blue = b.place::<Ball>("Blue");
    b.seed(inbox, [1]);
    b.transition("Make", (TakeEdge(inbox),), (red, blue), move |_: u32| {
        Distribution::new()
            .to(red, Ball::new("crimson"))
            .to(red, Ball::new("scarlet"))
    });
    let mut net = b.build().expect("same-typed output places are the point");

    net.step().expect("this body cannot fail");
    assert_eq!(colors(&net, red.id()), ["crimson", "scarlet"]);
    assert_eq!(
        net.count(blue.id()),
        0,
        "an output place a Distribution never names is not a build error"
    );
}
