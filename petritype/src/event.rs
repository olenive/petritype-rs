//! The event log: what changed, in order.

use crate::ids::{PlaceId, TokenId, TransitionId};

/// One change to the net, appended by the net and drained by the driver with
/// [`Net::drain_events`](crate::Net::drain_events).
///
/// The log is the cheap update channel for an observer: replaying the events
/// since the last drain costs O(changes), never O(state). The read API
/// ([`Net::places`](crate::Net::places),
/// [`Net::label`](crate::Net::label), ...) is the other channel, for the
/// initial mirror and for detail panels.
///
/// The enum is `#[non_exhaustive]`: match with a `_` arm. This slice only ever
/// emits [`Added`](Event::Added), [`Removed`](Event::Removed) and
/// [`Mutated`](Event::Mutated); the firing variants are declared now so that
/// observers written against this version keep compiling as the engine grows.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Event {
    /// One complete firing, consumed and produced together. Emitted by the
    /// sequential path, where no time passes between the two.
    Fired {
        /// The transition that fired.
        transition: TransitionId,
        /// The tokens taken out of the input places, and where they were.
        consumed: Vec<(PlaceId, TokenId)>,
        /// The tokens written to the output places, and where they went.
        produced: Vec<(PlaceId, TokenId)>,
    },
    /// Stage one of a concurrent firing: the inputs have left their places and
    /// the body is airborne.
    Launched {
        /// The transition now in flight.
        transition: TransitionId,
        /// The tokens taken out of the input places, and where they were.
        consumed: Vec<(PlaceId, TokenId)>,
    },
    /// Stage three of a concurrent firing: the body's outputs have landed.
    Distributed {
        /// The transition that completed.
        transition: TransitionId,
        /// The tokens written to the output places, and where they went.
        produced: Vec<(PlaceId, TokenId)>,
    },
    /// A firing whose body returned `Err` or panicked.
    Failed {
        /// The transition that failed.
        transition: TransitionId,
        /// The consumed tokens, which are gone unless `restored` is true.
        lost: Vec<(PlaceId, TokenId)>,
        /// Whether the inputs were put back, as asked for by
        /// `restore_tokens_on_failure`.
        restored: bool,
        /// The body's error, rendered with `Display`.
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
    /// A parameter value was replaced. Parameters are keyed by type and are
    /// deliberately not `Debug`, so only the type name is reported.
    ParamChanged {
        /// The parameter type whose value changed.
        type_name: &'static str,
    },
}
