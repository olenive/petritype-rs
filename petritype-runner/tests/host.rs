//! A net the host program owns: `Host` driven directly with a fixed clock,
//! and `HostServer` over a real TCP connection.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use petritype::{Net, NetBuilder, TakeEdge};
use petritype_runner::{Host, HostServer};
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

/// `Inbox -Double-> Out`, holding 1 and 2, and `Bad -Fail->`, empty.
fn doubler() -> Net {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    let bad = b.place::<u32>("Bad");
    b.serializable(inbox);
    b.seed(inbox, [1, 2]);
    b.transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,));
    b.transition(
        "Fail",
        (TakeEdge(bad),),
        (out,),
        |_: u32| -> Result<(u32,), Refused> { Err(Refused) },
    );
    b.build().expect("the net builds")
}

/// `In -Move-> Out`, holding `items`.
fn bin(items: &[u32]) -> Net {
    let mut b = NetBuilder::new();
    let input = b.place::<u32>("In");
    let output = b.place::<u32>("Out");
    b.seed(input, items.iter().copied());
    b.transition("Move", (TakeEdge(input),), (output,), |n: u32| (n,));
    b.build().expect("the bin builds")
}

/// A net with nothing of its own to fire, whose `Shelf` holds one bin with
/// 7 in it.
fn shelf() -> Net {
    let mut b = NetBuilder::new();
    let shelf = b.place_nested::<Net>("Shelf");
    b.seed(shelf, [bin(&[7])]);
    b.build().expect("the shelf builds")
}

// ----- the harness -------------------------------------------------------------

fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(1_791_020_102_413)
}

fn parsed(lines: Vec<String>) -> Vec<Value> {
    lines
        .iter()
        .map(|line| serde_json::from_str(line).expect("every line is JSON"))
        .collect()
}

/// What `change` did, as the lines that told of it.
fn changed<R>(host: &mut Host, change: impl FnOnce(&mut Net) -> R) -> (R, Vec<Value>) {
    let (result, lines) = host.change(now(), change);
    (result, parsed(lines))
}

/// The one line that answers `method`.
fn ask(host: &mut Host, method: &str, params: Value) -> Value {
    let request = json!({"id": "q", "method": method, "params": params}).to_string();
    let mut lines = parsed(host.request(&request, now()));
    assert_eq!(lines.len(), 1, "{lines:#?}");
    lines.remove(0)
}

fn result(host: &mut Host, method: &str, params: Value) -> Value {
    let line = ask(host, method, params);
    assert!(line.get("result").is_some(), "{line}");
    line["result"].clone()
}

/// Each line's method.
fn methods(lines: &[Value]) -> Vec<&str> {
    lines
        .iter()
        .map(|line| line["method"].as_str().expect("a notification"))
        .collect()
}

// ----- Host --------------------------------------------------------------------

#[test]
fn the_net_is_loaded_from_the_start_and_running() {
    let mut host = Host::new("doubler", doubler());
    let state = result(&mut host, "get_state", json!({}));
    assert_eq!(state["net"], "doubler");
    assert_eq!(state["generation"], 1);
    assert_eq!(state["marking_version"], 0);
    assert_eq!(state["running"], true);
    assert_eq!(state["marking"]["places"]["Inbox"]["count"], 2);

    let described = result(&mut host, "describe", json!({}));
    assert_eq!(described["nets"][0]["name"], "doubler");
}

#[test]
fn what_the_host_fires_is_told_logged_and_counted() {
    let mut host = Host::new("doubler", doubler());
    let (fired, lines) = changed(&mut host, |net| net.step());
    assert!(fired.expect("the body does not fail").is_some());
    assert_eq!(methods(&lines), ["fired"]);
    assert_eq!(lines[0]["params"]["transition"], "Double");
    assert_eq!(lines[0]["params"]["marking_version"], 1);
    assert_eq!(lines[0]["params"]["running"], true);

    // The second firing empties Inbox, so nothing is fireable any more.
    let (_, lines) = changed(&mut host, |net| net.step());
    assert_eq!(methods(&lines), ["fired", "fireable"]);
    assert_eq!(lines[1]["params"]["transitions"], json!([]));

    let state = result(&mut host, "get_state", json!({}));
    assert_eq!(state["marking_version"], 2);
    assert_eq!(state["marking"]["places"]["Out"]["count"], 2);
    assert_eq!(state["progress"]["step_count"], 2);
    assert_eq!(state["progress"]["last_fired"], "Double");

    let log = result(&mut host, "get_log", json!({}));
    let entries = log["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["transition"], "Double");
    assert_eq!(entries[0]["outcome"], "fired");
    assert_eq!(entries[0]["consumed"].as_array().map(Vec::len), Some(1));
    assert_eq!(entries[0]["produced"].as_array().map(Vec::len), Some(1));
}

#[test]
fn several_firings_in_one_change_are_told_in_order() {
    let mut host = Host::new("doubler", doubler());
    let (count, lines) = changed(&mut host, |net| {
        let mut count = 0;
        while net.step().expect("the body does not fail").is_some() {
            count += 1;
        }
        count
    });
    assert_eq!(count, 2);
    assert_eq!(methods(&lines), ["fired", "fired", "fireable"]);
    assert_eq!(lines[0]["params"]["marking_version"], 1);
    assert_eq!(lines[1]["params"]["marking_version"], 2);
}

#[test]
fn a_change_that_does_nothing_tells_nothing() {
    let mut host = Host::new("doubler", doubler());
    let (count, lines) = changed(&mut host, |net| {
        net.count(net.place_named("Inbox").expect("Inbox"))
    });
    assert_eq!(count, 2);
    assert!(lines.is_empty(), "{lines:#?}");
    assert_eq!(
        result(&mut host, "get_state", json!({}))["marking_version"],
        0
    );
}

#[test]
fn a_token_the_host_puts_in_is_told_as_added() {
    let mut host = Host::new("doubler", doubler());
    let (_, lines) = changed(&mut host, |net| {
        let inbox = net.place_ref::<u32>("Inbox").expect("Inbox holds numbers");
        net.put(inbox, 9)
    });
    assert_eq!(methods(&lines), ["added"]);
    assert_eq!(lines[0]["params"]["place"], "Inbox");
    assert_eq!(
        host.net().count(host.net().place_named("Inbox").unwrap()),
        3
    );
}

#[test]
fn a_staged_firing_is_logged_when_it_lands_with_what_it_took() {
    let mut host = Host::new("doubler", doubler());
    let (firing, lines) = changed(&mut host, |net| {
        let double = net.transition_named("Double").expect("Double");
        net.launch(double).expect("Double is fireable")
    });
    assert_eq!(methods(&lines), ["launched"]);
    assert_eq!(
        result(&mut host, "get_log", json!({}))["entries"],
        json!([])
    );

    let completed = firing.run();
    let (landed, lines) = changed(&mut host, |net| net.distribute(completed));
    landed.expect("the firing lands");
    assert_eq!(methods(&lines), ["distributed"]);

    let log = result(&mut host, "get_log", json!({}));
    let entry = &log["entries"][0];
    assert_eq!(entry["transition"], "Double");
    assert_eq!(entry["outcome"], "fired");
    assert_eq!(entry["consumed"].as_array().map(Vec::len), Some(1));
    assert_eq!(entry["produced"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        result(&mut host, "get_state", json!({}))["progress"]["step_count"],
        1
    );
}

#[test]
fn a_failed_firing_is_told_and_logged_and_not_counted() {
    let mut host = Host::new("doubler", doubler());
    let (failed, lines) = changed(&mut host, |net| {
        let bad = net.place_ref::<u32>("Bad").expect("Bad holds numbers");
        net.put(bad, 1);
        let fail = net.transition_named("Fail").expect("Fail");
        net.fire(fail)
    });
    assert!(failed.is_err());
    assert!(methods(&lines).contains(&"failed"), "{lines:#?}");

    let log = result(&mut host, "get_log", json!({}));
    assert_eq!(log["entries"][0]["outcome"], "failed");
    assert_eq!(
        result(&mut host, "get_state", json!({}))["progress"]["step_count"],
        0
    );
}

#[test]
fn the_wire_cannot_load_drive_or_change_the_net() {
    let mut host = Host::new("doubler", doubler());
    for (method, params) in [
        ("load", json!({"net": "doubler"})),
        ("reset", json!({})),
        ("shutdown", json!({})),
        ("step", json!({})),
        ("start", json!({})),
        ("stop", json!({})),
        ("inject", json!({"place": "Inbox", "token": 5})),
        ("enable", json!({"transition": "Double"})),
        ("disable", json!({"transition": "Double"})),
        ("set_param", json!({"param": "limit", "value": 1})),
        ("press", json!({"control": "go"})),
        ("set_filter", json!({})),
    ] {
        let line = ask(&mut host, method, params);
        assert_eq!(line["error"]["code"], "unsupported", "{method}: {line}");
    }
    assert_eq!(
        ask(&mut host, "dance", json!({}))["error"]["code"],
        "unknown_method"
    );
    let state = result(&mut host, "get_state", json!({}));
    assert_eq!(state["marking_version"], 0);
    assert_eq!(state["marking"]["places"]["Inbox"]["count"], 2);
}

#[test]
fn a_read_never_fires_the_net() {
    // `running` is true, and a runner's own net would take a firing after
    // answering. The host's is the program's to fire.
    let mut host = Host::new("doubler", doubler());
    for _ in 0..3 {
        result(&mut host, "get_state", json!({}));
    }
    assert_eq!(host.net().count(host.net().place_named("Out").unwrap()), 0);
}

#[test]
fn a_line_that_is_not_a_request_is_invalid_params() {
    let mut host = Host::new("doubler", doubler());
    let lines = parsed(host.request("not json", now()));
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["error"]["code"], "invalid_params");
}

#[test]
fn set_running_changes_what_every_later_line_says() {
    let mut host = Host::new("doubler", doubler());
    host.set_running(false);
    assert_eq!(result(&mut host, "get_state", json!({}))["running"], false);
    let (_, lines) = changed(&mut host, |net| net.step());
    assert_eq!(lines[0]["params"]["running"], false);
    let heartbeat: Value = serde_json::from_str(&host.heartbeat()).expect("JSON");
    assert_eq!(heartbeat["method"], "heartbeat");
    assert_eq!(heartbeat["params"]["running"], false);
    assert_eq!(heartbeat["params"]["marking_version"], 1);
}

#[test]
fn a_firing_inside_a_token_is_told_nested_and_its_net_can_be_read() {
    let mut host = Host::new("shelf", shelf());
    let state = result(&mut host, "get_state", json!({}));
    let held = &state["marking"]["places"]["Shelf"]["tokens"][0]["nets"][0];
    let nested_id = held["nested_id"].as_str().expect("a nested id").to_owned();

    let (step, lines) = changed(&mut host, |net| net.step_deep());
    assert!(step.expect("the body does not fail").is_some());
    assert_eq!(methods(&lines)[0], "nested", "{lines:#?}");
    assert_eq!(lines[0]["params"]["nested_id"], nested_id.as_str());
    assert_eq!(lines[0]["params"]["event"]["method"], "fired");
    assert_eq!(lines[0]["params"]["event"]["params"]["transition"], "Move");
    // The outer net's version does not move for what happens inside.
    assert_eq!(lines[0]["params"]["marking_version"], 0);

    let inner = result(&mut host, "get_state", json!({"nested_id": nested_id}));
    assert_eq!(inner["marking"]["places"]["Out"]["count"], 1);
    assert_eq!(inner["marking_version"], 1);
    assert_eq!(inner["progress"]["step_count"], 1);

    // The outer net counts the firing too, and the log holds it.
    let outer = result(&mut host, "get_state", json!({}));
    assert_eq!(outer["progress"]["step_count"], 1);
    let log = result(&mut host, "get_log", json!({}));
    assert_eq!(log["entries"][0]["transition"], "Move");
    assert_eq!(log["entries"][0]["nested_id"], nested_id.as_str());
}

#[test]
fn a_net_inside_a_token_the_host_puts_in_is_known_at_once() {
    let mut host = Host::new("shelf", shelf());
    let (_, lines) = changed(&mut host, |net| {
        let shelf = net.place_ref::<Net>("Shelf").expect("Shelf holds nets");
        net.put(shelf, bin(&[1, 2]))
    });
    assert_eq!(methods(&lines), ["added"]);
    let state = result(&mut host, "get_state", json!({}));
    let tokens = state["marking"]["places"]["Shelf"]["tokens"]
        .as_array()
        .expect("tokens");
    assert_eq!(tokens.len(), 2);
    let nested_id = tokens[1]["nets"][0]["nested_id"].as_str().expect("an id");
    let inner = result(&mut host, "get_state", json!({"nested_id": nested_id}));
    assert_eq!(inner["marking"]["places"]["In"]["count"], 2);
}

// ----- HostServer --------------------------------------------------------------

#[test]
fn a_host_and_its_server_can_be_shared_between_threads() {
    fn shared<T: Send + Sync + 'static>() {}
    shared::<Host>();
    shared::<HostServer>();
}

/// A viewer on a real connection, reading whole lines.
struct Viewer {
    stream: TcpStream,
    reader: BufReader<TcpStream>,
}

impl Viewer {
    fn connect(server: &HostServer) -> Viewer {
        let stream = TcpStream::connect(server.local_addr()).expect("the server listens");
        stream
            .set_read_timeout(Some(Duration::from_millis(20)))
            .expect("a timeout");
        let reader = BufReader::new(stream.try_clone().expect("a second handle"));
        Viewer { stream, reader }
    }

    fn send(&mut self, id: &str, method: &str) {
        let line = json!({"id": id, "method": method}).to_string();
        writeln!(self.stream, "{line}").expect("the server reads");
    }

    /// The next line, serving the server while waiting for it.
    fn next(&mut self, server: &mut HostServer) -> Value {
        self.maybe_next(server, Duration::from_secs(5))
            .expect("a line within five seconds")
    }

    /// The next line if one comes within `patience`.
    fn maybe_next(&mut self, server: &mut HostServer, patience: Duration) -> Option<Value> {
        let deadline = Instant::now() + patience;
        let mut line = String::new();
        loop {
            server.serve();
            match self.reader.read_line(&mut line) {
                Ok(0) => panic!("the server closed the connection"),
                Ok(_) if line.ends_with('\n') => {
                    return Some(serde_json::from_str(&line).expect("every line is JSON"));
                }
                // A timeout, perhaps part way through a line: keep reading.
                Ok(_) | Err(_) => {}
            }
            if Instant::now() >= deadline {
                return None;
            }
        }
    }
}

/// Serves until `viewers` are connected.
fn wait_for_viewers(server: &mut HostServer, viewers: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while server.viewers() != viewers {
        assert!(Instant::now() < deadline, "the viewers never arrived");
        server.serve();
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn a_viewer_attaches_reads_the_state_and_follows_the_program() {
    let mut server = HostServer::bind("127.0.0.1:0", "doubler", doubler()).expect("a free port");
    let mut viewer = Viewer::connect(&server);
    wait_for_viewers(&mut server, 1);

    viewer.send("1", "get_state");
    let state = viewer.next(&mut server);
    assert_eq!(state["id"], "1");
    assert_eq!(state["result"]["net"], "doubler");
    assert_eq!(state["result"]["marking_version"], 0);

    let fired = server.change(|net| net.step());
    assert!(fired.expect("the body does not fail").is_some());
    let line = viewer.next(&mut server);
    assert_eq!(line["method"], "fired");
    assert_eq!(line["params"]["marking_version"], 1);

    viewer.send("2", "step");
    let refused = viewer.next(&mut server);
    assert_eq!(refused["id"], "2");
    assert_eq!(refused["error"]["code"], "unsupported");
}

#[test]
fn a_viewer_hears_nothing_before_its_first_request_and_answers_are_its_own() {
    let mut server = HostServer::bind("127.0.0.1:0", "doubler", doubler()).expect("a free port");
    let mut first = Viewer::connect(&server);
    let mut second = Viewer::connect(&server);
    wait_for_viewers(&mut server, 2);

    first.send("1", "get_state");
    assert_eq!(first.next(&mut server)["id"], "1");

    // Only the first has asked anything: only it is told of the firing.
    server
        .change(|net| net.step())
        .expect("the body does not fail");
    assert_eq!(first.next(&mut server)["method"], "fired");
    assert!(
        second
            .maybe_next(&mut server, Duration::from_millis(100))
            .is_none()
    );

    // The second attaches late and reads the state as it is now.
    second.send("1", "get_state");
    let state = second.next(&mut server);
    assert_eq!(state["id"], "1");
    assert_eq!(state["result"]["marking_version"], 1);
    // Its answer went to it alone.
    assert!(
        first
            .maybe_next(&mut server, Duration::from_millis(100))
            .is_none()
    );

    // From here both follow.
    server
        .change(|net| net.step())
        .expect("the body does not fail");
    assert_eq!(first.next(&mut server)["method"], "fired");
    assert_eq!(second.next(&mut server)["method"], "fired");
}

#[test]
fn a_viewer_that_leaves_is_forgotten_and_the_program_goes_on() {
    let mut server = HostServer::bind("127.0.0.1:0", "doubler", doubler()).expect("a free port");
    let mut viewer = Viewer::connect(&server);
    wait_for_viewers(&mut server, 1);
    viewer.send("1", "get_state");
    viewer.next(&mut server);

    drop(viewer);
    wait_for_viewers(&mut server, 0);
    server
        .change(|net| net.step())
        .expect("the body does not fail");
    assert_eq!(
        server.net().count(server.net().place_named("Out").unwrap()),
        1
    );
}
