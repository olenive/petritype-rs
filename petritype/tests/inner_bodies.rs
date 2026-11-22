//! Integration tests for the observer of a staged deep step,
//! [`Net::step_deep_staged_with`] (DESIGN §6.6).
//!
//! An inner firing runs its body inside the deep step, before the step
//! returns, so a driver that reports what is running learns of such a body
//! only through the observer, which the step calls just before the body
//! starts. These tests are the three ways down to an inner body (a token's
//! net, a subnet, and a subnet inside a token's net), the turns that tell
//! of nothing (the net's own choice, a step that fires nothing, a body that
//! steps the nets of its own tokens), a failure, and that a step is the same
//! with an observer as without one.

use std::sync::{Arc, Mutex};

use petritype::{
    DeepStep, Event, InnerBody, MutateEdge, Net, NetBuilder, NetId, PathStep, PlaceId, StagedStep,
    Stepping, TakeEdge, TokenId, TransitionId,
};

// ----- a few nets -----------------------------------------------------------------

/// What the bodies and the observer saw, in the order they saw it.
type Diary = Arc<Mutex<Vec<String>>>;

/// What one call of the observer was told, owned.
#[derive(Debug, Clone, PartialEq)]
struct Told {
    transition: TransitionId,
    name: String,
    path: Vec<PathStep>,
    nested_ids: Vec<NetId>,
}

impl Told {
    fn of(body: &InnerBody<'_>) -> Told {
        assert_eq!(body.nested_id(), *body.nested_ids.last().unwrap());
        Told {
            transition: body.transition,
            name: body.name.to_owned(),
            path: body.path.to_vec(),
            nested_ids: body.nested_ids.to_vec(),
        }
    }
}

/// An inner net with `jobs` things to do, `Todo -Work-> Done`, whose body
/// writes `"Work <n>"` in `diary` as it runs.
fn worker(jobs: u32, diary: &Diary) -> (Net, TransitionId) {
    let diary = Arc::clone(diary);
    let mut b = NetBuilder::new();
    let todo = b.place::<u32>("Todo");
    let done = b.place::<u32>("Done");
    b.seed(todo, 0..jobs);
    let work = b
        .transition("Work", (TakeEdge(todo),), (done,), move |n: u32| {
            diary.lock().unwrap().push(format!("Work {n}"));
            (n,)
        })
        .id();
    (b.build().expect("a worker"), work)
}

/// One deep step with an observer that writes `"told <name>"` in `diary`
/// and keeps what it was told.
fn step_told(
    net: &mut Net,
    diary: &Diary,
) -> (
    Result<Option<StagedStep>, petritype::FiringFailed>,
    Vec<Told>,
) {
    let mut told = Vec::new();
    let step = net.step_deep_staged_with(|body| {
        diary.lock().unwrap().push(format!("told {}", body.name));
        told.push(Told::of(body));
    });
    (step, told)
}

/// The one token of `place`, and the nested id of its one net.
fn the_token(net: &Net, place: PlaceId) -> (TokenId, NetId) {
    let (token, _) = net.tokens(place).next().expect("a token");
    let id = net.nested(place, token).expect("a place of nets")[0].1.id();
    (token, id)
}

fn entries(diary: &Diary) -> Vec<String> {
    diary.lock().unwrap().clone()
}

// ----- the three ways down --------------------------------------------------------

#[test]
fn a_token_s_net_is_told_of_once_before_its_body_runs() {
    let diary = Diary::default();
    let (inner, work) = worker(2, &diary);
    let mut b = NetBuilder::new();
    let desk = b.place_nested::<Net>("Desk");
    b.seed(desk, [inner]);
    let mut net = b.build().unwrap();
    let (token, cabin) = the_token(&net, desk.id());

    for n in 0..2 {
        let (step, told) = step_told(&mut net, &diary);
        let Ok(Some(StagedStep::Inside(step))) = step else {
            panic!("an inner firing: {step:?}");
        };
        assert_eq!(step.transition(), work);
        assert_eq!(
            told,
            [Told {
                transition: work,
                name: "Work".into(),
                path: vec![PathStep::Token {
                    place: desk.id(),
                    token,
                    net: String::new(),
                }],
                nested_ids: vec![cabin],
            }],
            "firing {n}"
        );
    }
    assert_eq!(
        entries(&diary),
        ["told Work", "Work 0", "told Work", "Work 1"],
        "each body told of just before it ran"
    );
    let (step, told) = step_told(&mut net, &diary);
    assert_eq!(step.unwrap(), None);
    assert!(told.is_empty(), "nothing fired, nothing told");
}

#[test]
fn a_subnet_is_told_of_as_one_level_down() {
    let diary = Diary::default();
    let (level, work) = worker(1, &diary);
    let level_id = level.id();
    let mut b = NetBuilder::new();
    let level_1 = b.subnet_place("Level 1", level);
    let mut net = b.build().unwrap();

    let (step, told) = step_told(&mut net, &diary);
    assert!(matches!(
        step,
        Ok(Some(StagedStep::Inside(DeepStep::Subnet { place, nested_id, .. })))
            if place == level_1.id() && nested_id == level_id
    ));
    assert_eq!(
        told,
        [Told {
            transition: work,
            name: "Work".into(),
            path: vec![PathStep::Subnet {
                place: level_1.id()
            }],
            nested_ids: vec![level_id],
        }]
    );
    assert_eq!(entries(&diary), ["told Work", "Work 0"]);
}

#[test]
fn a_subnet_inside_a_token_s_net_is_told_of_with_both_levels() {
    let diary = Diary::default();
    let (hold, work) = worker(1, &diary);
    let hold_id = hold.id();
    let mut b = NetBuilder::new();
    let hold_place = b.subnet_place("Hold", hold);
    let ship = b.build().unwrap();
    let ship_id = ship.id();

    let mut b = NetBuilder::new();
    let quay = b.place_nested::<Net>("Quay");
    b.seed(quay, [ship]);
    let mut net = b.build().unwrap();
    let (token, _) = the_token(&net, quay.id());

    let (step, told) = step_told(&mut net, &diary);
    let Ok(Some(StagedStep::Inside(step))) = step else {
        panic!("an inner firing: {step:?}");
    };
    assert_eq!(step.transition(), work);
    let path = vec![
        PathStep::Token {
            place: quay.id(),
            token,
            net: String::new(),
        },
        PathStep::Subnet {
            place: hold_place.id(),
        },
    ];
    assert_eq!(
        told,
        [Told {
            transition: work,
            name: "Work".into(),
            path,
            nested_ids: vec![ship_id, hold_id],
        }]
    );
    assert_eq!(entries(&diary), ["told Work", "Work 0"]);
}

// ----- what is not told of --------------------------------------------------------

#[test]
fn the_net_s_own_turn_is_not_told_of() {
    let diary = Diary::default();
    let (inner, _) = worker(1, &diary);
    let mut b = NetBuilder::new();
    let desk = b.place_nested::<Net>("Desk");
    let inbox = b.place::<u32>("Inbox");
    let outbox = b.place::<u32>("Outbox");
    b.seed(desk, [inner]);
    b.seed(inbox, [7]);
    let forward = b
        .transition("Forward", (TakeEdge(inbox),), (outbox,), |n: u32| (n,))
        .id();
    // Outer first: the net's own choice comes before the desk's.
    b.stepping(desk, Stepping::OuterFirst);
    let mut net = b.build().unwrap();

    let (step, told) = step_told(&mut net, &diary);
    assert_eq!(step.unwrap(), Some(StagedStep::Chosen(forward)));
    assert!(told.is_empty(), "the choice is the caller's to fire");
    let firing = net.launch(forward).unwrap();
    net.distribute(firing.run()).unwrap();

    let (step, told) = step_told(&mut net, &diary);
    assert!(matches!(step, Ok(Some(StagedStep::Inside(_)))));
    assert_eq!(told.len(), 1, "then the desk's work, told of");
}

#[test]
fn a_body_that_steps_the_nets_of_its_own_token_is_not_told_of_for_them() {
    let diary = Diary::default();
    let (inner, _) = worker(2, &diary);
    let mut b = NetBuilder::new();
    let desk = b.place_nested::<Net>("Desk");
    let bell = b.place::<u32>("Bell");
    b.seed(desk, [inner]);
    b.seed(bell, [1]);
    // A passive desk: only `Ring`, a body of the net that is stepped,
    // steps the worker on it.
    b.stepping(desk, Stepping::Passive);
    let ring = b
        .transition(
            "Ring",
            (TakeEdge(bell), MutateEdge(desk)),
            (),
            |_: u32, worker: &mut Net| {
                worker.step().expect("the worker works");
            },
        )
        .id();
    let mut b2 = NetBuilder::new();
    let office = b2.place_nested::<Net>("Office");
    b2.seed(office, [b.build().unwrap()]);
    let mut net = b2.build().unwrap();

    // `Ring` is an inner body, one level down, and is told of; the `Work`
    // it fires two levels down is its own doing, and is not.
    let (step, told) = step_told(&mut net, &diary);
    let Ok(Some(StagedStep::Inside(step))) = step else {
        panic!("an inner firing: {step:?}");
    };
    assert_eq!(step.transition(), ring);
    assert_eq!(told.len(), 1);
    assert_eq!(told[0].transition, ring);
    assert_eq!(told[0].path.len(), 1);
    assert_eq!(entries(&diary), ["told Ring", "Work 0"]);
}

#[test]
fn a_failed_body_was_told_of_with_the_path_its_failure_carries() {
    #[derive(Debug)]
    struct Cracked;
    impl std::fmt::Display for Cracked {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("cracked")
        }
    }
    impl std::error::Error for Cracked {}

    let mut b = NetBuilder::new();
    let bench = b.place::<u32>("Bench");
    b.seed(bench, [1]);
    let glaze = b
        .transition(
            "Glaze",
            (TakeEdge(bench),),
            (),
            |_: u32| -> Result<(), Cracked> { Err(Cracked) },
        )
        .id();
    let kiln = b.build().unwrap();
    let mut b = NetBuilder::new();
    let shed = b.place_nested::<Net>("Shed");
    b.seed(shed, [kiln]);
    let mut net = b.build().unwrap();

    let (step, told) = step_told(&mut net, &Diary::default());
    let failed = step.expect_err("Glaze fails");
    assert_eq!(failed.transition, glaze);
    assert_eq!(told.len(), 1);
    assert_eq!(told[0].transition, glaze);
    assert_eq!(told[0].path, failed.path, "the same way down");
}

// ----- the same step --------------------------------------------------------------

/// A net with a bit of everything below it: a token's net under inner
/// first, a subnet taking turns, and a token's net holding a subnet under
/// outer first, besides an outer transition of its own.
fn busy(diary: &Diary) -> Net {
    let (worker_net, _) = worker(2, diary);
    let (level, _) = worker(2, diary);
    let (hold, _) = worker(1, diary);
    let mut b = NetBuilder::new();
    let hold_place = b.subnet_place("Hold", hold);
    b.stepping(hold_place, Stepping::TakeTurns);
    let ship = b.build().unwrap();

    let mut b = NetBuilder::new();
    let desk = b.place_nested::<Net>("Desk");
    let level_1 = b.subnet_place("Level 1", level);
    let quay = b.place_nested::<Net>("Quay");
    let inbox = b.place::<u32>("Inbox");
    let outbox = b.place::<u32>("Outbox");
    b.seed(desk, [worker_net]);
    b.seed(quay, [ship]);
    b.seed(inbox, [1, 2]);
    b.stepping(level_1, Stepping::TakeTurns);
    b.stepping(quay, Stepping::OuterFirst);
    b.transition("Forward", (TakeEdge(inbox),), (outbox,), |n: u32| (n,));
    b.build().unwrap()
}

/// Every net in `net`'s tree, by nested id, the net itself first: how two
/// builds of one net are compared, their ids aside.
fn ids_in(net: &Net) -> Vec<NetId> {
    let mut ids = vec![net.id()];
    for (place, info) in net.places() {
        if let Some(subnet) = net.subnet(place).filter(|_| info.subnet) {
            ids.extend(ids_in(subnet));
            continue;
        }
        for (token, _) in net.tokens(place) {
            for (_, inner) in net.nested(place, token).into_iter().flatten() {
                ids.extend(ids_in(inner));
            }
        }
    }
    ids
}

/// Runs `net` to rest, one staged deep step at a time, firing its own
/// choices in stages, and writes down every step and every event, with its
/// nets' ids replaced by their places in [`ids_in`].
fn story(net: &mut Net, watched: bool) -> String {
    let ids = ids_in(net);
    let mut lines = Vec::new();
    loop {
        let step = if watched {
            net.step_deep_staged_with(|_| {})
        } else {
            net.step_deep_staged()
        };
        let done = matches!(step, Ok(None));
        if let Ok(Some(StagedStep::Chosen(transition))) = step {
            let firing = net.launch(transition).unwrap();
            net.distribute(firing.run()).unwrap();
        }
        lines.push(format!("{step:?}"));
        lines.extend(net.drain_events().map(|event: Event| format!("{event:?}")));
        if done {
            break;
        }
    }
    let mut text = lines.join("\n");
    for (at, id) in ids.iter().enumerate() {
        text = text.replace(&format!("{id:?}"), &format!("net {at}"));
    }
    text
}

#[test]
fn a_step_is_the_same_with_an_observer_and_without_one() {
    let plain = story(&mut busy(&Diary::default()), false);
    let watched = story(&mut busy(&Diary::default()), true);
    assert!(
        plain.lines().count() > 12,
        "a story worth comparing:\n{plain}"
    );
    assert_eq!(watched, plain);
}

#[test]
fn each_inner_body_of_a_run_is_told_of_once() {
    let diary = Diary::default();
    let mut net = busy(&diary);
    let mut told = Vec::new();
    loop {
        match net.step_deep_staged_with(|body| told.push(body.name.to_owned())) {
            Ok(None) => break,
            Ok(Some(StagedStep::Chosen(transition))) => {
                let firing = net.launch(transition).unwrap();
                net.distribute(firing.run()).unwrap();
            }
            Ok(Some(_)) => {}
            Err(failed) => panic!("nothing fails: {failed}"),
        }
    }
    let bodies = entries(&diary).len();
    assert_eq!(
        bodies, 5,
        "two jobs on the desk, two on the level, one in the hold"
    );
    assert_eq!(told, vec!["Work"; bodies]);
}
