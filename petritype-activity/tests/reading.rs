//! What reading a saved file refuses, and what it keeps (since 2026-11-28):
//! a value no model, log or record could have written is refused with
//! where it was, rather than read into a state the lookups and the replays
//! cannot stand on; and a file an earlier version wrote, which its own
//! slips left a little untidy, still reads, tidied.

#![cfg(feature = "serde")]

use std::time::Duration;

use petritype::style::Rgb;
use petritype::{NetBuilder, PlaceId, TakeEdge, TokenId, TransitionId};
use petritype_activity::{
    Activity, ActivitySettings, Delta, Entry, EventLog, Grid, Look, Record, Recorder, Roster,
    Standing, TransitionDelta,
};
use petritype_layout::Layout;
use serde_json::{Value, json};

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// `In -> Go -> Out`, laid out, and a record of three frames of it, one
/// bin each.
fn record() -> Record {
    let mut b = NetBuilder::new();
    let input = b.place::<u32>("In");
    let out = b.place::<u32>("Out");
    b.seed(input, [1, 2]);
    b.transition("Go", (TakeEdge(input),), (out,), |n: u32| (n,));
    let mut net = b.build().expect("the net builds");
    let mut layout = Layout::default();
    layout.compute(&net);
    let grid = Grid::from_layout(&layout, &net.topology());
    let mut live = Activity::from_net(&net, ActivitySettings::default());
    let mut recorder = Recorder::new(ms(100), None, None);
    for _ in 0..3 {
        let _ = net.step().expect("a step");
        for event in net.drain_events() {
            live.apply(&event);
        }
        live.tick(ms(100));
        recorder.observe(live.take_delta(), ms(100));
    }
    recorder.record(&grid, Some(live.settings()))
}

fn as_value(record: &Record) -> Value {
    let mut text = Vec::new();
    record.write_json(&mut text).expect("it writes");
    serde_json::from_slice(&text).expect("JSON")
}

fn read_record(value: &Value) -> std::io::Result<Record> {
    Record::read_json(value.to_string().as_bytes())
}

#[test]
fn a_colour_written_any_way_but_rrggbb_is_refused() {
    let read = |text: &str| serde_json::from_str::<Look>(text);
    assert_eq!(
        read(r##"{"label":"car","colour":"#c81e28"}"##)
            .expect("a colour")
            .colour,
        Some(Rgb(200, 30, 40))
    );
    assert_eq!(read(r#"{"label":"car"}"#).expect("no colour").colour, None);
    assert_eq!(
        read(r#"{"label":"car","colour":null}"#)
            .expect("none")
            .colour,
        None
    );
    for bad in ["red", "#12345", "c81e28", "#c81e2g", ""] {
        let text = json!({"label": "car", "colour": bad}).to_string();
        let error = read(&text).expect_err(bad).to_string();
        assert!(error.contains("#rrggbb"), "{bad}: {error}");
    }
}

#[test]
fn settings_refuse_a_number_that_is_negative_or_not_finite() {
    let read = |text: &str| serde_json::from_str::<ActivitySettings>(text);
    let layer = |jump: &str| {
        format!(
            r#"{{"pulse": {{"jump": {jump}, "clock": "wall", "half_life_secs": 0.3, "half_life_firings": 3}}}}"#
        )
    };
    for bad in [
        r#"{"flow_gain": -0.25}"#.to_owned(),
        r#"{"stale_half_life_secs": -1}"#.to_owned(),
        r#"{"stale_floor": -0.5}"#.to_owned(),
        r#"{"count_cap": -16}"#.to_owned(),
        layer("-1"),
        // An `f32` cannot hold 1e39: it reads as infinite.
        layer("1e39"),
    ] {
        let error = read(&bad).expect_err(&bad).to_string();
        assert!(
            error.contains("finite number of zero or more"),
            "{bad}: {error}"
        );
    }
    assert_eq!(
        read(r#"{"stale_half_life_secs": null}"#)
            .expect("no dim")
            .stale_half_life_secs,
        None
    );
    assert_eq!(read(r#"{"half_count": 0}"#).expect("zero").half_count, 0.0);
    assert!(read(&layer("0.5")).is_ok());
    let defaults = serde_json::to_string(&ActivitySettings::default()).expect("they write");
    assert_eq!(
        read(&defaults).expect("they read"),
        ActivitySettings::default()
    );
}

#[test]
fn a_delta_refuses_a_time_in_flight_below_zero() {
    let read = |text: &str| serde_json::from_str::<TransitionDelta>(text);
    assert!(read(r#"{"in_flight_ms": -1.0}"#).is_err());
    assert_eq!(
        read(r#"{"in_flight_ms": 2.5}"#)
            .expect("a time")
            .in_flight_ms,
        2.5
    );
    assert!(
        serde_json::from_str::<Delta>(r#"{"transitions": {"0": {"in_flight_ms": -3}}}"#).is_err()
    );
}

#[test]
fn a_record_refuses_a_bin_width_of_zero() {
    let mut value = as_value(&record());
    assert_eq!(value["bin_width_ms"], 100);
    value["bin_width_ms"] = json!(1);
    assert!(read_record(&value).is_ok());
    value["bin_width_ms"] = json!(0);
    let error = read_record(&value).expect_err("no width").to_string();
    assert!(error.contains("at least 1 ms"), "{error}");
}

#[test]
fn a_grid_too_large_to_draw_is_refused_whatever_its_size() {
    let good = as_value(&record());
    let back = read_record(&good).expect("the record reads");
    assert_eq!(back, record());
    // Columns times rows past a `u64`, past the cap, and at the cap's
    // square root plus one on each side.
    for (columns, rows) in [(1_u64 << 40, 1_u64 << 40), (3000, 3000), (2049, 2049)] {
        let mut value = good.clone();
        value["grid"]["columns"] = json!(columns);
        value["grid"]["rows"] = json!(rows);
        let error = read_record(&value).expect_err("too large").to_string();
        assert!(error.contains("positions"), "{columns} by {rows}: {error}");
    }
    assert_eq!(Grid::MAX_POSITIONS, 2048 * 2048);
}

#[test]
fn a_record_whose_grid_names_a_large_id_replays_on_a_model_as_small_as_its_cells() {
    let mut value = as_value(&record());
    // The grid's first cell, `In`, renamed to an id far past the others.
    let node = value["grid"]["cells"][0]["node"]
        .as_object_mut()
        .expect("a node");
    assert_eq!(node.get("place"), Some(&json!(0)));
    node.insert("place".to_owned(), json!(1_000_000));
    let record = read_record(&value).expect("a sparse grid reads");
    let model = record.model(ActivitySettings::default());
    assert_eq!(
        (model.place_count(), model.transition_count()),
        (2, 1),
        "a slot for each cell, and none for the ids between"
    );
    assert_eq!(model.place_named("In"), Some(PlaceId::new(1_000_000)));
    assert_eq!(model.place(PlaceId::new(0)), None);
    let mut replay = record.replay(ActivitySettings::default());
    while replay.next_bin().is_some() {}
    assert_eq!(replay.folded(), 3);
}

#[test]
fn a_standing_state_out_of_order_or_naming_an_id_twice_is_refused() {
    let read = |text: &str| serde_json::from_str::<Standing>(text);
    for bad in [
        r#"{"counts": [[1, 2], [0, 1]]}"#,
        r#"{"counts": [[0, 1], [0, 2]]}"#,
        r#"{"in_flight": [2, 1]}"#,
        r#"{"in_flight": [1, 1]}"#,
    ] {
        assert!(read(bad).is_err(), "{bad}");
    }
    // A place at 0 names nothing, and is left out.
    let zero = read(r#"{"counts": [[0, 0], [1, 2]], "in_flight": [0, 3]}"#).expect("it reads");
    assert_eq!(
        zero,
        read(r#"{"counts": [[1, 2]], "in_flight": [0, 3]}"#).unwrap()
    );
    assert_eq!(zero.count(PlaceId::new(1)), 2);
    assert!(zero.is_in_flight(TransitionId::new(3)));
    assert_eq!(read("{}").expect("empty"), Standing::default());
}

#[test]
fn a_roster_out_of_order_is_refused_and_a_look_for_no_token_is_left_out() {
    let read = |text: &str| serde_json::from_str::<Roster>(text);
    for bad in [
        r#"{"places": [[1, [5]], [0, [6]]]}"#,
        r#"{"places": [[0, [5]], [0, [6]]]}"#,
        r#"{"places": [[0, [5, 6]]], "looks": [[6, {"label": "b"}], [5, {"label": "a"}]]}"#,
        r#"{"flying": [[2, []], [1, []]]}"#,
    ] {
        assert!(read(bad).is_err(), "{bad}");
    }
    // An earlier version could leave a look for a token named nowhere.
    let roster = read(
        r#"{"places": [[0, [5]], [1, []]],
            "looks": [[5, {"label": "kept"}], [9, {"label": "nobody's"}]],
            "flying": [[0, [[2, 7]]]]}"#,
    )
    .expect("it reads");
    assert_eq!(roster.tokens(PlaceId::new(0)), [TokenId::new(5)]);
    assert_eq!(
        roster.look(TokenId::new(5)).map(|l| &l.label[..]),
        Some("kept")
    );
    assert_eq!(roster.look(TokenId::new(9)), None);
    assert_eq!(roster.places().count(), 1, "the empty place is left out");
}

/// A log of `entries` mutations at positions 0, 1, 2 and so on, written.
fn log_value(capacity: Option<usize>, entries: u64) -> Value {
    let mut log = EventLog::new(capacity, None, Standing::default());
    for seq in 0..entries {
        log.push(
            Entry::Mutated {
                place: PlaceId::new(0),
                token: TokenId::new(seq),
            },
            seq,
            ms(seq),
        );
    }
    let mut text = Vec::new();
    log.write_json(&mut text).expect("it writes");
    serde_json::from_slice(&text).expect("JSON")
}

fn read_log(value: &Value) -> std::io::Result<EventLog> {
    EventLog::read_json(value.to_string().as_bytes())
}

#[test]
fn a_log_no_log_could_have_become_is_refused() {
    let good = log_value(Some(4), 6);
    let log = read_log(&good).expect("a ring reads");
    assert_eq!((log.first(), log.len()), (2, 4));

    let mut no_room = good.clone();
    no_room["capacity"] = json!(0);
    assert!(read_log(&no_room).is_err(), "a capacity of 0");

    let mut overfull = good.clone();
    overfull["capacity"] = json!(3);
    assert!(read_log(&overfull).is_err(), "four kept in a ring of three");

    let mut back_in_time = good.clone();
    back_in_time["entries"][2]["seq"] = json!(1);
    let error = read_log(&back_in_time).expect_err("down").to_string();
    assert!(error.contains("go down"), "{error}");
    // Two entries of one moment share a position.
    let mut together = good.clone();
    together["entries"][2]["seq"] = together["entries"][1]["seq"].clone();
    assert!(read_log(&together).is_ok());

    let mut endless = good.clone();
    endless["dropped"] = json!(u64::MAX - 2);
    assert!(read_log(&endless).is_err(), "indices with no room to grow");

    let mut unsorted = good;
    unsorted["base"] = json!({"counts": [[3, 1], [2, 1]]});
    assert!(read_log(&unsorted).is_err(), "a base out of order");
}

#[test]
fn a_log_read_by_any_serde_reader_counts_what_it_holds() {
    let mut log = EventLog::new(None, None, Standing::default());
    log.push_with(
        Entry::Added {
            place: PlaceId::new(0),
            token: TokenId::new(1),
        },
        0,
        Duration::ZERO,
        vec![(TokenId::new(1), Look::new(&"x".repeat(300), None, 200))],
    );
    let text = serde_json::to_string(&log).expect("it writes");
    let back: EventLog = serde_json::from_str(&text).expect("it reads");
    assert_eq!(back.bytes(), log.bytes());
    assert_eq!(back, log);
}

/// A record petritype-activity 0.3.0 wrote: a three-token queue run by
/// launches and steps, in a ring of eight bins, dated.
const RECORD_0_3_0: &str = r##"{"grid":{"columns":1,"rows":3,"cells":[{"column":0,"row":0,"node":{"place":0},"name":"Queue"},{"column":0,"row":2,"node":{"place":1},"name":"Done"},{"column":0,"row":1,"node":{"transition":0},"name":"Go"}]},"bin_width_ms":100,"start_unix_ms":1792499696789,"settings":{"pulse":{"jump":1.0,"clock":"wall","half_life_secs":0.3,"half_life_firings":3.0},"ember":{"jump":0.05,"clock":"wall","half_life_secs":20.0,"half_life_firings":50.0},"distribute_hit":0.5,"stale_half_life_secs":30.0,"stale_floor":0.25,"glow":"peak","count_curve":"halving","half_count":1.0,"count_cap":16.0,"flow_gain":0.25,"hue":"kind","decay_in_drawing":true},"bins":[{"delta":{"firings":1,"lifted":true,"places":{"0":{"pulse_hits":1,"ember_hits":1,"balance":-1,"count":2},"1":{"pulse_hits":1,"ember_hits":1,"balance":1,"count":1}},"transitions":{"0":{"fires":1,"distributes":1,"in_flight_ms":40.0}}}},{"delta":{"firings":1,"lifted":true,"places":{"0":{"pulse_hits":1,"ember_hits":1,"balance":-1,"count":1},"1":{"pulse_hits":1,"ember_hits":1,"balance":1,"count":2}},"transitions":{"0":{"fires":1}}},"log":[2,2]},{"delta":{"firings":1,"lifted":true,"places":{"0":{"pulse_hits":1,"ember_hits":1,"balance":-1},"1":{"pulse_hits":1,"ember_hits":1,"balance":1,"count":3}},"transitions":{"0":{"fires":1,"distributes":1,"in_flight_ms":40.0}}},"log":[3,3]},{"log":[5,5]},{"log":[5,5]},{"log":[5,5]}]}"##;

/// An event log petritype-activity 0.3.0 wrote of the same run, a ring of
/// four entries naming its tokens with their looks and colours.
const LOG_0_3_0: &str = r##"{"capacity":4,"start_unix_ms":1792499696789,"base":{"counts":[[0,2]],"in_flight":[0]},"tokens":{"places":[[0,[2]]],"looks":[[0,{"label":"n0","colour":"#c81e28"}],[2,{"label":"n2","colour":"#c81e28"}]],"flying":[[0,[[0,0]]]]},"dropped":1,"entries":[{"seq":1,"at_ms":7,"entry":{"event":"distributed","transition":0,"produced":[[1,3]]},"looks":[[3,{"label":"landed"}]]},{"seq":2,"at_ms":14,"entry":{"event":"fired","transition":0,"consumed":[[0,1]],"produced":[[1,4]]},"looks":[[4,{"label":"landed"}]]},{"seq":3,"at_ms":21,"entry":{"event":"launched","transition":0,"consumed":[[0,2]]}},{"seq":4,"at_ms":28,"entry":{"event":"distributed","transition":0,"produced":[[1,5]]},"looks":[[5,{"label":"landed"}]]}]}"##;

#[test]
fn a_record_and_a_log_written_by_0_3_0_read_back_and_write_the_same() {
    let record = Record::read_json(RECORD_0_3_0.as_bytes()).expect("the record reads");
    assert_eq!((record.len(), record.bin_width_ms()), (6, 100));
    assert_eq!(record.start_unix_ms, Some(1_792_499_696_789));
    let mut text = Vec::new();
    record.write_json(&mut text).expect("it writes");
    assert_eq!(String::from_utf8(text).expect("JSON is text"), RECORD_0_3_0);

    let log = EventLog::read_json(LOG_0_3_0.as_bytes()).expect("the log reads");
    assert_eq!((log.capacity(), log.len()), (Some(4), 4));
    assert!(log.dropped() > 0);
    assert_eq!(
        log.base_tokens()
            .look(TokenId::new(2))
            .and_then(|look| look.colour),
        Some(Rgb(200, 30, 40))
    );
    let mut text = Vec::new();
    log.write_json(&mut text).expect("it writes");
    assert_eq!(String::from_utf8(text).expect("JSON is text"), LOG_0_3_0);
}
