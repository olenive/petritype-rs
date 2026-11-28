//! `#[binding(crate = "..")]` and `#[routed(crate = "..")]` name the path
//! the expansion reaches `petritype` by, so a path to nothing is reported
//! where the expansion uses it.

#[derive(petritype::Inputs)]
#[binding(crate = "nowhere")]
struct Pricing {
    order: u32,
}

#[derive(petritype::Routed)]
#[routed(crate = "nowhere")]
enum Outcome {
    Big(u32),
}

fn main() {}
