//! More output places than the body returns slots.

use petritype::{NetBuilder, TakeEdge};

fn main() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let first = b.place::<u32>("First");
    let second = b.place::<u32>("Second");

    b.transition(
        "Too few slots",
        (TakeEdge(inbox),),
        (first, second),
        |n: u32| (n,),
    );
}
