//! A body parameter whose type is not the one its `Param` position declares.

use petritype::{NetBuilder, Param, TakeEdge};

/// Two parameter types of this crate's own, so that the expected message does
/// not depend on how rustc chooses to spell a standard-library path.
#[derive(Clone)]
struct ApiToken(u32);

#[derive(Clone)]
struct Scale(u32);

fn main() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.param(ApiToken(1));

    // `Param::<ApiToken>::new()` hands over an `ApiToken`, not a `Scale`.
    b.transition(
        "Wrong parameter type",
        (TakeEdge(inbox), Param::<ApiToken>::new()),
        (out,),
        |n: u32, scale: Scale| (n * scale.0,),
    );
}
