//! Groups on the wire (PROTOCOL.md §5.1, DESIGN §19.3.18), `Owner` driven
//! directly with a fixed clock: `topology.groups` and `group` on places,
//! subnet places and transitions, for the loaded net and for every inner
//! net, each written only when set. The shared transcript `11_groups.json`
//! shows them over the conformance net `groups`; these tests add the nested
//! id, a net a token carries, nets with no groups at any depth, and that a
//! firing and the log name no group.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use petritype::{Net, NetBuilder, Params};
use petritype_runner::protocol::Request;
use petritype_runner::{Input, LoadError, NetEntry, Outgoing, Owner};
use serde_json::{Value, json};

#[path = "../examples/parcel_runner/nets.rs"]
#[allow(dead_code)]
mod nets;

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
                Outgoing::Launch(firing) => lines.extend(self.feed(Input::Completed(firing.run()))),
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
}

fn loaded_groups() -> H {
    H::loaded(nets::groups_registry(), "groups")
}

fn the_box() -> Value {
    json!([{"place": "Box"}])
}

/// The entry named `name` among `entries`, a topology's places or
/// transitions.
fn named<'a>(entries: &'a Value, name: &str) -> &'a Value {
    entries
        .as_array()
        .expect("a list")
        .iter()
        .find(|entry| entry["name"] == name)
        .unwrap_or_else(|| panic!("no {name:?} in {entries}"))
}

/// Whether `group` or `groups` is a key anywhere in `value`.
fn mentions_group(value: &Value) -> bool {
    match value {
        Value::Object(map) => {
            map.contains_key("group")
                || map.contains_key("groups")
                || map.values().any(mentions_group)
        }
        Value::Array(items) => items.iter().any(mentions_group),
        _ => false,
    }
}

#[test]
fn the_loaded_net_s_topology_lists_its_groups_and_each_member_s_group() {
    let mut h = loaded_groups();
    let topology = h.result("get_state", json!({}))["topology"].clone();
    assert_eq!(topology["groups"], json!(["Sorting", "Packing"]));
    let places = &topology["places"];
    assert!(
        named(places, "Inbox").get("group").is_none(),
        "a place in no group carries no group"
    );
    assert_eq!(named(places, "Sorted")["group"], "Sorting");
    let the_box = named(places, "Box");
    assert_eq!(the_box["group"], "Packing");
    assert_eq!(the_box["subnet"], true);
    assert_eq!(
        the_box["ports"],
        json!([{"name": "Tray", "type_name": "alloc::string::String", "serializable": true}]),
        "a port listed in its subnet place carries no group: its group is the subnet's"
    );
    let transitions = &topology["transitions"];
    assert_eq!(named(transitions, "Sort")["group"], "Sorting");
    assert_eq!(named(transitions, "Pack")["group"], "Packing");
}

#[test]
fn an_inner_net_s_topology_lists_its_own_groups_by_path_and_by_nested_id() {
    let mut h = loaded_groups();
    let outer = h.result("get_state", json!({}));
    let nested_id = outer["marking"]["places"]["Box"]["subnet"]["nested_id"].clone();
    for address in [
        json!({"path": the_box()}),
        json!({ "nested_id": nested_id }),
    ] {
        let topology = h.result("get_state", address)["topology"].clone();
        assert_eq!(
            topology["groups"],
            json!(["Sealing"]),
            "the subnet's own groups, none of the net's"
        );
        let places = &topology["places"];
        assert_eq!(
            named(places, "Tray"),
            &json!({"name": "Tray", "type_name": "alloc::string::String", "serializable": true,
                    "port": true, "group": "Sealing"})
        );
        assert!(named(places, "Sealed").get("group").is_none());
        assert_eq!(named(&topology["transitions"], "Seal")["group"], "Sealing");
    }
}

/// `crates`: one place of nets, `Crates`, holding a net the template `Box`
/// builds, and no groups of its own.
fn crates(_: &Params) -> Result<Net, LoadError> {
    let templates = nets::groups::templates();
    let held = templates
        .build("Box", &Params::new())
        .map_err(|error| LoadError::Other(error.to_string()))?;
    let mut b = NetBuilder::new();
    b.templates(&templates);
    let crates = b.place_nested::<Net>("Crates");
    b.seed(crates, [held]);
    Ok(b.build()?)
}

#[test]
fn a_net_a_token_carries_shows_its_own_groups_and_the_net_around_it_none() {
    let mut h = H::loaded(vec![NetEntry::new("crates", crates)], "crates");
    let outer = h.result("get_state", json!({}));
    assert!(
        !mentions_group(&outer["topology"]),
        "the net around the token declares no group"
    );
    let held = &outer["marking"]["places"]["Crates"]["tokens"][0]["nets"][0];
    let path = held["path"].clone();
    let nested_id = held["nested_id"].clone();
    for address in [json!({ "path": path }), json!({ "nested_id": nested_id })] {
        let topology = h.result("get_state", address)["topology"].clone();
        assert_eq!(topology["groups"], json!(["Sealing"]));
        let tray = named(&topology["places"], "Tray");
        assert_eq!(tray["group"], "Sealing");
        assert_eq!(
            tray["port"], true,
            "a token's net shows its own ports, as every net does since 2026-11-23"
        );
        assert_eq!(named(&topology["transitions"], "Seal")["group"], "Sealing");
    }
}

#[test]
fn a_net_with_no_groups_shows_no_groups_and_no_group_at_any_depth() {
    let mut h = H::loaded(nets::levels_registry(), "levels");
    let state = h.result("get_state", json!({}));
    assert!(!mentions_group(&state["topology"]));
    let inner = h.result("get_state", json!({"path": [{"place": "Level 1"}]}));
    assert!(!mentions_group(&inner["topology"]), "nor a subnet");

    let mut h = H::loaded(nets::transit_registry(), "transit");
    let state = h.result("get_state", json!({}));
    assert!(!mentions_group(&state["topology"]));
    let held = &state["marking"]["places"]["Depot"]["tokens"][0]["nets"][0];
    let inner = h.result("get_state", json!({ "path": held["path"] }));
    assert!(!mentions_group(&inner["topology"]), "nor a token's net");
}

#[test]
fn a_firing_and_the_log_name_no_group() {
    let mut h = loaded_groups();
    let mut lines = Vec::new();
    // Sort, Pack into the box's tray, and the box's own Seal.
    for _ in 0..3 {
        lines.extend(h.send("step", json!({})));
    }
    let fired: Vec<&Value> = lines
        .iter()
        .filter_map(|line| line.get("result"))
        .map(|result| &result["transition"])
        .collect();
    assert_eq!(fired, [&json!("Sort"), &json!("Pack"), &json!("Seal")]);
    lines.extend(h.send("get_log", json!({})));
    lines.extend(h.send("get_place_tokens", json!({"place": "Sorted"})));
    lines.extend(h.send("get_state", json!({"path": the_box()})));
    let without_topology: Vec<Value> = lines
        .into_iter()
        .map(|mut line| {
            if let Some(result) = line.get_mut("result").and_then(Value::as_object_mut) {
                result.remove("topology");
            }
            line
        })
        .collect();
    assert!(
        !mentions_group(&Value::Array(without_topology)),
        "only a topology names a group"
    );
}
