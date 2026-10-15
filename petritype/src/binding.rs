//! What the code `#[derive(Inputs)]` and `#[derive(Outputs)]` generate
//! names: the engine traits and types a struct of edges implements and
//! calls, public so that a derive in another crate can reach them, and
//! hidden because nothing else should.
//!
//! The derives are the documented way to bind a body to a net by field name
//! (DESIGN §4.4, §4.5); [`Inputs`](crate::Inputs) and
//! [`Outputs`](crate::Outputs) say what they generate and why.
//! This module is to them what [`routing`](crate::routing) is to
//! [`Routed`](crate::Routed), with one difference: `routing` is a public
//! API in its own right, and this is not. Nothing here is covered by the
//! crate's stability promises, and an impl written by hand against it is a
//! contract the author keeps alone — a position that describes one number
//! of edges and takes another panics the firing.

use std::any::TypeId;

pub use crate::erased::ErasedCursor as Cursor;
pub use crate::sealed::{EdgeSealed, PlaceTupleSealed};

use crate::body::Accepts;
use crate::edge::{InputSpec, Reads};
use crate::erased::OutputTarget;
use crate::ids::{NetId, PlaceId};
use crate::place::ErasedToken;

/// What [`EdgeSealed::describe`] appends to: one spec per edge, in order.
///
/// An alias, so that the generated impl can spell the parameter's type
/// without the spec itself being nameable.
pub type Specs = Vec<InputSpec>;

/// The engine half of [`Reads<V>`](crate::Reads): takes a field's value out
/// of the erased inputs, as the edge's own extractor does.
///
/// Separate from `Reads` because of the two mutating kinds. A field of
/// type `&'a mut T` borrows the token for the struct's lifetime `'a`, which
/// only the cursor's own lifetime can fill: `MutateEdge<T>` reads
/// `&'a mut T` for every `'a`, and extracts `&'a mut T` at the cursor's
/// `'a` alone. The derive asks for this trait `for<'a>`, with the struct's
/// lifetime (or a fresh one) standing in for the cursor's, and for `Reads`
/// on the wiring struct itself, where the message is written.
///
/// Implemented beside each `Reads` impl, for the eight edge kinds and
/// [`Param<T>`](crate::Param), with the same bounds.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot fill a field of type `{V}`",
    label = "this input does not read `{V}`",
    note = "each kind of input reads one shape: `T` from `TakeEdge`, `SnapshotEdge`, \
            `TakeWhereEdge` or `Param::<T>::new()`; `Vec<T>` from `TakeAllEdge`, `DrainEdge` \
            or `SurveyEdge`; `&mut T` from `MutateEdge` or `MutateWhereEdge`"
)]
pub trait Extract<'a, V>: Reads<V> {
    /// Takes this field's value: one erased input, from the cursor.
    fn extract(inputs: &mut Cursor<'a, '_>) -> V;
}

/// What [`PlaceTupleSealed::describe`] appends to: `(place, net)` per output.
pub type Places = Vec<(PlaceId, NetId)>;

/// What [`PlaceTupleSealed::describe_types`] appends to: the token type of
/// each output.
pub type PlaceTypes = Vec<(TypeId, &'static str)>;

/// Appends the place an output field is wired to.
pub fn output_place<F: Accepts<V>, V>(field: &F, out: &mut Places) {
    out.push(field.place());
}

/// Appends the token type of the place an output field is wired to.
pub fn output_type<F: Accepts<V>, V>(out: &mut PlaceTypes) {
    out.push(F::token_type());
}

/// A struct a body returns, bound by name to `Outputs`, the struct of
/// outputs `#[derive(Outputs)]` generated for it: the named return form
/// ([`marker::Named`](crate::marker::Named)).
///
/// The derive implements it for the returned struct, once for its own
/// struct of outputs, which is how the named form is chosen: only that
/// struct's outputs make the impl apply.
pub trait NamedOutputs<Outputs> {
    /// Hands each field to [`Emit::field`], in field order: the order the
    /// struct of outputs described its places in.
    fn emit(self, out: &mut Emit<'_>);
}

/// Where [`NamedOutputs::emit`] puts a returned struct's tokens: field by
/// field, each addressed to the output of its position.
pub struct Emit<'a> {
    /// The tokens so far, each with its output index.
    out: &'a mut Vec<(OutputTarget, ErasedToken)>,
    /// The output index of the next field.
    next: usize,
}

impl<'a> Emit<'a> {
    /// A sink writing into one firing's token list, starting at output 0.
    pub(crate) fn new(out: &'a mut Vec<(OutputTarget, ErasedToken)>) -> Self {
        Emit { out, next: 0 }
    }

    /// Emits the next field's value, as the tokens its output makes of it.
    pub fn field<F: Accepts<V>, V>(&mut self, value: V) {
        F::push(value, self.next, self.out);
        self.next += 1;
    }
}

impl std::fmt::Debug for Emit<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Emit")
            .field("next", &self.next)
            .field("pushed", &self.out.len())
            .finish()
    }
}
