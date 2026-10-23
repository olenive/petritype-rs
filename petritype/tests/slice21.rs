//! Integration tests for the twenty-first slice: engine policies (DESIGN
//! §6.1, §6.6, §13).
//!
//! A third tie-break, [`TieBreak::RoundRobin`], takes a net's transitions in
//! turn; a deep-step policy, [`DeepPolicy::Interleaved`], takes a net and the
//! nets its tokens carry in turn, so that an outer net that can always fire
//! no longer starves its inner nets. The defaults are unchanged, which the
//! earlier slices' tests go on checking.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use petritype::drivers::{self, DriverError, Limits, Mode, OnFailure};
use petritype::{
    DeepPolicy, DeepStep, Nested, Net, NetBuilder, NetId, PlaceRef, StagedStep, TakeEdge, TieBreak,
    TransitionId,
};

// ----- round robin -------------------------------------------------------------

/// A chain `A -> First -> B -> Second -> C`, `a` tokens in `A` and `b` in `B`.
fn pipeline(a: u32, b_seed: u32, tie_break: TieBreak) -> (Net, TransitionId, TransitionId) {
    let mut b = NetBuilder::new();
    let start = b.place::<u32>("A");
    let mid = b.place::<u32>("B");
    let end = b.place::<u32>("C");
    b.seed(start, 0..a);
    b.seed(mid, 0..b_seed);
    let first = b
        .transition("First", (TakeEdge(start),), (mid,), |n: u32| (n,))
        .id();
    let second = b
        .transition("Second", (TakeEdge(mid),), (end,), |n: u32| (n,))
        .id();
    b.tie_break(tie_break);
    (b.build().expect("a pipeline"), first, second)
}

/// Three transitions competing for the tokens of one place.
fn rivals(tokens: u32) -> (Net, [TransitionId; 3]) {
    let mut b = NetBuilder::new();
    let pool = b.place::<u32>("Pool");
    let out = b.place::<u32>("Out");
    b.seed(pool, 0..tokens);
    let ids = ["Left", "Middle", "Right"].map(|name| {
        b.transition(name, (TakeEdge(pool),), (out,), |n: u32| (n,))
            .id()
    });
    b.tie_break(TieBreak::RoundRobin);
    (b.build().expect("three rivals"), ids)
}

/// Every firing `step` makes until the net is quiescent, in order.
fn run_order(net: &mut Net) -> Vec<TransitionId> {
    std::iter::from_fn(|| net.step().expect("nothing fails")).collect()
}

#[test]
fn a_pipeline_alternates_its_stages_under_round_robin() {
    let (mut net, first, second) = pipeline(3, 0, TieBreak::RoundRobin);
    assert_eq!(net.tie_break(), TieBreak::RoundRobin);
    assert_eq!(
        run_order(&mut net),
        [first, second, first, second, first, second]
    );

    // Not flow order: with tokens already in `B`, flow order drains `B`
    // first, and turn order still alternates.
    let (mut net, first, second) = pipeline(3, 2, TieBreak::RoundRobin);
    assert_eq!(
        run_order(&mut net),
        [first, second, first, second, first, second, second, second]
    );
    let (mut net, first, second) = pipeline(3, 2, TieBreak::LastDeclared);
    assert_eq!(
        run_order(&mut net),
        [second, second, first, second, first, second, first, second]
    );
}

#[test]
fn rivals_take_turns_skipping_what_cannot_fire_and_priority_still_wins() {
    let (mut net, [left, middle, right]) = rivals(6);
    assert_eq!(
        run_order(&mut net),
        [left, middle, right, left, middle, right]
    );

    // A switched-off transition is not fireable, so never a candidate.
    let (mut net, [left, middle, right]) = rivals(4);
    net.disable(middle).expect("Middle exists");
    assert_eq!(run_order(&mut net), [left, right, left, right]);

    // A tie is only a tie between equal priorities.
    let mut b = NetBuilder::new();
    let pool = b.place::<u32>("Pool");
    let out = b.place::<u32>("Out");
    b.seed(pool, 0..3);
    b.transition("Low", (TakeEdge(pool),), (out,), |n: u32| (n,));
    let high = b
        .transition("High", (TakeEdge(pool),), (out,), |n: u32| (n,))
        .priority(|_| 1.0)
        .id();
    b.tie_break(TieBreak::RoundRobin);
    let mut net = b.build().expect("a prioritised pair");
    assert_eq!(run_order(&mut net), [high, high, high]);
}

#[test]
fn the_cursor_moves_only_when_a_default_choice_is_taken() {
    let (mut net, [left, middle, right]) = rivals(12);

    // `select` reports and moves nothing.
    assert_eq!(
        net.select(),
        Some(left),
        "no cursor yet: the first declared"
    );
    assert_eq!(net.select(), Some(left));

    // `choose` is the choice taken, for a firing in stages.
    assert_eq!(net.choose(), Some(left));
    let firing = net.launch(left).expect("Left is fireable");
    net.distribute(firing.run()).expect("it lands");
    assert_eq!(net.select(), Some(middle), "the next in turn");

    // Firing by name is not the default selector's choice.
    net.fire(right).expect("Right is fireable");
    assert_eq!(net.select(), Some(middle), "the cursor stayed on Left");

    // A custom selector neither reads nor moves it.
    net.set_selector(|_, fireable| fireable.last().copied());
    assert_eq!(net.step().expect("fires"), Some(right));
    assert_eq!(net.step().expect("fires"), Some(right));
    net.clear_selector();
    assert_eq!(net.step().expect("fires"), Some(middle), "still after Left");

    // Another tie-break keeps the cursor moving, and only round robin reads
    // it.
    net.set_tie_break(TieBreak::FirstDeclared);
    assert_eq!(net.step().expect("fires"), Some(left));
    net.set_tie_break(TieBreak::RoundRobin);
    assert_eq!(
        net.select(),
        Some(middle),
        "after Left, the last choice taken"
    );
}

#[test]
fn every_net_has_its_own_cursor() {
    // An inner net of rivals inside a quiet outer net, stepped deep: its
    // turns are its own.
    let (inner, [left, middle, right]) = rivals(3);
    let mut b = NetBuilder::new();
    let desk = b.place_nested::<Net>("Desk");
    b.seed(desk, [inner]);
    b.tie_break(TieBreak::RoundRobin);
    let mut net = b.build().expect("one inner net");
    let order: Vec<TransitionId> = std::iter::from_fn(|| net.step_deep().expect("no failure"))
        .map(|step| step.transition())
        .collect();
    assert_eq!(order, [left, middle, right]);
}

#[cfg(feature = "serde")]
mod saved {
    use petritype::serde_json;
    use petritype::{DeepPolicy, NetBuilder, Params, TakeEdge, Templates, TieBreak};

    /// `Rivals`: `Left` and `Right` compete for `Pool`, which holds 0 to 5,
    /// under round robin and the interleaved deep policy.
    fn templates() -> Templates {
        let mut templates = Templates::new();
        templates.register("Rivals", |_: &Params| {
            let mut b = NetBuilder::new();
            let pool = b.place::<u32>("Pool");
            let out = b.place::<u32>("Out");
            b.serializable(pool);
            b.serializable(out);
            b.seed(pool, 0..6);
            b.transition("Left", (TakeEdge(pool),), (out,), |n: u32| (n,));
            b.transition("Right", (TakeEdge(pool),), (out,), |n: u32| (n,));
            b.tie_break(TieBreak::RoundRobin);
            b.deep_policy(DeepPolicy::Interleaved);
            b.build()
        });
        templates
    }

    #[test]
    fn a_restored_a_forked_or_a_rebuilt_net_starts_its_round_afresh() {
        let templates = templates();
        let mut net = templates.build("Rivals", &Params::new()).expect("built");
        let left = net.transition_named("Left").expect("Left");
        let right = net.transition_named("Right").expect("Right");
        assert_eq!(net.step().expect("fires"), Some(left));
        assert_eq!(net.select(), Some(right), "the cursor is on Left");

        let saved = serde_json::to_value(&net).expect("a stamped net saves");
        let restored = templates.restore(saved).expect("and restores");
        assert_eq!(restored.select(), Some(left), "the cursor is not saved");
        assert_eq!(restored.tie_break(), TieBreak::RoundRobin, "the factory's");

        let fork = net.fork().expect("a fork");
        assert_eq!(fork.select(), Some(left), "nor forked");
        assert_eq!(
            (fork.tie_break(), fork.deep_policy()),
            (TieBreak::RoundRobin, DeepPolicy::Interleaved),
            "the settings are"
        );

        // A reset is a rebuild, and a rebuilt net has no cursor either.
        let rebuilt = templates.build("Rivals", &Params::new()).expect("built");
        assert_eq!(rebuilt.select(), Some(left));
    }
}

// ----- the deep policy -----------------------------------------------------------

/// A bare inner net with `jobs` things to do: `Todo -Work-> Done`.
fn worker(jobs: u32) -> Net {
    let mut b = NetBuilder::new();
    let todo = b.place::<u32>("Todo");
    let done = b.place::<u32>("Done");
    b.seed(todo, 0..jobs);
    b.transition("Work", (TakeEdge(todo),), (done,), |n: u32| (n,));
    b.build().expect("a worker")
}

/// How much an inner net has left to do.
fn todo(net: &Net) -> usize {
    net.count(net.place_named("Todo").expect("a worker has a Todo"))
}

/// An outer net that can always fire (`Spin` puts back what it takes) and
/// a `Desk` of workers, plus a passive `Shelf` holding a worker no deep
/// step may touch.
struct Carousel {
    net: Net,
    desk: PlaceRef<Net>,
    spin: TransitionId,
}

fn carousel(policy: DeepPolicy, workers: &[u32]) -> Carousel {
    let mut b = NetBuilder::new();
    let wheel = b.place::<u32>("Wheel");
    let desk = b.place_nested::<Net>("Desk");
    let shelf = b.place_nested::<Net>("Shelf");
    b.passive(shelf);
    b.seed(wheel, [0]);
    b.seed(desk, workers.iter().map(|&jobs| worker(jobs)));
    b.seed(shelf, [worker(5)]);
    let spin = b
        .transition("Spin", (TakeEdge(wheel),), (wheel,), |n: u32| (n + 1,))
        .id();
    b.deep_policy(policy);
    Carousel {
        net: b.build().expect("a carousel"),
        desk,
        spin,
    }
}

/// The nested ids of the workers on the desk, in token order.
fn desk_ids(c: &Carousel) -> Vec<NetId> {
    let tokens: Vec<_> = c.net.tokens(c.desk.id()).map(|(token, _)| token).collect();
    tokens
        .into_iter()
        .map(|token| {
            c.net.nested(c.desk.id(), token).expect("a worker")[0]
                .1
                .id()
        })
        .collect()
}

/// What each work left on the desk, in token order.
fn desk_todo(c: &Carousel) -> Vec<usize> {
    let tokens: Vec<_> = c.net.tokens(c.desk.id()).map(|(token, _)| token).collect();
    tokens
        .into_iter()
        .map(|token| todo(c.net.nested(c.desk.id(), token).expect("a worker")[0].1))
        .collect()
}

/// Where a deep step fired: `None` for the outer net, else the nested id of
/// the innermost net.
fn fired_in(step: &DeepStep) -> Option<NetId> {
    match step {
        DeepStep::Here { .. } => None,
        DeepStep::Inside {
            nested_id, step, ..
        } => Some(fired_in(step).unwrap_or(*nested_id)),
    }
}

/// Where each of `n` deep steps fired.
fn deep_order(net: &mut Net, n: usize) -> Vec<Option<NetId>> {
    (0..n)
        .map(|_| {
            fired_in(
                &net.step_deep()
                    .expect("no failure")
                    .expect("something fires"),
            )
        })
        .collect()
}

#[test]
fn an_always_live_outer_net_starves_its_inner_net_under_outer_first() {
    let mut c = carousel(DeepPolicy::default(), &[2]);
    assert_eq!(c.net.deep_policy(), DeepPolicy::OuterFirst, "the default");
    assert_eq!(deep_order(&mut c.net, 10), [None; 10], "Spin, every time");
    assert_eq!(desk_todo(&c), [2], "the worker never had a turn");
}

#[test]
fn interleaved_gives_the_inner_net_its_turn() {
    let mut c = carousel(DeepPolicy::Interleaved, &[2]);
    let [worker] = desk_ids(&c)[..] else {
        panic!("one worker")
    };
    assert_eq!(
        deep_order(&mut c.net, 6),
        [None, Some(worker), None, Some(worker), None, None],
        "the outer net, the worker, round again; then the outer net alone"
    );
    assert_eq!(desk_todo(&c), [0]);
    let shelf = c.net.place_named("Shelf").expect("a shelf");
    let (token, _) = c.net.tokens(shelf).next().expect("a shelved worker");
    assert_eq!(
        todo(c.net.nested(shelf, token).expect("a worker")[0].1),
        5,
        "a passive place is not in the round"
    );

    // Set on the net, it takes effect from the next deep step.
    let mut c = carousel(DeepPolicy::OuterFirst, &[1]);
    assert_eq!(deep_order(&mut c.net, 2), [None, None]);
    c.net.set_deep_policy(DeepPolicy::Interleaved);
    let [worker] = desk_ids(&c)[..] else {
        panic!("one worker")
    };
    assert_eq!(deep_order(&mut c.net, 2), [Some(worker), None]);
}

#[test]
fn two_live_inner_nets_alternate_under_interleaved() {
    // No outer transition can fire: `Spin`'s wheel is empty.
    let quiet = |policy| {
        let mut c = carousel(policy, &[3, 3]);
        let wheel = c.net.place_named("Wheel").expect("a wheel");
        let (token, _) = c.net.tokens(wheel).next().expect("the wheel's token");
        c.net.remove(wheel, token).expect("removable");
        c
    };
    let mut c = quiet(DeepPolicy::Interleaved);
    let [a, b] = desk_ids(&c)[..] else {
        panic!("two workers")
    };
    assert_eq!(
        deep_order(&mut c.net, 6),
        [a, b, a, b, a, b].map(Some),
        "each in turn"
    );
    assert!(c.net.step_deep().expect("no failure").is_none());

    let mut c = quiet(DeepPolicy::OuterFirst);
    let [a, b] = desk_ids(&c)[..] else {
        panic!("two workers")
    };
    assert_eq!(
        deep_order(&mut c.net, 6),
        [a, a, a, b, b, b].map(Some),
        "the first until it is quiet"
    );
}

#[test]
fn the_round_starts_at_the_outer_net_when_its_cursor_has_gone() {
    let mut c = carousel(DeepPolicy::Interleaved, &[3, 3]);
    let [a, b] = desk_ids(&c)[..] else {
        panic!("two workers")
    };
    assert_eq!(deep_order(&mut c.net, 2), [None, Some(a)]);
    // `a` fired last; take its token away, and the round has nowhere to
    // resume but the start.
    let (token, _) = c.net.tokens(c.desk.id()).next().expect("a's token");
    c.net.remove(c.desk.id(), token).expect("removable");
    assert_eq!(deep_order(&mut c.net, 3), [None, Some(b), None]);
}

/// A token holding a net that holds a token holding a net: depth two.
#[derive(Debug)]
struct Crate {
    middle: Net,
}

impl Nested for Crate {
    fn nets(&self) -> Vec<(&str, &Net)> {
        vec![("middle", &self.middle)]
    }
    fn nets_mut(&mut self) -> Vec<(&str, &mut Net)> {
        vec![("middle", &mut self.middle)]
    }
}

#[test]
fn the_round_goes_down_before_it_goes_on() {
    let nested = |policy| {
        // The middle net works through two jobs of its own and holds a
        // worker with two more.
        let mut b = NetBuilder::new();
        let todo = b.place::<u32>("Todo");
        let done = b.place::<u32>("Done");
        let held = b.place_nested::<Net>("Held");
        b.seed(todo, 0..2);
        b.seed(held, [worker(2)]);
        b.transition("Work", (TakeEdge(todo),), (done,), |n: u32| (n,));
        let middle = b.build().expect("the middle net");
        let mut b = NetBuilder::new();
        let yard = b.place_nested::<Crate>("Yard");
        b.seed(yard, [Crate { middle }]);
        b.deep_policy(policy);
        let net = b.build().expect("the outer net");
        let (token, _) = net.tokens(yard.id()).next().expect("the crate");
        let ids = {
            let nets = net.nested(yard.id(), token).expect("a crate");
            let middle = nets[0].1;
            let held = middle.place_named("Held").expect("Held");
            let (inner, _) = middle.tokens(held).next().expect("the worker");
            (
                middle.id(),
                middle.nested(held, inner).expect("a worker")[0].1.id(),
            )
        };
        (net, ids)
    };
    let (mut net, (middle, bottom)) = nested(DeepPolicy::Interleaved);
    assert_eq!(
        deep_order(&mut net, 4),
        [middle, bottom, middle, bottom].map(Some),
        "the cycle is the outer net, the middle net, the bottom net"
    );
    let (mut net, (middle, bottom)) = nested(DeepPolicy::OuterFirst);
    assert_eq!(
        deep_order(&mut net, 4),
        [middle, middle, bottom, bottom].map(Some),
        "the middle net's own selector first, under the outer net's policy"
    );
}

#[test]
fn a_staged_deep_step_hands_back_the_outer_choice_in_its_turn() {
    let mut c = carousel(DeepPolicy::Interleaved, &[1]);
    let wheel = c.net.place_named("Wheel").expect("a wheel");

    let Some(StagedStep::Chosen(chosen)) = c.net.step_deep_staged().expect("no failure") else {
        panic!("the outer net's turn comes first");
    };
    assert_eq!(chosen, c.spin);
    assert_eq!(c.net.count(wheel), 1, "chosen, not fired");
    let firing = c.net.launch(chosen).expect("Spin is fireable");
    c.net.distribute(firing.run()).expect("it lands");

    let step = c.net.step_deep_staged().expect("no failure");
    assert!(
        matches!(step, Some(StagedStep::Inside(DeepStep::Inside { .. }))),
        "then the worker's, fired inline: {step:?}"
    );
    assert_eq!(desk_todo(&c), [0]);
    assert_eq!(
        c.net.step_deep_staged().expect("no failure"),
        Some(StagedStep::Chosen(c.spin))
    );
}

#[test]
fn the_sequential_drivers_honour_the_policy() {
    for (policy, left) in [(DeepPolicy::OuterFirst, 2), (DeepPolicy::Interleaved, 0)] {
        let mut c = carousel(policy, &[2]);
        let limits = Limits {
            stop_after_n_firings: Some(10),
            ..Limits::default()
        };
        let summary = drivers::run(&mut c.net, Mode::Sequential, limits, OnFailure::Stop)
            .expect("nothing fails");
        assert_eq!(summary.fired_count, 10, "{policy:?}");
        assert_eq!(desk_todo(&c), [left], "{policy:?}");
    }

    // The fuse blames what the round would have tried next.
    for (policy, blamed) in [
        (DeepPolicy::OuterFirst, "Spin"),
        (DeepPolicy::Interleaved, "Work"),
    ] {
        let mut c = carousel(policy, &[2]);
        let limits = Limits {
            error_after_n_firings: Some(1),
            ..Limits::default()
        };
        match drivers::run(&mut c.net, Mode::Sequential, limits, OnFailure::Stop) {
            Err(DriverError::TooManyFirings { name, .. }) => assert_eq!(name, blamed),
            other => panic!("{policy:?}: the fuse, not {other:?}"),
        }
    }
}

#[test]
fn the_concurrent_drivers_honour_the_policy() {
    for (policy, left) in [(DeepPolicy::OuterFirst, 3), (DeepPolicy::Interleaved, 0)] {
        let mut c = carousel(policy, &[3]);
        let limits = Limits {
            stop_after_n_firings: Some(40),
            ..Limits::default()
        };
        let summary = drivers::run(&mut c.net, Mode::Concurrent, limits, OnFailure::Stop)
            .expect("nothing fails");
        assert!(!summary.quiesced, "{policy:?}: Spin never stops");
        assert!(c.net.in_flight().is_empty());
        assert_eq!(
            desk_todo(&c),
            [left],
            "{policy:?}: outer bodies airborne or not, the worker has its turns"
        );
    }

    // On a clock, too: the worker finishes while `Spin` keeps launching.
    let mut c = carousel(DeepPolicy::Interleaved, &[3]);
    let desk = c.desk.id();
    let stop = AtomicBool::new(false);
    let mut ticks = 0;
    let summary = drivers::run_indefinitely(
        &mut c.net,
        Mode::Concurrent,
        Duration::from_millis(1),
        &stop,
        OnFailure::Stop,
        |net| {
            ticks += 1;
            let (token, _) = net.tokens(desk).next().expect("the worker");
            let left = todo(net.nested(desk, token).expect("a worker")[0].1);
            if left == 0 || ticks > 500 {
                stop.store(true, Ordering::Relaxed);
            }
        },
    )
    .expect("nothing fails");
    assert!(summary.fired_count >= 3);
    assert_eq!(desk_todo(&c), [0], "done in {ticks} ticks");
}

#[test]
fn a_failed_firing_moves_the_interleaved_cursor_too() {
    // An inner net whose one transition fails and puts its token back, so
    // that it could fail for ever, beside an outer net that can always fire.
    let mut inner = NetBuilder::new();
    let todo = inner.place::<u32>("Todo");
    let done = inner.place::<u32>("Done");
    inner.seed(todo, [1]);
    inner
        .transition(
            "Fail",
            (TakeEdge(todo),),
            (done,),
            |_: u32| -> Result<(u32,), std::fmt::Error> { Err(std::fmt::Error) },
        )
        .restore_tokens_on_failure();
    let inner = inner.build().expect("a failing worker");
    let mut b = NetBuilder::new();
    let wheel = b.place::<u32>("Wheel");
    let desk = b.place_nested::<Net>("Desk");
    b.seed(wheel, [0]);
    b.seed(desk, [inner]);
    b.transition("Spin", (TakeEdge(wheel),), (wheel,), |n: u32| (n + 1,));
    b.deep_policy(DeepPolicy::Interleaved);
    let mut net = b.build().expect("a carousel with a failing worker");

    let outcomes: Vec<&str> = (0..4)
        .map(|_| match net.step_deep() {
            Ok(Some(DeepStep::Here { .. })) => "outer",
            Err(failed) if !failed.path.is_empty() => "inner failed",
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        outcomes,
        ["outer", "inner failed", "outer", "inner failed"],
        "the failure was the inner net's turn, and the next is the outer net's"
    );
}
