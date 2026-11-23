//! A body handed a `&T` by a `ReadEdge`, or a `Vec<&T>` by a `ReadAllEdge`,
//! cannot write through it: the borrow is shared, so the token stays as it
//! was.

use petritype::{NetBuilder, ReadAllEdge, ReadEdge};

fn main() {
    let mut b = NetBuilder::new();
    let limit = b.place::<u32>("Limit");
    let seen = b.place::<u32>("Seen");

    b.transition(
        "Look",
        (ReadEdge(limit), ReadAllEdge(seen)),
        (),
        |n: &u32, all: Vec<&u32>| {
            *n += 1;
            for m in all {
                *m += 1;
            }
        },
    );
}
