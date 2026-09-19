//! Integration tests for the third slice: what can fire, which one does, and
//! what one firing changes.
//!
//! The vocabulary this slice settles is in every test below: **enabled** is
//! the author's on/off switch, **fireable** is "could fire right now".

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use petritype::{
    BuildError, Event, MutateEdge, Net, NetBuilder, NetError, PlaceId, SnapshotEdge, TakeAllEdge,
    TakeEdge, TokenId,
};

#[derive(Debug, Clone, PartialEq)]
struct Parcel {
    id: u32,
    weight: f32,
}

#[derive(Debug, Clone, PartialEq)]
struct Config {
    max_weight: f32,
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

/// Reads a `u32` place back as plain values, oldest first.
fn values(net: &Net, place: PlaceId) -> Vec<u32> {
    net.tokens(place)
        .map(|(_, value)| *value.downcast_ref::<u32>().expect("place holds u32"))
        .collect()
}

fn ids(net: &Net, place: PlaceId) -> Vec<TokenId> {
    net.tokens(place).map(|(id, _)| id).collect()
}

/// Every token of a place as the `(place, token)` pairs an event carries.
fn pairs(net: &Net, place: PlaceId) -> Vec<(PlaceId, TokenId)> {
    net.tokens(place).map(|(id, _)| (place, id)).collect()
}

/// Fires until nothing is fireable, and says how many firings that took.
fn run(net: &mut Net) -> usize {
    let mut fired = 0;
    while net.step().expect("no body in this net fails").is_some() {
        fired += 1;
    }
    fired
}

/// The example from DESIGN §5, fired once.
#[test]
fn the_weigh_example_fires() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<Parcel>("Inbox");
    let heavy = b.place::<Parcel>("Heavy");
    let light = b.place::<Parcel>("Light");
    let count = b.place::<u32>("Count");
    let config = b.place::<Config>("Config");
    b.seed(
        inbox,
        [
            Parcel { id: 1, weight: 3.0 },
            Parcel {
                id: 2,
                weight: 30.0,
            },
        ],
    );
    b.seed(config, [Config { max_weight: 10.0 }]);

    let weigh = b
        .transition(
            "Weigh",
            (TakeAllEdge(inbox), SnapshotEdge(config)),
            (heavy, light, count),
            |parcels: Vec<Parcel>, cfg: Config| {
                let (h, l): (Vec<_>, Vec<_>) =
                    parcels.into_iter().partition(|p| p.weight > cfg.max_weight);
                let n = (h.len() + l.len()) as u32;
                (h, l, Some(n))
            },
        )
        .id();
    let mut net = b.build().expect("the wiring of DESIGN §5");

    let consumed = pairs(&net, inbox.id());
    assert_eq!(net.fireable(), [weigh]);
    assert!(net.is_fireable(weigh));
    assert_eq!(net.step().expect("this body cannot fail"), Some(weigh));

    assert_eq!(net.count(inbox.id()), 0, "a take-all edge drained it");
    assert_eq!(
        net.count(config.id()),
        1,
        "a snapshot edge consumed nothing"
    );
    assert_eq!(
        net.tokens(heavy.id())
            .map(|(_, value)| value.downcast_ref::<Parcel>().unwrap().id)
            .collect::<Vec<_>>(),
        [2]
    );
    assert_eq!(values(&net, count.id()), [2]);
    assert_eq!(net.fired_count(), 1);
    assert_eq!(net.state_counter(), 1, "one event, one tick");

    // Produced in slot order: the heavy parcel, the light one, the count.
    let produced = [
        pairs(&net, heavy.id()),
        pairs(&net, light.id()),
        pairs(&net, count.id()),
    ]
    .concat();
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Fired {
            transition: weigh,
            consumed,
            produced,
        }]
    );
}

#[test]
fn tokens_are_taken_oldest_first_and_produced_ids_are_fresh() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [1, 2, 3]);
    b.transition("Copy", (TakeEdge(inbox),), (out,), |n: u32| (n,));
    let mut net = b.build().unwrap();

    let seeded = ids(&net, inbox.id());
    assert_eq!(run(&mut net), 3);
    assert_eq!(values(&net, out.id()), [1, 2, 3], "FIFO in, FIFO out");

    let produced = ids(&net, out.id());
    let mut sorted = produced.clone();
    sorted.sort();
    assert_eq!(produced, sorted, "ids are minted in order");
    assert!(
        produced[0] > seeded[2],
        "a consumed id retires; every produced token is a new token"
    );
}

#[test]
fn a_take_all_edge_drains_its_place() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let total = b.place::<u32>("Total");
    b.seed(inbox, [1, 2, 3]);
    let sum = b
        .transition("Sum", (TakeAllEdge(inbox),), (total,), |batch: Vec<u32>| {
            (batch.iter().sum::<u32>(),)
        })
        .id();
    let mut net = b.build().unwrap();

    let consumed = pairs(&net, inbox.id());
    assert_eq!(net.step().unwrap(), Some(sum));
    assert_eq!(net.count(inbox.id()), 0);
    assert_eq!(
        values(&net, total.id()),
        [6],
        "the whole batch, oldest first"
    );
    assert!(net.fireable().is_empty(), "an empty place is not fireable");

    match net.drain_events().next() {
        Some(Event::Fired { consumed: got, .. }) => assert_eq!(got, consumed),
        other => panic!("expected a Fired event, got {other:?}"),
    }
}

#[test]
fn a_snapshot_edge_reads_without_consuming() {
    let seen = Arc::new(Mutex::new(Vec::<Config>::new()));
    let log = Arc::clone(&seen);

    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let config = b.place::<Config>("Config");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [1]);
    b.seed(config, [Config { max_weight: 10.0 }]);
    b.transition(
        "Scale",
        (TakeEdge(inbox), SnapshotEdge(config)),
        (out,),
        move |n: u32, cfg: Config| {
            log.lock().expect("not poisoned").push(cfg.clone());
            (n * cfg.max_weight as u32,)
        },
    );
    let mut net = b.build().unwrap();

    let before = ids(&net, config.id());
    assert!(net.step().unwrap().is_some());

    assert_eq!(
        ids(&net, config.id()),
        before,
        "the same token is still there"
    );
    assert_eq!(
        net.tokens(config.id())
            .map(|(_, value)| value.downcast_ref::<Config>().unwrap().clone())
            .collect::<Vec<_>>(),
        [Config { max_weight: 10.0 }],
    );
    assert_eq!(
        *seen.lock().unwrap(),
        [Config { max_weight: 10.0 }],
        "the body saw a clone equal to the token"
    );
    assert_eq!(values(&net, out.id()), [10]);
}

#[test]
fn a_mutating_edge_changes_a_token_in_place() {
    let mut b = NetBuilder::new();
    let ticks = b.place::<u32>("Ticks");
    let live = b.place::<u32>("Live");
    b.seed(ticks, [5]);
    b.seed(live, [10, 99]);
    let bump = b
        .transition(
            "Bump",
            (TakeEdge(ticks), MutateEdge(live)),
            (),
            |tick: u32, live: &mut u32| {
                *live += tick;
            },
        )
        .id();
    let mut net = b.build().unwrap();

    let tick = pairs(&net, ticks.id());
    let before = ids(&net, live.id());
    assert_eq!(net.step().unwrap(), Some(bump));

    assert_eq!(
        values(&net, live.id()),
        [15, 99],
        "the front token, in place"
    );
    assert_eq!(ids(&net, live.id()), before, "same ids, same positions");
    assert_eq!(net.state_counter(), 2, "one Mutated and one Fired");
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [
            Event::Mutated {
                place: live.id(),
                token: before[0],
            },
            Event::Fired {
                transition: bump,
                consumed: tick,
                produced: Vec::new(),
            },
        ],
        "the mutation is reported before the firing that made it"
    );
}

#[test]
fn two_mutating_edges_borrow_two_places() {
    let mut b = NetBuilder::new();
    let left = b.place::<u32>("Left");
    let right = b.place::<u32>("Right");
    b.seed(left, [1]);
    b.seed(right, [2]);
    b.transition(
        "Swap",
        (MutateEdge(left), MutateEdge(right)),
        (),
        |a: &mut u32, b: &mut u32| std::mem::swap(a, b),
    );
    let mut net = b.build().unwrap();

    assert!(net.step().unwrap().is_some());
    assert_eq!(values(&net, left.id()), [2]);
    assert_eq!(values(&net, right.id()), [1]);
    assert_eq!(
        net.state_counter(),
        3,
        "one Mutated per mutating edge, plus the Fired"
    );
}

#[test]
fn an_empty_option_slot_produces_nothing() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [1, 2]);
    b.transition("Only even", (TakeEdge(inbox),), (out,), |n: u32| {
        (Some(n).filter(|n| n % 2 == 0),)
    });
    let mut net = b.build().unwrap();

    assert!(net.step().unwrap().is_some());
    assert_eq!(net.count(out.id()), 0, "the odd one produced no token");
    match net.drain_events().next() {
        Some(Event::Fired { produced, .. }) => assert!(produced.is_empty()),
        other => panic!("expected a Fired event, got {other:?}"),
    }

    assert!(net.step().unwrap().is_some());
    assert_eq!(values(&net, out.id()), [2]);
}

#[test]
fn a_vec_slot_produces_one_token_each() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [3]);
    b.transition("Fan out", (TakeEdge(inbox),), (out,), |n: u32| {
        ((1..=n).collect::<Vec<u32>>(),)
    });
    let mut net = b.build().unwrap();

    assert!(net.step().unwrap().is_some());
    assert_eq!(values(&net, out.id()), [1, 2, 3]);

    let produced = pairs(&net, out.id());
    match net.drain_events().next() {
        Some(Event::Fired { produced: got, .. }) => {
            assert_eq!(got, produced, "one token per element, in the Vec's order");
        }
        other => panic!("expected a Fired event, got {other:?}"),
    }
}

#[test]
fn a_transition_with_no_outputs_produces_nothing() {
    let counted = Arc::new(AtomicU32::new(0));
    let tally = Arc::clone(&counted);

    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    b.seed(inbox, [1, 2]);
    b.transition("Sink", (TakeEdge(inbox),), (), move |n: u32| {
        tally.fetch_add(n, Ordering::Relaxed);
    });
    let mut net = b.build().unwrap();

    assert_eq!(run(&mut net), 2);
    assert_eq!(counted.load(Ordering::Relaxed), 3);
    assert!(
        net.drain_events()
            .all(|event| matches!(event, Event::Fired { ref produced, .. } if produced.is_empty()))
    );
}

#[test]
fn a_transition_with_no_inputs_is_always_fireable() {
    let mut b = NetBuilder::new();
    let out = b.place::<u32>("Out");
    let spawn = b.transition("Spawn", (), (out,), || (1_u32,)).id();
    let mut net = b.build().unwrap();

    assert_eq!(net.fireable(), [spawn], "nothing has to be there for it");
    assert_eq!(net.step().unwrap(), Some(spawn));
    assert_eq!(values(&net, out.id()), [1]);
    assert_eq!(
        net.fireable(),
        [spawn],
        "a source transition never runs dry; only one step is taken here"
    );
}

#[test]
fn a_guard_that_says_no_makes_a_transition_unfireable() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [1]);
    let never = b
        .transition("Never", (TakeEdge(inbox),), (out,), |n: u32| (n,))
        .guard(|_net| false)
        .id();
    let mut net = b.build().unwrap();

    assert!(!net.is_fireable(never));
    assert!(net.is_enabled(never), "the switch is on; the guard says no");
    assert!(net.fireable().is_empty());
    assert_eq!(net.step().unwrap(), None);
    assert_eq!(net.count(inbox.id()), 1, "nothing was consumed");
}

#[test]
fn a_guard_may_read_the_marking() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let total = b.place::<u32>("Total");
    b.seed(inbox, [1]);
    let pairs_up = b
        .transition(
            "Pair up",
            (TakeAllEdge(inbox),),
            (total,),
            |batch: Vec<u32>| (batch.iter().sum::<u32>(),),
        )
        .guard(move |net| net.count(inbox.id()) >= 2)
        .id();
    let mut net = b.build().unwrap();

    assert!(!net.is_fireable(pairs_up), "one token is not two");
    assert_eq!(net.step().unwrap(), None);

    net.put(inbox, 2);
    assert!(net.is_fireable(pairs_up));
    assert_eq!(net.step().unwrap(), Some(pairs_up));
    assert_eq!(values(&net, total.id()), [3]);
}

#[test]
fn the_default_selector_prefers_the_higher_priority() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [1]);
    // Declared first and scoring 0.0, because it sets no priority at all.
    let plain = b
        .transition("Plain", (TakeEdge(inbox),), (out,), |n: u32| (n,))
        .id();
    let urgent = b
        .transition("Urgent", (TakeEdge(inbox),), (out,), |n: u32| (n * 10,))
        .priority(|_net| 1.0)
        .id();
    let mut net = b.build().unwrap();

    assert_eq!(net.fireable(), [plain, urgent], "declaration order");
    assert_eq!(net.step().unwrap(), Some(urgent), "priority beats order");
    assert_eq!(values(&net, out.id()), [10]);
}

#[test]
fn a_tie_in_priority_goes_to_the_transition_declared_first() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [1]);
    let first = b
        .transition("First", (TakeEdge(inbox),), (out,), |n: u32| (n,))
        .priority(|_net| 2.0)
        .id();
    let second = b
        .transition("Second", (TakeEdge(inbox),), (out,), |n: u32| (n * 10,))
        .priority(|_net| 2.0)
        .id();
    let mut net = b.build().unwrap();

    assert_eq!(net.fireable(), [first, second]);
    assert_eq!(net.step().unwrap(), Some(first));
    assert_eq!(values(&net, out.id()), [1]);
}

#[test]
fn a_custom_selector_chooses_and_sees_the_fireable_list() {
    let offered = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&offered);

    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [1]);
    let first = b
        .transition("First", (TakeEdge(inbox),), (out,), |n: u32| (n,))
        .priority(|_net| 5.0)
        .id();
    let second = b
        .transition("Second", (TakeEdge(inbox),), (out,), |n: u32| (n * 10,))
        .id();
    let mut net = b.build().unwrap();

    // The last one, whatever the priorities say.
    net.set_selector(move |_net, fireable| {
        log.lock().expect("not poisoned").push(fireable.to_vec());
        fireable.last().copied()
    });

    assert_eq!(net.step().unwrap(), Some(second));
    assert_eq!(values(&net, out.id()), [10]);
    assert_eq!(
        *offered.lock().unwrap(),
        [vec![first, second]],
        "the fireable transitions, in declaration order"
    );
}

#[test]
fn a_selector_may_decline_to_fire_anything() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [1]);
    let copy = b
        .transition("Copy", (TakeEdge(inbox),), (out,), |n: u32| (n,))
        .id();
    let mut net = b.build().unwrap();
    net.set_selector(|_net, _fireable| None);

    assert_eq!(net.fireable(), [copy], "fireable, but not chosen");
    assert_eq!(net.step().unwrap(), None);
    assert_eq!(net.fired_count(), 0);
    assert_eq!(net.count(inbox.id()), 1);
}

#[test]
fn disabling_a_transition_stops_it_firing() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [1]);
    let copy = b
        .transition("Copy", (TakeEdge(inbox),), (out,), |n: u32| (n,))
        .id();
    let mut net = b.build().unwrap();

    net.disable(copy).expect("a transition of this net");
    assert!(!net.is_enabled(copy));
    assert!(!net.is_fireable(copy));
    assert!(net.fireable().is_empty());
    assert_eq!(net.step().unwrap(), None);
    assert_eq!(net.state_counter(), 1);
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Toggled {
            transition: copy,
            enabled: false,
        }]
    );
}

#[test]
fn disabling_twice_reports_one_change() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    let copy = b
        .transition("Copy", (TakeEdge(inbox),), (out,), |n: u32| (n,))
        .id();
    let mut net = b.build().unwrap();

    net.disable(copy).unwrap();
    net.disable(copy).unwrap();
    assert_eq!(net.drain_events().count(), 1, "the switch moved once");
    assert_eq!(net.state_counter(), 1);
}

#[test]
fn enabling_a_transition_again_lets_it_fire() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [1]);
    let copy = b
        .transition("Copy", (TakeEdge(inbox),), (out,), |n: u32| (n,))
        .id();
    let mut net = b.build().unwrap();

    net.disable(copy).unwrap();
    assert_eq!(net.step().unwrap(), None);
    net.enable(copy).unwrap();
    assert!(net.is_enabled(copy));
    assert_eq!(net.step().unwrap(), Some(copy));
    assert_eq!(values(&net, out.id()), [1]);
}

#[test]
fn firing_a_named_transition_by_hand_works() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [1]);
    let copy = b
        .transition("Copy", (TakeEdge(inbox),), (out,), |n: u32| (n,))
        .id();
    // A second, higher-priority transition the selector would have chosen.
    b.transition("Other", (), (out,), || (99_u32,))
        .priority(|_net| 10.0);
    let mut net = b.build().unwrap();

    net.fire(copy).expect("fireable");
    assert_eq!(values(&net, out.id()), [1], "the one that was named fired");
    assert_eq!(net.fired_count(), 1);
}

#[test]
fn firing_a_transition_that_cannot_fire_is_an_error() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    let copy = b
        .transition("Copy", (TakeEdge(inbox),), (out,), |n: u32| (n,))
        .id();
    let mut net = b.build().unwrap();

    // Empty input place.
    assert!(matches!(
        net.fire(copy),
        Err(NetError::NotFireable { transition }) if transition == copy
    ));

    // Switched off, with a token waiting.
    net.put(inbox, 1);
    net.disable(copy).unwrap();
    assert!(matches!(
        net.fire(copy),
        Err(NetError::NotFireable { transition }) if transition == copy
    ));
    assert_eq!(net.count(inbox.id()), 1, "nothing was consumed");
}

#[test]
fn an_unknown_transition_is_rejected() {
    let mut other = NetBuilder::new();
    let p = other.place::<u32>("P");
    other.transition("First", (), (p,), || (1_u32,));
    let stranger = other.transition("Second", (), (p,), || (2_u32,)).id();

    let mut b = NetBuilder::new();
    let out = b.place::<u32>("Out");
    b.transition("Only one", (), (out,), || (1_u32,));
    let mut net = b.build().unwrap();

    assert!(!net.is_fireable(stranger));
    assert!(!net.is_enabled(stranger));
    assert!(matches!(
        net.fire(stranger),
        Err(NetError::NoSuchTransition { transition }) if transition == stranger
    ));
    assert!(matches!(
        net.disable(stranger),
        Err(NetError::NoSuchTransition { .. })
    ));
    assert_eq!(net.drain_events().count(), 0);
}

#[test]
fn a_body_that_returns_err_loses_its_inputs_and_leaves_the_net_usable() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [1, 2]);
    let fussy = b
        .transition(
            "Fussy",
            (TakeEdge(inbox),),
            (out,),
            |n: u32| -> Result<(u32,), Refused> { if n == 1 { Err(Refused) } else { Ok((n,)) } },
        )
        .id();
    let mut net = b.build().unwrap();
    let consumed = vec![pairs(&net, inbox.id())[0]];

    let failure = net.step().expect_err("the first token is refused");
    assert_eq!(failure.transition, fussy);
    assert!(
        !failure.restored,
        "this transition does not restore its inputs"
    );
    assert_eq!(failure.error.to_string(), "refused");
    assert!(failure.to_string().ends_with("failed: refused"));

    assert_eq!(net.count(inbox.id()), 1, "the consumed token is gone");
    assert_eq!(net.count(out.id()), 0, "nothing was distributed");
    assert_eq!(net.fired_count(), 0, "a failure is not a firing");
    assert_eq!(net.state_counter(), 1);
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Failed {
            transition: fussy,
            consumed,
            restored: false,
            error: "refused".to_owned(),
        }]
    );

    // The net is consistent: the body is back in its slot and the next token
    // fires normally.
    assert_eq!(net.step().unwrap(), Some(fussy));
    assert_eq!(values(&net, out.id()), [2]);
    assert_eq!(net.fired_count(), 1);
}

/// `fire` has no third answer for a body that fails, so the failure arrives
/// wrapped: [`NetError::FiringFailed`] carries the same value `step` returns.
#[test]
fn a_failing_body_reaches_fire_as_a_net_error() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [1]);
    let fussy = b
        .transition(
            "Fussy",
            (TakeEdge(inbox),),
            (out,),
            |_n: u32| -> Result<(u32,), Refused> { Err(Refused) },
        )
        .id();
    let mut net = b.build().unwrap();

    match net.fire(fussy) {
        Err(NetError::FiringFailed(failure)) => {
            assert_eq!(failure.transition, fussy);
            assert!(!failure.restored);
            assert_eq!(failure.error.to_string(), "refused");
        }
        other => panic!("expected a wrapped firing failure, got {other:?}"),
    }
    assert_eq!(net.fired_count(), 0);
    assert!(net.fireable().is_empty(), "its input was consumed and lost");
}

#[test]
fn a_body_may_count_its_own_firings() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [10, 20, 30]);
    let mut firings = 0_u32;
    b.transition("Number them", (TakeEdge(inbox),), (out,), move |_n: u32| {
        firings += 1;
        (firings,)
    });
    let mut net = b.build().unwrap();

    assert_eq!(run(&mut net), 3);
    assert_eq!(
        values(&net, out.id()),
        [1, 2, 3],
        "the body's own state survives between firings"
    );
}

#[test]
fn a_quiescent_net_fires_nothing() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.transition("Copy", (TakeEdge(inbox),), (out,), |n: u32| (n,));
    let mut net = b.build().unwrap();

    assert!(net.fireable().is_empty());
    assert_eq!(net.step().unwrap(), None);
    assert_eq!(net.fired_count(), 0);
    assert_eq!(net.state_counter(), 0);
    assert_eq!(net.drain_events().count(), 0);
}

#[test]
fn two_input_edges_into_one_place_are_a_build_error() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.transition(
        "Twice",
        (TakeEdge(inbox), TakeEdge(inbox)),
        (out,),
        |a: u32, c: u32| (a + c,),
    );
    match b.build() {
        Err(BuildError::DuplicateInputPlace { transition, place }) => {
            assert_eq!(transition, "Twice");
            assert_eq!(place, "Inbox");
        }
        other => panic!("expected a duplicate input place error, got {other:?}"),
    }

    // Any two kinds, not only two of the same.
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.transition(
        "Take and mutate",
        (TakeEdge(inbox), MutateEdge(inbox)),
        (out,),
        |a: u32, live: &mut u32| {
            *live += a;
            (*live,)
        },
    );
    assert!(matches!(
        b.build(),
        Err(BuildError::DuplicateInputPlace { .. })
    ));
}

/// DESIGN §6.6: a body is an ordinary closure, so it may own a whole net —
/// which is only true while `Net` is `Send + Sync` and needs no engine
/// support at all.
#[test]
fn a_net_can_be_the_body_of_a_transition() {
    // The inner net triples whatever is put into its input place.
    let mut inner_b = NetBuilder::new();
    let inner_in = inner_b.place::<u32>("Inner in");
    let inner_out = inner_b.place::<u32>("Inner out");
    inner_b.transition("Triple", (TakeEdge(inner_in),), (inner_out,), |n: u32| {
        (n * 3,)
    });
    let mut inner = inner_b.build().expect("the inner net");

    let mut b = NetBuilder::new();
    let outer_in = b.place::<u32>("Outer in");
    let outer_out = b.place::<u32>("Outer out");
    b.seed(outer_in, [1, 2]);
    let compose = b
        .transition(
            "Inner net",
            (TakeEdge(outer_in),),
            (outer_out,),
            move |n: u32| {
                inner.put(inner_in, n);
                // Run the inner net to quiescence, then drain its output.
                while inner
                    .step()
                    .expect("no body in the inner net fails")
                    .is_some()
                {}
                let drained: Vec<TokenId> =
                    inner.tokens(inner_out.id()).map(|(id, _)| id).collect();
                let mut results = Vec::new();
                for token in drained {
                    results.push(
                        *inner
                            .token::<u32>(inner_out.id(), token)
                            .expect("the place holds u32"),
                    );
                    inner.remove(inner_out.id(), token).expect("just read");
                }
                // The inner net's log is the inner net's business.
                inner.drain_events().for_each(drop);
                (results,)
            },
        )
        .id();
    let mut net = b.build().expect("the outer net");

    assert_eq!(net.step().unwrap(), Some(compose));
    assert_eq!(net.step().unwrap(), Some(compose));
    assert_eq!(values(&net, outer_out.id()), [3, 6]);
    assert_eq!(
        net.drain_events().count(),
        2,
        "observability stops at the boundary: two outer firings, nothing from inside"
    );
}
