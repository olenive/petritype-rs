//! Input edges: the four kinds, and what each one hands to the body.
//!
//! An input edge says *how* a transition reads a place, which fixes the type
//! of the matching body parameter:
//!
//! | edge | consumes | body parameter |
//! |---|---|---|
//! | [`TakeEdge<T>`] | the front token | `T` |
//! | [`TakeAllEdge<T>`] | every token | `Vec<T>` |
//! | [`SnapshotEdge<T>`] | nothing (clones the front token) | `T` |
//! | [`MutateEdge<T>`] | nothing (borrows the front token) | `&mut T` |
//!
//! All four require the place to be non-empty for the transition to be
//! enabled, including the two that do not consume.

use std::fmt;

use crate::arity::for_each_arity;
use crate::erased::ErasedInput;
use crate::ids::{NetId, PlaceId, PlaceRef};
use crate::sealed::{EdgeSealed, EdgeTupleSealed};

/// Which of the four input edge kinds an edge is, with its type forgotten.
///
/// This is the form the net stores and the read API reports
/// ([`TransitionInfo::inputs`](crate::TransitionInfo::inputs)); the typed
/// form is [`TakeEdge`] and the other three structs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EdgeKind {
    /// [`TakeEdge`]: consumes the front token.
    Take,
    /// [`TakeAllEdge`]: consumes every token.
    TakeAll,
    /// [`SnapshotEdge`]: reads a clone of the front token.
    Snapshot,
    /// [`MutateEdge`]: borrows the front token and may change it.
    Mutate,
}

impl EdgeKind {
    /// Whether firing through this edge removes tokens from its place.
    ///
    /// The two reading kinds leave the marking alone, which is what makes
    /// them safe to point at a configuration place that every transition
    /// reads.
    pub fn consumes(self) -> bool {
        matches!(self, EdgeKind::Take | EdgeKind::TakeAll)
    }
}

/// Consumes the front token of a place; the body parameter is `T`.
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
/// The transition is enabled when the place holds at least one token, so a
/// batch is "everything that has arrived by now", not a fixed size.
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
/// its label rather than destroying and recreating it. Because the body holds
/// a borrow into the net, a transition with a mutating edge is not
/// *detachable*: it runs inline instead of being handed to a worker.
pub struct MutateEdge<T>(
    /// The place whose front token to borrow.
    pub PlaceRef<T>,
);

/// One of the four input edge kinds.
///
/// Sealed: the four structs in this module are the only implementors, and the
/// bound exists so that a transition's inputs are checked at compile time.
/// The associated parameter type each kind produces is the table at the top
/// of this module.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not an input edge",
    label = "not an input edge",
    note = "an input must say how it reads its place: `TakeEdge(p)`, `TakeAllEdge(p)`, \
            `SnapshotEdge(p)` or `MutateEdge(p)`"
)]
pub trait InputEdge: EdgeSealed {
    /// This edge's kind, as the net records it.
    const KIND: EdgeKind;

    /// The place this edge reads.
    fn place(self) -> PlaceId;

    /// The net the place belongs to, checked by the builder before the edge
    /// is recorded.
    fn net(self) -> NetId;

    /// [`KIND`](InputEdge::KIND) as a value, for code that has an edge rather
    /// than a type.
    fn kind(self) -> EdgeKind {
        Self::KIND
    }
}

/// Moves the token out of an input, leaving [`ErasedInput::Taken`] behind.
///
/// The `expect`s cannot fire: the engine builds the variant from the edge's
/// own kind, and the token's type was fixed by the `PlaceRef<T>` inside the
/// edge. (The runtime type check in
/// [`NetError::TypeMismatch`](crate::NetError::TypeMismatch) is for the
/// erased outside-the-net route, not for this one.)
fn owned_token<T: 'static>(input: &mut ErasedInput<'_>) -> T {
    match std::mem::replace(input, ErasedInput::Taken) {
        ErasedInput::Token(token) => *token
            .downcast::<T>()
            .expect("an input edge's token has the type of its place"),
        other => panic!("expected one token for this edge, got {other:?}"),
    }
}

impl<T: Send + Sync + 'static> EdgeSealed for TakeEdge<T> {
    type Arg<'a> = T;

    fn extract(input: &mut ErasedInput<'_>) -> T {
        owned_token::<T>(input)
    }
}

impl<T: Send + Sync + 'static> InputEdge for TakeEdge<T> {
    const KIND: EdgeKind = EdgeKind::Take;

    fn place(self) -> PlaceId {
        self.0.id()
    }

    fn net(self) -> NetId {
        self.0.net()
    }
}

impl<T: Send + Sync + 'static> EdgeSealed for TakeAllEdge<T> {
    type Arg<'a> = Vec<T>;

    fn extract(input: &mut ErasedInput<'_>) -> Vec<T> {
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
}

impl<T: Send + Sync + 'static> InputEdge for TakeAllEdge<T> {
    const KIND: EdgeKind = EdgeKind::TakeAll;

    fn place(self) -> PlaceId {
        self.0.id()
    }

    fn net(self) -> NetId {
        self.0.net()
    }
}

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
}

impl<T: Clone + Send + Sync + 'static> InputEdge for SnapshotEdge<T> {
    const KIND: EdgeKind = EdgeKind::Snapshot;

    fn place(self) -> PlaceId {
        self.0.id()
    }

    fn net(self) -> NetId {
        self.0.net()
    }
}

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
}

impl<T: Send + Sync + 'static> InputEdge for MutateEdge<T> {
    const KIND: EdgeKind = EdgeKind::Mutate;

    fn place(self) -> PlaceId {
        self.0.id()
    }

    fn net(self) -> NetId {
        self.0.net()
    }
}

/// A tuple of input edges, `()` to twelve of them.
///
/// This is the type of the `inputs` argument of
/// [`NetBuilder::transition`](crate::NetBuilder::transition). A single edge
/// still goes in a tuple — `(TakeEdge(inbox),)` — because the tuple is what
/// carries the arity.
#[diagnostic::on_unimplemented(
    message = "a transition's inputs must be a tuple of input edges",
    label = "not a tuple of input edges",
    note = "write `(TakeEdge(p), SnapshotEdge(q))`, or `()` for a transition with no inputs; \
            a single input needs the trailing comma: `(TakeEdge(p),)`"
)]
pub trait InputEdges: EdgeTupleSealed {}

/// The handles are `Copy`, so the tuple is read by reference and the edges
/// are copied out of it.
macro_rules! impl_edges {
    ($(($t:ident, $r:ident, $i:tt)),*) => {
        impl<$($t: InputEdge,)*> EdgeTupleSealed for ($($t,)*) {
            #[allow(unused_variables, non_snake_case)]
            fn describe(&self, out: &mut Vec<(PlaceId, EdgeKind, NetId)>) {
                let ($($t,)*) = self;
                $(out.push((InputEdge::place(*$t), InputEdge::kind(*$t), InputEdge::net(*$t)));)*
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
    }

    #[test]
    fn only_the_taking_kinds_consume() {
        assert!(EdgeKind::Take.consumes());
        assert!(EdgeKind::TakeAll.consumes());
        assert!(!EdgeKind::Snapshot.consumes());
        assert!(!EdgeKind::Mutate.consumes());
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
                (PlaceId(0), EdgeKind::Take, net),
                (PlaceId(1), EdgeKind::Snapshot, net),
                (PlaceId(0), EdgeKind::Mutate, net),
            ]
        );
    }
}
