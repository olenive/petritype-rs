//! The DESIGN §5 example: every relationship in it type-checks.

use petritype::{NetBuilder, SnapshotEdge, TakeAllEdge};

#[derive(Debug, Clone)]
struct Parcel {
    id: u32,
    weight: f32,
}

#[derive(Debug, Clone)]
struct Config {
    max_weight: f32,
}

fn main() {
    let mut b = NetBuilder::new();

    let inbox = b.place::<Parcel>("Inbox");
    let heavy = b.place::<Parcel>("Heavy");
    let light = b.place::<Parcel>("Light");
    let count = b.place::<u32>("Count");
    let config = b.place::<Config>("Config");
    b.seed(inbox, [Parcel { id: 1, weight: 3.0 }, Parcel { id: 2, weight: 30.0 }]);
    b.seed(config, [Config { max_weight: 10.0 }]);

    let _weigh = b
        .transition(
            "Weigh",
            (TakeAllEdge(inbox), SnapshotEdge(config)),
            (heavy, light, count),
            |parcels: Vec<Parcel>, cfg: Config| {
                let (h, l): (Vec<_>, Vec<_>) =
                    parcels.into_iter().partition(|p| p.weight > cfg.max_weight);
                let n = (h.len() + l.len()) as u32;
                (h, l, Some(n))
            },
        )
        .id();

    let net = b.build().unwrap();
    assert_eq!(net.transitions().count(), 1);
}
