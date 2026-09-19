//! Transitions: what the net stores, what the read API reports, and the
//! handle the builder returns while a transition is being declared.

use std::fmt;

use crate::edge::EdgeKind;
use crate::erased::ErasedBody;
use crate::ids::{PlaceId, TransitionId};

/// One transition as the net holds it, with its body's types erased.
///
/// The fields this slice fills are the ones it can check; `guard`,
/// `priority`, `routing`, `restore_tokens_on_failure` and the rest of DESIGN
/// §4.4 arrive with the slices that read them.
pub(crate) struct TransitionSlot {
    /// The name given in the builder; unique within a net.
    pub(crate) name: String,
    /// Input places and how each is read, in declaration order — the order of
    /// the body's parameters.
    pub(crate) inputs: Vec<(PlaceId, EdgeKind)>,
    /// Output places in declaration order — the order of the slots in what
    /// the body returns.
    pub(crate) outputs: Vec<PlaceId>,
    /// The erased body. `Option` because firing takes it out of the net for
    /// the duration of the call: a transition with no body in its slot is in
    /// flight, which is also what stops it being launched twice.
    pub(crate) body: Option<ErasedBody>,
    /// Whether the transition may be selected at all. Transitions start
    /// enabled; `enable` / `disable` come with the firing slices.
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
            // The body is a closure: all a reader can be told is whether it
            // is at home.
            .field("in_flight", &self.body.is_none())
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
    /// Input places and how each is read, in declaration order.
    pub inputs: &'a [(PlaceId, EdgeKind)],
    /// Output places, in declaration order.
    pub outputs: &'a [PlaceId],
    /// Whether the transition may be selected for firing.
    pub enabled: bool,
}

/// The handle [`NetBuilder::transition`](crate::NetBuilder::transition)
/// returns, for the options that belong to one transition.
///
/// It holds the builder borrowed, which is why places are declared first: the
/// borrow lasts as long as the chain. In this version the only method is
/// [`id`](TransitionBuilder::id); `guard`, `priority`,
/// `output_distribution` and `restore_tokens_on_failure` arrive with the
/// slices that act on them, and each will take `&mut self` and return
/// `&mut Self` so that the chain in DESIGN §5 reads as written.
pub struct TransitionBuilder<'a> {
    /// The builder to write the options into. Held now so that the borrow —
    /// and the chaining it makes possible — is the same in this version as in
    /// the next one.
    #[expect(
        dead_code,
        reason = "the option methods that write through it arrive in later slices; the \
                  borrow itself is already part of the API's shape"
    )]
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
