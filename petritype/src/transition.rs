//! Transitions: what the net stores, what the read API reports, and the
//! handle the builder returns while a transition is being declared.

use std::fmt;

use crate::edge::EdgeKind;
use crate::erased::ErasedBody;
use crate::ids::{PlaceId, TransitionId};
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

/// One transition as the net holds it, with its body's types erased.
///
/// The fields this slice fills are the ones it can check;
/// `restore_tokens_on_failure` and the rest of DESIGN §4.4 arrive with the
/// slices that read them. There is no `routing` field, although DESIGN §4.4
/// lists one: a body that routes its outputs by type resolves its
/// `TypeId -> output index` table when it is declared and the erased body
/// captures it, so the net itself stores nothing about routing and firing
/// costs one lookup either way.
pub(crate) struct TransitionSlot {
    /// The name given in the builder; unique within a net.
    pub(crate) name: String,
    /// Input places and how each is read, in declaration order — the order of
    /// the body's parameters. The places are distinct:
    /// [`build`](crate::NetBuilder::build) rejects two input edges into one
    /// place.
    pub(crate) inputs: Vec<(PlaceId, EdgeKind)>,
    /// Output places in declaration order — the order of the slots in what
    /// the body returns.
    pub(crate) outputs: Vec<PlaceId>,
    /// The erased body. `Option` because firing takes it out of the net for
    /// the duration of the call: a transition with no body in its slot is in
    /// flight, which is also what stops it being launched twice.
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
    /// True unless some input is a [`MutateEdge`](crate::MutateEdge).
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
            .field("outputs", &self.outputs)
            // The body, the guard and the priority are closures: all a reader
            // can be told is whether they are there.
            .field("in_flight", &self.body.is_none())
            .field("guard", &self.guard.is_some())
            .field("priority", &self.priority.is_some())
            .field("enabled", &self.enabled)
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
    pub inputs: &'a [(PlaceId, EdgeKind)],
    /// Output places, in declaration order.
    pub outputs: &'a [PlaceId],
    /// The on/off switch, as
    /// [`Net::enable`](crate::Net::enable) and
    /// [`Net::disable`](crate::Net::disable) leave it.
    ///
    /// This is *not* whether the transition can fire now: a switched-on
    /// transition whose input places are empty, or whose guard says no, is
    /// still `enabled`. What can fire is
    /// [`Net::is_fireable`](crate::Net::is_fireable).
    pub enabled: bool,
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
/// `restore_tokens_on_failure` arrives with the slice that acts on it.
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
