//! A body that returns `Result<_, E>` where `E` does not implement
//! `std::error::Error`.
//!
//! A `Result` always means failure, and a failure has to be reportable: the
//! engine erases `E` to `Box<dyn Error + Send + Sync>` for the event log.

use petritype::{NetBuilder, TakeEdge};

/// No `Display`, no `Error`.
#[derive(Debug)]
struct Whoops;

fn main() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");

    b.transition(
        "Unreportable failure",
        (TakeEdge(inbox),),
        (out,),
        |n: u32| -> Result<(u32,), Whoops> { Ok((n,)) },
    );
}
