//! Bodies and outputs: the traits that let one `transition()` call accept a
//! closure of any shape, and check it against the declared edges and places.
//!
//! Three questions have to be answered at compile time:
//!
//! 1. Do the body's parameters match the input edges? — [`TransitionBody`],
//!    implemented for every arity from 0 to 12.
//! 2. Does what the body returns match the output places? —
//!    [`PositionalOutputs`], slot by slot, with [`IntoTokens`] deciding how
//!    many tokens one slot is worth.
//! 3. Is the return value an outcome at all? — [`IntoOutcome`], which also
//!    recognises `Result` as failure rather than as data.
//!
//! The answers are wrapped, once, into an
//! [`ErasedBody`](crate::erased::ErasedBody) at declaration time, so firing
//! costs no type juggling at all.

use std::error::Error;

use crate::arity::for_each_arity;
use crate::edge::InputEdge;
use crate::erased::{ErasedBody, ErasedInputs, ErasedOutcome};
use crate::ids::{NetId, PlaceId, PlaceRef};
use crate::place::ErasedToken;
use crate::sealed::{BodySealed, EdgeSealed, OutcomeSealed, PlaceTupleSealed, PositionalSealed};

pub mod marker {
    //! Type-level tags that tell the output forms apart.
    //!
    //! A transition body may return its outputs in more than one form, and
    //! the compiler picks the right reading by picking an impl. Two impls for
    //! the same return type would overlap, so each form carries a tag in the
    //! `Marker` parameter of [`IntoOutcome`](super::IntoOutcome) and
    //! [`TransitionBody`](super::TransitionBody) — the trick Bevy's
    //! `IntoSystem` and axum's `Handler` use for the same reason.
    //!
    //! You never name these types: the compiler infers them. They are public
    //! because they appear in the traits' type arguments.

    use std::marker::PhantomData;

    /// The body returned a tuple: slot *i* goes to output place *i*.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct Positional;

    /// The body returned `Result<O, E>`, where `O` is itself an outcome in
    /// the form `M`. Failure wraps any other form rather than replacing it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct Failing<M>(PhantomData<fn() -> M>);
}

/// How many tokens one positional output slot is worth.
///
/// For an output place of type `T`, a slot may be:
///
/// - `T` — exactly one token;
/// - `Option<T>` — zero or one;
/// - `Vec<T>` — as many as it holds, in order.
///
/// The place's type decides which reading applies, so there is no ambiguity
/// when `T` is itself an `Option` or a `Vec`: a slot of type `Option<u32>`
/// feeding a `PlaceRef<Option<u32>>` is **one** token holding an `Option`,
/// while the same value feeding a `PlaceRef<u32>` is zero or one `u32`.
/// Several tokens into a `Vec<u32>` place therefore need a
/// `Vec<Vec<u32>>` slot.
#[diagnostic::on_unimplemented(
    message = "an output slot of type `{Self}` does not fit an output place of type `{T}`",
    label = "not `{T}`, `Option<{T}>` or `Vec<{T}>`",
    note = "a positional output slot holds one token (`{T}`), zero or one (`Option<{T}>`) \
            or many (`Vec<{T}>`)"
)]
pub trait IntoTokens<T> {
    /// Hands over each token this slot produced, in order.
    ///
    /// `&mut dyn FnMut` rather than a generic sink: this is called once per
    /// slot per firing, the work is a push into a `Vec`, and one indirection
    /// costs less than an impl per slot type per arity.
    fn for_each_token(self, push: &mut dyn FnMut(T));
}

/// One value, one token.
impl<T> IntoTokens<T> for T {
    fn for_each_token(self, push: &mut dyn FnMut(T)) {
        push(self);
    }
}

/// Zero or one token: the slot that says "maybe nothing came of it".
impl<T> IntoTokens<T> for Option<T> {
    fn for_each_token(self, push: &mut dyn FnMut(T)) {
        if let Some(value) = self {
            push(value);
        }
    }
}

/// As many tokens as the vector holds, in order.
impl<T> IntoTokens<T> for Vec<T> {
    fn for_each_token(self, push: &mut dyn FnMut(T)) {
        for value in self {
            push(value);
        }
    }
}

/// A tuple of place handles, `()` to twelve of them: a transition's outputs.
///
/// The tuple is positional, so its order is the order of the slots in what
/// the body returns. A single output still needs the trailing comma:
/// `(count,)`.
#[diagnostic::on_unimplemented(
    message = "a transition's outputs must be a tuple of place handles",
    label = "not a tuple of place handles",
    note = "write `(heavy, light, count)`, or `()` for a transition that produces nothing; \
            a single output needs the trailing comma: `(count,)`"
)]
pub trait OutputPlaces: PlaceTupleSealed {}

/// What a body returns when its outputs are positional: a tuple with one slot
/// per output place.
///
/// Slot *i* must be [`IntoTokens`] for output place *i*'s type, which is what
/// makes a wrong order between differently-typed places a compile error. Two
/// places of the *same* type are distinguished only by order, and the author
/// has to care anyway.
#[diagnostic::on_unimplemented(
    message = "a transition body must return one slot per output place",
    label = "`{Self}` does not match the output places",
    note = "outputs `{Outputs}` want a tuple of that many slots, where slot i is `T`, \
            `Option<T>` or `Vec<T>` for the type of output place i",
    note = "returning a value that is not a tuple, to be routed to the output place of its \
            own type, is not supported yet"
)]
pub trait PositionalOutputs<Outputs>: PositionalSealed<Outputs> {}

/// What a transition body may return.
///
/// Either the outputs themselves, or `Result<Outputs, E>`. A `Result` always
/// means failure, never routing (DESIGN §7): a body that wants to send a
/// value to one place or another returns data that says so, not an `Err`.
///
/// # How later output forms plug in
///
/// The `Marker` parameter exists so that new forms can be added without
/// colliding with this slice's impls. Routed outputs will add
/// `impl<O: RoutedOutput> IntoOutcome<Outputs, marker::Routed> for O` and the
/// `Distribution` form another impl with its own tag; because the tags
/// differ, none of them overlaps the positional impl, and the compiler picks
/// the form by picking the impl whose bounds hold. `Result` needs no new
/// impl for either: [`marker::Failing<M>`] carries the inner form's tag.
#[diagnostic::on_unimplemented(
    message = "a transition body must return its outputs, or a `Result` of them",
    label = "`{Self}` is not a transition outcome",
    note = "return a tuple with one slot per output place, `()` if there are none, or \
            `Result<Outputs, E>` where `E: std::error::Error + Send + Sync + 'static`",
    note = "a `Result` always means failure, never routing"
)]
pub trait IntoOutcome<Outputs, Marker>: OutcomeSealed<Outputs, Marker> {}

impl<Outputs, O: PositionalOutputs<Outputs>> OutcomeSealed<Outputs, marker::Positional> for O {
    fn into_outcome(self) -> ErasedOutcome {
        let mut tokens = Vec::new();
        PositionalSealed::push_erased(self, &mut tokens);
        ErasedOutcome::Ok(tokens)
    }
}

impl<Outputs, O: PositionalOutputs<Outputs>> IntoOutcome<Outputs, marker::Positional> for O {}

impl<Outputs, O, E, M> OutcomeSealed<Outputs, marker::Failing<M>> for Result<O, E>
where
    O: IntoOutcome<Outputs, M>,
    E: Error + Send + Sync + 'static,
{
    fn into_outcome(self) -> ErasedOutcome {
        match self {
            Ok(outputs) => OutcomeSealed::into_outcome(outputs),
            // Erased here, at the edge of the typed API, so that the engine
            // and the event log speak one error type whatever bodies return.
            Err(error) => ErasedOutcome::Err(Box::new(error)),
        }
    }
}

impl<Outputs, O, E, M> IntoOutcome<Outputs, marker::Failing<M>> for Result<O, E>
where
    O: IntoOutcome<Outputs, M>,
    E: Error + Send + Sync + 'static,
{
}

/// A closure that can be a transition's body, given those input edges and
/// those output places.
///
/// Implemented for every `FnMut` of arity 0 to 12 whose parameters are what
/// the edges produce and whose return value is an [`IntoOutcome`]. The body
/// is `FnMut`, so it may keep private state between firings, and
/// `Send + Sync + 'static`, because it may run on another thread and the net
/// itself must stay `Send + Sync`.
///
/// `Marker` is inferred; see [`marker`].
#[diagnostic::on_unimplemented(
    message = "transition body parameters do not match its input edges",
    label = "this body cannot be called with what `{Edges}` produces",
    note = "one parameter per edge, in order: `TakeEdge(p)` gives `T`, `TakeAllEdge(p)` gives \
            `Vec<T>`, `SnapshotEdge(p)` gives `T`, `MutateEdge(p)` gives `&mut T`",
    note = "a body must be `Send + Sync + 'static`: capture an `Arc`, a `Mutex` or an atomic \
            rather than an `Rc` or a `RefCell`"
)]
pub trait TransitionBody<Edges, Outputs, Marker>: BodySealed<Edges, Outputs, Marker> {}

/// The heart of the slice: one impl per input arity.
///
/// The marker is `(M, R)` — the outcome form and the body's return type —
/// because an impl may not leave a type parameter to be pinned by a where
/// clause alone, and `R` appears only in the `FnMut` bound. Putting it in the
/// marker constrains it and costs nothing: the compiler infers the whole
/// marker at the call site.
macro_rules! impl_body {
    ($(($t:ident, $r:ident, $i:tt)),*) => {
        impl<F, R, M, Outputs, $($t,)*> BodySealed<($($t,)*), Outputs, (M, R)> for F
        where
            $($t: InputEdge,)*
            F: for<'a> FnMut($(<$t as EdgeSealed>::Arg<'a>,)*) -> R + Send + Sync + 'static,
            R: IntoOutcome<Outputs, M>,
        {
            #[allow(unused_mut, unused_variables)]
            fn into_erased(mut self) -> ErasedBody {
                Box::new(move |mut inputs: ErasedInputs<'_>| {
                    // One argument per edge, taken in declaration order.
                    // `iter_mut` and not indexing: the items borrow the
                    // slice rather than the iterator, so a `&mut` argument
                    // may still be alive when the next one is taken.
                    let mut args = inputs.iter_mut();
                    let returned = (self)($(
                        <$t as EdgeSealed>::extract(
                            args.next().expect("one erased input per input edge"),
                        ),
                    )*);
                    OutcomeSealed::into_outcome(returned)
                })
            }
        }

        impl<F, R, M, Outputs, $($t,)*> TransitionBody<($($t,)*), Outputs, (M, R)> for F
        where
            $($t: InputEdge,)*
            F: for<'a> FnMut($(<$t as EdgeSealed>::Arg<'a>,)*) -> R + Send + Sync + 'static,
            R: IntoOutcome<Outputs, M>,
        {
        }
    };
}

for_each_arity!(impl_body);

/// One impl per output arity, for both the place tuple and the value tuple
/// that fills it.
macro_rules! impl_outputs {
    ($(($t:ident, $r:ident, $i:tt)),*) => {
        impl<$($t: Send + Sync + 'static,)*> PlaceTupleSealed for ($(PlaceRef<$t>,)*) {
            #[allow(unused_variables, non_snake_case)]
            fn describe(&self, out: &mut Vec<(PlaceId, NetId)>) {
                let ($($t,)*) = self;
                $(out.push(($t.id(), $t.net()));)*
            }
        }

        impl<$($t: Send + Sync + 'static,)*> OutputPlaces for ($(PlaceRef<$t>,)*) {}

        impl<$($t,)* $($r,)*> PositionalSealed<($(PlaceRef<$t>,)*)> for ($($r,)*)
        where
            $($t: Send + Sync + 'static,)*
            $($r: IntoTokens<$t>,)*
        {
            #[allow(unused_variables, non_snake_case)]
            fn push_erased(self, out: &mut Vec<(usize, ErasedToken)>) {
                let ($($r,)*) = self;
                $(
                    $r.for_each_token(&mut |token| {
                        out.push(($i, Box::new(token) as ErasedToken))
                    });
                )*
            }
        }

        impl<$($t,)* $($r,)*> PositionalOutputs<($(PlaceRef<$t>,)*)> for ($($r,)*)
        where
            $($t: Send + Sync + 'static,)*
            $($r: IntoTokens<$t>,)*
        {
        }
    };
}

for_each_arity!(impl_outputs);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::InputEdges;
    use crate::erased::ErasedInput;
    use crate::{MutateEdge, TakeAllEdge, TakeEdge};

    /// Wraps a body exactly as `NetBuilder::transition` does, without needing
    /// a net: the tuples are only there to pin the type parameters.
    fn erase<Edges, Outputs, M, F>(_inputs: Edges, _outputs: Outputs, body: F) -> ErasedBody
    where
        Edges: InputEdges,
        Outputs: OutputPlaces,
        F: TransitionBody<Edges, Outputs, M>,
    {
        BodySealed::into_erased(body)
    }

    fn place<T>(index: u32) -> PlaceRef<T> {
        PlaceRef::new(NetId(0), PlaceId(index))
    }

    #[test]
    fn a_body_is_called_with_one_argument_per_edge() {
        let counter = place::<u32>(0);
        let notes = place::<String>(1);
        let mut body = erase(
            (MutateEdge(counter), TakeAllEdge(counter), TakeEdge(notes)),
            (counter, notes),
            |live: &mut u32, batch: Vec<u32>, note: String| {
                *live += batch.iter().sum::<u32>();
                (*live, format!("{note}!"))
            },
        );

        let mut token: ErasedToken = Box::new(10_u32);
        let outcome = body(ErasedInputs::new(vec![
            ErasedInput::Borrowed(&mut token),
            ErasedInput::Tokens(vec![Box::new(1_u32), Box::new(2_u32)]),
            ErasedInput::Token(Box::new(String::from("hi"))),
        ]));

        match outcome {
            ErasedOutcome::Ok(tokens) => {
                assert_eq!(tokens.len(), 2);
                assert_eq!(tokens[0].0, 0);
                assert_eq!(*tokens[0].1.downcast_ref::<u32>().unwrap(), 13);
                assert_eq!(tokens[1].0, 1);
                assert_eq!(tokens[1].1.downcast_ref::<String>().unwrap(), "hi!");
            }
            ErasedOutcome::Err(error) => panic!("unexpected failure: {error}"),
        }
        // The mutating edge wrote through to the token in its place.
        assert_eq!(*token.downcast_ref::<u32>().unwrap(), 13);
    }

    #[test]
    fn slots_may_be_one_maybe_one_or_many() {
        let numbers = place::<u32>(0);
        let mut body = erase((), (numbers, numbers, numbers), || {
            (1_u32, None::<u32>, vec![2_u32, 3])
        });
        match body(ErasedInputs::new(Vec::new())) {
            ErasedOutcome::Ok(tokens) => {
                let indexes: Vec<usize> = tokens.iter().map(|(index, _)| *index).collect();
                assert_eq!(indexes, [0, 2, 2], "the empty Option produced no token");
            }
            ErasedOutcome::Err(error) => panic!("unexpected failure: {error}"),
        }
    }

    #[test]
    fn an_err_return_is_a_failure_not_an_output() {
        #[derive(Debug)]
        struct Sad;
        impl std::fmt::Display for Sad {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("sad")
            }
        }
        impl Error for Sad {}

        let numbers = place::<u32>(0);
        let mut body = erase((), (numbers,), || -> Result<(u32,), Sad> { Err(Sad) });
        match body(ErasedInputs::new(Vec::new())) {
            ErasedOutcome::Ok(tokens) => panic!("expected a failure, got {tokens:?} tokens"),
            ErasedOutcome::Err(error) => assert_eq!(error.to_string(), "sad"),
        }
    }

    #[test]
    fn a_body_may_keep_state_between_firings() {
        let numbers = place::<u32>(0);
        let mut count = 0_u32;
        let mut body = erase((), (numbers,), move || {
            count += 1;
            (count,)
        });
        for expected in 1..=3_u32 {
            match body(ErasedInputs::new(Vec::new())) {
                ErasedOutcome::Ok(tokens) => {
                    assert_eq!(*tokens[0].1.downcast_ref::<u32>().unwrap(), expected);
                }
                ErasedOutcome::Err(error) => panic!("unexpected failure: {error}"),
            }
        }
    }
}
