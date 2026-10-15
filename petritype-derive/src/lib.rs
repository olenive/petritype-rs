//! The derives of [petritype]: `#[derive(Routed)]`, which lets a transition
//! body return a value and have it land in the output place of its type, and
//! `#[derive(Inputs)]` and `#[derive(Outputs)]`, which bind a body's inputs
//! and outputs to a net by field name.
//!
//! The macros are re-exported by the `petritype` crate, which is where they
//! are documented and where the examples live; there is no reason to depend
//! on this crate directly.
//!
//! [petritype]: https://docs.rs/petritype

#![warn(missing_docs, missing_debug_implementations, unreachable_pub)]
#![forbid(unsafe_code)]

mod binding;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{
    Data, DataEnum, DeriveInput, Error, Fields, GenericParam, Result, parse_macro_input,
    parse_quote,
};

/// What a variant that is not `Variant(T)` or `Variant` is told.
const ONE_PAYLOAD: &str = "a routed variant carries exactly one payload, `Variant(T)`";

/// The same, for a variant whose fields are named: there is no order to route
/// by, and a payload type of its own is what the author wants.
const NAMED_PAYLOAD: &str = "a routed variant carries exactly one payload, `Variant(T)`, not named fields: \
     give the fields a struct of their own and route that";

/// Unions have no variants to route by and no fields that can be moved out
/// safely.
const UNION: &str = "`Routed` cannot be derived for a union: derive it on an enum, whose variants \
                     carry the payloads, or on a struct, which is routed whole";

/// A token is an owned value that may outlive the firing that made it.
const LIFETIME: &str = "`Routed` cannot be derived for a type with a lifetime: a token is an owned \
                        value, so every routed payload must be `'static`";

/// Routes a returned value to the output place of its type.
///
/// On an **enum**, each single-field tuple variant `V(P)` emits its payload to
/// the output place that holds `P`, and a unit variant emits nothing:
///
/// ```ignore
/// #[derive(Routed)]
/// enum Outcome {
///     Ok(SuccessfulScrape),        // to the place of SuccessfulScrape
///     Unsure(UncertainScrape),     // to the place of UncertainScrape
///     Nothing,                     // no token at all
/// }
/// ```
///
/// On a **struct**, the whole value is the payload: it goes to the output
/// place of the struct's own type. That is how a bare value is routed, since
/// routing is by type and a std type such as `u32` cannot carry one.
///
/// A payload is a leaf: `V(Vec<Job>)` sends one `Vec<Job>` token to a
/// `Vec<Job>` place, it does not spread the jobs over a `Job` place.
///
/// The expansion is an implementation of `petritype::RoutedOutput`; the
/// crate's documentation of that trait says what the resulting net must
/// satisfy, and `NetBuilder::build` reports it when it does not.
#[proc_macro_derive(Routed)]
pub fn derive_routed(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    routed(input)
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

/// The derive proper, with errors as values so that every one of them is
/// reported through [`Error::into_compile_error`] and spanned at the piece of
/// the declaration that has to change.
fn routed(input: DeriveInput) -> Result<TokenStream2> {
    if let Some(lifetime) = input.generics.lifetimes().next() {
        return Err(Error::new_spanned(lifetime, LIFETIME));
    }

    let name = &input.ident;
    let (payloads, arms) = match &input.data {
        Data::Enum(data) => variants(data)?,
        // A struct is a leaf: it is its own payload, whatever its fields are.
        Data::Struct(_) => (
            vec![quote!(spec.payload::<Self>();)],
            vec![quote!(value => sink.push(value),)],
        ),
        Data::Union(data) => return Err(Error::new_spanned(data.union_token, UNION)),
    };

    // A payload type is stored as a token, so a generic one has to be able to
    // be: the bounds of the trait itself, pushed onto each type parameter.
    // Nothing is added for lifetimes (rejected above) or const parameters.
    let mut generics = input.generics.clone();
    for param in &mut generics.params {
        if let GenericParam::Type(parameter) = param {
            parameter.bounds.push(parse_quote!(::core::marker::Send));
            parameter.bounds.push(parse_quote!(::core::marker::Sync));
            parameter.bounds.push(parse_quote!('static));
        }
    }
    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();

    Ok(quote! {
        #[automatically_derived]
        impl #impl_generics ::petritype::routing::RoutedOutput for #name #type_generics
        #where_clause
        {
            fn describe(spec: &mut ::petritype::routing::RouteSpec) {
                #(#payloads)*
            }

            fn emit(self, sink: &mut ::petritype::routing::RouteSink<'_>) {
                match self {
                    #(#arms)*
                }
            }
        }
    })
}

/// One payload declaration and one match arm per variant, in declaration
/// order.
///
/// An enum with no variants yields no arms, which is what `match self {}` on
/// an uninhabited value wants.
fn variants(data: &DataEnum) -> Result<(Vec<TokenStream2>, Vec<TokenStream2>)> {
    let mut payloads = Vec::new();
    let mut arms = Vec::new();

    for variant in &data.variants {
        let name = &variant.ident;
        match &variant.fields {
            // Nothing to route, and nothing to check: the variant is a fact
            // about the firing, not a value the net carries.
            Fields::Unit => arms.push(quote!(Self::#name => {})),
            Fields::Unnamed(fields) if fields.unnamed.is_empty() => {
                arms.push(quote!(Self::#name() => {}));
            }
            Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                let payload = &fields.unnamed[0].ty;
                payloads.push(quote!(spec.payload::<#payload>();));
                arms.push(quote!(Self::#name(payload) => sink.push(payload),));
            }
            Fields::Unnamed(fields) => return Err(Error::new_spanned(fields, ONE_PAYLOAD)),
            Fields::Named(fields) => return Err(Error::new_spanned(fields, NAMED_PAYLOAD)),
        }
    }

    Ok((payloads, arms))
}

/// Binds a body's inputs to a net by field name.
///
/// On a struct with one named field per input, it generates a second
/// struct, the *wiring*, named after the first with `Edges` appended: the
/// same field names, one type parameter per field, each bounded by
/// `petritype::Reads<field type>`. A net fills each field with any input
/// that reads that type, and passes the wiring where an input goes:
///
/// ```ignore
/// #[derive(Inputs)]
/// pub struct Pricing {
///     pub order: Order,
///     pub promos: Vec<Promo>,
///     pub prices: Prices,
/// }
///
/// b.transition(
///     "Price Order",
///     (PricingEdges { order: TakeEdge(orders), promos: SurveyEdge(promos), prices: Param::new() },),
///     (priced,),
///     |p: Pricing| (p.order,),
/// );
/// ```
///
/// The wiring is one input position: the body takes a `Pricing` there, and
/// the transition's edges are its fields' edges in field order, which is
/// what the read API and the wire report. Like any input it goes in the
/// input tuple, a lone one with the trailing comma. It is as visible as the
/// struct, fields included, and `Clone`, `Copy` and `Debug` where its
/// inputs are.
///
/// The struct may have one lifetime, for fields of type `&'a mut T` that a
/// `MutateEdge` or a `MutateWhereEdge` fills; the body then takes
/// `Pricing<'_>`. It may not have type or const parameters (a field's type
/// is what the net wires against), and needs at least one named field.
///
/// `#[binding(edges = "Name")]` names the wiring something else, and
/// `#[binding(inputs = "Name")]` does so for this derive alone, for a struct
/// that also derives another binding.
///
/// The expansion implements the engine traits of `petritype::binding` for
/// the wiring; the crate's documentation of `Reads` says what may fill a
/// field.
#[proc_macro_derive(Inputs, attributes(binding))]
pub fn derive_inputs(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    binding::inputs(&input)
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

/// Binds a body's outputs to a net by field name.
///
/// On a struct a body returns, with one named field per output, it
/// generates the struct of outputs, named after it with `Edges` appended:
/// the same field names, one type parameter per field, each bounded by
/// `petritype::Accepts<field type>`. A net fills each field with a place
/// that accepts that type — a field `T` one token, `Option<T>` zero or one,
/// `Vec<T>` any number, into a place of `T` — and passes the struct of
/// outputs, whole, as the transition's outputs:
///
/// ```ignore
/// #[derive(Outputs)]
/// pub struct Priced {
///     pub order: Order,
///     pub note: Option<Note>,
///     pub restock: Vec<Item>,
/// }
///
/// b.transition("Price Order", (TakeEdge(orders),), PricedEdges { order: priced, note: log, restock }, price);
/// ```
///
/// Returning the struct, or a `Result` of it, is then the *named* output
/// form, beside the positional and the routed ones: each field's tokens go
/// to the place wired to its name. The transition's output edges are the
/// fields', in field order, and the form reads back as positional, since
/// that order is real. A struct that also derives `Routed` is routed by
/// type where the outputs are a tuple, and bound by name where they are its
/// own struct of outputs.
///
/// The struct may not have lifetimes (a token is an owned value), type or
/// const parameters, and needs at least one named field.
/// `#[binding(edges = "Name")]` names the struct of outputs something else,
/// and `#[binding(outputs = "Name")]` does so for this derive alone, for a
/// struct that also derives `Inputs`.
#[proc_macro_derive(Outputs, attributes(binding))]
pub fn derive_outputs(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    binding::outputs(&input)
        .unwrap_or_else(Error::into_compile_error)
        .into()
}
