//! A body with fewer parameters than the transition has input edges.

use petritype::{NetBuilder, TakeEdge};

fn main() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");

    // Two edges, one parameter.
    b.transition(
        "Wrong arity",
        (TakeEdge(inbox), TakeEdge(inbox)),
        (out,),
        |n: u32| (n,),
    );
}
