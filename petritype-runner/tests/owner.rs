//! `Owner` driven directly, with a fixed clock: every method, every error
//! code, and the orderings the protocol promises (DESIGN §19.3.8).

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use petritype::{Firing, MutateEdge, Nested, Net, NetBuilder, Param, TakeEdge, Templates};
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
        H::over(registry())
    }

    /// An owner over other nets than [`registry`]'s.
    fn over(nets: Vec<NetEntry>) -> H {
        let status: StatusCell = Arc::new(Mutex::new(None));
        H {
            owner: Owner::new(nets, Arc::clone(&status), Duration::from_secs(30)),
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
    assert_eq!(label.chars().count(), 1000, "the cap includes the marker");
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
        "invalid_params",
        "a parameter the net does not have (PROTOCOL.md §6.4, pinned by 05_nested_nets)"
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

// ----- nested nets (PROTOCOL.md §12) -------------------------------------------

/// The lid of a bin: while it is shut, nothing moves inside.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
struct Lid(bool);

/// The template `Bin`: `In -Move-> Out`, numbers, both serializable, and a
/// `lid` parameter. `Move` refuses 13.
fn bin(_: &Params) -> Result<Net, LoadError> {
    let mut b = NetBuilder::new();
    let input = b.place::<u32>("In");
    let output = b.place::<u32>("Out");
    b.serializable(input);
    b.serializable(output);
    b.param(Lid(false));
    b.serializable_param::<Lid>("lid");
    b.transition(
        "Move",
        (TakeEdge(input),),
        (output,),
        |n: u32| -> Result<(u32,), Refused> { if n == 13 { Err(Refused) } else { Ok((n,)) } },
    )
    .guard(|net: &Net| !net.param::<Lid>().is_some_and(|lid| lid.0));
    Ok(b.build()?)
}

/// A bin built from the template, with `items` in it.
fn filled(templates: &Templates, items: &[u32]) -> Net {
    let mut net = templates.build("Bin", &Params::new()).unwrap();
    let input = net.place_ref::<u32>("In").unwrap();
    net.extend(input, items.iter().copied());
    net
}

/// A truck: a name, and one inner net, its `bin`.
#[derive(serde::Serialize, serde::Deserialize)]
struct Truck {
    name: String,
    bin: Net,
}

impl std::fmt::Debug for Truck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Truck")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl Nested for Truck {
    fn nets(&self) -> Vec<(&str, &Net)> {
        vec![("bin", &self.bin)]
    }

    fn nets_mut(&mut self) -> Vec<(&str, &mut Net)> {
        vec![("bin", &mut self.bin)]
    }
}

/// `depot`: `Lot` holds the truck T1 (token 0), its bin holding 1 and 2;
/// `Gate` none; `Yard`, passive, one bare bin holding 5 (token 1); `Parcels`
/// none. `Load` takes a parcel into the front truck's bin (a mutate edge, so
/// inline), `Drive` moves a truck from `Lot` to `Gate` (a new token id, the
/// same bin), and `Crush` consumes one at `Gate`, and its bin with it.
fn depot(_: &Params) -> Result<Net, LoadError> {
    let mut templates = Templates::new();
    templates.register("Bin", bin);
    let mut b = NetBuilder::new();
    b.templates(&templates);
    let parcels = b.place::<u32>("Parcels");
    let lot = b.place_nested::<Truck>("Lot");
    let gate = b.place_nested::<Truck>("Gate");
    let yard = b.place_nested::<Net>("Yard");
    b.serializable(parcels);
    b.serializable(lot);
    b.serializable(gate);
    b.serializable(yard);
    b.passive(yard);
    b.seed(
        lot,
        [Truck {
            name: "T1".into(),
            bin: filled(&templates, &[1, 2]),
        }],
    );
    b.seed(yard, [filled(&templates, &[5])]);
    b.transition(
        "Load",
        (TakeEdge(parcels), MutateEdge(lot)),
        (),
        |n: u32, truck: &mut Truck| {
            let input = truck.bin.place_ref::<u32>("In").expect("a bin has an In");
            truck.bin.put(input, n);
        },
    );
    b.transition("Drive", (TakeEdge(lot),), (gate,), |truck: Truck| (truck,));
    b.transition("Crush", (TakeEdge(gate),), (), |_: Truck| {});
    Ok(b.build()?)
}

/// `deep`, two levels down: `Shelf` holds a bare net (the middle one) with
/// `In` holding 2 (token 0) and, in its `Box` (token 1), a bare bin holding
/// 1. Nothing fires out here.
fn deep(_: &Params) -> Result<Net, LoadError> {
    let mut templates = Templates::new();
    templates.register("Bin", bin);
    let mut b = NetBuilder::new();
    let input = b.place::<u32>("In");
    let output = b.place::<u32>("Out");
    b.serializable(input);
    b.serializable(output);
    b.seed(input, [2]);
    b.transition("Move", (TakeEdge(input),), (output,), |n: u32| (n,));
    let boxes = b.place_nested::<Net>("Box");
    b.seed(boxes, [filled(&templates, &[1])]);
    let middle = b.build()?;
    let mut b = NetBuilder::new();
    let shelf = b.place_nested::<Net>("Shelf");
    b.seed(shelf, [middle]);
    Ok(b.build()?)
}

fn nested_registry() -> Vec<NetEntry> {
    vec![NetEntry::new("depot", depot), NetEntry::new("deep", deep)]
}

fn loaded_over(net: &str) -> H {
    let mut h = H::over(nested_registry());
    let lines = h.send("load", json!({ "net": net }));
    assert_eq!(
        lines.last().unwrap()["result"]["status"],
        "loaded",
        "{lines:?}"
    );
    h
}

/// The first `nets` entry of the front token of `place` in a state.
fn held(state: &Value, place: &str) -> Value {
    state["marking"]["places"][place]["tokens"][0]["nets"][0].clone()
}

/// Ticks a running owner until it stops asking for one.
fn run_out(h: &mut H, mut lines: Vec<Value>) -> Vec<Value> {
    while h.owner.wants_tick() {
        lines.extend(h.tick());
    }
    lines
}

#[test]
fn nested_places_say_so_and_their_tokens_list_their_nets() {
    let mut h = loaded_over("depot");
    let state = h.result("get_state", json!({}));
    let places = &state["topology"]["places"];
    assert_eq!(
        places[0],
        json!({"name": "Parcels", "type_name": "u32", "serializable": true}),
        "a plain place says nothing of nesting"
    );
    assert_eq!(places[1]["nested"], true);
    assert!(places[1].get("passive").is_none());
    assert_eq!(
        (&places[3]["nested"], &places[3]["passive"]),
        (&json!(true), &json!(true))
    );

    let truck = held(&state, "Lot");
    assert!(truck["nested_id"].is_string());
    assert_eq!(
        truck,
        json!({"name": "bin", "nested_id": truck["nested_id"], "path": "depot/Lot/#0/bin",
               "template": "Bin", "count": 2})
    );
    let yard = held(&state, "Yard");
    assert_eq!(
        (&yard["name"], &yard["path"], &yard["count"]),
        (&json!(""), &json!("depot/Yard/#1/"), &json!(1)),
        "a bare net is unnamed, and its path ends in a slash"
    );
    assert_ne!(yard["nested_id"], truck["nested_id"]);

    let token = h.result("get_token", json!({"id": "0"}));
    assert_eq!(token["nets"], json!([truck]));
    assert_eq!(
        token["value"],
        json!({"name": "T1", "bin": {"template": "Bin", "marking": {"In": [1, 2], "Out": []},
               "params": {"lid": false}, "enabled": {"Move": true}}}),
        "the saved form of PROTOCOL.md §12.5"
    );
    assert!(token.get("path").is_none() && token.get("nested_id").is_none());
}

#[test]
fn a_path_and_a_nested_id_name_the_same_inner_net() {
    let mut h = loaded_over("depot");
    let state = h.result("get_state", json!({}));
    let truck = held(&state, "Lot");
    let by_id = h.result("get_state", json!({"nested_id": truck["nested_id"]}));
    let by_path = h.result("get_state", json!({"path": truck["path"]}));
    assert_eq!(by_id, by_path);
    assert_eq!(
        (
            &by_id["net"],
            &by_id["generation"],
            &by_id["marking_version"]
        ),
        (&json!("bin"), &json!(1), &json!(0)),
        "its name in its token, its first generation, its own version"
    );
    assert_eq!(
        (&by_id["path"], &by_id["nested_id"], &by_id["template"]),
        (&truck["path"], &truck["nested_id"], &json!("Bin"))
    );
    assert_eq!(by_id["marking"]["places"]["In"]["count"], 2);
    assert_eq!(by_id["transitions"]["Move"]["fireable"], true);
    assert_eq!(by_id["progress"]["step_count"], 0);

    let yard = held(&state, "Yard");
    let bare = h.result("get_state", json!({"path": "depot/Yard/#1/"}));
    assert_eq!(
        (&bare["net"], &bare["nested_id"]),
        (&json!(""), &yard["nested_id"])
    );
    assert_eq!(
        h.result("get_state", json!({"path": "depot"}))["net"],
        "depot",
        "the loaded net's name alone is the outer net"
    );

    let page = h.result(
        "get_place_tokens",
        json!({"place": "In", "nested_id": truck["nested_id"]}),
    );
    assert_eq!(
        page,
        json!({"place": "In", "marking_version": 0, "count": 2, "offset": 0, "limit": 200,
               "tokens": [{"id": "0", "label": "1"}, {"id": "1", "label": "2"}],
               "has_more": false, "path": truck["path"], "nested_id": truck["nested_id"]})
    );
    assert_eq!(
        h.result("get_token", json!({"id": "1", "path": truck["path"]})),
        json!({"id": "1", "place": "In", "type_name": "u32", "kind": "json", "value": 2,
               "path": truck["path"], "nested_id": truck["nested_id"]})
    );
    assert_eq!(
        h.error(
            "get_place_tokens",
            json!({"place": "Lot", "nested_id": truck["nested_id"]})
        ),
        "unknown_place",
        "an inner net has its own places"
    );
}

#[test]
fn an_address_that_names_no_live_net_says_which_segment_failed() {
    let mut h = loaded_over("depot");
    let lines = h.send("inject", json!({"place": "Parcels", "token": 7}));
    assert_eq!(lines.last().unwrap()["result"]["id"], "2");
    for (path, says) in [
        ("city/Lot/#0/bin", "\"depot\""),
        ("depot/Nowhere/#0/bin", "no place \"Nowhere\""),
        ("depot/Parcels/#2/bin", "#2 in \"Parcels\" holds no nets"),
        ("depot/Lot/#0", "stops after \"#0\""),
        ("depot/Lot/0/bin", "\"0\" is not a token"),
        ("depot/Lot/#7/bin", "no token #7"),
        ("depot/Lot/#0/cabin", "no net \"cabin\""),
    ] {
        let lines = h.send("get_state", json!({ "path": path }));
        assert_eq!(lines[0]["error"]["code"], "unknown_net", "{path}");
        let message = lines[0]["error"]["message"].as_str().unwrap();
        assert!(message.contains(says), "{path}: {message}");
    }
    let id = held(&h.result("get_state", json!({})), "Lot")["nested_id"].clone();
    assert_eq!(
        h.error("get_state", json!({"nested_id": "999999"})),
        "unknown_net"
    );
    assert_eq!(
        h.error("get_state", json!({"nested_id": 3})),
        "invalid_params",
        "a nested id is a string"
    );
    assert_eq!(
        h.error("get_state", json!({"nested_id": id, "path": "depot"})),
        "invalid_params",
        "one address or the other"
    );
    assert_eq!(
        h.error("get_log", json!({"nested_id": "999999"})),
        "unknown_net"
    );
    assert_eq!(
        h.error(
            "inject",
            json!({"place": "In", "token": 1, "nested_id": "999999"})
        ),
        "unknown_net"
    );
}

#[test]
fn switches_parameters_and_runs_are_the_outer_net_s_alone() {
    let mut h = loaded_over("depot");
    let id = held(&h.result("get_state", json!({})), "Lot")["nested_id"].clone();
    for (method, params) in [
        ("enable", json!({"transition": "Move", "nested_id": id})),
        ("disable", json!({"transition": "Move", "nested_id": id})),
        (
            "set_param",
            json!({"param": "lid", "value": true, "nested_id": id}),
        ),
        ("step", json!({"nested_id": id})),
        ("start", json!({"path": "depot/Lot/#0/bin"})),
        ("stop", json!({"nested_id": id})),
        ("reset", json!({"nested_id": id})),
    ] {
        assert_eq!(h.error(method, params), "invalid_params", "{method}");
    }
}

#[test]
fn an_inner_event_moves_its_own_net_s_version_and_not_the_outer_one() {
    let mut h = loaded_over("depot");
    let state = h.result("get_state", json!({}));
    let truck = held(&state, "Lot");
    let outer = state["marking_version"].clone();
    let lines = h.send(
        "inject",
        json!({"place": "In", "token": 3, "nested_id": truck["nested_id"]}),
    );
    assert_eq!(tags(&lines), ["nested", "#3"]);
    assert_eq!(
        lines[0]["params"],
        json!({"path": truck["path"], "nested_id": truck["nested_id"],
               "event": {"method": "added", "params": {"place": "In", "id": "2", "label": "3",
                                                       "marking_version": 1, "running": false}},
               "marking_version": outer, "running": false})
    );
    assert_eq!(
        lines[1]["result"],
        json!({"status": "injected", "id": "2", "marking_version": 1}),
        "the version of the net injected into"
    );
    let lines = h.send(
        "inject",
        json!({"place": "In", "token": 4, "path": truck["path"]}),
    );
    assert_eq!(lines[0]["params"]["event"]["params"]["marking_version"], 2);
    assert_eq!(lines[0]["params"]["marking_version"], outer);
    let state = h.result("get_state", json!({}));
    assert_eq!(
        state["marking_version"], outer,
        "the outer net did not move"
    );
    assert_eq!(
        held(&state, "Lot")["count"],
        4,
        "the hint, read with the outer state"
    );
    assert_eq!(
        h.result("get_state", json!({"nested_id": truck["nested_id"]}))["marking_version"],
        2
    );
}

#[test]
fn a_step_fires_inside_when_the_outer_net_has_nothing() {
    let mut h = loaded_over("depot");
    let truck = held(&h.result("get_state", json!({})), "Lot");
    let outer = h.result("disable", json!({"transition": "Drive"}))["marking_version"].clone();

    let lines = h.send("step", json!({}));
    assert_eq!(tags(&lines), ["nested", "#4"]);
    assert_eq!(
        lines[0]["params"],
        json!({"path": truck["path"], "nested_id": truck["nested_id"],
               "event": {"method": "fired", "params": {
                   "transition": "Move", "consumed": [["In", "0"]], "produced": [["Out", "2", "1"]],
                   "duration_ms": 0.0, "marking_version": 1, "running": false}},
               "marking_version": outer, "running": false})
    );
    assert_eq!(
        lines[1]["result"],
        json!({"fired": true, "transition": "Move", "path": truck["path"],
               "nested_id": truck["nested_id"], "marking_version": outer, "running": false})
    );
    let lines = h.send("step", json!({}));
    assert_eq!(tags(&lines), ["nested", "nested", "#5"]);
    assert_eq!(lines[1]["params"]["event"]["method"], "fireable");
    assert_eq!(
        lines[1]["params"]["event"]["params"]["transitions"],
        json!([])
    );
    // The yard's bin holds 5, but the yard is passive.
    assert_eq!(
        h.result("step", json!({})),
        json!({"fired": false, "marking_version": outer, "running": false})
    );

    let inner = h.result("get_state", json!({"nested_id": truck["nested_id"]}));
    assert_eq!(inner["progress"]["step_count"], 2);
    assert_eq!(inner["progress"]["last_fired"], "Move");
    assert_eq!(inner["progress"]["fired_counts"], json!({"Move": 2}));
    let state = h.result("get_state", json!({}));
    assert_eq!(
        state["progress"]["step_count"], 0,
        "the outer net fired nothing"
    );
    assert_eq!(state["progress"]["fired_counts"], json!({}));
}

#[test]
fn an_inner_fireable_set_follows_the_outer_one_and_only_when_it_changed() {
    let mut h = loaded_over("depot");
    h.send("disable", json!({"transition": "Drive"}));
    assert_eq!(
        tags(&h.send("step", json!({}))),
        ["nested", "#3"],
        "still Move"
    );
    assert_eq!(tags(&h.send("step", json!({}))), ["nested", "nested", "#4"]);
    let lines = h.send("inject", json!({"place": "Parcels", "token": 7}));
    assert_eq!(tags(&lines), ["added", "fireable", "#5"]);
    // `Load` reaches inside: its inner `added` comes before its `mutated`,
    // and the bin's `fireable` after the outer net's.
    let lines = h.send("step", json!({}));
    assert_eq!(
        tags(&lines),
        ["nested", "mutated", "fired", "fireable", "nested", "#6"]
    );
    assert_eq!(lines[0]["params"]["event"]["method"], "added");
    assert_eq!(lines[0]["params"]["event"]["params"]["label"], "7");
    assert_eq!(lines[3]["params"]["transitions"], json!([]));
    // Two `fired` and one `added` in the bin: version 3. A `fireable` moves
    // no version.
    assert_eq!(
        lines[4]["params"]["event"],
        json!({"method": "fireable", "params": {"transitions": ["Move"], "marking_version": 3,
                                                "running": false}})
    );
}

#[test]
fn a_moved_token_keeps_its_nested_id_and_a_consumed_one_takes_it_away() {
    let mut h = loaded_over("depot");
    let truck = held(&h.result("get_state", json!({})), "Lot");
    let lines = h.send("step", json!({}));
    assert_eq!(find(&lines, "launched")["transition"], "Drive");
    assert!(
        !tags(&lines).contains(&"nested".to_owned()),
        "nothing happened inside"
    );

    let state = h.result("get_state", json!({}));
    assert_eq!(state["marking"]["places"]["Lot"]["count"], 0);
    let moved = held(&state, "Gate");
    assert_eq!(moved["nested_id"], truck["nested_id"], "the stable name");
    assert!(moved["path"].as_str().unwrap().starts_with("depot/Gate/#"));
    assert_ne!(moved["path"], truck["path"]);
    assert_eq!(
        h.error("get_state", json!({"path": truck["path"]})),
        "unknown_net",
        "a path names a net as the marking stands"
    );
    assert_eq!(
        h.result("get_state", json!({"nested_id": truck["nested_id"]}))["path"],
        moved["path"]
    );
    let lines = h.send(
        "inject",
        json!({"place": "In", "token": 9, "nested_id": truck["nested_id"]}),
    );
    assert_eq!(lines[0]["params"]["path"], moved["path"]);

    let lines = h.send("step", json!({}));
    assert_eq!(find(&lines, "launched")["transition"], "Crush");
    assert_eq!(
        h.error("get_state", json!({"nested_id": truck["nested_id"]})),
        "unknown_net",
        "consumed and not put back: its inner net is gone (§12.6)"
    );
    assert_eq!(
        h.error("get_log", json!({"nested_id": truck["nested_id"]})),
        "unknown_net"
    );
}

#[test]
fn reset_makes_every_old_nested_id_unknown() {
    let mut h = loaded_over("depot");
    let truck = held(&h.result("get_state", json!({})), "Lot");
    h.send(
        "inject",
        json!({"place": "In", "token": 3, "nested_id": truck["nested_id"]}),
    );
    h.send("reset", json!({}));
    assert_eq!(
        h.error("get_state", json!({"nested_id": truck["nested_id"]})),
        "unknown_net"
    );
    let fresh = held(&h.result("get_state", json!({})), "Lot");
    assert_ne!(fresh["nested_id"], truck["nested_id"]);
    assert_eq!(fresh["path"], "depot/Lot/#0/bin");
    assert_eq!(
        h.result("get_state", json!({"nested_id": fresh["nested_id"]}))["marking_version"],
        0,
        "a new inner net starts its own count"
    );
}

#[test]
fn a_run_fires_inner_nets_and_stops_only_when_nothing_is_fireable_anywhere() {
    let mut h = loaded_over("depot");
    h.send("disable", json!({"transition": "Drive"}));
    let started = h.send("start", json!({}));
    let lines = run_out(&mut h, started);
    let fired: Vec<&Value> = lines
        .iter()
        .filter(|l| l["method"] == "nested" && l["params"]["event"]["method"] == "fired")
        .collect();
    assert_eq!(
        fired.len(),
        2,
        "both of the truck's numbers, none of the passive yard's"
    );
    assert_eq!(
        lines.last().unwrap(),
        &json!({"method": "execution_stopped", "params": {
            "reason": "no_fireable_transitions", "marking_version": lines.last().unwrap()["params"]["marking_version"],
            "running": false}})
    );
}

#[test]
fn a_read_of_a_net_whose_token_a_body_holds_waits_for_the_landing() {
    let mut h = loaded_over("depot");
    let state = h.result("get_state", json!({}));
    let (truck, yard) = (held(&state, "Lot"), held(&state, "Yard"));
    h.auto = false;
    assert_eq!(tags(&h.send("step", json!({}))), ["launched"]);
    assert!(
        h.send("get_state", json!({"nested_id": truck["nested_id"]}))
            .is_empty(),
        "the truck is in the body's hands"
    );
    assert_eq!(
        tags(&h.send("get_state", json!({"nested_id": yard["nested_id"]}))),
        ["#5"],
        "a net no body holds is read at once"
    );
    let lines = h.land();
    assert_eq!(tags(&lines), ["distributed", "fireable", "#3", "#4"]);
    assert!(
        lines[3]["result"]["path"]
            .as_str()
            .unwrap()
            .starts_with("depot/Gate/#"),
        "{lines:#?}"
    );
}

#[test]
fn an_inner_failure_is_a_failed_step_and_stops_a_run() {
    let mut h = loaded_over("depot");
    let truck = held(&h.result("get_state", json!({})), "Lot");
    h.send("disable", json!({"transition": "Drive"}));
    h.send(
        "inject",
        json!({"place": "In", "token": 13, "nested_id": truck["nested_id"]}),
    );
    h.send("step", json!({}));
    h.send("step", json!({}));
    let lines = h.send("step", json!({}));
    let failed = &lines[0]["params"];
    assert_eq!(failed["event"]["method"], "failed");
    assert_eq!(
        (
            &failed["event"]["params"]["error"],
            &failed["event"]["params"]["restored"]
        ),
        (&json!("refused"), &json!(false))
    );
    assert_eq!(
        lines.last().unwrap()["result"],
        json!({"fired": true, "transition": "Move", "failed": true, "path": truck["path"],
               "nested_id": truck["nested_id"],
               "marking_version": lines.last().unwrap()["result"]["marking_version"],
               "running": false})
    );
    let log = h.result("get_log", json!({"nested_id": truck["nested_id"]}));
    let last = log["entries"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(
        (&last["outcome"], &last["error"], &last["path"]),
        (&json!("failed"), &json!("refused"), &truck["path"])
    );

    let mut h = loaded_over("depot");
    let truck = held(&h.result("get_state", json!({})), "Lot");
    h.send("disable", json!({"transition": "Drive"}));
    h.send(
        "inject",
        json!({"place": "In", "token": 13, "nested_id": truck["nested_id"]}),
    );
    let started = h.send("start", json!({}));
    let lines = run_out(&mut h, started);
    let stopped = find(&lines, "execution_stopped");
    assert_eq!(
        (
            &stopped["reason"],
            &stopped["error_type"],
            &stopped["running"]
        ),
        (&json!("transition_failed"), &json!("error"), &json!(false))
    );
}

#[test]
fn get_log_keeps_one_ring_and_filters_it_by_the_chain_of_nets() {
    let mut h = loaded_over("deep");
    let middle = held(&h.result("get_state", json!({})), "Shelf");
    assert_eq!(middle["path"], "deep/Shelf/#0/");
    let leaf = held(
        &h.result("get_state", json!({"nested_id": middle["nested_id"]})),
        "Box",
    );
    assert_eq!(
        leaf["path"], "deep/Shelf/#0//Box/#1/",
        "a path through two bare nets, each level ending in its empty name"
    );

    // Depth two: one wrapper, the full path, the innermost net's version.
    let lines = h.send(
        "inject",
        json!({"place": "In", "token": 8, "path": leaf["path"]}),
    );
    assert_eq!(tags(&lines), ["nested", "#4"]);
    assert_eq!(
        (
            &lines[0]["params"]["path"],
            &lines[0]["params"]["nested_id"]
        ),
        (&leaf["path"], &leaf["nested_id"])
    );
    assert_eq!(lines[0]["params"]["event"]["params"]["marking_version"], 1);

    // The middle net's own `Move` first, then its inner net's.
    let first = h.result("step", json!({}));
    assert_eq!(
        (&first["transition"], &first["nested_id"]),
        (&json!("Move"), &middle["nested_id"])
    );
    let lines = h.send("step", json!({}));
    assert_eq!(lines[0]["params"]["path"], leaf["path"]);
    assert_eq!(lines[0]["params"]["event"]["method"], "fired");
    assert_eq!(
        lines.last().unwrap()["result"]["nested_id"],
        leaf["nested_id"]
    );
    h.send("step", json!({}));
    assert_eq!(h.result("step", json!({}))["fired"], false);

    let all = h.result("get_log", json!({}));
    let all = all["entries"].as_array().unwrap();
    let paths: Vec<&Value> = all.iter().map(|e| &e["path"]).collect();
    assert_eq!(paths, [&middle["path"], &leaf["path"], &leaf["path"]]);
    assert_eq!(all[0]["seq"], 1);
    let of_middle = h.result("get_log", json!({"nested_id": middle["nested_id"]}));
    assert_eq!(
        of_middle["entries"].as_array().unwrap().len(),
        3,
        "a net's entries and those of the nets inside it"
    );
    let of_leaf = h.result("get_log", json!({"path": leaf["path"]}));
    let of_leaf = of_leaf["entries"].as_array().unwrap();
    assert_eq!(of_leaf.len(), 2);
    assert!(of_leaf.iter().all(|e| e["nested_id"] == leaf["nested_id"]));
    assert_eq!(
        h.result("get_log", json!({"path": leaf["path"], "after": 2}))["entries"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
