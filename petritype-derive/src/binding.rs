//! `#[derive(Inputs)]` and `#[derive(Outputs)]`: a body's inputs and
//! outputs bound to a net by field name (DESIGN §4.4, §4.5).
//!
//! Each derive reads a struct with one named field per input or output and
//! writes a second struct, the *wiring*, with the same field names and one
//! type parameter per field, plus the engine impls that make the wiring one
//! input position of a transition, or the whole of its outputs. Everything
//! they name in `petritype` is either public (`Reads`, `Accepts`,
//! `InputEdge`, `OutputPlaces`) or in the hidden `petritype::binding`
//! module, as `Routed`'s expansion names `petritype::routing`.

use std::collections::HashSet;

use proc_macro2::{Ident, Span, TokenStream as TokenStream2, TokenTree};
use quote::{format_ident, quote, quote_spanned};
use syn::{Data, DeriveInput, Error, Fields, GenericParam, Lifetime, LitStr, Result, Type};

/// Which of the two derives is reading a struct: its name, its key in
/// `#[binding(..)]`, and what it says when it refuses a declaration.
struct Side {
    /// The key that names this derive's wiring alone.
    key: &'static str,
    /// How many lifetimes the struct may have.
    lifetimes: usize,
    /// A union has no fields a body could take or return by name.
    union: &'static str,
    /// An enum is a choice, and a binding fills every field.
    enumeration: &'static str,
    /// Binding by name needs names.
    tuple: &'static str,
    /// No fields is the empty tuple, which needs no derive.
    empty: &'static str,
    /// The wiring says which type each field is, so the types must be known.
    generic: &'static str,
    /// The lifetime past what `lifetimes` allows.
    lifetime: &'static str,
    /// The wiring carries bounds of its own.
    where_clause: &'static str,
}

/// `#[derive(Inputs)]`.
const INPUTS: Side = Side {
    key: "inputs",
    lifetimes: 1,
    union: "`Inputs` cannot be derived for a union: derive it on a struct with one named field \
            per input",
    enumeration: "`Inputs` cannot be derived for an enum: a transition reads every input at \
                  once, so derive it on a struct with one named field per input",
    tuple: "`Inputs` binds fields by name, so the struct needs named fields: \
            `struct Pricing { order: Order, .. }`; to bind by position, pass a tuple of edges \
            instead",
    empty: "`Inputs` needs at least one field: a transition with no inputs takes `()`",
    generic: "`Inputs` cannot be derived for a generic struct: each field's type is what a net \
              wires against, so it must be concrete; bind a generic body by position, with a \
              tuple of edges",
    lifetime: "`Inputs` allows one lifetime, the one its `&mut` fields borrow the net's tokens \
               for during a call",
    where_clause: "`Inputs` cannot be derived for a struct with a `where` clause: the struct of \
                   edges it generates carries bounds of its own",
};

/// `#[derive(Outputs)]`.
const OUTPUTS: Side = Side {
    key: "outputs",
    lifetimes: 0,
    union: "`Outputs` cannot be derived for a union: derive it on a struct with one named field \
            per output",
    enumeration: "`Outputs` cannot be derived for an enum: a struct bound by name fills every \
                  output it names; to send a value to one place or another, derive `Routed` on \
                  the enum instead",
    tuple: "`Outputs` binds fields by name, so the struct needs named fields: \
            `struct Priced { order: Order, .. }`; to bind by position, return a tuple instead",
    empty: "`Outputs` needs at least one field: a transition that produces nothing returns `()`",
    generic: "`Outputs` cannot be derived for a generic struct: each field's type is what a net \
              wires against, so it must be concrete; return a tuple from a generic body",
    lifetime: "`Outputs` cannot be derived for a type with a lifetime: a token is an owned \
               value, so every output field must be `'static`",
    where_clause: "`Outputs` cannot be derived for a struct with a `where` clause: the struct of \
                   outputs it generates carries bounds of its own",
};

/// What `#[binding(..)]` accepts.
const OPTION: &str = "expected `edges = \"Name\"`, `inputs = \"Name\"` or `outputs = \"Name\"`";

/// A struct the derive accepts: its name, visibility and fields, and the
/// names the expansion gives the wiring and its parts.
struct Shape<'a> {
    /// The struct the body takes or returns.
    name: &'a Ident,
    /// Its visibility, which the wiring and its fields copy.
    vis: &'a syn::Visibility,
    /// The wiring's name: `<name>Edges` unless an attribute says otherwise.
    edges: Ident,
    /// The struct's one lifetime, if it has one.
    lifetime: Option<&'a Lifetime>,
    /// The named fields, in declaration order.
    fields: Vec<(&'a Ident, &'a Type)>,
    /// One type parameter of the wiring per field, `F0` upwards, renamed if
    /// a field's type already uses one of those names.
    params: Vec<Ident>,
}

/// Generates the wiring for a struct of inputs; errors are spanned at the
/// part of the declaration that has to change.
pub(crate) fn inputs(input: &DeriveInput) -> Result<TokenStream2> {
    let shape = shape(input, &INPUTS)?;
    let Shape {
        name,
        vis,
        edges,
        lifetime,
        fields,
        params,
    } = &shape;

    // The lifetime the engine fills the struct at: the struct's own, so
    // that a `&'a mut T` field is written as the author wrote it, or a fresh
    // one that no field type mentions.
    let at = match lifetime {
        Some(lifetime) => (*lifetime).clone(),
        None => fresh_lifetime(fields),
    };
    let value_at = match lifetime {
        Some(_) => quote!(#name<#at>),
        None => quote!(#name),
    };

    let mut reads = Vec::new();
    let mut extracts = Vec::new();
    let mut members = Vec::new();
    let mut takes = Vec::new();
    let mut describes = Vec::new();
    for ((field, ty), param) in fields.iter().zip(params) {
        // Spanned at the field, so that "required by this bound" points at
        // the field of the struct the body takes, which names it.
        let span = field.span();
        let borrows = lifetime.is_some_and(|lifetime| mentions(ty, lifetime));
        reads.push(if borrows {
            quote_spanned!(span=> #param: for<#at> ::petritype::Reads<#ty>)
        } else {
            quote_spanned!(span=> #param: ::petritype::Reads<#ty>)
        });
        extracts.push(quote_spanned!(span=>
            #param: for<#at> ::petritype::binding::Extract<#at, #ty>
        ));
        let doc = format!("The input that fills `{field}`.");
        members.push(quote!(#[doc = #doc] #vis #field: #param));
        takes.push(quote!(
            #field: <#param as ::petritype::binding::Extract<#at, #ty>>::extract(inputs)
        ));
        describes.push(quote!(::petritype::binding::EdgeSealed::describe(&self.#field, out);));
    }

    let doc = format!(
        "How a [`{name}`] is read off a net: one input per field, each any input that reads the \
         field's type."
    );
    let doc_more = format!(
        "Generated by `#[derive(Inputs)]`. A value of it is one input position of a transition, \
         where the body takes a `{name}`; the transition's edges are its fields' edges, in field \
         order."
    );

    let items = quote! {
        #[doc = #doc]
        #[doc = ""]
        #[doc = #doc_more]
        #[derive(Clone, Copy, Debug)]
        #vis struct #edges<#(#params),*>
        where
            #(#reads,)*
        {
            #(#members,)*
        }

    };

    // The impls are spanned at the struct's name rather than at the derive:
    // a note that an impl's bound failed then shows the struct and the
    // field whose bound it was, and rustc does not suggest writing by hand
    // an impl the author never wrote.
    let impls = quote_spanned! {name.span()=>
        impl<#(#params),*> ::petritype::binding::EdgeSealed for #edges<#(#params),*>
        where
            #(#extracts,)*
        {
            type Arg<#at> = #value_at;

            fn extract<#at>(
                inputs: &mut ::petritype::binding::Cursor<#at, '_>,
            ) -> Self::Arg<#at> {
                #name {
                    #(#takes,)*
                }
            }

            fn describe(&self, out: &mut ::petritype::binding::Specs) {
                #(#describes)*
            }
        }

        impl<#(#params),*> ::petritype::InputEdge for #edges<#(#params),*>
        where
            #(#extracts,)*
        {
        }
    };
    Ok(quote!(#items #impls))
}

/// Generates the struct of outputs for a struct a body returns, and the
/// impl that makes returning it the named form.
pub(crate) fn outputs(input: &DeriveInput) -> Result<TokenStream2> {
    let shape = shape(input, &OUTPUTS)?;
    let Shape {
        name,
        vis,
        edges,
        fields,
        params,
        ..
    } = &shape;

    let mut accepts = Vec::new();
    let mut members = Vec::new();
    let mut places = Vec::new();
    let mut types = Vec::new();
    let mut emits = Vec::new();
    for ((field, ty), param) in fields.iter().zip(params) {
        // Spanned at the field, as for inputs.
        let span = field.span();
        accepts.push(quote_spanned!(span=> #param: ::petritype::Accepts<#ty>));
        let doc = format!("The output that takes `{field}`.");
        members.push(quote!(#[doc = #doc] #vis #field: #param));
        places.push(quote!(::petritype::binding::output_place::<#param, #ty>(&self.#field, out);));
        types.push(quote!(::petritype::binding::output_type::<#param, #ty>(out);));
        emits.push(quote!(out.field::<#param, #ty>(self.#field);));
    }

    let doc = format!(
        "Where a [`{name}`] goes in a net: one output per field, each any output that accepts \
         the field's type."
    );
    let doc_more = format!(
        "Generated by `#[derive(Outputs)]`. A value of it is the whole of a transition's \
         outputs, where the body returns a `{name}`; the transition's output edges are its \
         fields', in field order."
    );
    let items = quote! {
        #[doc = #doc]
        #[doc = ""]
        #[doc = #doc_more]
        #[derive(Clone, Copy, Debug)]
        #vis struct #edges<#(#params),*>
        where
            #(#accepts,)*
        {
            #(#members,)*
        }
    };

    // Spanned at the struct's name, as for inputs.
    let impls = quote_spanned! {name.span()=>
        impl<#(#params),*> ::petritype::binding::PlaceTupleSealed for #edges<#(#params),*>
        where
            #(#accepts,)*
        {
            fn describe(&self, out: &mut ::petritype::binding::Places) {
                #(#places)*
            }

            fn describe_types(out: &mut ::petritype::binding::PlaceTypes) {
                #(#types)*
            }
        }

        impl<#(#params),*> ::petritype::OutputPlaces for #edges<#(#params),*>
        where
            #(#accepts,)*
        {
        }

        impl<#(#params),*> ::petritype::binding::NamedOutputs<#edges<#(#params),*>> for #name
        where
            #(#accepts,)*
        {
            fn emit(self, out: &mut ::petritype::binding::Emit<'_>) {
                #(#emits)*
            }
        }
    };
    Ok(quote!(#items #impls))
}

/// Checks the declaration and reads off what the expansion needs.
fn shape<'a>(input: &'a DeriveInput, side: &Side) -> Result<Shape<'a>> {
    let lifetimes: Vec<&Lifetime> = input
        .generics
        .lifetimes()
        .map(|param| &param.lifetime)
        .collect();
    if let Some(extra) = lifetimes.get(side.lifetimes) {
        return Err(Error::new_spanned(extra, side.lifetime));
    }
    for param in &input.generics.params {
        match param {
            GenericParam::Type(param) => {
                return Err(Error::new_spanned(&param.ident, side.generic));
            }
            GenericParam::Const(param) => {
                return Err(Error::new_spanned(&param.ident, side.generic));
            }
            GenericParam::Lifetime(_) => {}
        }
    }
    if let Some(clause) = &input.generics.where_clause {
        return Err(Error::new_spanned(clause.where_token, side.where_clause));
    }

    let fields: Vec<(&Ident, &Type)> = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) if named.named.is_empty() => {
                return Err(Error::new_spanned(named, side.empty));
            }
            Fields::Named(named) => named
                .named
                .iter()
                .map(|field| (field.ident.as_ref().expect("a named field"), &field.ty))
                .collect(),
            Fields::Unnamed(unnamed) => return Err(Error::new_spanned(unnamed, side.tuple)),
            Fields::Unit => return Err(Error::new_spanned(&input.ident, side.empty)),
        },
        Data::Enum(data) => return Err(Error::new_spanned(data.enum_token, side.enumeration)),
        Data::Union(data) => return Err(Error::new_spanned(data.union_token, side.union)),
    };

    Ok(Shape {
        name: &input.ident,
        vis: &input.vis,
        edges: edges_name(input, side)?,
        lifetime: lifetimes.first().copied(),
        params: fresh_params(&fields),
        fields,
    })
}

/// The wiring's name: `#[binding(inputs = "..")]` (or `outputs`, for that
/// derive), else `#[binding(edges = "..")]`, else the struct's name with
/// `Edges` appended.
///
/// The other derive's key is accepted and left alone, for a struct that
/// derives both bindings and names each wiring separately.
fn edges_name(input: &DeriveInput, side: &Side) -> Result<Ident> {
    let mut general = None;
    let mut own = None;
    for attr in &input.attrs {
        if !attr.path().is_ident("binding") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            let mut other = None;
            let slot = if meta.path.is_ident("edges") {
                &mut general
            } else if meta.path.is_ident(side.key) {
                &mut own
            } else if meta.path.is_ident("inputs") || meta.path.is_ident("outputs") {
                &mut other
            } else {
                return Err(meta.error(OPTION));
            };
            let name: LitStr = meta.value()?.parse()?;
            *slot = Some(name.parse::<Ident>().map_err(|_| {
                Error::new(
                    name.span(),
                    "the name of the struct of edges must be an identifier",
                )
            })?);
            Ok(())
        })?;
    }
    Ok(own
        .or(general)
        .unwrap_or_else(|| format_ident!("{}Edges", input.ident)))
}

/// `F0`, `F1`, ...: the wiring's type parameters, one per field. If a field's
/// type mentions a name of that form, an underscore is put in front of all
/// of them until none clashes, so that a bound never reads the parameter
/// where the author meant a type.
fn fresh_params(fields: &[(&Ident, &Type)]) -> Vec<Ident> {
    let mut used = HashSet::new();
    for (_, ty) in fields {
        idents(quote!(#ty), &mut used);
    }
    let mut prefix = String::from("F");
    while (0..fields.len()).any(|index| used.contains(&format!("{prefix}{index}"))) {
        prefix.insert(0, '_');
    }
    (0..fields.len())
        .map(|index| Ident::new(&format!("{prefix}{index}"), Span::call_site()))
        .collect()
}

/// A lifetime for a struct that has none: `'a`, or the first of `'b`, `'c`,
/// ... that no field type uses, so that a `for<'a>` inside a field's type is
/// not shadowed.
fn fresh_lifetime(fields: &[(&Ident, &Type)]) -> Lifetime {
    let mut used = HashSet::new();
    for (_, ty) in fields {
        lifetimes(quote!(#ty), &mut used);
    }
    let name = ('a'..='z')
        .map(String::from)
        .find(|name| !used.contains(name))
        .unwrap_or_else(|| String::from("petritype"));
    Lifetime::new(&format!("'{name}"), Span::call_site())
}

/// Whether a field's type mentions the struct's lifetime: such a field is
/// bounded `for<'a>`, since the wiring itself has no lifetime.
fn mentions(ty: &Type, lifetime: &Lifetime) -> bool {
    let mut used = HashSet::new();
    lifetimes(quote!(#ty), &mut used);
    used.contains(&lifetime.ident.to_string())
}

/// Every identifier in some tokens, groups included.
fn idents(tokens: TokenStream2, out: &mut HashSet<String>) {
    for token in tokens {
        match token {
            TokenTree::Ident(ident) => {
                out.insert(ident.to_string());
            }
            TokenTree::Group(group) => idents(group.stream(), out),
            TokenTree::Punct(_) | TokenTree::Literal(_) => {}
        }
    }
}

/// The name of every lifetime in some tokens, without its quote: a lifetime
/// is a `'` joined to the identifier after it.
fn lifetimes(tokens: TokenStream2, out: &mut HashSet<String>) {
    let mut quoted = false;
    for token in tokens {
        match token {
            TokenTree::Punct(punct) => quoted = punct.as_char() == '\'',
            TokenTree::Ident(ident) => {
                if quoted {
                    out.insert(ident.to_string());
                }
                quoted = false;
            }
            TokenTree::Group(group) => {
                lifetimes(group.stream(), out);
                quoted = false;
            }
            TokenTree::Literal(_) => quoted = false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    /// The message a derive refuses this declaration with.
    fn refusal(derive: fn(&DeriveInput) -> Result<TokenStream2>, input: DeriveInput) -> String {
        match derive(&input) {
            Ok(_) => panic!("expected the derive to refuse this declaration"),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn the_derives_refuse_what_they_cannot_bind_by_name() {
        for (derive, side) in [(inputs as fn(&_) -> _, &INPUTS), (outputs, &OUTPUTS)] {
            let refused = |input| refusal(derive, input);
            assert_eq!(
                refused(parse_quote!(
                    enum E {
                        A(u32),
                    }
                )),
                side.enumeration
            );
            assert_eq!(refused(parse_quote!(union U { a: u32 })), side.union);
            assert_eq!(
                refused(parse_quote!(
                    struct S(u32);
                )),
                side.tuple
            );
            assert_eq!(
                refused(parse_quote!(
                    struct S;
                )),
                side.empty
            );
            assert_eq!(
                refused(parse_quote!(
                    struct S {}
                )),
                side.empty
            );
            assert_eq!(
                refused(parse_quote!(
                    struct S<T> {
                        a: T,
                    }
                )),
                side.generic
            );
            assert_eq!(
                refused(parse_quote!(
                    struct S<const N: usize> {
                        a: [u32; N],
                    }
                )),
                side.generic
            );
            assert_eq!(
                refused(parse_quote!(
                    struct S
                    where
                        u32: Copy,
                    {
                        a: u32,
                    }
                )),
                side.where_clause
            );
            assert_eq!(
                refused(parse_quote!(
                    #[binding(wiring = "X")]
                    struct S {
                        a: u32,
                    }
                )),
                OPTION
            );
        }
        assert_eq!(
            refusal(
                inputs,
                parse_quote!(
                    struct S<'a, 'b> {
                        a: &'a mut u32,
                        b: &'b mut u32,
                    }
                )
            ),
            INPUTS.lifetime
        );
        assert_eq!(
            refusal(
                outputs,
                parse_quote!(
                    struct S<'a> {
                        a: &'a u32,
                    }
                )
            ),
            OUTPUTS.lifetime
        );
        assert!(
            inputs(&parse_quote!(
                struct S<'a> {
                    a: &'a mut u32,
                }
            ))
            .is_ok()
        );
    }

    #[test]
    fn the_wiring_is_named_after_the_struct_unless_an_attribute_says_otherwise() {
        let named = |input: DeriveInput, side: &Side| edges_name(&input, side).unwrap().to_string();
        assert_eq!(
            named(
                parse_quote!(
                    struct Pricing {
                        a: u32,
                    }
                ),
                &INPUTS
            ),
            "PricingEdges"
        );
        assert_eq!(
            named(
                parse_quote!(
                    #[binding(edges = "Wiring")]
                    struct Pricing {
                        a: u32,
                    }
                ),
                &OUTPUTS
            ),
            "Wiring"
        );
        let both: DeriveInput = parse_quote!(
            #[binding(edges = "Both", inputs = "In", outputs = "Out")]
            struct Pricing {
                a: u32,
            }
        );
        assert_eq!(named(both.clone(), &INPUTS), "In");
        assert_eq!(named(both, &OUTPUTS), "Out");
    }

    #[test]
    fn the_parameters_and_the_lifetime_never_clash_with_a_field_type() {
        let a: Ident = parse_quote!(a);
        let b: Ident = parse_quote!(b);
        let plain: Type = parse_quote!(Vec<Order>);
        let clashing: Type = parse_quote!(Option<F1>);
        let names = |fields: &[(&Ident, &Type)]| -> Vec<String> {
            fresh_params(fields)
                .iter()
                .map(ToString::to_string)
                .collect()
        };
        assert_eq!(names(&[(&a, &plain), (&b, &plain)]), ["F0", "F1"]);
        assert_eq!(names(&[(&a, &plain), (&b, &clashing)]), ["_F0", "_F1"]);

        let higher: Type = parse_quote!(Box<dyn for<'a> Fn(&'a u32) + Send + Sync>);
        assert_eq!(fresh_lifetime(&[(&a, &plain)]).to_string(), "'a");
        assert_eq!(fresh_lifetime(&[(&a, &higher)]).to_string(), "'b");

        let borrowed: Type = parse_quote!(&'x mut u32);
        let x: Lifetime = parse_quote!('x);
        assert!(mentions(&borrowed, &x));
        assert!(!mentions(&plain, &x));
    }
}
