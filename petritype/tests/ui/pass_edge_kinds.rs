//! Each edge kind hands the body the parameter its row of the table promises,
//! including the borrow a mutating edge produces.

use petritype::{MutateEdge, NetBuilder, SnapshotEdge, TakeAllEdge, TakeEdge};

fn main() {
    let mut b = NetBuilder::new();
    let one = b.place::<u32>("One");
    let many = b.place::<u32>("Many");
    let read = b.place::<String>("Read");
    let live = b.place::<u32>("Live");

    b.transition(
        "Every kind",
        (TakeEdge(one), TakeAllEdge(many), SnapshotEdge(read), MutateEdge(live)),
        (),
        |taken: u32, batch: Vec<u32>, note: String, live: &mut u32| {
            *live += taken + batch.len() as u32 + note.len() as u32;
        },
    );

    let net = b.build().unwrap();
    assert_eq!(net.transitions().count(), 1);
}
