//! Identifiers: erased ids used by events and the read API, and the typed
//! place handle the builder hands back.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU32, Ordering};

/// Identifies one net within this process run.
///
/// Every [`NetBuilder`](crate::NetBuilder) mints one, and every
/// [`PlaceRef`] it hands out carries it. The net compares the stamp before it
/// uses the index, so a handle from another net is caught instead of silently
/// addressing whatever place happens to sit at that index — which matters as
/// soon as two nets are in scope at once, as in composition, where one net is
/// the body of a transition in another.
///
/// The id is unique within one process run only. It is not stable across
/// runs, is never serialised, and means nothing outside the process that
/// minted it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NetId(pub(crate) u32);

/// The source of [`NetId`]s. `Relaxed` is enough: the only requirement is
/// that no two builders get the same number, which `fetch_add` guarantees on
/// its own — nothing else is published through this counter.
static NEXT_NET_ID: AtomicU32 = AtomicU32::new(0);

impl NetId {
    /// Mints the next id. Called once per [`NetBuilder`](crate::NetBuilder).
    pub(crate) fn mint() -> NetId {
        NetId(NEXT_NET_ID.fetch_add(1, Ordering::Relaxed))
    }
}

/// Identifies a place within one [`Net`](crate::Net).
///
/// The id is an index into the net's place list. Ids are never reused, so an
/// id remains valid (or, once topology editing exists, a tombstone) for the
/// life of the net.
///
/// A `PlaceId` carries no [`NetId`]: it is the cheap read-API handle, used by
/// events, guards and inspectors that already know which net they are looking
/// at. The stamped handle is [`PlaceRef`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlaceId(pub(crate) u32);

/// Identifies a transition within one [`Net`](crate::Net).
///
/// Handed out in declaration order by
/// [`NetBuilder::transition`](crate::NetBuilder::transition) and never
/// reused. Like [`PlaceId`], it is unstamped.
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
/// travels with the handle, while the net stores tokens erased. It also
/// carries the [`NetId`] of the net that handed it out, so using it on
/// another net is caught rather than misread.
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
pub struct PlaceRef<T> {
    pub(crate) place: PlaceId,
    pub(crate) net: NetId,
    /// See the type-level comment: never `PhantomData<T>`.
    marker: PhantomData<fn() -> T>,
}

impl<T> PlaceRef<T> {
    /// Wraps a raw id. Only the builder knows that `place` really is a place
    /// of type `T` in the net `net`.
    pub(crate) fn new(net: NetId, place: PlaceId) -> Self {
        PlaceRef {
            place,
            net,
            marker: PhantomData,
        }
    }

    /// The erased id of this place, for the read API, events and
    /// [`Net::remove`](crate::Net::remove).
    pub fn id(self) -> PlaceId {
        self.place
    }

    /// The net this handle belongs to, as in [`Net::id`](crate::Net::id).
    pub fn net(self) -> NetId {
        self.net
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
            .field(&self.place)
            .field(&self.net)
            .field(&std::any::type_name::<T>())
            .finish()
    }
}

/// Two handles are the same handle only if they point at the same place *of
/// the same net*: the index alone is not an identity, because every net
/// numbers its places from zero.
impl<T> PartialEq for PlaceRef<T> {
    fn eq(&self, other: &Self) -> bool {
        self.place == other.place && self.net == other.net
    }
}

impl<T> Eq for PlaceRef<T> {}

impl<T> Hash for PlaceRef<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.place.hash(state);
        self.net.hash(state);
    }
}

/// What a method says when it is handed a place from another net.
///
/// Written once because both the builder and the net say it, and because the
/// interesting part is the pair of net ids: an author who sees them knows at
/// once that two nets are in play, which is the actual mistake.
pub(crate) fn foreign_place(method: &str, place: PlaceId, from: NetId, here: NetId) -> String {
    format!(
        "{method}: this handle points at place #{} of net #{}, but this is net #{} \
         (a PlaceRef only works with the net whose builder handed it out)",
        place.0, from.0, here.0
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::hash_map::DefaultHasher;
    use std::rc::Rc;

    fn assert_copy_send_sync_debug_eq_hash<T: Copy + Send + Sync + std::fmt::Debug + Eq + Hash>() {}

    fn hash_of<T: Hash>(value: &T) -> u64 {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn place_ref_traits_do_not_depend_on_t() {
        // `Rc<u32>` is neither `Send`, `Sync`, `Copy` nor `Eq`; the handle is
        // all of those anyway.
        assert_copy_send_sync_debug_eq_hash::<PlaceRef<Rc<u32>>>();
    }

    #[test]
    fn place_ref_is_copy_in_use() {
        let net = NetId(7);
        let a = PlaceRef::<String>::new(net, PlaceId(3));
        let b = a;
        assert_eq!(a, b);
        assert_eq!(a.id(), PlaceId(3));
        assert_eq!(a.net(), net);
    }

    #[test]
    fn the_same_index_in_two_nets_is_two_handles() {
        let here = PlaceRef::<String>::new(NetId(1), PlaceId(3));
        let there = PlaceRef::<String>::new(NetId(2), PlaceId(3));
        assert_ne!(here, there);
        assert_ne!(hash_of(&here), hash_of(&there));
    }

    #[test]
    fn net_ids_are_unique() {
        let a = NetId::mint();
        let b = NetId::mint();
        assert_ne!(a, b);
    }
}
