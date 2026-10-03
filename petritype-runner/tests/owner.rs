//! `Owner` driven directly, with a fixed clock: every method, every error
//! code, and the orderings the protocol promises (DESIGN §19.3.8).

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use petritype::{Firing, MutateEdge, Net, NetBuilder, Param, TakeEdge};
use petritype_runner::protocol::Request;
use petritype_runner::{
    Hosting, Input, LoadError, NetEntry, Outgoing, Owner, ParamSpec, Params, StatusCell,
};
use serde_json::{Value, json};

// ----- the nets ----------------------------------------------------------------

#[derive(Debug)]
struct Refused;

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("refused")
    }
}

impl std::error::Error for Refused {}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Limit(u32);

/// `Inbox -Double-> Mid -Tally(mutates Total)-> Done`, plus `Bad -Fail->`,
/// `Panics -Boom->`, `Odd -Odd->` (a priority that panics), a long `Notes`
/// token and a `limit` parameter.
fn pipeline(_: &Params) -> Result<Net, LoadError> {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let mid = b.place::<u32>("Mid");
    let total = b.place::<u64>("Total");
    let done = b.place::<u32>("Done");
    let bad = b.place::<u32>("Bad");
    let panics = b.place::<u32>("Panics");
    let odd = b.place::<u32>("Odd");
    let notes = b.place::<String>("Notes");
    b.serializable(inbox);
    b.serializable(bad);
    b.serializable(panics);
    b.seed(inbox, [1, 2]);
    b.seed(total, [0]);
    b.seed(notes, ["z".repeat(1500)]);
    b.param(Limit(10));
    b.serializable_param::<Limit>("limit");
    b.transition(
        "Double",
        (TakeEdge(inbox), Param::<Limit>::new()),
        (mid,),
        |n: u32, _: Limit| (n * 2,),
    );
    b.transition(
        "Tally",
        (TakeEdge(mid), MutateEdge(total)),
        (done,),
        |n: u32, total: &mut u64| {
            *total += u64::from(n);
            (n,)
        },
    );
    b.transition(
        "Fail",
        (TakeEdge(bad),),
        (done,),
        |_: u32| -> Result<(u32,), Refused> { Err(Refused) },
    );
    b.transition("Boom", (TakeEdge(panics),), (done,), |_: u32| -> (u32,) {
        panic!("kaboom")
    });
    b.transition("Odd", (TakeEdge(odd),), (done, mid), |n: u32| (n, n))
        .priority(|_| panic!("no priority today"));
    Ok(b.build()?)
}

/// `Inbox -Echo-> Out`, empty, hosted 24/7.
fn watcher(_: &Params) -> Result<Net, LoadError> {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.serializable(inbox);
    b.transition("Echo", (TakeEdge(inbox),), (out,), |n: u32| (n,));
    Ok(b.build()?)
}

/// `Ticks -Bump(mutates Counter)->`: only inline firings.
fn bumper(_: &Params) -> Result<Net, LoadError> {
    let mut b = NetBuilder::new();
    let ticks = b.place::<u32>("Ticks");
    let counter = b.place::<u64>("Counter");
    b.seed(ticks, [1, 2, 3]);
    b.seed(counter, [0]);
    b.transition(
        "Bump",
        (TakeEdge(ticks), MutateEdge(counter)),
        (),
        |_: u32, c: &mut u64| *c += 1,
    );
    Ok(b.build()?)
}

fn broken(_: &Params) -> Result<Net, LoadError> {
    let mut b = NetBuilder::new();
    b.place::<u32>("Twice");
    b.place::<u32>("Twice");
    Ok(b.build()?)
}

fn panicky(_: &Params) -> Result<Net, LoadError> {
    panic!("the factory fell over")
}

#[derive(serde::Deserialize)]
struct Sized_ {
    n: u32,
}

fn picky(params: Sized_) -> Result<Net, LoadError> {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    b.seed(inbox, 0..params.n);
    Ok(b.build()?)
}

fn registry() -> Vec<NetEntry> {
    vec![
        NetEntry::new("pipeline", pipeline)
            .description("numbers")
            .param(ParamSpec::new("size", "u32").default(2)),
        NetEntry::new("watcher", watcher).hosting(Hosting::Continuous),
        NetEntry::new("bumper", bumper).hosting(Hosting::Batch),
        NetEntry::new("broken", broken),
        NetEntry::new("panicky", panicky).hosting(Hosting::Cron("*/5 * * * *")),
        NetEntry::typed("picky", picky).param(ParamSpec::new("n", "u32").required()),
    ]
}

// ----- the harness -----------------------------------------------------------

/// The fixed clock: 2026-10-03T09:35:02.413Z.
fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(1_791_020_102_413)
}

struct H {
    owner: Owner,
    status: StatusCell,
    next: u64,
    /// Run each `Launch` at once and feed it back.
    auto: bool,
    held: Vec<Firing>,
}

impl H {
    fn new() -> H {
        let status: StatusCell = Arc::new(Mutex::new(None));
        H {
            owner: Owner::new(registry(), Arc::clone(&status), Duration::from_secs(30)),
            status,
            next: 0,
            auto: true,
            held: Vec::new(),
        }
    }

    fn loaded(net: &str) -> H {
        let mut h = H::new();
        let lines = h.send("load", json!({ "net": net }));
        assert_eq!(
            lines.last().unwrap()["result"]["status"],
            "loaded",
            "{lines:?}"
        );
        h
    }

    fn feed(&mut self, input: Input) -> Vec<Value> {
        let mut lines = Vec::new();
        for outgoing in self.owner.handle(input, now()) {
            match outgoing {
                Outgoing::Line(line) => {
                    lines.push(serde_json::from_str(&line).expect("every line is JSON"));
                }
                Outgoing::Launch(firing) => {
                    if self.auto {
                        let more = self.feed(Input::Completed(firing.run()));
                        lines.extend(more);
                    } else {
                        self.held.push(firing);
                    }
                }
                Outgoing::Exit => lines.push(json!("EXIT")),
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

    fn land(&mut self) -> Vec<Value> {
        let firing = self.held.remove(0);
        self.feed(Input::Completed(firing.run()))
    }

    fn tick(&mut self) -> Vec<Value> {
        self.feed(Input::Tick)
    }

    /// The last line, which must be the response to the last request.
    fn result(&mut self, method: &str, params: Value) -> Value {
        let lines = self.send(method, params);
        let last = lines.last().cloned().expect("a response");
        assert_eq!(last["id"], self.next.to_string(), "{lines:#?}");
        assert!(last.get("result").is_some(), "{last}");
        last["result"].clone()
    }

    fn error(&mut self, method: &str, params: Value) -> String {
        let lines = self.send(method, params);
        assert_eq!(lines.len(), 1, "{lines:#?}");
        assert_eq!(lines[0]["id"], self.next.to_string());
        lines[0]["error"]["code"]
            .as_str()
            .expect("an error")
            .to_owned()
    }
}

/// Each line as a tag: a notification's method, or `"#id"` for a response.
fn tags(lines: &[Value]) -> Vec<String> {
    lines
        .iter()
        .map(|line| match line.get("method") {
            Some(method) => method.as_str().unwrap().to_owned(),
            None if line.is_string() => line.as_str().unwrap().to_owned(),
            None => format!("#{}", line["id"].as_str().unwrap()),
        })
        .collect()
}

fn find<'a>(lines: &'a [Value], method: &str) -> &'a Value {
    &lines
        .iter()
        .find(|line| line["method"] == method)
        .unwrap_or_else(|| panic!("no {method} in {lines:#?}"))["params"]
}

fn versions(lines: &[Value]) -> Vec<u64> {
    lines
        .iter()
        .filter_map(|l| l["params"]["marking_version"].as_u64())
        .collect()
}

// ----- describe and the errors before load -----------------------------------

#[test]
fn describe_works_before_load() {
    let mut h = H::new();
    let d = h.result("describe", json!({}));
    assert_eq!(d["protocol_version"], 1);
    assert_eq!(d["runtime"], "rust");
    assert_eq!(d["runtime_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(d["nets"].as_array().unwrap().len(), 6);
    assert_eq!(
        d["nets"][0],
        json!({"name": "pipeline", "description": "numbers", "mode": "manual",
               "params": [{"name": "size", "type_name": "u32", "required": false, "default": 2}]})
    );
    assert_eq!(d["nets"][1]["mode"], "24/7");
    assert_eq!(d["nets"][2]["mode"], "batch");
    assert_eq!(d["nets"][4]["mode"], "cron");
    assert_eq!(d["nets"][4]["schedule"], "*/5 * * * *");
    assert_eq!(
        d["nets"][5]["params"][0],
        json!({"name": "n", "type_name": "u32", "required": true})
    );
    assert_eq!(serde_json::to_value(h.owner.describe()).unwrap(), d);
}

#[test]
fn every_method_but_describe_load_and_shutdown_needs_a_net() {
    let mut h = H::new();
    for method in [
        "get_state",
        "get_place_tokens",
        "get_token",
        "step",
        "start",
        "stop",
        "reset",
        "inject",
        "enable",
        "disable",
        "set_param",
        "get_log",
    ] {
        assert_eq!(h.error(method, json!({})), "not_loaded", "{method}");
    }
    assert_eq!(tags(&h.send("shutdown", json!({}))), ["#13", "EXIT"]);
}

#[test]
fn an_unknown_method_is_unknown_method_loaded_or_not() {
    let mut h = H::new();
    assert_eq!(h.error("fly", json!({})), "unknown_method");
    let mut h = H::loaded("pipeline");
    assert_eq!(h.error("fly", json!({})), "unknown_method");
}

#[test]
fn params_that_are_not_an_object_are_invalid_params() {
    let mut h = H::new();
    assert_eq!(h.error("describe", json!([1])), "invalid_params");
    assert_eq!(h.error("load", json!("pipeline")), "invalid_params");
}

// ----- load ------------------------------------------------------------------

#[test]
fn load_reports_progress_then_reloaded_then_answers() {
    let mut h = H::new();
    let lines = h.send("load", json!({"net": "pipeline", "params": {"size": 3}}));
    assert_eq!(tags(&lines), ["load_progress", "reloaded", "#1"]);
    assert_eq!(
        lines[0]["params"],
        json!({"step": "build", "message": "building pipeline", "marking_version": 0, "running": false})
    );
    assert_eq!(
        lines[1]["params"],
        json!({"generation": 1, "reason": "load", "marking_version": 0, "running": false})
    );
    assert_eq!(
        lines[2]["result"],
        json!({"status": "loaded", "generation": 1, "marking_version": 0})
    );
}

#[test]
fn load_errors() {
    let mut h = H::new();
    assert_eq!(h.error("load", json!({})), "invalid_params");
    assert_eq!(
        h.error(
            "load",
            json!({"module": "nets", "function": "build", "factory_params": {}})
        ),
        "invalid_params",
        "the Python-only form"
    );
    assert_eq!(h.error("load", json!({"net": "nope"})), "unknown_net");
    assert_eq!(
        h.error("load", json!({"net": "pipeline", "params": 3})),
        "invalid_params"
    );
    assert_eq!(
        h.error("load", json!({"net": "picky"})),
        "invalid_params",
        "n is required"
    );

    // These fail after `load_progress`, which goes out before the factory.
    for (params, code) in [
        (
            json!({"net": "picky", "params": {"n": "three"}}),
            "invalid_params",
        ),
        (json!({"net": "broken"}), "load_failed"),
        (json!({"net": "panicky"}), "load_failed"),
    ] {
        let lines = h.send("load", params.clone());
        assert_eq!(tags(&lines)[0], "load_progress");
        assert_eq!(lines[1]["error"]["code"], code, "{params}");
    }
    let lines = h.send("load", json!({"net": "panicky"}));
    let message = lines[1]["error"]["message"].as_str().unwrap();
    assert!(message.contains("the factory fell over"), "{message}");

    // Nothing loaded after all that, and the generation did not move.
    assert_eq!(h.error("get_state", json!({})), "not_loaded");
    let lines = h.send("load", json!({"net": "picky", "params": {"n": 2}}));
    assert_eq!(lines.last().unwrap()["result"]["generation"], 1);
}

#[test]
fn a_failed_load_keeps_the_previous_net() {
    let mut h = H::loaded("pipeline");
    h.send("load", json!({"net": "broken"}));
    let state = h.result("get_state", json!({}));
    assert_eq!(state["net"], "pipeline");
    assert_eq!(state["generation"], 1);
}

#[test]
fn generation_and_marking_version_across_load_and_reset() {
    let mut h = H::loaded("pipeline");
    h.send("step", json!({})); // launched, distributed: version 2
    h.send("disable", json!({"transition": "Fail"})); // toggled: 3
    let state = h.result("get_state", json!({}));
    assert_eq!(
        (
            state["generation"].clone(),
            state["marking_version"].clone()
        ),
        (json!(1), json!(3))
    );

    let lines = h.send("reset", json!({}));
    assert_eq!(tags(&lines), ["reloaded", "#5"]);
    assert_eq!(
        lines[0]["params"],
        json!({"generation": 2, "reason": "reset", "marking_version": 3, "running": false})
    );
    assert_eq!(
        lines[1]["result"],
        json!({"status": "reset", "generation": 2, "marking_version": 3})
    );
    let state = h.result("get_state", json!({}));
    assert_eq!(state["marking"]["places"]["Inbox"]["count"], 2, "rebuilt");
    assert_eq!(state["transitions"]["Fail"]["enabled"], true, "rebuilt");
    assert_eq!(state["progress"]["step_count"], 0);

    h.send("step", json!({}));
    let lines = h.send("load", json!({"net": "watcher"}));
    assert_eq!(lines[1]["params"]["generation"], 3);
    assert_eq!(
        lines[2]["result"],
        json!({"status": "loaded", "generation": 3, "marking_version": 5})
    );
}

// ----- reads ------------------------------------------------------------------

#[test]
fn get_state_has_the_protocol_shape() {
    let mut h = H::loaded("pipeline");
    let s = h.result("get_state", json!({}));
    assert_eq!(s["protocol_version"], 1);
    assert_eq!(s["runtime"], "rust");
    assert_eq!(s["net"], "pipeline");
    assert_eq!(s["running"], false);
    assert!(s["topology"].get("hints").is_none());
    assert_eq!(
        s["topology"]["places"][0],
        json!({"name": "Inbox", "type_name": "u32", "serializable": true})
    );
    assert_eq!(
        s["topology"]["transitions"][0],
        json!({"name": "Double", "detail": {"params": ["owner::Limit"], "detachable": true}})
    );
    assert_eq!(
        s["topology"]["transitions"][1]["detail"]["detachable"],
        false
    );
    let edges = s["topology"]["edges"].as_array().unwrap();
    assert_eq!(
        edges[0],
        json!({"place": "Inbox", "transition": "Double", "direction": "in", "kind": "take"})
    );
    assert_eq!(
        edges[1],
        json!({"place": "Mid", "transition": "Double", "direction": "out"}),
        "one output: no slot"
    );
    assert!(edges.contains(
        &json!({"place": "Total", "transition": "Tally", "direction": "in", "kind": "mutate"})
    ));
    assert!(
        edges
            .contains(&json!({"place": "Mid", "transition": "Odd", "direction": "out", "slot": 1}))
    );
    assert!(
        edges.contains(
            &json!({"place": "Done", "transition": "Odd", "direction": "out", "slot": 0})
        )
    );

    assert_eq!(
        s["transitions"]["Double"],
        json!({"enabled": true, "fireable": true, "in_flight": false, "priority": 0.0})
    );
    assert_eq!(
        s["transitions"]["Odd"],
        json!({"enabled": true, "fireable": false, "in_flight": false}),
        "a priority that panics is omitted"
    );
    assert_eq!(
        s["marking"]["places"]["Inbox"],
        json!({"count": 2, "tokens": [{"id": "0", "label": "1"}, {"id": "1", "label": "2"}]})
    );
    assert_eq!(
        s["progress"],
        json!({"step_count": 0, "last_fired": null, "fired_counts": {}, "last_progress_at": null})
    );
}

#[test]
fn a_label_is_cut_at_1000_characters() {
    let mut h = H::loaded("pipeline");
    let s = h.result("get_state", json!({}));
    let label = s["marking"]["places"]["Notes"]["tokens"][0]["label"]
        .as_str()
        .unwrap();
    assert_eq!(label.chars().count(), 1003);
    assert!(label.starts_with("\"zzz"));
    assert!(label.ends_with("z..."));
    // get_token's text is the whole thing.
    let id = s["marking"]["places"]["Notes"]["tokens"][0]["id"].clone();
    let t = h.result("get_token", json!({ "id": id }));
    assert_eq!(t["kind"], "text");
    assert_eq!(t["text"].as_str().unwrap().chars().count(), 1502);
}

#[test]
fn get_state_caps_tokens_but_not_counts() {
    let mut h = H::loaded("pipeline");
    let s = h.result("get_state", json!({"token_limit": 1}));
    assert_eq!(s["marking"]["places"]["Inbox"]["count"], 2);
    assert_eq!(
        s["marking"]["places"]["Inbox"]["tokens"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let s = h.result("get_state", json!({"token_limit": 0}));
    assert_eq!(
        s["marking"]["places"]["Inbox"]["tokens"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "clamped to 1"
    );
    assert_eq!(
        h.error("get_state", json!({"token_limit": "lots"})),
        "invalid_params"
    );
}

#[test]
fn get_place_tokens_pages() {
    let mut h = H::loaded("pipeline");
    let p = h.result(
        "get_place_tokens",
        json!({"place": "Inbox", "offset": 1, "limit": 5}),
    );
    assert_eq!(
        p,
        json!({"place": "Inbox", "marking_version": 0, "count": 2, "offset": 1, "limit": 5,
               "tokens": [{"id": "1", "label": "2"}], "has_more": false})
    );
    let p = h.result("get_place_tokens", json!({"place": "Inbox", "limit": 1}));
    assert_eq!(
        (p["offset"].clone(), p["has_more"].clone()),
        (json!(0), json!(true))
    );
    let p = h.result("get_place_tokens", json!({"place": "Inbox", "limit": 5000}));
    assert_eq!(p["limit"], 1000);
    assert_eq!(
        h.error("get_place_tokens", json!({"place": "Nope"})),
        "unknown_place"
    );
    assert_eq!(h.error("get_place_tokens", json!({})), "invalid_params");
}

#[test]
fn get_token_as_json_or_text() {
    let mut h = H::loaded("pipeline");
    let t = h.result("get_token", json!({"id": "1"}));
    assert_eq!(
        t,
        json!({"id": "1", "place": "Inbox", "type_name": "u32", "kind": "json", "value": 2})
    );
    let t = h.result("get_token", json!({"id": "2"}));
    assert_eq!(
        t,
        json!({"id": "2", "place": "Total", "type_name": "u64", "kind": "text", "text": "0"})
    );
    assert_eq!(h.error("get_token", json!({"id": "99"})), "unknown_token");
    assert_eq!(h.error("get_token", json!({"id": "x"})), "unknown_token");
    assert_eq!(h.error("get_token", json!({})), "invalid_params");
}

// ----- the firing cycle ------------------------------------------------------

#[test]
fn step_reports_launched_and_distributed_before_its_response() {
    let mut h = H::loaded("pipeline");
    let lines = h.send("step", json!({}));
    assert_eq!(tags(&lines), ["launched", "distributed", "fireable", "#2"]);
    assert_eq!(
        lines[0]["params"],
        json!({"transition": "Double", "consumed": [["Inbox", "0"]], "marking_version": 1, "running": false})
    );
    assert_eq!(
        lines[1]["params"],
        json!({"transition": "Double", "produced": [["Mid", "4", "2"]], "duration_ms": 0.0,
               "marking_version": 2, "running": false})
    );
    assert_eq!(
        lines[2]["params"]["transitions"],
        json!(["Double", "Tally"])
    );
    assert_eq!(
        lines[3]["result"],
        json!({"fired": true, "transition": "Double", "marking_version": 2, "running": false})
    );
    assert_eq!(*h.status.lock().unwrap(), None, "cleared on landing");
}

#[test]
fn step_with_nothing_fireable_fires_nothing() {
    let mut h = H::loaded("watcher");
    let lines = h.send("step", json!({}));
    assert_eq!(
        lines,
        [json!({"id": "2", "result": {"fired": false, "marking_version": 0, "running": false}})]
    );
}

#[test]
fn an_inline_firing_is_answered_in_one_handle_as_fired() {
    let mut h = H::loaded("bumper");
    h.auto = false;
    let lines = h.send("step", json!({}));
    assert!(h.held.is_empty(), "nothing to run elsewhere");
    assert_eq!(tags(&lines), ["mutated", "fired", "#2"]);
    assert_eq!(
        lines[0]["params"],
        json!({"place": "Counter", "id": "3", "label": "1", "marking_version": 1, "running": false})
    );
    assert_eq!(
        lines[1]["params"],
        json!({"transition": "Bump", "consumed": [["Ticks", "0"]], "produced": [],
               "duration_ms": 0.0, "marking_version": 2, "running": false})
    );
    assert_eq!(lines[2]["result"]["fired"], true);
}

#[test]
fn reads_are_answered_while_a_body_runs_and_the_rest_wait() {
    let mut h = H::loaded("pipeline");
    h.auto = false;
    let lines = h.send("step", json!({})); // #2
    assert_eq!(tags(&lines), ["launched"]);
    assert_eq!(h.held.len(), 1);
    assert_eq!(
        h.status.lock().unwrap().as_ref().map(|s| s.0.clone()),
        Some("Double".into())
    );

    let s = h.result("get_state", json!({})); // #3, at once
    assert_eq!(s["transitions"]["Double"]["in_flight"], true);
    assert_eq!(s["marking"]["places"]["Inbox"]["count"], 1);
    assert_eq!(tags(&h.send("get_log", json!({}))), ["#4"]);
    assert_eq!(tags(&h.send("get_token", json!({"id": "1"}))), ["#5"]);
    assert_eq!(
        tags(&h.send("get_place_tokens", json!({"place": "Inbox"}))),
        ["#6"]
    );
    assert!(
        h.send("disable", json!({"transition": "Fail"})).is_empty(),
        "#7 waits"
    );
    assert!(
        h.send("step", json!({})).is_empty(),
        "#8, a second step, waits"
    );
    assert!(
        h.send("describe", json!({})).len() == 1,
        "#9 reads nothing of the net"
    );

    let lines = h.land();
    // The first step lands and is answered; then #7; then #8 launches
    // Double again (highest priority, declared first) and waits.
    assert_eq!(
        tags(&lines),
        ["distributed", "fireable", "#2", "toggled", "#7", "launched"]
    );
    assert_eq!(versions(&lines), [2, 2, 3, 4]);
    let lines = h.land();
    assert_eq!(tags(&lines), ["distributed", "fireable", "#8"]);
    assert_eq!(lines[2]["result"]["transition"], "Double");
}

#[test]
fn start_runs_to_quiescence_and_stops() {
    let mut h = H::loaded("pipeline");
    let lines = h.send("start", json!({}));
    let t = tags(&lines);
    assert_eq!(t[0], "#2", "answered before the run's notifications");
    assert_eq!(
        lines[0]["result"],
        json!({"status": "started", "marking_version": 0, "running": true})
    );
    let mut all = lines;
    while h.owner.wants_tick() {
        all.extend(h.tick());
    }
    assert_eq!(tags(&all).last().unwrap(), "execution_stopped");
    let last = versions(&all).into_iter().max().unwrap();
    assert_eq!(
        all.last().unwrap()["params"],
        json!({"reason": "no_fireable_transitions", "marking_version": last, "running": false})
    );
    let fired: Vec<_> = all
        .iter()
        .filter(|l| l["method"] == "distributed" || l["method"] == "fired")
        .map(|l| l["params"]["transition"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(fired, ["Double", "Double", "Tally", "Tally"]);
    assert_eq!(
        all.iter()
            .filter(|l| l["method"] == "execution_stopped")
            .count(),
        1
    );
    let s = h.result("get_state", json!({}));
    assert_eq!(s["running"], false);
    assert_eq!(s["progress"]["step_count"], 4);
    assert_eq!(s["progress"]["last_fired"], "Tally");
    assert_eq!(
        s["progress"]["fired_counts"],
        json!({"Double": 2, "Tally": 2})
    );
    assert_eq!(
        s["progress"]["last_progress_at"],
        "2026-10-03T09:35:02.413Z"
    );
    // Notifications rose by exactly one each.
    let v = versions(
        &all.into_iter()
            .filter(|l| {
                l.get("method").is_some_and(|m| {
                    !["fireable", "execution_stopped"].contains(&m.as_str().unwrap())
                })
            })
            .collect::<Vec<_>>(),
    );
    assert_eq!(v, (1..=v.len() as u64).collect::<Vec<_>>());
}

#[test]
fn an_inline_run_asks_for_a_tick_between_firings() {
    let mut h = H::loaded("bumper");
    let lines = h.send("start", json!({}));
    assert_eq!(tags(&lines), ["#2", "mutated", "fired"]);
    assert!(h.owner.wants_tick());
    let stop = h.send("stop", json!({}));
    assert_eq!(stop[0]["result"]["status"], "stopped");
    assert!(!h.owner.wants_tick(), "stopped");
    assert!(h.tick().is_empty());
    h.send("start", json!({}));
    h.tick();
    let lines = h.tick();
    assert_eq!(tags(&lines), ["execution_stopped"]);
}

#[test]
fn stop_is_answered_while_a_body_runs_and_the_body_lands_normally() {
    let mut h = H::loaded("pipeline");
    h.auto = false;
    let lines = h.send("start", json!({}));
    assert_eq!(tags(&lines), ["#2", "launched"]);
    let lines = h.send("stop", json!({}));
    assert_eq!(
        lines,
        [
            json!({"id": "3", "result": {"status": "stopped", "marking_version": 1, "running": false}})
        ]
    );
    let lines = h.land();
    assert_eq!(tags(&lines), ["distributed", "fireable"]);
    assert_eq!(lines[0]["params"]["running"], false);
    assert!(h.held.is_empty(), "no next firing");
    assert!(h.tick().is_empty());
}

#[test]
fn a_continuous_net_idles_once_per_spell() {
    let mut h = H::loaded("watcher");
    let lines = h.send("start", json!({}));
    assert_eq!(tags(&lines), ["#2", "idle"]);
    assert_eq!(
        lines[1]["params"],
        json!({"idle_since": "2026-10-03T09:35:02.413Z", "marking_version": 0, "running": true})
    );
    assert!(h.tick().is_empty(), "same spell");
    assert!(h.tick().is_empty(), "same spell");

    let lines = h.send("inject", json!({"place": "Inbox", "token": 7}));
    assert_eq!(
        tags(&lines),
        [
            "added",
            "fireable",
            "#3",
            "launched",
            "distributed",
            "fireable",
            "idle"
        ]
    );
    assert_eq!(lines.last().unwrap()["params"]["running"], true);
    assert!(h.tick().is_empty());
    let s = h.result("get_state", json!({}));
    assert_eq!(s["running"], true);
    assert_eq!(s["marking"]["places"]["Out"]["count"], 1);
}

#[test]
fn a_failing_body_stops_a_run() {
    let mut h = H::loaded("pipeline");
    for t in ["Double", "Tally"] {
        h.send("disable", json!({ "transition": t }));
    }
    h.send("inject", json!({"place": "Bad", "token": 1}));
    let lines = h.send("start", json!({}));
    assert_eq!(
        tags(&lines),
        ["#5", "launched", "failed", "fireable", "execution_stopped"]
    );
    assert_eq!(
        lines[2]["params"],
        json!({"transition": "Fail", "error": "refused", "restored": false,
               "consumed": [["Bad", "4"]], "marking_version": 5, "running": true})
    );
    assert_eq!(
        lines[4]["params"],
        json!({"reason": "transition_failed", "error_type": "error", "message": "refused",
               "marking_version": 5, "running": false})
    );
    let log = h.result("get_log", json!({}));
    assert_eq!(
        log["entries"][0],
        json!({"seq": 1, "transition": "Fail", "started_at": "2026-10-03T09:35:02.413Z",
               "duration_ms": 0.0, "outcome": "failed", "consumed": [["Bad", "4"]],
               "produced": [], "error": "refused", "restored": false})
    );
    let s = h.result("get_state", json!({}));
    assert_eq!(s["progress"]["step_count"], 0, "a failure is not progress");
}

#[test]
fn a_panicking_body_stops_a_run_as_a_panic() {
    let mut h = H::loaded("pipeline");
    for t in ["Double", "Tally"] {
        h.send("disable", json!({ "transition": t }));
    }
    h.send("inject", json!({"place": "Panics", "token": 1}));
    let lines = h.send("start", json!({}));
    let stopped = find(&lines, "execution_stopped");
    assert_eq!(stopped["error_type"], "panic");
    assert!(stopped["message"].as_str().unwrap().contains("kaboom"));
}

#[test]
fn a_failing_step_is_answered_as_fired() {
    let mut h = H::loaded("pipeline");
    for t in ["Double", "Tally"] {
        h.send("disable", json!({ "transition": t }));
    }
    h.send("inject", json!({"place": "Bad", "token": 1}));
    let lines = h.send("step", json!({}));
    assert_eq!(tags(&lines), ["launched", "failed", "fireable", "#5"]);
    assert_eq!(lines[3]["result"]["fired"], true);
    assert_eq!(lines[3]["result"]["transition"], "Fail");
}

// ----- input -------------------------------------------------------------------

#[test]
fn inject_adds_a_token_and_answers_with_its_id() {
    let mut h = H::loaded("pipeline");
    let lines = h.send("inject", json!({"place": "Inbox", "token": 5}));
    assert_eq!(tags(&lines), ["added", "#2"]);
    assert_eq!(
        lines[0]["params"],
        json!({"place": "Inbox", "id": "4", "label": "5", "marking_version": 1, "running": false})
    );
    assert_eq!(
        lines[1]["result"],
        json!({"status": "injected", "id": "4", "marking_version": 1})
    );
    assert_eq!(
        h.error("inject", json!({"place": "Nope", "token": 1})),
        "unknown_place"
    );
    assert_eq!(
        h.error("inject", json!({"place": "Mid", "token": 1})),
        "not_serializable"
    );
    assert_eq!(
        h.error("inject", json!({"place": "Inbox", "token": "x"})),
        "invalid_token"
    );
    assert_eq!(
        h.error("inject", json!({"place": "Inbox"})),
        "invalid_params"
    );
}

#[test]
fn enable_and_disable_toggle_only_on_change_and_fireable_only_on_change() {
    let mut h = H::loaded("pipeline");
    let lines = h.send("disable", json!({"transition": "Double"}));
    assert_eq!(tags(&lines), ["toggled", "fireable", "#2"]);
    assert_eq!(
        lines[0]["params"],
        json!({"transition": "Double", "enabled": false, "marking_version": 1, "running": false})
    );
    assert_eq!(lines[1]["params"]["transitions"], json!([]));
    assert_eq!(lines[2]["result"], json!({"marking_version": 1}));
    let lines = h.send("disable", json!({"transition": "Double"}));
    assert_eq!(
        lines,
        [json!({"id": "3", "result": {"marking_version": 1}})]
    );

    // Off and on again: toggled twice, but the fireable set is sent each
    // time it differs from the last one sent.
    let lines = h.send("disable", json!({"transition": "Fail"}));
    assert_eq!(
        tags(&lines),
        ["toggled", "#4"],
        "Fail was not fireable anyway"
    );
    let lines = h.send("enable", json!({"transition": "Double"}));
    assert_eq!(tags(&lines), ["toggled", "fireable", "#5"]);
    assert_eq!(lines[1]["params"]["transitions"], json!(["Double"]));
    assert_eq!(
        h.error("enable", json!({"transition": "Nope"})),
        "unknown_transition"
    );
    assert_eq!(h.error("enable", json!({})), "invalid_params");
}

#[test]
fn set_param_reports_param_changed_under_the_request_name() {
    let mut h = H::loaded("pipeline");
    let lines = h.send(
        "set_param",
        json!({"param": "limit", "value": 3, "transition": "X"}),
    );
    assert_eq!(tags(&lines), ["param_changed", "#2"]);
    assert_eq!(
        lines[0]["params"],
        json!({"param": "limit", "marking_version": 1, "running": false})
    );
    assert_eq!(lines[1]["result"], json!({"marking_version": 1}));
    assert_eq!(
        h.error("set_param", json!({"param": "speed", "value": 1})),
        "unsupported"
    );
    assert_eq!(
        h.error("set_param", json!({"param": "limit", "value": "x"})),
        "invalid_params"
    );
    assert_eq!(
        h.error("set_param", json!({"param": "limit"})),
        "invalid_params"
    );
}

// ----- log and shutdown -------------------------------------------------------

#[test]
fn get_log_keeps_firings_by_names_with_seq_from_one() {
    let mut h = H::loaded("pipeline");
    h.send("step", json!({}));
    h.send("step", json!({}));
    h.send("step", json!({}));
    let log = h.result("get_log", json!({}));
    let entries = log["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(
        entries[0],
        json!({"seq": 1, "transition": "Double", "started_at": "2026-10-03T09:35:02.413Z",
               "duration_ms": 0.0, "outcome": "fired", "consumed": [["Inbox", "0"]],
               "produced": [["Mid", "4"]]})
    );
    assert_eq!(entries[2]["transition"], "Tally");
    let log = h.result("get_log", json!({"after": 1, "limit": 1}));
    assert_eq!(log["entries"].as_array().unwrap().len(), 1);
    assert_eq!(log["entries"][0]["seq"], 2);
    assert_eq!(h.error("get_log", json!({"after": -1})), "invalid_params");
}

#[test]
fn shutdown_answers_then_exits_even_with_a_body_airborne() {
    let mut h = H::loaded("pipeline");
    h.auto = false;
    h.send("step", json!({}));
    let lines = h.send("shutdown", json!({}));
    assert_eq!(
        lines,
        [
            json!({"id": "3", "result": {"status": "ok"}}),
            json!("EXIT")
        ]
    );
}
