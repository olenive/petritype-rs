//! Named fields in a routed variant: the fields are a type the net has no
//! name for, so there is no place to route them to.

use petritype::Routed;

#[derive(Debug, Clone)]
struct Parcel(u32);

#[derive(Routed)]
enum Outcome {
    Weighed { parcel: Parcel },
}

fn main() {
    let _ = Outcome::Weighed {
        parcel: Parcel(1),
    };
}
