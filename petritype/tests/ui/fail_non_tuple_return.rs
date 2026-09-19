//! A body that returns a bare `u32` rather than a tuple of slots.
//!
//! Routed outputs did not make this compile, and are not meant to: routing is
//! by type, so only a type that can be a destination may be routed — an enum
//! or struct with `Routed` derived on it. There is deliberately no impl of
//! `RoutedOutput` for a bare `T`, which would overlap `Option<T>` and
//! `Vec<T>` and would claim every std type as a destination. One value goes
//! in a one-slot tuple, `(n * 2,)`, or in a struct of its own.

use petritype::{NetBuilder, TakeEdge};

fn main() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");

    b.transition("Bare value", (TakeEdge(inbox),), (out,), |n: u32| n * 2);
}
