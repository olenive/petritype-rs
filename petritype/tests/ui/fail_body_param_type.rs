//! A body parameter whose type is not what its edge produces.

use petritype::{NetBuilder, TakeEdge};

/// A token type of this crate's own, so that the expected message does not
/// depend on how rustc chooses to spell a standard-library path.
#[derive(Debug, Clone)]
struct Note(u32);

fn main() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");

    // `TakeEdge(inbox)` hands over a `u32`, not a `Note`.
    b.transition("Wrong type", (TakeEdge(inbox),), (out,), |n: Note| (n.0,));
}
