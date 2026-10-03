//! The protocol transcripts in `tests/protocol/*.json`, run through `Owner`
//! with no process (DESIGN §19.3.8).
//!
//! # The transcript format
//!
//! ```json
//! {
//!   "net": "parcel-fleet",
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
//!   and `"<id>"` match any string, `"<ms>"` and `"<version>"` any number (a
//!   boolean is not a number), `"<any>"` any value at all. `{"<unordered>":
//!   [...]}` (an object with that one key) matches an array holding the same
//!   elements in some order, each matched by a distinct element. `"<id:NAME>"`
//!   matches any string and binds it to `NAME` for the rest of the
//!   transcript: a later `"<id:NAME>"` in an `expect` must be that string,
//!   and one anywhere in a `send`'s params is replaced by it. A line that
//!   fails to match binds nothing, and neither does an exchange that fails.
//!   Numbers compare by value (`0` matches `0.0`); objects must have exactly
//!   the expected keys; arrays the expected length, element by element.
//!   These are PROTOCOL.md §11's rules, and the Python harness's
//!   (`petritype/tests/test_protocol_transcripts.py`), but for one thing:
//!   `<unordered>` searches for a pairing where Python takes the first fit.
//! - A `launched` immediately followed by its `distributed` is folded into one
//!   `fired {transition, consumed, produced, duration_ms}` (stamped with the
//!   `distributed`'s `marking_version` and `running`) before comparing, so
//!   that a transcript reads the same for a runtime that reports a firing in
//!   one piece. The `marking_version`s still count both pieces.
//! - Type names are the example's: the harness rewrites this test crate's
//!   module path to `parcel_runner::nets::`, as the example binary spells it.
//!
//! Our own transcripts, over the example's demo nets, are in
//! `tests/protocol/`. The shared set of §11, over the conformance net
//! `parcel-distribution`, lives in the Python repository and is replayed
//! here from a sibling checkout by `the_shared_transcripts_hold`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
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

/// What `<id:NAME>` masks have bound so far in one transcript.
type Bindings = BTreeMap<String, String>;

const STRING_MASKS: [&str; 3] = ["<label>", "<time>", "<id>"];
const NUMBER_MASKS: [&str; 2] = ["<ms>", "<version>"];
const ANY: &str = "<any>";
const UNORDERED: &str = "<unordered>";

/// The `NAME` of a `"<id:NAME>"` mask, if `value` is one.
fn capture_name(value: &str) -> Option<&str> {
    value
        .strip_prefix("<id:")
        .and_then(|rest| rest.strip_suffix('>'))
        .filter(|name| !name.is_empty())
}

/// Whether `actual` matches `expected` with its masks. Binds the captures it
/// meets into `bindings`, which are left as they were when the match fails.
fn matches(expected: &Value, actual: &Value, bindings: &mut Bindings) -> bool {
    let mut trial = bindings.clone();
    let matched = match_value(expected, actual, &mut trial);
    if matched {
        *bindings = trial;
    }
    matched
}

fn match_value(expected: &Value, actual: &Value, bindings: &mut Bindings) -> bool {
    match expected {
        Value::String(mask) if mask == ANY => true,
        Value::String(mask) if STRING_MASKS.contains(&mask.as_str()) => actual.is_string(),
        // `is_number` is false for a boolean: a boolean is not a number.
        Value::String(mask) if NUMBER_MASKS.contains(&mask.as_str()) => actual.is_number(),
        Value::String(mask) if capture_name(mask).is_some() => {
            let name = capture_name(mask).unwrap();
            let Some(actual) = actual.as_str() else {
                return false;
            };
            bindings
                .entry(name.to_owned())
                .or_insert_with(|| actual.to_owned())
                == actual
        }
        Value::Number(e) => actual.is_number() && actual.as_f64() == e.as_f64(),
        Value::Array(e) => actual.as_array().is_some_and(|a| {
            a.len() == e.len() && e.iter().zip(a).all(|(e, a)| match_value(e, a, bindings))
        }),
        Value::Object(e) if e.len() == 1 && e.contains_key(UNORDERED) => {
            match_unordered(&e[UNORDERED], actual, bindings)
        }
        Value::Object(e) => actual.as_object().is_some_and(|a| {
            a.len() == e.len()
                && e.iter()
                    .all(|(key, e)| a.get(key).is_some_and(|a| match_value(e, a, bindings)))
        }),
        other => other == actual,
    }
}

/// `{"<unordered>": [...]}`: an array holding the same elements in any
/// order, each expected element matched by a distinct actual one.
///
/// It searches for such a pairing, backtracking, rather than giving each
/// expected element the first unused actual one it matches: with masks the
/// first fit can take an element a later expectation needed (`["<id>", "a"]`
/// against `["a", "b"]`), and PROTOCOL.md §11 asks only that the elements be
/// the same in some order. (The Python harness takes the first fit.)
fn match_unordered(expected: &Value, actual: &Value, bindings: &mut Bindings) -> bool {
    let (Some(expected), Some(actual)) = (expected.as_array(), actual.as_array()) else {
        return false;
    };
    let mut used = vec![false; actual.len()];
    expected.len() == actual.len() && pair_up(expected, actual, &mut used, bindings)
}

fn pair_up(
    expected: &[Value],
    actual: &[Value],
    used: &mut [bool],
    bindings: &mut Bindings,
) -> bool {
    let Some((first, rest)) = expected.split_first() else {
        return true;
    };
    for index in 0..actual.len() {
        if used[index] {
            continue;
        }
        let mut trial = bindings.clone();
        if !match_value(first, &actual[index], &mut trial) {
            continue;
        }
        used[index] = true;
        if pair_up(rest, actual, used, &mut trial) {
            *bindings = trial;
            return true;
        }
        used[index] = false;
    }
    false
}

/// `value` with every `"<id:NAME>"` replaced by its binding.
fn substitute(value: &Value, bindings: &Bindings, where_: &str) -> Value {
    match value {
        Value::String(s) if capture_name(s).is_some() => {
            let name = capture_name(s).unwrap();
            let bound = bindings
                .get(name)
                .unwrap_or_else(|| panic!("{where_}: {s} is sent before any line bound it"));
            Value::String(bound.clone())
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| substitute(item, bindings, where_))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), substitute(v, bindings, where_)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// One way a replay went wrong, with the file and exchange in the message.
struct Mismatch {
    message: String,
}

/// Replays one transcript through a fresh `Owner` over the example's
/// registry and returns every mismatch: the first line that differs in each
/// exchange, or the line count. An exchange that goes wrong binds nothing,
/// and the replay goes on to the next, so that one difference does not hide
/// the rest.
fn replay(path: &Path) -> Vec<Mismatch> {
    replay_with(path, nets::registry())
}

/// `replay` against a chosen registry: the shared set runs against the
/// conformance net alone (PROTOCOL.md §11), ours against everything the
/// example carries.
fn replay_with(path: &Path, registry: Vec<petritype_runner::NetEntry>) -> Vec<Mismatch> {
    let file = path.file_name().unwrap().to_string_lossy().into_owned();
    let whole = |message: String| vec![Mismatch { message }];
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) => return whole(format!("{file}: unreadable: {e}")),
    };
    let transcript: Value = match serde_json::from_str(&text) {
        Ok(transcript) => transcript,
        Err(e) => return whole(format!("{file}: not a transcript: {e}")),
    };
    let net = transcript["net"].as_str().expect("a net name");
    let exchanges = transcript["exchanges"].as_array().expect("exchanges");
    if !exchanges
        .iter()
        .any(|x| x["send"]["method"] == "load" && x["send"]["params"]["net"] == net)
    {
        return whole(format!("{file}: never loads its own net {net:?}"));
    }

    let mut owner = Owner::new(
        registry,
        Arc::new(Mutex::new(None)),
        Duration::from_secs(30),
    );
    let mut bindings = Bindings::new();
    let mut mismatches = Vec::new();
    for (index, exchange) in exchanges.iter().enumerate() {
        let send = &exchange["send"];
        let method = send["method"].as_str().expect("a method").to_owned();
        let where_ = format!("{file}, exchange {index} ({method})");
        let params = send.get("params").cloned().unwrap_or_else(|| json!({}));
        let request = Request {
            id: (index + 1).to_string(),
            method,
            params: substitute(&params, &bindings, &where_),
        };
        let actual: Vec<Value> = fold(run(&mut owner, Input::Request(request)))
            .into_iter()
            .map(normalise)
            .collect();
        let expected = exchange["expect"].as_array().expect("expect");
        let mut trial = bindings.clone();
        let differs = expected
            .iter()
            .zip(&actual)
            .position(|(e, a)| !matches(e, a, &mut trial));
        let message = if let Some(line) = differs {
            Some(format!(
                "{where_}, line {line}:\n  expected {}\n  actual   {}",
                expected[line], actual[line]
            ))
        } else if expected.len() != actual.len() {
            Some(format!(
                "{where_}: expected {} lines, got {}:\n  {}",
                expected.len(),
                actual.len(),
                actual
                    .iter()
                    .map(Value::to_string)
                    .collect::<Vec<_>>()
                    .join("\n  ")
            ))
        } else {
            None
        };
        match message {
            Some(message) => mismatches.push(Mismatch { message }),
            None => bindings = trial,
        }
    }
    mismatches
}

/// The `*.json` files in `dir`, sorted.
fn transcripts_in(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {dir:?}: {e}"))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    paths
}

#[test]
fn every_transcript_holds() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/protocol");
    let paths = transcripts_in(&dir);
    assert!(paths.len() >= 3, "the transcripts are missing from {dir:?}");
    let mismatches: Vec<String> = paths
        .iter()
        .flat_map(|path| replay(path))
        .map(|m| m.message)
        .collect();
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n\n"));
}

/// Where the shared transcripts are: `PETRITYPE_PROTOCOL_TRANSCRIPTS`, or the
/// Python repository checked out beside this one.
fn shared_dir() -> PathBuf {
    std::env::var_os("PETRITYPE_PROTOCOL_TRANSCRIPTS").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../petritype/tests/protocol"),
        PathBuf::from,
    )
}

/// The shared transcripts of PROTOCOL.md §11, over the conformance net
/// `parcel-distribution`, replayed exactly as ours are but against a registry
/// carrying that net alone, which §11 makes the rule so that a shared
/// transcript may pin `describe` in full.
///
/// They live in the Python repository (§11 makes it the home of the shared
/// set), not here, so this test reads them from a sibling checkout, or from
/// `PETRITYPE_PROTOCOL_TRANSCRIPTS`. Where neither is there (a crates.io
/// build, a CI job with one repository) it says so and passes: the shared set
/// is the Python repository's to carry, and our own transcripts above still
/// run.
#[test]
fn the_shared_transcripts_hold() {
    let dir = shared_dir();
    if !dir.is_dir() {
        eprintln!(
            "skipped: no shared transcripts at {dir:?} \
             (check out petritype beside petritype-rs or set PETRITYPE_PROTOCOL_TRANSCRIPTS)"
        );
        return;
    }
    let paths = transcripts_in(&dir);
    assert!(!paths.is_empty(), "no transcripts in {dir:?}");
    let mismatches: Vec<String> = paths
        .iter()
        .flat_map(|path| replay_with(path, nets::conformance_registry()))
        .map(|m| m.message)
        .collect();
    assert!(
        mismatches.is_empty(),
        "{} mismatches over {} shared transcripts in {dir:?}:\n\n{}",
        mismatches.len(),
        paths.len(),
        mismatches.join("\n\n")
    );
}

#[test]
fn the_masks() {
    let b = &mut Bindings::new();
    assert!(matches(
        &json!({"a": "<time>", "b": "<ms>"}),
        &json!({"a": "x", "b": 1.5}),
        b
    ));
    assert!(!matches(&json!({"a": "<ms>"}), &json!({"a": "1"}), b));
    assert!(
        !matches(&json!({"a": "<version>"}), &json!({"a": true}), b),
        "a boolean is not a number"
    );
    assert!(
        !matches(&json!({"a": 1}), &json!({"a": 1, "b": 2}), b),
        "exact keys"
    );
    assert!(matches(&json!(0), &json!(0.0), b));
    assert!(!matches(&json!(1), &json!(true), b) && !matches(&json!(true), &json!(1), b));
    assert!(matches(
        &json!({"d": "<any>"}),
        &json!({"d": {"x": [1, 2]}}),
        b
    ));
    assert!(matches(&json!({"d": "<any>"}), &json!({"d": null}), b));
    assert!(matches(
        &json!({"<unordered>": [1, 2, 2]}),
        &json!([2, 1, 2]),
        b
    ));
    assert!(!matches(
        &json!({"<unordered>": [1, 2, 2]}),
        &json!([2, 1, 1]),
        b
    ));
    assert!(!matches(&json!({"<unordered>": [1]}), &json!([1, 1]), b));
    assert!(
        matches(
            &json!({"<unordered>": ["<id>", "a"]}),
            &json!(["a", "b"]),
            b
        ),
        "a mask does not take what a later element needs"
    );
    assert!(
        matches(
            &json!({"<unordered>": [["<id:p>", 1], ["<id:p>", 2], ["<id>", 1]]}),
            &json!([["q", 1], ["r", 1], ["r", 2]]),
            b
        ) && b.get("p").map(String::as_str) == Some("r"),
        "a capture binds through the pairing that matched"
    );
    assert!(
        !matches(&json!({"<unordered>": [1], "x": 2}), &json!([1]), b),
        "only a lone <unordered> key is the mask"
    );
}

#[test]
fn captures() {
    let b = &mut Bindings::new();
    assert!(matches(&json!(["<id:x>", "<id:x>"]), &json!(["7", "7"]), b));
    assert_eq!(b.get("x").map(String::as_str), Some("7"));
    assert!(!matches(&json!("<id:x>"), &json!("8"), b), "bound already");
    assert!(
        !matches(&json!("<id:z>"), &json!(8), b),
        "a capture is a string"
    );
    assert!(
        !matches(&json!(["<id:y>", "<id:y>"]), &json!(["1", "2"]), b) && !b.contains_key("y"),
        "a failed match binds nothing"
    );
    assert_eq!(
        substitute(&json!({"id": "<id:x>", "n": ["<id:x>"]}), b, "here"),
        json!({"id": "7", "n": ["7"]})
    );
    assert_eq!(
        substitute(&json!("<id:>"), b, "here"),
        json!("<id:>"),
        "an empty name is no capture"
    );
}

#[test]
fn the_fold() {
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
