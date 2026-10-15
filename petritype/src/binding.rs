//! What the code `#[derive(Inputs)]` generates names: the engine traits and
//! types a struct of edges implements and calls, public so that a derive in
//! another crate can reach them, and hidden because nothing else should.
//!
//! The derives are the documented way to bind a body to a net by field name
//! (DESIGN §4.4); [`Inputs`](crate::Inputs) says what they generate and why.
//! This module is to them what [`routing`](crate::routing) is to
//! [`Routed`](crate::Routed), with one difference: `routing` is a public
//! API in its own right, and this is not. Nothing here is covered by the
//! crate's stability promises, and an impl written by hand against it is a
//! contract the author keeps alone — a position that describes one number
//! of edges and takes another panics the firing.

pub use crate::erased::ErasedCursor as Cursor;
pub use crate::sealed::EdgeSealed;

use crate::edge::{InputSpec, Reads};

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
