//! Integration tests for the seventh slice: what a firing does when it goes
//! wrong.
//!
//! Three properties, in the order they arrive. A body that panics fails its
//! firing instead of unwinding out of it, wherever stage two happened to run.
//! A failed firing is reported, once, as an `Event::Failed` naming the tokens
//! it consumed. And those tokens come back — front of their places, same ids,
//! same order — exactly for a transition declared with
//! `restore_tokens_on_failure`, which is what makes a failed firing undone
//! rather than merely recorded.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use petritype::{
    BodyPanicked, BuildError, Distribution, DistributionError, Event, MutateEdge, Net, NetBuilder,
    NetError, PlaceId, SnapshotEdge, TakeAllEdge, TakeEdge, TokenId,
};

/// A body's failure, erased into [`petritype::FiringFailed`] by the engine.
#[derive(Debug)]
struct Refused;

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("refused")
    }
}

impl std::error::Error for Refused {}

/// A token that cannot be cloned, for the places that give the ability up.
#[derive(Debug)]
struct Handle(u32);

/// A token that counts its own clones, so a test can say exactly how many
/// were taken and on whose behalf.
#[derive(Debug)]
struct Counted {
    value: u32,
    clones: Arc<AtomicUsize>,
}

impl Clone for Counted {
    fn clone(&self) -> Counted {
        self.clones.fetch_add(1, Ordering::SeqCst);
        Counted {
            value: self.value,
            clones: Arc::clone(&self.clones),
        }
    }
}

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

/// The values of a place of [`Counted`] tokens, read without cloning one.
fn counted(net: &Net, place: PlaceId) -> Vec<u32> {
    net.tokens(place)
        .map(|(_, value)| {
            value
                .downcast_ref::<Counted>()
                .expect("the place holds Counted")
                .value
        })
        .collect()
}

/// Every token of a place as the `(place, token)` pairs an event carries —
/// which is also the whole of what "the same tokens, in the same order" means
/// to a restored place.
fn pairs(net: &Net, place: PlaceId) -> Vec<(PlaceId, TokenId)> {
    net.tokens(place).map(|(id, _)| (place, id)).collect()
}

// ----- a panicking body ----------------------------------------------------

#[test]
fn a_panicking_body_fails_its_firing_and_comes_home() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [1]);
    let boom = b
        .transition("Boom", (TakeEdge(inbox),), (out,), |n: u32| {
            if n == 1 {
                panic!("boom");
            }
            (n,)
        })
        .id();
    let mut net = b.build().unwrap();
    let consumed = pairs(&net, inbox.id());

    let failure = net.step().expect_err("the body panicked");

    assert_eq!(failure.transition, boom);
    assert!(
        !failure.restored,
        "this transition does not restore its inputs"
    );
    assert_eq!(failure.error.to_string(), "panicked: boom");
    assert_eq!(
        failure
            .error
            .downcast_ref::<BodyPanicked>()
            .expect("a caught panic, not some other failure")
            .message,
        "boom",
        "the payload, without the prefix the Display adds",
    );
    assert!(failure.to_string().ends_with("failed: panicked: boom"));

    assert_eq!(net.count(inbox.id()), 0, "the consumed token is gone");
    assert_eq!(net.count(out.id()), 0, "nothing was distributed");
    assert_eq!(net.fired_count(), 0, "a failure is not a firing");
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Failed {
            transition: boom,
            consumed,
            restored: false,
            error: String::from("panicked: boom"),
        }],
    );

    // The body came home despite the panic, so the next token fires it.
    assert!(!net.is_in_flight(boom), "not stranded in flight");
    net.put(inbox, 2);
    assert_eq!(net.step().expect("the second token is fine"), Some(boom));
    assert_eq!(values::<u32>(&net, out.id()), [2]);
    assert_eq!(net.fired_count(), 1);
}

#[test]
fn an_inline_body_that_panics_after_mutating_keeps_the_mutation() {
    let mut b = NetBuilder::new();
    let ticks = b.place::<u32>("Ticks");
    let live = b.place::<u32>("Live");
    b.seed(ticks, [5]);
    b.seed(live, [10]);
    let bump = b
        .transition(
            "Bump",
            (TakeEdge(ticks), MutateEdge(live)),
            (),
            |tick: u32, live: &mut u32| {
                *live += tick;
                if *live > 0 {
                    panic!("boom");
                }
            },
        )
        .id();
    let mut net = b.build().unwrap();
    let mutated = pairs(&net, live.id());
    let consumed = pairs(&net, ticks.id());

    // The body runs inside `launch`, so the catch has to be there too.
    let failure = net.step().expect_err("the body panicked");

    assert_eq!(failure.transition, bump);
    assert_eq!(failure.error.to_string(), "panicked: boom");
    assert_eq!(
        values::<u32>(&net, live.id()),
        [15],
        "the mutation was made where the token lies, so it stands",
    );
    assert_eq!(pairs(&net, live.id()), mutated, "same token, same id");
    assert_eq!(net.count(ticks.id()), 0, "the consumed token is gone");
    assert_eq!(net.fired_count(), 0);
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [
            Event::Mutated {
                place: live.id(),
                token: mutated[0].1,
            },
            Event::Failed {
                transition: bump,
                consumed,
                restored: false,
                error: String::from("panicked: boom"),
            },
        ],
        "the mutations, then the event that ends the firing",
    );
    assert!(!net.is_in_flight(bump));
}

#[test]
fn a_panic_on_a_worker_thread_kills_neither_the_thread_nor_the_transition() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [1]);
    let boom = b
        .transition("Boom", (TakeEdge(inbox),), (out,), |n: u32| -> (u32,) {
            panic!("boom {n}");
        })
        .id();
    let mut net = b.build().unwrap();
    let consumed = pairs(&net, inbox.id());

    let firing = net.launch(boom).expect("fireable");
    let completed = std::thread::scope(|scope| {
        scope
            .spawn(move || firing.run())
            .join()
            .expect("the worker survived its body's panic")
    });

    assert!(!completed.succeeded(), "the panic became a failure");
    assert_eq!(completed.transition(), boom);
    assert_eq!(net.in_flight(), [boom], "still airborne until distributed");
    net.drain_events().for_each(drop);

    match net.distribute(completed) {
        Err(NetError::FiringFailed(failure)) => {
            assert_eq!(failure.transition, boom);
            assert_eq!(failure.error.to_string(), "panicked: boom 1");
            assert!(failure.error.downcast_ref::<BodyPanicked>().is_some());
        }
        other => panic!("expected a wrapped firing failure, got {other:?}"),
    }

    assert!(net.in_flight().is_empty(), "the body came home");
    assert_eq!(net.fired_count(), 0);
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Failed {
            transition: boom,
            consumed,
            restored: false,
            error: String::from("panicked: boom 1"),
        }],
        "Failed and not Distributed",
    );
}

#[test]
fn a_formatted_panic_message_reaches_the_failure() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [7]);
    // A formatted message is a `String` payload, where a literal one is a
    // `&'static str`: both have text to read.
    b.transition("Boom", (TakeEdge(inbox),), (out,), |n: u32| -> (u32,) {
        panic!("boom {n}");
    });
    let mut net = b.build().unwrap();

    let failure = net.step().expect_err("the body panicked");

    assert_eq!(
        failure
            .error
            .downcast_ref::<BodyPanicked>()
            .expect("a caught panic")
            .message,
        "boom 7",
    );
}

#[test]
fn a_panic_payload_that_is_not_a_string_still_makes_a_message() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [1]);
    b.transition("Boom", (TakeEdge(inbox),), (out,), |_n: u32| -> (u32,) {
        std::panic::panic_any(42_u32);
    });
    let mut net = b.build().unwrap();

    let failure = net.step().expect_err("the body panicked");

    assert_eq!(
        failure
            .error
            .downcast_ref::<BodyPanicked>()
            .expect("a caught panic")
            .message,
        "panic payload of unknown type",
        "an erased payload has no text, and Any cannot name its type either",
    );
    assert_eq!(
        failure.error.to_string(),
        "panicked: panic payload of unknown type",
    );
}

// ----- restoring the consumed tokens ---------------------------------------

#[test]
fn restoring_puts_every_consumed_token_back_in_front_with_its_id() {
    let mut b = NetBuilder::new();
    let jobs = b.place::<u32>("Jobs");
    let batch = b.place::<u32>("Batch");
    let out = b.place::<u32>("Out");
    b.seed(jobs, [1, 2]);
    b.seed(batch, [10, 20, 30]);
    let work = b
        .transition(
            "Work",
            (TakeEdge(jobs), TakeAllEdge(batch)),
            (out,),
            |job: u32, batch: Vec<u32>| -> Result<(u32,), Refused> {
                let _ = (job, batch);
                Err(Refused)
            },
        )
        .restore_tokens_on_failure()
        .id();
    let mut net = b.build().unwrap();
    let before_jobs = pairs(&net, jobs.id());
    let before_batch = pairs(&net, batch.id());
    // One edge takes the front token, the other takes the lot.
    let consumed: Vec<(PlaceId, TokenId)> = [&before_jobs[..1], &before_batch[..]].concat();

    let failure = net.step().expect_err("the body refuses");

    assert_eq!(failure.transition, work);
    assert!(failure.restored, "this transition restores its inputs");
    assert_eq!(failure.error.to_string(), "refused");

    assert_eq!(
        pairs(&net, jobs.id()),
        before_jobs,
        "the taken token is back in front of the one it was taken before",
    );
    assert_eq!(
        pairs(&net, batch.id()),
        before_batch,
        "the whole batch, in order"
    );
    assert_eq!(
        values::<u32>(&net, jobs.id()),
        [1, 2],
        "and they are the same values"
    );
    assert_eq!(values::<u32>(&net, batch.id()), [10, 20, 30]);
    assert_eq!(net.count(out.id()), 0, "nothing was distributed");
    assert_eq!(net.fired_count(), 0, "a failure is not a firing");
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Failed {
            transition: work,
            consumed: consumed.clone(),
            restored: true,
            error: String::from("refused"),
        }],
        "the tokens are named as consumed, and `restored` says where they are",
    );

    // Undone, not recorded: the net is exactly where it was, so the same
    // firing happens again and fails again.
    let failure = net.step().expect_err("and again, with the same tokens");
    assert!(failure.restored);
    assert_eq!(pairs(&net, jobs.id()), before_jobs);
    assert_eq!(pairs(&net, batch.id()), before_batch);
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Failed {
            transition: work,
            consumed,
            restored: true,
            error: String::from("refused"),
        }],
    );
}

#[test]
fn restoring_undoes_a_firing_whose_body_panicked() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [1, 2]);
    let boom = b
        .transition("Boom", (TakeEdge(inbox),), (out,), |_n: u32| -> (u32,) {
            panic!("boom");
        })
        .restore_tokens_on_failure()
        .id();
    let mut net = b.build().unwrap();
    let before = pairs(&net, inbox.id());

    let failure = net.step().expect_err("the body panicked");

    assert!(
        failure.restored,
        "a panic is a failure like any other, so the flag applies to it too",
    );
    assert!(failure.error.downcast_ref::<BodyPanicked>().is_some());
    assert_eq!(
        pairs(&net, inbox.id()),
        before,
        "both tokens, ids and order"
    );
    assert_eq!(values::<u32>(&net, inbox.id()), [1, 2]);
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Failed {
            transition: boom,
            consumed: before[..1].to_vec(),
            restored: true,
            error: String::from("panicked: boom"),
        }],
    );
}

#[test]
fn restoring_undoes_a_firing_that_named_a_place_it_may_not_write_to() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    // Declared in the net, but not an output of the transition below.
    let elsewhere = b.place::<u32>("Elsewhere");
    b.seed(inbox, [1, 2]);
    let make = b
        .transition("Make", (TakeEdge(inbox),), (out,), move |n: u32| {
            Distribution::new().to(elsewhere, n)
        })
        .restore_tokens_on_failure()
        .id();
    let mut net = b.build().expect("a distribution needs no routing table");
    let before = pairs(&net, inbox.id());

    let failure = net.step().expect_err("the pair names a place it may not");

    assert!(
        failure.restored,
        "the engine's own refusal fails the firing, and a failed firing restores",
    );
    assert!(matches!(
        failure.error.downcast_ref::<DistributionError>(),
        Some(DistributionError::NotAnOutput { transition, place, .. })
            if *transition == make && *place == elsewhere.id()
    ));
    assert_eq!(
        pairs(&net, inbox.id()),
        before,
        "the token is back, with its id"
    );
    assert_eq!(net.count(out.id()), 0, "and nothing was written anywhere");
    assert_eq!(net.count(elsewhere.id()), 0);
    assert!(matches!(
        net.drain_events().next(),
        Some(Event::Failed { restored: true, .. })
    ));
}

#[test]
fn a_snapshot_place_is_untouched_by_a_restore() {
    let mut b = NetBuilder::new();
    let jobs = b.place::<u32>("Jobs");
    let config = b.place::<u32>("Config");
    let out = b.place::<u32>("Out");
    b.seed(jobs, [1]);
    b.seed(config, [100]);
    let work = b
        .transition(
            "Work",
            (TakeEdge(jobs), SnapshotEdge(config)),
            (out,),
            |job: u32, limit: u32| -> Result<(u32,), Refused> {
                let _ = (job, limit);
                Err(Refused)
            },
        )
        .restore_tokens_on_failure()
        .id();
    let mut net = b.build().unwrap();
    let before_jobs = pairs(&net, jobs.id());
    let before_config = pairs(&net, config.id());

    let failure = net.step().expect_err("the body refuses");

    assert!(failure.restored);
    assert_eq!(
        pairs(&net, jobs.id()),
        before_jobs,
        "the consumed token is back"
    );
    assert_eq!(
        pairs(&net, config.id()),
        before_config,
        "the snapshot place never lost a token, so it gains nothing either",
    );
    assert_eq!(values::<u32>(&net, config.id()), [100]);
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Failed {
            transition: work,
            consumed: before_jobs.clone(),
            restored: true,
            error: String::from("refused"),
        }],
        "a snapshot consumes nothing, so it is not in the list either",
    );

    // And it stays untouched however often the firing is retried: the
    // snapshot is a clone that was never in a place to begin with.
    net.step().expect_err("the same firing, the same refusal");
    assert_eq!(pairs(&net, config.id()), before_config);
    assert_eq!(
        net.count(config.id()),
        1,
        "one snapshot per firing, no copies left behind"
    );
}

#[test]
fn a_firing_that_stands_drops_its_clones_and_reports_nothing_extra() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [21]);
    let double = b
        .transition(
            "Double",
            (TakeEdge(inbox),),
            (out,),
            |n: u32| -> Result<(u32,), Refused> { Ok((n * 2,)) },
        )
        .restore_tokens_on_failure()
        .id();
    let mut net = b.build().unwrap();
    let consumed = pairs(&net, inbox.id());

    assert_eq!(net.step().expect("this body succeeds"), Some(double));

    assert_eq!(net.count(inbox.id()), 0, "a successful firing consumes");
    assert_eq!(values::<u32>(&net, out.id()), [42]);
    assert_eq!(net.fired_count(), 1);
    let produced = pairs(&net, out.id());
    assert!(
        produced[0].1 > consumed[0].1,
        "the produced token is a new token with a fresh id, not a clone come home",
    );
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [Event::Fired {
            transition: double,
            consumed,
            produced,
        }],
        "one Fired, exactly as for a transition that does not restore",
    );
}

// ----- what restoring costs ------------------------------------------------

#[test]
fn restoring_clones_one_token_per_consumed_token_per_firing() {
    let clones = Arc::new(AtomicUsize::new(0));
    let mut b = NetBuilder::new();
    let jobs = b.place::<Counted>("Jobs");
    let batch = b.place::<Counted>("Batch");
    let out = b.place::<u32>("Out");
    let token = |value: u32| Counted {
        value,
        clones: Arc::clone(&clones),
    };
    b.seed(jobs, [token(1), token(2)]);
    b.seed(batch, [token(10), token(20), token(30)]);
    b.transition(
        "Work",
        (TakeEdge(jobs), TakeAllEdge(batch)),
        (out,),
        |job: Counted, batch: Vec<Counted>| -> Result<(u32,), Refused> {
            let _ = (job, batch);
            Err(Refused)
        },
    )
    .restore_tokens_on_failure();
    let mut net = b.build().unwrap();
    assert_eq!(clones.load(Ordering::SeqCst), 0, "seeding clones nothing");

    net.step().expect_err("the body refuses");

    assert_eq!(
        clones.load(Ordering::SeqCst),
        4,
        "one clone per consumed token: one taken, three taken all",
    );
    assert_eq!(
        counted(&net, jobs.id()),
        [1, 2],
        "and they are the values that were there"
    );
    assert_eq!(counted(&net, batch.id()), [10, 20, 30]);

    // The price is per firing, and it is paid before the body runs — so the
    // second firing pays it again on the tokens the first one put back.
    net.step().expect_err("and again");
    assert_eq!(clones.load(Ordering::SeqCst), 8);
}

#[test]
fn a_transition_that_does_not_restore_clones_nothing() {
    let clones = Arc::new(AtomicUsize::new(0));
    let mut b = NetBuilder::new();
    let jobs = b.place::<Counted>("Jobs");
    let out = b.place::<u32>("Out");
    b.seed(
        jobs,
        [Counted {
            value: 1,
            clones: Arc::clone(&clones),
        }],
    );
    b.transition(
        "Work",
        (TakeEdge(jobs),),
        (out,),
        |job: Counted| -> Result<(u32,), Refused> {
            let _ = job;
            Err(Refused)
        },
    );
    let mut net = b.build().unwrap();

    let failure = net.step().expect_err("the body refuses");

    assert!(!failure.restored);
    assert_eq!(
        clones.load(Ordering::SeqCst),
        0,
        "the flag is what costs a clone; without it the token simply moves",
    );
    assert_eq!(net.count(jobs.id()), 0, "and the token is gone");
}

// ----- what build refuses --------------------------------------------------

#[test]
fn build_refuses_restoring_a_transition_that_consumes_a_no_clone_place() {
    let mut b = NetBuilder::new();
    let files = b.place_no_clone::<Handle>("Files");
    let out = b.place::<u32>("Out");
    b.transition("Read", (TakeEdge(files),), (out,), |file: Handle| (file.0,))
        .restore_tokens_on_failure();

    let error = b
        .build()
        .expect_err("there is no cloner to take a clone with");

    assert!(matches!(
        error,
        BuildError::RestoreOfNoClonePlace { ref transition, ref place }
            if transition == "Read" && place == "Files"
    ));
    assert!(error.to_string().contains("Files"), "{error}");

    // A TakeAllEdge is the same question, and gets the same answer.
    let mut b = NetBuilder::new();
    let files = b.place_no_clone::<Handle>("Files");
    let out = b.place::<u32>("Out");
    b.transition(
        "Read all",
        (TakeAllEdge(files),),
        (out,),
        |files: Vec<Handle>| (files.len() as u32,),
    )
    .restore_tokens_on_failure();
    assert!(matches!(
        b.build().expect_err("a batch of uncloneable tokens is no better"),
        BuildError::RestoreOfNoClonePlace { ref transition, .. } if transition == "Read all"
    ));
}

#[test]
fn a_mutating_edge_from_a_no_clone_place_may_restore_all_the_same() {
    let mut b = NetBuilder::new();
    let files = b.place_no_clone::<Handle>("Files");
    let jobs = b.place::<u32>("Jobs");
    b.seed(files, [Handle(1)]);
    b.seed(jobs, [7]);
    let write = b
        .transition(
            "Write",
            (TakeEdge(jobs), MutateEdge(files)),
            (),
            |job: u32, file: &mut Handle| -> Result<(), Refused> {
                file.0 += job;
                Err(Refused)
            },
        )
        .restore_tokens_on_failure()
        .id();
    let mut net = b
        .build()
        .expect("a mutating edge consumes nothing, so there is nothing to clone");
    let before_jobs = pairs(&net, jobs.id());
    let file = pairs(&net, files.id());

    let failure = net.step().expect_err("the body refuses");

    assert!(failure.restored);
    assert_eq!(
        pairs(&net, jobs.id()),
        before_jobs,
        "the token the transition consumed is back",
    );
    assert_eq!(
        pairs(&net, files.id()),
        file,
        "and the uncloneable one never moved"
    );
    assert_eq!(
        net.token::<Handle>(files.id(), file[0].1)
            .expect("the file is where it was")
            .0,
        8,
        "its mutation stands, as it does for a transition that does not restore",
    );
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [
            Event::Mutated {
                place: files.id(),
                token: file[0].1,
            },
            Event::Failed {
                transition: write,
                consumed: before_jobs,
                restored: true,
                error: String::from("refused"),
            },
        ],
    );
}

// ----- through the stages --------------------------------------------------

#[test]
fn restoring_works_through_the_staged_path_too() {
    let mut b = NetBuilder::new();
    let jobs = b.place::<u32>("Jobs");
    let out = b.place::<u32>("Out");
    b.seed(jobs, [10, 20]);
    let work = b
        .transition(
            "Work",
            (TakeAllEdge(jobs),),
            (out,),
            |batch: Vec<u32>| -> Result<(u32,), Refused> {
                let _ = batch;
                Err(Refused)
            },
        )
        .restore_tokens_on_failure()
        .id();
    let mut net = b.build().unwrap();
    let consumed = pairs(&net, jobs.id());

    let firing = net.launch(work).expect("fireable");
    assert_eq!(net.count(jobs.id()), 0, "taken at launch, as always");

    // A token arrives while the firing is airborne, which is what tells the
    // front of the place from the back of it.
    let late = net.put(jobs, 30);

    let completed = firing.run();
    assert!(!completed.succeeded());
    match net.distribute(completed) {
        Err(NetError::FiringFailed(failure)) => assert!(failure.restored),
        other => panic!("expected a wrapped firing failure, got {other:?}"),
    }

    assert_eq!(
        pairs(&net, jobs.id()),
        [consumed.clone(), vec![(jobs.id(), late)]].concat(),
        "the restored tokens go in front of the one that arrived meanwhile",
    );
    assert_eq!(values::<u32>(&net, jobs.id()), [10, 20, 30]);
    assert_eq!(net.count(out.id()), 0);
    assert_eq!(net.fired_count(), 0);
    assert!(net.in_flight().is_empty(), "the body came home");
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [
            Event::Launched {
                transition: work,
                consumed: consumed.clone(),
            },
            Event::Added {
                place: jobs.id(),
                token: late,
            },
            Event::Failed {
                transition: work,
                consumed,
                restored: true,
                error: String::from("refused"),
            },
        ],
        "Launched, then whatever happened meanwhile, then Failed",
    );
}
