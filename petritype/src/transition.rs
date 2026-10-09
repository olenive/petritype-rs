//! Transitions: what the net stores, what the read API reports, and the
//! handle the builder returns while a transition is being declared.

use std::any::TypeId;
use std::fmt;

use crate::edge::EdgeKind;
use crate::erased::ErasedBody;
use crate::ids::{PlaceId, TransitionId};
use crate::keys::Filter;
use crate::net::Net;

/// A transition's guard: may this transition fire, given the whole net?
///
/// Called with `&Net` during the scan in
/// [`Net::fireable`](crate::Net::fireable), so a guard may read any place's
/// count or tokens, not only its own transition's inputs. `Fn` rather than
/// `FnMut`: a guard is a question, asked as often as the engine likes, and
/// answering it must not change the net.
pub(crate) type Guard = Box<dyn Fn(&Net) -> bool + Send + Sync>;

/// A transition's priority: how much the default selector wants to fire it.
///
/// Called with `&Net`, like a guard, so a priority may depend on the marking.
/// A transition without one scores `0.0`.
pub(crate) type Priority = Box<dyn Fn(&Net) -> f64 + Send + Sync>;

/// Where one of the body's arguments comes from (DESIGN §10).
///
/// A transition's inputs are written as one tuple, which is also the body's
/// parameter list, but the two kinds of input are stored apart: the edges in
/// [`TransitionSlot::inputs`](TransitionSlot::inputs), which is what
/// fireability, the read API and the build-time checks read, and the
/// parameters nowhere but here. This is the thread back: one entry per
/// argument, in tuple order, saying which list the argument comes out of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArgSource {
    /// The argument of the *i*th input edge of this transition.
    Edge(usize),
    /// The clone of a parameter value, looked up in the net by type.
    Param {
        /// `TypeId::of::<T>()`, the key
        /// [`Net::set_param`](crate::Net::set_param) stores it under.
        type_id: TypeId,
        /// `std::any::type_name::<T>()`, for
        /// [`BuildError::MissingParam`](crate::BuildError::MissingParam).
        /// Also kept, in tuple order, in
        /// [`TransitionSlot::params`](TransitionSlot::params), which is where
        /// the read API hands it out as a slice.
        type_name: &'static str,
    },
}

/// One transition as the net holds it, with its body's types erased.
///
/// There is no `routing` field, although DESIGN §4.4 lists one: a body that routes its outputs by type resolves its
/// `TypeId -> output index` table when it is declared and the erased body
/// captures it, so the net itself stores nothing about routing and firing
/// costs one lookup either way.
pub(crate) struct TransitionSlot {
    /// The name given in the builder; unique within a net.
    pub(crate) name: String,
    /// Input places and how each is read, in declaration order, with the
    /// [parameters](crate::Param) left out. The places are distinct:
    /// [`build`](crate::NetBuilder::build) rejects two input edges into one
    /// place.
    ///
    /// Edges only, because everything that reads this field is about places:
    /// fireability, [`Net::edges`](crate::Net::edges), the duplicate-place
    /// check, the acyclicity and source walks, and stage one's consumption.
    /// [`args`](TransitionSlot::args) is what remembers where the parameters
    /// stood among them.
    pub(crate) inputs: Vec<(PlaceId, EdgeKind)>,
    /// One entry per input edge, beside [`inputs`](TransitionSlot::inputs):
    /// the filter of a [keyed](EdgeKind::is_keyed) edge, `None` for every
    /// other. Apart rather than inside the kind so that the kind stays
    /// `Copy` and everything that reads only places reads on unchanged.
    pub(crate) filters: Vec<Option<Filter>>,
    /// The declared parameter types by name, in declaration order — what
    /// [`TransitionInfo::params`](TransitionInfo::params) reports so that a
    /// renderer can badge the transition (DESIGN §10).
    pub(crate) params: Vec<&'static str>,
    /// One entry per body argument, in the order the input tuple was
    /// written: an index into [`inputs`](TransitionSlot::inputs), or a
    /// parameter type to look up in the net.
    pub(crate) args: Vec<ArgSource>,
    /// Output places in declaration order — the order of the slots in what
    /// the body returns.
    pub(crate) outputs: Vec<PlaceId>,
    /// Whether the body returns its outputs by position, read off its return
    /// type when the transition was declared, so that it survives the body
    /// being out of its slot while in flight.
    pub(crate) positional: bool,
    /// The erased body. `Option` because firing takes it out of the net: it
    /// travels with the [`Firing`](crate::Firing) from
    /// [`launch`](crate::Net::launch) until
    /// [`distribute`](crate::Net::distribute) puts it back, and an empty slot
    /// is the whole of the in-flight mark — there is no register of airborne
    /// firings, which is also what stops one transition being launched twice
    /// ([`Net::in_flight`](crate::Net::in_flight)).
    pub(crate) body: Option<ErasedBody>,
    /// The author's extra condition on firing, or `None` for "no condition".
    pub(crate) guard: Option<Guard>,
    /// What the default selector ranks this transition by, or `None` for
    /// `0.0`.
    pub(crate) priority: Option<Priority>,
    /// The on/off switch: a disabled transition is never fireable, whatever
    /// its places hold. Transitions start switched on;
    /// [`Net::enable`](crate::Net::enable) and
    /// [`Net::disable`](crate::Net::disable) flip it.
    pub(crate) enabled: bool,
    /// Whether a failed firing puts the tokens it consumed back, as asked for
    /// by
    /// [`TransitionBuilder::restore_tokens_on_failure`](crate::TransitionBuilder::restore_tokens_on_failure).
    ///
    /// Off by default. When it is on, stage one clones every token a
    /// consuming edge takes, the clones travel with the firing, and a failure
    /// puts them back at the front of their places
    /// ([`Net::distribute`](crate::Net::distribute)).
    pub(crate) restore_tokens_on_failure: bool,
    /// True unless some input is a [`MutateEdge`](crate::MutateEdge) or a
    /// [`MutateWhereEdge`](crate::MutateWhereEdge).
    ///
    /// A mutating edge means the body borrows a token where it lies, so the
    /// firing cannot leave the net and cannot be handed to a worker. Derived
    /// from the edge kinds, never asked of the author.
    pub(crate) detachable: bool,
}

impl fmt::Debug for TransitionSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TransitionSlot")
            .field("name", &self.name)
            .field("inputs", &self.inputs)
            .field("filters", &self.filters)
            // The type names only: a parameter's value is never printed
            // anywhere (DESIGN §10).
            .field("params", &self.params)
            .field("outputs", &self.outputs)
            .field("positional", &self.positional)
            // The body, the guard and the priority are closures: all a reader
            // can be told is whether they are there.
            .field("in_flight", &self.body.is_none())
            .field("guard", &self.guard.is_some())
            .field("priority", &self.priority.is_some())
            .field("enabled", &self.enabled)
            .field("restore_tokens_on_failure", &self.restore_tokens_on_failure)
            .field("detachable", &self.detachable)
            .finish()
    }
}

/// A view of one transition, for building and refreshing a diagram.
///
/// Returned by value from [`Net::transitions`](crate::Net::transitions), like
/// [`PlaceInfo`](crate::PlaceInfo): the net stores the pieces and this struct
/// is assembled on demand, so it can never go stale. The slices borrow the
/// net, so nothing is allocated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct TransitionInfo<'a> {
    /// The transition's name, unique within the net.
    pub name: &'a str,
    /// Input places and how each is read, in declaration order. The places
    /// are distinct.
    ///
    /// Edges only. A [`Param<T>`](crate::Param) is an input of the
    /// transition but not an edge of the net — it reads no place and is no
    /// node in a diagram — so it appears in [`params`](TransitionInfo::params)
    /// instead, and never in [`Net::edges`](crate::Net::edges).
    pub inputs: &'a [(PlaceId, EdgeKind)],
    /// One entry per input edge, in the order of
    /// [`inputs`](TransitionInfo::inputs): the [`Filter`] of a
    /// [keyed](EdgeKind::is_keyed) edge
    /// ([`TakeWhereEdge`](crate::TakeWhereEdge),
    /// [`MutateWhereEdge`](crate::MutateWhereEdge)), as declared, and `None`
    /// for every other kind (slice 19).
    ///
    /// Beside `inputs` rather than inside it, so that the edge list keeps its
    /// shape and [`EdgeKind`] stays `Copy`; `filters[i]` belongs to
    /// `inputs[i]`.
    pub filters: &'a [Option<Filter>],
    /// The [parameter](crate::Param) types this transition declared, by
    /// name, in declaration order (DESIGN §10).
    ///
    /// Type names and nothing else: a parameter has no `Debug` bound, and a
    /// reader is told what a transition *needs*, never what it was given. A
    /// renderer badges the transition with these; the values live on the net
    /// and are read with [`Net::param`](crate::Net::param).
    pub params: &'a [&'static str],
    /// Output places, in declaration order.
    pub outputs: &'a [PlaceId],
    /// Whether the body returns its outputs by position: a tuple, one slot
    /// per output place ([`PositionalOutputs`](crate::PositionalOutputs)),
    /// `()` included. False for a body that routes its payloads by type
    /// ([`RoutedOutput`](crate::RoutedOutput)) or names their places in a
    /// [`Distribution`](crate::Distribution); a wrapped body is what its
    /// [output distribution function](crate::OutputDistribution) returns,
    /// and a `Result` what it wraps (DESIGN §4.5).
    ///
    /// It says what an output edge's [`slot`](crate::EdgeInfo::Output)
    /// means. For a positional body the slot is the position in the
    /// returned tuple the place is filled from, which a diagram may label;
    /// otherwise it is only the order the places were declared in. The net
    /// protocol writes `slot` for positional outputs alone, as Python
    /// petritype writes its `return_index`.
    pub positional: bool,
    /// The on/off switch, as
    /// [`Net::enable`](crate::Net::enable) and
    /// [`Net::disable`](crate::Net::disable) leave it.
    ///
    /// This is *not* whether the transition can fire now: a switched-on
    /// transition whose input places are empty, or whose guard says no, is
    /// still `enabled`. What can fire is
    /// [`Net::is_fireable`](crate::Net::is_fireable).
    pub enabled: bool,
    /// Whether this transition's body is out of its slot right now: it was
    /// [launched](crate::Net::launch) and the firing has not been
    /// [distributed](crate::Net::distribute) yet.
    ///
    /// An in-flight transition is never fireable, whatever its places hold
    /// and whatever its switch says. A diagram usually draws it differently —
    /// this is the transition that is *working*.
    pub in_flight: bool,
    /// Whether a firing of this transition can leave the net.
    ///
    /// False exactly when some input is a
    /// [`MutateEdge`](crate::MutateEdge) or a
    /// [`MutateWhereEdge`](crate::MutateWhereEdge), whose body parameter
    /// borrows a token where it lies: such a firing runs inline inside
    /// [`launch`](crate::Net::launch) instead of being handed to a worker.
    /// Derived from the edge kinds, never asked of the author.
    pub detachable: bool,
}

/// The handle [`NetBuilder::transition`](crate::NetBuilder::transition)
/// returns, for the options that belong to one transition.
///
/// It holds the builder borrowed, which is why places are declared first: the
/// borrow lasts as long as the chain. Each option takes `&mut self` and
/// returns `&mut Self`, so they chain, and [`id`](TransitionBuilder::id)
/// yields the [`TransitionId`] when something later names this transition:
///
/// ```
/// # use petritype::{NetBuilder, TakeEdge};
/// # let mut b = NetBuilder::new();
/// # let inbox = b.place::<u32>("Inbox");
/// # let out = b.place::<u32>("Out");
/// let double = b
///     .transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,))
///     .guard(move |net| net.count(inbox.id()) >= 2)
///     .priority(|_| 1.0)
///     .id();
/// ```
///
/// `output_distribution` is deliberately *not* here, although DESIGN §5 lists
/// it: by the time this handle exists the body's return type has been erased,
/// and the point of an output distribution function is that its own return
/// type decides the output form. It is a method on the body instead —
/// [`OutputDistribution::output_distribution`](crate::OutputDistribution::output_distribution),
/// applied before `transition()` is called.
pub struct TransitionBuilder<'a> {
    /// The builder to write the options into.
    pub(crate) builder: &'a mut crate::NetBuilder,
    pub(crate) id: TransitionId,
}

impl TransitionBuilder<'_> {
    /// The id of the transition just declared, for the read API, events and
    /// the firing stages. Only needed if something later names this
    /// transition.
    pub fn id(&self) -> TransitionId {
        self.id
    }

    /// Adds a condition on firing: the transition is
    /// [fireable](crate::Net::fireable) only while `f` returns true.
    ///
    /// The guard is asked with the whole net in hand, so it may look
    /// anywhere — `net.count(place.id())`, `net.token::<T>(..)`, a clock, a
    /// captured atomic — and it is asked once per transition per scan, so it
    /// should be cheap and must not be counted on to run a fixed number of
    /// times. A guard that returns false is not an error: the transition
    /// simply does not appear in [`Net::fireable`](crate::Net::fireable).
    ///
    /// The guard is a question about the net, so it takes `&Net` and cannot
    /// change it. It outlives the call that declared it, so a guard that
    /// names a place is written `move |net| ...`: a
    /// [`PlaceRef`](crate::PlaceRef) is `Copy`, so moving one in costs
    /// nothing and leaves the original usable. A second call replaces the
    /// first.
    pub fn guard<F>(&mut self, f: F) -> &mut Self
    where
        F: Fn(&Net) -> bool + Send + Sync + 'static,
    {
        self.builder.transition_slot(self.id).guard = Some(Box::new(f));
        self
    }

    /// Ranks this transition for the default selector: of the fireable
    /// transitions, the one scoring highest fires, and ties go to the one
    /// declared first.
    ///
    /// A transition without a priority scores `0.0`, so a single
    /// `.priority(|_| 1.0)` is enough to put one transition ahead of every
    /// other. The score may depend on the marking, since `f` is given the
    /// net. `NaN` is read as `0.0`.
    ///
    /// A custom [`selector`](crate::Net::set_selector) may ignore priorities
    /// entirely. Like a guard, a priority outlives its declaration, so one
    /// that names a place is written `move |net| ...`. A second call replaces
    /// the first.
    pub fn priority<F>(&mut self, f: F) -> &mut Self
    where
        F: Fn(&Net) -> f64 + Send + Sync + 'static,
    {
        self.builder.transition_slot(self.id).priority = Some(Box::new(f));
        self
    }

    /// Puts the tokens a firing of this transition consumed back in their
    /// places if it fails, instead of losing them.
    ///
    /// A failed firing loses its inputs by default: the body owns them by the
    /// time it fails, and may have changed or dropped them. This flag buys
    /// them back with a clone. Stage one clones every token a consuming edge
    /// takes — [`TakeEdge`](crate::TakeEdge),
    /// [`TakeAllEdge`](crate::TakeAllEdge) and
    /// [`TakeWhereEdge`](crate::TakeWhereEdge) — before the body sees
    /// anything, the clones travel with the firing, and any failure of it (an
    /// `Err`, a panic, or a [`Distribution`](crate::Distribution) naming a
    /// place the transition may not write to) puts them back at the **front**
    /// of their places, in the order they were taken and with the ids they
    /// had. The firing is undone rather than recorded, so the transition can
    /// simply fire again — which is what lets a driver retry one.
    ///
    /// For a `TakeWhereEdge` the front is not where the token was: it may
    /// have been taken from the middle of its place. The id is kept and the
    /// position is not; the token is still the first its filter accepts, so
    /// the next firing takes it again.
    ///
    /// [`SnapshotEdge`](crate::SnapshotEdge),
    /// [`MutateEdge`](crate::MutateEdge) and
    /// [`MutateWhereEdge`](crate::MutateWhereEdge) consume nothing, so there
    /// is nothing to restore for them: a snapshot's clone was never in a
    /// place, and a mutation was made where the token lies and stands,
    /// failure or not.
    ///
    /// The cost is one clone per consumed token per firing of *this*
    /// transition, taken whether or not the firing goes on to fail.
    /// Transitions without the flag clone nothing. A token that is expensive
    /// to copy wants an `Arc` inside it, where a clone is a counter bump.
    ///
    /// ```
    /// # use petritype::{NetBuilder, TakeEdge};
    /// # #[derive(Debug)] struct Refused;
    /// # impl std::fmt::Display for Refused {
    /// #     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    /// #         f.write_str("refused")
    /// #     }
    /// # }
    /// # impl std::error::Error for Refused {}
    /// # let mut b = NetBuilder::new();
    /// # let inbox = b.place::<u32>("Inbox");
    /// # let out = b.place::<u32>("Out");
    /// # b.seed(inbox, [1]);
    /// b.transition(
    ///     "Fussy",
    ///     (TakeEdge(inbox),),
    ///     (out,),
    ///     |_n: u32| -> Result<(u32,), Refused> { Err(Refused) },
    /// )
    /// .restore_tokens_on_failure();
    /// let mut net = b.build()?;
    /// let token = net.tokens(inbox.id()).next().unwrap().0;
    ///
    /// let failure = net.step().expect_err("this body always fails");
    /// assert!(failure.restored);
    /// assert_eq!(net.count(inbox.id()), 1, "the token is back");
    /// assert_eq!(
    ///     net.tokens(inbox.id()).next().unwrap().0,
    ///     token,
    ///     "and it is the same token, not a new one",
    /// );
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// [`BuildError::RestoreOfNoClonePlace`](crate::BuildError::RestoreOfNoClonePlace)
    /// from [`build`](crate::NetBuilder::build) if this transition consumes a
    /// place declared with
    /// [`place_no_clone`](crate::NetBuilder::place_no_clone), which has no
    /// cloner to take the clones with. Reading such a place through a
    /// [`MutateEdge`](crate::MutateEdge) is fine: nothing is consumed, so
    /// nothing is cloned.
    pub fn restore_tokens_on_failure(&mut self) -> &mut Self {
        self.builder
            .transition_slot(self.id)
            .restore_tokens_on_failure = true;
        self
    }
}

impl fmt::Debug for TransitionBuilder<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Deliberately not the whole builder: this is a handle to one
        // transition, and printing the net around it would bury that.
        f.debug_struct("TransitionBuilder")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}
