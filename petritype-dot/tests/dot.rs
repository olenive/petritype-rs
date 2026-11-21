//! Integration tests for `petritype-dot`: the text a net renders to.
//!
//! The renderer has no state and no opinions of its own (the DOT it produces
//! is a pure function of the read API and the [`Style`]), so the tests here
//! compare whole strings where they can and single attribute lines where a
//! whole graph would bury the point. The first test is a golden file written
//! out in full: everything else is a variation on it, and if the shape of a
//! line ever changes, that test says so first.

use std::error::Error;
use std::fmt;

use petritype::{
    DrainEdge, Event, Filter, MutateEdge, MutateWhereEdge, Net, NetBuilder, Param, PlaceRef,
    SnapshotEdge, SurveyEdge, TakeAllEdge, TakeEdge, TakeWhereEdge, TransitionId,
};
use petritype_dot::{Highlight, RankDir, Style, dot, dot_with};

/// A configuration token, read through a snapshot edge.
#[derive(Debug, Clone, PartialEq)]
struct Cfg(u32);

/// A running total, borrowed through a mutating edge.
#[derive(Debug, Clone, PartialEq)]
struct Tally(u32);

/// A parameter type, which a diagram names but never prints.
#[derive(Clone)]
struct ApiToken(String);

/// A body's refusal, so that a firing can fail on purpose.
#[derive(Debug)]
struct Refused;

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("refused")
    }
}

impl Error for Refused {}

/// `Inbox -> Double -> Middle -> Halve -> Out`, seeded with two tokens: the
/// smallest net with two transitions and something in it.
fn chain() -> (Net, TransitionId) {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let middle = b.place::<u32>("Middle");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [1, 2]);
    let double = b
        .transition("Double", (TakeEdge(inbox),), (middle,), |n: u32| (n * 2,))
        .id();
    b.transition("Halve", (TakeEdge(middle),), (out,), |n: u32| (n / 2,));
    (b.build().expect("the net builds"), double)
}

/// One transition reading four places four different ways and writing two,
/// which is every edge attribute this crate knows how to draw. The two
/// consuming places come back, so that a caller can put something in them.
fn every_edge_kind() -> (Net, PlaceRef<u32>, PlaceRef<u32>) {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let batch = b.place::<u32>("Batch");
    let config = b.place::<Cfg>("Config");
    let tally = b.place::<Tally>("Tally");
    let out = b.place::<u32>("Out");
    let spare = b.place::<u32>("Spare");
    b.seed(config, [Cfg(2)]);
    b.seed(tally, [Tally(0)]);
    b.transition(
        "Scale",
        (
            TakeEdge(inbox),
            TakeAllEdge(batch),
            SnapshotEdge(config),
            MutateEdge(tally),
        ),
        (out, spare),
        |n: u32, rest: Vec<u32>, cfg: Cfg, tally: &mut Tally| {
            tally.0 += 1;
            (n * cfg.0, rest)
        },
    );
    (b.build().expect("the net builds"), inbox, batch)
}

/// The lines of a rendered graph that mention one node id, so that a test can
/// look at one node without pinning the rest of the graph.
fn lines_for<'a>(text: &'a str, id: &str) -> Vec<&'a str> {
    text.lines()
        .filter(|line| line.trim_start().starts_with(id))
        .collect()
}

// ----- the shape of the text -----------------------------------------------

#[test]
fn a_small_net_renders_exactly_this() {
    let (net, _double) = chain();

    assert_eq!(
        dot(&net),
        concat!(
            "digraph \"petritype\" {\n",
            "  rankdir=TB;\n",
            "  node [fontname=\"Helvetica\"];\n",
            "  p0 [shape=oval, style=filled, fillcolor=deepskyblue, color=black, penwidth=1, label=\"Inbox\\n1\\n2\"];\n",
            "  p1 [shape=oval, style=filled, fillcolor=deepskyblue, color=black, penwidth=1, label=\"Middle\"];\n",
            "  p2 [shape=oval, style=filled, fillcolor=deepskyblue, color=black, penwidth=1, label=\"Out\"];\n",
            "  t0 [shape=box, style=filled, fillcolor=lightgreen, color=black, penwidth=1, label=\"Double\"];\n",
            "  t1 [shape=box, style=filled, fillcolor=lightgreen, color=black, penwidth=1, label=\"Halve\"];\n",
            "  p0 -> t0 [color=black, penwidth=1];\n",
            "  t0 -> p1 [color=black, penwidth=1];\n",
            "  p1 -> t1 [color=black, penwidth=1];\n",
            "  t1 -> p2 [color=black, penwidth=1];\n",
            "}\n",
        ),
    );
}

#[test]
fn a_snapshot_renders_the_same_text_as_its_net() {
    let (mut net, inbox, batch) = every_edge_kind();
    // Something in every place, and a firing behind us, so that the two
    // renders have a marking and a history to disagree about.
    net.extend(inbox, [7, 8]);
    net.extend(batch, [9]);
    net.step().expect("the transition fires");

    let snapshot = net.snapshot().expect("every place clones");
    assert_eq!(dot(&net), dot(&snapshot));

    // And with a style, including the highlight, which is not read off the
    // source at all.
    let style = Style {
        rankdir: RankDir::LR,
        max_tokens: 1,
        highlight: Some(Highlight {
            transition: net.transition_named("Scale").expect("the transition"),
            places: vec![inbox.id()],
            failed: false,
        }),
        ..Style::default()
    };
    assert_eq!(dot_with(&net, &style), dot_with(&snapshot, &style));
}

#[test]
fn tokens_can_be_hidden_or_cut_short() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [1, 2, 3, 4, 5]);
    b.transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,));
    let net = b.build().expect("the net builds");

    let bare = Style {
        show_tokens: false,
        ..Style::default()
    };
    assert!(lines_for(&dot_with(&net, &bare), "p0")[0].ends_with("label=\"Inbox\"];"));
    assert!(!dot_with(&net, &bare).contains("Inbox\\n1"));

    let cut = Style {
        max_tokens: 2,
        ..Style::default()
    };
    assert!(
        lines_for(&dot_with(&net, &cut), "p0")[0]
            .contains("label=\"Inbox\\n1\\n2\\n… and 3 more\""),
        "two tokens and a count of the rest: {}",
        lines_for(&dot_with(&net, &cut), "p0")[0],
    );

    // Everything fits: no tail line at all.
    let roomy = Style {
        max_tokens: 5,
        ..Style::default()
    };
    assert!(!dot_with(&net, &roomy).contains("more"));
}

#[test]
fn a_token_label_is_escaped_for_dot() {
    let mut b = NetBuilder::new();
    let notes = b.place::<String>("Notes");
    let out = b.place::<String>("Out");
    b.seed(notes, [String::from("say \"hi\"\\ok\nnext")]);
    b.transition("Copy", (TakeEdge(notes),), (out,), |s: String| (s,));
    let net = b.build().expect("the net builds");

    // `Debug` escapes the value once, and the renderer escapes that again, so
    // every backslash in the DOT source is one Graphviz will read back, and
    // the only bare `"` left are the pair delimiting the label.
    let line = lines_for(&dot(&net), "p0")[0].to_owned();
    assert_eq!(
        line,
        concat!(
            "  p0 [shape=oval, style=filled, fillcolor=deepskyblue, color=black, penwidth=1, ",
            r#"label="Notes\n\"say \\\"hi\\\"\\\\ok\\nnext\""];"#,
        ),
    );
}

// ----- what just happened --------------------------------------------------

#[test]
fn a_fired_event_outlines_its_transition_places_and_edges() {
    let (mut net, double) = chain();
    net.step().expect("Double fires");
    let fired = net.drain_events().next_back().expect("one Fired event");
    assert!(matches!(fired, Event::Fired { .. }));

    let style = Style {
        highlight: Highlight::from_event(&fired),
        ..Style::default()
    };
    let text = dot_with(&net, &style);

    assert_eq!(
        Highlight::from_event(&fired).expect("a firing"),
        Highlight {
            transition: double,
            places: vec![
                net.place_named("Inbox").expect("the place"),
                net.place_named("Middle").expect("the place"),
            ],
            failed: false,
        },
    );
    assert!(lines_for(&text, "t0")[0].contains("color=magenta, penwidth=3"));
    assert!(lines_for(&text, "p0")[0].contains("color=magenta, penwidth=2"));
    assert!(lines_for(&text, "p1")[0].contains("color=magenta, penwidth=2"));
    assert!(text.contains("  p0 -> t0 [color=magenta, penwidth=2];"));
    assert!(text.contains("  t0 -> p1 [color=magenta, penwidth=2];"));

    // The other half of the net is untouched.
    assert!(lines_for(&text, "t1")[0].contains("color=black, penwidth=1"));
    assert!(lines_for(&text, "p2")[0].contains("color=black, penwidth=1"));
    assert!(text.contains("  p1 -> t1 [color=black, penwidth=1];"));
}

#[test]
fn a_failed_event_outlines_in_red() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.seed(inbox, [1]);
    b.transition(
        "Refuse",
        (TakeEdge(inbox),),
        (out,),
        |_: u32| -> Result<(u32,), Refused> { Err(Refused) },
    );
    let mut net = b.build().expect("the net builds");

    net.step().expect_err("the body refuses");
    let failed = net.drain_events().next_back().expect("one Failed event");
    let highlight = Highlight::from_event(&failed).expect("a firing");
    assert!(highlight.failed);

    let text = dot_with(
        &net,
        &Style {
            highlight: Some(highlight),
            ..Style::default()
        },
    );
    assert!(lines_for(&text, "t0")[0].contains("color=red, penwidth=3"));
    assert!(lines_for(&text, "p0")[0].contains("color=red, penwidth=2"));
    assert!(text.contains("  p0 -> t0 [color=red, penwidth=2];"));
    // Nothing was produced, so the output edge and place stay at rest.
    assert!(text.contains("  t0 -> p1 [color=black, penwidth=1];"));
}

#[test]
fn an_in_flight_transition_is_gold_and_stays_gold_when_highlighted() {
    let (mut net, double) = chain();
    let firing = net.launch(double).expect("Double launches");
    let launched = net.drain_events().next_back().expect("one Launched event");

    let resting = dot(&net);
    assert!(
        lines_for(&resting, "t0")[0]
            .contains("style=filled, fillcolor=gold, color=darkorange, penwidth=2"),
        "the body is out: {}",
        lines_for(&resting, "t0")[0],
    );

    let lit = dot_with(
        &net,
        &Style {
            highlight: Highlight::from_event(&launched),
            ..Style::default()
        },
    );
    assert!(
        lines_for(&lit, "t0")[0].contains("fillcolor=gold, color=darkorange, penwidth=2"),
        "working now beats fired a moment ago",
    );
    // The place it took from is outlined all the same.
    assert!(lines_for(&lit, "p0")[0].contains("color=magenta, penwidth=2"));

    net.distribute(firing.run()).expect("the firing lands");
}

#[test]
fn a_disabled_transition_is_grey_and_dashed() {
    let (mut net, double) = chain();
    net.disable(double).expect("the transition is this net's");

    let line = lines_for(&dot(&net), "t0")[0].to_owned();
    assert_eq!(
        line,
        "  t0 [shape=box, style=\"filled,dashed\", fillcolor=lightgray, \
         color=black, penwidth=1, label=\"Double\"];",
    );
    // Its neighbour is unaffected.
    assert!(lines_for(&dot(&net), "t1")[0].contains("style=filled, fillcolor=lightgreen"));
}

// ----- edges, parameters, and the header -----------------------------------

#[test]
fn every_edge_kind_has_its_own_attributes() {
    let (net, _inbox, _batch) = every_edge_kind();
    let text = dot(&net);

    assert!(
        text.contains("  p0 -> t0 [color=black, penwidth=1];\n"),
        "TakeEdge"
    );
    assert!(
        text.contains("  p1 -> t0 [color=black, penwidth=1, label=\"*\"];\n"),
        "TakeAllEdge takes the lot, and says so",
    );
    assert!(
        text.contains("  p2 -> t0 [style=dashed, color=dimgray, arrowhead=normal, penwidth=1];\n"),
        "SnapshotEdge reads without consuming",
    );
    assert!(
        text.contains(
            "  p3 -> t0 [dir=both, arrowhead=dot, arrowtail=dot, color=dimgray, penwidth=2];\n"
        ),
        "MutateEdge reads and writes",
    );
    // Two outputs, so the slot each one is written from is worth spelling out.
    assert!(text.contains("  t0 -> p4 [color=black, penwidth=1, label=\"0\"];\n"));
    assert!(text.contains("  t0 -> p5 [color=black, penwidth=1, label=\"1\"];\n"));

    // One output needs no number: there is nothing to tell it apart from.
    let (single, _double) = chain();
    assert!(dot(&single).contains("  t0 -> p1 [color=black, penwidth=1];\n"));
}

/// The two edges an empty place satisfies are drawn as their all-at-once
/// counterparts, `TakeAllEdge` and `SnapshotEdge`, with a hollow arrowhead,
/// and both say `*`: every token, possibly none. The survey is the shop's,
/// the drain the traffic light's.
#[test]
fn drain_and_survey_edges_have_a_hollow_arrowhead() {
    let mut b = NetBuilder::new();
    let orders = b.place::<String>("Orders");
    let promotions = b.place::<String>("Promotions");
    let priced = b.place::<String>("Priced");
    b.transition(
        "Price Order",
        (TakeEdge(orders), SurveyEdge(promotions)),
        (priced,),
        |order: String, _: Vec<String>| (order,),
    );
    let shop = dot(&b.build().expect("the shop builds"));
    assert!(
        shop.contains(
            "  p1 -> t0 [style=dashed, color=dimgray, arrowhead=empty, penwidth=1, label=\"*\"];\n"
        ),
        "SurveyEdge reads every token, possibly none",
    );
    assert!(shop.contains("  p0 -> t0 [color=black, penwidth=1];\n"));

    // The light is one place, its token the colour: `Go` drains the cars
    // and reads the light, and `Change` takes it and writes the next colour
    // back. (The example's guard on `Go`, its controls and its style change
    // nothing in the drawing.)
    #[derive(Debug, Clone, Copy)]
    enum Colour {
        Red,
    }
    let mut b = NetBuilder::new();
    let colour = b.place::<Colour>("Light");
    let waiting = b.place::<String>("Waiting");
    let passed = b.place::<String>("Passed");
    b.seed(colour, [Colour::Red]);
    b.transition(
        "Go",
        (DrainEdge(waiting), SnapshotEdge(colour)),
        (passed,),
        |cars: Vec<String>, _: Colour| (cars,),
    );
    b.transition("Change", (TakeEdge(colour),), (colour,), |light: Colour| {
        (light,)
    });
    let light = dot(&b.build().expect("the light builds"));
    assert!(
        light.contains("  p1 -> t0 [color=black, arrowhead=empty, penwidth=1, label=\"*\"];\n"),
        "DrainEdge takes every token, possibly none",
    );
    assert!(
        light.contains("  p0 -> t0 [style=dashed, color=dimgray, arrowhead=normal, penwidth=1];\n"),
        "a SnapshotEdge keeps its plain arrowhead: it needs a token",
    );
    assert!(light.contains("  t0 -> p2 [color=black, penwidth=1];\n"));
    assert!(light.contains("  p0 -> t1 [color=black, penwidth=1];\n"));
    assert!(
        light.contains("  t1 -> p0 [color=black, penwidth=1];\n"),
        "taken from `Light` and written back: two edges, one each way",
    );
}

/// A keyed edge is drawn as its plain counterpart, labelled with its filter:
/// clauses joined by `and`, a parameter clause as `$name`, and `any` for no
/// clauses at all.
#[test]
fn a_keyed_edge_is_labelled_with_its_filter() {
    let mut b = NetBuilder::new();
    let birds = b.place::<(String, i64)>("Birds");
    let nests = b.place::<(String, i64)>("Nests");
    let out = b.place::<(String, i64)>("Out");
    for place in [birds, nests] {
        b.keys(place, ["class", "legs"], |(class, legs): &(String, i64)| {
            [class.as_str().into(), (*legs).into()]
        });
    }
    b.transition(
        "Fly",
        (
            TakeWhereEdge(
                birds,
                Filter::new()
                    .key_in("class", ["Bird", "Bat"])
                    .key_is_param("legs", "legs"),
            ),
            MutateWhereEdge(nests, Filter::new()),
        ),
        (out,),
        |bird: (String, i64), _: &mut (String, i64)| (bird,),
    );
    let text = dot(&b.build().unwrap());

    assert!(
        text.contains(
            "  p0 -> t0 [color=black, penwidth=1, label=\"class in {Bird, Bat} and legs = $legs\"];\n"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "  p1 -> t0 [dir=both, arrowhead=dot, arrowtail=dot, color=dimgray, penwidth=2, \
             label=\"any\"];\n"
        ),
        "{text}"
    );
}

#[test]
fn parameters_are_a_second_label_line_of_short_type_names() {
    let mut b = NetBuilder::new();
    let urls = b.place::<String>("Urls");
    let pages = b.place::<String>("Pages");
    b.param(ApiToken(String::from("secret")));
    b.param(vec![Cfg(1)]);
    b.transition(
        "Fetch",
        (
            TakeEdge(urls),
            Param::<ApiToken>::new(),
            Param::<Vec<Cfg>>::new(),
        ),
        (pages,),
        |url: String, token: ApiToken, cfgs: Vec<Cfg>| {
            (format!("{url}/{}/{}", token.0, cfgs.len()),)
        },
    );
    let net = b.build().expect("the net builds");

    let line = lines_for(&dot(&net), "t0")[0].to_owned();
    assert!(
        line.ends_with("label=\"Fetch\\n[ApiToken, Vec<Cfg>]\"];"),
        "paths dropped, generics kept: {line}",
    );
    // A parameter is not an edge: the transition still has exactly two.
    assert_eq!(dot(&net).matches(" -> ").count(), 2);
    // And the value is never printed, secret or not.
    assert!(!dot(&net).contains("secret"));
}

#[test]
fn the_header_carries_the_layout_and_the_graph_name() {
    let (net, _double) = chain();
    let style = Style {
        rankdir: RankDir::LR,
        graph_name: String::from("a \"named\" net"),
        ..Style::default()
    };
    let text = dot_with(&net, &style);

    assert!(text.starts_with(concat!(
        "digraph \"a \\\"named\\\" net\" {\n",
        "  rankdir=LR;\n",
        "  node [fontname=\"Helvetica\"];\n",
    )));
    assert_eq!(dot(&net).lines().next(), Some("digraph \"petritype\" {"));
}

#[test]
fn a_net_with_no_transitions_still_renders() {
    let mut b = NetBuilder::new();
    let _empty: PlaceRef<u32> = b.place::<u32>("Empty");
    let net = b.build().expect("a net of one place is a net");

    assert_eq!(
        dot(&net),
        concat!(
            "digraph \"petritype\" {\n",
            "  rankdir=TB;\n",
            "  node [fontname=\"Helvetica\"];\n",
            "  p0 [shape=oval, style=filled, fillcolor=deepskyblue, color=black, penwidth=1, label=\"Empty\"];\n",
            "}\n",
        ),
    );
}

#[test]
fn a_net_with_no_subnet_places_renders_as_it_did_before_them() {
    // Every kind of edge, a firing outlined, left to right: the text the
    // commit before subnet places came to this crate wrote, word for word.
    let (mut net, inbox, batch) = every_edge_kind();
    net.put(inbox, 3);
    net.put(batch, 4);
    net.put(batch, 5);
    net.step().expect("Scale fires");
    let last = net.drain_events().next_back().expect("one firing");
    net.put(inbox, 6);
    let style = Style {
        rankdir: RankDir::LR,
        highlight: Highlight::from_event(&last),
        ..Style::default()
    };
    assert_eq!(
        dot_with(&net, &style),
        concat!(
            "digraph \"petritype\" {\n",
            "  rankdir=LR;\n",
            "  node [fontname=\"Helvetica\"];\n",
            "  p0 [shape=oval, style=filled, fillcolor=deepskyblue, color=magenta, penwidth=2, label=\"Inbox\\n6\"];\n",
            "  p1 [shape=oval, style=filled, fillcolor=deepskyblue, color=magenta, penwidth=2, label=\"Batch\"];\n",
            "  p2 [shape=oval, style=filled, fillcolor=deepskyblue, color=black, penwidth=1, label=\"Config\\nCfg(2)\"];\n",
            "  p3 [shape=oval, style=filled, fillcolor=deepskyblue, color=black, penwidth=1, label=\"Tally\\nTally(1)\"];\n",
            "  p4 [shape=oval, style=filled, fillcolor=deepskyblue, color=magenta, penwidth=2, label=\"Out\\n6\"];\n",
            "  p5 [shape=oval, style=filled, fillcolor=deepskyblue, color=magenta, penwidth=2, label=\"Spare\\n4\\n5\"];\n",
            "  t0 [shape=box, style=filled, fillcolor=lightgreen, color=magenta, penwidth=3, label=\"Scale\"];\n",
            "  p0 -> t0 [color=magenta, penwidth=2];\n",
            "  p1 -> t0 [color=magenta, penwidth=2, label=\"*\"];\n",
            "  p2 -> t0 [style=dashed, color=dimgray, arrowhead=normal, penwidth=1];\n",
            "  p3 -> t0 [dir=both, arrowhead=dot, arrowtail=dot, color=dimgray, penwidth=2];\n",
            "  t0 -> p4 [color=magenta, penwidth=2, label=\"0\"];\n",
            "  t0 -> p5 [color=magenta, penwidth=2, label=\"1\"];\n",
            "}\n",
        ),
    );
}

// ----- subnet places -------------------------------------------------------

/// A level: `Lobby -> Fight -> Arena -> Rest -> Exit Hall`, whose lobby and
/// exit hall are its ports.
fn level() -> Net {
    let mut b = NetBuilder::new();
    let lobby = b.place::<u32>("Lobby");
    let arena = b.place::<u32>("Arena");
    let exit = b.place::<u32>("Exit Hall");
    b.port(lobby);
    b.port(exit);
    b.transition("Fight", (TakeEdge(lobby),), (arena,), |n: u32| (n,));
    b.transition("Rest", (TakeEdge(arena),), (exit,), |n: u32| (n,));
    b.build().expect("the level builds")
}

/// `Door -> Enter -> Level 3 [Lobby | Exit Hall] -> Leave -> Out`, with one
/// hero at the door and two in the lobby. Returns the subnet place too.
fn world() -> (Net, petritype::PlaceId) {
    let mut b = NetBuilder::new();
    let door = b.place::<u32>("Door");
    let level_3 = b.subnet_place("Level 3", level());
    let lobby = b.port_of::<u32>(level_3, "Lobby");
    let exit = b.port_of::<u32>(level_3, "Exit Hall");
    let out = b.place::<u32>("Out");
    b.seed(door, [1]);
    b.transition("Enter", (TakeEdge(door),), (lobby,), |n: u32| (n,));
    b.transition("Leave", (TakeEdge(exit),), (out,), |n: u32| (n,));
    let mut net = b.build().expect("the world builds");
    net.put(lobby, 2);
    net.put(lobby, 3);
    (net, level_3.id())
}

#[test]
fn a_subnet_place_is_one_box_with_a_cell_per_port() {
    let (net, _level) = world();

    // The ports, places 2 and 3 of the world, are cells of p1, not nodes.
    assert_eq!(
        dot(&net),
        concat!(
            "digraph \"petritype\" {\n",
            "  rankdir=TB;\n",
            "  node [fontname=\"Helvetica\"];\n",
            "  p0 [shape=oval, style=filled, fillcolor=deepskyblue, color=black, penwidth=1, label=\"Door\\n1\"];\n",
            "  p1 [shape=Mrecord, style=filled, fillcolor=lightskyblue, color=black, penwidth=1, label=\"{Level 3|{<port0> Lobby\\n2|<port1> Exit Hall\\n0}}\"];\n",
            "  p4 [shape=oval, style=filled, fillcolor=deepskyblue, color=black, penwidth=1, label=\"Out\"];\n",
            "  t0 [shape=box, style=filled, fillcolor=lightgreen, color=black, penwidth=1, label=\"Enter\"];\n",
            "  t1 [shape=box, style=filled, fillcolor=lightgreen, color=black, penwidth=1, label=\"Leave\"];\n",
            "  p0 -> t0 [color=black, penwidth=1];\n",
            "  t0 -> p1 [color=black, penwidth=1, headport=port0];\n",
            "  p1 -> t1 [color=black, penwidth=1, tailport=port1];\n",
            "  t1 -> p4 [color=black, penwidth=1];\n",
            "}\n",
        ),
    );
}

#[test]
fn a_snapshot_draws_a_subnet_place_as_its_net_does() {
    let (net, _level) = world();
    let snapshot = net.snapshot().expect("the world snapshots");
    assert_eq!(dot(&net), dot(&snapshot));
}

#[test]
fn with_tokens_hidden_a_cell_shows_its_port_s_name_alone() {
    let (net, _level) = world();
    let style = Style {
        show_tokens: false,
        ..Style::default()
    };
    let text = dot_with(&net, &style);
    assert!(
        lines_for(&text, "p1 [")[0]
            .ends_with("label=\"{Level 3|{<port0> Lobby|<port1> Exit Hall}}\"];")
    );
}

#[test]
fn a_firing_into_a_port_outlines_the_box_and_the_edge_to_its_cell() {
    let (mut net, _level) = world();
    net.drain_events().for_each(drop);
    net.step().expect("Enter fires");
    let fired = net.drain_events().next_back().expect("one Fired event");
    let style = Style {
        highlight: Highlight::from_event(&fired),
        ..Style::default()
    };
    let text = dot_with(&net, &style);

    assert!(lines_for(&text, "p1 [")[0].contains("color=magenta, penwidth=2"));
    assert!(lines_for(&text, "p1 [")[0].contains("<port0> Lobby\\n3|<port1> Exit Hall\\n0"));
    assert!(text.contains("  t0 -> p1 [color=magenta, penwidth=2, headport=port0];"));
    assert!(text.contains("  p1 -> t1 [color=black, penwidth=1, tailport=port1];"));
}

#[test]
fn an_edge_of_any_kind_on_a_port_keeps_its_attributes_and_ends_at_the_cell() {
    let mut b = NetBuilder::new();
    let level_3 = b.subnet_place("Level 3", level());
    let lobby = b.port_of::<u32>(level_3, "Lobby");
    let exit = b.port_of::<u32>(level_3, "Exit Hall");
    let seen = b.place::<u32>("Seen");
    let all = b.place::<u32>("All");
    b.transition("Peek", (SnapshotEdge(lobby),), (seen,), |n: u32| (n,));
    b.transition("Sweep", (DrainEdge(exit),), (all,), |n: Vec<u32>| {
        (n.len() as u32,)
    });
    let net = b.build().expect("the net builds");
    let text = dot(&net);

    assert!(text.contains(
        "  p0 -> t0 [style=dashed, color=dimgray, arrowhead=normal, penwidth=1, tailport=port0];"
    ));
    assert!(text.contains(
        "  p0 -> t1 [color=black, arrowhead=empty, penwidth=1, label=\"*\", tailport=port1];"
    ));
    assert!(text.contains("  t0 -> p3 [color=black, penwidth=1];"));
}

#[test]
fn a_subnet_place_s_names_are_escaped_for_a_record() {
    let mut b = NetBuilder::new();
    let gate = b.place::<u32>("\"Gate\" <1>");
    let drive = b.place::<u32>(r"C:\in");
    b.port(gate);
    b.port(drive);
    let wing = b.build().expect("the wing builds");
    let mut b = NetBuilder::new();
    let _wing = b.subnet_place("Wing {A|B}", wing);
    let net = b.build().expect("the net builds");

    assert_eq!(
        lines_for(&dot(&net), "p0 [")[0].trim_start(),
        concat!(
            "p0 [shape=Mrecord, style=filled, fillcolor=lightskyblue, color=black, penwidth=1, ",
            r#"label="{Wing \{A\|B\}|{<port0> \"Gate\" \<1\>\n0|<port1> C:\\in\n0}}"];"#,
        ),
    );
}

#[test]
fn a_subnet_place_with_no_ports_is_a_box_with_its_title() {
    let mut b = NetBuilder::new();
    let _sealed = b.subnet_place("Sealed", level_without_ports());
    let net = b.build().expect("the net builds");
    assert!(lines_for(&dot(&net), "p0 [")[0].ends_with("label=\"{Sealed}\"];"));
}

/// A net with one place and no ports.
fn level_without_ports() -> Net {
    let mut b = NetBuilder::new();
    let _inside: PlaceRef<u32> = b.place::<u32>("Inside");
    b.build().expect("the level builds")
}

#[test]
fn in_a_subnet_s_own_drawing_a_port_has_a_second_ring() {
    let (net, level_3) = world();
    let inside = net.subnet(level_3).expect("a subnet place");
    let text = dot(inside);

    assert_eq!(
        lines_for(&text, "p0 [")[0].trim_start(),
        "p0 [shape=oval, style=filled, fillcolor=deepskyblue, color=black, penwidth=1, \
         peripheries=2, label=\"Lobby\\n2\\n3\"];"
    );
    assert!(lines_for(&text, "p1 [")[0].contains("penwidth=1, label=\"Arena\""));
    assert!(lines_for(&text, "p2 [")[0].contains("penwidth=1, peripheries=2, label=\"Exit Hall\""));
    // The subnet's own edges are plain: there are no cells inside it.
    assert!(text.contains("  p0 -> t0 [color=black, penwidth=1];"));
}
