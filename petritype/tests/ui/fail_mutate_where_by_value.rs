//! A `MutateWhereEdge` lends the body the token its filter chose, where it
//! lies, so its parameter is a `&mut T`; taking a `T` by value is the
//! mistake this catches.

use petritype::{Filter, MutateWhereEdge, NetBuilder};

fn main() {
    let mut b = NetBuilder::new();
    let tally = b.place::<u32>("Tally");
    b.keys(tally, ["even"], |n: &u32| [(n % 2 == 0).into()]);

    b.transition(
        "Bump an odd one",
        (MutateWhereEdge(tally, Filter::new().key_in("even", [false])),),
        (),
        |n: u32| {
            let _ = n + 1;
        },
    );
}
