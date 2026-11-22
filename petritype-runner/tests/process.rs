//! The example binary as a real process: requests over a pipe, responses in
//! order, a heartbeat, and a body's `println!` on stderr rather than in the
//! protocol stream.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

mod common;

use common::example_binary as binary;

struct Session {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    seen: Vec<Value>,
}

impl Session {
    fn spawn() -> Session {
        Session::spawn_with(&[])
    }

    /// The binary started with `args`.
    fn spawn_with(args: &[&str]) -> Session {
        Session::start(binary(), args, "100")
    }

    /// The `slow_bodies` example, its heartbeat every [`HEARTBEAT_MS`].
    fn slow() -> Session {
        Session::start(common::example_named("slow_bodies"), &[], HEARTBEAT_MS)
    }

    /// `program` started with `args`, its heartbeat every `heartbeat_ms`.
    fn start(program: std::path::PathBuf, args: &[&str], heartbeat_ms: &str) -> Session {
        let mut child = Command::new(program)
            .args(args)
            .env("PETRITYPE_RUNNER_HEARTBEAT_MS", heartbeat_ms)
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
        self.send(id, method, params);
        self.response(id)
    }

    /// Sends a request and reads nothing.
    fn send(&mut self, id: &str, method: &str, params: Value) {
        let request = json!({"id": id, "method": method, "params": params});
        writeln!(self.stdin, "{request}").unwrap();
        self.stdin.flush().unwrap();
    }

    /// The next line, kept in `seen`.
    fn next_line(&mut self) -> Value {
        let line = self
            .lines
            .recv_timeout(Duration::from_secs(20))
            .expect("a line within 20 seconds");
        let value: Value = serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("stdout carried a non-JSON line {line:?}: {e}"));
        self.seen.push(value.clone());
        value
    }

    /// Reads lines until the response to `id`, and returns it.
    fn response(&mut self, id: &str) -> Value {
        loop {
            let value = self.next_line();
            if value.get("method").is_none() {
                assert_eq!(value["id"], id, "responses arrive in order");
                return value;
            }
        }
    }

    /// Reads lines until a heartbeat that `wanted` picks, and returns its
    /// params.
    fn heartbeat_where(&mut self, wanted: impl Fn(&Value) -> bool) -> Value {
        loop {
            let value = self.next_line();
            if value["method"] == "heartbeat" && wanted(&value["params"]) {
                return value["params"].clone();
            }
        }
    }
}

/// The heartbeat period the tests of a body in flight run with: short
/// beside [`BODY_MS`], so that every body is beaten through many times.
const HEARTBEAT_MS: &str = "20";

/// How long each body of `slow-office` takes in those tests: long beside
/// the heartbeat, and beside a busy machine's pauses.
const BODY_MS: u64 = 500;

/// The params of every heartbeat among `lines` that names `transition`.
fn heartbeats_naming<'a>(lines: &'a [Value], transition: &str) -> Vec<&'a Value> {
    lines
        .iter()
        .filter(|line| line["method"] == "heartbeat")
        .map(|line| &line["params"])
        .filter(|params| params["executing_transition"] == transition)
        .collect()
}

/// A heartbeat written while a body runs inline names it: one inside the
/// clerk's net with the net's `path` and `nested_id`, the same two a read
/// of the desk gives, and one with a mutate edge in the loaded net with
/// neither. Every heartbeat naming a body is checked, those the writer let
/// out after the body's own lines included, so none needs to come at a
/// given moment; the bodies are many heartbeats long, so some come.
#[test]
fn a_heartbeat_names_an_inline_body_and_the_inner_net_it_runs_in() {
    let mut s = Session::slow();
    let load = s.call(
        "1",
        "load",
        json!({"net": "slow-office", "params": {"ms": BODY_MS, "jobs": 1, "tallies": 1}}),
    );
    assert_eq!(load["result"]["status"], "loaded");
    let state = s.call("2", "get_state", json!({}));
    let clerk = state["result"]["marking"]["places"]["Desk"]["tokens"][0]["nets"][0].clone();
    assert!(clerk["nested_id"].is_string(), "{clerk}");

    let step = s.call("3", "step", json!({}));
    assert_eq!(step["result"]["transition"], "File");
    assert_eq!(step["result"]["path"], clerk["path"]);
    let step = s.call("4", "step", json!({}));
    assert_eq!(step["result"]["transition"], "Tally");
    // A heartbeat that names nothing, once nothing runs.
    let after = s.heartbeat_where(|params| params["executing_transition"].is_null());
    assert!(after["step_started_at"].is_null());
    assert!(after.get("path").is_none() && after.get("nested_id").is_none());

    let filing = heartbeats_naming(&s.seen, "File");
    assert!(!filing.is_empty(), "a heartbeat while File ran");
    for params in filing {
        assert!(params["step_started_at"].is_string());
        assert_eq!(params["path"], clerk["path"]);
        assert_eq!(params["nested_id"], clerk["nested_id"]);
    }
    let tallying = heartbeats_naming(&s.seen, "Tally");
    assert!(!tallying.is_empty(), "a heartbeat while Tally ran");
    for params in tallying {
        assert!(params["step_started_at"].is_string());
        assert!(params.get("path").is_none(), "{params}");
        assert!(params.get("nested_id").is_none(), "{params}");
    }

    let shutdown = s.call("5", "shutdown", json!({}));
    assert_eq!(shutdown["result"]["status"], "ok");
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

/// The binary serves the `workshop` net: a cracked pot fails the firing
/// that holds it, which loses the pot, and gives it back once the net is
/// loaded with `restore`.
#[test]
fn a_failed_firing_over_real_pipes() {
    let mut s = Session::spawn();

    let describe = s.call("1", "describe", json!({}));
    let nets = describe["result"]["nets"].as_array().unwrap();
    let workshop = nets
        .iter()
        .find(|net| net["name"] == "workshop")
        .expect("the binary serves workshop");
    assert_eq!(
        workshop["params"],
        json!([{"name": "restore", "type_name": "bool", "required": false, "default": false}])
    );

    // Loaded with no parameters, a failed firing loses its pot.
    let load = s.call("2", "load", json!({"net": "workshop"}));
    assert_eq!(load["result"]["status"], "loaded");
    s.call(
        "3",
        "inject",
        json!({"place": "Bench", "token": "Cracked bowl"}),
    );
    let mark = s.seen.len();
    let step = s.call("4", "step", json!({}));
    assert_eq!(step["result"]["transition"], "Glaze");
    assert_eq!(step["result"]["failed"], true);
    let failed = s.seen[mark..]
        .iter()
        .find(|line| line["method"] == "failed")
        .expect("a failed notification");
    assert_eq!(failed["params"]["restored"], false);
    assert_eq!(failed["params"]["error"], "Cracked bowl broke");
    let state = s.call("5", "get_state", json!({}));
    assert_eq!(state["result"]["marking"]["places"]["Bench"]["count"], 0);

    // Loaded with `restore`, it puts the pot back, with its id.
    let load = s.call(
        "6",
        "load",
        json!({"net": "workshop", "params": {"restore": true}}),
    );
    assert_eq!(load["result"]["generation"], 2);
    let inject = s.call(
        "7",
        "inject",
        json!({"place": "Bench", "token": "Cracked cup"}),
    );
    let cup = inject["result"]["id"].clone();
    let mark = s.seen.len();
    let step = s.call("8", "step", json!({}));
    assert_eq!(step["result"]["failed"], true);
    let failed = s.seen[mark..]
        .iter()
        .find(|line| line["method"] == "failed")
        .expect("a failed notification");
    assert_eq!(failed["params"]["restored"], true);
    let state = s.call("9", "get_state", json!({}));
    let bench = &state["result"]["marking"]["places"]["Bench"];
    assert_eq!(bench["count"], 1);
    assert_eq!(bench["tokens"][0]["id"], cup);

    let log = s.call("10", "get_log", json!({}));
    let entries = log["result"]["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "the log starts again at the second load");
    assert_eq!(entries[0]["outcome"], "failed");
    assert_eq!(entries[0]["restored"], true);

    let shutdown = s.call("11", "shutdown", json!({}));
    assert_eq!(shutdown["result"]["status"], "ok");
}

/// The binary serves the `odd-names` net: names that hold a slash, a hash,
/// a space and a double quote go out and come back exactly as given, and a
/// path step that names a near miss is refused.
#[test]
fn names_with_any_character_over_real_pipes() {
    const INBOX: &str = r#"Inbox #1 / "new""#;
    const CABINET: &str = r#"Cabinet "C" #2/3"#;
    const TRAY: &str = r#"Clerk's tray #4 / "in""#;
    const FOLDER: &str = r#"Folder "D" #5/6"#;
    const FILE_IT: &str = r#"File it #1 / "now""#;
    const SORT: &str = r#"Sort #2 / "A-Z""#;
    const TEMPLATE: &str = r#"Cabinet #9 / "steel""#;
    let mut s = Session::spawn();

    let load = s.call("1", "load", json!({"net": "odd-names"}));
    assert_eq!(load["result"]["status"], "loaded");

    let state = s.call("2", "get_state", json!({}));
    let places: Vec<&Value> = state["result"]["topology"]["places"]
        .as_array()
        .unwrap()
        .iter()
        .map(|place| &place["name"])
        .collect();
    assert_eq!(places, [INBOX, CABINET]);
    let cabinet = &state["result"]["marking"]["places"][CABINET];
    assert_eq!(cabinet["subnet"]["template"], TEMPLATE);
    assert_eq!(cabinet["ports"][TRAY]["count"], 0);

    let inside = s.call("3", "get_state", json!({"path": [{"place": CABINET}]}));
    assert_eq!(inside["result"]["net"], CABINET);
    assert_eq!(inside["result"]["template"], TEMPLATE);
    let cut = s.call(
        "4",
        "get_state",
        json!({"path": [{"place": r#"Cabinet "C" #2"#}]}),
    );
    assert_eq!(cut["error"]["code"], "unknown_net");

    let step = s.call("5", "step", json!({}));
    assert_eq!(step["result"]["transition"], FILE_IT);
    let tray = s.call(
        "6",
        "get_place_tokens",
        json!({"place": CABINET, "port": TRAY}),
    );
    assert_eq!(tray["result"]["port"], TRAY);
    assert_eq!(tray["result"]["count"], 1);

    let step = s.call("7", "step", json!({}));
    assert_eq!(step["result"]["transition"], SORT);
    assert_eq!(step["result"]["path"], json!([{"place": CABINET}]));
    let folder = s.call(
        "8",
        "get_place_tokens",
        json!({"place": FOLDER, "path": [{"place": CABINET}]}),
    );
    assert_eq!(folder["result"]["count"], 1);
    let memo = folder["result"]["tokens"][0]["id"].clone();
    let token = s.call(
        "9",
        "get_token",
        json!({"id": memo, "path": [{"place": CABINET}]}),
    );
    assert_eq!(token["result"]["place"], FOLDER);
    assert_eq!(token["result"]["value"], "Memo");

    let log = s.call("10", "get_log", json!({}));
    let transitions: Vec<&Value> = log["result"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| &entry["transition"])
        .collect();
    assert_eq!(transitions, [FILE_IT, SORT]);

    let shutdown = s.call("11", "shutdown", json!({}));
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

/// `--net <name>` carries that net alone, before or after `--describe`.
#[test]
fn the_net_option_carries_one_net() {
    for args in [
        ["--net", "workshop", "--describe"],
        ["--describe", "--net", "workshop"],
    ] {
        let output = Command::new(binary())
            .args(args)
            .output()
            .expect("the example runs");
        assert!(output.status.success(), "{args:?}");
        let describe: Value = serde_json::from_slice(&output.stdout).unwrap();
        let names: Vec<&Value> = describe["nets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|net| &net["name"])
            .collect();
        assert_eq!(names, ["workshop"], "{args:?}");
    }

    // Served, it loads that net and no other.
    let mut s = Session::spawn_with(&["--net", "odd-names"]);
    let describe = s.call("1", "describe", json!({}));
    assert_eq!(describe["result"]["nets"].as_array().unwrap().len(), 1);
    let load = s.call("2", "load", json!({"net": "odd-names"}));
    assert_eq!(load["result"]["status"], "loaded");
    let other = s.call("3", "load", json!({"net": "parcel-fleet"}));
    assert_eq!(other["error"]["code"], "unknown_net");
    let shutdown = s.call("4", "shutdown", json!({}));
    assert_eq!(shutdown["result"]["status"], "ok");
}

/// A name the binary does not carry is refused, with the names it does.
#[test]
fn the_net_option_refuses_an_unknown_name() {
    let output = Command::new(binary())
        .args(["--net", "nowhere"])
        .output()
        .expect("the example runs");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(r#"unknown net "nowhere" for --net"#)
            && stderr.contains("parcel-fleet, parcel-distribution,")
            && stderr.contains("odd-names"),
        "{stderr}"
    );

    let output = Command::new(binary())
        .arg("--net")
        .output()
        .expect("the example runs");
    assert_eq!(output.status.code(), Some(2), "a name is needed");
    assert!(String::from_utf8_lossy(&output.stderr).contains("usage"));
}

#[test]
fn other_arguments_are_a_usage_error() {
    let output = Command::new(binary())
        .arg("--frobnicate")
        .output()
        .expect("the example runs");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("usage") && stderr.contains("--net <name>"));
}
