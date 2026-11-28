//! A guard is a question the library asks to decide what fires, and a
//! question changes no net: it is given the net by shared reference, so a
//! guard that puts a token into its net, the mistake this catches, does not
//! compile. A priority and a selector are given `&Net` the same way, and a
//! key function `&T`.

use petritype::{NetBuilder, TakeEdge};

fn main() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.port(inbox);

    b.transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,))
        .guard(move |net| {
            net.put(inbox, 1);
            true
        });
}
