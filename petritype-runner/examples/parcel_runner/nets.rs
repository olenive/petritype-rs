//! The nets the `parcel_runner` example carries, in a module of their own so
//! that the transcript tests can load the very same registry without a
//! process.

use std::fmt;

use petritype::{MutateEdge, Net, NetBuilder, TakeAllEdge, TakeEdge};
use petritype_runner::{Hosting, LoadError, NetEntry, ParamSpec};
use serde::{Deserialize, Serialize};

/// A parcel. Places of parcels are marked serializable, so a host can
/// `inject` one as JSON and read one back with `get_token`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Parcel {
    pub id: u32,
    pub weight: u32,
}

/// The trucks, the one loading next, and how many parcels they have taken.
/// Changed in place by a mutate edge, which is what makes `Dispatch` run
/// inline.
#[derive(Debug, Clone, PartialEq)]
pub struct Fleet {
    pub trucks: u32,
    pub next_truck: u32,
    pub loads: u32,
}

/// A net-wide switch a host can flip with `set_param {param: "paused"}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Paused(pub bool);

/// Why `Inspect` failed.
#[derive(Debug)]
pub struct TooHeavy {
    pub id: u32,
    pub weight: u32,
}

impl fmt::Display for TooHeavy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "parcel {} is too heavy: {} kg", self.id, self.weight)
    }
}

impl std::error::Error for TooHeavy {}

/// `load {params}` for the parcel fleet net, decoded before the factory runs.
#[derive(Debug, Deserialize)]
pub struct ParcelParams {
    pub trucks: u32,
}

/// `Receiving -Sort-> Sorted -Dispatch-> Dispatched`, `Dispatch` also
/// mutating `Fleet`, and `Inspection -Inspect-> Receiving`, which refuses a
/// parcel over 100 kg. Two parcels arrive with the net.
pub fn parcel_fleet(params: ParcelParams) -> Result<Net, LoadError> {
    if params.trucks == 0 {
        return Err(LoadError::InvalidParams("trucks must be at least 1".into()));
    }
    let mut b = NetBuilder::new();
    let receiving = b.place::<Parcel>("Receiving");
    let sorted = b.place::<Parcel>("Sorted");
    let fleet = b.place::<Fleet>("Fleet");
    let dispatched = b.place::<Parcel>("Dispatched");
    let inspection = b.place::<Parcel>("Inspection");
    b.serializable(receiving);
    b.serializable(inspection);
    b.seed(
        receiving,
        [Parcel { id: 1, weight: 5 }, Parcel { id: 2, weight: 12 }],
    );
    b.seed(
        fleet,
        [Fleet {
            trucks: params.trucks,
            next_truck: 1,
            loads: 0,
        }],
    );
    b.param(Paused(false));
    b.serializable_param::<Paused>("paused");

    b.transition("Sort", (TakeEdge(receiving),), (sorted,), |p: Parcel| {
        // A body that prints: on stdout this would corrupt the protocol, so
        // the runner moves it to stderr.
        println!("sorting parcel {}", p.id);
        (p,)
    })
    .guard(|net: &Net| !net.param::<Paused>().is_some_and(|p| p.0));
    b.transition(
        "Dispatch",
        (TakeEdge(sorted), MutateEdge(fleet)),
        (dispatched,),
        |p: Parcel, fleet: &mut Fleet| {
            fleet.loads += 1;
            fleet.next_truck = fleet.next_truck % fleet.trucks + 1;
            (p,)
        },
    );
    b.transition(
        "Inspect",
        (TakeEdge(inspection),),
        (receiving,),
        |p: Parcel| -> Result<(Parcel,), TooHeavy> {
            if p.weight > 100 {
                Err(TooHeavy {
                    id: p.id,
                    weight: p.weight,
                })
            } else {
                Ok((p,))
            }
        },
    );
    Ok(b.build()?)
}

/// `Inbox -Shelve-> Shelf`, empty at load: a net that waits for parcels to be
/// injected, so it is hosted 24/7 and idles in between.
pub fn parcel_intake(_: &petritype_runner::Params) -> Result<Net, LoadError> {
    let mut b = NetBuilder::new();
    let inbox = b.place::<Parcel>("Inbox");
    let shelf = b.place::<Parcel>("Shelf");
    b.serializable(inbox);
    b.transition("Shelve", (TakeEdge(inbox),), (shelf,), |p: Parcel| (p,));
    Ok(b.build()?)
}

/// The shared conformance net of PROTOCOL.md §11, `parcel-distribution`:
/// `petritype/examples/parcel_distribution.rs` copied, with its places marked
/// serializable so that the transcripts the Python repository holds
/// (`petritype/tests/protocol/`) replay here too. Its Python twin is
/// `petritype/tests/protocol_nets/parcel_distribution.py`; names, seeds,
/// edges, slots, the guard and the deferral rule must stay the same in all
/// three.
pub mod conformance {
    use super::*;

    /// A parcel, and how many trucks have already gone without it. The field
    /// names are the Python model's, so the JSON forms agree; `deferrals`
    /// defaults to 0 there, and so here.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct Parcel {
        pub id: u32,
        #[serde(default)]
        pub deferrals: u32,
    }

    /// `Receiving -Sort-> Sorting -Stage-> Staging Area -Dispatch Truck->
    /// Dispatched`, seeded with eight parcels. `Dispatch Truck` takes the
    /// whole platform, sends slot 0 away and writes slot 1 back to the
    /// platform; every fourth parcel misses its first truck.
    pub fn parcel_distribution(_: &petritype_runner::Params) -> Result<Net, LoadError> {
        let mut b = NetBuilder::new();
        let receiving = b.place::<Parcel>("Receiving");
        let sorting = b.place::<Parcel>("Sorting");
        let staging = b.place::<Parcel>("Staging Area");
        let dispatched = b.place::<Parcel>("Dispatched");
        for place in [receiving, sorting, staging, dispatched] {
            b.serializable(place);
        }
        b.seed(receiving, (1..=8).map(|id| Parcel { id, deferrals: 0 }));

        b.transition("Sort", (TakeEdge(receiving),), (sorting,), |p: Parcel| (p,));
        b.transition("Stage", (TakeEdge(sorting),), (staging,), |p: Parcel| (p,));
        b.transition(
            "Dispatch Truck",
            (TakeAllEdge(staging),),
            // Slot 0 leaves on the truck, slot 1 waits for the next one.
            (dispatched, staging),
            |parcels: Vec<Parcel>| {
                let (mut gone, mut left) = (Vec::new(), Vec::new());
                for mut parcel in parcels {
                    // Every fourth parcel misses its first truck.
                    if parcel.id % 4 == 0 && parcel.deferrals == 0 {
                        parcel.deferrals += 1;
                        left.push(parcel);
                    } else {
                        gone.push(parcel);
                    }
                }
                (gone, left)
            },
        )
        .guard(move |net: &Net| {
            // A truck waits for a load, unless nothing more is coming.
            net.count(staging.id()) >= 2
                || (net.count(receiving.id()) == 0 && net.count(sorting.id()) == 0)
        });
        Ok(b.build()?)
    }
}

/// The conformance net's entry, as PROTOCOL.md §11 names and describes it.
fn conformance_entry() -> NetEntry {
    NetEntry::new("parcel-distribution", conformance::parcel_distribution)
        .description("Sort, stage and dispatch parcels")
        .hosting(Hosting::Manual)
}

/// The conformance net alone: what PROTOCOL.md §11 says the shared
/// transcripts are replayed against, so that they may pin `describe` in full.
/// Used by `tests/transcripts.rs`, which includes this module; the example
/// binary itself carries every net.
#[allow(dead_code)]
pub fn conformance_registry() -> Vec<NetEntry> {
    vec![conformance_entry()]
}

/// Every net the example binary carries.
pub fn registry() -> Vec<NetEntry> {
    vec![
        NetEntry::typed("parcel-fleet", parcel_fleet)
            .description("Sort and dispatch parcels")
            .param(ParamSpec::new("trucks", "u32").default(2)),
        conformance_entry(),
        NetEntry::new("parcel-intake", parcel_intake)
            .description("Shelve parcels as they are injected")
            .hosting(Hosting::Continuous),
    ]
}
