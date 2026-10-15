//! A struct of inputs with a type parameter: a net wires each field against
//! its type, so the type has to be known.

use petritype::Inputs;

#[derive(Inputs)]
struct Pricing<T> {
    order: T,
    promos: Vec<T>,
}

fn main() {}
