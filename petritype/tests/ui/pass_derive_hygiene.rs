//! The derives compile whatever the caller has in scope (since 0.4.0): in a
//! `#[no_implicit_prelude]` module, beside derive macros imported under the
//! names `Clone` and `Debug`, for structs called `F0` and `F1`, the names of
//! the wiring's own type parameters, and with `petritype` reached through
//! another path. A field type whose own `for<'a>` the wiring's lifetime
//! must not shadow is wired too, and a field `#[cfg]` leaves out is left
//! out of the wiring: the compiler removes it before the derive sees the
//! struct.

#[no_implicit_prelude]
mod bare {
    #[derive(::petritype::Inputs)]
    pub struct Pricing {
        pub order: u32,
    }

    #[derive(::petritype::Outputs)]
    pub struct Priced {
        pub order: u32,
    }

    #[derive(::petritype::Routed)]
    pub enum Outcome {
        Big(u32),
        Nothing,
    }

    #[derive(::petritype::Routed)]
    pub struct Leaf(pub u64);
}

mod shadowed {
    // Derive macros under the names of the derives the wiring has: an
    // expansion that wrote `#[derive(Clone, Copy, Debug)]` would run these
    // on its wiring, and they refuse a generic struct.
    #[allow(unused_imports)]
    use petritype::Inputs as Debug;
    #[allow(unused_imports)]
    use petritype::Outputs as Clone;

    #[derive(petritype::Inputs)]
    pub struct Pricing {
        pub order: u32,
    }

    #[derive(petritype::Outputs)]
    pub struct Priced {
        pub order: u32,
    }
}

/// `petritype` by another path, as a re-export gives it.
mod engine {
    pub use petritype::*;
}

#[derive(petritype::Inputs, petritype::Outputs)]
#[binding(inputs = "ViaIn", outputs = "ViaOut", crate = "crate::engine")]
struct Via {
    n: u32,
}

#[derive(petritype::Routed)]
#[routed(crate = "crate::engine")]
enum Routes {
    Small(u8),
}

/// A struct called by the name the first type parameter would have.
#[derive(petritype::Inputs)]
struct F0 {
    n: u32,
}

/// And one called by the second's, with two fields.
#[derive(petritype::Outputs)]
struct F1 {
    n: u32,
    m: u64,
}

/// A field type with a `for<'a>` of its own.
#[derive(petritype::Inputs)]
struct Hooked {
    hook: for<'a> fn(&'a u32) -> u32,
}

/// A field `#[cfg]` leaves out names a type that does not exist.
#[derive(petritype::Inputs, petritype::Outputs)]
#[binding(inputs = "ConfiguredIn", outputs = "ConfiguredOut")]
struct Configured {
    n: u32,
    #[cfg(any())]
    gone: NoSuchType,
}

fn double(n: &u32) -> u32 {
    n * 2
}

fn main() {
    use petritype::{NetBuilder, TakeEdge};

    let mut b = NetBuilder::new();
    let ns = b.place::<u32>("Ns");
    let ms = b.place::<u64>("Ms");
    let hooks = b.place::<for<'a> fn(&'a u32) -> u32>("Hooks");
    let smalls = b.place::<u8>("Smalls");
    b.seed(ns, [1, 2, 3, 4]);
    b.seed(hooks, [double as for<'a> fn(&'a u32) -> u32]);
    b.transition(
        "Bare",
        (bare::PricingEdges { order: TakeEdge(ns) },),
        bare::PricedEdges { order: ns },
        |p: bare::Pricing| bare::Priced { order: p.order },
    );
    b.transition(
        "Shadowed",
        (shadowed::PricingEdges { order: TakeEdge(ns) },),
        shadowed::PricedEdges { order: ns },
        |p: shadowed::Pricing| shadowed::Priced { order: p.order },
    );
    b.transition(
        "Via",
        (ViaIn { n: TakeEdge(ns) },),
        ViaOut { n: ns },
        |v: Via| v,
    );
    b.transition("Route", (TakeEdge(smalls),), (smalls,), |n: u8| Routes::Small(n));
    b.transition(
        "Named",
        (F0Edges { n: TakeEdge(ns) },),
        F1Edges { n: ns, m: ms },
        |f: F0| F1 { n: f.n, m: u64::from(f.n) },
    );
    b.transition(
        "Hook",
        (HookedEdges { hook: TakeEdge(hooks) }, TakeEdge(ns)),
        (ns,),
        |h: Hooked, n: u32| ((h.hook)(&n),),
    );
    b.transition(
        "Configured",
        (ConfiguredIn { n: TakeEdge(ns) },),
        ConfiguredOut { n: ns },
        |c: Configured| c,
    );
    let net = b.build().expect("every derive wires");
    assert_eq!(net.transitions().count(), 7);
}
