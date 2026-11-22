//! The nets the `parcel_runner` example carries, in a module of their own so
//! that the transcript tests can load the very same registry without a
//! process.
//!
//! Besides the parcel nets it carries the protocol's five other conformance
//! nets: `transit` (PROTOCOL.md §11, §12), `wildlife` (§11, §13), `shop` and
//! `traffic-light` (§11, §5.1), the core's own example nets, included from
//! `petritype/examples/transit/net.rs`, `petritype/examples/wildlife/net.rs`,
//! `petritype/examples/shop/net.rs` and
//! `petritype/examples/traffic_light/net.rs` as they are, not copied; and
//! `stepping-rules` (§11, §12.3), which has no example of its own and is
//! written here. The conformance nets `levels` (§11, §12.7) and `groups`
//! (§11, §5.1) are written here too, each carried alone by its own registry
//! ([`levels_registry`], [`groups_registry`]) for the transcripts over it
//! and by the example binary with the rest.

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
/// their filters name). It is the keys net of §13. The file is the core's
/// example (`petritype/examples/wildlife/net.rs`), which depends on
/// `petritype`, `serde` and `serde_json` only, for this.
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
/// going in one firing of `Go` while it is green, through a drain edge, the
/// other input edge that accepts an empty place, `"drain"` on the wire
/// (§5.1); `Go` reads the light through a snapshot edge, and `Change` moves
/// it on. It declares two controls, `car` and `change`, which `press` runs,
/// and colours the light's token, which fills its place (§5.1). The file is
/// the core's example (`petritype/examples/traffic_light/net.rs`), which
/// depends on `petritype` and `serde` only, for this.
#[path = "../../../petritype/examples/traffic_light/net.rs"]
#[allow(dead_code)]
pub mod traffic_light_net;

/// The light as §11 seeds it: red (`"red"` on the wire) in `Light`, nobody
/// waiting, every place one a host may `inject` into, a car's name or a
/// colour's. No parameters of either kind. The net never quiesces: `Change`
/// is always fireable.
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

/// The protocol's conformance net `stepping-rules` (PROTOCOL.md §11, §12.3):
/// places of nets under each of the four stepping rules, and a second level,
/// so that a transcript shows which transition each `step` fires. Its
/// names, places, transitions, templates and seed are fixed by §11; the
/// Python twin is `tests/protocol_nets/stepping_rules.py`. Every body
/// passes its job on unchanged, and every token is an integer or a net.
pub mod stepping_rules {
    use std::error::Error;

    use petritype::{Net, NetBuilder, Params, Stepping, TakeEdge, Templates};
    use petritype_runner::LoadError;

    /// The parameters that build a worker with `jobs` jobs.
    fn jobs(jobs: u32) -> Params {
        let mut params = Params::new();
        params.insert("jobs".into(), jobs.into());
        params
    }

    /// The template `Worker`: `Todo -Work-> Done`, both of integers, `Work`
    /// taking the front of `Todo` and putting it into `Done`. Built with
    /// `jobs` *n*, it holds `0` up to *n* less one in `Todo`, in that order;
    /// with none, nothing.
    fn worker(params: &Params) -> Result<Net, petritype::BuildError> {
        let jobs = params
            .get("jobs")
            .and_then(serde_json::Value::as_u64)
            .map_or(0, |jobs| u32::try_from(jobs).unwrap_or(u32::MAX));
        let mut b = NetBuilder::new();
        let todo = b.place::<u32>("Todo");
        let done = b.place::<u32>("Done");
        b.serializable(todo);
        b.serializable(done);
        b.seed(todo, 0..jobs);
        b.transition("Work", (TakeEdge(todo),), (done,), |job: u32| (job,));
        b.build()
    }

    /// The template `Team`: `Todo -Plan-> Done` as a worker's, holding `0`,
    /// and `Helpers`, a place of nets that takes turns with the team,
    /// holding one worker with two jobs.
    fn team(workers: &Templates) -> Result<Net, Box<dyn Error + Send + Sync>> {
        let mut b = NetBuilder::new();
        b.templates(workers);
        let todo = b.place::<u32>("Todo");
        let done = b.place::<u32>("Done");
        let helpers = b.place_nested::<Net>("Helpers");
        b.serializable(todo);
        b.serializable(done);
        b.serializable(helpers);
        b.stepping(helpers, Stepping::TakeTurns);
        b.seed(todo, [0]);
        b.seed(helpers, [workers.build("Worker", &jobs(2))?]);
        b.transition("Plan", (TakeEdge(todo),), (done,), |job: u32| (job,));
        Ok(b.build()?)
    }

    /// `Worker` and `Team`, each under its name, which every place of nets
    /// here is built and restored from.
    pub fn templates() -> Templates {
        let mut workers = Templates::new();
        workers.register("Worker", worker);
        let mut templates = workers.clone();
        templates.register("Team", move |_: &Params| team(&workers));
        templates
    }

    /// The net as §11 seeds it: `Late` (outer first) one worker with one
    /// job, `Ring` (take turns) one with two, `Early` (inner first) one
    /// team, `Shelf` (passive) one worker with five, `Inbox` `0`, `1` and
    /// `2`, `Outbox` nothing, and `Forward` from `Inbox` to `Outbox`. A token
    /// of a place of nets is a net, so its one inner net is named `""`.
    pub fn stepping_rules(_: &petritype_runner::Params) -> Result<Net, LoadError> {
        let templates = templates();
        let build = |name: &str, params: &Params| {
            templates
                .build(name, params)
                .map_err(|error| LoadError::Other(error.to_string()))
        };
        let mut b = NetBuilder::new();
        b.templates(&templates);
        let late = b.place_nested::<Net>("Late");
        let ring = b.place_nested::<Net>("Ring");
        let early = b.place_nested::<Net>("Early");
        let shelf = b.place_nested::<Net>("Shelf");
        let inbox = b.place::<u32>("Inbox");
        let outbox = b.place::<u32>("Outbox");
        for (place, rule) in [
            (late, Stepping::OuterFirst),
            (ring, Stepping::TakeTurns),
            (early, Stepping::InnerFirst),
            (shelf, Stepping::Passive),
        ] {
            b.serializable(place);
            b.stepping(place, rule);
        }
        b.serializable(inbox);
        b.serializable(outbox);
        b.seed(late, [build("Worker", &jobs(1))?]);
        b.seed(ring, [build("Worker", &jobs(2))?]);
        b.seed(early, [build("Team", &Params::new())?]);
        b.seed(shelf, [build("Worker", &jobs(5))?]);
        b.seed(inbox, 0..3);
        b.transition("Forward", (TakeEdge(inbox),), (outbox,), |job: u32| (job,));
        Ok(b.build()?)
    }
}

/// The `stepping-rules` entry, as PROTOCOL.md §11 names and describes it.
fn stepping_rules_entry() -> NetEntry {
    NetEntry::new("stepping-rules", stepping_rules::stepping_rules)
        .description("Nets under each stepping rule take their turns")
        .hosting(Hosting::Manual)
}

/// The `stepping-rules` net alone, for the transcripts over it (PROTOCOL.md
/// §11).
#[allow(dead_code)]
pub fn stepping_rules_registry() -> Vec<NetEntry> {
    vec![stepping_rules_entry()]
}

/// The protocol's conformance net `levels` (PROTOCOL.md §11, §12.7): a
/// small world with two levels built from one template, a hero entering a
/// level through one port place and leaving through another. Its names,
/// places, transitions, guards, template and seed are fixed by §11; the
/// Python twin is `tests/protocol_nets/levels.py`. It is not the core's
/// `levels` example, whose heroes count the levels they clear and fight
/// once on the way in: this one walks into the arena, fights there and
/// wins, so that a transcript shows a firing out of a port, one wholly
/// inside and one into a port.
pub mod levels {
    use petritype::{Net, NetBuilder, Params, PlaceId, TakeEdge, Templates};
    use petritype_runner::LoadError;
    use serde::{Deserialize, Serialize};

    /// A hero, and whether they have fought in the level they are in.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct Hero {
        pub name: String,
        pub fought: bool,
    }

    /// The hero at the front of `place`.
    fn front(net: &Net, place: PlaceId) -> Option<&Hero> {
        let (token, _) = net.tokens(place).next()?;
        net.token::<Hero>(place, token)
    }

    /// The template `Level`: `Lobby -Walk In-> Arena -Fight-> Arena -Win->
    /// Exit Hall`, the lobby and the exit hall its ports, every place
    /// serializable and empty. `Walk In` sets `fought` false; `Fight`, for
    /// a front hero who has not fought, sets it true; `Win`, for one who
    /// has, hands them on. No parameters.
    fn level(_: &Params) -> Result<Net, petritype::BuildError> {
        let mut b = NetBuilder::new();
        let lobby = b.place::<Hero>("Lobby");
        let arena = b.place::<Hero>("Arena");
        let exit = b.place::<Hero>("Exit Hall");
        for place in [lobby, arena, exit] {
            b.serializable(place);
        }
        b.port(lobby);
        b.port(exit);
        b.transition("Walk In", (TakeEdge(lobby),), (arena,), |hero: Hero| {
            (Hero {
                fought: false,
                ..hero
            },)
        });
        b.transition("Fight", (TakeEdge(arena),), (arena,), |hero: Hero| {
            (Hero {
                fought: true,
                ..hero
            },)
        })
        .guard(move |net: &Net| front(net, arena.id()).is_some_and(|hero| !hero.fought));
        b.transition("Win", (TakeEdge(arena),), (exit,), |hero: Hero| (hero,))
            .guard(move |net: &Net| front(net, arena.id()).is_some_and(|hero| hero.fought));
        b.build()
    }

    /// `Level`, under its name, which builds both levels and stamps them.
    pub fn templates() -> Templates {
        let mut templates = Templates::new();
        templates.register("Level", level);
        templates
    }

    /// The world as §11 seeds it: `Door` holding Ada, who has not fought;
    /// `Level 1` and `Level 2`, subnet places each holding a `Level`, inner
    /// first; and `Outside`. `Enter` takes the hero from the door into
    /// level 1's lobby, `Climb` from level 1's exit hall into level 2's
    /// lobby, and `Leave` from level 2's exit hall to `Outside`.
    pub fn levels(_: &petritype_runner::Params) -> Result<Net, LoadError> {
        let templates = templates();
        let level = || {
            templates
                .build("Level", &Params::new())
                .map_err(|error| LoadError::Other(error.to_string()))
        };
        let mut b = NetBuilder::new();
        b.templates(&templates);
        let door = b.place::<Hero>("Door");
        let level_1 = b.subnet_place("Level 1", level()?);
        let level_2 = b.subnet_place("Level 2", level()?);
        let outside = b.place::<Hero>("Outside");
        b.serializable(door);
        b.serializable(outside);
        let lobby_1 = b.port_of::<Hero>(level_1, "Lobby");
        let exit_1 = b.port_of::<Hero>(level_1, "Exit Hall");
        let lobby_2 = b.port_of::<Hero>(level_2, "Lobby");
        let exit_2 = b.port_of::<Hero>(level_2, "Exit Hall");
        b.seed(
            door,
            [Hero {
                name: "Ada".to_owned(),
                fought: false,
            }],
        );
        b.transition("Enter", (TakeEdge(door),), (lobby_1,), |hero: Hero| (hero,));
        b.transition("Climb", (TakeEdge(exit_1),), (lobby_2,), |hero: Hero| {
            (hero,)
        });
        b.transition("Leave", (TakeEdge(exit_2),), (outside,), |hero: Hero| {
            (hero,)
        });
        Ok(b.build()?)
    }
}

/// The `levels` entry, as PROTOCOL.md §11 names and describes it.
fn levels_entry() -> NetEntry {
    NetEntry::new("levels", levels::levels)
        .description("Heroes enter two levels through one port and leave through another")
        .hosting(Hosting::Manual)
}

/// The `levels` net alone, for the transcripts over it (PROTOCOL.md §11).
#[allow(dead_code)]
pub fn levels_registry() -> Vec<NetEntry> {
    vec![levels_entry()]
}

/// The protocol's conformance net `groups` (PROTOCOL.md §11, §5.1): letters
/// sorted in the group `Sorting` and packed in the group `Packing` into the
/// port `Tray` of the subnet place `Box`, whose subnet seals them in a
/// group of its own, `Sealing`, so that a transcript shows `groups` and
/// `group` in the topology of a net and of an inner net. Its names, places,
/// transitions, groups, template and seed are fixed by §11; the Python twin
/// is `tests/protocol_nets/groups.py`. Every body hands its letter on
/// unchanged, and none fails.
pub mod groups {
    use petritype::{Net, NetBuilder, Params, TakeEdge, Templates};
    use petritype_runner::LoadError;

    /// The template `Box`: `Tray -Seal-> Sealed`, both of strings and
    /// serializable, `Tray` its port, and `Tray` and `Seal` in its one
    /// group, `Sealing`. No parameters, no tokens.
    fn sealing_box(_: &Params) -> Result<Net, petritype::BuildError> {
        let mut b = NetBuilder::new();
        let sealing = b.group("Sealing");
        let tray = b.place::<String>("Tray");
        let sealed = b.place::<String>("Sealed");
        b.serializable(tray);
        b.serializable(sealed);
        b.port(tray);
        b.in_group(sealing, tray);
        b.transition("Seal", (TakeEdge(tray),), (sealed,), |letter: String| {
            (letter,)
        })
        .group(sealing);
        b.build()
    }

    /// `Box`, under its name, which builds the box's net and stamps it.
    pub fn templates() -> Templates {
        let mut templates = Templates::new();
        templates.register("Box", sealing_box);
        templates
    }

    /// The net as §11 seeds it: `Inbox` holding `"Letter"`, in no group;
    /// `Sorted`, empty, in `Sorting`; and `Box`, a subnet place in
    /// `Packing` holding a `Box`, inner first. `Sort`, in `Sorting`, takes
    /// the letter from `Inbox` into `Sorted`, and `Pack`, in `Packing`, from
    /// `Sorted` into the box's `Tray`.
    pub fn groups(_: &petritype_runner::Params) -> Result<Net, LoadError> {
        let templates = templates();
        let the_box = templates
            .build("Box", &Params::new())
            .map_err(|error| LoadError::Other(error.to_string()))?;
        let mut b = NetBuilder::new();
        b.templates(&templates);
        let sorting = b.group("Sorting");
        let packing = b.group("Packing");
        let inbox = b.place::<String>("Inbox");
        let sorted = b.place::<String>("Sorted");
        let box_place = b.subnet_place("Box", the_box);
        b.serializable(inbox);
        b.serializable(sorted);
        b.in_group(sorting, sorted);
        b.in_group(packing, box_place);
        let tray = b.port_of::<String>(box_place, "Tray");
        b.seed(inbox, ["Letter".to_owned()]);
        b.transition("Sort", (TakeEdge(inbox),), (sorted,), |letter: String| {
            (letter,)
        })
        .group(sorting);
        b.transition("Pack", (TakeEdge(sorted),), (tray,), |letter: String| {
            (letter,)
        })
        .group(packing);
        Ok(b.build()?)
    }
}

/// The `groups` entry, as PROTOCOL.md §11 names and describes it.
fn groups_entry() -> NetEntry {
    NetEntry::new("groups", groups::groups)
        .description(
            "Letters are sorted in one group and packed in another, into a box that seals them in a group of its own",
        )
        .hosting(Hosting::Manual)
}

/// The `groups` net alone, for the transcripts over it (PROTOCOL.md §11).
#[allow(dead_code)]
pub fn groups_registry() -> Vec<NetEntry> {
    vec![groups_entry()]
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
        stepping_rules_entry(),
        levels_entry(),
        groups_entry(),
    ]
}
