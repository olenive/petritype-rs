//! Error types. All are `#[non_exhaustive]`: match with a `_` arm.

use std::any::Any;

use crate::ids::{NetId, PlaceId, TokenId, TransitionId};

/// A net that cannot be built, returned by
/// [`NetBuilder::build`](crate::NetBuilder::build).
///
/// Build errors are wiring mistakes that the type system cannot catch. They
/// are found once, before the net runs.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BuildError {
    /// Two places were given the same name. Names identify places to a reader
    /// and to [`Net::place_named`](crate::Net::place_named), so they must be
    /// unique.
    #[error("duplicate place name: {name:?}")]
    DuplicatePlaceName {
        /// The name used more than once.
        name: String,
    },
    /// Two transitions were given the same name, which
    /// [`Net::transition_named`](crate::Net::transition_named) and every
    /// diagram would then be unable to tell apart.
    #[error("duplicate transition name: {name:?}")]
    DuplicateTransitionName {
        /// The name used more than once.
        name: String,
    },
    /// A [`SnapshotEdge`](crate::SnapshotEdge) reads a place declared with
    /// [`place_no_clone`](crate::NetBuilder::place_no_clone).
    ///
    /// A snapshot is a clone of the front token, and such a place recorded no
    /// cloner. The token type may well be `Clone` — the place is what gave
    /// the ability up — so this cannot be a compile error. Use a
    /// [`TakeEdge`](crate::TakeEdge), or declare the place with
    /// [`place`](crate::NetBuilder::place).
    #[error("transition {transition:?} snapshots place {place:?}, which cannot clone its tokens")]
    SnapshotOfNoClonePlace {
        /// The transition whose edge it is.
        transition: String,
        /// The place it reads.
        place: String,
    },
    /// One transition reads one place through two input edges, whatever their
    /// kinds.
    ///
    /// A transition's input places must be distinct, because both halves of
    /// firing assume it: "[fireable](crate::Net::fireable) iff every input
    /// place is non-empty" would have to count how many tokens each edge
    /// wants otherwise, and the `&mut` borrows a
    /// [`MutateEdge`](crate::MutateEdge) hands the body are disjoint only
    /// because no two edges name the same place. Take a batch with one
    /// [`TakeAllEdge`](crate::TakeAllEdge), or split the work between two
    /// transitions.
    #[error("transition {transition:?} reads place {place:?} through more than one input edge")]
    DuplicateInputPlace {
        /// The transition whose edges they are.
        transition: String,
        /// The place read more than once.
        place: String,
    },
    /// A transition whose body [routes](crate::RoutedOutput) its outputs by
    /// type may emit a payload of a type none of its output places holds.
    ///
    /// Either an output place is missing from the declaration, or the body
    /// returns a payload the net has nowhere to put. A positional body,
    /// where the slot rather than the type picks the place, has no such
    /// question to answer.
    #[error("transition {transition:?} routes {type_name} to no output place")]
    UnroutableOutput {
        /// The transition whose body it is.
        transition: String,
        /// The payload type with nowhere to go.
        type_name: &'static str,
    },
    /// Two of a transition's output places hold the same type, so that type
    /// no longer picks one of them.
    ///
    /// Routing by type is only unambiguous while the types are distinct.
    /// Return a tuple instead, where order picks the place, or give one of
    /// the two places a newtype of its own so that the types differ again.
    #[error("transition {transition:?} has more than one output place of type {type_name}")]
    AmbiguousOutput {
        /// The transition whose output places they are.
        transition: String,
        /// The payload type with more than one destination.
        type_name: &'static str,
    },
    /// A transition declares an output place that no payload type of its
    /// routed body can reach.
    ///
    /// The declaration is what a diagram draws, so an edge no token can
    /// travel is a lie about the net. Drop the output place, or return a
    /// value that can reach it.
    #[error("transition {transition:?} can route nothing to output place {place:?}")]
    UnreachableOutput {
        /// The transition whose output place it is.
        transition: String,
        /// The place nothing can reach.
        place: String,
    },
    /// A transition asked to
    /// [`restore_tokens_on_failure`](crate::TransitionBuilder::restore_tokens_on_failure)
    /// consumes from a place declared with
    /// [`place_no_clone`](crate::NetBuilder::place_no_clone).
    ///
    /// Restoring works on clones taken before the body runs, because the body
    /// owns the tokens themselves and may have consumed or wrecked them by
    /// the time it fails; a place with no cloner has nothing to take. Only
    /// the consuming edges are affected — [`TakeEdge`](crate::TakeEdge) and
    /// [`TakeAllEdge`](crate::TakeAllEdge) — since a
    /// [`MutateEdge`](crate::MutateEdge) takes nothing out of its place and
    /// so has nothing to put back, and a
    /// [`SnapshotEdge`](crate::SnapshotEdge) of such a place is already
    /// refused by [`SnapshotOfNoClonePlace`](BuildError::SnapshotOfNoClonePlace).
    /// Drop the flag, or declare the place with
    /// [`place`](crate::NetBuilder::place).
    #[error(
        "transition {transition:?} restores its inputs on failure but consumes place {place:?}, \
         which cannot clone its tokens"
    )]
    RestoreOfNoClonePlace {
        /// The transition whose flag it is.
        transition: String,
        /// The place it consumes.
        place: String,
    },
    /// A transition consumes nothing, in a net built with
    /// [`assert_no_source_transitions`](crate::NetBuilder::assert_no_source_transitions).
    ///
    /// Such a *source* transition takes no token out of any place when it
    /// fires — read edges do not count, since a
    /// [`SnapshotEdge`](crate::SnapshotEdge) and a
    /// [`MutateEdge`](crate::MutateEdge) leave their places as they found
    /// them — so having fired changes nothing about whether it can fire
    /// again, and the net never quiesces. Give it a
    /// [`TakeEdge`](crate::TakeEdge) or a
    /// [`TakeAllEdge`](crate::TakeAllEdge), or drop the assertion:
    /// generators and transitions fed from outside the net are deliberate
    /// sources, and are bounded while the net runs instead.
    #[error("transition {transition:?} consumes nothing, so it could fire forever")]
    SourceTransition {
        /// The offending transition — the first one, in declaration order.
        transition: String,
    },
    /// The net's token flow has a cycle, in a net built with
    /// [`assert_acyclic`](crate::NetBuilder::assert_acyclic).
    ///
    /// Tokens can go round it for ever, so the net has no finish line the
    /// marking is working towards. Break the loop, or drop the assertion: a
    /// net that legitimately runs for ever is bounded while it runs instead.
    #[error("the net's token flow has a cycle: {}", path.join(" -> "))]
    Cycle {
        /// The nodes of the cycle by name, from the node the walk closed on
        /// round to itself: the first and the last name are the same one,
        /// and places and transitions alternate in between.
        path: Vec<String>,
    },
}

/// Misuse of the net's API at run time.
///
/// These are recoverable in the sense that the net is left untouched: nothing
/// is added, removed or mutated when one of these is returned — with the one
/// exception of [`FiringFailed`](NetError::FiringFailed), where the net is
/// consistent but the firing's inputs are gone (unless the transition asked
/// to [restore](crate::TransitionBuilder::restore_tokens_on_failure) them).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum NetError {
    /// No place with this id exists in this net. Usually a handle from a
    /// different net.
    #[error("no such place: {place:?}")]
    NoSuchPlace {
        /// The offending id.
        place: PlaceId,
    },
    /// No transition with this id exists in this net. Usually an id from a
    /// different net, since ids are never reused within one.
    #[error("no such transition: {transition:?}")]
    NoSuchTransition {
        /// The offending id.
        transition: TransitionId,
    },
    /// [`Net::fire`](crate::Net::fire) was told to fire a transition that
    /// cannot fire: it is switched off, one of its input places is empty, or
    /// its guard says no. [`Net::fireable`](crate::Net::fireable) lists the
    /// ones that can.
    #[error("transition {transition:?} is not fireable")]
    NotFireable {
        /// The transition that was asked to fire.
        transition: TransitionId,
    },
    /// The place exists but holds no token with this id. The token may have
    /// been consumed, removed, or never have belonged to this place.
    #[error("place {place:?} holds no token {token:?}")]
    NoSuchToken {
        /// The place that was searched.
        place: PlaceId,
        /// The token that was not found.
        token: TokenId,
    },
    /// A value of the wrong type was offered to a place. Only reachable
    /// through a handle from another net or, later, through an erased
    /// command: the typed API makes this a compile error.
    #[error("place {place:?} holds {expected}, not {found}")]
    TypeMismatch {
        /// The place that was written to.
        place: PlaceId,
        /// The type the place holds.
        expected: &'static str,
        /// The type that was offered.
        found: &'static str,
    },
    /// [`Net::distribute`](crate::Net::distribute) was handed a
    /// [`Completed`](crate::Completed) that another net launched.
    ///
    /// Checked before anything else, because a
    /// [`TransitionId`] means nothing outside its own net: every net numbers
    /// its transitions from zero, so the id would otherwise name whichever
    /// transition happens to sit at that index here. The net is untouched —
    /// and so is the net that launched the firing, whose transition stays in
    /// flight, since the `Completed` was consumed by the call.
    #[error("transition {transition:?} was launched by net {from:?}, not this one")]
    ForeignFiring {
        /// The transition the firing belongs to, in the net that launched it.
        transition: TransitionId,
        /// That net.
        from: NetId,
    },
    /// [`Net::distribute`](crate::Net::distribute) was handed a
    /// [`Completed`](crate::Completed) for a transition whose body is already
    /// in its slot.
    ///
    /// A transition is in flight from
    /// [`launch`](crate::Net::launch) until the firing it produced is
    /// distributed ([`Net::in_flight`](crate::Net::in_flight)), and a
    /// `Completed` is consumed by the call that distributes it, so the safe
    /// API gives no way to distribute one twice. This is the guard that keeps
    /// it that way: nothing is written, and the outputs of the rejected
    /// firing are dropped with it.
    #[error("transition {transition:?} is not in flight")]
    NotInFlight {
        /// The transition that was not airborne.
        transition: TransitionId,
    },
    /// A firing this call started failed. [`Net::fire`](crate::Net::fire)
    /// reports a body's failure this way; [`Net::step`](crate::Net::step)
    /// returns the [`FiringFailed`] itself.
    #[error(transparent)]
    FiringFailed(#[from] FiringFailed),
}

/// An erased [`Command`](crate::Command) the net would not carry out,
/// returned by [`Net::apply`](crate::Net::apply).
///
/// A rejected command changes nothing and emits nothing (DESIGN §9), so this
/// error is always a complete report: the net is exactly as it was before the
/// call. Most of it is a [`NetError`] — the erased route is where a wrong
/// type or a stale id can really arrive, since the typed API makes both a
/// compile error or a panic. The rest is the JSON route's own vocabulary,
/// where places and transitions travel by name and tokens as
/// `serde_json::Value`, so there are two more ways to be wrong before the net
/// is ever asked.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CommandError {
    /// The net rejected the command: an unknown place, transition or token,
    /// or a token of the wrong type for its place.
    #[error(transparent)]
    Net(#[from] NetError),
    /// A JSON command names a place that was not marked
    /// [`serializable`](crate::NetBuilder::serializable), so the net has no
    /// way to decode its tokens — or, for
    /// [`Command::to_json`](crate::Command::to_json), to encode them.
    ///
    /// Serialisation is opt-in per place; mark the place in the builder.
    #[cfg(feature = "serde")]
    #[error("place {place:?} is not marked serializable")]
    NotSerializable {
        /// The place whose tokens cannot cross a wire.
        place: PlaceId,
    },
    /// A token in a JSON command did not parse as the place's type.
    ///
    /// The JSON is well-formed but the wrong shape — `"seven"` for a place of
    /// `u32`. Nothing is applied: the command is rejected at the first token
    /// that fails, before the net is touched.
    #[cfg(feature = "serde")]
    #[error("place {place:?}: token does not parse as {expected}: {source}")]
    InvalidToken {
        /// The place the token was meant for.
        place: PlaceId,
        /// The type that place holds, `std::any::type_name::<T>()`.
        expected: &'static str,
        /// What `serde_json` made of it.
        #[source]
        source: serde_json::Error,
    },
    /// A JSON command named a place that does not exist.
    ///
    /// The JSON form addresses places by name rather than by
    /// [`PlaceId`], because a name is what an author or a saved file knows
    /// and it survives a change in declaration order.
    #[cfg(feature = "serde")]
    #[error("no place named {name:?}")]
    NoSuchPlaceName {
        /// The name that matched no place.
        name: String,
    },
    /// A JSON command named a transition that does not exist.
    #[cfg(feature = "serde")]
    #[error("no transition named {name:?}")]
    NoSuchTransitionName {
        /// The name that matched no transition.
        name: String,
    },
}

/// A firing whose body returned `Err` or panicked.
///
/// By the time this is returned the net is consistent again (DESIGN §7): the
/// body is back in its slot, the transition is not in flight, and nothing was
/// distributed, so the next [`step`](crate::Net::step) works. What the firing
/// consumed is *gone* unless [`restored`](FiringFailed::restored) says
/// otherwise; [`Event::Failed`](crate::Event::Failed) records which tokens
/// those were.
///
/// A panicking body arrives here like any other failure, with
/// [`BodyPanicked`] as the [`error`](FiringFailed::error) — downcast it to
/// tell the two apart.
///
/// The transition's name is not carried: an error is read next to the net
/// that produced it, and
/// [`Net::transitions`](crate::Net::transitions) turns the id into a name.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
#[error("transition {transition:?} failed: {error}")]
pub struct FiringFailed {
    /// The transition whose body failed.
    pub transition: TransitionId,
    /// What the body returned, erased at the edge of the typed API so that
    /// the engine speaks one error type whatever bodies return. It is also
    /// this error's [`source`](std::error::Error::source).
    #[source]
    pub error: Box<dyn std::error::Error + Send + Sync>,
    /// Whether the consumed tokens were put back in their places.
    ///
    /// True exactly for a transition declared with
    /// [`restore_tokens_on_failure`](crate::TransitionBuilder::restore_tokens_on_failure),
    /// whose consumed tokens are back at the front of their places with the
    /// ids they had. False otherwise: a failed firing loses its inputs by
    /// default, because the body owned them and may have changed or dropped
    /// them before it failed.
    pub restored: bool,
}

/// A body that panicked, as the failure its firing reports.
///
/// Panics are caught (DESIGN §7): [`Firing::run`](crate::Firing::run) wraps
/// the call to a detachable body, and [`launch`](crate::Net::launch) wraps
/// the inline call it makes for a transition with a
/// [`MutateEdge`](crate::MutateEdge) input, both in
/// [`catch_unwind`](std::panic::catch_unwind). Either way the panic becomes
/// this error, the firing fails like one whose body returned `Err`, and the
/// body goes home to its slot — so one panicking token does not strand a
/// transition in flight for good.
///
/// What comes home is whatever the panic left behind: a body is `FnMut` and
/// may keep state of its own, and a panic can stop it half way through
/// changing it. The net's own state is not in question — nothing was
/// distributed — and a token a [`MutateEdge`](crate::MutateEdge) body had
/// already changed stays changed, since the change was made where the token
/// lies.
///
/// Nothing can be caught under `panic = "abort"`, where the process dies at
/// the panic, as the profile asked.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
#[error("panicked: {message}")]
pub struct BodyPanicked {
    /// The panic payload as text: the message a `panic!` carried, or
    /// `"panic payload of unknown type"` for a payload that is neither a
    /// `&str` nor a `String` — which only
    /// [`panic_any`](std::panic::panic_any) can produce.
    pub message: String,
}

impl BodyPanicked {
    /// Reads the message out of a caught payload.
    ///
    /// The two types `panic!` itself produces, in that order: a literal
    /// message is a `&'static str` and a formatted one a `String`. Anything
    /// else came from [`panic_any`](std::panic::panic_any) and has no text to
    /// read, so the type is not named either — `Any` cannot say what it is.
    pub(crate) fn from_payload(payload: Box<dyn Any + Send>) -> BodyPanicked {
        let message = if let Some(message) = payload.downcast_ref::<&'static str>() {
            (*message).to_owned()
        } else if let Some(message) = payload.downcast_ref::<String>() {
            message.clone()
        } else {
            String::from("panic payload of unknown type")
        };
        BodyPanicked { message }
    }
}
