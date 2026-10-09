//! Integration tests for the seventeenth slice: what the protocol runner
//! needed from the core to put nested nets on the wire (DESIGN §6.6, §8.2,
//! §19.3.11; PROTOCOL.md §12).
//!
//! Three reads and one step: `Net::step_inside`, the inner half of
//! `step_deep` for a driver that has already asked the outer selector;
//! `Net::param_name`, the wire name behind a `ParamChanged`'s type name; and
//! `positional` on `TransitionInfo` and `TopologyTransition`, which says
//! whether an output edge's slot is a position in what the body returns.

use petritype::{
    DeepStep, Distribution, Event, Net, NetBuilder, OutputDistribution, Routed, TakeEdge,
};

// ----- step_inside ------------------------------------------------------------

/// `In -Move-> Out`, holding `items`; `Move` refuses 13.
fn bin(items: &[u32]) -> Net {
    #[derive(Debug)]
    struct Refused;
    impl std::fmt::Display for Refused {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("refused")
        }
    }
    impl std::error::Error for Refused {}
    let mut b = NetBuilder::new();
    let input = b.place::<u32>("In");
    let output = b.place::<u32>("Out");
    b.seed(input, items.iter().copied());
    b.transition(
        "Move",
        (TakeEdge(input),),
        (output,),
        |n: u32| -> Result<(u32,), Refused> { if n == 13 { Err(Refused) } else { Ok((n,)) } },
    );
    b.build().unwrap()
}

#[test]
fn step_inside_fires_an_inner_net_and_never_the_outer_one() {
    let mut b = NetBuilder::new();
    let ready = b.place::<u32>("Ready");
    let done = b.place::<u32>("Done");
    let shelf = b.place_nested::<Net>("Shelf");
    let lazy = b.place_nested::<Net>("Lazy");
    b.passive(lazy);
    b.seed(ready, [1]);
    b.seed(shelf, [bin(&[1])]);
    b.seed(lazy, [bin(&[2])]);
    b.transition("Out Here", (TakeEdge(ready),), (done,), |n: u32| (n,));
    let mut net = b.build().unwrap();
    assert!(!net.fireable().is_empty(), "the outer net has work");

    let step = net
        .step_inside()
        .unwrap()
        .expect("the shelf's bin had work");
    assert!(matches!(&step, DeepStep::Inside { place, .. } if *place == shelf.id()));
    assert_eq!(
        net.count(ready.id()),
        1,
        "the outer transition was not fired"
    );
    let events: Vec<Event> = net.drain_events().collect();
    assert!(
        matches!(&events[..], [Event::Nested { place, event, .. }]
            if *place == shelf.id() && matches!(**event, Event::Fired { .. })),
        "lifted by the time it returns: {events:?}"
    );
    assert_eq!(
        net.step_inside().unwrap(),
        None,
        "the passive bin is never stepped"
    );
    assert_eq!(
        net.fired_count(),
        0,
        "an inner firing is not the outer net's"
    );
}

#[test]
fn step_inside_reports_an_inner_failure_with_its_path() {
    let mut b = NetBuilder::new();
    let shelf = b.place_nested::<Net>("Shelf");
    b.seed(shelf, [bin(&[13])]);
    let mut net = b.build().unwrap();
    let failed = net.step_inside().unwrap_err();
    assert_eq!(failed.name, "Move");
    assert_eq!(failed.path.len(), 1);
    assert_eq!(failed.path[0].0, shelf.id());
    assert_eq!(failed.path[0].2, "");
    let events: Vec<Event> = net.drain_events().collect();
    assert!(
        matches!(&events[..], [Event::Nested { event, .. }] if matches!(**event, Event::Failed { .. }))
    );
}

// ----- param_name -------------------------------------------------------------

#[cfg(feature = "serde")]
mod with_serde {
    use petritype::serde_json;
    use petritype::{Event, NetBuilder};
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct Speed(u32);

    /// A parameter that never crosses a wire.
    #[derive(Debug, Clone)]
    struct Secret;

    #[test]
    fn param_name_is_the_wire_name_behind_a_param_changed() {
        let mut b = NetBuilder::new();
        b.param(Speed(1));
        b.serializable_param::<Speed>("speed");
        b.param(Secret);
        let mut net = b.build().unwrap();

        net.set_param(Speed(2)).unwrap();
        net.set_param(Secret).unwrap();
        let changed: Vec<&'static str> = net
            .drain_events()
            .map(|event| match event {
                Event::ParamChanged { type_name } => type_name,
                other => panic!("only parameters changed: {other:?}"),
            })
            .collect();
        let names: Vec<Option<&str>> = changed
            .iter()
            .map(|type_name| net.param_name(type_name))
            .collect();
        assert_eq!(
            names,
            [Some("speed"), None],
            "a parameter without a wire name has none to give"
        );
        assert_eq!(net.param_name("no::Such"), None);
    }

    #[test]
    fn a_topology_writes_positional_only_when_it_is_false() {
        use petritype::{Distribution, Net, TakeEdge};
        let mut b = NetBuilder::new();
        let inbox = b.place::<u32>("Inbox");
        let left = b.place::<u32>("Left");
        let right = b.place::<u32>("Right");
        b.transition("Split", (TakeEdge(inbox),), (left, right), |n: u32| (n, n));
        b.transition("Route", (TakeEdge(left),), (inbox, right), move |n: u32| {
            Distribution::new().to(right, n)
        });
        let net: Net = b.build().unwrap();
        let topology = serde_json::to_value(net.topology()).unwrap();
        assert!(topology["transitions"][0].get("positional").is_none());
        assert_eq!(topology["transitions"][1]["positional"], false);
    }
}

// ----- positional -------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Routed)]
struct Small(u32);

#[derive(Debug, Clone, PartialEq, Routed)]
struct Large(u32);

#[derive(Routed)]
enum Size {
    Small(Small),
    Large(Large),
}

#[derive(Debug)]
struct Never;

impl std::fmt::Display for Never {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("never")
    }
}

impl std::error::Error for Never {}

#[test]
fn every_transition_says_whether_its_body_returns_by_position() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let small = b.place::<Small>("Small");
    let large = b.place::<Large>("Large");
    let a = b.place::<u32>("A");
    let z = b.place::<u32>("Z");
    let tuple = b
        .transition("Tuple", (TakeEdge(inbox),), (a, z), |n: u32| (n, n))
        .id();
    let none = b.transition("Sink", (TakeEdge(z),), (), |_: u32| {}).id();
    let routed = b
        .transition("Sort", (TakeEdge(a),), (small, large), |n: u32| {
            if n < 10 {
                Size::Small(Small(n))
            } else {
                Size::Large(Large(n))
            }
        })
        .id();
    let named = b
        .transition("Pick", (TakeEdge(inbox),), (a, z), move |n: u32| {
            Distribution::new().to(if n.is_multiple_of(2) { a } else { z }, n)
        })
        .id();
    let fallible = b
        .transition(
            "Maybe",
            (TakeEdge(z),),
            (a, inbox),
            move |n: u32| -> Result<Distribution, Never> { Ok(Distribution::new().to(a, n)) },
        )
        .id();
    let wrapped = b
        .transition(
            "Wrapped",
            (TakeEdge(a),),
            (z, inbox),
            (|n: u32| n).output_distribution(move |n: u32| Distribution::new().to(z, n)),
        )
        .id();
    let net = b.build().unwrap();

    let positional = |t| {
        net.transitions()
            .find(|(id, _)| *id == t)
            .map(|(_, info)| info.positional)
            .unwrap()
    };
    assert!(positional(tuple));
    assert!(positional(none), "`()` is the empty tuple");
    assert!(!positional(routed), "routed by type");
    assert!(!positional(named), "a distribution names its places");
    assert!(
        !positional(fallible),
        "a `Result` has the form of what it wraps"
    );
    assert!(
        !positional(wrapped),
        "a wrapped body has the form of what its distribution function returns"
    );

    let topology = net.topology();
    let flags: Vec<bool> = topology.transitions.iter().map(|t| t.positional).collect();
    assert_eq!(flags, [true, true, false, false, false, false]);
}

#[test]
fn an_in_flight_transition_and_a_snapshot_still_say_so() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [1]);
    let pick = b
        .transition("Pick", (TakeEdge(inbox),), (out, inbox), move |n: u32| {
            Distribution::new().to(out, n)
        })
        .id();
    let mut net = b.build().unwrap();
    let firing = net.launch(pick).unwrap();
    let in_flight = net.transitions().find(|(id, _)| *id == pick).unwrap().1;
    assert!(in_flight.in_flight);
    assert!(
        !in_flight.positional,
        "read off the return type when declared, not off the body in its slot"
    );
    let snapshot = net.snapshot().unwrap();
    assert!(!snapshot.transitions().next().unwrap().1.positional);
    net.distribute(firing.run()).unwrap();
}
