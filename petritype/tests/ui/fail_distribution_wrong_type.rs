//! A token that does not fit the place it is addressed to.
//!
//! This is the half of the `Distribution` form that *is* static:
//! `to` takes a `PlaceRef<T>` and a `T`, so the handle carries the place's
//! token type and rustc checks the pair like any other argument. What it
//! cannot check is that the place is one of the transition's outputs — that
//! is `DistributionError`, at firing time.

use petritype::{Distribution, NetBuilder};

fn main() {
    let mut b = NetBuilder::new();
    let count = b.place::<u32>("Count");

    let _ = Distribution::new().to(count, "text");
}
