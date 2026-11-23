//! Closed nets on the wire (PROTOCOL.md §3, §5.1, §6.4, §12; DESIGN §6.9,
//! since 2026-11-23), `Owner` driven directly with a fixed clock: `inject`
//! answers `closed_net` at a place of a closed net that is not one of its
//! ports, at the top level and by path, and lands at a port and in a net
//! declared open, whose topology says `open: true`; and the `post-office`
//! conformance net, step by step, as the nineteenth shared recorded
//! fixture goes.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use petritype::{Net, NetBuilder, TakeEdge};
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
        "a reader knows it is closed by its port"
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
    // `levels`: the world marks no port and is open; each level is closed.
    let mut h = H::loaded(nets::levels_registry(), "levels");
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

// ----- post-office ---------------------------------------------------------------

#[test]
fn the_post_office_delivers_at_a_port_refuses_an_inject_into_its_ledger_and_a_forgery() {
    let mut h = H::loaded(nets::post_office_registry(), "post-office");
    let state = h.result("get_state", json!({}));
    assert!(
        state["topology"].get("open").is_none(),
        "it marks no port, and is open without saying so"
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
