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
/// runs and means nothing outside the process that minted it.
///
/// It is also the id of an inner net, in a net whose tokens carry nets of
/// their own: [`Event::Nested`](crate::Event::Nested) names the inner net it
/// lifted an event from by its `NetId`, which stays the same while the token
/// moves and its [`TokenId`] does not. So it prints as its bare number and,
/// with the `serde` feature, serialises as one (never deserialises) — as an
/// opaque, process-local id to follow a token by, not as something to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
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

    /// The number this id is: what `Display` prints and the `serde` feature
    /// writes.
    pub const fn get(self) -> u32 {
        self.0
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
///
/// With the `serde` feature it is a bare JSON number, so that a
/// `JsonCommand` and an event log read as data rather than as wrappers.
/// [`NetId`] serialises too, but only one way: it means nothing outside the
/// process that minted it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct PlaceId(pub(crate) u32);

/// Identifies a transition within one [`Net`](crate::Net).
///
/// Handed out in declaration order by
/// [`NetBuilder::transition`](crate::NetBuilder::transition) and never
/// reused. Like [`PlaceId`], it is unstamped, and a bare JSON number with the
/// `serde` feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct TransitionId(pub(crate) u32);

/// Identifies one token within one [`Net`](crate::Net).
///
/// Token ids are handed out in increasing order and are never reused: a
/// removed or consumed token retires its id for good, so a visualiser can key
/// its own state by `TokenId` without fear of collision.
///
/// It is the one id a `JsonCommand` speaks in — places and transitions
/// travel by name there, but a token has no name, and its id is what the
/// event log already gave the sender. A bare JSON number with the `serde`
/// feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct TokenId(pub(crate) u64);

/// The ids print as their bare number, the spelling the `serde` feature
/// uses, so a visualiser or a log can say `#7` without going through
/// `Debug`.
macro_rules! display_as_number {
    ($($id:ident),*) => {$(
        impl fmt::Display for $id {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }
    )*};
}
display_as_number!(NetId, PlaceId, TransitionId, TokenId);

/// The ids parse from their bare number, the inverse of `Display` and the
/// spelling a wire protocol uses for a token id (`"17"`).
macro_rules! parse_as_number {
    ($($id:ident : $int:ty),*) => {$(
        impl std::str::FromStr for $id {
            type Err = std::num::ParseIntError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                s.parse::<$int>().map($id)
            }
        }
    )*};
}
parse_as_number!(PlaceId: u32, TransitionId: u32, TokenId: u64);

impl PlaceId {
    /// A place id from its index in the net's place list: the number
    /// `Display` prints and a [`Topology`](crate::Topology) entry carries.
    ///
    /// For a mirror of a net in another process, which numbers places in
    /// topology order (DESIGN §19). An id the net never issued is harmless:
    /// the read API answers `None`, `false` or nothing for it, and no typed
    /// [`PlaceRef`] can be made from one.
    pub const fn new(index: u32) -> PlaceId {
        PlaceId(index)
    }

    /// The index this id is.
    pub const fn index(self) -> u32 {
        self.0
    }
}

impl TransitionId {
    /// A transition id from its index in the net's transition list, as
    /// [`PlaceId::new`].
    pub const fn new(index: u32) -> TransitionId {
        TransitionId(index)
    }

    /// The index this id is.
    pub const fn index(self) -> u32 {
        self.0
    }
}

impl TokenId {
    /// A token id from its number: what `Display` prints, what the event log
    /// and a wire protocol carry.
    ///
    /// For a mirror that reads ids off the wire (DESIGN §19). A number the
    /// net never issued, or has retired, is harmless: the read API answers
    /// `None` for it.
    pub const fn new(number: u64) -> TokenId {
        TokenId(number)
    }

    /// The number this id is.
    pub const fn get(self) -> u64 {
        self.0
    }
}

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

    #[test]
    fn ids_display_as_their_bare_number() {
        assert_eq!(PlaceId(3).to_string(), "3");
        assert_eq!(TransitionId(4).to_string(), "4");
        assert_eq!(format!("#{}", TokenId(7)), "#7");
        assert_eq!(NetId(5).to_string(), "5");
        assert_eq!(NetId(5).get(), 5);
    }
}
