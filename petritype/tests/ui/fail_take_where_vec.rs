//! A `TakeWhereEdge` hands over the one token its filter chose, so its
//! parameter is a `T`; asking for a `Vec<T>`, as if it took every match, is
//! the mistake this catches.

use petritype::{Filter, NetBuilder, TakeWhereEdge};

fn main() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.keys(inbox, ["even"], |n: &u32| [(n % 2 == 0).into()]);

    b.transition(
        "Every even one",
        (TakeWhereEdge(inbox, Filter::new().key_in("even", [true])),),
        (out,),
        |all: Vec<u32>| (all,),
    );
}
