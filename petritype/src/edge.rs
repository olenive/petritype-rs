//! Input edges: the eight kinds, and what each one hands to the body.
//!
//! An input edge says *how* a transition reads a place, which fixes the type
//! of the matching body parameter:
//!
//! | edge | consumes | body parameter | presence required |
//! |---|---|---|---|
//! | [`TakeEdge<T>`] | the front token | `T` | yes |
//! | [`TakeAllEdge<T>`] | every token | `Vec<T>` | yes |
//! | [`SnapshotEdge<T>`] | nothing (clones the front token) | `T` | yes |
//! | [`MutateEdge<T>`] | nothing (borrows the front token) | `&mut T` | yes |
//! | [`TakeWhereEdge<T>`] | the oldest token a [`Filter`] accepts | `T` | a token the filter accepts |
//! | [`MutateWhereEdge<T>`] | nothing (borrows the oldest token a [`Filter`] accepts) | `&mut T` | a token the filter accepts |
//! | [`DrainEdge<T>`] | every token, zero or more | `Vec<T>` | no |
//! | [`SurveyEdge<T>`] | nothing (clones every token, zero or more) | `Vec<T>` | no |
//!
//! The first four require the place to be non-empty for the transition to be
//! [fireable](crate::Net::fireable), including the two that do not consume;
//! the two *keyed* edges require it to hold a token their filter accepts
//! (DESIGN §8.3). The last two never hold the transition back: they read
//! whatever the place holds, possibly nothing, which is right for a place a
//! transition only needs to look at when something is there — the
//! promotions running, the subscribers to notify — and saves the "empty
//! twin", a second transition guarded on the place being empty
//! ([`EdgeKind::requires_presence`], DESIGN §4.4). One transition may read
//! one place through one edge only: two edges into the same place are a
//! [`BuildError::DuplicateInputPlace`](crate::BuildError::DuplicateInputPlace).
//!
//! A fifth kind of input is not an edge at all: [`Param<T>`](crate::Param)
//! reads a value off the net rather than out of a place (DESIGN §10). It
//! implements the same [`InputEdge`] trait, because that trait is really "one
//! input position of a transition", and it is the only implementor that names
//! no place.

use std::any::TypeId;
use std::fmt;

use crate::arity::for_each_arity;
use crate::erased::ErasedInput;
use crate::ids::{NetId, PlaceId, PlaceRef, TransitionId};
use crate::keys::Filter;
use crate::sealed::{EdgeSealed, EdgeTupleSealed};

/// Which of the eight input edge kinds an edge is, with its type forgotten.
///
/// This is the form the net stores and the read API reports
/// ([`TransitionInfo::inputs`](crate::TransitionInfo::inputs)); the typed
/// form is [`TakeEdge`] and the other seven structs. The filter of a keyed
/// kind is beside it, not in it, so that the kind stays `Copy`:
/// [`TransitionInfo::filters`](crate::TransitionInfo::filters) and
/// [`EdgeInfo::Input`]'s `filter`.
///
/// With the `serde` feature it is `Serialize` (never `Deserialize`) as a
/// snake_case string — `"take"`, `"take_all"`, `"snapshot"`, `"mutate"`,
/// `"take_where"`, `"mutate_where"`, `"drain"`, `"survey"`, the protocol's
/// names (PROTOCOL.md §5.1) — for logs and tests.
///
/// `#[non_exhaustive]` since the two keyed kinds were added (slice 19):
/// match with a `_` arm, which a reader of the protocol fills by drawing a
/// plain edge. The two kinds that accept an empty place came after them
/// (slice 22) and needed no other change of shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[non_exhaustive]
pub enum EdgeKind {
    /// [`TakeEdge`]: consumes the front token.
    Take,
    /// [`TakeAllEdge`]: consumes every token.
    TakeAll,
    /// [`SnapshotEdge`]: reads a clone of the front token.
    Snapshot,
    /// [`MutateEdge`]: borrows the front token and may change it.
    Mutate,
    /// [`TakeWhereEdge`]: consumes the oldest token its filter accepts.
    TakeWhere,
    /// [`MutateWhereEdge`]: borrows the oldest token its filter accepts and
    /// may change it.
    MutateWhere,
    /// [`DrainEdge`]: consumes every token, zero or more.
    Drain,
    /// [`SurveyEdge`]: reads a clone of every token, zero or more.
    Survey,
}

impl EdgeKind {
    /// Whether firing through this edge removes tokens from its place:
    /// [`Take`](EdgeKind::Take), [`TakeAll`](EdgeKind::TakeAll),
    /// [`TakeWhere`](EdgeKind::TakeWhere) and [`Drain`](EdgeKind::Drain).
    ///
    /// The reading kinds leave the marking alone, which is what makes them
    /// safe to point at a configuration place that every transition reads.
    /// A `Drain` removes every token there is, which may be none: it
    /// consumes, but a firing is not guaranteed to take anything through
    /// it, which is why the termination checks do not count it
    /// ([`NetBuilder::assert_no_source_transitions`](crate::NetBuilder::assert_no_source_transitions)).
    pub fn consumes(self) -> bool {
        matches!(
            self,
            EdgeKind::Take | EdgeKind::TakeAll | EdgeKind::TakeWhere | EdgeKind::Drain
        )
    }

    /// Whether this edge holds its transition back until its place has
    /// something for it: true for the six kinds that need a token present
    /// (a token their filter accepts, for a keyed one), false for
    /// [`Drain`](EdgeKind::Drain) and [`Survey`](EdgeKind::Survey), which
    /// read every token there is, zero or more, and so are satisfied by an
    /// empty place.
    ///
    /// [`Net::fireable`](crate::Net::fireable) skips the presence check for
    /// an edge that does not require it; the transition's other edges and
    /// its guard still apply. A viewer can mark such an edge so that a
    /// reader tells it from a [`TakeAll`](EdgeKind::TakeAll) or a
    /// [`Snapshot`](EdgeKind::Snapshot).
    pub fn requires_presence(self) -> bool {
        !matches!(self, EdgeKind::Drain | EdgeKind::Survey)
    }

    /// Whether every firing through this edge takes at least one token out
    /// of its place: the [consuming](EdgeKind::consumes) kinds that
    /// [require presence](EdgeKind::requires_presence) —
    /// [`Take`](EdgeKind::Take), [`TakeAll`](EdgeKind::TakeAll) and
    /// [`TakeWhere`](EdgeKind::TakeWhere), but not
    /// [`Drain`](EdgeKind::Drain), which may take nothing.
    ///
    /// What the termination proof counts: the source rule asks every
    /// transition for one such edge, and the acyclicity walk follows only
    /// these into a transition (DESIGN §12).
    pub(crate) fn always_consumes(self) -> bool {
        self.consumes() && self.requires_presence()
    }

    /// Whether this edge chooses its token with a [`Filter`]:
    /// [`TakeWhere`](EdgeKind::TakeWhere) and
    /// [`MutateWhere`](EdgeKind::MutateWhere). Exactly these carry a filter
    /// in [`TransitionInfo::filters`](crate::TransitionInfo::filters) and
    /// [`EdgeInfo::Input`].
    pub fn is_keyed(self) -> bool {
        matches!(self, EdgeKind::TakeWhere | EdgeKind::MutateWhere)
    }

    /// Whether this edge hands its body a borrow into the place, which ties
    /// the firing to the net: [`Mutate`](EdgeKind::Mutate) and
    /// [`MutateWhere`](EdgeKind::MutateWhere).
    pub(crate) fn mutates(self) -> bool {
        matches!(self, EdgeKind::Mutate | EdgeKind::MutateWhere)
    }
}

/// What one input position of a transition is, as the tuple of inputs
/// describes itself to the builder.
///
/// Two variants because a transition's inputs are two different things
/// (DESIGN §10): edges, which read places and are what a diagram draws, and
/// [parameters](crate::Param), which read a value off the net and are not
/// nodes at all. The builder splits them — the edges become
/// [`TransitionInfo::inputs`](crate::TransitionInfo::inputs), the parameters
/// [`TransitionInfo::params`](crate::TransitionInfo::params) — and keeps the
/// tuple order beside them so that the body's arguments can be rebuilt in it.
///
/// [`EdgeKind`] deliberately gains no variant for a parameter: a parameter is
/// not a *kind of edge*, and everything that reads edges — fireability,
/// [`Net::edges`](crate::Net::edges), the acyclicity and source checks — must
/// go on seeing places only.
///
/// Nameable only inside the crate: it is the argument of a sealed trait's
/// method, like [`ErasedInput`] and the rest of the erased plumbing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputSpec {
    /// An input edge: which place, how it is read, and the net the handle
    /// carried, which the builder checks before recording anything.
    Edge {
        /// The place this edge reads.
        place: PlaceId,
        /// How it reads it.
        kind: EdgeKind,
        /// The net the place handle belongs to.
        net: NetId,
        /// The filter of a [keyed](EdgeKind::is_keyed) edge; `None` for the
        /// others.
        filter: Option<Filter>,
    },
    /// A [`Param<T>`](crate::Param): no place, no net, just the type whose
    /// value the body is handed a clone of.
    Param {
        /// `TypeId::of::<T>()`, the key the value is stored under.
        type_id: TypeId,
        /// `std::any::type_name::<T>()`, for
        /// [`TransitionInfo::params`](crate::TransitionInfo::params) and for
        /// [`BuildError::MissingParam`](crate::BuildError::MissingParam).
        type_name: &'static str,
    },
}

/// One edge of the net, as a diagram draws it.
///
/// This is what [`Net::edges`](crate::Net::edges) reports: the same wiring
/// [`Net::transitions`](crate::Net::transitions) carries per transition, but
/// one edge at a time, which is the form a layout, a DOT file or a set of
/// view entities is built from.
///
/// The two directions are two variants rather than a `from`/`to` pair
/// because they carry different things: an input edge carries the
/// [kind](EdgeKind) saying *how* the transition reads the place, and an
/// output edge carries the position of its slot among the transition's
/// outputs, which is what a positional body returns into.
///
/// With the `serde` feature it is `Serialize` (never `Deserialize`) in the
/// core's id form, for logs and tests, tagged by direction under `"edge"`
/// (`"kind"` is taken by the input edge's [`EdgeKind`]):
/// `{"edge":"input","place":0,"transition":1,"kind":"take"}` and
/// `{"edge":"output","transition":1,"place":2,"slot":0}`; a keyed edge
/// carries its filter, `{"edge":"input",..,"kind":"take_where","filter":
/// {"clauses":[{"key":"class","values":["Bird"]}]}}`, and no other edge
/// has the field. Shapes keyed by name, for a wire protocol, live outside
/// this crate.
///
/// Not `Copy` since slice 19: an input edge may carry a [`Filter`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(tag = "edge", rename_all = "snake_case"))]
#[non_exhaustive]
pub enum EdgeInfo {
    /// place → transition, with how the transition reads it.
    Input {
        /// The place read.
        place: PlaceId,
        /// The transition reading it.
        transition: TransitionId,
        /// Which of the eight ways it is read; only the
        /// [consuming](EdgeKind::consumes) kinds move tokens.
        kind: EdgeKind,
        /// The filter of a [keyed](EdgeKind::is_keyed) edge, as declared
        /// (a [`Clause::IsParam`](crate::Clause::IsParam) unevaluated);
        /// `None` for every other kind.
        #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
        filter: Option<Filter>,
    },
    /// transition → place, positional index `slot` among the transition's
    /// outputs.
    Output {
        /// The transition writing.
        transition: TransitionId,
        /// The place written to.
        place: PlaceId,
        /// This place's index in the transition's declared outputs, which is
        /// the slot a positional body returns into. Counted from zero.
        slot: usize,
    },
}

/// Consumes the front token of a place; the body parameter is `T`.
///
/// Places are FIFO, so the front token is the oldest one. A transition may
/// hold only one input edge into a given place, so two `TakeEdge`s do not
/// take two tokens: that is a
/// [`BuildError::DuplicateInputPlace`](crate::BuildError::DuplicateInputPlace),
/// and taking several tokens at once is what [`TakeAllEdge`] is for.
///
/// ```
/// # use petritype::{NetBuilder, TakeEdge};
/// # let mut b = NetBuilder::new();
/// let inbox = b.place::<u32>("Inbox");
/// let out = b.place::<u32>("Out");
/// b.transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,));
/// ```
pub struct TakeEdge<T>(
    /// The place to take from.
    pub PlaceRef<T>,
);

/// Consumes every token of a place at once; the body parameter is `Vec<T>`,
/// oldest first.
///
/// The transition is fireable when the place holds at least one token, so a
/// batch is "everything that has arrived by now", not a fixed size. A
/// [`DrainEdge`] takes the same batch and is fireable on an empty place too,
/// handing the body an empty `Vec`.
pub struct TakeAllEdge<T>(
    /// The place to drain.
    pub PlaceRef<T>,
);

/// Reads the front token of a place without consuming it; the body parameter
/// is `T`, a clone.
///
/// The clone is taken when the firing starts, so the body sees a value that
/// cannot change under it even if the firing runs on another thread. A place
/// declared with
/// [`place_no_clone`](crate::NetBuilder::place_no_clone) cannot be the source
/// of a snapshot edge: that is a
/// [`BuildError::SnapshotOfNoClonePlace`](crate::BuildError::SnapshotOfNoClonePlace).
pub struct SnapshotEdge<T>(
    /// The place to read.
    pub PlaceRef<T>,
);

/// Borrows the front token of a place and may change it in place; the body
/// parameter is `&mut T`.
///
/// The token keeps its id and its position in the queue, so a view refreshes
/// its label rather than destroying and recreating it: one
/// [`Event::Mutated`](crate::Event::Mutated) per mutating edge is emitted
/// before the firing's [`Event::Fired`](crate::Event::Fired), and a body that
/// mutates and then fails has still mutated. Because the body holds a borrow
/// into the net, a transition with a mutating edge is not *detachable*: it
/// runs inline instead of being handed to a worker.
///
/// Several mutating edges in one transition hand the body several `&mut`s at
/// once, which is sound because they point at different places: two input
/// edges into one place are a
/// [`BuildError::DuplicateInputPlace`](crate::BuildError::DuplicateInputPlace).
pub struct MutateEdge<T>(
    /// The place whose front token to borrow.
    pub PlaceRef<T>,
);

/// Consumes the oldest token of a place that a [`Filter`] accepts; the body
/// parameter is `T`.
///
/// A *keyed* edge (DESIGN §8.3): the transition is
/// [fireable](crate::Net::fireable) only while the place holds a token whose
/// [keys](crate::NetBuilder::keys) pass every clause of the filter (and its
/// guard, asked after, passes), and a firing takes the first such token in
/// queue order — the oldest, in a place nothing has been put back into —
/// wherever it stands. Every other token stays where it is, with its id: no
/// [`Removed`](crate::Event::Removed) or [`Added`](crate::Event::Added) is
/// emitted for them, and the firing's `consumed` names the one token taken.
/// This is what "take everything, keep one, put the rest back" did at the
/// cost of a fresh id for every token it touched.
///
/// The filter is asked of this one place, so its
/// [`places`](Filter::places) and [`prefix`](Filter::prefix) are not
/// consulted; a [`Clause::IsParam`](crate::Clause::IsParam) reads the net's
/// parameter of that wire name each time fireability is asked. A filter
/// without clauses accepts every token, and the edge then takes the front
/// one, as a [`TakeEdge`] would. The place must declare keys
/// ([`BuildError::KeyedEdgeOnUnkeyedPlace`](crate::BuildError::KeyedEdgeOnUnkeyedPlace)):
/// a keyed edge chooses by them.
///
/// A transition that
/// [restores its inputs on failure](crate::TransitionBuilder::restore_tokens_on_failure)
/// puts the token back at the **front** of the place, as every restore does,
/// with its id: the id is kept, the position is not.
///
/// ```
/// # use petritype::{Filter, KeyValue, NetBuilder, TakeWhereEdge};
/// let mut b = NetBuilder::new();
/// let forest = b.place::<(String, String)>("Forest");
/// let flying = b.place::<(String, String)>("Flying");
/// b.keys(forest, ["class"], |(_, class): &(String, String)| [class.as_str().into()]);
/// b.seed(forest, [("Fox".into(), "Mammal".into()), ("Robin".into(), "Bird".into())]);
/// b.transition(
///     "Fly",
///     (TakeWhereEdge(forest, Filter::new().key_in("class", ["Bird"])),),
///     (flying,),
///     |bird: (String, String)| (bird,),
/// );
/// let mut net = b.build()?;
/// let (fox, _) = net.tokens(forest.id()).next().unwrap();
///
/// net.step()?;
/// assert_eq!(net.count(flying.id()), 1, "the robin flew");
/// assert_eq!(net.tokens(forest.id()).next().unwrap().0, fox, "the fox stayed, same id");
/// assert_eq!(net.step()?, None, "no bird left");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct TakeWhereEdge<T>(
    /// The place to take from.
    pub PlaceRef<T>,
    /// Which tokens it may take.
    pub Filter,
);

/// Borrows the oldest token of a place that a [`Filter`] accepts and may
/// change it in place; the body parameter is `&mut T`.
///
/// A [`MutateEdge`] that chooses its token as a [`TakeWhereEdge`] does: the
/// transition is fireable only while the place holds a token the filter
/// accepts, and the body is handed the first such token in queue order,
/// where it stands. The token keeps its id and its position; the firing
/// reports it in an [`Event::Mutated`](crate::Event::Mutated), and its keys
/// are extracted again afterwards, so a body that changes what the filter
/// reads changes what the next firing finds. A transition with one is not
/// detachable, as with a `MutateEdge`. The place must declare keys
/// ([`BuildError::KeyedEdgeOnUnkeyedPlace`](crate::BuildError::KeyedEdgeOnUnkeyedPlace)).
pub struct MutateWhereEdge<T>(
    /// The place whose token to borrow.
    pub PlaceRef<T>,
    /// Which tokens it may borrow.
    pub Filter,
);

/// Consumes every token of a place, zero or more; the body parameter is
/// `Vec<T>`, oldest first, and may be empty.
///
/// A [`TakeAllEdge`] that never holds its transition back: the edge is
/// satisfied by an empty place ([`EdgeKind::requires_presence`] is false),
/// so the transition is [fireable](crate::Net::fireable) whenever its other
/// edges and its guard say so, and the body is handed whatever the place
/// held when the firing started — in Petri-net terms a *reset arc*. It is
/// for a place the transition must empty when it fires but must not wait
/// on: "end every promotion running when the day closes", where none
/// running is a perfectly good answer. Without it that is two transitions,
/// one with the edge and one guarded on the place being empty.
///
/// Firing takes the tokens as a `TakeAllEdge` does: the firing's
/// `consumed` ([`Event::Fired`](crate::Event::Fired)) names each of them,
/// which may be none from this place, and a transition that
/// [restores its inputs on failure](crate::TransitionBuilder::restore_tokens_on_failure)
/// puts them back at the front of the place, oldest first, with their ids.
///
/// Because it may consume nothing, it does not count as consuming for
/// [`assert_no_source_transitions`](crate::NetBuilder::assert_no_source_transitions)
/// or [`assert_acyclic`](crate::NetBuilder::assert_acyclic): a transition
/// whose only input is a `DrainEdge` fires for ever on an empty place.
///
/// ```
/// # use petritype::{DrainEdge, NetBuilder, TakeEdge};
/// let mut b = NetBuilder::new();
/// let day_end = b.place::<String>("Day End");
/// let promotions = b.place::<String>("Promotions");
/// let expired = b.place::<String>("Expired");
/// b.transition(
///     "Close Day",
///     (TakeEdge(day_end), DrainEdge(promotions)),
///     (expired,),
///     |_close: String, running: Vec<String>| {
///         (running.into_iter().map(|p| format!("{p} expired")).collect::<Vec<_>>(),)
///     },
/// );
/// let mut net = b.build()?;
///
/// net.put(day_end, "close".to_owned());
/// assert!(net.step()?.is_some(), "fires with no promotion running");
/// assert_eq!(net.count(expired.id()), 0);
///
/// net.extend(promotions, ["10% off".to_owned(), "Free shipping".to_owned()]);
/// net.put(day_end, "close".to_owned());
/// net.step()?;
/// assert_eq!(net.count(promotions.id()), 0, "every promotion ended");
/// assert_eq!(net.count(expired.id()), 2);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct DrainEdge<T>(
    /// The place to empty.
    pub PlaceRef<T>,
);

/// Reads every token of a place without consuming any, zero or more; the
/// body parameter is `Vec<T>`, clones, oldest first, and may be empty.
///
/// A read that never holds its transition back: the edge is satisfied by an
/// empty place ([`EdgeKind::requires_presence`] is false), so the
/// transition is [fireable](crate::Net::fireable) whenever its other edges
/// and its guard say so, and the body is handed a clone of whatever the
/// place held when the firing started. It is a pure data dependency, for a
/// place the transition only needs to look at when something is there:
/// "apply whatever promotions are running" to an order, where none running
/// is a perfectly good answer.
///
/// The clones are taken when the firing starts, as a [`SnapshotEdge`]'s
/// is, so the body sees values that cannot change under it, and nothing
/// moves: no token leaves the place, the firing's `consumed` names none of
/// them, and a transition whose reads are snapshots and surveys stays
/// *detachable* — its firing may run on another thread. A place declared
/// with [`place_no_clone`](crate::NetBuilder::place_no_clone), a
/// [nested](crate::NetBuilder::place_nested) one among them, cannot be the
/// source of a survey edge: that is a
/// [`BuildError::SnapshotOfNoClonePlace`](crate::BuildError::SnapshotOfNoClonePlace).
///
/// ```
/// # use petritype::{NetBuilder, SurveyEdge, TakeEdge};
/// let mut b = NetBuilder::new();
/// let orders = b.place::<String>("Orders");
/// let promotions = b.place::<String>("Promotions");
/// let priced = b.place::<String>("Priced");
/// let discounts = b.place::<String>("Discounts Applied");
/// b.transition(
///     "Price Order",
///     (TakeEdge(orders), SurveyEdge(promotions)),
///     (priced, discounts),
///     |order: String, running: Vec<String>| {
///         let applied: Vec<String> = running.iter().map(|p| format!("{p} on {order}")).collect();
///         (order, applied)
///     },
/// );
/// let mut net = b.build()?;
///
/// net.put(orders, "Order 1".to_owned());
/// assert!(net.step()?.is_some(), "fires with no promotion running");
/// assert_eq!(net.count(discounts.id()), 0);
///
/// net.put(promotions, "10% off".to_owned());
/// net.put(orders, "Order 2".to_owned());
/// net.step()?;
/// assert_eq!(net.count(discounts.id()), 1);
/// assert_eq!(net.count(promotions.id()), 1, "the promotion is still running");
/// assert!(net.fireable().is_empty(), "no order, whatever the promotions");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct SurveyEdge<T>(
    /// The place to read.
    pub PlaceRef<T>,
);

/// One input position of a transition.
///
/// The name says "edge" because that is what an input almost always is: one
/// of the eight kinds in this module, reading one place. The ninth
/// implementor is [`Param<T>`](crate::Param), which reads a value off the net
/// instead (DESIGN §10) — no place, no node in the diagram — and is here
/// because the tuple of inputs is also the list of the body's parameters,
/// and a parameter occupies one of those positions like anything else.
///
/// Sealed: the nine structs are the only implementors, and the bound exists
/// so that a transition's inputs are checked at compile time. What each one
/// hands the body is the table at the top of this module, plus `T` for a
/// `Param<T>`. The engine half — how an input describes itself to the builder
/// and how its argument is taken out — is `EdgeSealed`, so that this trait
/// carries nothing a place edge and a parameter cannot both answer.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not an input edge",
    label = "not an input edge",
    note = "an input must say how it reads its place: `TakeEdge(p)`, `TakeAllEdge(p)`, \
            `SnapshotEdge(p)`, `MutateEdge(p)`, by a filter `TakeWhereEdge(p, f)` or \
            `MutateWhereEdge(p, f)`, or every token, possibly none, `DrainEdge(p)` or \
            `SurveyEdge(p)`; `Param::<T>::new()` is the input that reads no place"
)]
pub trait InputEdge: EdgeSealed {}

/// Moves the token out of an input, leaving [`ErasedInput::Taken`] behind.
///
/// The `expect`s cannot fire: the engine builds the variant from the edge's
/// own kind, and the token's type was fixed by the `PlaceRef<T>` inside the
/// edge. (The runtime type check in
/// [`NetError::TypeMismatch`](crate::NetError::TypeMismatch) is for the
/// erased outside-the-net route, not for this one.)
pub(crate) fn owned_token<T: 'static>(input: &mut ErasedInput<'_>) -> T {
    match std::mem::replace(input, ErasedInput::Taken) {
        ErasedInput::Token(token) => *token
            .downcast::<T>()
            .expect("an input edge's token has the type of its place"),
        other => panic!("expected one token for this edge, got {other:?}"),
    }
}

/// Moves every token out of an input, oldest first, leaving
/// [`ErasedInput::Taken`] behind: what a [`TakeAllEdge`] and a [`DrainEdge`]
/// took, or the clones a [`SurveyEdge`] made. The `expect`s cannot fire, as
/// in [`owned_token`].
pub(crate) fn owned_tokens<T: 'static>(input: &mut ErasedInput<'_>) -> Vec<T> {
    match std::mem::replace(input, ErasedInput::Taken) {
        ErasedInput::Tokens(tokens) => tokens
            .into_iter()
            .map(|token| {
                *token
                    .downcast::<T>()
                    .expect("an input edge's token has the type of its place")
            })
            .collect(),
        other => panic!("expected every token for this edge, got {other:?}"),
    }
}

/// The `spec` of a place edge, written once: the handle's place and net, and
/// the kind this edge type stands for; a keyed edge's filter, cloned, when
/// the kind is followed by `keyed`.
macro_rules! edge_spec {
    ($kind:expr) => {
        fn spec(&self) -> InputSpec {
            InputSpec::Edge {
                place: self.0.id(),
                kind: $kind,
                net: self.0.net(),
                filter: None,
            }
        }
    };
    ($kind:expr, keyed) => {
        fn spec(&self) -> InputSpec {
            InputSpec::Edge {
                place: self.0.id(),
                kind: $kind,
                net: self.0.net(),
                filter: Some(self.1.clone()),
            }
        }
    };
}

impl<T: Send + Sync + 'static> EdgeSealed for TakeEdge<T> {
    type Arg<'a> = T;

    fn extract(input: &mut ErasedInput<'_>) -> T {
        owned_token::<T>(input)
    }

    edge_spec!(EdgeKind::Take);
}

impl<T: Send + Sync + 'static> InputEdge for TakeEdge<T> {}

impl<T: Send + Sync + 'static> EdgeSealed for TakeAllEdge<T> {
    type Arg<'a> = Vec<T>;

    fn extract(input: &mut ErasedInput<'_>) -> Vec<T> {
        owned_tokens::<T>(input)
    }

    edge_spec!(EdgeKind::TakeAll);
}

impl<T: Send + Sync + 'static> InputEdge for TakeAllEdge<T> {}

/// The `Clone` bound is the compile-time half of what a snapshot needs; the
/// other half — that the *place* recorded a cloner — is checked at
/// [`build`](crate::NetBuilder::build), because
/// [`place_no_clone`](crate::NetBuilder::place_no_clone) can hold a type that
/// happens to be `Clone`.
impl<T: Clone + Send + Sync + 'static> EdgeSealed for SnapshotEdge<T> {
    type Arg<'a> = T;

    fn extract(input: &mut ErasedInput<'_>) -> T {
        owned_token::<T>(input)
    }

    edge_spec!(EdgeKind::Snapshot);
}

impl<T: Clone + Send + Sync + 'static> InputEdge for SnapshotEdge<T> {}

impl<T: Send + Sync + 'static> EdgeSealed for MutateEdge<T> {
    /// The one kind whose parameter borrows: the token stays in its place and
    /// the body writes through the reference.
    type Arg<'a> = &'a mut T;

    fn extract<'a>(input: &'a mut ErasedInput<'_>) -> &'a mut T {
        match input {
            ErasedInput::Borrowed(token) => (**token)
                .downcast_mut::<T>()
                .expect("an input edge's token has the type of its place"),
            other => panic!("expected a borrowed token for this edge, got {other:?}"),
        }
    }

    edge_spec!(EdgeKind::Mutate);
}

impl<T: Send + Sync + 'static> InputEdge for MutateEdge<T> {}

impl<T: Send + Sync + 'static> EdgeSealed for TakeWhereEdge<T> {
    type Arg<'a> = T;

    fn extract(input: &mut ErasedInput<'_>) -> T {
        owned_token::<T>(input)
    }

    edge_spec!(EdgeKind::TakeWhere, keyed);
}

impl<T: Send + Sync + 'static> InputEdge for TakeWhereEdge<T> {}

impl<T: Send + Sync + 'static> EdgeSealed for MutateWhereEdge<T> {
    /// Borrows, as a [`MutateEdge`] does, the token the filter chose.
    type Arg<'a> = &'a mut T;

    fn extract<'a>(input: &'a mut ErasedInput<'_>) -> &'a mut T {
        match input {
            ErasedInput::Borrowed(token) => (**token)
                .downcast_mut::<T>()
                .expect("an input edge's token has the type of its place"),
            other => panic!("expected a borrowed token for this edge, got {other:?}"),
        }
    }

    edge_spec!(EdgeKind::MutateWhere, keyed);
}

impl<T: Send + Sync + 'static> InputEdge for MutateWhereEdge<T> {}

/// Hands over what it took as a [`TakeAllEdge`] does; the engine fills the
/// `Vec` from whatever the place holds, nothing included.
impl<T: Send + Sync + 'static> EdgeSealed for DrainEdge<T> {
    type Arg<'a> = Vec<T>;

    fn extract(input: &mut ErasedInput<'_>) -> Vec<T> {
        owned_tokens::<T>(input)
    }

    edge_spec!(EdgeKind::Drain);
}

impl<T: Send + Sync + 'static> InputEdge for DrainEdge<T> {}

/// The `Clone` bound is the compile-time half of what a survey needs, as for
/// a [`SnapshotEdge`]; the place's cloner is checked at
/// [`build`](crate::NetBuilder::build).
impl<T: Clone + Send + Sync + 'static> EdgeSealed for SurveyEdge<T> {
    type Arg<'a> = Vec<T>;

    fn extract(input: &mut ErasedInput<'_>) -> Vec<T> {
        owned_tokens::<T>(input)
    }

    edge_spec!(EdgeKind::Survey);
}

impl<T: Clone + Send + Sync + 'static> InputEdge for SurveyEdge<T> {}

/// A tuple of input edges, `()` to sixteen of them.
///
/// This is the type of the `inputs` argument of
/// [`NetBuilder::transition`](crate::NetBuilder::transition). A single edge
/// still goes in a tuple — `(TakeEdge(inbox),)` — because the tuple is what
/// carries the arity, and a [`Param<T>`](crate::Param) may stand anywhere in
/// it: the tuple is the body's parameter list, in order.
#[diagnostic::on_unimplemented(
    message = "a transition's inputs must be a tuple of input edges",
    label = "not a tuple of input edges",
    note = "write `(TakeEdge(p), SnapshotEdge(q))`, or `()` for a transition with no inputs; \
            a single input needs the trailing comma: `(TakeEdge(p),)`"
)]
pub trait InputEdges: EdgeTupleSealed {}

/// The tuple is read by reference: the plain handles are `Copy`, and a keyed
/// edge's filter is cloned into its spec.
macro_rules! impl_edges {
    ($(($t:ident, $r:ident, $i:tt)),*) => {
        impl<$($t: InputEdge,)*> EdgeTupleSealed for ($($t,)*) {
            #[allow(unused_variables, non_snake_case)]
            fn describe(&self, out: &mut Vec<InputSpec>) {
                let ($($t,)*) = self;
                $(out.push(EdgeSealed::spec($t));)*
            }
        }

        impl<$($t: InputEdge,)*> InputEdges for ($($t,)*) {}
    };
}

for_each_arity!(impl_edges);

/// The hand-written `Debug`s keep the `T: Debug` bound off the handle, as on
/// [`PlaceRef`] itself.
macro_rules! edge_handle_impls {
    ($name:ident) => {
        impl<T> Clone for $name<T> {
            fn clone(&self) -> Self {
                *self
            }
        }

        impl<T> Copy for $name<T> {}

        impl<T> fmt::Debug for $name<T> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_tuple(stringify!($name)).field(&self.0).finish()
            }
        }
    };
}

edge_handle_impls!(TakeEdge);
edge_handle_impls!(TakeAllEdge);
edge_handle_impls!(SnapshotEdge);
edge_handle_impls!(MutateEdge);
edge_handle_impls!(DrainEdge);
edge_handle_impls!(SurveyEdge);

/// A keyed edge holds its filter, so it is `Clone` and not `Copy`; still no
/// bound on `T`.
macro_rules! keyed_edge_handle_impls {
    ($name:ident) => {
        impl<T> Clone for $name<T> {
            fn clone(&self) -> Self {
                $name(self.0, self.1.clone())
            }
        }

        impl<T> fmt::Debug for $name<T> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_tuple(stringify!($name))
                    .field(&self.0)
                    .field(&self.1)
                    .finish()
            }
        }
    };
}

keyed_edge_handle_impls!(TakeWhereEdge);
keyed_edge_handle_impls!(MutateWhereEdge);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::NetId;
    use std::rc::Rc;

    #[test]
    fn edge_handles_do_not_depend_on_t() {
        fn assert_handle<H: Copy + Send + Sync + fmt::Debug>() {}

        // `Rc<u32>` is neither `Copy`, `Send` nor `Sync`.
        assert_handle::<TakeEdge<Rc<u32>>>();
        assert_handle::<TakeAllEdge<Rc<u32>>>();
        assert_handle::<SnapshotEdge<Rc<u32>>>();
        assert_handle::<MutateEdge<Rc<u32>>>();
        assert_handle::<DrainEdge<Rc<u32>>>();
        assert_handle::<SurveyEdge<Rc<u32>>>();

        // A keyed edge carries its filter, so it clones rather than copies.
        fn assert_keyed<H: Clone + Send + Sync + fmt::Debug>() {}
        assert_keyed::<TakeWhereEdge<Rc<u32>>>();
        assert_keyed::<MutateWhereEdge<Rc<u32>>>();
    }

    /// Every kind, in declaration order.
    const ALL: [EdgeKind; 8] = [
        EdgeKind::Take,
        EdgeKind::TakeAll,
        EdgeKind::Snapshot,
        EdgeKind::Mutate,
        EdgeKind::TakeWhere,
        EdgeKind::MutateWhere,
        EdgeKind::Drain,
        EdgeKind::Survey,
    ];

    /// The kinds a predicate holds for, in declaration order.
    fn kinds(predicate: fn(EdgeKind) -> bool) -> Vec<EdgeKind> {
        ALL.into_iter().filter(|&kind| predicate(kind)).collect()
    }

    #[test]
    fn only_the_taking_kinds_consume() {
        assert!(EdgeKind::Take.consumes());
        assert!(EdgeKind::TakeAll.consumes());
        assert!(EdgeKind::TakeWhere.consumes());
        assert!(EdgeKind::Drain.consumes());
        assert!(!EdgeKind::Snapshot.consumes());
        assert!(!EdgeKind::Mutate.consumes());
        assert!(!EdgeKind::MutateWhere.consumes());
        assert!(!EdgeKind::Survey.consumes());
    }

    #[test]
    fn the_keyed_kinds_are_the_where_kinds_and_the_mutating_ones_borrow() {
        assert_eq!(
            kinds(EdgeKind::is_keyed),
            [EdgeKind::TakeWhere, EdgeKind::MutateWhere]
        );
        assert_eq!(
            kinds(EdgeKind::mutates),
            [EdgeKind::Mutate, EdgeKind::MutateWhere]
        );
    }

    /// Exactly the two zero-or-more kinds accept an empty place, and a drain
    /// is the one consuming kind that does not always take a token.
    #[test]
    fn drain_and_survey_alone_accept_an_empty_place() {
        let absent: Vec<EdgeKind> = ALL
            .into_iter()
            .filter(|kind| !kind.requires_presence())
            .collect();
        assert_eq!(absent, [EdgeKind::Drain, EdgeKind::Survey]);
        assert_eq!(
            kinds(EdgeKind::always_consumes),
            [EdgeKind::Take, EdgeKind::TakeAll, EdgeKind::TakeWhere]
        );
        assert!(!EdgeKind::Drain.is_keyed() && !EdgeKind::Drain.mutates());
        assert!(!EdgeKind::Survey.is_keyed() && !EdgeKind::Survey.mutates());
    }

    #[test]
    fn a_tuple_of_edges_describes_itself_in_order() {
        let net = NetId(3);
        let a = PlaceRef::<u32>::new(net, PlaceId(0));
        let b = PlaceRef::<String>::new(net, PlaceId(1));
        let mut out = Vec::new();
        EdgeTupleSealed::describe(&(TakeEdge(a), SnapshotEdge(b), MutateEdge(a)), &mut out);
        assert_eq!(
            out,
            [
                InputSpec::Edge {
                    place: PlaceId(0),
                    kind: EdgeKind::Take,
                    net,
                    filter: None,
                },
                InputSpec::Edge {
                    place: PlaceId(1),
                    kind: EdgeKind::Snapshot,
                    net,
                    filter: None,
                },
                InputSpec::Edge {
                    place: PlaceId(0),
                    kind: EdgeKind::Mutate,
                    net,
                    filter: None,
                },
            ]
        );
    }

    /// The two zero-or-more edges describe themselves by kind alone, with no
    /// filter, like the plain kinds.
    #[test]
    fn drain_and_survey_describe_themselves_without_a_filter() {
        let net = NetId(6);
        let a = PlaceRef::<u32>::new(net, PlaceId(0));
        let b = PlaceRef::<String>::new(net, PlaceId(1));
        let mut out = Vec::new();
        EdgeTupleSealed::describe(&(DrainEdge(a), SurveyEdge(b)), &mut out);
        assert_eq!(
            out,
            [
                InputSpec::Edge {
                    place: PlaceId(0),
                    kind: EdgeKind::Drain,
                    net,
                    filter: None,
                },
                InputSpec::Edge {
                    place: PlaceId(1),
                    kind: EdgeKind::Survey,
                    net,
                    filter: None,
                },
            ]
        );
    }

    /// A keyed edge describes itself with its filter, cloned out of the
    /// tuple, which stays usable.
    #[test]
    fn a_keyed_edge_describes_itself_with_its_filter() {
        let net = NetId(5);
        let a = PlaceRef::<u32>::new(net, PlaceId(0));
        let b = PlaceRef::<u32>::new(net, PlaceId(1));
        let birds = Filter::new().key_in("class", ["Bird"]);
        let inputs = (
            TakeWhereEdge(a, birds.clone()),
            MutateWhereEdge(b, Filter::new()),
        );
        let mut out = Vec::new();
        EdgeTupleSealed::describe(&inputs, &mut out);
        assert_eq!(
            out,
            [
                InputSpec::Edge {
                    place: PlaceId(0),
                    kind: EdgeKind::TakeWhere,
                    net,
                    filter: Some(birds),
                },
                InputSpec::Edge {
                    place: PlaceId(1),
                    kind: EdgeKind::MutateWhere,
                    net,
                    filter: Some(Filter::new()),
                },
            ]
        );
        assert_eq!(inputs.0.0, a, "the tuple is only read");
    }

    /// A tuple may mix the two kinds of input, and it describes them in the
    /// order they were written — which is the order the body's parameters
    /// are in.
    #[test]
    fn a_tuple_may_mix_edges_and_parameters() {
        #[derive(Clone)]
        struct Config;

        let net = NetId(4);
        let a = PlaceRef::<u32>::new(net, PlaceId(0));
        let mut out = Vec::new();
        EdgeTupleSealed::describe(&(crate::Param::<Config>::new(), TakeEdge(a)), &mut out);
        assert!(matches!(out[0], InputSpec::Param { .. }));
        assert!(matches!(
            out[1],
            InputSpec::Edge {
                kind: EdgeKind::Take,
                ..
            }
        ));
    }
}
