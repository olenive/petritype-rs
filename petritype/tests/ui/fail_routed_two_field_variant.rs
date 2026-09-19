//! A routed variant carries one payload: the place it goes to is the type of
//! that payload, and two fields name no type at all.

use petritype::Routed;

#[derive(Debug, Clone)]
struct Parcel(u32);

#[derive(Routed)]
enum Outcome {
    Weighed(Parcel, f32),
    Nothing,
}

fn main() {
    let _ = Outcome::Nothing;
}
