//! Closed nets on the wire (PROTOCOL.md §3, §5.1, §6.4, §12; DESIGN §6.9,
//! since 2026-11-23), `Owner` driven directly with a fixed clock: `inject`
//! answers `closed_net` at a place of a closed net that is not one of its
//! ports, at the top level and by path, and lands at a port and in a net
//! declared open, whose topology says `open: true`; inside a closed net
//! (since 2026-11-28) it answers `closed_net` at any place of the nets
//! inside, by path and by nested id alike, and a body's write at a port its
//! edge does not name fails its firing; and the `post-office` conformance
//! net, step by step, as the nineteenth shared recorded fixture goes.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use petritype::{
    MutateEdge, Nested, Net, NetBuilder, NetError, Params, Stepping, TakeEdge, Templates,
};
use petritype_runner::protocol::Request;
use petritype_runner::{Input, LoadError, NetEntry, Outgoing, Owner};
use serde_json::{Value, json};

#[path = "../examples/parcel_runner/nets.rs"]
#[allow(dead_code)]
mod nets;

// ----- the harness -------------------------------------------------------------

fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(1_795_770_000_000)
}

/// An owner over some nets: ids `"1"`, `"2"`, ... in order, and every
/// launch landed at once.
struct H {
    owner: Owner,
    next: u64,
}

impl H {
    fn loaded(nets: Vec<NetEntry>, net: &str) -> H {
        let mut h = H {
            owner: Owner::new(nets, Arc::new(Mutex::new(None)), Duration::from_secs(30)),
            next: 0,
        };
        let lines = h.send("load", json!({ "net": net }));
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
                Outgoing::Launch(firing) => {
                    lines.extend(self.feed(Input::Completed(firing.run())));
                }
                Outgoing::Exit => {}
            }
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

    fn result(&mut self, method: &str, params: Value) -> Value {
        let lines = self.send(method, params);
        let last = lines.last().cloned().expect("a response");
        assert!(last.get("result").is_some(), "{method}: {lines:#?}");
        last["result"].clone()
    }

    /// The error a request was answered with, and the lines before it.
    fn error(&mut self, method: &str, params: Value) -> (Value, Vec<Value>) {
        let mut lines = self.send(method, params);
        let last = lines.pop().expect("a response");
        assert!(last.get("error").is_some(), "{method}: {last:#?}");
        (last["error"].clone(), lines)
    }
}

/// The names of the tokens of `place` in a state's marking.
fn labels(state: &Value, place: &str) -> Vec<String> {
    state["marking"]["places"][place]["tokens"]
        .as_array()
        .unwrap_or_else(|| panic!("{place} in {state:#}"))
        .iter()
        .map(|token| token["label"].as_str().expect("a label").to_owned())
        .collect()
}

// ----- the top level -----------------------------------------------------------

/// A closed net at the top level: `Inbox`, its port, and `Ledger`; and a
/// subnet place `Level`, a closed level whose `Lobby` is its port.
fn desk(_: &petritype_runner::Params) -> Result<Net, LoadError> {
    let mut level = NetBuilder::new();
    let lobby = level.place::<String>("Lobby");
    level.serializable(lobby);
    level.port(lobby);
    let level = level.build()?;
    let mut b = NetBuilder::new();
    let inbox = b.place::<String>("Inbox");
    let ledger = b.place::<String>("Ledger");
    b.serializable(inbox);
    b.serializable(ledger);
    b.port(inbox);
    let level = b.subnet_place("Level", level);
    let lobby = b.port_of::<String>(level, "Lobby");
    b.transition("File", (TakeEdge(inbox),), (ledger,), |letter: String| {
        (letter,)
    })
    .guard(|_| false);
    let _ = lobby;
    Ok(b.build()?)
}

/// The same desk, declared open.
fn open_desk(params: &petritype_runner::Params) -> Result<Net, LoadError> {
    let mut b = NetBuilder::new();
    let inbox = b.place::<String>("Inbox");
    let ledger = b.place::<String>("Ledger");
    b.serializable(inbox);
    b.serializable(ledger);
    b.port(inbox);
    b.open();
    let _ = params;
    Ok(b.build()?)
}

fn desks() -> Vec<NetEntry> {
    vec![
        NetEntry::new("desk", desk),
        NetEntry::new("open-desk", open_desk),
    ]
}

#[test]
fn an_inject_at_the_top_level_lands_at_a_port_and_is_closed_net_anywhere_else() {
    let mut h = H::loaded(desks(), "desk");
    let landed = h.result("inject", json!({"place": "Inbox", "token": "Dear Ada"}));
    assert_eq!(landed["status"], "injected");

    let before = h.result("get_state", json!({}))["marking_version"].clone();
    let (error, lines) = h.error("inject", json!({"place": "Ledger", "token": "IOU"}));
    assert_eq!(error["code"], "closed_net");
    let message = error["message"].as_str().expect("a message");
    assert!(
        message.contains("\"Ledger\"") && message.contains("declare the net open"),
        "{message}"
    );
    assert!(lines.is_empty(), "nothing was told: {lines:#?}");

    // A window of a closed net is a place of that net, and never its port.
    let (error, _) = h.error(
        "inject",
        json!({"place": "Level", "port": "Lobby", "token": "Bram"}),
    );
    assert_eq!(error["code"], "closed_net");

    let state = h.result("get_state", json!({}));
    assert_eq!(state["marking_version"], before, "nothing changed");
    assert_eq!(labels(&state, "Ledger"), Vec::<String>::new());
    assert!(
        state["topology"].get("open").is_none(),
        "a closed net writes no open"
    );
    assert_eq!(
        state["topology"]["places"][0]["port"], true,
        "Inbox, its port, where a write from outside lands"
    );
}

#[test]
fn an_inject_into_a_net_declared_open_lands_anywhere_and_its_topology_says_so() {
    let mut h = H::loaded(desks(), "open-desk");
    let landed = h.result("inject", json!({"place": "Ledger", "token": "IOU"}));
    assert_eq!(landed["status"], "injected");
    let state = h.result("get_state", json!({}));
    assert_eq!(state["topology"]["open"], true);
    assert_eq!(labels(&state, "Ledger"), ["\"IOU\""]);
}

#[test]
fn an_inject_by_path_into_a_subnet_is_held_to_its_ports() {
    // The world of `levels` declared open, so that the subnet's own rule
    // is what holds the inject; each level is closed.
    let levels = vec![NetEntry::new("open-levels", nets::levels::open_levels)];
    let mut h = H::loaded(levels, "open-levels");
    let level = json!([{"place": "Level 1"}]);
    let landed = h.result(
        "inject",
        json!({"place": "Lobby", "path": level, "token": {"name": "Bram", "fought": false}}),
    );
    assert_eq!(landed["status"], "injected");
    let (error, _) = h.error(
        "inject",
        json!({"place": "Arena", "path": level, "token": {"name": "Cato", "fought": false}}),
    );
    assert_eq!(error["code"], "closed_net");
    let inside = h.result("get_state", json!({ "path": level }));
    assert!(inside["topology"].get("open").is_none());
    assert_eq!(inside["marking"]["places"]["Arena"]["count"], 0);
}

// ----- a closed net shields the nets inside it (since 2026-11-28) --------------

/// The template `Post`: `Inbox`, its port, and `Ledger`, both of strings
/// and serializable, and no transitions.
fn post_templates() -> Templates {
    let mut templates = Templates::new();
    templates.register("Post", |_: &Params| {
        let mut b = NetBuilder::new();
        let inbox = b.place::<String>("Inbox");
        let ledger = b.place::<String>("Ledger");
        b.serializable(inbox);
        b.serializable(ledger);
        b.port(inbox);
        b.build()
    });
    templates
}

/// A sorting office: `Sacks`, a place of nets (passive), holding a `Post`,
/// and the subnet place `Branch`, holding another. Declared open when
/// `open` is.
fn sorting_office(open: bool) -> Result<Net, LoadError> {
    let templates = post_templates();
    let build = || {
        templates
            .build("Post", &petritype::Params::new())
            .map_err(|error| LoadError::Other(error.to_string()))
    };
    let mut b = NetBuilder::new();
    b.templates(&templates);
    let sacks = b.place_nested::<Net>("Sacks");
    b.serializable(sacks);
    b.stepping(sacks, Stepping::Passive);
    b.seed(sacks, [build()?]);
    b.subnet_place("Branch", build()?);
    if open {
        b.open();
    }
    Ok(b.build()?)
}

fn offices() -> Vec<NetEntry> {
    vec![
        NetEntry::new("sorting-office", |_| sorting_office(false)),
        NetEntry::new("open-sorting-office", |_| sorting_office(true)),
    ]
}

#[test]
fn an_inject_by_path_and_by_nested_id_lands_inside_an_open_net_only() {
    // Into the post in a sack and the post in the subnet place, each
    // addressed both ways: inside the closed office each answers
    // `closed_net`, naming the office, and nothing is added; inside the
    // open one each lands.
    for open in [false, true] {
        let name = if open {
            "open-sorting-office"
        } else {
            "sorting-office"
        };
        let mut h = H::loaded(offices(), name);
        let places = h.result("get_state", json!({}))["marking"]["places"].clone();
        let carried = places["Sacks"]["tokens"][0]["nets"][0].clone();
        let subnet = places["Branch"]["subnet"].clone();
        for net in [carried, subnet] {
            for address in [
                json!({"path": net["path"]}),
                json!({"nested_id": net["nested_id"]}),
            ] {
                let mut params = json!({"place": "Inbox", "token": "Letter"});
                params
                    .as_object_mut()
                    .unwrap()
                    .extend(address.as_object().unwrap().clone());
                if open {
                    let landed = h.result("inject", params);
                    assert_eq!(landed["status"], "injected", "{address}");
                } else {
                    let (error, lines) = h.error("inject", params);
                    assert_eq!(error["code"], "closed_net", "{address}");
                    let message = error["message"].as_str().expect("a message");
                    assert!(
                        message.contains("of net \"Post\": the net is inside net")
                            && message.contains("which is closed"),
                        "{message}"
                    );
                    assert!(lines.is_empty(), "nothing was told: {lines:#?}");
                }
            }
            // The place and the token are answered first, inside a closed
            // net as anywhere.
            let (error, _) = h.error(
                "inject",
                json!({"place": "Nowhere", "token": "Letter", "path": net["path"]}),
            );
            assert_eq!(error["code"], "unknown_place");
            let (error, _) = h.error(
                "inject",
                json!({"place": "Inbox", "token": 7, "path": net["path"]}),
            );
            assert_eq!(error["code"], "invalid_token");
            let inside = h.result("get_state", json!({"nested_id": net["nested_id"]}));
            assert_eq!(
                inside["marking"]["places"]["Inbox"]["count"],
                if open { 2 } else { 0 }
            );
        }
    }
}

/// A unit: `Orders In` and `Shots Out`, both serializable ports.
fn unit_templates() -> Templates {
    let mut templates = Templates::new();
    templates.register("Unit", |_: &Params| {
        let mut b = NetBuilder::new();
        let orders_in = b.place::<String>("Orders In");
        let shots_out = b.place::<String>("Shots Out");
        b.serializable(orders_in);
        b.serializable(shots_out);
        b.port(orders_in);
        b.port(shots_out);
        b.build()
    });
    templates
}

/// `use`: `Orders` holding `"Hold"`, and `Field` holding a unit; `Use`
/// takes the order and lends the body the unit through an edge that names
/// `Orders In`, and the body puts the order into `Shots Out`, which its
/// edge does not name. Declared open when `open` is.
fn field(open: bool) -> Result<Net, LoadError> {
    let templates = unit_templates();
    let unit = templates
        .build("Unit", &petritype::Params::new())
        .map_err(|error| LoadError::Other(error.to_string()))?;
    let mut b = NetBuilder::new();
    b.templates(&templates);
    let orders = b.place::<String>("Orders");
    let field = b.place_nested::<Net>("Field");
    b.serializable(orders);
    b.serializable(field);
    b.seed(orders, ["Hold".to_owned()]);
    b.seed(field, [unit]);
    b.stepping(field, Stepping::Passive);
    if open {
        b.open();
    }
    b.transition(
        "Use",
        (TakeEdge(orders), MutateEdge(field).reaching(["Orders In"])),
        (),
        |order: String, unit: &mut Net| -> Result<(), NetError> {
            let shots = unit.place_ref::<String>("Shots Out").expect("a unit");
            unit.try_put(shots, order).map(drop)
        },
    );
    Ok(b.build()?)
}

#[test]
fn the_runner_fails_the_body_s_write_and_injects_at_the_same_port_inside_an_open_net_only() {
    for open in [false, true] {
        let entries = vec![NetEntry::new("use", move |_| field(open))];
        let mut h = H::loaded(entries, "use");
        let lines = h.send("step", json!({}));
        let failed = lines
            .iter()
            .find(|line| line["method"] == "failed")
            .unwrap_or_else(|| panic!("a failed firing: {lines:#?}"));
        let error = failed["params"]["error"].as_str().expect("an error");
        assert!(
            error.contains("\"Shots Out\"") && error.contains(".reaching([\"Shots Out\"])"),
            "{error}"
        );
        let state = h.result("get_state", json!({}));
        let held = state["marking"]["places"]["Field"]["tokens"][0]["nets"][0].clone();
        let params = json!({"place": "Shots Out", "path": held["path"], "token": "Shot"});
        if open {
            assert_eq!(h.result("inject", params)["status"], "injected");
        } else {
            let (error, _) = h.error("inject", params);
            assert_eq!(error["code"], "closed_net");
            assert!(
                error["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("is inside net")),
                "{error}"
            );
        }
        let inside = h.result("get_state", json!({"path": held["path"]}));
        assert_eq!(
            inside["marking"]["places"]["Shots Out"]["count"],
            if open { 1 } else { 0 }
        );
    }
}

// ----- a body replaces the net of a token it was lent ----------------------------

/// A courier, a token that carries a net, its round.
#[derive(Debug)]
struct Courier {
    round: Net,
}

impl Nested for Courier {
    fn nets(&self) -> Vec<(&str, &Net)> {
        vec![("round", &self.round)]
    }
    fn nets_mut(&mut self) -> Vec<(&str, &mut Net)> {
        vec![("round", &mut self.round)]
    }
}

/// A round: `Inbox`, its port, started with `inbox`, and `File`, the
/// round's own move, from there to `Ledger`.
fn round(inbox: &[&str]) -> Result<Net, petritype::BuildError> {
    let mut b = NetBuilder::new();
    let inbox_place = b.place::<String>("Inbox");
    let ledger = b.place::<String>("Ledger");
    b.serializable(inbox_place);
    b.serializable(ledger);
    b.port(inbox_place);
    b.seed(inbox_place, inbox.iter().map(|letter| (*letter).to_owned()));
    b.transition(
        "File",
        (TakeEdge(inbox_place),),
        (ledger,),
        |letter: String| (letter,),
    );
    b.build()
}

/// `test-depot`: a closed depot whose `Restart` gives the courier it is
/// lent a new round, with a parcel in its inbox.
fn depot(_: &petritype_runner::Params) -> Result<Net, LoadError> {
    let mut b = NetBuilder::new();
    let couriers = b.place_nested::<Courier>("Couriers");
    let orders = b.place::<String>("Orders");
    let done = b.place::<String>("Done");
    b.serializable(orders);
    b.serializable(done);
    b.port(orders);
    b.seed(couriers, [Courier { round: round(&[])? }]);
    b.seed(orders, ["Restart".to_owned()]);
    b.transition(
        "Restart",
        (MutateEdge(couriers), TakeEdge(orders)),
        (done,),
        |courier: &mut Courier, order: String| {
            courier.round = round(&["Parcel"]).expect("a round");
            (order,)
        },
    );
    Ok(b.build()?)
}

#[test]
fn a_net_a_body_puts_in_a_token_it_was_lent_has_a_new_nested_id_and_the_old_one_is_unknown() {
    let mut h = H::loaded(vec![NetEntry::new("test-depot", depot)], "test-depot");
    let round_of_the_courier = |h: &mut H| {
        let state = h.result("get_state", json!({}));
        let nets = &state["marking"]["places"]["Couriers"]["tokens"][0]["nets"];
        assert_eq!(nets.as_array().map(Vec::len), Some(1), "{nets:#}");
        nets[0].clone()
    };
    let before = round_of_the_courier(&mut h);
    assert_eq!(h.result("step", json!({}))["transition"], "Restart");
    let after = round_of_the_courier(&mut h);
    assert_ne!(after["nested_id"], before["nested_id"]);
    assert_eq!(
        after["path"], before["path"],
        "the same token, the same net name"
    );
    let (error, _) = h.error("get_state", json!({"nested_id": before["nested_id"]}));
    assert_eq!(error["code"], "unknown_net");
    let new = h.result("get_state", json!({"nested_id": after["nested_id"]}));
    assert_eq!(new["marking"]["places"]["Inbox"]["count"], 1);
    let stepped = h.result("step", json!({}));
    assert_eq!(stepped["nested_id"], after["nested_id"]);
    assert_eq!(stepped["transition"], "File");
}

// ----- post-office ---------------------------------------------------------------

#[test]
fn the_post_office_delivers_at_a_port_refuses_an_inject_into_its_ledger_and_a_forgery() {
    let mut h = H::loaded(nets::post_office_registry(), "post-office");
    let state = h.result("get_state", json!({}));
    assert_eq!(
        state["topology"]["open"], true,
        "declared open, so that each post is judged by its own rule"
    );
    let tokens = state["marking"]["places"]["Posts"]["tokens"]
        .as_array()
        .expect("two posts")
        .clone();
    let path = |post: &Value| json!([{"place": "Posts", "token": post["id"], "net": ""}]);
    let (post, open_post) = (path(&tokens[0]), path(&tokens[1]));

    let first = h.result("get_state", json!({ "path": post }));
    assert_eq!(first["template"], "Post");
    assert_eq!(first["topology"]["places"][0]["port"], true, "Inbox");
    assert!(
        first["topology"]["places"][1].get("port").is_none(),
        "Ledger"
    );
    assert!(first["topology"].get("open").is_none(), "closed");
    let second = h.result("get_state", json!({ "path": open_post }));
    assert_eq!(second["template"], "Open Post");
    assert_eq!(second["topology"]["open"], true);

    // Deliver: the letter lands in the post's inbox, its port.
    let delivered = h.result("step", json!({}));
    assert_eq!(delivered["transition"], "Deliver");
    assert!(delivered.get("failed").is_none(), "{delivered:#}");

    // Inject by path: at the closed post's inbox, then its ledger, then
    // the open post's ledger.
    let landed = h.result(
        "inject",
        json!({"place": "Inbox", "path": post, "token": "Note"}),
    );
    assert_eq!(landed["status"], "injected");
    let (error, lines) = h.error(
        "inject",
        json!({"place": "Ledger", "path": post, "token": "Note"}),
    );
    assert_eq!(error["code"], "closed_net");
    assert!(lines.is_empty(), "nothing was told: {lines:#?}");
    let landed = h.result(
        "inject",
        json!({"place": "Ledger", "path": open_post, "token": "Note"}),
    );
    assert_eq!(landed["status"], "injected");
    let first = h.result("get_state", json!({ "path": post }));
    assert_eq!(labels(&first, "Inbox"), ["\"Dear Ada\"", "\"Note\""]);
    assert_eq!(labels(&first, "Ledger"), Vec::<String>::new());

    // Forge: a detachable firing, launched, then failed at the closed
    // post's ledger, losing what it took, as any failure without a
    // restore does.
    let lines = h.send("step", json!({}));
    let methods: Vec<&str> = lines
        .iter()
        .filter_map(|line| line["method"].as_str())
        .collect();
    assert_eq!(&methods[..2], ["launched", "failed"], "{lines:#?}");
    let failed = &lines[1]["params"];
    assert_eq!(failed["transition"], "Forge");
    assert_eq!(failed["restored"], false);
    assert!(
        !lines.iter().any(|line| line["method"] == "nested"),
        "the refused write told nothing: {lines:#?}"
    );
    let answer = lines.last().expect("the answer");
    assert_eq!(answer["result"]["failed"], true);

    let state = h.result("get_state", json!({}));
    assert_eq!(labels(&state, "Forgeries"), Vec::<String>::new());
    let posts = state["marking"]["places"]["Posts"]["tokens"]
        .as_array()
        .expect("the posts");
    assert_eq!(posts.len(), 1, "only the open post is left");
    assert_eq!(posts[0]["id"], tokens[1]["id"]);
}
