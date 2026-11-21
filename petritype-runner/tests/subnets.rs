//! Subnet places and port places on the wire (PROTOCOL.md §12.7, DESIGN
//! §19.3.17), `Owner` and `Host` driven directly with a fixed clock: what the
//! shared transcript `09_subnet_places.json` cannot show, chiefly the rule on
//! versions in each of its cases, keys and filters on a port, two depths, a
//! subnet inside a token's net and a token's net inside a port, the reads
//! that wait while a firing touches a port, and a hosted net's subnets.

use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use petritype::control::{Control, Pressed};
use petritype::{MutateEdge, Net, NetBuilder, Stepping, TakeEdge, Templates};
use petritype_runner::protocol::Request;
use petritype_runner::{Host, Input, LoadError, NetEntry, Outgoing, Owner, Params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[path = "../examples/parcel_runner/nets.rs"]
#[allow(dead_code)]
mod nets;

use nets::levels::Hero;

// ----- the harness -------------------------------------------------------------

fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(1_795_770_000_000)
}

/// An owner over some nets: ids `"1"`, `"2"`, ... in order, and every launch
/// landed at once unless `auto` is off, when it is held for [`H::land`].
struct H {
    owner: Owner,
    next: u64,
    auto: bool,
    held: Vec<petritype::Firing>,
}

impl H {
    fn over(nets: Vec<NetEntry>) -> H {
        H {
            owner: Owner::new(nets, Arc::new(Mutex::new(None)), Duration::from_secs(30)),
            next: 0,
            auto: true,
            held: Vec::new(),
        }
    }

    fn loaded(nets: Vec<NetEntry>, net: &str, params: Value) -> H {
        let mut h = H::over(nets);
        let lines = h.send("load", json!({"net": net, "params": params}));
        assert_eq!(
            lines.last().unwrap()["result"]["status"],
            "loaded",
            "{lines:#?}"
        );
        h
    }

    fn feed(&mut self, input: Input) -> Vec<Value> {
        let mut lines = Vec::new();
        for outgoing in self.owner.handle(input, now()) {
            match outgoing {
                Outgoing::Line(line) => lines.push(serde_json::from_str(&line).unwrap()),
                Outgoing::Launch(firing) if self.auto => {
                    lines.extend(self.feed(Input::Completed(firing.run())));
                }
                Outgoing::Launch(firing) => self.held.push(firing),
                Outgoing::Exit => {}
            }
        }
        // A run that fired inline asks for a tick to fire again.
        if self.owner.wants_tick() {
            lines.extend(self.feed(Input::Tick));
        }
        lines
    }

    fn send(&mut self, method: &str, params: Value) -> Vec<Value> {
        self.next += 1;
        self.feed(Input::Request(Request {
            id: self.next.to_string(),
            method: method.into(),
            params,
        }))
    }

    fn land(&mut self) -> Vec<Value> {
        let firing = self.held.remove(0);
        self.feed(Input::Completed(firing.run()))
    }

    fn result(&mut self, method: &str, params: Value) -> Value {
        let lines = self.send(method, params);
        let last = lines.last().cloned().expect("a response");
        assert!(last.get("result").is_some(), "{method}: {lines:#?}");
        last["result"].clone()
    }

    fn error(&mut self, method: &str, params: Value) -> String {
        let lines = self.send(method, params);
        let last = lines.last().expect("a response");
        last["error"]["code"]
            .as_str()
            .unwrap_or_else(|| panic!("{method} {lines:#?}"))
            .to_owned()
    }

    fn nested_id(&mut self, path: &Value) -> String {
        let state = self.result("get_state", json!({ "path": path }));
        state["nested_id"]
            .as_str()
            .expect("an inner net")
            .to_owned()
    }

    fn version(&mut self, path: &Value) -> u64 {
        let state = self.result("get_state", json!({ "path": path }));
        state["marking_version"].as_u64().expect("a version")
    }
}

/// The params of the loaded net's own notifications called `method`.
fn outer<'a>(lines: &'a [Value], method: &str) -> Vec<&'a Value> {
    lines
        .iter()
        .filter(|line| line["method"] == method)
        .map(|line| &line["params"])
        .collect()
}

/// `(wrapper, event)` params of each `nested` line, of one method.
fn inner<'a>(lines: &'a [Value], method: &str) -> Vec<(&'a Value, &'a Value)> {
    lines
        .iter()
        .filter(|line| line["method"] == "nested" && line["params"]["event"]["method"] == method)
        .map(|line| (&line["params"], &line["params"]["event"]["params"]))
        .collect()
}

/// Each line as `method`, `nested:method`, or `result` or `error`.
fn sequence(lines: &[Value]) -> Vec<String> {
    lines
        .iter()
        .map(|line| match line["method"].as_str() {
            Some("nested") => format!(
                "nested:{}",
                line["params"]["event"]["method"].as_str().unwrap()
            ),
            Some(method) => method.to_owned(),
            None if line.get("result").is_some() => "result".to_owned(),
            None => "error".to_owned(),
        })
        .collect()
}

fn l1() -> Value {
    json!([{"place": "Level 1"}])
}

fn l2() -> Value {
    json!([{"place": "Level 2"}])
}

fn hero(name: &str, fought: bool) -> Hero {
    Hero {
        name: name.to_owned(),
        fought,
    }
}

// ----- the nets ----------------------------------------------------------------

/// The conformance net `levels` (PROTOCOL.md §11), and the template it is
/// built from loaded alone.
fn levels() -> Vec<NetEntry> {
    let mut entries = nets::levels_registry();
    entries.push(NetEntry::new("lone-level", |_: &Params| {
        nets::levels::templates()
            .build("Level", &Params::new())
            .map_err(|error| LoadError::Other(error.to_string()))
    }));
    entries
}

fn loaded_levels() -> H {
    H::loaded(levels(), "levels", json!({}))
}

/// A knight, keyed by `name` and `role`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Knight {
    name: String,
    role: String,
}

fn knight(name: &str, role: &str) -> Knight {
    Knight {
        name: name.into(),
        role: role.into(),
    }
}

fn keyed(b: &mut NetBuilder, place: petritype::PlaceRef<Knight>) {
    b.serializable(place);
    b.keys(place, ["name", "role"], |k: &Knight| {
        [k.name.clone().into(), k.role.clone().into()]
    });
}

/// `keyed-world`: `Door` (Ada the knight, Bo the mage) `-Enter->` the
/// `Lobby` port of `Level`, whose ports are keyed; `Promote` mutates the
/// front knight of the lobby into a lord, through a mutate edge on the port,
/// each time it takes one of `Ticks`; the control `clear` empties the lobby
/// from the world.
fn keyed_world(_: &Params) -> Result<Net, LoadError> {
    let mut templates = Templates::new();
    templates.register("Keyed Level", |_: &Params| {
        let mut b = NetBuilder::new();
        let lobby = b.place::<Knight>("Lobby");
        let exit = b.place::<Knight>("Exit Hall");
        keyed(&mut b, lobby);
        keyed(&mut b, exit);
        b.port(lobby);
        b.port(exit);
        b.build()
    });
    let level = templates
        .build("Keyed Level", &Params::new())
        .map_err(|error| LoadError::Other(error.to_string()))?;
    let mut b = NetBuilder::new();
    let door = b.place::<Knight>("Door");
    keyed(&mut b, door);
    let ticks = b.place::<u32>("Ticks");
    let level = b.subnet_place("Level", level);
    let lobby = b.port_of::<Knight>(level, "Lobby");
    b.seed(door, [knight("Ada", "Knight"), knight("Bo", "Mage")]);
    b.seed(ticks, [1]);
    b.transition("Enter", (TakeEdge(door),), (lobby,), |k: Knight| (k,));
    b.transition(
        "Promote",
        (TakeEdge(ticks), MutateEdge(lobby)),
        (),
        |_: u32, k: &mut Knight| k.role = "Lord".into(),
    );
    b.control(Control::button(
        "clear",
        "Clear the lobby",
        move |net, _| {
            net.set_tokens(lobby, Vec::<Knight>::new());
            Ok(Pressed::Applied)
        },
    ));
    Ok(b.build()?)
}

/// `tower`: `Door` (Ada, Bo) `-Enter->` the `Lobby` port of `Floor`, whose
/// subnet has `Lobby` (a port), `Room` (a subnet place, its rule the load
/// parameter `room`, passive unless given) and `Hall`; `Gather` takes from
/// the floor's lobby and from the room's port `Den` at once, into the hall,
/// and `Go In` from the lobby into the den. The room's `Sit` takes from its
/// den into its `Seat`.
fn tower(params: &Params) -> Result<Net, LoadError> {
    let rule = match params.get("room").and_then(Value::as_str) {
        None | Some("passive") => Stepping::Passive,
        Some("inner_first") => Stepping::InnerFirst,
        Some(other) => return Err(LoadError::InvalidParams(format!("no rule {other:?}"))),
    };
    let mut templates = Templates::new();
    templates.register("Room", |_: &Params| {
        let mut b = NetBuilder::new();
        let den = b.place::<Hero>("Den");
        let seat = b.place::<Hero>("Seat");
        b.port(den);
        b.transition("Sit", (TakeEdge(den),), (seat,), |h: Hero| (h,));
        b.build()
    });
    let room = templates
        .build("Room", &Params::new())
        .map_err(|error| LoadError::Other(error.to_string()))?;
    let mut b = NetBuilder::new();
    let lobby = b.place::<Hero>("Lobby");
    let room = b.subnet_place("Room", room);
    let hall = b.place::<Hero>("Hall");
    let den = b.port_of::<Hero>(room, "Den");
    b.port(lobby);
    b.stepping(room, rule);
    b.transition(
        "Gather",
        (TakeEdge(lobby), TakeEdge(den)),
        (hall,),
        |h: Hero, _: Hero| (h,),
    );
    b.transition("Go In", (TakeEdge(lobby),), (den,), |h: Hero| (h,));
    let floor = b.build()?;
    let mut b = NetBuilder::new();
    let door = b.place::<Hero>("Door");
    let floor = b.subnet_place("Floor", floor);
    let lobby = b.port_of::<Hero>(floor, "Lobby");
    b.seed(door, [hero("Ada", false), hero("Bo", false)]);
    b.transition("Enter", (TakeEdge(door),), (lobby,), |h: Hero| (h,));
    Ok(b.build()?)
}

fn floor_path() -> Value {
    json!([{"place": "Floor"}])
}

fn room_path() -> Value {
    json!([{"place": "Floor"}, {"place": "Room"}])
}

/// `harbour`: a ship, a token's net built by the template `Ship`, whose
/// `Load` takes `"crate"` from its `Deck` into the `Bay` port of its `Hold`,
/// a subnet built by `Hold`, which `Stow`s it in its `Deep`; the world's
/// `Scrap` takes the ship for scrap.
fn harbour(_: &Params) -> Result<Net, LoadError> {
    let mut holds = Templates::new();
    holds.register("Hold", |_: &Params| {
        let mut b = NetBuilder::new();
        let bay = b.place::<String>("Bay");
        let deep = b.place::<String>("Deep");
        b.serializable(bay);
        b.serializable(deep);
        b.port(bay);
        b.transition("Stow", (TakeEdge(bay),), (deep,), |c: String| (c,));
        b.build()
    });
    let mut templates = holds.clone();
    templates.register(
        "Ship",
        move |_: &Params| -> Result<Net, Box<dyn Error + Send + Sync>> {
            let mut b = NetBuilder::new();
            b.templates(&holds);
            let deck = b.place::<String>("Deck");
            b.serializable(deck);
            let hold = b.subnet_place("Hold", holds.build("Hold", &Params::new())?);
            let bay = b.port_of::<String>(hold, "Bay");
            b.seed(deck, ["crate".to_owned()]);
            b.transition("Load", (TakeEdge(deck),), (bay,), |c: String| (c,));
            Ok(b.build()?)
        },
    );
    let ship = templates
        .build("Ship", &Params::new())
        .map_err(|error| LoadError::Other(error.to_string()))?;
    let mut b = NetBuilder::new();
    b.templates(&templates);
    let harbour = b.place_nested::<Net>("Harbour");
    let scrapyard = b.place::<String>("Scrapyard");
    b.serializable(harbour);
    b.seed(harbour, [ship]);
    b.transition("Scrap", (TakeEdge(harbour),), (scrapyard,), |_: Net| {
        ("scrap".to_owned(),)
    });
    Ok(b.build()?)
}

/// `yard`: a worker, a bare net with `Todo [7] -Work-> Done`, waits in the
/// passive `Pool` until `Hire` puts it on the `Bench` of the `Yard`, a port
/// whose tokens carry nets.
fn yard(_: &Params) -> Result<Net, LoadError> {
    let mut b = NetBuilder::new();
    let todo = b.place::<u32>("Todo");
    let done = b.place::<u32>("Done");
    b.seed(todo, [7]);
    b.transition("Work", (TakeEdge(todo),), (done,), |n: u32| (n,));
    let worker = b.build()?;
    let mut b = NetBuilder::new();
    let bench = b.place_nested::<Net>("Bench");
    b.port(bench);
    let inside = b.build()?;
    let mut b = NetBuilder::new();
    let pool = b.place_nested::<Net>("Pool");
    let yard = b.subnet_place("Yard", inside);
    let bench = b.port_of::<Net>(yard, "Bench");
    b.stepping(pool, Stepping::Passive);
    b.seed(pool, [worker]);
    b.transition("Hire", (TakeEdge(pool),), (bench,), |w: Net| (w,));
    Ok(b.build()?)
}

/// `unstamped`: a subnet built with no template, whose one place `In` is a
/// port.
fn unstamped(_: &Params) -> Result<Net, LoadError> {
    let mut b = NetBuilder::new();
    let input = b.place::<u32>("In");
    b.port(input);
    let inside = b.build()?;
    let mut b = NetBuilder::new();
    b.subnet_place("Inside", inside);
    Ok(b.build()?)
}

#[derive(Debug)]
struct Refused;

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("refused")
    }
}

impl Error for Refused {}

/// `edges`: a passive `Level` (the conformance template); `Refuse` takes a
/// hero from its lobby and fails, putting the hero back; `Look` reads the
/// lobby through a snapshot
/// edge; `Sweep` drains its exit hall. Each but `Refuse` takes a tick of its
/// own, so that it can fire with nothing to take from a port.
fn edges(_: &Params) -> Result<Net, LoadError> {
    let level = nets::levels::templates()
        .build("Level", &Params::new())
        .map_err(|error| LoadError::Other(error.to_string()))?;
    let mut b = NetBuilder::new();
    let level = b.subnet_place("Level", level);
    let looks = b.place::<u32>("Looks");
    let sweeps = b.place::<u32>("Sweeps");
    let done = b.place::<u32>("Done");
    let lobby = b.port_of::<Hero>(level, "Lobby");
    let exit = b.port_of::<Hero>(level, "Exit Hall");
    b.stepping(level, Stepping::Passive);
    b.seed(looks, [1]);
    b.seed(sweeps, [2]);
    b.transition(
        "Refuse",
        (TakeEdge(lobby),),
        (done,),
        |_: Hero| -> Result<(u32,), Refused> { Err(Refused) },
    )
    .restore_tokens_on_failure();
    b.transition(
        "Look",
        (TakeEdge(looks), petritype::SnapshotEdge(lobby)),
        (done,),
        |n: u32, _: Hero| (n,),
    );
    b.transition(
        "Sweep",
        (TakeEdge(sweeps), petritype::DrainEdge(exit)),
        (done,),
        |n: u32, _: Vec<Hero>| (n,),
    );
    Ok(b.build()?)
}

fn everything() -> Vec<NetEntry> {
    let mut entries = levels();
    entries.extend([
        NetEntry::new("keyed-world", keyed_world),
        NetEntry::new("tower", tower),
        NetEntry::new("harbour", harbour),
        NetEntry::new("yard", yard),
        NetEntry::new("unstamped", unstamped),
        NetEntry::new("edges", edges),
    ]);
    entries
}

fn loaded(net: &str, params: Value) -> H {
    H::loaded(everything(), net, params)
}

// ----- loading and the records -------------------------------------------------

#[test]
fn a_net_with_subnet_places_loads_and_each_subnet_is_known_from_the_load() {
    let mut h = loaded_levels();
    let state = h.result("get_state", json!({}));
    let entry = &state["marking"]["places"]["Level 1"];
    let id = entry["subnet"]["nested_id"].clone();
    assert_eq!(
        entry,
        &json!({"subnet": {"nested_id": id, "path": l1(), "template": "Level", "count": 0},
                "ports": {"Lobby": {"count": 0, "tokens": []}, "Exit Hall": {"count": 0, "tokens": []}}})
    );
    let by_id = h.result("get_state", json!({ "nested_id": id }));
    let by_path = h.result("get_state", json!({"path": l1()}));
    assert_eq!(by_id, by_path);
    assert_eq!(
        (
            &by_path["net"],
            &by_path["generation"],
            &by_path["template"],
            &by_path["path"]
        ),
        (&json!("Level 1"), &json!(1), &json!("Level"), &l1())
    );
    assert_ne!(
        by_path["nested_id"],
        state["marking"]["places"]["Level 2"]["subnet"]["nested_id"]
    );
}

#[test]
fn a_subnet_has_its_own_progress_and_log_view() {
    let mut h = loaded_levels();
    for _ in 0..4 {
        h.send("step", json!({}));
    }
    let inside = h.result("get_state", json!({"path": l1()}));
    assert_eq!(
        inside["progress"]["fired_counts"],
        json!({"Walk In": 1, "Fight": 1, "Win": 1})
    );
    assert_eq!(inside["progress"]["last_fired"], "Win");
    let world = h.result("get_state", json!({}));
    assert_eq!(world["progress"]["fired_counts"], json!({"Enter": 1}));
    let log = h.result("get_log", json!({"path": l1()}));
    let entries = log["entries"].as_array().unwrap();
    let names: Vec<&Value> = entries.iter().map(|e| &e["transition"]).collect();
    assert_eq!(names, [&json!("Walk In"), &json!("Fight"), &json!("Win")]);
    assert!(entries.iter().all(|e| e["path"] == l1()));
}

#[test]
fn a_reset_builds_new_subnets_and_takes_their_baselines_afresh() {
    let mut h = loaded_levels();
    let before = h.nested_id(&l1());
    h.send("step", json!({}));
    h.send("reset", json!({}));
    let after = h.nested_id(&l1());
    assert_ne!(after, before);
    assert_eq!(
        h.error("get_state", json!({"nested_id": before})),
        "unknown_net"
    );
    let lines = h.send("step", json!({}));
    let fireables = inner(&lines, "fireable");
    assert_eq!(fireables.len(), 1, "{lines:#?}");
    assert_eq!(fireables[0].0["nested_id"], after);
    assert_eq!(fireables[0].1["transitions"], json!(["Walk In"]));
}

#[test]
fn a_run_goes_through_both_levels_to_rest() {
    let mut h = loaded_levels();
    let lines = h.send("start", json!({}));
    let stopped = outer(&lines, "execution_stopped");
    assert_eq!(stopped.last().unwrap()["reason"], "no_fireable_transitions");
    let state = h.result("get_state", json!({}));
    assert_eq!(state["marking"]["places"]["Outside"]["count"], 1);
    assert_eq!(
        state["progress"]["fired_counts"],
        json!({"Enter": 1, "Climb": 1, "Leave": 1})
    );
    for level in ["Level 1", "Level 2"] {
        assert_eq!(state["marking"]["places"][level]["subnet"]["count"], 0);
    }
}

// ----- the topology ------------------------------------------------------------

#[test]
fn the_outer_topology_lists_a_subnet_place_with_its_ports_and_edges_name_a_port() {
    let mut h = loaded_levels();
    let topology = h.result("get_state", json!({}))["topology"].clone();
    let names: Vec<&Value> = topology["places"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| &p["name"])
        .collect();
    assert_eq!(
        names,
        [
            &json!("Door"),
            &json!("Level 1"),
            &json!("Level 2"),
            &json!("Outside")
        ],
        "nothing else of the subnets"
    );
    let level = &topology["places"][1];
    assert_eq!(
        (&level["type_name"], &level["subnet"], &level["stepping"]),
        (&json!("Level"), &json!(true), &json!("inner_first"))
    );
    assert!(level.get("serializable").is_none());
    let ports: Vec<&Value> = level["ports"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| &p["name"])
        .collect();
    assert_eq!(ports, [&json!("Lobby"), &json!("Exit Hall")]);
    assert!(level["ports"][0].get("port").is_none());
    let edges = topology["edges"].as_array().unwrap();
    assert!(edges.contains(
        &json!({"place": "Level 1", "port": "Exit Hall", "transition": "Climb",
                "direction": "in", "kind": "take"})
    ));
    assert!(edges.contains(
        &json!({"place": "Level 2", "port": "Lobby", "transition": "Climb", "direction": "out"})
    ));
}

#[test]
fn a_subnet_s_own_topology_marks_its_ports_and_a_net_loaded_alone_does_not() {
    let mut h = loaded_levels();
    let marks = |state: &Value| -> Vec<Value> {
        state["topology"]["places"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.get("port").cloned().unwrap_or(Value::Null))
            .collect()
    };
    let inside = h.result("get_state", json!({"path": l1()}));
    assert_eq!(marks(&inside), [json!(true), Value::Null, json!(true)]);
    h.result("load", json!({"net": "lone-level"}));
    let alone = h.result("get_state", json!({}));
    assert_eq!(
        marks(&alone),
        [Value::Null, Value::Null, Value::Null],
        "no net around it reaches them"
    );
}

#[test]
fn a_subnet_place_without_a_template_has_an_empty_type_name() {
    let mut h = loaded("unstamped", json!({}));
    let state = h.result("get_state", json!({}));
    assert_eq!(state["topology"]["places"][0]["type_name"], "");
    assert!(
        state["marking"]["places"]["Inside"]["subnet"]
            .get("template")
            .is_none()
    );
}

// ----- addresses ---------------------------------------------------------------

#[test]
fn a_subnet_step_names_its_subnet_and_the_wrong_steps_name_none() {
    let mut h = loaded_levels();
    assert_eq!(
        h.result("get_state", json!({"path": l1()}))["net"],
        "Level 1"
    );
    for (path, says) in [
        (
            json!([{"place": "Door"}]),
            "step 0 of the path: place \"Door\" is not a subnet place",
        ),
        (
            json!([{"place": "Nowhere"}]),
            "step 0 of the path: no place named \"Nowhere\"",
        ),
        (
            json!([{"place": "Level 1", "token": "0", "net": ""}]),
            "step 0 of the path: place \"Level 1\" is a subnet place",
        ),
        (
            json!([{"place": "Level 1"}, {"place": "Lobby"}]),
            "step 1 of the path: place \"Lobby\" is not a subnet place",
        ),
    ] {
        let lines = h.send("get_state", json!({ "path": path }));
        assert_eq!(lines[0]["error"]["code"], "unknown_net", "{path}");
        let message = lines[0]["error"]["message"].as_str().unwrap();
        assert!(message.starts_with(says), "{path}: {message}");
    }
}

#[test]
fn two_depths_of_subnets() {
    let mut h = loaded("tower", json!({"room": "inner_first"}));
    let room = h.nested_id(&room_path());
    assert_eq!(
        h.result("get_state", json!({"nested_id": room}))["path"],
        room_path()
    );
    let floor = h.result("get_state", json!({"path": floor_path()}));
    let entry = &floor["marking"]["places"]["Room"]["subnet"];
    assert_eq!(
        (&entry["path"], &entry["nested_id"]),
        (&room_path(), &json!(room))
    );
    h.send("step", json!({})); // Enter
    h.send("step", json!({})); // the floor's Go In
    let lines = h.send("step", json!({})); // the room's Sit, two levels down
    let answer = &lines.last().unwrap()["result"];
    assert_eq!(
        (&answer["path"], &answer["nested_id"]),
        (&room_path(), &json!(room))
    );
    let fired = inner(&lines, "fired");
    assert_eq!(fired.len(), 1);
    assert_eq!(
        (&fired[0].0["path"], &fired[0].1["transition"]),
        (&room_path(), &json!("Sit"))
    );
}

#[test]
fn a_subnet_inside_a_token_s_net() {
    let mut h = loaded("harbour", json!({}));
    let state = h.result("get_state", json!({}));
    let token = &state["marking"]["places"]["Harbour"]["tokens"][0];
    let ship = token["nets"][0]["nested_id"].clone();
    let ship_path = json!([{"place": "Harbour", "token": token["id"], "net": ""}]);
    let hold_path =
        json!([{"place": "Harbour", "token": token["id"], "net": ""}, {"place": "Hold"}]);
    let hold = h.result("get_state", json!({"path": hold_path}));
    assert_eq!(
        (&hold["net"], &hold["template"], &hold["path"]),
        (&json!("Hold"), &json!("Hold"), &hold_path)
    );
    let ship_state = h.result("get_state", json!({ "nested_id": ship }));
    assert_eq!(
        ship_state["marking"]["places"]["Hold"]["subnet"]["path"],
        hold_path
    );

    let lines = h.send("step", json!({})); // the ship's Load, into its hold's bay
    let answer = lines.last().unwrap()["result"].clone();
    assert_eq!(answer["path"], ship_path);
    let fired = inner(&lines, "fired");
    assert_eq!(fired.len(), 1, "{lines:#?}");
    let (wrapper, load) = fired[0];
    assert_eq!(
        load["produced"][0][0],
        json!({"place": "Hold", "port": "Bay"})
    );
    let versions = load["subnet_marking_versions"].as_object().unwrap();
    assert_eq!(versions.keys().collect::<Vec<_>>(), ["Hold"]);
    assert_eq!(
        wrapper["marking_version"], answer["marking_version"],
        "the world sees no port of the hold"
    );
    assert_eq!(h.version(&hold_path), versions["Hold"].as_u64().unwrap());

    let lines = h.send("step", json!({})); // the hold's Stow, two levels down
    assert_eq!(lines.last().unwrap()["result"]["path"], hold_path);
    let fired = inner(&lines, "fired");
    assert_eq!(fired[0].0["path"], hold_path);
    let ship_now = h.version(&ship_path);
    assert_eq!(
        fired[0].1["outer_marking_version"], ship_now,
        "the ship sees its hold's bay"
    );
}

#[test]
fn a_subnet_dies_with_the_token_whose_net_holds_it() {
    let mut h = loaded("harbour", json!({}));
    let state = h.result("get_state", json!({}));
    let token = &state["marking"]["places"]["Harbour"]["tokens"][0];
    let hold_path =
        json!([{"place": "Harbour", "token": token["id"], "net": ""}, {"place": "Hold"}]);
    let hold = h.nested_id(&hold_path);
    let steps: Vec<Value> = (0..3)
        .map(|_| h.result("step", json!({}))["transition"].clone())
        .collect();
    assert_eq!(steps, [json!("Load"), json!("Stow"), json!("Scrap")]);
    assert_eq!(
        h.error("get_state", json!({"nested_id": hold})),
        "unknown_net"
    );
    assert_eq!(
        h.error("get_state", json!({"path": hold_path})),
        "unknown_net"
    );
}

#[test]
fn a_token_s_net_inside_a_port() {
    let mut h = loaded("yard", json!({}));
    let lines = h.send("step", json!({})); // Hire: the worker onto the yard's bench
    assert_eq!(lines.last().unwrap()["result"]["transition"], "Hire");
    let produced = &outer(&lines, "distributed")[0]["produced"][0];
    assert_eq!(produced[0], json!({"place": "Yard", "port": "Bench"}));
    let bench = h.result(
        "get_place_tokens",
        json!({"place": "Yard", "port": "Bench"}),
    );
    let token = bench["tokens"][0].clone();
    let worker_path =
        json!([{"place": "Yard"}, {"place": "Bench", "token": token["id"], "net": ""}]);
    assert_eq!(
        token["nets"][0]["path"], worker_path,
        "a port's tokens' nets are reached through the subnet"
    );
    let world = h.result("get_state", json!({}));
    assert_eq!(
        world["marking"]["places"]["Yard"]["ports"]["Bench"]["tokens"][0]["nets"][0]["path"],
        worker_path
    );
    let lines = h.send("step", json!({})); // the worker's Work, on the bench
    assert_eq!(lines.last().unwrap()["result"]["path"], worker_path);
    assert_eq!(inner(&lines, "fired")[0].0["path"], worker_path);
    let worker = h.result("get_state", json!({"path": worker_path}));
    assert_eq!(worker["progress"]["fired_counts"], json!({"Work": 1}));
    let through = h.result(
        "get_token",
        json!({"id": token["id"], "path": [{"place": "Yard"}]}),
    );
    assert_eq!(through["nets"][0]["path"], worker_path);
}

// ----- references --------------------------------------------------------------

#[test]
fn a_firing_names_a_port_by_the_object_and_so_does_the_log() {
    let mut h = loaded_levels();
    let enter = h.send("step", json!({}));
    let ada = outer(&enter, "launched")[0]["consumed"][0][1].clone();
    assert_eq!(
        outer(&enter, "launched")[0]["consumed"],
        json!([["Door", ada]])
    );
    let produced = &outer(&enter, "distributed")[0]["produced"][0];
    assert_eq!(produced[0], json!({"place": "Level 1", "port": "Lobby"}));
    for _ in 0..3 {
        h.send("step", json!({}));
    }
    let climb = h.send("step", json!({}));
    assert_eq!(
        outer(&climb, "launched")[0]["consumed"][0][0],
        json!({"place": "Level 1", "port": "Exit Hall"})
    );
    let log = h.result("get_log", json!({}));
    let entries = log["entries"].as_array().unwrap();
    assert_eq!(
        entries[0]["produced"][0][0],
        json!({"place": "Level 1", "port": "Lobby"})
    );
    let last = entries.last().unwrap();
    assert_eq!(
        last["consumed"][0][0],
        json!({"place": "Level 1", "port": "Exit Hall"})
    );
    assert!(
        last.get("path").is_none(),
        "the world's firing is the world's entry"
    );
}

#[test]
fn added_removed_and_mutated_name_a_port_with_port_beside_place() {
    let mut h = loaded("keyed-world", json!({}));
    h.send("disable", json!({"transition": "Promote"}));
    h.send("step", json!({}));
    h.send("step", json!({}));
    h.send("disable", json!({"transition": "Enter"}));
    h.send("enable", json!({"transition": "Promote"}));
    let lines = h.send("step", json!({})); // Promote: Ada, at the front, becomes a lord
    let mutated = outer(&lines, "mutated");
    assert_eq!(mutated.len(), 1, "{lines:#?}");
    assert_eq!(
        (
            &mutated[0]["place"],
            &mutated[0]["port"],
            &mutated[0]["keys"]
        ),
        (
            &json!("Level"),
            &json!("Lobby"),
            &json!({"name": "Ada", "role": "Lord"})
        )
    );
    let level = h.version(&json!([{"place": "Level"}]));
    assert_eq!(
        mutated[0]["subnet_marking_versions"],
        json!({"Level": level})
    );
    let lines = h.send("press", json!({"control": "clear"}));
    let removed: Vec<(&Value, &Value)> = outer(&lines, "removed")
        .into_iter()
        .map(|r| (&r["place"], &r["port"]))
        .collect();
    assert_eq!(removed, [(&json!("Level"), &json!("Lobby")); 2]);
    let lines = h.send(
        "inject",
        json!({"place": "Level", "port": "Exit Hall", "token": {"name": "Cy", "role": "Bard"}}),
    );
    let added = outer(&lines, "added");
    assert_eq!(
        (&added[0]["place"], &added[0]["port"], &added[0]["keys"]),
        (
            &json!("Level"),
            &json!("Exit Hall"),
            &json!({"name": "Cy", "role": "Bard"})
        )
    );
}

#[test]
fn hidden_names_a_port_under_its_subnet_s_filter() {
    let mut h = loaded("keyed-world", json!({}));
    let mages = json!({"clauses": [{"key": "role", "values": ["Mage"]}]});
    h.result(
        "set_filter",
        json!({"filter": mages, "path": [{"place": "Level"}]}),
    );
    let lines = h.send(
        "inject",
        json!({"place": "Level", "port": "Lobby", "token": {"name": "Ada", "role": "Knight"}}),
    );
    let hidden = outer(&lines, "hidden");
    assert_eq!(
        (&hidden[0]["kind"], &hidden[0]["place"], &hidden[0]["port"]),
        (&json!("added"), &json!("Level"), &json!("Lobby"))
    );
    assert!(hidden[0]["subnet_marking_versions"].get("Level").is_some());
    let lines = h.send(
        "inject",
        json!({"place": "Level", "port": "Lobby", "token": {"name": "Bo", "role": "Mage"}}),
    );
    assert_eq!(outer(&lines, "added")[0]["port"], "Lobby");
}

// ----- state -------------------------------------------------------------------

#[test]
fn a_subnet_place_s_entry_holds_its_subnet_and_its_ports_under_the_token_limit() {
    let mut h = loaded_levels();
    for name in ["Bram", "Cato", "Dara"] {
        h.result(
            "inject",
            json!({"place": "Level 2", "port": "Lobby", "token": {"name": name, "fought": false}}),
        );
    }
    h.result(
        "inject",
        json!({"place": "Arena", "path": l2(), "token": {"name": "Eve", "fought": false}}),
    );
    let state = h.result("get_state", json!({"token_limit": 2}));
    let entry = &state["marking"]["places"]["Level 2"];
    assert_eq!(
        entry["subnet"]["count"], 4,
        "its ports included, and its own places"
    );
    assert_eq!(entry["ports"]["Lobby"]["count"], 3);
    assert_eq!(
        entry["ports"]["Lobby"]["tokens"].as_array().unwrap().len(),
        2
    );
    assert_eq!(
        entry["ports"]["Exit Hall"],
        json!({"count": 0, "tokens": []})
    );
}

#[test]
fn a_port_is_projected_by_its_subnet_s_filter_in_both_views() {
    let mut h = loaded("keyed-world", json!({}));
    h.send("step", json!({}));
    h.send("step", json!({}));
    let mages = json!({"clauses": [{"key": "role", "values": ["Mage"]}]});
    h.result(
        "set_filter",
        json!({"filter": mages, "path": [{"place": "Level"}]}),
    );
    let state = h.result("get_state", json!({}));
    let lobby = &state["marking"]["places"]["Level"]["ports"]["Lobby"];
    assert_eq!(
        (&lobby["count"], &lobby["matching"]),
        (&json!(2), &json!(1))
    );
    assert_eq!(lobby["tokens"][0]["keys"]["name"], "Bo");
    assert!(state["marking"]["places"]["Door"].get("matching").is_none());
    let inside = h.result("get_state", json!({"path": [{"place": "Level"}]}));
    let own = &inside["marking"]["places"]["Lobby"];
    assert_eq!((&own["count"], &own["matching"]), (&json!(2), &json!(1)));
    let page = h.result(
        "get_place_tokens",
        json!({"place": "Level", "port": "Lobby"}),
    );
    assert_eq!((&page["count"], &page["matching"]), (&json!(2), &json!(1)));

    let places = |result: &Value| -> Vec<(Value, Value)> {
        result["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| {
                (
                    m["place"].clone(),
                    m.get("port").cloned().unwrap_or(Value::Null),
                )
            })
            .collect()
    };
    let found = h.result("query", json!({}));
    assert_eq!(
        places(&found),
        [
            (json!("Ticks"), Value::Null),
            (json!("Level"), json!("Lobby"))
        ],
        "the world's own places by its own filter, none; the port by its subnet's"
    );
    assert_eq!(found["matches"][1]["token"]["id"], lobby["tokens"][0]["id"]);
    let everyone = h.result("query", json!({"filter": {}}));
    assert_eq!(
        places(&everyone),
        [
            (json!("Ticks"), Value::Null),
            (json!("Level"), json!("Lobby")),
            (json!("Level"), json!("Lobby"))
        ],
        "a filter of its own, ports included"
    );
}

#[test]
fn a_filter_on_the_net_around_projects_its_own_places_alone() {
    let mut h = loaded("keyed-world", json!({}));
    h.send("step", json!({})); // Enter Ada, a knight, into the lobby
    let mages = json!({"clauses": [{"key": "role", "values": ["Mage"]}]});
    h.result("set_filter", json!({"filter": mages}));
    let state = h.result("get_state", json!({}));
    assert_eq!(state["marking"]["places"]["Door"]["matching"], 1);
    let lobby = &state["marking"]["places"]["Level"]["ports"]["Lobby"];
    assert!(lobby.get("matching").is_none(), "{lobby}");
    assert_eq!(lobby["tokens"].as_array().unwrap().len(), 1);
    // Bo the mage enters: kept by the world's filter where he was, and told
    // in the port with nothing hidden, since the subnet has no filter.
    let lines = h.send("step", json!({}));
    let produced = &outer(&lines, "distributed")[0]["produced"];
    assert_eq!(produced.as_array().unwrap().len(), 1, "{lines:#?}");
    assert_eq!(
        outer(&lines, "launched")[0]["consumed"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

// ----- requests ----------------------------------------------------------------

#[test]
fn a_port_s_tokens_are_read_and_put_both_ways_with_the_subnet_s_ids() {
    let mut h = loaded_levels();
    let by_pair = h.result(
        "inject",
        json!({"place": "Level 1", "port": "Lobby", "token": {"name": "Bram", "fought": false}}),
    );
    let by_place = h.result(
        "inject",
        json!({"place": "Lobby", "path": l1(), "token": {"name": "Cato", "fought": false}}),
    );
    let first: u64 = by_pair["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        by_place["id"],
        (first + 1).to_string(),
        "one counter, the subnet's"
    );
    assert!(by_pair.get("path").is_none());
    assert_eq!(by_place["path"], l1());
    let outside = h.result(
        "get_place_tokens",
        json!({"place": "Level 1", "port": "Lobby"}),
    );
    let inside = h.result("get_place_tokens", json!({"place": "Lobby", "path": l1()}));
    assert_eq!(outside["tokens"], inside["tokens"]);
    assert_eq!(
        (&outside["place"], &outside["port"]),
        (&json!("Level 1"), &json!("Lobby"))
    );
    assert!(inside.get("port").is_none());
    assert_eq!(outside["marking_version"], h.version(&json!([])));
    assert_eq!(inside["marking_version"], h.version(&l1()));
}

#[test]
fn a_port_s_token_is_read_through_its_subnet() {
    let mut h = loaded_levels();
    let lines = h.send("step", json!({})); // Enter: Ada into level 1's lobby
    let ada = outer(&lines, "distributed")[0]["produced"][0][1].clone();
    // The subnet minted her id in the lobby; the world's own places hold no
    // token by it, and a port's tokens are read through the subnet.
    assert_eq!(h.error("get_token", json!({"id": ada})), "unknown_token");
    let value = h.result("get_token", json!({"id": ada, "path": l1()}));
    assert_eq!(
        (&value["place"], &value["kind"], &value["value"]),
        (
            &json!("Lobby"),
            &json!("json"),
            &json!({"name": "Ada", "fought": false})
        )
    );
    assert_eq!(
        h.error("get_token", json!({"id": "+0", "path": l1()})),
        "unknown_token"
    );
}

#[test]
fn the_errors_of_naming_a_port() {
    let mut h = loaded_levels();
    let hero = json!({"name": "Bram", "fought": false});
    for (method, params, code) in [
        (
            "get_place_tokens",
            json!({"place": "Level 1"}),
            "invalid_params",
        ),
        (
            "inject",
            json!({"place": "Level 1", "token": hero}),
            "invalid_params",
        ),
        (
            "get_place_tokens",
            json!({"place": "Level 1", "port": "Arena"}),
            "unknown_place",
        ),
        (
            "get_place_tokens",
            json!({"place": "Level 1", "port": "Kitchen"}),
            "unknown_place",
        ),
        (
            "inject",
            json!({"place": "Door", "port": "Lobby", "token": hero}),
            "unknown_place",
        ),
        (
            "get_place_tokens",
            json!({"place": "Nowhere", "port": "Lobby"}),
            "unknown_place",
        ),
        (
            "get_place_tokens",
            json!({"place": "Level 1", "port": 3}),
            "invalid_params",
        ),
        (
            "get_place_tokens",
            json!({"place": "Level 1", "port": "Lobby", "path": l1()}),
            "unknown_place",
        ),
        (
            "inject",
            json!({"place": "Level 1", "port": "Lobby", "token": {"name": "Bram"}}),
            "invalid_token",
        ),
    ] {
        assert_eq!(h.error(method, params.clone()), code, "{method} {params}");
    }
}

#[test]
fn filters_and_distinct_name_a_port_by_the_object() {
    let mut h = loaded("keyed-world", json!({}));
    h.send("step", json!({}));
    let lobby = json!({"place": "Level", "port": "Lobby"});
    let ports = |result: &Value| -> Vec<Value> {
        result["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m.get("port").cloned().unwrap_or(Value::Null))
            .collect()
    };
    let found = h.result("query", json!({"filter": {"places": [lobby]}}));
    assert_eq!(ports(&found), [json!("Lobby")]);
    assert_eq!(found["matches"][0]["place"], "Level");
    assert_eq!(
        h.result("query", json!({"filter": {"places": ["Level"]}}))["count"],
        0,
        "it holds none"
    );
    let found = h.result("query", json!({"filter": {"prefix": "Lob"}}));
    assert_eq!(ports(&found), [json!("Lobby")], "a port by its own name");
    let values = h.result("distinct", json!({"key": "name", "places": [lobby]}));
    assert_eq!(values["values"], json!(["Ada"]));
    let values = h.result("distinct", json!({"key": "name"}));
    assert_eq!(values["values"], json!(["Bo", "Ada"]));
    h.result("set_filter", json!({"filter": {"places": [lobby, "Door"]}}));
    for (bad, code) in [
        (
            json!({"places": [{"place": "Level", "port": "Kitchen"}]}),
            "unknown_place",
        ),
        (
            json!({"places": [{"place": "Door", "port": "Lobby"}]}),
            "unknown_place",
        ),
        (json!({"places": [{"place": "Level"}]}), "invalid_params"),
        (
            json!({"places": [{"place": "Level", "port": 7}]}),
            "invalid_params",
        ),
        (json!({"places": "Level"}), "invalid_params"),
    ] {
        assert_eq!(h.error("query", json!({ "filter": bad })), code, "{bad}");
    }
    let other_keys = json!({"places": [{"place": "Level", "port": "Lobby", "note": "ignored"}]});
    assert_eq!(
        h.result("query", json!({ "filter": other_keys }))["count"],
        1
    );
    assert_eq!(
        h.error(
            "distinct",
            json!({"key": "name", "places": [{"place": 1, "port": "x"}]})
        ),
        "invalid_params"
    );
}

// ----- one place, two views ----------------------------------------------------

#[test]
fn a_change_is_told_once_by_the_net_through_which_it_was_made() {
    let mut h = loaded_levels();
    let lines = h.send("step", json!({})); // Enter, told by the world
    assert_eq!(
        sequence(&lines),
        [
            "launched",
            "distributed",
            "fireable",
            "nested:fireable",
            "result"
        ]
    );
    let lines = h.send("step", json!({})); // Walk In, told by the level, in one piece
    assert_eq!(
        sequence(&lines),
        ["nested:fired", "nested:fireable", "result"]
    );
}

#[test]
fn after_a_change_of_a_port_both_nets_fireable_sets_are_compared_the_outer_first() {
    let mut h = loaded_levels();
    let lines = h.send(
        "inject",
        json!({"place": "Exit Hall", "path": l2(), "token": {"name": "Cy", "fought": true}}),
    );
    assert_eq!(
        sequence(&lines),
        ["nested:added", "fireable", "result"],
        "the level's set did not change"
    );
    assert_eq!(
        outer(&lines, "fireable")[0]["transitions"],
        json!(["Enter", "Leave"])
    );
    let lines = h.send(
        "inject",
        json!({"place": "Level 2", "port": "Lobby", "token": {"name": "Di", "fought": false}}),
    );
    assert_eq!(sequence(&lines), ["added", "nested:fireable", "result"]);
}

#[test]
fn a_net_s_fireable_precedes_the_nets_inside_it_at_two_depths() {
    let mut h = loaded("tower", json!({"room": "inner_first"}));
    h.send("step", json!({})); // Enter
    let lines = h.send("step", json!({})); // the floor's Go In, into the room's den
    let paths: Vec<&Value> = inner(&lines, "fireable")
        .iter()
        .map(|(w, _)| &w["path"])
        .collect();
    assert_eq!(paths, [&floor_path(), &room_path()]);
}

// ----- versions ----------------------------------------------------------------

#[test]
fn told_by_the_outer_net_the_subnet_s_version_rides_along_and_it_goes_on_from_there() {
    let mut h = loaded_levels();
    let before = h.version(&l1());
    let lines = h.send("step", json!({})); // Enter, into level 1's lobby
    assert_eq!(
        outer(&lines, "distributed")[0]["subnet_marking_versions"],
        json!({"Level 1": before + 1})
    );
    assert!(
        outer(&lines, "launched")[0]
            .get("subnet_marking_versions")
            .is_none(),
        "Door is the world's"
    );
    assert_eq!(
        inner(&lines, "fireable")[0].1["marking_version"],
        before + 1
    );
    let lines = h.send("step", json!({})); // Walk In, told by the level
    assert_eq!(
        inner(&lines, "fired")[0].1["marking_version"],
        before + 2,
        "no version skipped"
    );
}

#[test]
fn told_by_the_subnet_the_outer_version_rides_along_and_on_the_wrapper() {
    let mut h = loaded_levels();
    h.send("step", json!({})); // Enter
    let world = h.version(&json!([]));
    let lines = h.send("step", json!({})); // Walk In, out of the lobby
    let (wrapper, walk_in) = inner(&lines, "fired")[0];
    assert_eq!(walk_in["outer_marking_version"], world + 1);
    assert_eq!(wrapper["marking_version"], world + 1);
    assert_eq!(
        lines.last().unwrap()["result"]["marking_version"],
        world + 1
    );
    let lines = h.send("step", json!({})); // Fight, wholly inside
    let (wrapper, fight) = inner(&lines, "fired")[0];
    assert!(fight.get("outer_marking_version").is_none());
    assert_eq!(wrapper["marking_version"], world + 1);
    assert_eq!(h.version(&json!([])), world + 1);
}

#[test]
fn one_notification_may_carry_both_fields() {
    let mut h = loaded("tower", json!({})); // the room passive: its den keeps what it is given
    h.send("step", json!({})); // Enter Ada
    // The floor's Go In, out of its own lobby and into the room's den: one
    // inline firing, so one `fired` with both.
    let lines = h.send("step", json!({}));
    let (_, go_in) = inner(&lines, "fired")[0];
    assert_eq!(go_in["transition"], "Go In");
    assert!(go_in.get("outer_marking_version").is_some());
    assert!(go_in["subnet_marking_versions"].get("Room").is_some());
    h.send("step", json!({})); // Enter Bo
    let lines = h.send("step", json!({})); // the floor's Gather, from its lobby and the room's den
    let (wrapper, gather) = inner(&lines, "fired")[0];
    assert_eq!(gather["transition"], "Gather");
    assert_eq!(
        wrapper["marking_version"], gather["outer_marking_version"],
        "the world is the floor's outer net"
    );
    let room = h.version(&room_path());
    assert_eq!(gather["subnet_marking_versions"], json!({"Room": room}));
}

#[test]
fn deeper_down_the_wrapper_carries_the_loaded_net_s_version_unchanged() {
    let mut h = loaded("tower", json!({"room": "inner_first"}));
    h.send("step", json!({})); // Enter
    h.send("step", json!({})); // Go In
    let world = h.version(&json!([]));
    let floor = h.version(&floor_path());
    let lines = h.send("step", json!({})); // the room's Sit, out of its den, which the floor sees
    let (wrapper, sit) = inner(&lines, "fired")[0];
    assert_eq!(sit["outer_marking_version"], floor + 1);
    assert_eq!(
        wrapper["marking_version"], world,
        "the world does not see the den"
    );
    assert_eq!(h.version(&floor_path()), floor + 1);
    assert_eq!(h.version(&json!([])), world);
}

#[test]
fn a_failed_firing_restores_into_the_port_and_names_the_pair() {
    let mut h = loaded("edges", json!({}));
    let path = json!([{"place": "Level"}]);
    let ada = h.result(
        "inject",
        json!({"place": "Level", "port": "Lobby", "token": {"name": "Ada", "fought": false}}),
    )["id"]
        .clone();
    let before = h.version(&path);
    h.send("disable", json!({"transition": "Look"}));
    let lines = h.send("step", json!({})); // Refuse: out of the lobby, and back
    let launched = outer(&lines, "launched");
    let failed = outer(&lines, "failed");
    let lobby = json!({"place": "Level", "port": "Lobby"});
    assert_eq!(launched[0]["consumed"], json!([[lobby, ada]]));
    assert_eq!(
        launched[0]["subnet_marking_versions"],
        json!({"Level": before + 1})
    );
    assert_eq!(
        (&failed[0]["restored"], &failed[0]["consumed"]),
        (&json!(true), &json!([[lobby, ada]]))
    );
    assert_eq!(
        failed[0]["subnet_marking_versions"],
        json!({"Level": before + 2})
    );
    let page = h.result("get_place_tokens", json!({"place": "Lobby", "path": path}));
    assert_eq!(page["tokens"][0]["id"], ada, "back with its id");
    assert_eq!(page["marking_version"], before + 2);
}

#[test]
fn a_read_edge_and_a_drain_of_an_empty_port_change_nothing_there() {
    let mut h = loaded("edges", json!({}));
    let path = json!([{"place": "Level"}]);
    h.result(
        "inject",
        json!({"place": "Level", "port": "Lobby", "token": {"name": "Ada", "fought": false}}),
    );
    h.send("disable", json!({"transition": "Refuse"}));
    let before = h.version(&path);
    for transition in ["Look", "Sweep"] {
        let lines = h.send("step", json!({}));
        assert_eq!(lines.last().unwrap()["result"]["transition"], transition);
        for line in outer(&lines, "launched")
            .into_iter()
            .chain(outer(&lines, "distributed"))
        {
            assert!(
                line.get("subnet_marking_versions").is_none(),
                "{transition}: {line}"
            );
        }
    }
    assert_eq!(h.version(&path), before, "the level's version did not move");
}

#[test]
fn a_reader_that_follows_one_net_sees_a_change_told_by_the_other_as_a_gap() {
    let mut h = loaded_levels();
    let level = h.nested_id(&l1());
    let (mut world_lines, mut level_lines) = (Vec::new(), Vec::new());
    let mut sends = vec![("step", json!({})); 5]; // Enter, Walk In, Fight, Win, Climb
    sends.push((
        "inject",
        json!({"place": "Lobby", "path": l1(), "token": {"name": "Bram", "fought": false}}),
    ));
    for (method, params) in sends {
        for line in h.send(method, params) {
            if line["method"] == "nested" && line["params"]["nested_id"] == level {
                level_lines.push(line["params"]["event"]["params"].clone());
            } else if line["method"].is_string() && line["method"] != "nested" {
                world_lines.push(line["params"].clone());
            }
        }
    }
    let steps = |stamps: &[Value]| -> Vec<u64> {
        let versions: Vec<u64> = stamps
            .iter()
            .filter(|params| params.get("transitions").is_none())
            .map(|params| params["marking_version"].as_u64().unwrap())
            .collect();
        versions.windows(2).map(|pair| pair[1] - pair[0]).collect()
    };
    // Walk In and Win moved the world, told by the level; Enter and Climb
    // the level, told by the world: each reader meets the next value more
    // than one on.
    assert_eq!(steps(&world_lines), [1, 3, 1]);
    assert!(steps(&level_lines).into_iter().max().unwrap() > 1);
}

#[test]
fn a_reader_that_follows_both_nets_sees_no_version_skipped_in_either() {
    let mut h = loaded_levels();
    let level = h.nested_id(&l1());
    let (mut world, mut inside) = (0, 0);
    for _ in 0..5 {
        for line in h.send("step", json!({})) {
            let params = &line["params"];
            if line["method"] == "nested" && params["nested_id"] == level {
                let event = &params["event"];
                if event["method"] != "fireable" {
                    assert_eq!(event["params"]["marking_version"], inside + 1);
                    inside += 1;
                }
                if let Some(outer) = event["params"]["outer_marking_version"].as_u64() {
                    assert_eq!(outer, world + 1);
                    world = outer;
                }
            } else if [
                "launched",
                "distributed",
                "fired",
                "added",
                "removed",
                "mutated",
                "hidden",
            ]
            .contains(&line["method"].as_str().unwrap_or(""))
            {
                assert_eq!(params["marking_version"], world + 1, "{line}");
                world += 1;
                if let Some(v) = params["subnet_marking_versions"]["Level 1"].as_u64() {
                    assert_eq!(v, inside + 1);
                    inside = v;
                }
            }
        }
    }
    assert_eq!(h.version(&json!([])), world);
    assert_eq!(h.version(&l1()), inside);
}

// ----- reads during a firing ---------------------------------------------------

#[test]
fn a_subnet_is_read_after_the_landing_of_an_outer_firing_on_its_port() {
    let mut h = loaded_levels();
    h.auto = false;
    let lines = h.send("step", json!({})); // Enter, held in flight
    assert_eq!(sequence(&lines), ["launched"]);
    assert_eq!(
        h.send("get_state", json!({"path": l1()})),
        Vec::<Value>::new(),
        "it waits"
    );
    let waiting = h.next.to_string();
    let other = h.result("get_state", json!({"path": l2()}));
    assert_eq!(
        other["net"], "Level 2",
        "a subnet the firing does not touch is read at once"
    );
    let state = h.result("get_state", json!({}));
    assert_eq!(state["transitions"]["Enter"]["in_flight"], true);
    assert_eq!(state["marking"]["places"]["Door"]["count"], 0);
    assert_eq!(
        state["marking"]["places"]["Level 1"]["ports"]["Lobby"]["count"],
        0
    );
    let lines = h.land();
    let answer = lines
        .iter()
        .find(|line| line["id"] == waiting)
        .expect("the waiting read is answered at the landing");
    assert_eq!(answer["result"]["marking"]["places"]["Lobby"]["count"], 1);
}

// ----- a hosted net ------------------------------------------------------------

/// A hosted `levels` world, and its two subnets' nested ids.
fn hosted() -> (Host, Vec<String>) {
    let net = nets::levels::levels(&Params::new()).expect("the world builds");
    let ids = ["Level 1", "Level 2"]
        .map(|name| {
            net.subnet(net.place_named(name).unwrap())
                .unwrap()
                .id()
                .to_string()
        })
        .to_vec();
    (Host::new("levels", net), ids)
}

/// `line` with each of `ids` written as its position, so that two worlds
/// built alike read alike.
fn alike(line: &str, ids: &[String]) -> String {
    ids.iter()
        .enumerate()
        .fold(line.to_owned(), |line, (at, id)| {
            line.replace(
                &format!(r#""nested_id":"{id}""#),
                &format!(r#""nested_id":"level {at}""#),
            )
        })
}

fn read_all(host: &mut Host, ids: &[String]) -> Vec<String> {
    let mut asked = vec![
        r#"{"id":"s","method":"get_state"}"#.to_owned(),
        r#"{"id":"l","method":"get_log"}"#.to_owned(),
    ];
    asked.extend(ids.iter().map(|id| {
        json!({"id": "n", "method": "get_state", "params": {"nested_id": id}}).to_string()
    }));
    asked
        .iter()
        .flat_map(|request| host.request(request, now()))
        .map(|line| alike(&line, ids))
        .collect()
}

#[test]
fn a_hosted_net_s_subnets_are_told_read_and_kept_quiet() {
    let (mut watched, watched_ids) = hosted();
    let (mut quiet, quiet_ids) = hosted();
    quiet.set_listening(false);
    let step = |host: &mut Host| {
        host.change(now(), |net| net.step_deep().expect("nothing fails"))
            .1
    };
    // Enter, then level 1's Walk In, Fight and Win: the subnet's firings and
    // its ports, told by both nets, while one host is quiet.
    for _ in 0..4 {
        let lines = step(&mut watched);
        assert!(!lines.is_empty());
        assert_eq!(step(&mut quiet), Vec::<String>::new());
    }
    let lines: Vec<Value> = {
        let (_, lines) = watched.change(now(), |net| {
            let level = net.place_named("Level 2").unwrap();
            let exit = net.port_ref::<Hero>(level, "Exit Hall").unwrap();
            net.put(exit, hero("Cy", true))
        });
        lines
            .iter()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    };
    let added = outer(&lines, "added");
    assert_eq!(
        (&added[0]["place"], &added[0]["port"]),
        (&json!("Level 2"), &json!("Exit Hall"))
    );
    assert!(added[0]["subnet_marking_versions"].get("Level 2").is_some());
    quiet.change(now(), |net| {
        let level = net.place_named("Level 2").unwrap();
        let exit = net.port_ref::<Hero>(level, "Exit Hall").unwrap();
        net.put(exit, hero("Cy", true))
    });

    quiet.set_listening(true);
    assert_eq!(
        read_all(&mut quiet, &quiet_ids),
        read_all(&mut watched, &watched_ids)
    );
    // From the switch on, the two tell alike: Climb, level 2's three, Leave
    // and Leave again, and then nothing.
    for _ in 0..7 {
        let told = |lines: Vec<String>, ids: &[String]| -> Vec<String> {
            lines.iter().map(|line| alike(line, ids)).collect()
        };
        assert_eq!(
            told(step(&mut quiet), &quiet_ids),
            told(step(&mut watched), &watched_ids)
        );
        assert_eq!(
            read_all(&mut quiet, &quiet_ids),
            read_all(&mut watched, &watched_ids)
        );
    }
    let state: Value =
        serde_json::from_str(&watched.request(r#"{"id":"s","method":"get_state"}"#, now())[0])
            .unwrap();
    assert_eq!(state["result"]["marking"]["places"]["Outside"]["count"], 2);
}

#[test]
fn the_program_may_change_a_subnet_through_its_scope_and_the_subnet_tells_it() {
    let (mut host, ids) = hosted();
    let before: Value =
        serde_json::from_str(&host.request(r#"{"id":"s","method":"get_state"}"#, now())[0])
            .unwrap();
    let world = before["result"]["marking_version"].as_u64().unwrap();
    let (_, lines) = host.change(now(), |net| {
        let level = net.place_named("Level 2").unwrap();
        net.subnet_scope(level, |inside| {
            let lobby = inside.place_ref::<Hero>("Lobby").unwrap();
            inside.put(lobby, hero("Di", false))
        })
        .expect("nothing is in flight")
    });
    let lines: Vec<Value> = lines
        .iter()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(sequence(&lines), ["nested:added", "nested:fireable"]);
    let (wrapper, added) = inner(&lines, "added")[0];
    assert_eq!(
        (&wrapper["path"], &wrapper["nested_id"]),
        (&l2(), &json!(ids[1]))
    );
    assert_eq!(added["place"], "Lobby");
    assert!(
        added.get("port").is_none(),
        "the subnet names its own place alone"
    );
    assert_eq!(added["outer_marking_version"], world + 1);
    assert_eq!(wrapper["marking_version"], world + 1);
}
