//! A `TakeAllEdge` hands over every token at once, so its parameter is a
//! `Vec<T>`; asking for a single `T` is the mistake this catches.

use petritype::{NetBuilder, TakeAllEdge};

fn main() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");

    b.transition("Not a batch", (TakeAllEdge(inbox),), (out,), |n: u32| (n,));
}
