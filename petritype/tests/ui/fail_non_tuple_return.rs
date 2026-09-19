//! A body that returns a bare value rather than a tuple of slots.
//!
//! This must fail *for now*: routed outputs, where a value goes to the output
//! place of its type without any tuple to order, are a later slice. When they
//! land, this file becomes a `pass` case — the marker parameter on
//! `IntoOutcome` is what lets that impl be added without disturbing the
//! positional one.

use petritype::{NetBuilder, TakeEdge};

fn main() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");

    b.transition("Bare value", (TakeEdge(inbox),), (out,), |n: u32| n * 2);
}
