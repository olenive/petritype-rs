//! A body that captures an `Rc`, which is neither `Send` nor `Sync`.
//!
//! Bodies may run on another thread and the net itself must stay
//! `Send + Sync`, so the bound is on the body, not on the net that holds it.
//! `Arc`, `Mutex` and the atomics are the way to keep state.

use std::rc::Rc;

use petritype::{NetBuilder, TakeEdge};

fn main() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");

    let shared = Rc::new(7_u32);
    b.transition("Not sendable", (TakeEdge(inbox),), (out,), move |n: u32| {
        (n + *shared,)
    });
}
