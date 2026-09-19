//! A positional output slot whose type is not the type of its output place.

use petritype::{NetBuilder, TakeEdge};

/// A token type of this crate's own, so that the expected message does not
/// depend on how rustc chooses to spell a standard-library path.
#[derive(Debug, Clone)]
struct Note(u32);

fn main() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let notes = b.place::<Note>("Notes");

    // Slot 0 feeds a place of `Note`s, so a `u32` does not fit it.
    b.transition("Wrong slot", (TakeEdge(inbox),), (notes,), |n: u32| (n,));
}
