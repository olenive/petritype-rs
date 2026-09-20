//! The engine half of the public traits.
//!
//! Each public trait in [`crate::edge`] and [`crate::body`] — `InputEdge`,
//! `InputEdges`, `OutputPlaces`, `PositionalOutputs`, `IntoOutcome`,
//! `TransitionBody` — has a supertrait here, named after it with a `Sealed`
//! suffix, which is also what a reader of the public documentation sees in
//! the trait's header. The split does two jobs at once:
//!
//! - **Sealing.** These traits are not nameable outside the crate, so no
//!   outside type can implement the public ones. Slices to come add impls
//!   here; nobody else can add impls that would collide with them.
//! - **Hiding the erased plumbing.** A public trait may not mention
//!   [`ErasedInput`](crate::erased::ErasedInput) and friends in its method
//!   signatures, but these traits may, so the engine's calling convention
//!   stays an implementation detail.
//!
//! Every impl here is written next to the impl of its public partner, by the
//! same macro, so the two halves cannot drift apart.

use std::any::TypeId;

use crate::edge::InputSpec;
use crate::erased::{
    ErasedBody, ErasedInput, ErasedOutcome, OutputTarget, RouteTable, RoutingProblem,
};
use crate::ids::{NetId, PlaceId};
use crate::place::ErasedToken;

/// The engine half of [`InputEdge`](crate::InputEdge): what the body
/// parameter for this input is, how to take it out of an erased input, and
/// what the input says it is.
pub trait EdgeSealed: Copy {
    /// The body parameter this input produces, for a body called with tokens
    /// that live for `'a`.
    ///
    /// A generic associated type, because [`MutateEdge`](crate::MutateEdge)
    /// produces `&'a mut T`: the parameter borrows the token where it lies,
    /// for the duration of one call. The other three kinds, and
    /// [`Param<T>`](crate::Param), hand over owned values and ignore `'a`.
    type Arg<'a>;

    /// Takes this input's argument out of the input the engine prepared.
    ///
    /// Panics if the variant does not match the edge kind, which the engine
    /// chooses, and `expect`s the downcast, which the typed handle in the
    /// edge has already guaranteed.
    fn extract<'a>(input: &'a mut ErasedInput<'_>) -> Self::Arg<'a>;

    /// What this input is: a place edge with its kind and the net its handle
    /// carried, or a parameter with its type (DESIGN §10).
    ///
    /// Takes `self` rather than being a constant because an edge's answer
    /// depends on the handle inside it, and `Self: Copy` makes that free.
    fn spec(self) -> InputSpec;
}

/// The engine half of [`InputEdges`](crate::InputEdges): the erased
/// description of a tuple of inputs, in declaration order.
pub trait EdgeTupleSealed {
    /// Appends one [`InputSpec`] per input position, in tuple order, so the
    /// builder can check the stamps, record the wiring, and keep the order
    /// the body's parameters are in.
    fn describe(&self, out: &mut Vec<InputSpec>);
}

/// The engine half of [`OutputPlaces`](crate::OutputPlaces): the erased
/// description of a tuple of place handles, in declaration order.
pub trait PlaceTupleSealed {
    /// Appends `(place, net)` for each output place.
    fn describe(&self, out: &mut Vec<(PlaceId, NetId)>);

    /// Appends `(type id, type name)` for the token type of each output
    /// place.
    ///
    /// Static, because routing is resolved from types alone: the handles
    /// say which places a transition writes to, and this says what they
    /// hold, which is the half a payload type is matched against.
    fn describe_types(out: &mut Vec<(TypeId, &'static str)>);
}

/// The engine half of [`PositionalOutputs`](crate::PositionalOutputs):
/// turning a returned tuple into tokens tagged with their output index.
pub trait PositionalSealed<Out> {
    /// Appends `(target, token)` for every token this return value produced,
    /// slot by slot. The target is always an
    /// [`Index`](OutputTarget::Index): a slot's position *is* its output
    /// place's index.
    fn push_erased(self, out: &mut Vec<(OutputTarget, ErasedToken)>);
}

/// The engine half of [`IntoOutcome`](crate::IntoOutcome): turning whatever
/// the body returned into the one shape the engine distributes.
pub trait OutcomeSealed<Out, M> {
    /// Resolves this output form against those output places, once, when the
    /// transition is declared.
    ///
    /// Only the routed form has anything to resolve; the others answer with
    /// an empty table. The `Err` is stashed by
    /// [`transition`](crate::NetBuilder::transition), which must stay
    /// infallible, and reported by
    /// [`build`](crate::NetBuilder::build), which knows the transition's
    /// name.
    fn routing() -> Result<RouteTable, RoutingProblem>;

    /// Consumes the body's return value, sending each token to the output
    /// place the form says it belongs to.
    fn into_outcome(self, routes: &RouteTable) -> ErasedOutcome;
}

/// The engine half of [`TransitionBody`](crate::TransitionBody): wrapping a
/// typed body into an erased one.
pub trait BodySealed<E, O, M> {
    /// Wraps this body, once, at declaration time. The wrapper extracts one
    /// argument per edge, calls the body, and converts what it returns.
    ///
    /// The routing table is resolved here too, where the return type and the
    /// output places are both in scope, and captured by the wrapper — which
    /// is why no table is stored beside the transition. A table that cannot
    /// be resolved comes back as the second half of the pair, for
    /// [`build`](crate::NetBuilder::build) to refuse the net with.
    fn into_erased(self) -> (ErasedBody, Option<RoutingProblem>);
}
