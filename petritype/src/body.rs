//! Bodies and outputs: the traits that let one `transition()` call accept a
//! closure of any shape, and check it against the declared edges and places.
//!
//! Three questions have to be answered at compile time:
//!
//! 1. Do the body's parameters match the input edges? — [`TransitionBody`],
//!    implemented for every arity from 0 to 12.
//! 2. Does what the body returns match the output places? —
//!    [`PositionalOutputs`], slot by slot, with [`IntoTokens`] deciding how
//!    many tokens one slot is worth; [`RoutedOutput`](crate::RoutedOutput),
//!    payload type by payload type; or nothing at all for a
//!    [`Distribution`](crate::Distribution), which names its places itself
//!    and is checked when its tokens land.
//! 3. Is the return value an outcome at all? — [`IntoOutcome`], which also
//!    recognises `Result` as failure rather than as data.
//!
//! What answers question 2 may be the return type of the body or of an
//! [output distribution function](crate::OutputDistribution) composed after
//! it; the pair is a body too, and the impls for it are written by the same
//! arity macro.
//!
//! The answers are wrapped, once, into an
//! [`ErasedBody`](crate::erased::ErasedBody) at declaration time, so firing
//! costs no type juggling at all.

use std::any::{TypeId, type_name};
use std::error::Error;

use crate::arity::for_each_arity;
use crate::distribution::WithOutputDistribution;
use crate::edge::InputEdge;
use crate::erased::{
    ErasedBody, ErasedInputs, ErasedOutcome, OutputTarget, RouteTable, RoutingProblem,
};
use crate::ids::{NetId, PlaceId, PlaceRef};
use crate::place::ErasedToken;
use crate::routing::{RouteSink, RouteSpec, RoutedOutput};
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

    /// The body returned one value to be routed by type: each payload goes
    /// to the output place that holds its type, whatever order the outputs
    /// were declared in. See [`RoutedOutput`](crate::RoutedOutput).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct Routed;

    /// The body returned a [`Distribution`](crate::Distribution): the places
    /// were named outright, so neither order nor type picks them.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct Distributed;

    /// The body returned `Result<O, E>`, where `O` is itself an outcome in
    /// the form `M`. Failure wraps any other form rather than replacing it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct Failing<M>(PhantomData<fn() -> M>);

    /// An [output distribution function](crate::OutputDistribution) composed
    /// after a body that returns its result directly.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct Plain;

    /// An [output distribution function](crate::OutputDistribution) composed
    /// after a body that returns `Result<R, E>`: the function takes the `R`,
    /// and an `Err` fails the firing without calling it.
    ///
    /// The tag is what keeps the two wrapper impls apart — they differ only
    /// in the shape of the body's return type, which no where-clause of the
    /// other could rule in.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct Fallible;
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
    note = "a value that is not a tuple is routed by type instead: derive `Routed` on the enum \
            or struct you return, so that each payload goes to the output place that holds it"
)]
pub trait PositionalOutputs<Outputs>: PositionalSealed<Outputs> {}

/// What a transition body may return.
///
/// One of the output forms, or `Result<Outputs, E>` of one. A `Result` always
/// means failure, never routing (DESIGN §7): a body that wants to send a
/// value to one place or another returns data that says so, not an `Err`.
///
/// The three forms are told apart by the return type:
///
/// - **positional** — a tuple, one slot per output place, matched by order
///   ([`PositionalOutputs`]);
/// - **routed** — a value that is not a tuple, matched by type
///   ([`RoutedOutput`](crate::RoutedOutput)), which is what
///   [`#[derive(Routed)]`](crate::Routed) implements;
/// - **distribution** — a [`Distribution`](crate::Distribution), whose pairs
///   name their places outright, for the decisions only the data can make.
///
/// # How the output forms plug in
///
/// The `Marker` parameter is what keeps them from colliding. Positional
/// outputs are `impl<O: PositionalOutputs<Outputs>> IntoOutcome<Outputs,
/// marker::Positional> for O`, routed ones the same shape under
/// [`marker::Routed`], and the distribution form is written for the concrete
/// [`Distribution`](crate::Distribution) under [`marker::Distributed`].
/// Because the tags differ, none of the impls overlaps another, and the
/// compiler picks the form by picking the impl whose bounds hold. `Result`
/// needs no new impl for any of them: [`marker::Failing<M>`] carries the
/// inner form's tag.
///
/// A body wrapped with
/// [`output_distribution`](crate::OutputDistribution::output_distribution)
/// adds nothing here either: the wrapper is a *body*, and the form is read
/// from what the distribution function returns, which is again one of these
/// three.
#[diagnostic::on_unimplemented(
    message = "a transition body must return its outputs, or a `Result` of them",
    label = "`{Self}` is not a transition outcome",
    note = "positional: outputs `{Outputs}` want a tuple of that many slots, where slot i is \
            `T`, `Option<T>` or `Vec<T>` for the type of output place i, and `()` for no \
            outputs at all",
    note = "routed: a value that is not a tuple, whose every payload goes to the output place \
            that holds its type — derive `Routed` on the enum or struct you return",
    note = "distribution: a `Distribution` of (place, token) pairs, built with \
            `Distribution::new().to(place, token)`, for choosing between output places of the \
            same type",
    note = "any of the three may be wrapped in `Result<_, E>` where \
            `E: std::error::Error + Send + Sync + 'static`; a `Result` always means failure, \
            never routing"
)]
pub trait IntoOutcome<Outputs, Marker>: OutcomeSealed<Outputs, Marker> {}

impl<Outputs, O: PositionalOutputs<Outputs>> OutcomeSealed<Outputs, marker::Positional> for O {
    /// Nothing to resolve: a slot's index *is* its output place's index.
    fn routing() -> Result<RouteTable, RoutingProblem> {
        Ok(RouteTable::empty())
    }

    fn positional() -> bool {
        true
    }

    fn into_outcome(self, _routes: &RouteTable) -> ErasedOutcome {
        let mut tokens = Vec::new();
        PositionalSealed::push_erased(self, &mut tokens);
        ErasedOutcome::Ok(tokens)
    }
}

impl<Outputs, O: PositionalOutputs<Outputs>> IntoOutcome<Outputs, marker::Positional> for O {}

/// The routed form: the payload types of what the body returns are matched
/// against the types of the output places, and each emitted payload is tagged
/// with the index that match gave it.
///
/// No overlap with the positional impl above, although both are written for a
/// bare `O`: the marker differs, and nothing implements both
/// [`PositionalOutputs`] (tuples only) and
/// [`RoutedOutput`](crate::RoutedOutput) (never a tuple).
impl<Outputs, O> OutcomeSealed<Outputs, marker::Routed> for O
where
    Outputs: OutputPlaces,
    O: RoutedOutput,
{
    fn routing() -> Result<RouteTable, RoutingProblem> {
        let mut spec = RouteSpec::new();
        O::describe(&mut spec);
        let mut outputs = Vec::new();
        <Outputs as PlaceTupleSealed>::describe_types(&mut outputs);
        RouteTable::resolve(spec.payloads(), &outputs)
    }

    fn positional() -> bool {
        false
    }

    fn into_outcome(self, routes: &RouteTable) -> ErasedOutcome {
        let mut tokens = Vec::new();
        self.emit(&mut RouteSink::new(routes, &mut tokens));
        ErasedOutcome::Ok(tokens)
    }
}

impl<Outputs, O> IntoOutcome<Outputs, marker::Routed> for O
where
    Outputs: OutputPlaces,
    O: RoutedOutput,
{
}

impl<Outputs, O, E, M> OutcomeSealed<Outputs, marker::Failing<M>> for Result<O, E>
where
    O: IntoOutcome<Outputs, M>,
    E: Error + Send + Sync + 'static,
{
    /// The inner form's resolution: wrapping outputs in a `Result` says
    /// nothing about where they go.
    fn routing() -> Result<RouteTable, RoutingProblem> {
        <O as OutcomeSealed<Outputs, M>>::routing()
    }

    /// The inner form's, likewise.
    fn positional() -> bool {
        <O as OutcomeSealed<Outputs, M>>::positional()
    }

    fn into_outcome(self, routes: &RouteTable) -> ErasedOutcome {
        match self {
            Ok(outputs) => OutcomeSealed::into_outcome(outputs, routes),
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
/// It is also implemented for such a closure with an
/// [output distribution function](crate::OutputDistribution) composed after
/// it — a [`WithOutputDistribution`], which is a body of the same arity whose
/// outputs are read from what the *function* returns. The function takes the
/// body's result, or, when the body returns `Result<R, E>`, the `R` inside an
/// `Ok`.
///
/// `Marker` is inferred; see [`marker`].
#[diagnostic::on_unimplemented(
    message = "transition body parameters do not match its input edges",
    label = "this body cannot be called with what `{Edges}` produces",
    note = "one parameter per edge, in order: `TakeEdge(p)` gives `T`, `TakeAllEdge(p)` gives \
            `Vec<T>`, `SnapshotEdge(p)` gives `T`, `MutateEdge(p)` gives `&mut T`",
    note = "with `.output_distribution(f)`, the output distribution function `f` takes exactly \
            what the body returns — the value inside the `Ok` if that is a `Result` — and \
            returns the outputs in any form, usually a `Distribution`",
    note = "a body must be `Send + Sync + 'static`: capture an `Arc`, a `Mutex` or an atomic \
            rather than an `Rc` or a `RefCell`"
)]
pub trait TransitionBody<Edges, Outputs, Marker>: BodySealed<Edges, Outputs, Marker> {}

/// The heart of the slice: one impl per input arity, and now three of them —
/// a bare closure, and a closure with an
/// [output distribution function](crate::OutputDistribution) composed after
/// it, whose body returns its result either directly or in a `Result`.
///
/// The marker is `(M, R)` for a bare body — the outcome form and the body's
/// return type — because an impl may not leave a type parameter to be pinned
/// by a where clause alone, and `R` appears only in the `FnMut` bound. Putting
/// it in the marker constrains it and costs nothing: the compiler infers the
/// whole marker at the call site.
///
/// The wrapped forms need the same trick twice over, so their marker is
/// `(M, R, R2, tag)`: `R` is what the body returns, `R2` what the
/// distribution function returns and therefore what the output form is read
/// from, and the tag is [`marker::Plain`] or [`marker::Fallible`]. The two
/// tags exist because the impls differ only in the *shape* of `R` — `R`
/// against `Result<R, E>` — which coherence cannot see, since a `Result` is a
/// perfectly good `R`. With different tags the impls cannot overlap, and at a
/// call site the compiler discards whichever one the distribution function's
/// own parameter type contradicts: a function taking `Ball` rules out the
/// fallible reading of a body returning `Ball`, and a function taking `Ball`
/// after a body returning `Result<Ball, E>` rules out the plain one. That is
/// also why the crate asks for annotated closure parameters — an unannotated
/// `|ball|` leaves nothing to choose by.
macro_rules! impl_body {
    ($(($t:ident, $r:ident, $i:tt)),*) => {
        impl<F, R, M, Outputs, $($t,)*> BodySealed<($($t,)*), Outputs, (M, R)> for F
        where
            $($t: InputEdge,)*
            F: for<'a> FnMut($(<$t as EdgeSealed>::Arg<'a>,)*) -> R + Send + Sync + 'static,
            R: IntoOutcome<Outputs, M>,
        {
            #[allow(unused_mut, unused_variables)]
            fn into_erased(mut self) -> (ErasedBody, Option<RoutingProblem>) {
                // Resolved here, the one place where the return type and the
                // output places are both known, and then captured: firing
                // looks an index up and no more. An unresolvable table is
                // replaced by an empty one and reported, because
                // `transition()` cannot fail and the net will not build.
                let (routes, problem) = match <R as OutcomeSealed<Outputs, M>>::routing() {
                    Ok(routes) => (routes, None),
                    Err(problem) => (RouteTable::empty(), Some(problem)),
                };
                let body: ErasedBody = Box::new(move |mut inputs: ErasedInputs<'_>| {
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
                    OutcomeSealed::into_outcome(returned, &routes)
                });
                (body, problem)
            }

            fn positional() -> bool {
                <R as OutcomeSealed<Outputs, M>>::positional()
            }
        }

        impl<F, R, M, Outputs, $($t,)*> TransitionBody<($($t,)*), Outputs, (M, R)> for F
        where
            $($t: InputEdge,)*
            F: for<'a> FnMut($(<$t as EdgeSealed>::Arg<'a>,)*) -> R + Send + Sync + 'static,
            R: IntoOutcome<Outputs, M>,
        {
        }

        // ----- the same body, with an output distribution function after it -

        impl<F, G, R, R2, M, Outputs, $($t,)*>
            BodySealed<($($t,)*), Outputs, (M, R, R2, marker::Plain)>
            for WithOutputDistribution<F, G>
        where
            $($t: InputEdge,)*
            F: for<'a> FnMut($(<$t as EdgeSealed>::Arg<'a>,)*) -> R + Send + Sync + 'static,
            G: FnMut(R) -> R2 + Send + Sync + 'static,
            R2: IntoOutcome<Outputs, M>,
        {
            #[allow(unused_mut, unused_variables)]
            fn into_erased(mut self) -> (ErasedBody, Option<RoutingProblem>) {
                // `R2` and not `R`: the distribution function's return type is
                // what decides the output form, so the body's is merely its
                // input (DESIGN §4.5).
                let (routes, problem) = match <R2 as OutcomeSealed<Outputs, M>>::routing() {
                    Ok(routes) => (routes, None),
                    Err(problem) => (RouteTable::empty(), Some(problem)),
                };
                let body: ErasedBody = Box::new(move |mut inputs: ErasedInputs<'_>| {
                    let mut args = inputs.iter_mut();
                    let returned = (self.body)($(
                        <$t as EdgeSealed>::extract(
                            args.next().expect("one erased input per input edge"),
                        ),
                    )*);
                    // Both halves are stage two, so the routing decision runs
                    // wherever the body ran — off-thread in concurrent mode.
                    OutcomeSealed::into_outcome((self.distribute)(returned), &routes)
                });
                (body, problem)
            }

            fn positional() -> bool {
                <R2 as OutcomeSealed<Outputs, M>>::positional()
            }
        }

        #[diagnostic::do_not_recommend]
        impl<F, G, R, R2, M, Outputs, $($t,)*>
            TransitionBody<($($t,)*), Outputs, (M, R, R2, marker::Plain)>
            for WithOutputDistribution<F, G>
        where
            $($t: InputEdge,)*
            F: for<'a> FnMut($(<$t as EdgeSealed>::Arg<'a>,)*) -> R + Send + Sync + 'static,
            G: FnMut(R) -> R2 + Send + Sync + 'static,
            R2: IntoOutcome<Outputs, M>,
        {
        }

        impl<F, G, R, E, R2, M, Outputs, $($t,)*>
            BodySealed<($($t,)*), Outputs, (M, Result<R, E>, R2, marker::Fallible)>
            for WithOutputDistribution<F, G>
        where
            $($t: InputEdge,)*
            F: for<'a> FnMut($(<$t as EdgeSealed>::Arg<'a>,)*) -> Result<R, E>
                + Send + Sync + 'static,
            G: FnMut(R) -> R2 + Send + Sync + 'static,
            E: Error + Send + Sync + 'static,
            R2: IntoOutcome<Outputs, M>,
        {
            #[allow(unused_mut, unused_variables)]
            fn into_erased(mut self) -> (ErasedBody, Option<RoutingProblem>) {
                let (routes, problem) = match <R2 as OutcomeSealed<Outputs, M>>::routing() {
                    Ok(routes) => (routes, None),
                    Err(problem) => (RouteTable::empty(), Some(problem)),
                };
                let body: ErasedBody = Box::new(move |mut inputs: ErasedInputs<'_>| {
                    let mut args = inputs.iter_mut();
                    let returned = (self.body)($(
                        <$t as EdgeSealed>::extract(
                            args.next().expect("one erased input per input edge"),
                        ),
                    )*);
                    match returned {
                        Ok(value) => {
                            OutcomeSealed::into_outcome((self.distribute)(value), &routes)
                        }
                        // A failure is a failure whatever would have routed
                        // it: the distribution function is not called at all,
                        // because there is no result to distribute.
                        Err(error) => ErasedOutcome::Err(Box::new(error)),
                    }
                });
                (body, problem)
            }

            fn positional() -> bool {
                <R2 as OutcomeSealed<Outputs, M>>::positional()
            }
        }

        #[diagnostic::do_not_recommend]
        impl<F, G, R, E, R2, M, Outputs, $($t,)*>
            TransitionBody<($($t,)*), Outputs, (M, Result<R, E>, R2, marker::Fallible)>
            for WithOutputDistribution<F, G>
        where
            $($t: InputEdge,)*
            F: for<'a> FnMut($(<$t as EdgeSealed>::Arg<'a>,)*) -> Result<R, E>
                + Send + Sync + 'static,
            G: FnMut(R) -> R2 + Send + Sync + 'static,
            E: Error + Send + Sync + 'static,
            R2: IntoOutcome<Outputs, M>,
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

            #[allow(unused_variables)]
            fn describe_types(out: &mut Vec<(TypeId, &'static str)>) {
                $(out.push((TypeId::of::<$t>(), type_name::<$t>()));)*
            }
        }

        impl<$($t: Send + Sync + 'static,)*> OutputPlaces for ($(PlaceRef<$t>,)*) {}

        impl<$($t,)* $($r,)*> PositionalSealed<($(PlaceRef<$t>,)*)> for ($($r,)*)
        where
            $($t: Send + Sync + 'static,)*
            $($r: IntoTokens<$t>,)*
        {
            #[allow(unused_variables, non_snake_case)]
            fn push_erased(self, out: &mut Vec<(OutputTarget, ErasedToken)>) {
                let ($($r,)*) = self;
                $(
                    $r.for_each_token(&mut |token| {
                        out.push((OutputTarget::Index($i), Box::new(token) as ErasedToken))
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
    /// a net: the tuples are only there to pin the type parameters. Panics
    /// on a routing problem, which the tests that expect one ask for with
    /// [`routing_of`] instead.
    fn erase<Edges, Outputs, M, F>(_inputs: Edges, _outputs: Outputs, body: F) -> ErasedBody
    where
        Edges: InputEdges,
        Outputs: OutputPlaces,
        F: TransitionBody<Edges, Outputs, M>,
    {
        let (body, problem) = BodySealed::into_erased(body);
        assert_eq!(problem, None, "this body's outputs resolve");
        body
    }

    /// What resolving this body's return type against those output places
    /// found, without a net and without firing.
    fn routing_of<Edges, Outputs, M, F>(
        _inputs: Edges,
        _outputs: Outputs,
        body: F,
    ) -> Option<RoutingProblem>
    where
        Edges: InputEdges,
        Outputs: OutputPlaces,
        F: TransitionBody<Edges, Outputs, M>,
    {
        BodySealed::into_erased(body).1
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
                assert_eq!(tokens[0].0, OutputTarget::Index(0));
                assert_eq!(*tokens[0].1.downcast_ref::<u32>().unwrap(), 13);
                assert_eq!(tokens[1].0, OutputTarget::Index(1));
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
                let targets: Vec<OutputTarget> = tokens.iter().map(|(target, _)| *target).collect();
                assert_eq!(
                    targets,
                    [
                        OutputTarget::Index(0),
                        OutputTarget::Index(2),
                        OutputTarget::Index(2)
                    ],
                    "the empty Option produced no token"
                );
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

    /// Two payload types, hand-written exactly as `#[derive(Routed)]` writes
    /// it for an enum: the unit variant emits nothing.
    #[derive(Debug)]
    enum Outcome {
        Number(u32),
        Note(String),
        Nothing,
    }

    impl RoutedOutput for Outcome {
        fn describe(spec: &mut RouteSpec) {
            spec.payload::<u32>();
            spec.payload::<String>();
        }

        fn emit(self, sink: &mut RouteSink<'_>) {
            match self {
                Outcome::Number(value) => sink.push(value),
                Outcome::Note(value) => sink.push(value),
                Outcome::Nothing => {}
            }
        }
    }

    #[test]
    fn a_routed_body_sends_each_payload_to_the_place_of_its_type() {
        let numbers = place::<u32>(0);
        let notes = place::<String>(1);
        // Declared notes first: a routed body does not care about the order.
        let mut body = erase((TakeEdge(numbers),), (notes, numbers), |n: u32| {
            if n == 0 {
                Outcome::Number(7)
            } else {
                Outcome::Note(String::from("hi"))
            }
        });

        let landed = |outcome: ErasedOutcome| match outcome {
            ErasedOutcome::Ok(tokens) => tokens,
            ErasedOutcome::Err(error) => panic!("unexpected failure: {error}"),
        };

        let tokens = landed(body(ErasedInputs::new(vec![ErasedInput::Token(Box::new(
            0_u32,
        ))])));
        assert_eq!(tokens.len(), 1);
        assert_eq!(
            tokens[0].0,
            OutputTarget::Index(1),
            "the u32 place, whatever its position"
        );
        assert_eq!(*tokens[0].1.downcast_ref::<u32>().unwrap(), 7);

        let tokens = landed(body(ErasedInputs::new(vec![ErasedInput::Token(Box::new(
            1_u32,
        ))])));
        assert_eq!(tokens[0].0, OutputTarget::Index(0), "the String place");
        assert_eq!(tokens[0].1.downcast_ref::<String>().unwrap(), "hi");
    }

    #[test]
    fn a_unit_variant_produces_nothing() {
        let numbers = place::<u32>(0);
        let notes = place::<String>(1);
        let mut body = erase((), (numbers, notes), || Outcome::Nothing);
        match body(ErasedInputs::new(Vec::new())) {
            ErasedOutcome::Ok(tokens) => assert!(tokens.is_empty()),
            ErasedOutcome::Err(error) => panic!("unexpected failure: {error}"),
        }
    }

    #[test]
    fn routing_is_resolved_when_the_body_is_wrapped() {
        let numbers = place::<u32>(0);
        // No place for the `String` payload, and the problem is found
        // without firing anything.
        assert!(matches!(
            routing_of((), (numbers,), || Outcome::Number(1)),
            Some(RoutingProblem::Unroutable { .. })
        ));
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
