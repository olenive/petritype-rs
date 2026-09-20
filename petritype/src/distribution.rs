//! The third output form: the author names the place each token goes to.
//!
//! The positional form matches slots to places by order and the routed form
//! matches payloads to places by type. Both decide everything before the net
//! runs, which is what makes them checkable — and what makes them unable to
//! answer "which of these three *same-typed* places does *this* value belong
//! in?". That question is what a [`Distribution`] answers: a list of typed
//! `(place, token)` pairs, built while the body runs, from the data the body
//! produced.
//!
//! ```
//! use petritype::{Distribution, NetBuilder, TakeEdge};
//!
//! #[derive(Debug, Clone, PartialEq)]
//! struct Ball { color: String }
//!
//! let mut b = NetBuilder::new();
//! let paint = b.place::<String>("Paint");
//! let red = b.place::<Ball>("Red");
//! let blue = b.place::<Ball>("Blue");
//! let other = b.place::<Ball>("Other");
//! b.seed(paint, [String::from("blue")]);
//!
//! b.transition(
//!     "Make a Ball",
//!     (TakeEdge(paint),),
//!     (red, blue, other),
//!     move |color: String| {
//!         let ball = Ball { color };
//!         let target = match ball.color.as_str() {
//!             "red" => red,
//!             "blue" => blue,
//!             _ => other,
//!         };
//!         Distribution::new().to(target, ball)
//!     },
//! );
//! let mut net = b.build()?;
//!
//! net.step()?;
//! assert_eq!(net.count(blue.id()), 1);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Keeping the domain function out of it
//!
//! The body above knows which places exist, which is exactly what DESIGN §4.5
//! says a domain function should not have to know. The
//! [`output_distribution`](OutputDistribution::output_distribution) method
//! splits the two apart: the body computes the value, an *output distribution
//! function* written beside the net decides where it goes, and the pair of
//! them is what the transition is declared with.
//!
//! ```
//! use petritype::{Distribution, NetBuilder, OutputDistribution, TakeEdge};
//!
//! # #[derive(Debug, Clone, PartialEq)]
//! # struct Ball { color: String }
//! /// The domain function: no `PlaceRef` in sight, and unchanged when the
//! /// net is rewired.
//! fn make_a_ball(color: String) -> Ball {
//!     Ball { color }
//! }
//!
//! # let mut b = NetBuilder::new();
//! # let paint = b.place::<String>("Paint");
//! # let red = b.place::<Ball>("Red");
//! # let blue = b.place::<Ball>("Blue");
//! # let other = b.place::<Ball>("Other");
//! # b.seed(paint, [String::from("red")]);
//! b.transition(
//!     "Make a Ball",
//!     (TakeEdge(paint),),
//!     (red, blue, other),
//!     make_a_ball.output_distribution(move |ball: Ball| {
//!         let target = match ball.color.as_str() {
//!             "red" => red,
//!             "blue" => blue,
//!             _ => other,
//!         };
//!         Distribution::new().to(target, ball)
//!     }),
//! );
//! # let mut net = b.build()?;
//! # net.step()?;
//! # assert_eq!(net.count(red.id()), 1);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # What is checked, and when
//!
//! That a token fits the place it is sent to is a compile error, because
//! [`to`](Distribution::to) takes a `PlaceRef<T>` and a `T`. That the place is
//! one of *this transition's* declared outputs cannot be: the pairs are built
//! from data, so nothing static knows which places a firing will name. It is
//! checked by the engine, before any token is written, and a token addressed
//! anywhere else fails the whole firing with a [`DistributionError`]
//! (DESIGN §4.5).
//!
//! The same gap means there is **no build-time reachability check** for this
//! form: a transition may declare an output place that no `Distribution` ever
//! names, and the net still builds. The routed form's
//! [`UnreachableOutput`](crate::BuildError::UnreachableOutput) has no
//! counterpart here, so a diagram drawn from the declaration may show an edge
//! this transition never uses.

use std::fmt;

use crate::body::{IntoOutcome, OutputPlaces, marker};
use crate::erased::{ErasedOutcome, OutputTarget, RouteTable, RoutingProblem};
use crate::ids::{NetId, PlaceId, PlaceRef, TransitionId};
use crate::place::ErasedToken;
use crate::sealed::OutcomeSealed;

/// Tokens addressed to places by name: the output form for decisions that
/// only the data can make.
///
/// Built pair by pair and returned from a body or from an
/// [output distribution function](OutputDistribution). Each pair records the
/// place's id, the [`NetId`] its handle carried, and the token with its type
/// erased; the typed handle in [`to`](Distribution::to) is what makes the
/// token fit the place, so the erasure loses nothing that was checkable.
///
/// Anything a `Vec` of pairs allows is allowed: two tokens to one place, no
/// tokens at all, the same place named twice, several places of the same type.
/// Tokens land in the order they were added.
///
/// ```
/// # use petritype::{Distribution, NetBuilder};
/// # let mut b = NetBuilder::new();
/// # let red = b.place::<u32>("Red");
/// # let blue = b.place::<u32>("Blue");
/// // Chained, by value, when the pairs are known one by one ...
/// let one = Distribution::new().to(red, 1).to(blue, 2);
/// assert_eq!(one.len(), 2);
///
/// // ... or pushed, in place, when a loop decides how many there are.
/// let mut many = Distribution::new();
/// for n in 0..3 {
///     many.push(if n % 2 == 0 { red } else { blue }, n);
/// }
/// assert_eq!(many.len(), 3);
///
/// // Producing nothing is an ordinary outcome, not a failure.
/// assert!(Distribution::new().is_empty());
/// ```
pub struct Distribution {
    /// One entry per token, in the order they will be pushed into their
    /// places. The [`NetId`] travels with the id because a handle from
    /// another net must be caught rather than misread as an index into this
    /// one.
    pairs: Vec<(PlaceId, NetId, ErasedToken)>,
}

impl Distribution {
    /// An empty distribution: the starting point of every chain, and a
    /// perfectly good answer on its own for a firing that produces nothing.
    ///
    /// The one constructor, as `dev-docs/NAMING_CONVENTIONS.md` asks: pairs
    /// are added with [`to`](Distribution::to) or
    /// [`push`](Distribution::push), and there is no second spelling for the
    /// same thing.
    pub fn new() -> Self {
        Distribution { pairs: Vec::new() }
    }

    /// Adds one token for one place and hands the distribution back, so that
    /// pairs chain: `Distribution::new().to(red, ball).to(count, 1)`.
    ///
    /// The handle carries the place's token type, so a token of the wrong
    /// type does not compile. What the handle cannot carry is whether the
    /// place is an output of the transition being declared — see
    /// [`DistributionError`].
    pub fn to<T: Send + Sync + 'static>(mut self, place: PlaceRef<T>, token: T) -> Self {
        self.push(place, token);
        self
    }

    /// [`to`](Distribution::to) for a caller that is not chaining: a loop, a
    /// branch that adds a pair sometimes, or a fan-out whose width is known
    /// only at run time.
    pub fn push<T: Send + Sync + 'static>(&mut self, place: PlaceRef<T>, token: T) {
        self.pairs
            .push((place.id(), place.net(), Box::new(token) as ErasedToken));
    }

    /// How many tokens this distribution will produce. Not how many places it
    /// names: two tokens for one place count as two.
    pub fn len(&self) -> usize {
        self.pairs.len()
    }

    /// Whether this distribution produces no tokens at all.
    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }
}

impl Default for Distribution {
    fn default() -> Self {
        Distribution::new()
    }
}

impl fmt::Debug for Distribution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The tokens are erased, so there is nothing to print of them; what a
        // reader can use is how many there are and where they are addressed.
        f.debug_struct("Distribution")
            .field("pairs", &self.pairs.len())
            .field("places", &Addressed(&self.pairs))
            .finish()
    }
}

/// The place ids of a distribution's pairs, printed as a list.
struct Addressed<'a>(&'a [(PlaceId, NetId, ErasedToken)]);

impl fmt::Debug for Addressed<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.0.iter().map(|(place, _, _)| place))
            .finish()
    }
}

/// A `Distribution` is an outcome in its own right, under its own marker, so
/// that it never competes with the positional impl (tuples only) or the routed
/// one (never a tuple, and `Distribution` is not
/// [`RoutedOutput`](crate::RoutedOutput)).
impl<Outputs: OutputPlaces> OutcomeSealed<Outputs, marker::Distributed> for Distribution {
    /// Nothing to resolve, and nothing that *could* be resolved: which places
    /// a firing names is decided by the data, so the table stays empty and
    /// the check happens at distribution time instead.
    fn routing() -> Result<RouteTable, RoutingProblem> {
        Ok(RouteTable::empty())
    }

    fn into_outcome(self, _routes: &RouteTable) -> ErasedOutcome {
        ErasedOutcome::Ok(
            self.pairs
                .into_iter()
                .map(|(place, net, token)| (OutputTarget::Place(place, net), token))
                .collect(),
        )
    }
}

impl<Outputs: OutputPlaces> IntoOutcome<Outputs, marker::Distributed> for Distribution {}

/// Composes an output distribution function after a body, so that the body
/// stays a plain domain function.
///
/// Implemented for every type, without bounds: what may be a body and what may
/// be a distribution function is decided by
/// [`NetBuilder::transition`](crate::NetBuilder::transition), where the edges
/// and the output places are known, and saying it twice would only move the
/// error message further from the mistake.
///
/// DESIGN §5 writes this as an option on the transition —
/// `b.transition(..).output_distribution(f)` — but by then the body's return
/// type has been erased, and the whole point is that *`f`'s* return type
/// decides the output form. So it wraps the body instead, before
/// `transition()` ever sees it.
///
/// ```
/// # use petritype::{Distribution, NetBuilder, OutputDistribution, TakeEdge};
/// # let mut b = NetBuilder::new();
/// # let inbox = b.place::<u32>("Inbox");
/// # let small = b.place::<u32>("Small");
/// # let large = b.place::<u32>("Large");
/// fn double(n: u32) -> u32 {
///     n * 2
/// }
///
/// b.transition(
///     "Double",
///     (TakeEdge(inbox),),
///     (small, large),
///     double.output_distribution(move |n: u32| {
///         Distribution::new().to(if n < 10 { small } else { large }, n)
///     }),
/// );
/// ```
pub trait OutputDistribution: Sized {
    /// Runs `f` on whatever this body returns, and makes *`f`'s* return value
    /// the transition's outputs.
    ///
    /// `f` sees the body's result and nothing else — not the inputs, not the
    /// net — and it runs in stage two, with the body, so in concurrent mode it
    /// runs wherever the body ran. It may return any output form: a
    /// [`Distribution`], a positional tuple, or a value
    /// [routed](crate::RoutedOutput) by its type.
    ///
    /// A body that returns `Result<R, E>` is composed too: `f` takes the `R`,
    /// and an `Err` short-circuits to a failed firing without calling `f` at
    /// all.
    fn output_distribution<G>(self, f: G) -> WithOutputDistribution<Self, G> {
        WithOutputDistribution {
            body: self,
            distribute: f,
        }
    }
}

impl<F> OutputDistribution for F {}

/// A body with an [output distribution function](OutputDistribution) composed
/// after it: what
/// [`output_distribution`](OutputDistribution::output_distribution) returns,
/// and itself a transition body.
///
/// The pair is a body of the same arity as the one inside it — the wrapper
/// changes only what the *return* value means — so
/// [`NetBuilder::transition`](crate::NetBuilder::transition) takes it wherever
/// it takes a closure. The [`TransitionBody`](crate::TransitionBody) impls
/// live beside the plain ones, so that one arity macro writes both.
pub struct WithOutputDistribution<F, G> {
    /// The domain function, called with one argument per input edge.
    pub(crate) body: F,
    /// The author's router, called with what `body` returned.
    pub(crate) distribute: G,
}

impl<F, G> fmt::Debug for WithOutputDistribution<F, G> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Two closures: nothing to print but the shape.
        f.debug_struct("WithOutputDistribution")
            .finish_non_exhaustive()
    }
}

/// A [`Distribution`] the engine could not apply, which fails the firing that
/// produced it.
///
/// Both variants mean the same kind of mistake: a pair whose place is not one
/// this transition may write to. They are found while the firing's targets
/// are resolved, *before* any of its tokens are written, so a distribution
/// with a good pair and a bad one writes neither (DESIGN §6.2). The firing
/// then fails exactly as a body returning `Err`
/// does: the consumed tokens are lost — or put back, for a transition that
/// [restores them](crate::TransitionBuilder::restore_tokens_on_failure) — an
/// [`Event::Failed`](crate::Event::Failed) records them, and the error is the
/// [`source`](std::error::Error::source) of the
/// [`FiringFailed`](crate::FiringFailed) that comes back from
/// [`step`](crate::Net::step) or [`fire`](crate::Net::fire).
///
/// Neither can be a compile error, which is the price of this form: the pairs
/// are built from data, so which places a firing names is not known until it
/// has run.
///
/// # Names and ids
///
/// An error escapes the scope the net lives in: a `?` in `main` prints the
/// message and exits the process, and by then an id alone is useless to the
/// person reading the log. These messages are read by humans far more often
/// than they are matched on by programs — ids are the programs' side, and
/// [`Event`](crate::Event)s already serve it while the net is alive. So both
/// variants carry a name beside each id: the names go in the `Display`
/// message, and the ids stay as the fields a program matches on.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DistributionError {
    /// A token was addressed to a place that is not one of the transition's
    /// declared output places — including any place of another net, whose
    /// handle carries a different [`NetId`].
    ///
    /// Add the place to the transition's output tuple, or send the token
    /// somewhere the declaration already allows. The output tuple is what a
    /// diagram draws, so a firing is not allowed to write outside it.
    #[error(
        "transition {transition_name:?} ({transition:?}) distributed a token to place \
         {name:?} ({place:?}), which is not one of its outputs"
    )]
    NotAnOutput {
        /// The transition whose firing it was.
        transition: TransitionId,
        /// The transition's name, for the message; the id is the field to
        /// match on.
        transition_name: String,
        /// The place the token was addressed to.
        place: PlaceId,
        /// The place's name, for the message; the id is the field to match
        /// on. `"<another net>"` when the handle was stamped with another
        /// net, whose places this net cannot name — the id is then that
        /// net's, and means nothing here either.
        name: String,
    },
    /// A token of the wrong type was addressed to one of the transition's own
    /// output places.
    ///
    /// [`Distribution::to`](Distribution::to) takes a `PlaceRef<T>` and a `T`,
    /// so this is unreachable through the typed API; the check is the
    /// engine's defence against a handle built for another net that happens
    /// to share an index with a place of this one.
    #[error(
        "transition {transition_name:?} ({transition:?}) distributed a token to place \
         {name:?} ({place:?}), which holds {expected}"
    )]
    WrongType {
        /// The transition whose firing it was.
        transition: TransitionId,
        /// The transition's name, for the message; the id is the field to
        /// match on.
        transition_name: String,
        /// The place the token was addressed to.
        place: PlaceId,
        /// The place's name, for the message; the id is the field to match
        /// on. Always a real name here: this check is reached only for a
        /// place the transition declared as an output.
        name: String,
        /// The type that place holds.
        expected: &'static str,
    },
}

/// The stand-in name in a [`NotAnOutput`](DistributionError::NotAnOutput) for
/// a place of another net.
///
/// A [`PlaceRef`] stamped with a different [`NetId`] names nothing here, and
/// the place at that index in *this* net is a different place entirely, so
/// naming it would be a lie. Written like the engine's other stand-ins —
/// `"<erased>"` for a type a `dyn Any` cannot name — so that a reader can see
/// at a glance that it is not a name the author wrote.
pub(crate) const FOREIGN_PLACE: &str = "<another net>";

#[cfg(test)]
mod tests {
    use super::*;

    fn place<T>(index: u32) -> PlaceRef<T> {
        PlaceRef::new(NetId(0), PlaceId(index))
    }

    /// The pairs of a distribution, as the engine reads them.
    fn targets(distribution: Distribution) -> Vec<OutputTarget> {
        let outcome = OutcomeSealed::<(PlaceRef<u32>,), marker::Distributed>::into_outcome(
            distribution,
            &RouteTable::empty(),
        );
        match outcome {
            ErasedOutcome::Ok(tokens) => tokens.into_iter().map(|(target, _)| target).collect(),
            ErasedOutcome::Err(error) => panic!("a distribution is never a failure: {error}"),
        }
    }

    #[test]
    fn pairs_keep_the_order_they_were_added_in() {
        let first = place::<u32>(0);
        let second = place::<u32>(1);
        let distribution = Distribution::new().to(second, 1).to(first, 2).to(second, 3);
        assert_eq!(distribution.len(), 3);
        assert_eq!(
            targets(distribution),
            [
                OutputTarget::Place(PlaceId(1), NetId(0)),
                OutputTarget::Place(PlaceId(0), NetId(0)),
                OutputTarget::Place(PlaceId(1), NetId(0)),
            ]
        );
    }

    #[test]
    fn an_empty_distribution_produces_nothing() {
        assert!(Distribution::new().is_empty());
        assert!(targets(Distribution::default()).is_empty());
    }

    #[test]
    fn push_and_to_add_the_same_pair() {
        let target = place::<String>(4);
        let mut pushed = Distribution::new();
        pushed.push(target, String::from("x"));
        assert_eq!(
            targets(pushed),
            targets(Distribution::new().to(target, String::from("x")))
        );
    }

    #[test]
    fn a_handle_carries_the_net_it_came_from() {
        let mine = PlaceRef::<u32>::new(NetId(1), PlaceId(0));
        let theirs = PlaceRef::<u32>::new(NetId(2), PlaceId(0));
        assert_ne!(
            targets(Distribution::new().to(mine, 1)),
            targets(Distribution::new().to(theirs, 1)),
            "the stamp is what tells two nets' place #0 apart"
        );
    }

    #[test]
    fn the_debug_shows_how_many_pairs_and_where() {
        let red = place::<u32>(2);
        let blue = place::<u32>(5);
        let printed = format!("{:?}", Distribution::new().to(red, 1).to(blue, 2));
        assert!(printed.contains("pairs: 2"), "{printed}");
        assert!(printed.contains("PlaceId(2)"), "{printed}");
    }
}
