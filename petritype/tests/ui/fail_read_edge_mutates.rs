//! A `ReadEdge` lends the body the front token to look at, so its parameter
//! is a `&T`; asking for a `&mut T`, to change what is only lent to be read,
//! is the mistake this catches.

use petritype::{NetBuilder, ReadEdge};

fn main() {
    let mut b = NetBuilder::new();
    let limit = b.place::<u32>("Limit");

    b.transition("Raise", (ReadEdge(limit),), (), |n: &mut u32| {
        *n += 1;
    });
}
