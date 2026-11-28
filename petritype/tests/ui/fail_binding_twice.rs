//! A key given twice in `#[binding(..)]` or `#[routed(..)]`, in one
//! attribute or across two, is refused (since 0.4.0): the second would
//! silently replace the first.

#[derive(petritype::Inputs)]
#[binding(edges = "First", edges = "Second")]
struct Pricing {
    order: u32,
}

#[derive(petritype::Outputs)]
#[binding(outputs = "First")]
#[binding(outputs = "Second")]
struct Priced {
    order: u32,
}

#[derive(petritype::Routed)]
#[routed(crate = "::petritype", crate = "::petritype")]
enum Outcome {
    Big(u32),
}

fn main() {}
