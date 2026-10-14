//! A `DrainEdge` hands over every token there is, possibly none, so its
//! parameter is a `Vec<T>`; asking for a single `T`, which an empty place
//! could not supply, is the mistake this catches. (`u32` rather than
//! `String`: a type's path in the message must not depend on the features.)

use petritype::{DrainEdge, NetBuilder};

fn main() {
    let mut b = NetBuilder::new();
    let tickets = b.place::<u32>("Tickets");
    let out = b.place::<u32>("Out");

    b.transition("Not a batch", (DrainEdge(tickets),), (out,), |n: u32| (n,));
}
