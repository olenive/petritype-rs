//! Routed outputs: a returned value goes to the output place of its type.
//!
//! The positional form in [`crate::IntoTokens`] matches slots to places by
//! order. The routed form matches them by **type**: a body returns one value,
//! and each payload inside it lands in the one output place that holds a
//! payload of that type. The order the outputs were declared in does not
//! matter, and a two-way answer is a two-variant enum rather than a tuple of
//! `Option`s where exactly one is `Some`.
//!
//! ```
//! use petritype::{NetBuilder, Routed, TakeEdge};
//!
//! #[derive(Debug, Clone)]
//! struct Parsed(u32);
//! #[derive(Debug, Clone)]
//! struct Rejected(String);
//!
//! #[derive(Routed)]
//! enum Outcome {
//!     Parsed(Parsed),
//!     Rejected(Rejected),
//! }
//!
//! let mut b = NetBuilder::new();
//! let inbox = b.place::<String>("Inbox");
//! let parsed = b.place::<Parsed>("Parsed");
//! let rejected = b.place::<Rejected>("Rejected");
//!
//! b.transition(
//!     "Parse",
//!     (TakeEdge(inbox),),
//!     (parsed, rejected),
//!     |line: String| match line.parse::<u32>() {
//!         Ok(n) => Outcome::Parsed(Parsed(n)),
//!         Err(_) => Outcome::Rejected(Rejected(line)),
//!     },
//! );
//! let mut net = b.build()?;
//!
//! net.put(inbox, String::from("41"));
//! net.step()?;
//! assert_eq!(net.count(parsed.id()), 1);
//! assert_eq!(net.count(rejected.id()), 0);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # What is routable
//!
//! Routing is for sum types and the wrappers around them, because a type is
//! only a destination when it is unambiguously one:
//!
//! - [`#[derive(Routed)]`](crate::Routed) on an enum: one payload type per
//!   variant, a unit variant emitting nothing;
//! - `#[derive(Routed)]` on a struct: a leaf, routed whole to the place of
//!   its own type;
//! - [`Either<A, B>`]: the two-way answer without declaring an enum;
//! - `Option<T>` where `T: RoutedOutput`: `None` emits nothing;
//! - `Vec<T>` where `T: RoutedOutput`: every element emits, so a `Vec` is
//!   always a batch;
//! - `Result<T, E>` where `T: RoutedOutput`: `Err` is a failure, as in every
//!   other output form (DESIGN §7), never a route.
//!
//! There is deliberately **no** impl for a bare `T`: it would overlap
//! `Option<T>` and `Vec<T>`, and a std type has no place of its own to be
//! routed to anyway. A single value goes in a one-slot tuple, `(value,)`, or
//! in a struct of its own with `Routed` derived on it.
//!
//! # What is checked, and when
//!
//! Routing is resolved from types alone, when the transition is declared: the
//! payload types come from [`RoutedOutput::describe`] and the output place
//! types from the tuple of handles, so the whole `type -> output index` table
//! is built before the net runs. What that resolution finds is reported by
//! [`NetBuilder::build`](crate::NetBuilder::build), naming the transition:
//!
//! - [`BuildError::UnroutableOutput`](crate::BuildError::UnroutableOutput):
//!   a payload type with no output place to go to;
//! - [`BuildError::AmbiguousOutput`](crate::BuildError::AmbiguousOutput):
//!   two output places of one type, so the type no longer picks one;
//! - [`BuildError::UnreachableOutput`](crate::BuildError::UnreachableOutput):
//!   an output place nothing can reach, because a diagram that draws an
//!   edge no token can travel is lying.

use std::any::{TypeId, type_name};
use std::fmt;

use crate::erased::{OutputTarget, RouteTable};
use crate::place::ErasedToken;

/// A value that knows which output places its contents belong in.
///
/// Implemented by [`#[derive(Routed)]`](crate::Routed), which is the
/// documented way: the derive writes both halves from one declaration and so
/// cannot let them disagree. A hand-written impl is allowed (for a type of
/// your own whose payloads are not simply its variants, such as a wrapper
/// that spreads a map over two places), and then the two halves are a
/// contract the author keeps:
///
/// - [`describe`](RoutedOutput::describe) lists every payload type that
///   [`emit`](RoutedOutput::emit) can push, whether or not this particular
///   value pushes it. It is called once, when the transition is declared, and
///   is what the build-time checks see.
/// - [`emit`](RoutedOutput::emit) pushes payloads of those types and no
///   others. Pushing an undeclared type panics, because no output place was
///   resolved for it.
///
/// Both are static in the payload types: routing never inspects a value to
/// find out where it goes, so a net's wiring is known before it runs.
///
/// ```
/// use petritype::{NetBuilder, Routed, TakeEdge};
///
/// // A struct with the derive on it is a leaf: one payload, its own type.
/// #[derive(Debug, Clone, Routed)]
/// struct Accepted(u32);
/// #[derive(Debug, Clone, Routed)]
/// struct Rejected(u32);
///
/// // An enum with the derive on it is a choice between payloads.
/// #[derive(Routed)]
/// enum Verdict {
///     Yes(Accepted),
///     No(Rejected),
///     Undecided,
/// }
///
/// let mut b = NetBuilder::new();
/// let inbox = b.place::<u32>("Inbox");
/// let accepted = b.place::<Accepted>("Accepted");
/// let rejected = b.place::<Rejected>("Rejected");
/// b.seed(inbox, [1]);
///
/// b.transition("Judge", (TakeEdge(inbox),), (accepted, rejected), |n: u32| {
///     if n == 0 { Verdict::Undecided } else { Verdict::No(Rejected(n)) }
/// });
/// let mut net = b.build()?;
///
/// net.step()?;
/// assert_eq!(net.count(rejected.id()), 1);
/// assert_eq!(net.count(accepted.id()), 0, "this firing chose the other variant");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be routed to an output place by its type",
    label = "not a routed output",
    note = "derive `Routed` on the enum or struct you return, or return a tuple with one slot \
            per output place"
)]
pub trait RoutedOutput: Send + Sync + 'static {
    /// Lists every payload type this value may emit.
    fn describe(spec: &mut RouteSpec);

    /// Emits the value's payload(s).
    fn emit(self, sink: &mut RouteSink<'_>);
}

/// The list [`RoutedOutput::describe`] fills in: one entry per payload type
/// the value may emit.
///
/// Repeating a type is harmless (two variants may carry the same payload and
/// both mean the same place), so the entry is kept once.
#[derive(Debug)]
pub struct RouteSpec {
    /// Payload types in declaration order, each with its name for the error
    /// message that may have to mention it.
    payloads: Vec<(TypeId, &'static str)>,
}

impl RouteSpec {
    /// An empty spec, filled by one call to
    /// [`RoutedOutput::describe`](RoutedOutput::describe).
    pub(crate) fn new() -> Self {
        RouteSpec {
            payloads: Vec::new(),
        }
    }

    /// Declares that this value may emit a `P`.
    ///
    /// The bound is the token bound minus `Debug` and `Clone`, which the
    /// output place has already demanded of the same type.
    pub fn payload<P: Send + Sync + 'static>(&mut self) {
        let id = TypeId::of::<P>();
        if !self.payloads.iter().any(|(seen, _)| *seen == id) {
            self.payloads.push((id, type_name::<P>()));
        }
    }

    /// What was declared, for the resolution the erased `RouteTable` does.
    pub(crate) fn payloads(&self) -> &[(TypeId, &'static str)] {
        &self.payloads
    }
}

/// Where [`RoutedOutput::emit`] puts the payloads: one push per token.
///
/// Each push looks the payload's type up in the table resolved when the
/// transition was declared and tags the token with the output place's index,
/// which is the same shape every other output form produces.
pub struct RouteSink<'a> {
    /// The `TypeId -> output index` table for this transition.
    routes: &'a RouteTable,
    /// The tokens produced so far, in the order they were pushed.
    out: &'a mut Vec<(OutputTarget, ErasedToken)>,
}

impl<'a> RouteSink<'a> {
    /// A sink writing into one firing's token list.
    pub(crate) fn new(
        routes: &'a RouteTable,
        out: &'a mut Vec<(OutputTarget, ErasedToken)>,
    ) -> Self {
        RouteSink { routes, out }
    }

    /// Emits one token of type `P` to the output place that holds `P`.
    ///
    /// # Panics
    ///
    /// If `P` was not declared by [`RoutedOutput::describe`]: a declared
    /// payload type always has exactly one output place, because
    /// [`NetBuilder::build`](crate::NetBuilder::build) refuses a net where it
    /// does not, so this can only be a `RoutedOutput` impl whose two halves
    /// disagree.
    pub fn push<P: Send + Sync + 'static>(&mut self, value: P) {
        let Some(index) = self.routes.index_of(TypeId::of::<P>()) else {
            panic!(
                "nothing routes {}: `RoutedOutput::emit` pushed a payload type that its \
                 `describe` did not list",
                type_name::<P>()
            );
        };
        self.out
            .push((OutputTarget::Index(index), Box::new(value) as ErasedToken));
    }
}

impl fmt::Debug for RouteSink<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RouteSink")
            .field("routes", self.routes)
            .field("pushed", &self.out.len())
            .finish()
    }
}

/// One of two things, each routed to its own output place.
///
/// The two-way answer without declaring an enum for it:
/// `Either<Accepted, Rejected>` says as much as a two-variant enum would.
/// Both sides are themselves [`RoutedOutput`] (a payload struct says it is a
/// leaf by deriving [`Routed`](crate::Routed)), so `Either<Outcome, Rejected>`
/// nests around a routed enum, and `Option<Either<A, B>>` says "maybe one of
/// two".
///
/// This is petritype's own type rather than the `either` crate's, so that the
/// public API of the crate carries no dependency that authors have to match.
///
/// ```
/// # use petritype::{Either, NetBuilder, Routed, TakeEdge};
/// #[derive(Debug, Clone, Routed)]
/// struct Accepted(u32);
/// #[derive(Debug, Clone, Routed)]
/// struct Rejected(u32);
/// # let mut b = NetBuilder::new();
/// # let inbox = b.place::<u32>("Inbox");
/// let accepted = b.place::<Accepted>("Accepted");
/// let rejected = b.place::<Rejected>("Rejected");
/// b.transition("Check", (TakeEdge(inbox),), (accepted, rejected), |n: u32| {
///     if n % 2 == 0 { Either::Left(Accepted(n)) } else { Either::Right(Rejected(n)) }
/// });
/// # let net = b.build()?;
/// # Ok::<(), petritype::BuildError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Either<A, B> {
    /// The first of the two.
    Left(A),
    /// The second of the two.
    Right(B),
}

/// Both sides are routed, so either side's payloads are this value's
/// payloads.
impl<A: RoutedOutput, B: RoutedOutput> RoutedOutput for Either<A, B> {
    fn describe(spec: &mut RouteSpec) {
        A::describe(spec);
        B::describe(spec);
    }

    fn emit(self, sink: &mut RouteSink<'_>) {
        match self {
            Either::Left(value) => value.emit(sink),
            Either::Right(value) => value.emit(sink),
        }
    }
}

/// Zero or one routed value: `None` emits nothing at all.
///
/// The payload types are the same either way (what a transition *may*
/// produce is what the diagram draws), so an empty `Option` does not make an
/// output place unreachable.
impl<T: RoutedOutput> RoutedOutput for Option<T> {
    fn describe(spec: &mut RouteSpec) {
        T::describe(spec);
    }

    fn emit(self, sink: &mut RouteSink<'_>) {
        if let Some(value) = self {
            value.emit(sink);
        }
    }
}

/// A batch: every element emits, in order.
///
/// A routed `Vec<T>` is always the batch reading, never one token holding a
/// vector. For the latter, make the `Vec` the payload of a variant:
/// `Batch(Vec<Job>)` sends one token to a `Vec<Job>` place, because a payload
/// is a leaf.
impl<T: RoutedOutput> RoutedOutput for Vec<T> {
    fn describe(spec: &mut RouteSpec) {
        T::describe(spec);
    }

    fn emit(self, sink: &mut RouteSink<'_>) {
        for value in self {
            value.emit(sink);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::erased::RoutingProblem;

    /// Two leaves, written by hand exactly as the derive writes them for a
    /// struct. A std type such as `String` is deliberately not routable, so
    /// a second destination needs a second type of its own.
    #[derive(Debug, Clone, PartialEq)]
    struct Leaf(u32);

    #[derive(Debug, Clone, PartialEq)]
    struct Note(String);

    impl RoutedOutput for Leaf {
        fn describe(spec: &mut RouteSpec) {
            spec.payload::<Self>();
        }

        fn emit(self, sink: &mut RouteSink<'_>) {
            sink.push(self);
        }
    }

    impl RoutedOutput for Note {
        fn describe(spec: &mut RouteSpec) {
            spec.payload::<Self>();
        }

        fn emit(self, sink: &mut RouteSink<'_>) {
            sink.push(self);
        }
    }

    /// The payload types of `T`, as the build-time checks see them.
    fn payloads_of<T: RoutedOutput>() -> Vec<&'static str> {
        let mut spec = RouteSpec::new();
        T::describe(&mut spec);
        spec.payloads().iter().map(|(_, name)| *name).collect()
    }

    /// The output index of each token `T` emits into a table where `Leaf` is
    /// output 0 and `Note` is output 1.
    fn emitted<T: RoutedOutput>(value: T) -> Vec<usize> {
        let both = [
            (TypeId::of::<Leaf>(), "Leaf"),
            (TypeId::of::<Note>(), "Note"),
        ];
        let routes = RouteTable::resolve(&both, &both).expect("one place per payload type");
        let mut tokens = Vec::new();
        value.emit(&mut RouteSink::new(&routes, &mut tokens));
        tokens
            .into_iter()
            .map(|(target, _)| match target {
                OutputTarget::Index(index) => index,
                other => panic!("a routed payload is tagged with an index, not {other:?}"),
            })
            .collect()
    }

    #[test]
    fn a_leaf_describes_and_emits_its_own_type() {
        assert_eq!(payloads_of::<Leaf>().len(), 1);
        assert_eq!(emitted(Leaf(1)), [0]);
    }

    #[test]
    fn an_option_emits_zero_or_one_and_describes_both() {
        assert_eq!(payloads_of::<Option<Leaf>>().len(), 1);
        assert!(emitted(None::<Leaf>).is_empty());
        assert_eq!(emitted(Some(Leaf(1))), [0]);
    }

    #[test]
    fn a_vec_is_always_a_batch() {
        assert_eq!(emitted(vec![Leaf(1), Leaf(2), Leaf(3)]), [0, 0, 0]);
    }

    #[test]
    fn either_describes_both_sides_and_emits_one() {
        assert_eq!(payloads_of::<Either<Leaf, Note>>().len(), 2);
        assert_eq!(emitted(Either::<Leaf, Note>::Left(Leaf(1))), [0]);
        assert_eq!(
            emitted(Either::<Leaf, Note>::Right(Note(String::from("x")))),
            [1]
        );
    }

    #[test]
    fn a_repeated_payload_type_is_one_entry() {
        assert_eq!(payloads_of::<Either<Leaf, Leaf>>().len(), 1);
    }

    #[test]
    fn a_payload_without_a_place_is_unroutable() {
        let problem = RouteTable::resolve(&[(TypeId::of::<Leaf>(), "Leaf")], &[]).unwrap_err();
        assert!(matches!(problem, RoutingProblem::Unroutable { type_name } if type_name == "Leaf"));
    }

    #[test]
    fn two_places_of_one_type_are_ambiguous() {
        let problem = RouteTable::resolve(
            &[(TypeId::of::<Leaf>(), "Leaf")],
            &[
                (TypeId::of::<Leaf>(), "Leaf"),
                (TypeId::of::<Leaf>(), "Leaf"),
            ],
        )
        .unwrap_err();
        assert!(matches!(problem, RoutingProblem::Ambiguous { type_name } if type_name == "Leaf"));
    }

    #[test]
    fn a_place_no_payload_reaches_is_unreachable() {
        let problem = RouteTable::resolve(
            &[(TypeId::of::<Leaf>(), "Leaf")],
            &[
                (TypeId::of::<Leaf>(), "Leaf"),
                (TypeId::of::<Note>(), "Note"),
            ],
        )
        .unwrap_err();
        assert!(matches!(problem, RoutingProblem::Unreachable { output } if output == 1));
    }

    #[test]
    #[should_panic(expected = "`describe` did not list")]
    fn emitting_an_undeclared_payload_panics() {
        let routes = RouteTable::resolve(&[], &[]).expect("nothing to route");
        let mut tokens = Vec::new();
        RouteSink::new(&routes, &mut tokens).push(Leaf(1));
    }
}
