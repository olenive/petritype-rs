//! Error types. All are `#[non_exhaustive]`: match with a `_` arm.

use crate::ids::{PlaceId, TokenId, TransitionId};

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
}

/// Misuse of the net's API at run time.
///
/// These are recoverable in the sense that the net is left untouched: nothing
/// is added, removed or mutated when one of these is returned — with the one
/// exception of [`FiringFailed`](NetError::FiringFailed), where the net is
/// consistent but the firing's inputs are gone.
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
    /// A firing this call started failed. [`Net::fire`](crate::Net::fire)
    /// reports a body's failure this way; [`Net::step`](crate::Net::step)
    /// returns the [`FiringFailed`] itself.
    #[error(transparent)]
    FiringFailed(#[from] FiringFailed),
}

/// A firing whose body returned `Err`.
///
/// By the time this is returned the net is consistent again (DESIGN §7): the
/// body is back in its slot, the transition is not in flight, and nothing was
/// distributed, so the next [`step`](crate::Net::step) works. What the firing
/// consumed is *gone* unless [`restored`](FiringFailed::restored) says
/// otherwise; [`Event::Failed`](crate::Event::Failed) records which tokens
/// those were.
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
    /// Always `false` for now: `restore_tokens_on_failure` arrives with the
    /// slice that adds the flag, and until then a failed firing loses its
    /// inputs.
    pub restored: bool,
}
