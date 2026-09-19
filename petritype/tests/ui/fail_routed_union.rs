//! A union has no variants to route by: which field is live is not something
//! the net can read.

use petritype::Routed;

#[derive(Routed)]
union Choice {
    number: u32,
    flag: bool,
}

fn main() {
    let _ = Choice { number: 1 };
}
