//! A nested place of a token type that does not say which nets it owns.

use petritype::{Net, NetBuilder};

/// A vehicle that carries a net, but has not implemented `Nested`: the engine
/// could not find the net inside it, so the place is refused.
#[derive(Debug)]
struct Vehicle {
    inner: Net,
}

fn main() {
    let mut b = NetBuilder::new();
    let _depot = b.place_nested::<Vehicle>("Depot");
}
