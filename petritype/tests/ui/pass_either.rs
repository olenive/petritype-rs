//! `Either` routes two answers without an enum of one's own, and nests
//! inside `Option` and `Result` like any other routed value.

use petritype::{Either, NetBuilder, Routed, TakeEdge};

// Each side of an `Either` is itself a routed value, so a payload struct
// says so with the derive: it is a leaf, routed whole to the place of its
// own type.
#[derive(Debug, Clone, Routed)]
struct Accepted(u32);

#[derive(Debug, Clone, Routed)]
struct Rejected(u32);

#[derive(Debug, Clone, Routed)]
struct Logged(u32);

#[derive(Debug)]
struct Refused;

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("refused")
    }
}

impl std::error::Error for Refused {}

#[derive(Routed)]
enum Verdict {
    Yes(Accepted),
    No(Rejected),
}

fn main() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let accepted = b.place::<Accepted>("Accepted");
    let rejected = b.place::<Rejected>("Rejected");
    let logged = b.place::<Logged>("Logged");

    b.transition(
        "Check",
        (TakeEdge(inbox),),
        (accepted, rejected),
        |n: u32| {
            if n % 2 == 0 {
                Either::Left(Accepted(n))
            } else {
                Either::Right(Rejected(n))
            }
        },
    );

    // A routed value inside an `Option` inside a `Result`: failure wraps the
    // form rather than replacing it, and `None` simply emits nothing.
    b.transition(
        "Log",
        (TakeEdge(inbox),),
        (logged, accepted, rejected),
        |n: u32| -> Result<Option<Either<Logged, Verdict>>, Refused> {
            match n {
                0 => Err(Refused),
                1 => Ok(None),
                2 => Ok(Some(Either::Left(Logged(n)))),
                _ => Ok(Some(Either::Right(Verdict::Yes(Accepted(n))))),
            }
        },
    );

    let net = b.build().unwrap();
    assert_eq!(net.transitions().count(), 2);
}
