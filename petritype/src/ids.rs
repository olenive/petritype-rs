//! Identifiers: erased ids used by events and the read API, and the typed
//! place handle the builder hands back.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

/// Identifies a place within one [`Net`](crate::Net).
///
/// The id is an index into the net's place list. Ids are never reused, so an
/// id remains valid (or, once topology editing exists, a tombstone) for the
/// life of the net.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlaceId(pub(crate) u32);

/// Identifies a transition within one [`Net`](crate::Net).
///
/// Transitions arrive in a later slice; the id exists now because events and
/// the error types name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TransitionId(pub(crate) u32);

/// Identifies one token within one [`Net`](crate::Net).
///
/// Token ids are handed out in increasing order and are never reused: a
/// removed or consumed token retires its id for good, so a visualiser can key
/// its own state by `TokenId` without fear of collision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TokenId(pub(crate) u64);

/// A typed handle to a place, returned by
/// [`NetBuilder::place`](crate::NetBuilder::place).
///
/// `PlaceRef<T>` is what makes wiring mistakes compile errors: the token type
/// travels with the handle, while the net stores tokens erased.
///
/// The phantom is `fn() -> T` rather than `T` on purpose. `PhantomData<T>`
/// would make the handle behave, for auto traits and drop checking, as if it
/// owned a `T`: a `PlaceRef<Rc<Job>>` would not be `Send`, and deriving
/// `Copy`/`Clone`/`Eq`/`Hash` would add a `T: Copy`/`T: Eq`/... bound. A
/// function-pointer type is `Copy + Send + Sync` whatever its return type is,
/// and it is covariant in `T`, which is the right variance for a handle that
/// only ever *produces* `T`s. For the same reason the trait impls below are
/// written out by hand: `#[derive]` would add `T:` bounds that the handle does
/// not need.
pub struct PlaceRef<T>(
    pub(crate) PlaceId,
    /// See the type-level comment: never `PhantomData<T>`.
    pub(crate) PhantomData<fn() -> T>,
);

impl<T> PlaceRef<T> {
    /// Wraps a raw id. Only the builder knows that `id` really is a place of
    /// type `T`.
    pub(crate) fn new(id: PlaceId) -> Self {
        PlaceRef(id, PhantomData)
    }

    /// The erased id of this place, for the read API, events and
    /// [`Net::remove`](crate::Net::remove).
    pub fn id(self) -> PlaceId {
        self.0
    }
}

impl<T> Clone for PlaceRef<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for PlaceRef<T> {}

impl<T> fmt::Debug for PlaceRef<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PlaceRef")
            .field(&self.0)
            .field(&std::any::type_name::<T>())
            .finish()
    }
}

impl<T> PartialEq for PlaceRef<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<T> Eq for PlaceRef<T> {}

impl<T> Hash for PlaceRef<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    fn assert_copy_send_sync_debug_eq_hash<T: Copy + Send + Sync + std::fmt::Debug + Eq + Hash>() {}

    #[test]
    fn place_ref_traits_do_not_depend_on_t() {
        // `Rc<u32>` is neither `Send`, `Sync`, `Copy` nor `Eq`; the handle is
        // all of those anyway.
        assert_copy_send_sync_debug_eq_hash::<PlaceRef<Rc<u32>>>();
    }

    #[test]
    fn place_ref_is_copy_in_use() {
        let a = PlaceRef::<String>::new(PlaceId(3));
        let b = a;
        assert_eq!(a, b);
        assert_eq!(a.id(), PlaceId(3));
    }
}
