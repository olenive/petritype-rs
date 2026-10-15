//! `#[derive(Inputs)]`: a body's inputs bound to a net by field name.
//!
//! The derive reads a struct with one named field per input and writes a
//! second struct, the *wiring*, with the same field names and one type
//! parameter per field, plus the engine impls that make the wiring one input
//! position of a transition. Everything it names in `petritype` is either
//! public (`Reads`, `InputEdge`) or in the hidden `petritype::binding`
//! module, as `Routed`'s expansion names `petritype::routing`.

use std::collections::HashSet;

use proc_macro2::{Ident, Span, TokenStream as TokenStream2, TokenTree};
use quote::{format_ident, quote, quote_spanned};
use syn::{Data, DeriveInput, Error, Fields, GenericParam, Lifetime, LitStr, Result, Type};

/// A union has no fields a body could take by name.
const UNION: &str = "`Inputs` cannot be derived for a union: derive it on a struct with one named \
                     field per input";

/// An enum is a choice, and a transition reads every one of its inputs.
const ENUM: &str = "`Inputs` cannot be derived for an enum: a transition reads every input at once, \
                    so derive it on a struct with one named field per input";

/// Binding by name needs names.
const TUPLE: &str = "`Inputs` binds fields by name, so the struct needs named fields: \
                     `struct Pricing { order: Order, .. }`; to bind by position, pass a tuple of \
                     edges instead";

/// A struct of no inputs is the empty tuple, which needs no derive.
const EMPTY: &str = "`Inputs` needs at least one field: a transition with no inputs takes `()`";

/// The wiring says which type each field reads, so the types must be known.
const GENERIC: &str = "`Inputs` cannot be derived for a generic struct: each field's type is what \
                       a net wires against, so it must be concrete; bind a generic body by \
                       position, with a tuple of edges";

/// One lifetime is the one `&mut` fields borrow the net's tokens for.
const TWO_LIFETIMES: &str = "`Inputs` allows one lifetime, the one its `&mut` fields borrow the \
                             net's tokens for during a call";

/// The wiring carries bounds of its own.
const WHERE: &str = "`Inputs` cannot be derived for a struct with a `where` clause: the struct of \
                     edges it generates carries bounds of its own";

/// What `#[binding(..)]` accepts.
const OPTION: &str = "expected `edges = \"Name\"`, `inputs = \"Name\"` or `outputs = \"Name\"`";

/// A struct the derive accepts: its name, visibility and fields, and the
/// names the expansion gives the wiring and its parts.
struct Shape<'a> {
    /// The struct the body takes.
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
    let shape = shape(input)?;
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

/// Checks the declaration and reads off what the expansion needs.
fn shape(input: &DeriveInput) -> Result<Shape<'_>> {
    let mut lifetimes = input.generics.lifetimes();
    let lifetime = lifetimes.next().map(|param| &param.lifetime);
    if let Some(second) = lifetimes.next() {
        return Err(Error::new_spanned(&second.lifetime, TWO_LIFETIMES));
    }
    for param in &input.generics.params {
        match param {
            GenericParam::Type(param) => return Err(Error::new_spanned(&param.ident, GENERIC)),
            GenericParam::Const(param) => return Err(Error::new_spanned(&param.ident, GENERIC)),
            GenericParam::Lifetime(_) => {}
        }
    }
    if let Some(clause) = &input.generics.where_clause {
        return Err(Error::new_spanned(clause.where_token, WHERE));
    }

    let fields: Vec<(&Ident, &Type)> = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) if named.named.is_empty() => {
                return Err(Error::new_spanned(named, EMPTY));
            }
            Fields::Named(named) => named
                .named
                .iter()
                .map(|field| (field.ident.as_ref().expect("a named field"), &field.ty))
                .collect(),
            Fields::Unnamed(unnamed) => return Err(Error::new_spanned(unnamed, TUPLE)),
            Fields::Unit => return Err(Error::new_spanned(&input.ident, EMPTY)),
        },
        Data::Enum(data) => return Err(Error::new_spanned(data.enum_token, ENUM)),
        Data::Union(data) => return Err(Error::new_spanned(data.union_token, UNION)),
    };

    Ok(Shape {
        name: &input.ident,
        vis: &input.vis,
        edges: edges_name(input)?,
        lifetime,
        params: fresh_params(&fields),
        fields,
    })
}

/// The wiring's name: `#[binding(inputs = "..")]`, else
/// `#[binding(edges = "..")]`, else the struct's name with `Edges` appended.
///
/// `outputs` is accepted and left alone, for a struct that derives both
/// bindings and names each wiring separately.
fn edges_name(input: &DeriveInput) -> Result<Ident> {
    let mut general = None;
    let mut own = None;
    for attr in &input.attrs {
        if !attr.path().is_ident("binding") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            let slot = if meta.path.is_ident("edges") {
                &mut general
            } else if meta.path.is_ident("inputs") {
                &mut own
            } else if meta.path.is_ident("outputs") {
                let _: LitStr = meta.value()?.parse()?;
                return Ok(());
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

    /// The message the derive refuses this declaration with.
    fn refusal(input: DeriveInput) -> String {
        match inputs(&input) {
            Ok(_) => panic!("expected the derive to refuse this declaration"),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn the_derive_refuses_what_it_cannot_bind_by_name() {
        assert_eq!(
            refusal(parse_quote!(
                enum E {
                    A(u32),
                }
            )),
            ENUM
        );
        assert_eq!(refusal(parse_quote!(union U { a: u32 })), UNION);
        assert_eq!(
            refusal(parse_quote!(
                struct S(u32);
            )),
            TUPLE
        );
        assert_eq!(
            refusal(parse_quote!(
                struct S;
            )),
            EMPTY
        );
        assert_eq!(
            refusal(parse_quote!(
                struct S {}
            )),
            EMPTY
        );
        assert_eq!(
            refusal(parse_quote!(
                struct S<T> {
                    a: T,
                }
            )),
            GENERIC
        );
        assert_eq!(
            refusal(parse_quote!(
                struct S<const N: usize> {
                    a: [u32; N],
                }
            )),
            GENERIC
        );
        assert_eq!(
            refusal(parse_quote!(
                struct S<'a, 'b> {
                    a: &'a mut u32,
                    b: &'b mut u32,
                }
            )),
            TWO_LIFETIMES
        );
        assert_eq!(
            refusal(parse_quote!(
                struct S
                where
                    u32: Copy,
                {
                    a: u32,
                }
            )),
            WHERE
        );
        assert_eq!(
            refusal(parse_quote!(
                #[binding(wiring = "X")]
                struct S {
                    a: u32,
                }
            )),
            OPTION
        );
    }

    #[test]
    fn the_wiring_is_named_after_the_struct_unless_an_attribute_says_otherwise() {
        let named = |input: DeriveInput| edges_name(&input).unwrap().to_string();
        assert_eq!(
            named(parse_quote!(
                struct Pricing {
                    a: u32,
                }
            )),
            "PricingEdges"
        );
        assert_eq!(
            named(parse_quote!(
                #[binding(edges = "Wiring")]
                struct Pricing {
                    a: u32,
                }
            )),
            "Wiring"
        );
        assert_eq!(
            named(parse_quote!(
                #[binding(edges = "Both", inputs = "In", outputs = "Out")]
                struct Pricing {
                    a: u32,
                }
            )),
            "In"
        );
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
