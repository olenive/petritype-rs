//! A subnet place holds a net and no tokens, so no edge names it: its handle
//! is a `SubnetRef`, which is no `PlaceRef`. The net around it reaches the
//! subnet through the handles `port_of` gives for its ports.

use petritype::{NetBuilder, TakeEdge};

fn main() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let level = b.subnet_place("Level 3", NetBuilder::new().build().unwrap());

    b.transition("Rob", (TakeEdge(level),), (inbox,), |n: u32| (n,));
    b.transition("Stuff", (TakeEdge(inbox),), (level,), |n: u32| (n,));
}
