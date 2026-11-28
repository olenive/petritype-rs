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
//! written here. The conformance nets `levels` (§11, §12.7), `groups` (§11,
//! §5.1), `workshop` (§11, §6.3), `cellar` (§11, §12.7, §13), `harbour`
//! (§11, §12.1, §12.6) and `odd-names` (§11, §2) are written here too, each
//! carried alone by its own registry ([`levels_registry`],
//! [`groups_registry`], [`workshop_registry`], [`cellar_registry`],
//! [`harbour_registry`], [`odd_names_registry`]) for the transcripts over it
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
/// parcel over 100 kg. Two parcels arrive with the net. `Receiving` and
/// `Inspection` are its ports, where parcels come in from outside: the net
/// is closed, as every net not declared open is, so a host's `inject` lands
/// only there.
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
    b.port(receiving);
    b.port(inspection);
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
/// injected at `Inbox`, its port, so it is hosted 24/7 and idles in between.
pub fn parcel_intake(_: &petritype_runner::Params) -> Result<Net, LoadError> {
    let mut b = NetBuilder::new();
    let inbox = b.place::<Parcel>("Inbox");
    let shelf = b.place::<Parcel>("Shelf");
    b.serializable(inbox);
    b.port(inbox);
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
    /// Dispatched`, seeded with eight parcels. `Receiving` is its port,
    /// where parcels arrive from outside. `Dispatch Truck` takes the whole
    /// platform, sends slot 0 away and writes slot 1 back to the platform;
    /// every fourth parcel misses its first truck.
    pub fn parcel_distribution(_: &petritype_runner::Params) -> Result<Net, LoadError> {
        let mut b = NetBuilder::new();
        let receiving = b.place::<Parcel>("Receiving");
        let sorting = b.place::<Parcel>("Sorting");
        let staging = b.place::<Parcel>("Staging Area");
        let dispatched = b.place::<Parcel>("Dispatched");
        for place in [receiving, sorting, staging, dispatched] {
            b.serializable(place);
        }
        b.port(receiving);
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

/// The shop as §11 seeds it: empty everywhere, `Orders` and `Promotions` the
/// places a host may `inject` a string into, its ports. No parameters of
/// either kind.
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
/// waiting, `Waiting` the one place a host may `inject` a car's name into,
/// its port. No parameters of either kind. The net never quiesces: `Change`
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
    /// lobby, and `Leave` from level 2's exit hall to `Outside`. The world
    /// marks no port and is closed, as every net not declared open is, so a
    /// host's `inject` by the pair of a level's port on the world is
    /// refused (`closed_net`).
    pub fn levels(_: &petritype_runner::Params) -> Result<Net, LoadError> {
        world(false)
    }

    /// The world of [`levels`] declared open, which the conformance net is
    /// not: for this runner's own tests, which put a hero into a level's
    /// port by the pair on the world. It is no conformance net, and the
    /// example binary does not carry it.
    #[allow(dead_code)]
    pub fn open_levels(_: &petritype_runner::Params) -> Result<Net, LoadError> {
        world(true)
    }

    /// The world, declared open when `open` is.
    fn world(open: bool) -> Result<Net, LoadError> {
        let templates = templates();
        let level = || {
            templates
                .build("Level", &Params::new())
                .map_err(|error| LoadError::Other(error.to_string()))
        };
        let mut b = NetBuilder::new();
        b.templates(&templates);
        if open {
            b.open();
        }
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

/// The protocol's conformance net `workshop` (PROTOCOL.md §11, §6.3,
/// §12.7): pots glazed on a bench and unloaded from a kiln through its
/// port, where a cracked pot fails the firing that holds it, so that a
/// transcript shows a failed firing that loses its token and one that gives
/// it back, each also for a firing that took its token from a port place.
/// Its names, places, transitions, template, load parameter and seed are
/// fixed by §11; the Python twin is `tests/protocol_nets/workshop.py`.
pub mod workshop {
    use std::fmt;

    use petritype::{Net, NetBuilder, Params, TakeEdge, Templates};
    use petritype_runner::LoadError;
    use serde::Deserialize;

    /// Why a firing failed: the pot it held, whose text begins with
    /// `"Cracked"`, broke.
    #[derive(Debug)]
    pub struct Cracked(pub String);

    impl fmt::Display for Cracked {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{} broke", self.0)
        }
    }

    impl std::error::Error for Cracked {}

    /// Both transitions' body: a cracked pot fails the firing, and any
    /// other moves on unchanged.
    fn handle(pot: String) -> Result<(String,), Cracked> {
        if pot.starts_with("Cracked") {
            Err(Cracked(pot))
        } else {
            Ok((pot,))
        }
    }

    /// The template `Kiln`: one place, `Hatch`, its port, of strings and
    /// serializable, and no transition. No parameters, no tokens.
    fn kiln(_: &Params) -> Result<Net, petritype::BuildError> {
        let mut b = NetBuilder::new();
        let hatch = b.place::<String>("Hatch");
        b.serializable(hatch);
        b.port(hatch);
        b.build()
    }

    /// `Kiln`, under its name, which builds the kiln's net and stamps it.
    pub fn templates() -> Templates {
        let mut templates = Templates::new();
        templates.register("Kiln", kiln);
        templates
    }

    /// `workshop`'s load parameter: `restore`, whether a failed firing puts
    /// back what it took, `false` unless given.
    #[derive(Deserialize)]
    pub struct WorkshopParams {
        #[serde(default)]
        pub restore: bool,
    }

    /// The net as §11 seeds it: `Bench`, `Kiln` (a subnet place holding a
    /// `Kiln`, inner first) and `Shelf`, every place empty. `Glaze` takes a
    /// pot from the bench and `Unload` one from the kiln's `Hatch`, each
    /// putting it on the shelf. The Python library restores tokens on
    /// failure for a whole net and the core for each transition, so both
    /// transitions take `restore`'s value.
    ///
    /// `Bench` is the workshop's port, where pots come in, and the workshop
    /// is declared open, so that a transcript's `inject` addressed to the
    /// kiln by path lands at `Hatch`, the kiln's port: a closed net shields
    /// the nets inside it from the program and the runner (PROTOCOL.md §11;
    /// the Python library since 2026-11-27, this one since 2026-11-28).
    pub fn workshop(params: WorkshopParams) -> Result<Net, LoadError> {
        let templates = templates();
        let the_kiln = templates
            .build("Kiln", &Params::new())
            .map_err(|error| LoadError::Other(error.to_string()))?;
        let mut b = NetBuilder::new();
        b.templates(&templates);
        let bench = b.place::<String>("Bench");
        let kiln_place = b.subnet_place("Kiln", the_kiln);
        let shelf = b.place::<String>("Shelf");
        b.serializable(bench);
        b.serializable(shelf);
        b.port(bench);
        b.open();
        let hatch = b.port_of::<String>(kiln_place, "Hatch");
        for (name, from) in [("Glaze", bench), ("Unload", hatch)] {
            let mut transition = b.transition(name, (TakeEdge(from),), (shelf,), handle);
            if params.restore {
                transition.restore_tokens_on_failure();
            }
        }
        Ok(b.build()?)
    }
}

/// The `workshop` entry, as PROTOCOL.md §11 names and describes it, with
/// its one load parameter, `restore`.
fn workshop_entry() -> NetEntry {
    NetEntry::typed("workshop", workshop::workshop)
        .description(
            "Pots are glazed on a bench and unloaded from a kiln, and a cracked pot fails the firing that holds it",
        )
        .param(ParamSpec::new("restore", "bool").default(false))
        .hosting(Hosting::Manual)
}

/// The `workshop` net alone, for the transcripts over it (PROTOCOL.md §11).
#[allow(dead_code)]
pub fn workshop_registry() -> Vec<NetEntry> {
    vec![workshop_entry()]
}

/// The protocol's conformance net `cellar` (PROTOCOL.md §11, §12.7, §13):
/// bottles on a rack in a cellar, a subnet place whose one port the net
/// around it reaches from outside, so that a transcript shows a port's
/// tokens read, changed and filtered by that net. Its names, places,
/// transition, key, template and seed are fixed by §11; the Python twin is
/// `tests/protocol_nets/cellar.py`.
pub mod cellar {
    use petritype::{KeyValue, MutateEdge, Net, NetBuilder, Params, Templates};
    use petritype_runner::LoadError;
    use serde::{Deserialize, Serialize};

    /// A bottle, and how many years it has aged.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct Bottle {
        pub name: String,
        pub years: i64,
    }

    /// The one key every place of bottles declares.
    const KEY_NAMES: [&str; 1] = ["years"];

    /// A bottle's key: its `years`.
    fn keys_of(bottle: &Bottle) -> [KeyValue; 1] {
        [KeyValue::Int(bottle.years)]
    }

    /// The template `Cellar`: one place, `Rack`, its port, holding Rioja at
    /// 0 years, and no transition. No parameters.
    fn cellar_net(_: &Params) -> Result<Net, petritype::BuildError> {
        let mut b = NetBuilder::new();
        let rack = b.place::<Bottle>("Rack");
        b.serializable(rack);
        b.keys(rack, KEY_NAMES, keys_of);
        b.port(rack);
        b.seed(
            rack,
            [Bottle {
                name: "Rioja".to_owned(),
                years: 0,
            }],
        );
        b.build()
    }

    /// `Cellar`, under its name, which builds the cellar's net and stamps
    /// it.
    pub fn templates() -> Templates {
        let mut templates = Templates::new();
        templates.register("Cellar", cellar_net);
        templates
    }

    /// The net as §11 seeds it: `Table`, holding Claret at 4 years, and
    /// `Cellar`, a subnet place holding a `Cellar`, inner first. `Age`
    /// mutates the bottle at the front of the cellar's `Rack`, one year
    /// older, and has no output, so it is fireable whenever the rack holds
    /// a bottle.
    ///
    /// The house is declared open, so that a transcript's `inject` by the
    /// pair on the house brings a bottle into the rack: a closed net refuses
    /// a write from outside at the pair of a subnet's port, which is never
    /// one of its own ports. The cellar is closed and takes the bottle
    /// because `Rack` is its port.
    pub fn cellar(_: &petritype_runner::Params) -> Result<Net, LoadError> {
        let templates = templates();
        let the_cellar = templates
            .build("Cellar", &Params::new())
            .map_err(|error| LoadError::Other(error.to_string()))?;
        let mut b = NetBuilder::new();
        b.templates(&templates);
        let table = b.place::<Bottle>("Table");
        let cellar_place = b.subnet_place("Cellar", the_cellar);
        b.serializable(table);
        b.keys(table, KEY_NAMES, keys_of);
        b.open();
        let rack = b.port_of::<Bottle>(cellar_place, "Rack");
        b.seed(
            table,
            [Bottle {
                name: "Claret".to_owned(),
                years: 4,
            }],
        );
        b.transition("Age", (MutateEdge(rack),), (), |bottle: &mut Bottle| {
            bottle.years += 1;
        });
        Ok(b.build()?)
    }
}

/// The `cellar` entry, as PROTOCOL.md §11 names and describes it.
fn cellar_entry() -> NetEntry {
    NetEntry::new("cellar", cellar::cellar)
        .description("Bottles age on a rack in a cellar that the house reaches through its port")
        .hosting(Hosting::Manual)
}

/// The `cellar` net alone, for the transcripts over it (PROTOCOL.md §11).
#[allow(dead_code)]
pub fn cellar_registry() -> Vec<NetEntry> {
    vec![cellar_entry()]
}

/// The protocol's conformance net `harbour` (PROTOCOL.md §11, §12.1,
/// §12.6, §12.7): ships that are nets of their own, each holding a subnet
/// place, so that a transcript shows a subnet inside a net that a token
/// carries, its path through the token's step, its path as the token moves,
/// and its end when the token is consumed. Its names, places, transitions,
/// templates and seed are fixed by §11; the Python twin is
/// `tests/protocol_nets/harbour.py`. Every body hands its token on
/// unchanged, and none fails.
pub mod harbour {
    use std::error::Error;

    use petritype::{Net, NetBuilder, Params, TakeEdge, Templates};
    use petritype_runner::LoadError;

    /// The template `Hold`: `Hatch -Settle-> Bay`, both of strings,
    /// serializable and empty, `Hatch` its port. No parameters.
    fn hold(_: &Params) -> Result<Net, petritype::BuildError> {
        let mut b = NetBuilder::new();
        let hatch = b.place::<String>("Hatch");
        let bay = b.place::<String>("Bay");
        b.serializable(hatch);
        b.serializable(bay);
        b.port(hatch);
        b.transition("Settle", (TakeEdge(hatch),), (bay,), |cargo: String| {
            (cargo,)
        });
        b.build()
    }

    /// The template `Ship`: `Deck`, of strings and serializable, holding
    /// `"Crate"`, and `Hold`, a subnet place holding a `Hold`, inner first;
    /// `Stow` takes the crate from the deck into the hold's `Hatch`. No
    /// parameters.
    fn ship(holds: &Templates) -> Result<Net, Box<dyn Error + Send + Sync>> {
        let mut b = NetBuilder::new();
        b.templates(holds);
        let deck = b.place::<String>("Deck");
        let hold_place = b.subnet_place("Hold", holds.build("Hold", &Params::new())?);
        b.serializable(deck);
        let hatch = b.port_of::<String>(hold_place, "Hatch");
        b.seed(deck, ["Crate".to_owned()]);
        b.transition(
            "Stow",
            (TakeEdge(deck),),
            (hatch,),
            |cargo: String| (cargo,),
        );
        Ok(b.build()?)
    }

    /// `Hold` and `Ship`, each under its name, which build and stamp the
    /// ship's net and its hold's, and which the places of ships restore
    /// from.
    pub fn templates() -> Templates {
        let mut holds = Templates::new();
        holds.register("Hold", hold);
        let mut templates = holds.clone();
        templates.register("Ship", move |_: &Params| ship(&holds));
        templates
    }

    /// The net as §11 seeds it: `Quay`, a place of nets holding one ship,
    /// and `Sea`, a place of nets holding none, both inner first. `Sail`
    /// takes the ship from the quay and puts it to sea, its nets going with
    /// it, and `Scrap` takes it from the sea and puts nothing back. A token
    /// of a place of nets is a net, so its one inner net is named `""`.
    pub fn harbour(_: &petritype_runner::Params) -> Result<Net, LoadError> {
        let templates = templates();
        let the_ship = templates
            .build("Ship", &Params::new())
            .map_err(|error| LoadError::Other(error.to_string()))?;
        let mut b = NetBuilder::new();
        b.templates(&templates);
        let quay = b.place_nested::<Net>("Quay");
        let sea = b.place_nested::<Net>("Sea");
        b.serializable(quay);
        b.serializable(sea);
        b.seed(quay, [the_ship]);
        b.transition("Sail", (TakeEdge(quay),), (sea,), |ship: Net| (ship,));
        b.transition("Scrap", (TakeEdge(sea),), (), |_: Net| {});
        Ok(b.build()?)
    }
}

/// The `harbour` entry, as PROTOCOL.md §11 names and describes it.
fn harbour_entry() -> NetEntry {
    NetEntry::new("harbour", harbour::harbour)
        .description("A ship stows its crate in its hold, sails and is scrapped")
        .hosting(Hosting::Manual)
}

/// The `harbour` net alone, for the transcripts over it (PROTOCOL.md §11).
#[allow(dead_code)]
pub fn harbour_registry() -> Vec<NetEntry> {
    vec![harbour_entry()]
}

/// The protocol's conformance net `odd-names` (PROTOCOL.md §2, §11, §12.1):
/// places, transitions, a port, a subnet place and a template whose names
/// each hold a slash, a hash, a space and a double quote, so that a
/// transcript shows such names addressed by path and carried in requests,
/// results, notifications and the log, compared exactly as given. Its
/// names, places, transitions, template and seed are fixed by §11; the
/// Python twin is `tests/protocol_nets/odd_names.py`. Every body hands its
/// memo on unchanged, and none fails.
pub mod odd_names {
    use petritype::{Net, NetBuilder, Params, TakeEdge, Templates};
    use petritype_runner::LoadError;

    /// The outer net's place that holds the memo.
    pub const INBOX: &str = r#"Inbox #1 / "new""#;
    /// The subnet place.
    pub const CABINET: &str = r#"Cabinet "C" #2/3"#;
    /// The subnet's port, whose apostrophe is U+0027.
    pub const TRAY: &str = r#"Clerk's tray #4 / "in""#;
    /// The subnet's other place.
    pub const FOLDER: &str = r#"Folder "D" #5/6"#;
    /// The outer net's transition, from the inbox into the port.
    pub const FILE_IT: &str = r#"File it #1 / "now""#;
    /// The subnet's transition, from the port into the folder.
    pub const SORT: &str = r#"Sort #2 / "A-Z""#;
    /// The template that builds the subnet.
    pub const TEMPLATE: &str = r#"Cabinet #9 / "steel""#;

    /// The template [`TEMPLATE`]: the port [`TRAY`] and then [`FOLDER`],
    /// both of strings, serializable and empty, with [`SORT`] from one to
    /// the other. No parameters.
    fn cabinet(_: &Params) -> Result<Net, petritype::BuildError> {
        let mut b = NetBuilder::new();
        let tray = b.place::<String>(TRAY);
        let folder = b.place::<String>(FOLDER);
        b.serializable(tray);
        b.serializable(folder);
        b.port(tray);
        b.transition(SORT, (TakeEdge(tray),), (folder,), |memo: String| (memo,));
        b.build()
    }

    /// The cabinet's template, under its name, which builds the subnet and
    /// stamps it.
    pub fn templates() -> Templates {
        let mut templates = Templates::new();
        templates.register(TEMPLATE, cabinet);
        templates
    }

    /// The net as §11 seeds it: [`INBOX`], holding `"Memo"`, and
    /// [`CABINET`], a subnet place holding a cabinet, inner first;
    /// [`FILE_IT`] takes the memo from the inbox into the cabinet's
    /// [`TRAY`]. The inbox is the net's port, where a transcript `inject`s
    /// a note.
    pub fn odd_names(_: &petritype_runner::Params) -> Result<Net, LoadError> {
        let templates = templates();
        let the_cabinet = templates
            .build(TEMPLATE, &Params::new())
            .map_err(|error| LoadError::Other(error.to_string()))?;
        let mut b = NetBuilder::new();
        b.templates(&templates);
        let inbox = b.place::<String>(INBOX);
        let cabinet_place = b.subnet_place(CABINET, the_cabinet);
        b.serializable(inbox);
        b.port(inbox);
        let tray = b.port_of::<String>(cabinet_place, TRAY);
        b.seed(inbox, ["Memo".to_owned()]);
        b.transition(FILE_IT, (TakeEdge(inbox),), (tray,), |memo: String| (memo,));
        Ok(b.build()?)
    }
}

/// The `odd-names` entry, as PROTOCOL.md §11 names and describes it.
fn odd_names_entry() -> NetEntry {
    NetEntry::new("odd-names", odd_names::odd_names)
        .description("Every name holds a slash, a hash, a space and a double quote")
        .hosting(Hosting::Manual)
}

/// The `odd-names` net alone, for the transcripts over it (PROTOCOL.md
/// §11).
#[allow(dead_code)]
pub fn odd_names_registry() -> Vec<NetEntry> {
    vec![odd_names_entry()]
}

/// The protocol's conformance net `couriers` (PROTOCOL.md §11, §13.5):
/// orders loaded onto the courier each names, and calls that send the
/// courier they name out, so that a transcript shows a keyed edge that
/// chooses its token by another input of the firing, in both forms, a
/// source token with no match passed over, and a failed firing putting both
/// tokens back. Its names, places, keys, transitions and seed are fixed by
/// §11; the Python twin is `tests/protocol_nets/couriers.py`.
pub mod couriers {
    use std::fmt;

    use petritype::{Filter, KeyValue, MutateWhereEdge, Net, NetBuilder, TakeEdge, TakeWhereEdge};
    use petritype_runner::LoadError;
    use serde::{Deserialize, Serialize};

    /// A courier, and how many parcels it carries.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct Courier {
        pub name: String,
        pub parcels: i64,
    }

    /// An order for an item, naming the courier it is for.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct Order {
        pub item: String,
        pub courier: String,
    }

    /// A call for the courier it names.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct Call {
        pub courier: String,
    }

    /// Why `Dispatch` failed: the courier it called carries nothing.
    #[derive(Debug)]
    pub struct NothingToCarry(pub String);

    impl fmt::Display for NothingToCarry {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{} has nothing to carry", self.0)
        }
    }

    impl std::error::Error for NothingToCarry {}

    fn courier(name: &str) -> Courier {
        Courier {
            name: name.to_owned(),
            parcels: 0,
        }
    }

    fn order(item: &str, courier: &str) -> Order {
        Order {
            item: item.to_owned(),
            courier: courier.to_owned(),
        }
    }

    /// The net as §11 seeds it: `Couriers` (keyed by `name`) holding Ada,
    /// Ben and Dot, none carrying anything; `Orders` (keyed by `courier`)
    /// holding a lamp for Cleo, a book for Ben and a kite for Ada; `Calls`
    /// (keyed by `courier`), `Loaded` and `On Road` empty. `Load` takes an
    /// order and hands its body the courier it names, which carries one
    /// parcel more, the order going on to `Loaded`; `Dispatch` takes a call
    /// and the courier it names, which goes on the road, or fails the
    /// firing when it carries nothing. Both restore their inputs on
    /// failure, as the Python net does for the whole net. `Couriers` and
    /// `Calls` are its ports, where a transcript `inject`s a courier and the
    /// calls.
    pub fn couriers(_: &petritype_runner::Params) -> Result<Net, LoadError> {
        let mut b = NetBuilder::new();
        let couriers = b.place::<Courier>("Couriers");
        let orders = b.place::<Order>("Orders");
        let calls = b.place::<Call>("Calls");
        let loaded = b.place::<Order>("Loaded");
        let on_road = b.place::<Courier>("On Road");
        b.serializable(couriers);
        b.serializable(orders);
        b.serializable(calls);
        b.serializable(loaded);
        b.serializable(on_road);
        b.port(couriers);
        b.port(calls);
        b.keys(couriers, ["name"], |c: &Courier| {
            [KeyValue::from(c.name.as_str())]
        });
        b.keys(orders, ["courier"], |o: &Order| {
            [KeyValue::from(o.courier.as_str())]
        });
        b.keys(calls, ["courier"], |c: &Call| {
            [KeyValue::from(c.courier.as_str())]
        });
        b.seed(couriers, [courier("Ada"), courier("Ben"), courier("Dot")]);
        b.seed(
            orders,
            [
                order("Lamp", "Cleo"),
                order("Book", "Ben"),
                order("Kite", "Ada"),
            ],
        );
        b.transition(
            "Load",
            (
                TakeEdge(orders),
                MutateWhereEdge(
                    couriers,
                    Filter::new().key_is_input("name", orders, "courier"),
                ),
            ),
            (loaded,),
            |order: Order, courier: &mut Courier| {
                courier.parcels += 1;
                (order,)
            },
        )
        .restore_tokens_on_failure();
        b.transition(
            "Dispatch",
            (
                TakeEdge(calls),
                TakeWhereEdge(
                    couriers,
                    Filter::new().key_is_input("name", calls, "courier"),
                ),
            ),
            (on_road,),
            |_: Call, courier: Courier| -> Result<(Courier,), NothingToCarry> {
                if courier.parcels == 0 {
                    Err(NothingToCarry(courier.name))
                } else {
                    Ok((courier,))
                }
            },
        )
        .restore_tokens_on_failure();
        Ok(b.build()?)
    }
}

/// The protocol's conformance net `roll-call` (PROTOCOL.md §11, §5.1): a
/// roll call that reads the squads in the barracks, tokens that are each a
/// net, without taking or copying them, through each of the three read
/// edges, so that a transcript shows them on the wire, firings that name no
/// squad, and the squads' ids and their nets' ids unchanged. Its names,
/// places, key, template, transitions, roll lines and seed are fixed by
/// §11; the Python twin is `tests/protocol_nets/roll_call.py`.
pub mod roll_call {
    use petritype::{
        Filter, KeyValue, Net, NetBuilder, Params, ReadAllEdge, ReadEdge, ReadWhereEdge, TakeEdge,
        Templates,
    };
    use petritype_runner::LoadError;
    use serde::{Deserialize, Serialize};

    /// An inspection of the squad a leader leads.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct Inspection {
        pub leader: String,
    }

    /// The template `Squad`: one place, `Members`, of strings and
    /// serializable, holding the names of the template parameter `members`
    /// in order (no one without it, as a saved squad is rebuilt before its
    /// marking is loaded), and no transition. No net parameters, so nothing
    /// of `members` goes on the wire.
    ///
    /// Since 2026-11-28 a squad starts with its members: it was built empty
    /// and filled from outside before, which a squad, closed as every net
    /// not declared open is, refuses.
    fn squad(params: &Params) -> Result<Net, petritype::BuildError> {
        let names = params
            .get("members")
            .and_then(serde_json::Value::as_array)
            .map(|names| {
                names
                    .iter()
                    .filter_map(|name| name.as_str().map(str::to_owned))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut b = NetBuilder::new();
        let members = b.place::<String>("Members");
        b.serializable(members);
        b.seed(members, names);
        b.build()
    }

    /// `Squad`, under its name, which builds a squad's net and stamps it.
    pub fn templates() -> Templates {
        let mut templates = Templates::new();
        templates.register("Squad", squad);
        templates
    }

    /// The names in a squad's `Members`, front first.
    fn members(squad: &Net) -> Vec<String> {
        let Some(place) = squad.place_named("Members") else {
            return Vec::new();
        };
        squad
            .tokens(place)
            .filter_map(|(id, _)| squad.token::<String>(place, id).cloned())
            .collect()
    }

    /// A squad as a roll line writes it: its leader, the first name in its
    /// `Members` (`""` for none), and its size, `Ada 2`.
    fn line(squad: &Net) -> String {
        let members = members(squad);
        let leader = members.first().map_or("", String::as_str);
        format!("{leader} {}", members.len())
    }

    /// A squad of these members, built from the template with them.
    fn squad_of(templates: &Templates, names: &[&str]) -> Result<Net, LoadError> {
        let mut params = Params::new();
        params.insert("members".into(), serde_json::json!(names));
        templates
            .build("Squad", &params)
            .map_err(|error| LoadError::Other(error.to_string()))
    }

    /// The net as §11 seeds it: `Barracks`, a place of nets keyed by
    /// `leader`, holding a squad of Ada and Ben and a squad of Cy; `Calls`,
    /// `Checks`, `Inspections` (keyed by `leader`) and `Roll`, empty, the
    /// first three its ports, where a transcript `inject`s requests. `Call
    /// the Roll` reads every squad, `Check the Front` the front one, and
    /// `Inspect` the one an inspection names, each taking its request and
    /// writing one roll line.
    pub fn roll_call(_: &petritype_runner::Params) -> Result<Net, LoadError> {
        let templates = templates();
        let squads = [
            squad_of(&templates, &["Ada", "Ben"])?,
            squad_of(&templates, &["Cy"])?,
        ];
        let mut b = NetBuilder::new();
        b.templates(&templates);
        let barracks = b.place_nested::<Net>("Barracks");
        let calls = b.place::<String>("Calls");
        let checks = b.place::<String>("Checks");
        let inspections = b.place::<Inspection>("Inspections");
        let roll = b.place::<String>("Roll");
        b.serializable(barracks);
        b.serializable(calls);
        b.serializable(checks);
        b.serializable(inspections);
        b.serializable(roll);
        b.port(calls);
        b.port(checks);
        b.port(inspections);
        b.keys(barracks, ["leader"], |squad: &Net| {
            [KeyValue::from(
                members(squad).first().cloned().unwrap_or_default(),
            )]
        });
        b.keys(inspections, ["leader"], |inspection: &Inspection| {
            [KeyValue::from(inspection.leader.as_str())]
        });
        b.seed(barracks, squads);
        b.transition(
            "Call the Roll",
            (TakeEdge(calls), ReadAllEdge(barracks)),
            (roll,),
            |caller: String, squads: Vec<&Net>| {
                let lines: Vec<String> = squads.iter().map(|squad| line(squad)).collect();
                if lines.is_empty() {
                    (format!("{caller}: nobody"),)
                } else {
                    (format!("{caller}: {}", lines.join(", ")),)
                }
            },
        );
        b.transition(
            "Check the Front",
            (TakeEdge(checks), ReadEdge(barracks)),
            (roll,),
            |checker: String, squad: &Net| (format!("{checker}: {}", line(squad)),),
        );
        b.transition(
            "Inspect",
            (
                TakeEdge(inspections),
                ReadWhereEdge(
                    barracks,
                    Filter::new().key_is_input("leader", inspections, "leader"),
                ),
            ),
            (roll,),
            |_: Inspection, squad: &Net| (format!("Inspected {}", line(squad)),),
        );
        Ok(b.build()?)
    }
}

/// The protocol's conformance net `trays` (PROTOCOL.md §11, §5.1, §12): a
/// unit, a token that is a net with two ports, handed an order through one
/// and its shots collected from the other by the net around it, each outer
/// edge naming the port it reaches. Its names, places, template,
/// transitions, guard, shot and seed are fixed by §11; the Python twin is
/// `tests/protocol_nets/trays.py`.
pub mod trays {
    use petritype::{MutateEdge, Net, NetBuilder, Params, TakeEdge, Templates};
    use petritype_runner::LoadError;

    /// The template `Unit`: `Orders In` and `Shots Out`, both of strings,
    /// serializable and empty, both ports; `Fire` takes the front order and
    /// produces `"Shot: <order>"`. No parameters.
    fn unit(_: &Params) -> Result<Net, petritype::BuildError> {
        let mut b = NetBuilder::new();
        let orders_in = b.place::<String>("Orders In");
        let shots_out = b.place::<String>("Shots Out");
        b.serializable(orders_in);
        b.serializable(shots_out);
        b.port(orders_in);
        b.port(shots_out);
        b.transition(
            "Fire",
            (TakeEdge(orders_in),),
            (shots_out,),
            |order: String| (format!("Shot: {order}"),),
        );
        b.build()
    }

    /// `Unit`, under its name, which builds a unit's net and stamps it.
    pub fn templates() -> Templates {
        let mut templates = Templates::new();
        templates.register("Unit", unit);
        templates
    }

    /// Whether the front unit of `Field` has a shot in `Shots Out`:
    /// `Collect`'s guard.
    fn a_shot_waits(net: &Net) -> bool {
        let Some(field) = net.place_named("Field") else {
            return false;
        };
        net.tokens(field)
            .next()
            .and_then(|(token, _)| net.token::<Net>(field, token))
            .is_some_and(|unit| {
                unit.place_named("Shots Out")
                    .is_some_and(|tray| unit.count(tray) > 0)
            })
    }

    /// The net as §11 seeds it: `Orders` and `Reports` empty, and `Field`,
    /// a place of nets holding one unit, empty, inner first. `Hand Over`
    /// takes an order and puts it into the front unit's `Orders In`;
    /// `Collect` takes every shot out of the front unit's `Shots Out` into
    /// `Reports`, guarded on there being one. `Orders` is the net's port,
    /// where a transcript `inject`s the order.
    pub fn trays(_: &petritype_runner::Params) -> Result<Net, LoadError> {
        let templates = templates();
        let the_unit = templates
            .build("Unit", &Params::new())
            .map_err(|error| LoadError::Other(error.to_string()))?;
        let mut b = NetBuilder::new();
        b.templates(&templates);
        let orders = b.place::<String>("Orders");
        let field = b.place_nested::<Net>("Field");
        let reports = b.place::<String>("Reports");
        b.serializable(orders);
        b.serializable(field);
        b.serializable(reports);
        b.port(orders);
        b.seed(field, [the_unit]);
        b.transition(
            "Hand Over",
            (TakeEdge(orders), MutateEdge(field).reaching(["Orders In"])),
            (),
            |order: String, unit: &mut Net| {
                if let Some(tray) = unit.place_ref::<String>("Orders In") {
                    unit.put(tray, order);
                }
            },
        );
        b.transition(
            "Collect",
            (MutateEdge(field).reaching(["Shots Out"]),),
            (reports,),
            |unit: &mut Net| {
                let Some(tray) = unit.place_ref::<String>("Shots Out") else {
                    return (Vec::new(),);
                };
                let shots: Vec<(petritype::TokenId, String)> = unit
                    .tokens(tray.id())
                    .filter_map(|(id, _)| {
                        unit.token::<String>(tray.id(), id)
                            .map(|shot| (id, shot.clone()))
                    })
                    .collect();
                for (id, _) in &shots {
                    // The id was just read from the tray, so it is there.
                    let _ = unit.remove(tray.id(), *id);
                }
                (shots.into_iter().map(|(_, shot)| shot).collect::<Vec<_>>(),)
            },
        )
        .guard(a_shot_waits);
        Ok(b.build()?)
    }
}

/// The protocol's conformance net `post-office` (PROTOCOL.md §11, §12,
/// DESIGN §6.9): posts, tokens that are each a net, one closed, as every
/// net not declared open is, and one the same but declared open, so that a
/// transcript shows a body's write at a port landing, an `inject` by path
/// landing at a port, refused at a private place of the closed post, and
/// landing there in the open one, and last a body's write into a private
/// place failing its firing. Its names, places, templates, transitions and
/// seed follow §11; the Python twin is `tests/protocol_nets/post_office.py`.
///
/// Neither transition restores its inputs: a transition that consumes a
/// place of nets cannot, in this core, whose nets have no cloner
/// (`BuildError::RestoreOfNoClonePlace`), so the refused `Forge` loses the
/// forgery and the post it took, as any failure without a restore does.
pub mod post_office {
    use petritype::{MutateEdge, Net, NetBuilder, NetError, Params, TakeEdge, Templates};
    use petritype_runner::LoadError;

    /// A post: `Inbox` and `Ledger`, both of strings, serializable and
    /// empty, `Inbox` a port, and no transitions; declared open when
    /// `open` is.
    fn post(open: bool) -> Result<Net, petritype::BuildError> {
        let mut b = NetBuilder::new();
        let inbox = b.place::<String>("Inbox");
        let ledger = b.place::<String>("Ledger");
        b.serializable(inbox);
        b.serializable(ledger);
        b.port(inbox);
        if open {
            b.open();
        }
        b.build()
    }

    /// `Post`, closed as every net not declared open is, and `Open Post`,
    /// the same declared open, each under its name. No parameters.
    pub fn templates() -> Templates {
        let mut templates = Templates::new();
        templates.register("Post", |_: &Params| post(false));
        templates.register("Open Post", |_: &Params| post(true));
        templates
    }

    /// The net as §11 seeds it: `Letters` holding `"Dear Ada"`,
    /// `Forgeries` holding `"IOU"`, and `Posts`, a place of nets, inner
    /// first, holding a `Post` and then an `Open Post`, both empty. The net
    /// is declared open, so that each post is judged by its own rule: a
    /// closed net shields the nets inside it from the program and the
    /// runner (PROTOCOL.md §11; the Python library since 2026-11-27, this
    /// one since 2026-11-28). `Deliver` takes a letter and puts
    /// it into the front post's `Inbox`, the port its edge reaches; `Forge`
    /// takes a forgery and the front post, writes the forgery into the
    /// post's `Ledger` and returns the post to `Posts`, and the closed post
    /// refuses the write, failing the firing. Neither restores its inputs.
    pub fn post_office(_: &petritype_runner::Params) -> Result<Net, LoadError> {
        let templates = templates();
        let build = |name: &str| {
            templates
                .build(name, &Params::new())
                .map_err(|error| LoadError::Other(error.to_string()))
        };
        let posts_seed = [build("Post")?, build("Open Post")?];
        let mut b = NetBuilder::new();
        b.templates(&templates);
        let letters = b.place::<String>("Letters");
        let forgeries = b.place::<String>("Forgeries");
        let posts = b.place_nested::<Net>("Posts");
        b.serializable(letters);
        b.serializable(forgeries);
        b.serializable(posts);
        b.seed(letters, ["Dear Ada".to_owned()]);
        b.seed(forgeries, ["IOU".to_owned()]);
        b.seed(posts, posts_seed);
        b.open();
        b.transition(
            "Deliver",
            (TakeEdge(letters), MutateEdge(posts).reaching(["Inbox"])),
            (),
            |letter: String, post: &mut Net| -> Result<(), NetError> {
                if let Some(inbox) = post.place_ref::<String>("Inbox") {
                    post.try_put(inbox, letter)?;
                }
                Ok(())
            },
        );
        b.transition(
            "Forge",
            (TakeEdge(forgeries), TakeEdge(posts)),
            (posts,),
            |forgery: String, mut post: Net| -> Result<(Net,), NetError> {
                if let Some(ledger) = post.place_ref::<String>("Ledger") {
                    post.try_put(ledger, forgery)?;
                }
                Ok((post,))
            },
        );
        Ok(b.build()?)
    }
}

/// The `post-office` entry, as PROTOCOL.md §11 names and describes it.
fn post_office_entry() -> NetEntry {
    NetEntry::new("post-office", post_office::post_office)
        .description(
            "Letters are delivered to a post's inbox, and a forgery written into its ledger is refused",
        )
        .hosting(Hosting::Manual)
}

/// The `post-office` net alone, for the transcripts over it (PROTOCOL.md
/// §11).
#[allow(dead_code)]
pub fn post_office_registry() -> Vec<NetEntry> {
    vec![post_office_entry()]
}

/// The `trays` entry, as PROTOCOL.md §11 names and describes it.
fn trays_entry() -> NetEntry {
    NetEntry::new("trays", trays::trays)
        .description(
            "A unit is handed an order through one tray and its shots are collected from another",
        )
        .hosting(Hosting::Manual)
}

/// The `trays` net alone, for the transcripts over it (PROTOCOL.md §11).
#[allow(dead_code)]
pub fn trays_registry() -> Vec<NetEntry> {
    vec![trays_entry()]
}

/// The `roll-call` entry, as PROTOCOL.md §11 names and describes it.
fn roll_call_entry() -> NetEntry {
    NetEntry::new("roll-call", roll_call::roll_call)
        .description("A roll call reads the squads in the barracks without taking them")
        .hosting(Hosting::Manual)
}

/// The `roll-call` net alone, for the transcripts over it (PROTOCOL.md
/// §11).
#[allow(dead_code)]
pub fn roll_call_registry() -> Vec<NetEntry> {
    vec![roll_call_entry()]
}

/// The `couriers` entry, as PROTOCOL.md §11 names and describes it.
fn couriers_entry() -> NetEntry {
    NetEntry::new("couriers", couriers::couriers)
        .description(
            "Orders are loaded onto the courier they name, and a call sends that courier out",
        )
        .hosting(Hosting::Manual)
}

/// The `couriers` net alone, for the transcripts over it (PROTOCOL.md §11).
#[allow(dead_code)]
pub fn couriers_registry() -> Vec<NetEntry> {
    vec![couriers_entry()]
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
        workshop_entry(),
        cellar_entry(),
        harbour_entry(),
        odd_names_entry(),
        couriers_entry(),
        roll_call_entry(),
        trays_entry(),
        post_office_entry(),
    ]
}
