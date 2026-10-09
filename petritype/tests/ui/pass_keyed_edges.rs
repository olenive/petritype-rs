//! The two keyed edges hand the body what their plain counterparts do: a
//! `TakeWhereEdge` a `T`, a `MutateWhereEdge` a `&mut T`, each the token its
//! filter chose.

use petritype::{Filter, MutateWhereEdge, NetBuilder, TakeWhereEdge};

fn main() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let tally = b.place::<u32>("Tally");
    for place in [inbox, tally] {
        b.keys(place, ["even"], |n: &u32| [(n % 2 == 0).into()]);
    }
    b.seed(inbox, [1, 2]);
    b.seed(tally, [3, 4]);

    b.transition(
        "Count an even one into an odd one",
        (
            TakeWhereEdge(inbox, Filter::new().key_in("even", [true])),
            MutateWhereEdge(tally, Filter::new().key_in("even", [false])),
        ),
        (),
        |even: u32, odd: &mut u32| {
            *odd += even;
        },
    );

    let mut net = b.build().unwrap();
    assert_eq!(net.step().unwrap().map(|t| t.index()), Some(0));
}
