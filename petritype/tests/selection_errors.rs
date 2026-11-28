//! A choice the net cannot make is an error (since 2026-11-28): a fireable
//! transition whose priority is `NaN`, and a custom selector's answer that
//! is not among the transitions it was offered. Whatever asked fails with
//! [`SelectionFailed`], inside a [`StepError`] for a step and a
//! [`DriverError::SelectionFailed`] for a driver, and nothing fires. Until
//! then a `NaN` counted as `0.0` and such an answer fired nothing. An
//! infinity is a priority like any other.
//!
//! The Python library's `TestANaNPriorityIsAnError`
//! (`tests/test_guards_and_priorities.py`) is the same rule: its default
//! selector raises `ValueError`.

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use petritype::drivers::{self, DriverError, Limits, Mode, OnFailure};
use petritype::{
    MutateEdge, Net, NetBuilder, NetError, PathStep, PlaceId, SelectionFailed, SelectionFault,
    StepError, TakeEdge, TieBreak, TransitionId,
};

const TIE_BREAKS: [TieBreak; 3] = [
    TieBreak::FirstDeclared,
    TieBreak::LastDeclared,
    TieBreak::RoundRobin,
];

/// One place, `In`, holding one token and feeding every named transition,
/// each of which puts into an `Out` of its own, so exactly one of them can
/// fire. `priority` gives each its priority, `None` for one that sets none.
fn shared_input(
    names: &[&str],
    priority: impl Fn(&str) -> Option<f64>,
    tie_break: TieBreak,
) -> (Net, PlaceId, Vec<TransitionId>) {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("In");
    b.seed(inbox, [1]);
    let mut ids = Vec::new();
    for name in names {
        let out = b.place::<u32>(&format!("{name} Out"));
        let mut transition = b.transition(*name, (TakeEdge(inbox),), (out,), |n: u32| (n + 1,));
        if let Some(score) = priority(name) {
            transition.priority(move |_| score);
        }
        ids.push(transition.id());
    }
    b.tie_break(tie_break);
    (b.build().expect("the net builds"), inbox.id(), ids)
}

/// The `NaN` a choice failed on, by its transition.
fn nan_on(failed: &SelectionFailed) -> TransitionId {
    match &failed.why {
        SelectionFault::NanPriority { transition, .. } => *transition,
        other => panic!("a NaN priority, not {other:?}"),
    }
}

/// Nothing fired and nothing was told.
fn untouched(net: &mut Net, inbox: PlaceId) {
    assert_eq!(net.count(inbox), 1, "the token is where it was");
    assert_eq!(net.fired_count(), 0);
    assert_eq!(net.drain_events().count(), 0, "nothing happened");
}

#[test]
fn a_nan_priority_fails_whatever_asked_and_nothing_fires() {
    for tie_break in TIE_BREAKS {
        for (on, at) in [("A", 0), ("B", 1)] {
            let nan = |name: &str| (name == on).then_some(f64::NAN);
            let (mut net, inbox, ids) = shared_input(&["A", "B"], nan, tie_break);
            net.drain_events().for_each(drop);
            let context = format!("{tie_break:?}, NaN on {on}");

            let failed = net.priority_of(ids[at]).expect_err(&context);
            assert_eq!(nan_on(&failed), ids[at], "{context}");
            assert!(failed.path.is_empty());
            assert_eq!(nan_on(&net.select().expect_err(&context)), ids[at]);
            let chose = net.choose().expect_err(&context);
            assert_eq!(nan_on(chose.selection().expect("a choice")), ids[at]);
            for stepped in [net.step().map(|_| ()), net.step_deep().map(|_| ())] {
                let error = stepped.expect_err(&context);
                let failed = error.selection().expect("a choice, not a firing");
                assert_eq!(nan_on(failed), ids[at], "{context}");
                assert!(error.firing().is_none());
            }
            let failed = net.step().unwrap_err();
            assert_eq!(
                failed.to_string(),
                format!(
                    "the priority of transition {on:?} ({:?}) is NaN, which ranks neither above \
                     nor below any other, so no transition can be chosen by it. Give it a \
                     number; infinity ranks as itself",
                    ids[at]
                )
            );
            untouched(&mut net, inbox);
        }
    }
}

#[test]
fn an_infinity_ranks_as_itself() {
    for tie_break in TIE_BREAKS {
        let priority = |name: &str| match name {
            "A" => Some(f64::NEG_INFINITY),
            "C" => Some(f64::INFINITY),
            _ => None,
        };
        let (mut net, _, ids) = shared_input(&["A", "B", "C"], priority, tie_break);
        assert_eq!(net.priority_of(ids[0]), Ok(f64::NEG_INFINITY));
        assert_eq!(net.priority_of(ids[1]), Ok(0.0), "none set");
        assert_eq!(net.priority_of(ids[2]), Ok(f64::INFINITY));
        assert_eq!(net.step().unwrap(), Some(ids[2]), "{tie_break:?}");
    }
    // Two infinities tie, and the tie-break settles it.
    for (tie_break, winner) in [(TieBreak::FirstDeclared, 0), (TieBreak::LastDeclared, 1)] {
        let (mut net, _, ids) = shared_input(&["A", "B"], |_| Some(f64::INFINITY), tie_break);
        assert_eq!(net.step().unwrap(), Some(ids[winner]), "{tie_break:?}");
    }
    // Every priority minus infinity: they all tie at the top.
    let (mut net, _, ids) = shared_input(
        &["A", "B"],
        |_| Some(f64::NEG_INFINITY),
        TieBreak::RoundRobin,
    );
    assert_eq!(net.step().unwrap(), Some(ids[0]));
}

#[test]
fn a_selector_that_names_a_transition_it_was_not_offered_is_an_error() {
    // An id this net does not have: the third transition of another net.
    let (_, _, others) = shared_input(&["X", "Y", "Z"], |_| None, TieBreak::FirstDeclared);
    let foreign = others[2];

    let (mut net, inbox, ids) = shared_input(&["A", "B"], |_| None, TieBreak::FirstDeclared);
    net.disable(ids[1]).expect("B exists");
    net.drain_events().for_each(drop);
    let offered = vec![(ids[0], "A".to_owned())];
    for (chosen, name) in [(ids[1], Some("B".to_owned())), (foreign, None)] {
        net.set_selector(move |_, _| Some(chosen));
        let expected = SelectionFault::NotOffered {
            chosen,
            name: name.clone(),
            offered: offered.clone(),
        };
        let failed = net.select().unwrap_err();
        assert_eq!((&failed.why, failed.path.len()), (&expected, 0));
        let chose = net.choose().unwrap_err();
        assert_eq!(chose.selection().map(|f| &f.why), Some(&expected));
        let error = net.step().unwrap_err();
        assert_eq!(error.selection().map(|f| &f.why), Some(&expected));
        let message = error.to_string();
        match name {
            Some(_) => assert_eq!(
                message,
                format!(
                    "the selector chose transition \"B\" ({:?}), which is not fireable now; it \
                     was offered the fireable transitions [\"A\" ({:?})], and answers with one \
                     of them, or None to fire nothing",
                    ids[1], ids[0]
                )
            ),
            None => assert!(
                message.starts_with(&format!(
                    "the selector chose {foreign:?}, which this net does not have;"
                )),
                "{message}"
            ),
        }
        untouched(&mut net, inbox);
    }
    // One of the transitions offered, or None, is a choice.
    net.set_selector(|_, _| None);
    assert_eq!(net.step().unwrap(), None);
    net.set_selector(|_, fireable| fireable.first().copied());
    assert_eq!(net.step().unwrap(), Some(ids[0]));
}

/// A net carried by a token in `Desk`, whose one transition, `Work`, has a
/// priority of `NaN`.
fn desk_around_a_nan() -> (Net, PlaceId, PlaceId) {
    let mut inner = NetBuilder::new();
    let todo = inner.place::<u32>("Todo");
    let done = inner.place::<u32>("Done");
    inner.seed(todo, [1]);
    inner
        .transition("Work", (TakeEdge(todo),), (done,), |n: u32| (n,))
        .priority(|_| f64::NAN);
    let inner = inner.build().expect("the inner net builds");
    let mut b = NetBuilder::new();
    let desk = b.place_nested::<Net>("Desk");
    b.seed(desk, [inner]);
    (b.build().expect("the net builds"), desk.id(), todo.id())
}

#[test]
fn a_choice_an_inner_net_cannot_make_says_where_and_fires_nothing() {
    let (mut net, desk, todo) = desk_around_a_nan();
    let (token, _) = net.tokens(desk).next().expect("a desk");
    net.drain_events().for_each(drop);

    for stepped in [net.step_deep(), net.step_inside()] {
        let StepError::SelectionFailed(failed) = stepped.expect_err("Work's priority is NaN")
        else {
            panic!("a choice, not a firing");
        };
        let way = vec![PathStep::Token {
            place: desk,
            token,
            net: String::new(),
        }];
        assert_eq!(failed.path, way);
        assert!(
            failed
                .to_string()
                .starts_with("the priority of transition \"Work\" (TransitionId(0)) inside "),
            "{failed}"
        );
    }
    let inner = &net.nested(desk, token).expect("the desk's net")[0].1;
    assert_eq!(inner.count(todo), 1, "Work did not fire");
    assert_eq!(net.drain_events().count(), 0);
}

#[test]
fn a_choice_a_subnet_cannot_make_names_the_subnet_place() {
    let mut inner = NetBuilder::new();
    let todo = inner.place::<u32>("Todo");
    let done = inner.place::<u32>("Done");
    inner.seed(todo, [1]);
    inner
        .transition("Work", (TakeEdge(todo),), (done,), |n: u32| (n,))
        .priority(|_| f64::NAN);
    let inner = inner.build().expect("the subnet builds");
    let mut b = NetBuilder::new();
    let level = b.subnet_place("Level", inner);
    let mut net = b.build().expect("the net builds");
    let failed = net.step_deep().expect_err("Work's priority is NaN");
    assert_eq!(failed.path(), [PathStep::Subnet { place: level.id() }]);
    assert!(matches!(
        failed.selection().map(|f| &f.why),
        Some(SelectionFault::NanPriority { name, .. }) if name == "Work"
    ));
}

#[test]
fn a_run_stops_on_a_choice_it_cannot_make_whatever_on_failure_says() {
    for on_failure in [OnFailure::Stop, OnFailure::Disable, OnFailure::Continue] {
        let (mut net, inbox, ids) = shared_input(
            &["A", "B"],
            |name| (name == "B").then_some(f64::NAN),
            TieBreak::FirstDeclared,
        );
        let error = drivers::run(&mut net, Mode::Sequential, Limits::default(), on_failure)
            .expect_err("B's priority is NaN");
        let DriverError::SelectionFailed { summary, error } = error else {
            panic!("{on_failure:?}: a choice, not {error:?}");
        };
        assert_eq!(nan_on(&error), ids[1]);
        assert_eq!(
            (summary.quiesced, summary.fired_count, summary.failed_count),
            (false, 0, 0),
            "{on_failure:?}: nothing fired, nothing failed"
        );
        assert_eq!(net.count(inbox), 1);
        assert!(
            net.is_enabled(ids[1]),
            "{on_failure:?}: nothing is switched off"
        );

        let stop = AtomicBool::new(false);
        let error = drivers::run_indefinitely(
            &mut net,
            Mode::Sequential,
            Duration::from_millis(1),
            &stop,
            on_failure,
            |_| {},
        )
        .expect_err("B's priority is NaN");
        assert!(
            matches!(error, DriverError::SelectionFailed { .. }),
            "{error:?}"
        );
    }
    // The concurrent driver steps inner nets by their own selectors.
    for on_failure in [OnFailure::Stop, OnFailure::Continue] {
        let (mut net, ..) = desk_around_a_nan();
        let error = drivers::run(&mut net, Mode::Concurrent, Limits::default(), on_failure)
            .expect_err("Work's priority is NaN");
        let DriverError::SelectionFailed { summary, error } = error else {
            panic!("{on_failure:?}: a choice, not {error:?}");
        };
        assert_eq!(error.path.len(), 1, "inside the desk's net");
        assert_eq!((summary.fired_count, summary.failed_count), (0, 0));
        let stop = AtomicBool::new(false);
        let error = drivers::run_indefinitely(
            &mut net,
            Mode::Concurrent,
            Duration::from_millis(1),
            &stop,
            on_failure,
            |_| {},
        )
        .expect_err("Work's priority is NaN");
        assert!(
            matches!(error, DriverError::SelectionFailed { .. }),
            "{error:?}"
        );
    }
}

#[test]
fn a_body_that_steps_a_net_it_holds_passes_the_choice_on_as_a_net_error() {
    let (inner, _, _) = shared_input(&["A"], |_| Some(f64::NAN), TieBreak::FirstDeclared);
    let mut b = NetBuilder::new();
    let cranks = b.place::<()>("Cranks");
    let machines = b.place_nested::<Net>("Machines");
    b.seed(cranks, [()]);
    b.seed(machines, [inner]);
    b.stepping(machines, petritype::Stepping::Passive);
    b.transition(
        "Crank",
        (TakeEdge(cranks), MutateEdge(machines)),
        (),
        |(): (), machine: &mut Net| -> Result<(), NetError> {
            machine.step()?;
            Ok(())
        },
    );
    let mut net = b.build().expect("the net builds");
    let failed = net
        .step()
        .expect_err("the body passes the choice on")
        .into_firing()
        .expect("the body's firing failed");
    let error = failed.error.downcast_ref::<NetError>().expect("a NetError");
    let NetError::SelectionFailed(chosen) = error else {
        panic!("a choice, not {error:?}");
    };
    assert!(matches!(chosen.why, SelectionFault::NanPriority { .. }));
    assert!(
        chosen.path.is_empty(),
        "the body stepped the machine itself"
    );
}
