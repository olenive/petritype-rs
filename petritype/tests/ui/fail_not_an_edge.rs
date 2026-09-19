//! A place handle used as an input without saying how the place is read.

use petritype::NetBuilder;

fn main() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");

    b.transition("No edge kind", (inbox,), (out,), |n: u32| (n,));
}
