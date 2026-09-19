//! Integration tests for the fourth slice: outputs routed by type.
//!
//! The question this slice answers is "where does this value go?" without the
//! author writing an order down. A body returns one value; each payload
//! inside it lands in the one output place that holds that payload's type;
//! the wiring is resolved when the transition is declared and reported by
//! `build`.

use petritype::{
    BuildError, Either, Event, Net, NetBuilder, NetError, PlaceId, Routed, TakeEdge, TokenId,
};

/// The DESIGN §4.5 web-scrape shape: one page in, one of two answers out.
#[derive(Debug, Clone, PartialEq)]
struct ScrapedData(&'static str);

#[derive(Debug, Clone, PartialEq)]
struct SuccessfulScrape(&'static str);

#[derive(Debug, Clone, PartialEq)]
struct UncertainScrapeResult(&'static str);

#[derive(Routed)]
enum Outcome {
    Ok(SuccessfulScrape),
    Unsure(UncertainScrapeResult),
    /// Nothing came of this page at all.
    Nothing,
}

/// A leaf: the whole struct is the payload, so it goes to the place of its
/// own type. This is how a bare value is routed, since a std type such as
/// `u32` has no place of its own to claim.
#[derive(Debug, Clone, PartialEq, Routed)]
struct Receipt(u32);

#[derive(Debug, Clone, PartialEq, Routed)]
struct Complaint(u32);

/// One job, and a batch of them: a payload is a leaf, so `Batch(Vec<Job>)` is
/// one token for a `Vec<Job>` place rather than many tokens for a `Job` one.
#[derive(Debug, Clone, PartialEq)]
struct Job(u32);

#[derive(Routed)]
enum Work {
    One(Job),
    Batch(Vec<Job>),
}

/// A payload type that arrives as a type parameter: the derive bounds it the
/// way the trait does, so the impl holds for every payload a place can hold.
#[derive(Routed)]
enum Tagged<T> {
    Value(T),
    Nothing,
}

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

#[test]
fn each_variant_lands_in_the_place_of_its_payload_type() {
    let mut b = NetBuilder::new();
    let pages = b.place::<ScrapedData>("Pages");
    let successes = b.place::<SuccessfulScrape>("Successes");
    let uncertain = b.place::<UncertainScrapeResult>("Uncertain");
    b.seed(pages, [ScrapedData("price: 3"), ScrapedData("price: ?")]);
    let interpret = b
        .transition(
            "Interpret",
            (TakeEdge(pages),),
            (successes, uncertain),
            |data: ScrapedData| {
                if data.0.contains('?') {
                    Outcome::Unsure(UncertainScrapeResult(data.0))
                } else {
                    Outcome::Ok(SuccessfulScrape(data.0))
                }
            },
        )
        .id();
    let mut net = b.build().expect("one output place per payload type");

    assert_eq!(net.step().expect("this body cannot fail"), Some(interpret));
    assert_eq!(
        values::<SuccessfulScrape>(&net, successes.id()),
        [SuccessfulScrape("price: 3")]
    );
    assert_eq!(net.count(uncertain.id()), 0);

    assert_eq!(net.step().expect("this body cannot fail"), Some(interpret));
    assert_eq!(
        values::<UncertainScrapeResult>(&net, uncertain.id()),
        [UncertainScrapeResult("price: ?")],
        "the second page went the other way, by its type alone"
    );
    assert_eq!(net.count(successes.id()), 1);
}

#[test]
fn the_order_of_the_output_places_does_not_matter() {
    /// The same net twice, with the outputs declared in both orders.
    fn scrape(swapped: bool) -> (Net, PlaceId, PlaceId) {
        let mut b = NetBuilder::new();
        let pages = b.place::<ScrapedData>("Pages");
        let successes = b.place::<SuccessfulScrape>("Successes");
        let uncertain = b.place::<UncertainScrapeResult>("Uncertain");
        b.seed(pages, [ScrapedData("price: 3")]);
        let body = |data: ScrapedData| Outcome::Ok(SuccessfulScrape(data.0));
        if swapped {
            b.transition(
                "Interpret",
                (TakeEdge(pages),),
                (uncertain, successes),
                body,
            );
        } else {
            b.transition(
                "Interpret",
                (TakeEdge(pages),),
                (successes, uncertain),
                body,
            );
        }
        let mut net = b.build().expect("either order resolves");
        net.step().expect("this body cannot fail");
        (net, successes.id(), uncertain.id())
    }

    let (declared, successes, uncertain) = scrape(false);
    let (swapped, swapped_successes, swapped_uncertain) = scrape(true);
    assert_eq!(
        values::<SuccessfulScrape>(&declared, successes),
        values::<SuccessfulScrape>(&swapped, swapped_successes),
    );
    assert_eq!(declared.count(uncertain), 0);
    assert_eq!(swapped.count(swapped_uncertain), 0);
}

#[test]
fn a_unit_variant_produces_nothing() {
    let mut b = NetBuilder::new();
    let pages = b.place::<ScrapedData>("Pages");
    let successes = b.place::<SuccessfulScrape>("Successes");
    let uncertain = b.place::<UncertainScrapeResult>("Uncertain");
    b.seed(pages, [ScrapedData("")]);
    let interpret = b
        .transition(
            "Interpret",
            (TakeEdge(pages),),
            (successes, uncertain),
            |_: ScrapedData| Outcome::Nothing,
        )
        .id();
    let mut net = b
        .build()
        .expect("a unit variant reaches no place, and needs none");

    assert_eq!(net.step().expect("this body cannot fail"), Some(interpret));
    assert_eq!(net.count(successes.id()), 0);
    assert_eq!(net.count(uncertain.id()), 0);
    assert_eq!(
        net.fired_count(),
        1,
        "a firing that produced nothing still fired"
    );
    assert!(matches!(
        fired(&mut net),
        Event::Fired { ref produced, .. } if produced.is_empty()
    ));
}

#[test]
fn a_derived_struct_is_routed_whole() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let receipts = b.place::<Receipt>("Receipts");
    b.seed(inbox, [7]);
    b.transition("Receipt", (TakeEdge(inbox),), (receipts,), |n: u32| {
        Receipt(n)
    });
    let mut net = b.build().expect("the struct's own type is its destination");

    net.step().expect("this body cannot fail");
    assert_eq!(values::<Receipt>(&net, receipts.id()), [Receipt(7)]);
}

#[test]
fn either_routes_one_of_two_without_an_enum_of_its_own() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let receipts = b.place::<Receipt>("Receipts");
    let complaints = b.place::<Complaint>("Complaints");
    b.seed(inbox, [2, 3]);
    b.transition(
        "Check",
        (TakeEdge(inbox),),
        (receipts, complaints),
        |n: u32| {
            if n.is_multiple_of(2) {
                Either::Left(Receipt(n))
            } else {
                Either::Right(Complaint(n))
            }
        },
    );
    let mut net = b.build().expect("both sides have a place");

    net.step().expect("this body cannot fail");
    net.step().expect("this body cannot fail");
    assert_eq!(values::<Receipt>(&net, receipts.id()), [Receipt(2)]);
    assert_eq!(values::<Complaint>(&net, complaints.id()), [Complaint(3)]);
}

#[test]
fn an_option_routes_zero_or_one_token() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let receipts = b.place::<Receipt>("Receipts");
    b.seed(inbox, [0, 5]);
    b.transition("Maybe", (TakeEdge(inbox),), (receipts,), |n: u32| {
        (n > 0).then_some(Receipt(n))
    });
    let mut net = b
        .build()
        .expect("an empty Option still declares its payload");

    net.step().expect("this body cannot fail");
    assert_eq!(net.count(receipts.id()), 0, "None emitted nothing");
    net.step().expect("this body cannot fail");
    assert_eq!(values::<Receipt>(&net, receipts.id()), [Receipt(5)]);
}

#[test]
fn a_vec_of_routed_values_is_a_batch() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let receipts = b.place::<Receipt>("Receipts");
    let complaints = b.place::<Complaint>("Complaints");
    b.seed(inbox, [3]);
    b.transition(
        "Spread",
        (TakeEdge(inbox),),
        (receipts, complaints),
        |n: u32| {
            (0..n)
                .map(|i| {
                    if i == 0 {
                        Either::Right(Complaint(i))
                    } else {
                        Either::Left(Receipt(i))
                    }
                })
                .collect::<Vec<_>>()
        },
    );
    let mut net = b.build().expect("every element routes by its own type");

    net.step().expect("this body cannot fail");
    assert_eq!(
        values::<Receipt>(&net, receipts.id()),
        [Receipt(1), Receipt(2)],
        "in the order the vector held them"
    );
    assert_eq!(values::<Complaint>(&net, complaints.id()), [Complaint(0)]);
}

#[test]
fn a_payload_is_a_leaf_even_when_it_is_a_vec() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let one = b.place::<Job>("One");
    let batches = b.place::<Vec<Job>>("Batches");
    b.seed(inbox, [2, 1]);
    b.transition("Split", (TakeEdge(inbox),), (one, batches), |n: u32| {
        if n == 1 {
            Work::One(Job(n))
        } else {
            Work::Batch((0..n).map(Job).collect())
        }
    });
    let mut net = b.build().expect("a Vec payload wants a Vec place");

    net.step().expect("this body cannot fail");
    assert_eq!(net.count(one.id()), 0);
    assert_eq!(
        values::<Vec<Job>>(&net, batches.id()),
        [vec![Job(0), Job(1)]],
        "one token holding the whole batch, not two tokens"
    );

    // The other variant of the same enum carries a bare `Job`, which is a
    // different type and so a different place.
    net.step().expect("this body cannot fail");
    assert_eq!(values::<Job>(&net, one.id()), [Job(1)]);
    assert_eq!(net.count(batches.id()), 1);
}

#[test]
fn a_generic_routed_enum_routes_its_payload() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let receipts = b.place::<Receipt>("Receipts");
    b.seed(inbox, [1, 2]);
    b.transition("Tag", (TakeEdge(inbox),), (receipts,), |n: u32| {
        if n == 1 {
            Tagged::Value(Receipt(n))
        } else {
            Tagged::Nothing
        }
    });
    let mut net = b.build().expect("the payload type is the parameter");

    net.step().expect("this body cannot fail");
    net.step().expect("this body cannot fail");
    assert_eq!(values::<Receipt>(&net, receipts.id()), [Receipt(1)]);
}

#[test]
fn a_routed_body_may_return_a_result() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let receipts = b.place::<Receipt>("Receipts");
    b.seed(inbox, [1]);
    b.transition(
        "Try",
        (TakeEdge(inbox),),
        (receipts,),
        |n: u32| -> Result<Receipt, Refused> { Ok(Receipt(n)) },
    );
    let mut net = b
        .build()
        .expect("failure wraps the routed form, it does not replace it");

    net.step().expect("this body returned Ok");
    assert_eq!(values::<Receipt>(&net, receipts.id()), [Receipt(1)]);
}

#[test]
fn an_err_from_a_routed_body_is_a_failure_and_loses_its_inputs() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let receipts = b.place::<Receipt>("Receipts");
    b.seed(inbox, [1]);
    let fussy = b
        .transition(
            "Fussy",
            (TakeEdge(inbox),),
            (receipts,),
            |_: u32| -> Result<Receipt, Refused> { Err(Refused) },
        )
        .id();
    let mut net = b.build().expect("an Err says nothing about routing");

    let consumed = pairs(&net, inbox.id());
    match net.fire(fussy) {
        Err(NetError::FiringFailed(failure)) => {
            assert_eq!(failure.transition, fussy);
            assert_eq!(failure.error.to_string(), "refused");
            assert!(!failure.restored);
        }
        other => panic!("expected a firing failure, got {other:?}"),
    }
    assert_eq!(net.count(receipts.id()), 0, "nothing was distributed");
    assert_eq!(net.fired_count(), 0);
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Failed {
            transition: fussy,
            lost: consumed,
            restored: false,
            error: String::from("refused"),
        }]
    );
}

#[test]
fn fired_names_the_place_each_routed_token_landed_in() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let receipts = b.place::<Receipt>("Receipts");
    let complaints = b.place::<Complaint>("Complaints");
    b.seed(inbox, [1]);
    let spread = b
        .transition(
            "Spread",
            (TakeEdge(inbox),),
            (complaints, receipts),
            |n: u32| {
                vec![
                    Either::Left(Receipt(n)),
                    Either::Right(Complaint(n)),
                    Either::Left(Receipt(n + 1)),
                ]
            },
        )
        .id();
    let mut net = b.build().expect("both payload types have a place");

    let consumed = pairs(&net, inbox.id());
    assert_eq!(net.step().expect("this body cannot fail"), Some(spread));

    // Produced in emission order, each pair naming the place its type chose.
    let receipt_tokens = pairs(&net, receipts.id());
    let complaint_tokens = pairs(&net, complaints.id());
    assert_eq!(
        fired(&mut net),
        Event::Fired {
            transition: spread,
            consumed,
            produced: vec![receipt_tokens[0], complaint_tokens[0], receipt_tokens[1],],
        }
    );
}

#[test]
fn a_payload_type_with_no_output_place_is_named_by_build() {
    let mut b = NetBuilder::new();
    let pages = b.place::<ScrapedData>("Pages");
    let successes = b.place::<SuccessfulScrape>("Successes");
    // No place for the uncertain half: declaring the transition still
    // succeeds, because `transition` never fails.
    b.transition(
        "Interpret",
        (TakeEdge(pages),),
        (successes,),
        |data: ScrapedData| Outcome::Ok(SuccessfulScrape(data.0)),
    );

    let error = b.build().unwrap_err();
    assert!(matches!(
        error,
        BuildError::UnroutableOutput { ref transition, type_name }
            if transition == "Interpret" && type_name.ends_with("UncertainScrapeResult")
    ));
}

#[test]
fn two_output_places_of_one_type_are_named_by_build() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let here = b.place::<Receipt>("Here");
    let there = b.place::<Receipt>("There");
    b.transition("Receipt", (TakeEdge(inbox),), (here, there), |n: u32| {
        Receipt(n)
    });

    let error = b.build().unwrap_err();
    assert!(matches!(
        error,
        BuildError::AmbiguousOutput { ref transition, type_name }
            if transition == "Receipt" && type_name.ends_with("Receipt")
    ));
}

#[test]
fn an_output_place_nothing_can_reach_is_named_by_build() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let receipts = b.place::<Receipt>("Receipts");
    let complaints = b.place::<Complaint>("Complaints");
    // The body can only ever produce a `Receipt`, so the edge drawn to
    // "Complaints" is a lie about this net.
    b.transition(
        "Receipt",
        (TakeEdge(inbox),),
        (receipts, complaints),
        |n: u32| Receipt(n),
    );

    let error = b.build().unwrap_err();
    assert!(matches!(
        error,
        BuildError::UnreachableOutput { ref transition, ref place }
            if transition == "Receipt" && place == "Complaints"
    ));
}

#[test]
fn a_positional_body_may_still_have_two_outputs_of_one_type() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let heavy = b.place::<u32>("Heavy");
    let light = b.place::<u32>("Light");
    b.seed(inbox, [11]);
    // Order is the only thing that tells these two apart, and that is what
    // the positional form is for: no routing question is asked of it.
    b.transition("Sort", (TakeEdge(inbox),), (heavy, light), |n: u32| {
        if n > 10 {
            (Some(n), None)
        } else {
            (None, Some(n))
        }
    });
    let mut net = b.build().expect("a positional body routes nothing by type");

    net.step().expect("this body cannot fail");
    assert_eq!(values::<u32>(&net, heavy.id()), [11]);
    assert_eq!(net.count(light.id()), 0);
}

#[test]
fn a_routing_problem_is_reported_by_build_not_by_transition() {
    let mut b = NetBuilder::new();
    let pages = b.place::<ScrapedData>("Pages");
    let successes = b.place::<SuccessfulScrape>("Successes");
    // The handle still works: the problem is stashed, not raised.
    b.transition(
        "Interpret",
        (TakeEdge(pages),),
        (successes,),
        |data: ScrapedData| Outcome::Ok(SuccessfulScrape(data.0)),
    )
    .guard(|_| true)
    .priority(|_| 1.0);
    // A second, well-wired transition is unaffected by the first one's
    // problem, and the error names the transition that has it.
    b.transition("Copy", (TakeEdge(successes),), (), |_: SuccessfulScrape| {});

    assert!(matches!(
        b.build().unwrap_err(),
        BuildError::UnroutableOutput { ref transition, .. } if transition == "Interpret"
    ));
}

#[test]
fn a_transition_with_no_outputs_and_a_routed_body_that_emits_nothing_builds() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    b.seed(inbox, [1]);
    // Every variant is a unit variant, so there is nothing to route and
    // nothing to declare.
    #[derive(Routed)]
    enum Silence {
        Quiet,
    }

    b.transition("Nothing", (TakeEdge(inbox),), (), |_: u32| Silence::Quiet);
    let mut net = b.build().expect("no payload types, no output places");

    net.step().expect("this body cannot fail");
    assert_eq!(net.fired_count(), 1);
}
