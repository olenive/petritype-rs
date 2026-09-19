//! A routed type that borrows: a token is an owned value the net keeps, so a
//! payload cannot be a reference into something else.

use petritype::Routed;

#[derive(Debug, Clone)]
struct Parcel(u32);

#[derive(Routed)]
enum Outcome<'a> {
    Weighed(&'a Parcel),
}

fn main() {
    let parcel = Parcel(1);
    let _ = Outcome::Weighed(&parcel);
}
