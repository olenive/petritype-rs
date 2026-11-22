//! The example binary as a real process: requests over a pipe, responses in
//! order, a heartbeat, and a body's `println!` on stderr rather than in the
//! protocol stream.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

/// Builds the example and returns the path of its binary.
fn binary() -> PathBuf {
    let status = Command::new(env!("CARGO"))
        .args([
            "build",
            "--example",
            "parcel_runner",
            "-p",
            "petritype-runner",
            "-q",
        ])
        .status()
        .expect("cargo runs");
    assert!(status.success(), "the example builds");
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(
        || {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .expect("the crate is in a workspace")
                .join("target")
        },
        PathBuf::from,
    );
    let path = target
        .join("debug")
        .join("examples")
        .join(format!("parcel_runner{}", std::env::consts::EXE_SUFFIX));
    assert!(path.exists(), "no binary at {path:?}");
    path
}

struct Session {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    seen: Vec<Value>,
}

impl Session {
    fn spawn() -> Session {
        let mut child = Command::new(binary())
            .env("PETRITYPE_RUNNER_HEARTBEAT_MS", "100")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the example starts");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if tx.send(line.expect("stdout is UTF-8")).is_err() {
                    return;
                }
            }
        });
        Session {
            child,
            stdin,
            lines,
            seen: Vec::new(),
        }
    }

    /// Sends a request and reads lines until its response; returns the
    /// response.
    fn call(&mut self, id: &str, method: &str, params: Value) -> Value {
        let request = json!({"id": id, "method": method, "params": params});
        writeln!(self.stdin, "{request}").unwrap();
        self.stdin.flush().unwrap();
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(20))
                .unwrap_or_else(|_| panic!("no response to {method}"));
            let value: Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("stdout carried a non-JSON line {line:?}: {e}"));
            self.seen.push(value.clone());
            if value.get("method").is_none() {
                assert_eq!(value["id"], id, "responses arrive in order");
                return value;
            }
        }
    }
}

#[test]
fn a_session_over_real_pipes() {
    let mut s = Session::spawn();

    let describe = s.call("1", "describe", json!({}));
    assert_eq!(describe["result"]["runtime"], "rust");

    let load = s.call(
        "2",
        "load",
        json!({"net": "parcel-fleet", "params": {"trucks": 2}}),
    );
    assert_eq!(load["result"]["status"], "loaded");

    // The first step fires Sort, whose body prints.
    let step = s.call("3", "step", json!({}));
    assert_eq!(step["result"]["fired"], true);
    assert_eq!(step["result"]["transition"], "Sort");

    // Long enough for a few heartbeats at 100 ms.
    thread::sleep(Duration::from_millis(350));
    let state = s.call("4", "get_state", json!({}));
    assert_eq!(state["result"]["marking"]["places"]["Sorted"]["count"], 1);

    let shutdown = s.call("5", "shutdown", json!({}));
    assert_eq!(shutdown["result"]["status"], "ok");

    // Anything left on stdout must be protocol too.
    drop(s.stdin);
    while let Ok(line) = s.lines.recv_timeout(Duration::from_secs(10)) {
        let value: Value = serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("stdout carried a non-JSON line {line:?}: {e}"));
        s.seen.push(value);
    }
    let status = s.child.wait().unwrap();
    assert!(status.success(), "exit {status:?}");

    let mut stderr = String::new();
    s.child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(
        stderr.contains("sorting parcel 1"),
        "the body's println is on stderr: {stderr:?}"
    );

    let heartbeat = s
        .seen
        .iter()
        .find(|line| line["method"] == "heartbeat")
        .expect("a heartbeat while loaded");
    let params = &heartbeat["params"];
    assert!(params["step_started_at"].is_null());
    assert!(params["executing_transition"].is_null());
    assert!(params["marking_version"].is_u64());
    assert_eq!(params["running"], false);

    let methods: Vec<&str> = s
        .seen
        .iter()
        .filter_map(|line| line["method"].as_str())
        .filter(|m| *m != "heartbeat")
        .collect();
    assert_eq!(
        methods,
        [
            "load_progress",
            "reloaded",
            "launched",
            "distributed",
            "fireable"
        ]
    );
}

/// The binary serves the `levels` net: a hero goes in through a port of a
/// subnet place, and the subnet answers for itself by its path.
#[test]
fn a_net_with_subnet_places_over_real_pipes() {
    let mut s = Session::spawn();

    let describe = s.call("1", "describe", json!({}));
    let nets = describe["result"]["nets"].as_array().unwrap();
    assert!(nets.iter().any(|net| net["name"] == "levels"));

    let load = s.call("2", "load", json!({"net": "levels"}));
    assert_eq!(load["result"]["status"], "loaded");

    let state = s.call("3", "get_state", json!({}));
    let places = &state["result"]["marking"]["places"];
    assert_eq!(places["Door"]["count"], 1);
    assert_eq!(places["Level 1"]["ports"]["Lobby"]["count"], 0);
    assert_eq!(
        places["Level 1"]["subnet"]["path"],
        json!([{"place": "Level 1"}])
    );

    // The world's Enter puts the hero into Level 1's Lobby port.
    let step = s.call("4", "step", json!({}));
    assert_eq!(step["result"]["transition"], "Enter");
    let state = s.call("5", "get_state", json!({}));
    let places = &state["result"]["marking"]["places"];
    assert_eq!(places["Door"]["count"], 0);
    assert_eq!(places["Level 1"]["ports"]["Lobby"]["count"], 1);

    // The level's own Walk In takes the hero from the port, inside.
    let step = s.call("6", "step", json!({}));
    assert_eq!(step["result"]["transition"], "Walk In");
    assert_eq!(step["result"]["path"], json!([{"place": "Level 1"}]));
    let inside = s.call("7", "get_state", json!({"path": [{"place": "Level 1"}]}));
    let places = &inside["result"]["marking"]["places"];
    assert_eq!(places["Lobby"]["count"], 0);
    assert_eq!(places["Arena"]["count"], 1);

    let shutdown = s.call("8", "shutdown", json!({}));
    assert_eq!(shutdown["result"]["status"], "ok");
}

/// The binary serves the `groups` net: its topology lists its groups and
/// each member's, and the subnet's topology, read by its path, its own.
#[test]
fn a_net_with_groups_over_real_pipes() {
    let mut s = Session::spawn();

    let describe = s.call("1", "describe", json!({}));
    let nets = describe["result"]["nets"].as_array().unwrap();
    assert!(nets.iter().any(|net| net["name"] == "groups"));

    let load = s.call("2", "load", json!({"net": "groups"}));
    assert_eq!(load["result"]["status"], "loaded");

    let state = s.call("3", "get_state", json!({}));
    let topology = &state["result"]["topology"];
    assert_eq!(topology["groups"], json!(["Sorting", "Packing"]));
    let places: Vec<&Value> = topology["places"]
        .as_array()
        .unwrap()
        .iter()
        .map(|place| &place["group"])
        .collect();
    assert_eq!(places, [&Value::Null, &json!("Sorting"), &json!("Packing")]);
    let transitions: Vec<&Value> = topology["transitions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|transition| &transition["group"])
        .collect();
    assert_eq!(transitions, [&json!("Sorting"), &json!("Packing")]);

    let inside = s.call("4", "get_state", json!({"path": [{"place": "Box"}]}));
    assert_eq!(inside["result"]["topology"]["groups"], json!(["Sealing"]));

    let shutdown = s.call("5", "shutdown", json!({}));
    assert_eq!(shutdown["result"]["status"], "ok");
}

#[test]
fn a_malformed_line_gets_an_invalid_params_answer() {
    let mut s = Session::spawn();
    writeln!(s.stdin, "this is not json").unwrap();
    writeln!(s.stdin, r#"{{"id": "7"}}"#).unwrap();
    s.stdin.flush().unwrap();
    for id in ["", "7"] {
        let line = s.lines.recv_timeout(Duration::from_secs(20)).unwrap();
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["id"], id);
        assert_eq!(value["error"]["code"], "invalid_params");
    }
    // End of file exits without a response.
    drop(s.stdin);
    assert!(s.child.wait().unwrap().success());
    assert!(s.lines.recv_timeout(Duration::from_secs(10)).is_err());
}

#[test]
fn describe_flag_prints_one_line() {
    let output = Command::new(binary())
        .arg("--describe")
        .output()
        .expect("the example runs");
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert_eq!(text.lines().count(), 1);
    let describe: Value = serde_json::from_str(text.trim()).unwrap();
    assert_eq!(describe["protocol_version"], 2);
    assert_eq!(describe["nets"][0]["name"], "parcel-fleet");
    assert_eq!(describe["nets"][1]["name"], "parcel-distribution");
    assert_eq!(
        describe["nets"][1]["description"],
        "Sort, stage and dispatch parcels"
    );
    assert_eq!(describe["nets"][2]["mode"], "24/7");
}

#[test]
fn other_arguments_are_a_usage_error() {
    let output = Command::new(binary())
        .arg("--frobnicate")
        .output()
        .expect("the example runs");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("usage"));
}
