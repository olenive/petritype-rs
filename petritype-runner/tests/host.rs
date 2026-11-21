//! A net the host program owns: `Host` driven directly with a fixed clock,
//! and `HostServer` over a real TCP connection.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use petritype::{Firing, Nested, Net, NetBuilder, NetId, TakeEdge};
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

// ----- a host nobody is listening to ---------------------------------------------

/// A token carrying one bin. It prints as `crate` and nothing more: a net's
/// own `Debug` prints its id, which is unique in the process, and two yards
/// built alike must label their crates alike.
struct Crate(Net);

impl std::fmt::Debug for Crate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("crate")
    }
}

impl Nested for Crate {
    fn nets(&self) -> Vec<(&str, &Net)> {
        vec![("bin", &self.0)]
    }

    fn nets_mut(&mut self) -> Vec<(&str, &mut Net)> {
        vec![("bin", &mut self.0)]
    }
}

/// `Inbox -Double-> Out`, holding 1, 2 and 3, and `Shelf -Ship-> Dock`,
/// whose crates carry bins: on the shelf, one holding 7 and one empty. The
/// bins' ids go into `ids`, in the order they are made.
fn yard(ids: &mut Vec<NetId>) -> Net {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    let shelf = b.place_nested::<Crate>("Shelf");
    let dock = b.place_nested::<Crate>("Dock");
    b.seed(inbox, [1, 2, 3]);
    b.seed(shelf, [packed(&[7], ids), packed(&[], ids)]);
    b.transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,));
    b.transition("Ship", (TakeEdge(shelf),), (dock,), |c: Crate| (c,));
    b.build().expect("the yard builds")
}

/// A crate carrying a bin that holds `items`, its id added to `ids`.
fn packed(items: &[u32], ids: &mut Vec<NetId>) -> Crate {
    let bin = bin(items);
    ids.push(bin.id());
    Crate(bin)
}

/// One change the program makes to a yard.
#[derive(Debug, Clone, Copy)]
enum Act {
    /// Fires an outer transition in one piece.
    Fire(&'static str),
    /// Enables or disables an outer transition.
    Toggle(&'static str, bool),
    /// Puts a new crate on the shelf, its bin holding these.
    PutCrate(&'static [u32]),
    /// Launches an outer transition, to be distributed by a later `Land`.
    Launch(&'static str),
    /// Distributes the firing in flight.
    Land,
    /// Changes the bin in the crate at this position in this place, oldest
    /// first.
    InBin(&'static str, usize, Inside),
}

/// What an [`Act::InBin`] does inside the bin.
#[derive(Debug, Clone, Copy)]
enum Inside {
    /// One step of the bin's own.
    Step,
    /// Puts a number in its `In`, which gives `Move` something to fire.
    PutIn(u32),
    /// Puts a number in its `Out`, which changes nothing that can fire.
    PutOut(u32),
}

use Act::{Fire, Land, Launch, PutCrate, Toggle};
use Inside::{PutIn, PutOut, Step};

/// One yard as the program drives it: its bins' ids, in the order they were
/// made, and the staged firing in flight.
#[derive(Default)]
struct Run {
    ids: Vec<NetId>,
    flight: Option<Firing>,
}

impl Run {
    fn act(&mut self, net: &mut Net, act: Act) {
        let transition = |net: &Net, name: &str| net.transition_named(name).expect(name);
        match act {
            Fire(name) => net.fire(transition(net, name)).expect("the firing works"),
            Toggle(name, on) => {
                let t = transition(net, name);
                let toggled = if on { net.enable(t) } else { net.disable(t) };
                toggled.expect("a transition");
            }
            PutCrate(items) => {
                let shelf = net.place_ref::<Crate>("Shelf").expect("Shelf holds crates");
                net.put(shelf, packed(items, &mut self.ids));
            }
            Launch(name) => {
                let firing = net.launch(transition(net, name)).expect("it is fireable");
                assert!(
                    self.flight.replace(firing).is_none(),
                    "one flight at a time"
                );
            }
            Land => {
                let completed = self.flight.take().expect("a firing in flight").run();
                net.distribute(completed).expect("it lands");
            }
            Act::InBin(place, nth, inside) => {
                let place = net.place_named(place).expect("the place");
                let (token, _) = net.tokens(place).nth(nth).expect("the crate");
                net.nested_scope(place, token, |nets| {
                    let bin = &mut *nets[0].1;
                    let put = |bin: &mut Net, place: &str, n: u32| {
                        let place = bin.place_ref::<u32>(place).expect("a bin place");
                        bin.put(place, n);
                    };
                    match inside {
                        Step => {
                            bin.step().expect("the body does not fail");
                        }
                        PutIn(n) => put(bin, "In", n),
                        PutOut(n) => put(bin, "Out", n),
                    }
                })
                .expect("a crate in a nested place");
            }
        }
    }
}

/// Makes `act` on `host`'s yard and returns the lines, unparsed.
fn make(host: &mut Host, run: &mut Run, act: Act) -> Vec<String> {
    host.change(now(), |net| run.act(net, act)).1
}

/// `line` with each id of `ids` written as its place in the list, as a
/// `nested_id` and in an `unknown_net` message. Two yards built alike
/// differ in nothing else: a net's id is unique in the process.
fn alike(line: &str, ids: &[NetId]) -> String {
    ids.iter()
        .enumerate()
        .fold(line.to_owned(), |line, (at, id)| {
            line.replace(
                &format!(r#""nested_id":"{id}""#),
                &format!(r#""nested_id":"bin {at}""#),
            )
            .replace(
                &format!(r#"nested id \"{id}\""#),
                &format!(r#"nested id \"bin {at}\""#),
            )
        })
}

/// Everything a viewer can read of a yard: the outer net's state, its log,
/// and each bin's state, each line as `alike` writes it.
fn everything(host: &mut Host, ids: &[NetId]) -> Vec<String> {
    let mut asked = vec![
        r#"{"id":"s","method":"get_state"}"#.to_owned(),
        r#"{"id":"l","method":"get_log"}"#.to_owned(),
    ];
    asked.extend(ids.iter().map(|id| {
        json!({"id": "b", "method": "get_state", "params": {"nested_id": id.to_string()}})
            .to_string()
    }));
    asked
        .iter()
        .flat_map(|request| host.request(request, now()))
        .map(|line| alike(&line, ids))
        .collect()
}

/// The transitions `get_state` reports fireable in the net `nested_id`
/// names, the outer net for `None`, sorted; `None` when it cannot be read
/// (its token is in a body's hands).
fn fireable_read(host: &mut Host, nested_id: Option<&str>) -> Option<Vec<String>> {
    let params = nested_id.map_or_else(|| json!({}), |id| json!({"nested_id": id}));
    let line = ask(host, "get_state", params);
    let transitions = line["result"]["transitions"].as_object()?;
    let mut names: Vec<String> = transitions
        .iter()
        .filter(|(_, t)| t["fireable"] == true)
        .map(|(name, _)| name.clone())
        .collect();
    names.sort();
    Some(names)
}

/// The set the `fireable` line among `lines` tells for the net `nested_id`
/// names, the outer net for `None`, sorted, if a line tells one.
fn fireable_told(lines: &[Value], nested_id: Option<&str>) -> Option<Vec<String>> {
    let mut told = lines.iter().filter_map(|line| {
        let (event, id) = match line["method"].as_str() {
            Some("nested") => (
                &line["params"]["event"],
                line["params"]["nested_id"].as_str(),
            ),
            _ => (line, None),
        };
        (event["method"] == "fireable" && id == nested_id).then(|| {
            let mut names: Vec<String> = event["params"]["transitions"]
                .as_array()
                .expect("transitions")
                .iter()
                .map(|name| name.as_str().expect("a name").to_owned())
                .collect();
            names.sort();
            names
        })
    });
    let first = told.next();
    assert!(
        told.next().is_none(),
        "one fireable line per net: {lines:#?}"
    );
    first
}

/// What a viewer holds of each fireable set, the outer net's and then each
/// bin's in the order of `ids`: what `get_state` reported, or a `fireable`
/// line told it since. `None` for a net it could not read.
struct Held(Vec<Option<Vec<String>>>);

impl Held {
    /// The nets, in the order kept: `None` for the outer one.
    fn nets(ids: &[NetId]) -> Vec<Option<String>> {
        std::iter::once(None)
            .chain(ids.iter().map(|id| Some(id.to_string())))
            .collect()
    }

    /// What a viewer that has just read every net holds.
    fn read(host: &mut Host, ids: &[NetId]) -> Held {
        Held(
            Held::nets(ids)
                .iter()
                .map(|id| fireable_read(host, id.as_deref()))
                .collect(),
        )
    }

    /// Asserts that `lines`, those of one change, tell each net's fireable
    /// set exactly when it now differs from what the viewer holds, and
    /// takes in what they told. A net the viewer could not read before is
    /// left out of the rule: it has nothing to differ from.
    fn follow(&mut self, host: &mut Host, ids: &[NetId], lines: &[Value]) {
        let now = Held::read(host, ids);
        self.0.resize(now.0.len(), None);
        for (at, id) in Held::nets(ids).iter().enumerate() {
            let told = fireable_told(lines, id.as_deref());
            if let (Some(held), Some(now)) = (&self.0[at], &now.0[at]) {
                let expected = (now != held).then(|| now.clone());
                assert_eq!(told, expected, "net {id:?} after {lines:#?}");
            }
        }
        *self = now;
    }
}

#[test]
fn a_host_quiet_for_a_while_tells_what_one_listening_throughout_tells() {
    let (mut watched_run, mut quiet_run) = (Run::default(), Run::default());
    let mut watched = Host::new("yard", yard(&mut watched_run.ids));
    let mut quiet = Host::new("yard", yard(&mut quiet_run.ids));
    quiet.set_listening(false);

    let while_quiet = [
        Fire("Double"),
        // A crate with a bin nobody has seen, put in while quiet.
        PutCrate(&[4, 5]),
        // The bin holding 7 empties: its `Move` is no longer fireable.
        Act::InBin("Shelf", 0, Step),
        Act::InBin("Shelf", 2, Step),
        // The emptied bin goes to the dock in one piece, the empty one staged.
        Fire("Ship"),
        Toggle("Double", false),
        Launch("Ship"),
        Land,
        // Both docked bins get something to move again.
        Act::InBin("Dock", 0, PutIn(8)),
        Act::InBin("Dock", 1, PutIn(3)),
    ];
    for act in while_quiet {
        assert!(!make(&mut watched, &mut watched_run, act).is_empty());
        assert_eq!(make(&mut quiet, &mut quiet_run, act), Vec::<String>::new());
    }

    quiet.set_listening(true);
    assert_eq!(
        everything(&mut quiet, &quiet_run.ids),
        everything(&mut watched, &watched_run.ids)
    );

    let from_the_switch = [
        // Double comes back: the outer set is the one from before the quiet
        // part again, and differs from what the quiet part left.
        Toggle("Double", true),
        // Nothing new can fire in this bin: no `fireable` line.
        Act::InBin("Dock", 1, PutOut(1)),
        Act::InBin("Shelf", 0, Step),
        Act::InBin("Dock", 0, Step),
        Fire("Double"),
        PutCrate(&[6]),
        Launch("Ship"),
        Land,
        Act::InBin("Shelf", 0, Step),
        Fire("Double"),
        Fire("Ship"),
    ];
    for act in from_the_switch {
        let told = |lines: Vec<String>, ids: &[NetId]| -> Vec<String> {
            lines.iter().map(|line| alike(line, ids)).collect()
        };
        let watched_lines = make(&mut watched, &mut watched_run, act);
        let quiet_lines = make(&mut quiet, &mut quiet_run, act);
        assert!(!watched_lines.is_empty(), "{act:?} tells something");
        assert_eq!(
            told(quiet_lines, &quiet_run.ids),
            told(watched_lines, &watched_run.ids),
            "{act:?}"
        );
        assert_eq!(
            everything(&mut quiet, &quiet_run.ids),
            everything(&mut watched, &watched_run.ids),
            "after {act:?}"
        );
    }
}

#[test]
fn the_first_fireable_line_after_the_switch_follows_what_the_viewer_read() {
    let mut run = Run::default();
    let mut host = Host::new("yard", yard(&mut run.ids));
    host.set_listening(false);
    // Quietly: the outer set loses Double, the full bin is emptied and the
    // empty one is given something to move.
    for act in [
        Toggle("Double", false),
        Act::InBin("Shelf", 0, Step),
        Act::InBin("Shelf", 1, PutIn(3)),
    ] {
        assert!(make(&mut host, &mut run, act).is_empty());
    }

    host.set_listening(true);
    let mut held = Held::read(&mut host, &run.ids);
    assert_eq!(held.0[0], Some(vec!["Ship".to_owned()]));
    assert_eq!(held.0[1], Some(vec![]));
    assert_eq!(held.0[2], Some(vec!["Move".to_owned()]));

    for (act, expected) in [
        // The filled bin's set stays what the viewer read, which is not
        // what it was before the quiet part.
        (Act::InBin("Shelf", 1, PutOut(1)), vec!["nested"]),
        // The emptied bin can move again: it differs from what was read,
        // though not from what it was before the quiet part.
        (Act::InBin("Shelf", 0, PutIn(2)), vec!["nested", "nested"]),
        // The outer set is still `Ship` alone, as read.
        (Fire("Ship"), vec!["fired"]),
        // Double comes back, as it was before the quiet part.
        (Toggle("Double", true), vec!["toggled", "fireable"]),
    ] {
        let lines = parsed(make(&mut host, &mut run, act));
        assert_eq!(methods(&lines), expected, "{act:?}");
        held.follow(&mut host, &run.ids, &lines);
    }
}

#[test]
fn a_staged_firing_may_cross_the_switch_either_way() {
    // Launched while quiet, distributed after the switch.
    let mut run = Run::default();
    let mut host = Host::new("yard", yard(&mut run.ids));
    host.set_listening(false);
    for act in [Act::InBin("Shelf", 0, Step), Launch("Ship")] {
        assert!(make(&mut host, &mut run, act).is_empty());
    }
    host.set_listening(true);
    let mut held = Held::read(&mut host, &run.ids);
    // The emptied bin is in the body's hands, and cannot be read.
    assert_eq!(held.0[1], None);
    let lines = parsed(make(&mut host, &mut run, Land));
    assert_eq!(methods(&lines)[0], "distributed");
    // Once back, the bin that changed while quiet is told of.
    assert_eq!(
        fireable_told(&lines, Some(&run.ids[0].to_string())),
        Some(vec![])
    );
    held.follow(&mut host, &run.ids, &lines);
    let lines = parsed(make(&mut host, &mut run, Act::InBin("Dock", 0, PutIn(5))));
    held.follow(&mut host, &run.ids, &lines);
    assert!(fireable_told(&lines, Some(&run.ids[0].to_string())).is_some());
    // The firing's log entry has what it took, from before the switch.
    let log = result(&mut host, "get_log", json!({}));
    let shipped = &log["entries"][1];
    assert_eq!(shipped["transition"], "Ship");
    assert_eq!(shipped["consumed"].as_array().map(Vec::len), Some(1));
    assert_eq!(shipped["produced"].as_array().map(Vec::len), Some(1));

    // Launched while listening, distributed while quiet.
    let mut run = Run::default();
    let mut host = Host::new("yard", yard(&mut run.ids));
    let lines = parsed(make(&mut host, &mut run, Launch("Ship")));
    assert_eq!(methods(&lines), ["launched"]);
    host.set_listening(false);
    for act in [Land, Act::InBin("Dock", 0, Step), Toggle("Double", false)] {
        assert!(make(&mut host, &mut run, act).is_empty());
    }
    host.set_listening(true);
    let mut held = Held::read(&mut host, &run.ids);
    for act in [
        Act::InBin("Dock", 0, PutIn(1)),
        Fire("Ship"),
        Toggle("Double", true),
    ] {
        let lines = parsed(make(&mut host, &mut run, act));
        held.follow(&mut host, &run.ids, &lines);
    }
    let log = result(&mut host, "get_log", json!({}));
    let landed = &log["entries"][0];
    assert_eq!(landed["transition"], "Ship");
    assert_eq!(landed["consumed"].as_array().map(Vec::len), Some(1));
    assert_eq!(landed["produced"].as_array().map(Vec::len), Some(1));
}

#[test]
fn a_quiet_host_makes_no_lines_and_keeps_its_log_and_progress() {
    let mut watched = Host::new("doubler", doubler());
    let mut quiet = Host::new("doubler", doubler());
    quiet.set_listening(false);
    let changes: [fn(&mut Net, &mut Option<Firing>); 6] = [
        |net, _| {
            net.step().expect("the body does not fail");
        },
        |net, _| {
            net.step().expect("the body does not fail");
        },
        |net, _| {
            net.put(net.place_ref::<u32>("Inbox").expect("Inbox"), 5);
        },
        |net, _| {
            net.put(net.place_ref::<u32>("Bad").expect("Bad"), 1);
            let fail = net.transition_named("Fail").expect("Fail");
            assert!(net.fire(fail).is_err());
        },
        |net, flight| {
            let double = net.transition_named("Double").expect("Double");
            *flight = Some(net.launch(double).expect("Double is fireable"));
        },
        |net, flight| {
            let completed = flight.take().expect("in flight").run();
            net.distribute(completed).expect("it lands");
        },
    ];
    let (mut watched_flight, mut quiet_flight) = (None, None);
    for change in changes {
        let (_, lines) = watched.change(now(), |net| change(net, &mut watched_flight));
        assert!(!lines.is_empty());
        let (_, lines) = quiet.change(now(), |net| change(net, &mut quiet_flight));
        assert!(lines.is_empty(), "{lines:#?}");
    }

    // Read while still quiet: the same as the host that told everything.
    let log = result(&mut quiet, "get_log", json!({}));
    let outcomes: Vec<&str> = log["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|entry| entry["outcome"].as_str().expect("an outcome"))
        .collect();
    assert_eq!(outcomes, ["fired", "fired", "failed", "fired"]);
    let state = result(&mut quiet, "get_state", json!({}));
    assert_eq!(state["progress"]["step_count"], 3);
    assert_eq!(state["progress"]["fired_counts"]["Double"], 3);
    for method in ["get_state", "get_log"] {
        let request = json!({"id": "q", "method": method}).to_string();
        assert_eq!(
            quiet.request(&request, now()),
            watched.request(&request, now()),
            "{method}"
        );
    }
    assert_eq!(quiet.heartbeat(), watched.heartbeat());
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

/// Whether `server`'s host is telling, as its `Debug` says.
fn listening(server: &HostServer) -> bool {
    let shown = format!("{server:?}");
    assert!(shown.contains("listening: "), "{shown}");
    shown.contains("listening: true")
}

/// Puts `n` in the doubler's `Inbox`.
fn put_in(server: &mut HostServer, n: u32) {
    server.change(|net| net.put(net.place_ref::<u32>("Inbox").expect("Inbox"), n));
}

/// Fires `Double` until `Inbox` is empty.
fn double_all(server: &mut HostServer) {
    server.change(|net| while net.step().expect("the body does not fail").is_some() {});
}

/// What a viewer that has just attached is told: the state after a long
/// unwatched run, then the next change, with the fireable set it read.
fn attach_and_follow(server: &mut HostServer, version: u64, fired: u64) {
    let mut viewer = Viewer::connect(server);
    wait_for_viewers(server, 1);
    // Connected and silent, it does not count as listening.
    assert!(!listening(server));

    viewer.send("1", "get_state");
    let state = viewer.next(server);
    assert_eq!(state["result"]["marking_version"], version);
    assert_eq!(state["result"]["progress"]["step_count"], fired);
    assert_eq!(state["result"]["marking"]["places"]["Out"]["count"], fired);
    assert_eq!(state["result"]["transitions"]["Double"]["fireable"], false);
    assert!(listening(server));

    // Double can fire again: it differs from what the viewer read, so the
    // viewer is told, although it is what the host knew before it was quiet.
    put_in(server, 9);
    let added = viewer.next(server);
    assert_eq!(added["method"], "added");
    assert_eq!(added["params"]["marking_version"], version + 1);
    let fireable = viewer.next(server);
    assert_eq!(fireable["method"], "fireable");
    assert_eq!(fireable["params"]["transitions"], json!(["Double"]));

    drop(viewer);
    wait_for_viewers(server, 0);
    assert!(!listening(server));
}

#[test]
fn a_server_nobody_watches_is_quiet_until_a_viewer_asks_and_after_the_last_leaves() {
    let mut server = HostServer::bind("127.0.0.1:0", "doubler", doubler()).expect("a free port");
    assert!(!listening(&server));
    // A long run with nobody attached: a token in and a firing, many times
    // over, then Inbox emptied.
    for n in 0..500 {
        put_in(&mut server, n);
        server
            .change(|net| net.step())
            .expect("the body does not fail");
    }
    double_all(&mut server);
    attach_and_follow(&mut server, 1_002, 502);

    // Quiet again, and the next viewer is served as the first was.
    double_all(&mut server);
    attach_and_follow(&mut server, 1_004, 503);
}
