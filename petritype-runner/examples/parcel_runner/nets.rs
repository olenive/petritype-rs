//! The nets the `parcel_runner` example carries, in a module of their own so
//! that the transcript tests can load the very same registry without a
//! process.
//!
//! Besides the parcel nets it carries the protocol's four other conformance
//! nets, `transit` (PROTOCOL.md §11, §12), `wildlife` (§11, §13), `shop` and
//! `traffic-light` (§11, §5.1): the core's own example nets, included from
//! `petritype/examples/transit/net.rs`, `petritype/examples/wildlife/net.rs`,
//! `petritype/examples/shop/net.rs` and
//! `petritype/examples/traffic_light/net.rs` as they are, not copied.

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

/// The protocol's conformance net `transit` (PROTOCOL.md §11): a city whose
/// vehicles carry nets of their own, the nested-nets net of §12, which runs
/// its journey under a plain `step` with nothing switched off. The file is
/// the core's example (`petritype/examples/transit/net.rs`), which depends
/// on `petritype`, `serde` and `serde_json` only so that it can be included
/// here as it is; whatever of it this binary does not call is there for the
/// core example and its tests.
#[path = "../../../petritype/examples/transit/net.rs"]
#[allow(dead_code)]
pub mod transit_net;

/// The city as §11 seeds it: one bus at `Depot` and one ferry at `Harbour`,
/// every door open, seven passengers on the platforms of `Depot`,
/// `Central` and `Harbour`, every transition switched on. Its one
/// parameter, `destination` (`null`, or a station every departure goes to
/// instead of the next on the loop), is a net parameter set with
/// `set_param`, and since 2026-10-12 a load parameter too, its value as
/// built, as the Python factory's keyword is (`TransitParams`).
pub fn transit(params: TransitParams) -> Result<Net, LoadError> {
    transit_net::build_city_with(params.destination)
        .map(|city| city.net)
        .map_err(|error| LoadError::Other(error.to_string()))
}

/// `transit`'s load parameters: `destination`, a station's name or `null`,
/// decoded as the net parameter is, so a number is `invalid_params`.
#[derive(Deserialize)]
pub struct TransitParams {
    #[serde(default)]
    destination: Option<transit_net::Location>,
}

/// The `transit` entry, as PROTOCOL.md §11 names and describes it.
fn transit_entry() -> NetEntry {
    NetEntry::typed("transit", transit)
        .description("Vehicles board, carry and alight passengers")
        .param(ParamSpec::new("destination", "Option<Location>").default(serde_json::Value::Null))
        .hosting(Hosting::Manual)
}

/// The `transit` net alone, for the transcripts over it (PROTOCOL.md §11:
/// replayed against a runner carrying the transcript's own net alone).
#[allow(dead_code)]
pub fn transit_registry() -> Vec<NetEntry> {
    vec![transit_entry()]
}

/// The protocol's conformance net `wildlife` (PROTOCOL.md §11): species
/// across habitats, each place declaring the keys `species`, `class` and
/// `diet`, and every transition but `Settle` a keyed edge (two migrations
/// taking the oldest bird, a spread and two declines mutating the species
/// their filters name) — the keys net of §13. The file is the core's example
/// (`petritype/examples/wildlife/net.rs`), which depends on `petritype`,
/// `serde` and `serde_json` only, for this.
#[path = "../../../petritype/examples/wildlife/net.rs"]
#[allow(dead_code)]
pub mod wildlife_net;

/// The habitats as §11 seeds them: twenty species-in-a-habitat, four to
/// each of the five habitats, nothing migrating. Its parameters, `season`
/// (`"Autumn"`) and `spread_threshold` (`200`), are net parameters set with
/// `set_param`, not load parameters, so the entry declares none.
pub fn wildlife(_: &petritype_runner::Params) -> Result<Net, LoadError> {
    wildlife_net::build_wildlife()
        .map(|wildlife| wildlife.net)
        .map_err(LoadError::from)
}

/// The `wildlife` entry, as PROTOCOL.md §11 names and describes it.
fn wildlife_entry() -> NetEntry {
    NetEntry::new("wildlife", wildlife)
        .description("Species migrate, spread and decline across habitats")
        .hosting(Hosting::Manual)
}

/// The `wildlife` net alone, for the transcripts over it (PROTOCOL.md §11).
#[allow(dead_code)]
pub fn wildlife_registry() -> Vec<NetEntry> {
    vec![wildlife_entry()]
}

/// The protocol's conformance net `shop` (PROTOCOL.md §11): orders priced
/// with whatever promotions are running, through a survey edge, an input
/// edge that accepts an empty place, `"survey"` on the wire (§5.1). The file
/// is the core's example (`petritype/examples/shop/net.rs`), which depends
/// on `petritype` only, for this.
#[path = "../../../petritype/examples/shop/net.rs"]
#[allow(dead_code)]
pub mod shop_net;

/// The shop as §11 seeds it: empty everywhere, every place one a host may
/// `inject` a string into. No parameters of either kind.
pub fn shop(_: &petritype_runner::Params) -> Result<Net, LoadError> {
    shop_net::build_shop()
        .map(|shop| shop.net)
        .map_err(LoadError::from)
}

/// The `shop` entry, as PROTOCOL.md §11 names and describes it.
fn shop_entry() -> NetEntry {
    NetEntry::new("shop", shop)
        .description("Price orders with whatever promotions are running")
        .hosting(Hosting::Manual)
}

/// The `shop` net alone, for the transcripts over it (PROTOCOL.md §11).
#[allow(dead_code)]
pub fn shop_registry() -> Vec<NetEntry> {
    vec![shop_entry()]
}

/// The protocol's conformance net `traffic-light` (PROTOCOL.md §11): cars
/// waiting at a light that cycles red, green and amber, every one of them
/// going when it turns green, through a drain edge, the other input edge
/// that accepts an empty place, `"drain"` on the wire (§5.1); a car that
/// arrives while the light is green reads it through a snapshot edge and
/// goes straight through. The file is the core's example
/// (`petritype/examples/traffic_light/net.rs`), which depends on `petritype`
/// only, for this.
#[path = "../../../petritype/examples/traffic_light/net.rs"]
#[allow(dead_code)]
pub mod traffic_light_net;

/// The light as §11 seeds it: the light in `Red`, nobody waiting, every
/// place one a host may `inject` a string into. No parameters of either
/// kind. The net never quiesces: some light transition is always fireable.
pub fn traffic_light(_: &petritype_runner::Params) -> Result<Net, LoadError> {
    traffic_light_net::build_traffic_light()
        .map(|light| light.net)
        .map_err(LoadError::from)
}

/// The `traffic-light` entry, as PROTOCOL.md §11 names and describes it.
fn traffic_light_entry() -> NetEntry {
    NetEntry::new("traffic-light", traffic_light)
        .description(
            "Cars wait at a light that cycles red, green and amber, and all go when it turns green",
        )
        .hosting(Hosting::Manual)
}

/// The `traffic-light` net alone, for the transcripts over it (PROTOCOL.md
/// §11).
#[allow(dead_code)]
pub fn traffic_light_registry() -> Vec<NetEntry> {
    vec![traffic_light_entry()]
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
        transit_entry(),
        wildlife_entry(),
        shop_entry(),
        traffic_light_entry(),
    ]
}
