//! The event log: what changed, in order.

use crate::ids::{NetId, PlaceId, TokenId, TransitionId};

/// One change to the net, appended by the net and drained by the driver with
/// [`Net::drain_events`](crate::Net::drain_events).
///
/// The log is the cheap update channel for an observer: replaying the events
/// since the last drain costs O(changes), never O(state). The read API
/// ([`Net::places`](crate::Net::places),
/// [`Net::label`](crate::Net::label), ...) is the other channel, for the
/// initial mirror and for detail panels.
///
/// The enum is `#[non_exhaustive]`: match with a `_` arm.
///
/// Which variants a firing produces depends on the path it took, and the two
/// never mix: [`step`](crate::Net::step) and [`fire`](crate::Net::fire) log
/// one [`Fired`](Event::Fired), while [`launch`](crate::Net::launch) and
/// [`distribute`](crate::Net::distribute) log
/// [`Launched`](Event::Launched) and then [`Distributed`](Event::Distributed),
/// because time passes in between. Either way a firing's
/// [`Mutated`](Event::Mutated) events come immediately before the event that
/// ends it, and a failure ends it with [`Failed`](Event::Failed) instead.
///
/// With the `serde` feature it is `Serialize` (never `Deserialize`) in the
/// core's id form, for logs and tests: tagged by variant under `"event"`,
/// ids as bare numbers, so a [`Fired`](Event::Fired) reads
/// `{"event":"fired","transition":0,"consumed":[[0,17]],"produced":[[1,18]]}`
/// and a [`ParamChanged`](Event::ParamChanged)
/// `{"event":"param_changed","type_name":"..."}`. Shapes keyed by name, for a
/// wire protocol, live outside this crate.
///
/// A net whose tokens carry nets of their own ([`Nested`](crate::Nested))
/// reports what happens inside them here too, each inner event wrapped in a
/// [`Nested`](Event::Nested) that says where it happened.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(tag = "event", rename_all = "snake_case"))]
#[non_exhaustive]
pub enum Event {
    /// One complete firing, consumed and produced together. Emitted by the
    /// sequential path — [`step`](crate::Net::step) and
    /// [`fire`](crate::Net::fire) — where no time passes between the two.
    Fired {
        /// The transition that fired.
        transition: TransitionId,
        /// The tokens taken out of the input places, and where they were.
        consumed: Vec<(PlaceId, TokenId)>,
        /// The tokens written to the output places, and where they went.
        produced: Vec<(PlaceId, TokenId)>,
    },
    /// Stage one of a staged firing ([`Net::launch`](crate::Net::launch)):
    /// the inputs have left their places and the body is airborne.
    ///
    /// The transition is [in flight](crate::Net::in_flight) from here until
    /// its [`Distributed`](Event::Distributed) or
    /// [`Failed`](Event::Failed).
    Launched {
        /// The transition now in flight.
        transition: TransitionId,
        /// The tokens taken out of the input places, and where they were.
        consumed: Vec<(PlaceId, TokenId)>,
    },
    /// Stage three of a staged firing
    /// ([`Net::distribute`](crate::Net::distribute)): the body's outputs have
    /// landed. What was consumed was reported by the
    /// [`Launched`](Event::Launched) that opened this firing.
    Distributed {
        /// The transition that completed.
        transition: TransitionId,
        /// The tokens written to the output places, and where they went.
        produced: Vec<(PlaceId, TokenId)>,
    },
    /// A firing whose body returned `Err` or panicked, or whose
    /// [`Distribution`](crate::Distribution) named a place the transition may
    /// not write to. Nothing was distributed.
    Failed {
        /// The transition that failed.
        transition: TransitionId,
        /// The tokens this firing took out of the input places, and where
        /// they were — the same list a successful firing reports as
        /// `consumed`. Where they are now is what `restored` says.
        consumed: Vec<(PlaceId, TokenId)>,
        /// Whether the tokens in `consumed` were put back, as asked for by
        /// [`restore_tokens_on_failure`](crate::TransitionBuilder::restore_tokens_on_failure).
        ///
        /// True: every one of them is back at the *front* of its place, in
        /// the order it was taken and with the id it had — the firing was
        /// undone, and an observer that re-reads the marking when this event
        /// arrives sees them there. Restoring emits no
        /// [`Added`](Event::Added) of its own, because nothing new appeared:
        /// this flag is the whole report. (True with an empty `consumed` for
        /// a restoring transition that consumes nothing at all, which reads
        /// nothing but snapshots and mutating edges.)
        ///
        /// False: they are gone, and their ids are retired.
        restored: bool,
        /// The failure, rendered with `Display`. A panicking body reads
        /// `"panicked: ..."` ([`BodyPanicked`](crate::BodyPanicked)); the
        /// value itself is on the
        /// [`FiringFailed`](crate::FiringFailed) the failing call returns.
        error: String,
    },
    /// A token appeared in a place from outside the net: an
    /// [`extend`](crate::Net::extend), a [`put`](crate::Net::put) or the
    /// second half of a [`set_tokens`](crate::Net::set_tokens). The seeded
    /// marking of a fresh net is setup rather than change and is not
    /// reported here.
    Added {
        /// The place the token is now in.
        place: PlaceId,
        /// The new token's id, never seen before in this net.
        token: TokenId,
    },
    /// A token was taken out of a place from outside the net: a
    /// [`remove`](crate::Net::remove) or the first half of a
    /// [`set_tokens`](crate::Net::set_tokens). Tokens consumed by a firing are
    /// reported by [`Fired`](Event::Fired) or [`Launched`](Event::Launched)
    /// instead.
    Removed {
        /// The place the token was in.
        place: PlaceId,
        /// The retired token id, which will never be handed out again.
        token: TokenId,
    },
    /// A token changed in place, keeping its id and its position in the
    /// queue: a [`replace`](crate::Net::replace), or a mutating edge.
    Mutated {
        /// The place holding the token.
        place: PlaceId,
        /// The token, whose id is unchanged — its label and JSON are not.
        token: TokenId,
    },
    /// A transition was switched on or off.
    Toggled {
        /// The transition that was toggled.
        transition: TransitionId,
        /// Its new state.
        enabled: bool,
    },
    /// A parameter value was replaced: a
    /// [`set_param`](crate::Net::set_param) or an erased
    /// [`Command::SetParam`](crate::Command::SetParam).
    ///
    /// Parameters are keyed by type and are deliberately not `Debug`, so only
    /// the type name is reported — a parameter may be a secret, and an event
    /// log is the last place for one. Firings already in flight are
    /// unaffected: each took its clone at launch.
    ParamChanged {
        /// The parameter type whose value changed.
        type_name: &'static str,
    },
    /// The marking was replaced wholesale, by an `import_marking` (a
    /// `serde` feature method): something changed that is not told as
    /// events. Re-read the marking.
    ///
    /// An import is not a sequence of changes, so it reports no
    /// [`Removed`](Event::Removed) or [`Added`](Event::Added) per token: it
    /// emits this once, and moves [`state_counter`](crate::Net::state_counter)
    /// by one. Every token after it has an id the reader has not seen.
    /// Python petritype has an event of the same name, recorded per place
    /// with the place's name; an import here replaces every place at once,
    /// so it is one event with no fields. The runner reports it as
    /// `reloaded {reason: "resync"}`.
    Resynced,
    /// Something happened inside an inner net of one of this net's tokens
    /// (DESIGN §6.6): `event` is what that net logged, and the other fields
    /// say where it was. Deeper levels nest: an event two nets down is a
    /// `Nested` inside a `Nested`.
    ///
    /// An inner net's events are *lifted* into this log by the net itself,
    /// never by a driver, at fixed moments, each with the place and the token
    /// id the token has at that moment:
    ///
    /// 1. after an inner firing made by [`step_deep`](crate::Net::step_deep),
    ///    straight away, under the place the token sits in;
    /// 2. after a [`MutateEdge`](crate::MutateEdge) body ran on the token,
    ///    just before that firing's [`Mutated`](Event::Mutated) for it — so a
    ///    firing reads `Nested`*, `Mutated`, then
    ///    [`Fired`](Event::Fired), [`Distributed`](Event::Distributed) or
    ///    [`Failed`](Event::Failed);
    /// 3. when a firing consumes the token, at launch, before its
    ///    [`Launched`](Event::Launched) or [`Fired`](Event::Fired), under the
    ///    place it was taken from and the id it had there — nothing is lost
    ///    with a token that is not put back;
    /// 4. when a firing produces the token, after its
    ///    [`Fired`](Event::Fired) or [`Distributed`](Event::Distributed),
    ///    under the place it landed in and its *new* id, so that a reader has
    ///    seen the id before anything is said about it;
    /// 5. when a token arrives from outside ([`put`](crate::Net::put),
    ///    [`extend`](crate::Net::extend), [`set_tokens`](crate::Net::set_tokens),
    ///    [`replace`](crate::Net::replace), and their
    ///    [`Command`](crate::Command)s), after its [`Added`](Event::Added) or
    ///    [`Mutated`](Event::Mutated); when one leaves ([`remove`](crate::Net::remove),
    ///    [`set_tokens`](crate::Net::set_tokens), [`replace`](crate::Net::replace)),
    ///    before its [`Removed`](Event::Removed) or [`Mutated`](Event::Mutated);
    /// 6. when the closure given to [`nested_scope`](crate::Net::nested_scope)
    ///    returns.
    ///
    /// Each lifted event is one change here, and moves
    /// [`state_counter`](crate::Net::state_counter) by one. Seeded tokens are
    /// setup: whatever their inner nets logged before the net was built is
    /// dropped by [`build`](crate::NetBuilder::build).
    ///
    /// With the `serde` feature the wrapped event is under `"inner"`, since
    /// `"event"` is the tag: `{"event":"nested","place":0,"token":3,
    /// "net":"cabin","nested_id":7,"inner":{"event":"fired",...}}`.
    Nested {
        /// The nested place the token was in when this was lifted.
        place: PlaceId,
        /// The token, by the id it had in that place when this was lifted.
        /// A token taken and put back gets a new id; `nested_id` does not
        /// change.
        token: TokenId,
        /// The inner net's name, as [`Nested::nets`](crate::Nested::nets)
        /// gave it; `""` for a bare [`Net`](crate::Net) token.
        net: String,
        /// The inner net's own [`Net::id`](crate::Net::id): stable while the
        /// token moves from place to place and its [`TokenId`] changes, so it
        /// is the id to follow one token's inner life by. Process-local,
        /// like every [`NetId`].
        nested_id: NetId,
        /// What the inner net logged.
        #[cfg_attr(feature = "serde", serde(rename = "inner"))]
        event: Box<Event>,
    },
}
