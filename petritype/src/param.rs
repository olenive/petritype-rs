//! Parameters: the inputs of a transition that are not places (DESIGN §10).
//!
//! Python petritype injected run-varying values — secrets, API tokens, tuning
//! constants — into a transition as `kwargs`, and changed them at run time
//! with `SetParam`, without adding a node to the diagram. [`Param<T>`] is the
//! Rust equivalent: an input position declared alongside the input edges,
//! which hands the body a **clone** of the net's current value of type `T`.
//!
//! ```
//! # use petritype::{NetBuilder, Param, TakeEdge};
//! #[derive(Clone)]
//! struct ApiToken(String);
//!
//! let mut b = NetBuilder::new();
//! let urls = b.place::<String>("Urls");
//! let pages = b.place::<String>("Pages");
//! b.param(ApiToken("s3cret".into()));
//! b.transition(
//!     "Fetch",
//!     (TakeEdge(urls), Param::<ApiToken>::new()),
//!     (pages,),
//!     |url: String, token: ApiToken| (format!("{url}?token={}", token.0),),
//! );
//! # let net = b.build()?;
//! # Ok::<(), petritype::BuildError>(())
//! ```
//!
//! A parameter is stored once per net, keyed by [`TypeId`], so two values of
//! one underlying type need newtypes of their own. It is not a place and not
//! an edge: it never appears in [`Net::edges`](crate::Net::edges), never
//! affects [fireability](crate::Net::fireable), and is never consumed,
//! restored or reported in an event other than
//! [`Event::ParamChanged`](crate::Event::ParamChanged).

use std::any::{TypeId, type_name};
use std::fmt;
use std::marker::PhantomData;

use crate::edge::{InputEdge, InputSpec, owned_token};
use crate::erased::ErasedCursor;
use crate::place::{CloneFn, ErasedToken};
#[cfg(feature = "serde")]
use crate::place::{FromJsonFn, ToJsonFn};
use crate::sealed::EdgeSealed;

/// One input position of a transition that is not a place: the body is handed
/// a clone of the net's current value of type `T` (DESIGN §10).
///
/// Written `Param::<T>::new()` in an input tuple, beside the edges:
///
/// ```
/// # use petritype::{NetBuilder, Param, TakeEdge};
/// # #[derive(Clone)] struct Scale(u32);
/// # let mut b = NetBuilder::new();
/// # let inbox = b.place::<u32>("Inbox");
/// # let out = b.place::<u32>("Out");
/// b.param(Scale(3));
/// b.transition(
///     "Scale",
///     (TakeEdge(inbox), Param::<Scale>::new()),
///     (out,),
///     |n: u32, scale: Scale| (n * scale.0,),
/// );
/// # let net = b.build()?;
/// # Ok::<(), petritype::BuildError>(())
/// ```
///
/// DESIGN §10 writes the bare `Param::<ApiToken>`; a struct with a
/// [`PhantomData`] field needs a value, so the spelling here is
/// `Param::<ApiToken>::new()` (or `Param::default()` where the type can be
/// inferred). Like the edge handles it is `Copy` and carries no data at all —
/// the type parameter *is* the declaration.
///
/// # What the body receives
///
/// A clone, taken when the firing is launched. The body may run on another
/// thread and cannot borrow the net, and a
/// [`set_param`](crate::Net::set_param) between the launch and the landing
/// must not change what an airborne body is working with. A parameter with
/// heavy fields puts them in an `Arc`, as a token does.
///
/// # Bounds
///
/// `T: Clone + Send + Sync + 'static`, and deliberately **not** `Debug`: a
/// parameter may be a secret, and nothing in the engine ever prints one. A
/// visualiser is told the type name — [`TransitionInfo::params`](crate::TransitionInfo::params)
/// — and no more.
pub struct Param<T>(PhantomData<fn() -> T>);

impl<T> Param<T> {
    /// Declares an input position for the parameter of type `T`.
    ///
    /// The value itself is registered on the builder with
    /// [`NetBuilder::param`](crate::NetBuilder::param); a transition that
    /// declares a parameter no value was registered for is a
    /// [`BuildError::MissingParam`](crate::BuildError::MissingParam).
    pub fn new() -> Self {
        Param(PhantomData)
    }
}

impl<T> Default for Param<T> {
    fn default() -> Self {
        Param::new()
    }
}

/// Hand-written, like the edge handles': a `Param<T>` is a declaration, not a
/// value, so it is `Copy` and printable whatever `T` is — including a `T`
/// that is deliberately not `Debug`.
impl<T> Clone for Param<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Param<T> {}

impl<T> fmt::Debug for Param<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Param<{}>", type_name::<T>())
    }
}

/// The parameter is handed over as one owned token, exactly as a
/// [`TakeEdge`](crate::TakeEdge) is: stage one cloned the net's value into it.
impl<T: Clone + Send + Sync + 'static> EdgeSealed for Param<T> {
    type Arg<'a> = T;

    fn extract(inputs: &mut ErasedCursor<'_, '_>) -> T {
        owned_token::<T>(inputs.next())
    }

    fn describe(&self, out: &mut Vec<InputSpec>) {
        out.push(InputSpec::Param {
            type_id: TypeId::of::<T>(),
            type_name: type_name::<T>(),
        });
    }
}

impl<T: Clone + Send + Sync + 'static> InputEdge for Param<T> {}

/// One parameter value as the net holds it: erased, with the cloner that
/// copies it for a body and, under `serde`, the wire name and codecs that let
/// a JSON command replace it.
///
/// The same hand-rolled vtable idea as [`PlaceMeta`](crate::place::PlaceMeta),
/// and the same functions — [`clone_of`](crate::place::clone_of),
/// [`to_json_of`](crate::place::to_json_of),
/// [`from_json_of`](crate::place::from_json_of) — instantiated by the builder,
/// which is the one place `T` is still known.
pub(crate) struct ParamSlot {
    /// `TypeId::of::<T>()`: the key a parameter is stored under, and what an
    /// erased [`Command::SetParam`](crate::Command::SetParam) is matched
    /// against.
    pub(crate) type_id: TypeId,
    /// `std::any::type_name::<T>()`, the only thing about a parameter a
    /// reader is ever told.
    pub(crate) type_name: &'static str,
    /// The current value. Replaced whole by
    /// [`Net::set_param`](crate::Net::set_param).
    pub(crate) value: ErasedToken,
    /// `clone_of::<T>`: what stage one hands the body, and what a
    /// [`Snapshot`](crate::Snapshot) takes a copy with.
    pub(crate) clone: CloneFn,
    /// The name this parameter travels under in a
    /// [`JsonCommand`](crate::JsonCommand), as given to
    /// [`serializable_param`](crate::NetBuilder::serializable_param), or
    /// `None` for a parameter that cannot cross a wire.
    ///
    /// A wire key has to be chosen by a human, as a place's is:
    /// `type_name::<T>()` is a compiler detail, not a stable name.
    #[cfg(feature = "serde")]
    pub(crate) name: Option<String>,
    /// `to_json_of::<T>`, `Some` exactly when [`name`](ParamSlot::name) is.
    #[cfg(feature = "serde")]
    pub(crate) to_json: Option<ToJsonFn>,
    /// `from_json_of::<T>`, `Some` exactly when [`name`](ParamSlot::name) is.
    #[cfg(feature = "serde")]
    pub(crate) from_json: Option<FromJsonFn>,
}

impl ParamSlot {
    /// A copy of the current value, for a body about to run or for a
    /// [`Snapshot`](crate::Snapshot).
    pub(crate) fn clone_value(&self) -> ErasedToken {
        (self.clone)(&*self.value)
    }
}

/// Cloning a slot clones its value through the recorded cloner: a parameter
/// type is always `Clone`, so unlike a place this can never fail.
impl Clone for ParamSlot {
    fn clone(&self) -> Self {
        ParamSlot {
            type_id: self.type_id,
            type_name: self.type_name,
            value: self.clone_value(),
            clone: self.clone,
            #[cfg(feature = "serde")]
            name: self.name.clone(),
            #[cfg(feature = "serde")]
            to_json: self.to_json,
            #[cfg(feature = "serde")]
            from_json: self.from_json,
        }
    }
}

/// The type name, never the value: a parameter may be a secret, and there is
/// no `Debug` bound on one anyway (DESIGN §10).
impl fmt::Debug for ParamSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = f.debug_struct("ParamSlot");
        out.field("type_name", &self.type_name);
        #[cfg(feature = "serde")]
        out.field("name", &self.name);
        out.finish_non_exhaustive()
    }
}

/// Where the parameter of this type is, if the net has one.
///
/// A linear scan: a net has a handful of parameters at most, and a scan over
/// that many `TypeId`s beats hashing one. The index rather than the slot,
/// because replacing a value needs a `&mut` that a shared lookup cannot hand
/// back.
pub(crate) fn param_slot_index(params: &[ParamSlot], type_id: TypeId) -> Option<usize> {
    params.iter().position(|slot| slot.type_id == type_id)
}

/// The slot holding the parameter of this type, if the net has one.
pub(crate) fn param_slot(params: &[ParamSlot], type_id: TypeId) -> Option<&ParamSlot> {
    Some(&params[param_slot_index(params, type_id)?])
}

/// The current value of the parameter of type `T`, downcast.
///
/// Written once because both [`Net::param`](crate::Net::param) and
/// [`Snapshot::param`](crate::Snapshot::param) are this function.
pub(crate) fn param_of<T: 'static>(params: &[ParamSlot]) -> Option<&T> {
    param_slot(params, TypeId::of::<T>())?
        .value
        .downcast_ref::<T>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    #[test]
    fn a_param_handle_does_not_depend_on_t() {
        fn assert_handle<H: Copy + Send + Sync + fmt::Debug>() {}

        // `Rc<u32>` is neither `Copy`, `Send` nor `Sync`, and the handle is
        // all three regardless — it carries no `T` at all.
        assert_handle::<Param<Rc<u32>>>();
    }

    #[test]
    fn a_param_describes_itself_by_type() {
        #[derive(Clone)]
        struct ApiToken;

        let mut specs = Vec::new();
        EdgeSealed::describe(&Param::<ApiToken>::new(), &mut specs);
        assert_eq!(
            specs,
            [InputSpec::Param {
                type_id: TypeId::of::<ApiToken>(),
                type_name: type_name::<ApiToken>(),
            }],
        );
    }

    #[test]
    fn debug_names_the_type_and_nothing_else() {
        struct Secret;
        assert!(format!("{:?}", Param::<Secret>::new()).contains("Secret"));
    }
}
