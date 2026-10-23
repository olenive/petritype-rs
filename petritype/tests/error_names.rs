//! Integration tests for one cross-cutting decision: a runtime error about a
//! thing that *exists* names it as well as numbering it.
//!
//! An error escapes the scope the net lives in. A `?` in `main` prints the
//! message and exits the process, and by then `PlaceId(3)` is useless to the
//! person reading the log: the net that could have turned it into a name is
//! gone. So the name goes in the `Display` message and the id stays as the
//! field a program matches on, and these tests are the message half of that
//! bargain: one per variant that gained a name, asserting the quoted name
//! *and* the id.
//!
//! Three of the variants are not reachable from out here and are tested
//! beside the code that raises them: [`NetError::NoClonePlace`] in `slice11`,
//! and `NetError::NotInFlight` and `DistributionError::WrongType` in the
//! crate's own `firing` tests, both of which need a handle or a `Completed`
//! only the crate can mint.
//!
//! A not-found error ([`NetError::NoSuchPlace`],
//! [`NetError::NoSuchTransition`], [`NetError::ForeignFiring`]) carries its
//! id alone, and is not tested here: there is no name to carry.

use std::any::Any;

use petritype::drivers::{self, Limits, Mode, OnFailure};
use petritype::{
    Command, CommandError, Distribution, DistributionError, NetBuilder, NetError, TakeEdge,
};

/// The error a failing body returns, so that the failure has something to
/// say for itself.
#[derive(Debug)]
struct Broken;

impl std::fmt::Display for Broken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the scales are broken")
    }
}

impl std::error::Error for Broken {}

#[test]
fn a_transition_that_cannot_fire_is_named() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    let double = b
        .transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,))
        .id();
    let mut net = b.build().expect("a one-transition net");

    let error = net.fire(double).expect_err("Inbox is empty");
    assert!(
        matches!(error, NetError::NotFireable { transition, .. } if transition == double),
        "{error:?}",
    );
    assert_eq!(
        error.to_string(),
        format!("transition \"Double\" ({double:?}) is not fireable"),
    );
}

#[test]
fn a_place_that_holds_no_such_token_is_named() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let mut net = b.build().expect("a net with one place");

    // An id retires with its token, so removing the same token twice is the
    // shortest way to an id this place has no token for.
    let token = net.put(inbox, 1);
    net.remove(inbox.id(), token).expect("the token is there");

    let error = net
        .remove(inbox.id(), token)
        .expect_err("and now it is not");
    assert!(
        matches!(error, NetError::NoSuchToken { place, .. } if place == inbox.id()),
        "{error:?}",
    );
    assert_eq!(
        error.to_string(),
        format!(
            "place \"Inbox\" ({:?}) holds no token {token:?}",
            inbox.id()
        ),
    );
}

#[test]
fn a_place_offered_the_wrong_type_is_named() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let mut net = b.build().expect("a net with one place");
    let token = net.put(inbox, 1);

    let value: Box<dyn Any + Send + Sync> = Box::new("nope");
    let error = net
        .apply(Command::Replace {
            place: inbox.id(),
            token,
            value,
        })
        .expect_err("Inbox holds u32");
    assert!(
        matches!(
            error,
            CommandError::Net(NetError::TypeMismatch { place, .. }) if place == inbox.id()
        ),
        "{error:?}",
    );
    assert_eq!(
        error.to_string(),
        format!("place \"Inbox\" ({:?}) holds u32, not <erased>", inbox.id()),
    );
}

#[test]
fn a_failed_firing_names_its_transition() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [1]);
    let weigh = b
        .transition(
            "Weigh",
            (TakeEdge(inbox),),
            (out,),
            |_n: u32| -> Result<(u32,), Broken> { Err(Broken) },
        )
        .id();
    let mut net = b.build().expect("a one-transition net");

    let failure = net.step().expect_err("the body returns Err");
    assert_eq!(failure.transition, weigh);
    assert_eq!(
        failure.to_string(),
        format!("transition \"Weigh\" ({weigh:?}) failed: the scales are broken"),
    );
}

#[test]
fn a_spent_fuse_names_the_transition_it_refused() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [1]);
    let double = b
        .transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,))
        .id();
    let mut net = b.build().expect("a one-transition net");

    let limits = Limits {
        error_after_n_firings: Some(0),
        ..Limits::default()
    };
    let error = drivers::run(&mut net, Mode::Sequential, limits, OnFailure::Stop)
        .expect_err("a spent fuse and a fireable transition");
    assert_eq!(
        error.to_string(),
        format!(
            "the fuse of 0 firings is spent, and transition \"Double\" ({double:?}) \
             attempted another"
        ),
    );
}

#[test]
fn a_distribution_outside_the_declaration_names_both_ends() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let uploads = b.place::<u32>("Uploads");
    let elsewhere = b.place::<u32>("Elsewhere");
    b.seed(inbox, [1]);
    let weigh = b
        .transition("Weigh", (TakeEdge(inbox),), (uploads,), move |n: u32| {
            Distribution::new().to(elsewhere, n)
        })
        .id();
    let mut net = b.build().expect("a distribution needs no routing table");

    let failure = net.step().expect_err("the pair names a place it may not");
    let refused = failure
        .error
        .downcast_ref::<DistributionError>()
        .expect("the firing failed on its distribution");
    assert!(
        matches!(
            refused,
            DistributionError::NotAnOutput { transition, place, .. }
                if *transition == weigh && *place == elsewhere.id()
        ),
        "{refused:?}",
    );
    assert_eq!(
        refused.to_string(),
        format!(
            "transition \"Weigh\" ({weigh:?}) distributed a token to place \"Elsewhere\" \
             ({:?}), which is not one of its outputs",
            elsewhere.id(),
        ),
    );
}

#[cfg(feature = "serde")]
mod json {
    use petritype::{CommandError, JsonCommand, NetBuilder};
    use serde_json::Value;

    #[test]
    fn a_place_with_no_codec_is_named() {
        let mut b = NetBuilder::new();
        let notes = b.place::<String>("Notes");
        let mut net = b.build().expect("a net with one place");

        let error = net
            .apply_json(JsonCommand::Extend {
                place: String::from("Notes"),
                tokens: vec![Value::from("hello")],
            })
            .expect_err("Notes was never marked serializable");
        assert!(
            matches!(error, CommandError::NotSerializable { place, .. } if place == notes.id()),
            "{error:?}",
        );
        assert_eq!(
            error.to_string(),
            format!(
                "place \"Notes\" ({:?}) is not marked serializable",
                notes.id()
            ),
        );
    }

    #[test]
    fn a_token_that_does_not_parse_names_its_place() {
        let mut b = NetBuilder::new();
        let counts = b.place::<u32>("Counts");
        b.serializable(counts);
        let mut net = b.build().expect("a net with one place");

        let error = net
            .apply_json(JsonCommand::Extend {
                place: String::from("Counts"),
                tokens: vec![Value::from("seven")],
            })
            .expect_err("\"seven\" is not a u32");
        assert!(
            matches!(error, CommandError::InvalidToken { place, .. } if place == counts.id()),
            "{error:?}",
        );
        let message = error.to_string();
        assert!(
            message.starts_with(&format!(
                "place \"Counts\" ({:?}): token does not parse as u32: ",
                counts.id()
            )),
            "{message}",
        );
    }
}
