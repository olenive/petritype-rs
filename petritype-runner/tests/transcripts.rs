//! The protocol transcripts in `tests/protocol/*.json`, run through `Owner`
//! with no process (DESIGN §19.3.8).
//!
//! # The transcript format
//!
//! ```json
//! {
//!   "net": "parcel-distribution",
//!   "params": {"trucks": 2},
//!   "exchanges": [
//!     {"send": {"method": "step", "params": {}},
//!      "expect": [{"method": "fired", "params": {...}}, {"id": "2", "result": {...}}]}
//!   ]
//! }
//! ```
//!
//! - `net` and `params` say which net the transcript is about; the
//!   exchanges load it themselves, so that `load` is tested too.
//! - Each `send` is a request without its `id`: the harness numbers them
//!   `"1"`, `"2"`, ... in order, and a missing `params` is `{}`.
//! - `expect` is every line the request causes, in order: its notifications,
//!   then its response. A body that is launched is run at once and fed back,
//!   and a run that asks for a tick gets one, so the lines of the whole
//!   firing belong to the exchange that caused it.
//! - Masks may stand for any value in any position: `"<label>"`, `"<time>"`
//!   and `"<id>"` match any string, `"<ms>"` and `"<version>"` any number.
//!   Numbers compare by value (`0` matches `0.0`); objects must have exactly
//!   the expected keys.
//! - A `launched` immediately followed by its `distributed` is folded into one
//!   `fired {transition, consumed, produced, duration_ms}` (stamped with the
//!   `distributed`'s `marking_version` and `running`) before comparing, so
//!   that a transcript reads the same for a runtime that reports a firing in
//!   one piece. The `marking_version`s still count both pieces.
//! - Type names are the example's: the harness rewrites this test crate's
//!   module path to `parcel_runner::nets::`, as the example binary spells it.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use petritype_runner::protocol::Request;
use petritype_runner::{Input, Outgoing, Owner};
use serde_json::{Map, Value, json};

#[path = "../examples/parcel_runner/nets.rs"]
#[allow(dead_code)]
mod nets;

/// Every line one input causes, firings fed back and ticks given.
fn run(owner: &mut Owner, input: Input) -> Vec<Value> {
    let now = UNIX_EPOCH + Duration::from_millis(1_791_020_102_413);
    let mut lines = Vec::new();
    let mut pending = vec![input];
    while let Some(input) = pending.pop() {
        for outgoing in owner.handle(input, now) {
            match outgoing {
                Outgoing::Line(line) => {
                    lines.push(serde_json::from_str(&line).expect("every line is JSON"));
                }
                Outgoing::Launch(firing) => pending.push(Input::Completed(firing.run())),
                Outgoing::Exit => {}
            }
        }
        if pending.is_empty() && owner.wants_tick() {
            pending.push(Input::Tick);
        }
    }
    lines
}

/// `launched` + its `distributed` as one `fired`.
fn fold(lines: Vec<Value>) -> Vec<Value> {
    let mut folded: Vec<Value> = Vec::with_capacity(lines.len());
    for line in lines {
        if line["method"] == "distributed"
            && let Some(previous) = folded.last()
            && previous["method"] == "launched"
            && previous["params"]["transition"] == line["params"]["transition"]
        {
            let launched = folded.pop().unwrap();
            let (l, d) = (&launched["params"], &line["params"]);
            folded.push(json!({"method": "fired", "params": {
                "transition": d["transition"],
                "consumed": l["consumed"],
                "produced": d["produced"],
                "duration_ms": d["duration_ms"],
                "marking_version": d["marking_version"],
                "running": d["running"],
            }}));
            continue;
        }
        folded.push(line);
    }
    folded
}

/// This crate's spelling of the example's types, rewritten to the
/// example binary's.
fn normalise(value: Value) -> Value {
    match value {
        Value::String(s) => {
            Value::String(s.replace("transcripts::nets::", "parcel_runner::nets::"))
        }
        Value::Array(items) => Value::Array(items.into_iter().map(normalise).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, normalise(v)))
                .collect::<Map<_, _>>(),
        ),
        other => other,
    }
}

fn matches(expected: &Value, actual: &Value) -> bool {
    match expected {
        Value::String(mask) if ["<label>", "<time>", "<id>"].contains(&mask.as_str()) => {
            actual.is_string()
        }
        Value::String(mask) if ["<ms>", "<version>"].contains(&mask.as_str()) => actual.is_number(),
        Value::Number(e) => actual.as_f64().is_some_and(|a| Some(a) == e.as_f64()),
        Value::Array(e) => actual
            .as_array()
            .is_some_and(|a| a.len() == e.len() && e.iter().zip(a).all(|(e, a)| matches(e, a))),
        Value::Object(e) => actual.as_object().is_some_and(|a| {
            a.len() == e.len()
                && e.iter()
                    .all(|(key, e)| a.get(key).is_some_and(|a| matches(e, a)))
        }),
        other => other == actual,
    }
}

fn check(path: &Path) {
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    let text = std::fs::read_to_string(path).unwrap();
    let transcript: Value =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name}: not a transcript: {e}"));
    let net = transcript["net"].as_str().expect("a net name");
    let exchanges = transcript["exchanges"].as_array().expect("exchanges");
    assert!(
        exchanges
            .iter()
            .any(|x| x["send"]["method"] == "load" && x["send"]["params"]["net"] == net),
        "{name}: never loads its own net {net:?}"
    );

    let mut owner = Owner::new(
        nets::registry(),
        Arc::new(Mutex::new(None)),
        Duration::from_secs(30),
    );
    for (index, exchange) in exchanges.iter().enumerate() {
        let send = &exchange["send"];
        let request = Request {
            id: (index + 1).to_string(),
            method: send["method"].as_str().expect("a method").to_owned(),
            params: send.get("params").cloned().unwrap_or_else(|| json!({})),
        };
        let actual: Vec<Value> = fold(run(&mut owner, Input::Request(request)))
            .into_iter()
            .map(normalise)
            .collect();
        let expected = exchange["expect"].as_array().expect("expect");
        for (line, (e, a)) in expected.iter().zip(&actual).enumerate() {
            assert!(
                matches(e, a),
                "{name}, exchange {index} ({}), line {line}:\n  expected {e}\n  actual   {a}",
                send["method"]
            );
        }
        assert_eq!(
            expected.len(),
            actual.len(),
            "{name}, exchange {index} ({}): expected {} lines, got {}:\n  {}",
            send["method"],
            expected.len(),
            actual.len(),
            actual
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n  ")
        );
    }
}

#[test]
fn every_transcript_holds() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/protocol");
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    assert!(paths.len() >= 3, "the transcripts are missing from {dir:?}");
    for path in paths {
        check(&path);
    }
}

#[test]
fn the_masks_and_the_fold() {
    assert!(matches(
        &json!({"a": "<time>", "b": "<ms>"}),
        &json!({"a": "x", "b": 1.5})
    ));
    assert!(!matches(&json!({"a": "<ms>"}), &json!({"a": "1"})));
    assert!(
        !matches(&json!({"a": 1}), &json!({"a": 1, "b": 2})),
        "exact keys"
    );
    assert!(matches(&json!(0), &json!(0.0)));
    let folded = fold(vec![
        json!({"method": "launched", "params": {"transition": "T", "consumed": [["A", "1"]], "marking_version": 1, "running": false}}),
        json!({"method": "distributed", "params": {"transition": "T", "produced": [], "duration_ms": 2.0, "marking_version": 2, "running": false}}),
        json!({"method": "launched", "params": {"transition": "T", "consumed": [], "marking_version": 3, "running": false}}),
    ]);
    assert_eq!(folded.len(), 2);
    assert_eq!(
        folded[0],
        json!({"method": "fired", "params": {"transition": "T", "consumed": [["A", "1"]],
               "produced": [], "duration_ms": 2.0, "marking_version": 2, "running": false}})
    );
    assert_eq!(folded[1]["method"], "launched", "no distributed follows it");
}
