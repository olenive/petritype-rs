//! Erased token storage and the per-place vtable of monomorphised functions.

use std::any::{Any, TypeId, type_name};
use std::collections::VecDeque;
use std::fmt;

use crate::ids::{PlaceId, TokenId, TransitionId};
use crate::keys::KeysVtable;
use crate::nested::{NestedVtable, Stepping};
use crate::net::Net;
use crate::style::{PlaceStyle, StyleFn};

/// A token as the net stores it: owned, type-erased, and safe to move to
/// another thread with the net.
pub(crate) type ErasedToken = Box<dyn Any + Send + Sync>;

/// Erases a batch of typed tokens, keeping their order.
///
/// Written once because both halves of external input need it: the typed
/// methods on [`Net`](crate::Net), which erase on the way in, and the typed
/// constructors of [`Command`](crate::Command), which erase on the way to a
/// channel.
pub(crate) fn erase_all<T: Send + Sync + 'static>(
    tokens: impl IntoIterator<Item = T>,
) -> Vec<ErasedToken> {
    tokens
        .into_iter()
        .map(|token| Box::new(token) as ErasedToken)
        .collect()
}

/// Renders a token for a label. See [`debug_of`].
pub(crate) type DebugFn = fn(&dyn Any) -> String;

/// Deep-copies a token. See [`clone_of`].
pub(crate) type CloneFn = fn(&dyn Any) -> ErasedToken;

/// Serialises a token. See [`to_json_of`].
#[cfg(feature = "serde")]
pub(crate) type ToJsonFn = fn(&dyn Any) -> Result<serde_json::Value, serde_json::Error>;

/// Deserialises a token. See [`from_json_of`].
#[cfg(feature = "serde")]
pub(crate) type FromJsonFn = fn(serde_json::Value) -> Result<ErasedToken, serde_json::Error>;

/// Everything the net knows about a place other than its tokens.
///
/// `PlaceMeta` is a hand-rolled vtable. Instead of requiring the token type to
/// implement some `Token` trait and storing `Box<dyn Token>` per token, the
/// builder, which is the one place where the concrete `T` is still known,
/// instantiates the generic functions below and coerces them to plain `fn`
/// pointers. The cost is one pointer per place, not per token, and the token
/// itself needs no bound beyond what the builder method asked for.
#[derive(Clone)]
pub(crate) struct PlaceMeta {
    /// The name given in the builder; unique within a net.
    pub(crate) name: String,
    /// `std::any::type_name::<T>()`, for diagnostics and visualisers.
    pub(crate) type_name: &'static str,
    /// `TypeId::of::<T>()`, for the one runtime type check on the erased
    /// insertion path.
    pub(crate) type_id: TypeId,
    /// `debug_of::<T>`.
    pub(crate) debug: DebugFn,
    /// `clone_of::<T>`, or `None` for a place created with
    /// [`NetBuilder::place_no_clone`](crate::NetBuilder::place_no_clone).
    ///
    /// Its absence is what
    /// [`build`](crate::NetBuilder::build) rejects a snapshot edge, and a
    /// consuming edge of a transition that
    /// [restores its inputs](crate::TransitionBuilder::restore_tokens_on_failure),
    /// for. It is called by snapshot edges, by the clone stage one takes for
    /// such a transition, and by
    /// [`Net::snapshot`](crate::Net::snapshot), which is refused outright
    /// when any place lacks it
    /// ([`NetError::NoClonePlace`](crate::NetError::NoClonePlace)), except a
    /// nested place under the `serde` feature, whose tokens it forks.
    pub(crate) clone: Option<CloneFn>,
    /// `to_json_of::<T>`, `Some` only for a place marked
    /// [`serializable`](crate::NetBuilder::serializable).
    #[cfg(feature = "serde")]
    pub(crate) to_json: Option<ToJsonFn>,
    /// `from_json_of::<T>`, `Some` only for a place marked
    /// [`serializable`](crate::NetBuilder::serializable). It is what
    /// [`Command::from_json`](crate::Command::from_json) decodes a wire
    /// token with.
    #[cfg(feature = "serde")]
    pub(crate) from_json: Option<FromJsonFn>,
    /// `T::nets` and `T::nets_mut`, `Some` only for a place declared with
    /// [`place_nested`](crate::NetBuilder::place_nested): what lets the
    /// engine find, step and drain the inner nets of a token whose type it
    /// has forgotten (DESIGN §6.6). Such a place is also a no-clone place,
    /// so `clone` is `None` beside it; with the `serde` feature the vtable
    /// carries `T::fork` instead, which is what
    /// [`Net::snapshot`](crate::Net::snapshot) copies its tokens with.
    pub(crate) nested: Option<NestedVtable>,
    /// How the nets this place holds take their turns beside the net around
    /// them, as set by [`NetBuilder::stepping`](crate::NetBuilder::stepping)
    /// or [`Net::set_stepping`](crate::Net::set_stepping); the default,
    /// [`InnerFirst`](Stepping::InnerFirst), until then. Read only for a
    /// place that [holds nets](PlaceMeta::holds_nets).
    pub(crate) stepping: Stepping,
    /// The key names and the extractor declared with
    /// [`NetBuilder::keys`](crate::NetBuilder::keys), or `None` for a place
    /// whose tokens have no keys (DESIGN §8.3). Unlike the entries above it
    /// holds a closure, in an `Arc`, so cloning the metadata shares it.
    pub(crate) keys: Option<KeysVtable>,
    /// The function declared with
    /// [`NetBuilder::style`](crate::NetBuilder::style) (or
    /// [`styled`](crate::NetBuilder::styled)) that gives each token its
    /// [`Style`](crate::style::Style), or `None` for a place whose tokens
    /// have none. A closure in an `Arc`, as `keys` is.
    pub(crate) style: Option<StyleFn>,
    /// The rule declared with
    /// [`NetBuilder::place_style`](crate::NetBuilder::place_style) for
    /// drawing the place itself, or `None`.
    pub(crate) place_style: Option<PlaceStyle>,
    /// Whether the place is a subnet place
    /// ([`NetBuilder::subnet_place`](crate::NetBuilder::subnet_place)): it
    /// holds a subnet in its slot and no tokens of its own.
    pub(crate) subnet: bool,
    /// For the place that stands in this net for a port of one of its
    /// subnet places, where the port is. `None` for every other place. See
    /// [`Window`].
    pub(crate) window: Option<Window>,
    /// Whether the place was marked a port of this net with
    /// [`NetBuilder::port`](crate::NetBuilder::port), for a net around it to
    /// reach it through.
    pub(crate) port: bool,
    /// The position, in the net's declared order of groups, of the group
    /// [`NetBuilder::in_group`](crate::NetBuilder::in_group) put the place
    /// in, or `None` for a place in no group. Only drawings read it. A
    /// window is in none, whatever group its port is in inside the subnet.
    pub(crate) group: Option<u32>,
}

impl PlaceMeta {
    /// Whether the place holds nets that this net steps, and so has a
    /// [`Stepping`] rule: a place declared with
    /// [`place_nested`](crate::NetBuilder::place_nested), whose tokens carry
    /// nets of their own, or a subnet place. A window is neither: the nets
    /// in its port's tokens are its subnet's to step, under the port's own
    /// rule.
    pub(crate) fn holds_nets(&self) -> bool {
        (self.nested.is_some() || self.subnet) && self.window.is_none()
    }
}

/// Where the tokens of a *window* live: the place a net has for each port of
/// each of its subnet places.
///
/// A window is a real place of the net around the subnet, with its own
/// [`PlaceId`] there, the port's name and a copy of the port's metadata, so
/// that edges, guards, the read API and external input name it as any other
/// place. Its own queue stays empty: every function that touches a place's
/// tokens finds the port's queue in the subnet through this, along with the
/// subnet's key index and token counter, so that the port has one queue and
/// one set of ids whichever net looks at it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Window {
    /// The subnet place, in the net around the subnet.
    pub(crate) subnet: PlaceId,
    /// The port, as a place of the subnet.
    pub(crate) port: PlaceId,
}

/// What a subnet place holds: the subnet, and which of the outer net's
/// transitions reach it through its ports.
pub(crate) struct Subnet {
    /// The subnet itself, for the outer net's whole life.
    pub(crate) net: Net,
    /// Every transition of the outer net with an edge on one of this
    /// subnet's windows, in declaration order: while one of them is in
    /// flight the subnet takes no turn and cannot be entered.
    pub(crate) through: Vec<TransitionId>,
}

impl fmt::Debug for PlaceMeta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlaceMeta")
            .field("name", &self.name)
            .field("type_name", &self.type_name)
            .field(
                "keys",
                &self
                    .keys
                    .as_ref()
                    .map(|keys| &keys.names[..])
                    .unwrap_or(&[]),
            )
            .field("styled", &self.style.is_some())
            .field("place_style", &self.place_style)
            .field("subnet", &self.subnet)
            .field("window", &self.window)
            .field("port", &self.port)
            .field("group", &self.group)
            .finish_non_exhaustive()
    }
}

/// One place: its metadata and its tokens, oldest first.
pub(crate) struct PlaceSlot {
    pub(crate) meta: PlaceMeta,
    /// FIFO: tokens are pushed at the back and consumed from the front.
    /// Always empty for a subnet place and for a window.
    pub(crate) tokens: VecDeque<(TokenId, ErasedToken)>,
    /// The subnet of a subnet place, boxed so that every other place pays
    /// one pointer for it; `None` for every other place.
    pub(crate) subnet: Option<Box<Subnet>>,
}

impl PlaceSlot {
    /// The index of `token` in this place, or `None` if it holds no such
    /// token. Linear, because places are short and ordered.
    pub(crate) fn position(&self, token: TokenId) -> Option<usize> {
        self.tokens.iter().position(|(id, _)| *id == token)
    }

    /// A deep copy of this place (the same tokens under the same ids), or
    /// `None` for a place declared with
    /// [`place_no_clone`](crate::NetBuilder::place_no_clone), which recorded
    /// no cloner.
    ///
    /// This is what [`Net::snapshot`](crate::Net::snapshot) is built from,
    /// and the `None` is the whole of
    /// [`NetError::NoClonePlace`](crate::NetError::NoClonePlace), but for a
    /// nested place under the `serde` feature, which the snapshot forks token
    /// by token instead. The [`PlaceMeta`] comes along, so a copied place
    /// answers the read API (labels, JSON, counts) exactly as the original
    /// does.
    pub(crate) fn try_clone(&self) -> Option<PlaceSlot> {
        let clone = self.meta.clone?;
        Some(PlaceSlot {
            // Spelled out: `meta.clone` is the cloner *of a token*, and
            // `meta.clone()` would be the metadata's own `Clone`, which is
            // what is wanted here but reads like a typo.
            meta: Clone::clone(&self.meta),
            tokens: self
                .tokens
                .iter()
                .map(|(id, value)| (*id, clone(&**value)))
                .collect(),
            // A subnet place records no cloner, so it never gets this far.
            subnet: None,
        })
    }
}

impl fmt::Debug for PlaceSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = f.debug_struct("PlaceSlot");
        out.field("meta", &self.meta)
            .field("count", &self.tokens.len());
        if let Some(subnet) = &self.subnet {
            out.field("subnet", &subnet.net.id());
        }
        out.finish()
    }
}

/// The type id of a subnet place: a type no value outside this crate can
/// have, so that the one type check on the insertion path refuses every
/// token offered to a subnet place.
#[derive(Debug)]
pub(crate) struct SubnetMarker;

/// The type name a subnet place reports, a stand-in in the style of
/// `"<erased>"`: it holds no tokens, so it has no token type to name.
pub(crate) const SUBNET_TYPE_NAME: &str = "<subnet>";

/// `debug_of::<T>` formats an erased token with `T`'s `Debug`.
///
/// The `&dyn Any` argument is always a token of this place's type, so the
/// downcast always succeeds; the fallback string exists so that a bug here
/// shows up in a label rather than as a panic inside a visualiser.
pub(crate) fn debug_of<T: fmt::Debug + 'static>(value: &dyn Any) -> String {
    match value.downcast_ref::<T>() {
        Some(value) => format!("{value:?}"),
        None => format!("<not a {}>", type_name::<T>()),
    }
}

/// `clone_of::<T>` deep-copies an erased token.
///
/// Recorded by [`NetBuilder::place`](crate::NetBuilder::place) and never
/// called unless the author asks for a snapshot, a snapshot edge or token
/// restoration on failure (once per consumed token per firing, in that last
/// case).
pub(crate) fn clone_of<T: Clone + Send + Sync + 'static>(value: &dyn Any) -> ErasedToken {
    Box::new(
        value
            .downcast_ref::<T>()
            .expect("token in a place is always of the place's type")
            .clone(),
    )
}

/// `to_json_of::<T>` renders an erased token as JSON.
///
/// Fallible, because a `Serialize` impl may refuse: a token holding a
/// [`Net`](crate::Net) that no template built, for one. Readers that only
/// want something to show take [`json_or_message`]; a save takes the error.
#[cfg(feature = "serde")]
pub(crate) fn to_json_of<T: serde::Serialize + 'static>(
    value: &dyn Any,
) -> Result<serde_json::Value, serde_json::Error> {
    match value.downcast_ref::<T>() {
        Some(value) => serde_json::to_value(value),
        None => Ok(serde_json::Value::Null),
    }
}

/// A token as JSON for a reader rather than a save: a token whose
/// `Serialize` refuses shows the refusal as a string, as it always has on
/// [`Net::json`](crate::Net::json) and
/// [`Command::to_json`](crate::Command::to_json).
#[cfg(feature = "serde")]
pub(crate) fn json_or_message(to_json: ToJsonFn, value: &dyn Any) -> serde_json::Value {
    to_json(value).unwrap_or_else(|error| serde_json::Value::String(error.to_string()))
}

/// `from_json_of::<T>` parses a token from JSON, erasing it again.
#[cfg(feature = "serde")]
pub(crate) fn from_json_of<T>(value: serde_json::Value) -> Result<ErasedToken, serde_json::Error>
where
    T: serde::de::DeserializeOwned + Send + Sync + 'static,
{
    serde_json::from_value::<T>(value).map(|value| Box::new(value) as ErasedToken)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_of_uses_the_concrete_type() {
        let token: ErasedToken = Box::new(vec![1_u8, 2]);
        let debug: DebugFn = debug_of::<Vec<u8>>;
        assert_eq!(debug(&*token), "[1, 2]");
    }

    #[test]
    fn debug_of_does_not_panic_on_the_wrong_type() {
        let token: ErasedToken = Box::new(7_u32);
        let debug: DebugFn = debug_of::<String>;
        assert!(debug(&*token).starts_with("<not a "));
    }

    #[test]
    fn clone_of_round_trips() {
        let token: ErasedToken = Box::new(String::from("x"));
        let clone: CloneFn = clone_of::<String>;
        let copy = clone(&*token);
        assert_eq!(copy.downcast_ref::<String>().unwrap(), "x");
    }
}
