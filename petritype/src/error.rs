//! Error types. All are `#[non_exhaustive]`: match with a `_` arm.

use crate::ids::{PlaceId, TokenId};

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
}

/// Misuse of the net's API at run time.
///
/// These are recoverable in the sense that the net is left untouched: nothing
/// is added, removed or mutated when one of these is returned.
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
}
