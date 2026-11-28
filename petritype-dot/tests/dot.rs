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
    ReadAllEdge, ReadEdge, ReadWhereEdge, SnapshotEdge, SurveyEdge, TakeAllEdge, TakeEdge,
    TakeWhereEdge, TransitionId,
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
/// consuming places start with `in_inbox` and `in_batch`, and `Inbox` comes
/// back. The net marks no port, so it takes nothing from outside: what a
/// test wants in it is in it from the start.
fn every_edge_kind(in_inbox: &[u32], in_batch: &[u32]) -> (Net, PlaceRef<u32>) {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let batch = b.place::<u32>("Batch");
    let config = b.place::<Cfg>("Config");
    let tally = b.place::<Tally>("Tally");
    let out = b.place::<u32>("Out");
    let spare = b.place::<u32>("Spare");
    b.seed(inbox, in_inbox.to_vec());
    b.seed(batch, in_batch.to_vec());
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
    (b.build().expect("the net builds"), inbox)
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
    // Something in every place, and a firing behind us, so that the two
    // renders have a marking and a history to disagree about.
    let (mut net, inbox) = every_edge_kind(&[7, 8], &[9]);
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
    let (net, _inbox) = every_edge_kind(&[], &[]);
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

/// Two take edges of one transition on one place (since 0.4.0) are two
/// arrows, each drawn as itself: a take-where with its own filter, and both
/// outlined by a firing that took a token through each.
#[test]
fn two_edges_on_one_place_are_two_arrows() {
    let mut b = NetBuilder::new();
    let hall = b.place::<(String, String)>("Ballroom");
    let floor = b.place::<Vec<(String, String)>>("Floor");
    b.keys(hall, ["role"], |(_, role): &(String, String)| {
        [role.as_str().into()]
    });
    let dancer = |name: &str, role: &str| (name.to_owned(), role.to_owned());
    b.seed(hall, [dancer("Ada", "lady"), dancer("Ben", "gentleman")]);
    b.transition(
        "Dance",
        (
            TakeWhereEdge(hall, Filter::new().key_in("role", ["lady"])),
            TakeWhereEdge(hall, Filter::new().key_in("role", ["gentleman"])),
        ),
        (floor,),
        |a: (String, String), c: (String, String)| (vec![a, c],),
    );
    let mut net = b.build().unwrap();
    let arrows = |text: &str| -> Vec<String> {
        lines_for(text, "p0 -> t0")
            .into_iter()
            .map(str::to_owned)
            .collect()
    };
    assert_eq!(
        arrows(&dot(&net)),
        [
            "  p0 -> t0 [color=black, penwidth=1, label=\"role in {lady}\"];",
            "  p0 -> t0 [color=black, penwidth=1, label=\"role in {gentleman}\"];",
        ]
    );

    net.step().expect("Dance fires");
    let fired = net.drain_events().next_back().expect("one Fired event");
    let style = Style {
        highlight: Highlight::from_event(&fired),
        ..Style::default()
    };
    let lit = arrows(&dot_with(&net, &style));
    assert_eq!(lit.len(), 2, "{lit:?}");
    assert!(
        lit.iter().all(|line| line.contains("color=magenta")),
        "{lit:?}"
    );
}

/// A keyed edge that chooses by another input names that input's place and
/// key, `name = Orders.courier`, and a port as `Subnet/Port`; a snapshot
/// and a live net draw it alike.
#[test]
fn a_clause_that_chooses_by_another_input_names_its_place_and_key() {
    let mut yard = NetBuilder::new();
    let bay = yard.place::<(String, i64)>("Bay");
    yard.keys(bay, ["name"], |(name, _): &(String, i64)| {
        [name.as_str().into()]
    });
    yard.port(bay);
    let yard = yard.build().unwrap();

    let mut b = NetBuilder::new();
    let orders = b.place::<(String, i64)>("Orders");
    let couriers = b.place::<(String, i64)>("Couriers");
    b.keys(orders, ["courier"], |(courier, _): &(String, i64)| {
        [courier.as_str().into()]
    });
    b.keys(couriers, ["name"], |(name, _): &(String, i64)| {
        [name.as_str().into()]
    });
    let yard = b.subnet_place("Yard", yard);
    let bay = b.port_of::<(String, i64)>(yard, "Bay");
    b.transition(
        "Load",
        (
            TakeEdge(orders),
            MutateWhereEdge(
                couriers,
                Filter::new().key_is_input("name", orders, "courier"),
            ),
        ),
        (),
        |_: (String, i64), courier: &mut (String, i64)| courier.1 += 1,
    );
    b.transition(
        "Call",
        (
            SnapshotEdge(bay),
            TakeWhereEdge(orders, Filter::new().key_is_input("courier", bay, "name")),
        ),
        (),
        |_: (String, i64), _: (String, i64)| {},
    );
    let net = b.build().unwrap();
    let text = dot(&net);

    assert!(
        text.contains(
            "  p1 -> t0 [dir=both, arrowhead=dot, arrowtail=dot, color=dimgray, penwidth=2, \
             label=\"name = Orders.courier\"];\n"
        ),
        "{text}"
    );
    assert!(
        text.contains("label=\"courier = Yard/Bay.name\"]"),
        "{text}"
    );
    assert_eq!(text, dot(&net.snapshot().unwrap()));
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
    // The firing takes 3, the front of `Inbox`, and leaves 6.
    let (mut net, _inbox) = every_edge_kind(&[3, 6], &[4, 5]);
    net.step().expect("Scale fires");
    let last = net.drain_events().next_back().expect("one firing");
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
/// exit hall are its ports, the lobby holding `heroes`.
fn level(heroes: &[u32]) -> Net {
    let mut b = NetBuilder::new();
    let lobby = b.place::<u32>("Lobby");
    let arena = b.place::<u32>("Arena");
    let exit = b.place::<u32>("Exit Hall");
    b.port(lobby);
    b.port(exit);
    b.seed(lobby, heroes.to_vec());
    b.transition("Fight", (TakeEdge(lobby),), (arena,), |n: u32| (n,));
    b.transition("Rest", (TakeEdge(arena),), (exit,), |n: u32| (n,));
    b.build().expect("the level builds")
}

/// `Door -> Enter -> Level 3 [Lobby | Exit Hall] -> Leave -> Out`, with one
/// hero at the door and two in the lobby, each there from the start: the
/// world is closed, as every net not declared open is, so from outside it
/// takes nothing at the level's ports. Returns the subnet place too.
fn world() -> (Net, petritype::PlaceId) {
    let mut b = NetBuilder::new();
    let door = b.place::<u32>("Door");
    let level_3 = b.subnet_place("Level 3", level(&[2, 3]));
    let lobby = b.port_of::<u32>(level_3, "Lobby");
    let exit = b.port_of::<u32>(level_3, "Exit Hall");
    let out = b.place::<u32>("Out");
    b.seed(door, [1]);
    b.transition("Enter", (TakeEdge(door),), (lobby,), |n: u32| (n,));
    b.transition("Leave", (TakeEdge(exit),), (out,), |n: u32| (n,));
    let net = b.build().expect("the world builds");
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
    let level_3 = b.subnet_place("Level 3", level(&[]));
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

/// In the drawing of the closed net that marks it, a port is one dotted
/// ring, and its other places are plain.
#[test]
fn in_a_subnet_s_own_drawing_a_port_is_one_dotted_ring() {
    let (net, level_3) = world();
    let inside = net.subnet(level_3).expect("a subnet place");
    assert!(inside.is_closed());
    let text = dot(inside);

    assert_eq!(
        lines_for(&text, "p0 [")[0].trim_start(),
        "p0 [shape=oval, style=\"filled,dotted\", fillcolor=deepskyblue, color=black, \
         penwidth=1, label=\"Lobby\\n2\\n3\"];"
    );
    assert_eq!(
        lines_for(&text, "p1 [")[0].trim_start(),
        "p1 [shape=oval, style=filled, fillcolor=deepskyblue, color=black, penwidth=1, \
         label=\"Arena\"];"
    );
    assert_eq!(
        lines_for(&text, "p2 [")[0].trim_start(),
        "p2 [shape=oval, style=\"filled,dotted\", fillcolor=deepskyblue, color=black, \
         penwidth=1, label=\"Exit Hall\"];"
    );
    assert!(!text.contains("peripheries"), "{text}");
    // The subnet's own edges are plain: there are no cells inside it.
    assert!(text.contains("  p0 -> t0 [color=black, penwidth=1];"));
}

/// A highlight recolours a port and keeps its dots: it changes only `color`
/// and `penwidth`.
#[test]
fn a_highlighted_port_keeps_its_dots() {
    let mut net = level(&[2]);
    net.drain_events().for_each(drop);
    net.step().expect("Fight fires");
    let fired = net.drain_events().next_back().expect("one Fired event");
    let style = Style {
        highlight: Highlight::from_event(&fired),
        ..Style::default()
    };
    let text = dot_with(&net, &style);
    assert_eq!(
        lines_for(&text, "p0 [")[0].trim_start(),
        "p0 [shape=oval, style=\"filled,dotted\", fillcolor=deepskyblue, color=magenta, \
         penwidth=2, label=\"Lobby\"];"
    );
    assert!(
        lines_for(&text, "p1 [")[0]
            .contains("style=filled, fillcolor=deepskyblue, color=magenta, penwidth=2")
    );
    assert!(
        lines_for(&text, "p2 [")[0]
            .contains("style=\"filled,dotted\", fillcolor=deepskyblue, color=black, penwidth=1")
    );
}

// ----- groups ---------------------------------------------------------------------

/// A hike: `Energy`, in no group, which `Fund Walk` (in `Walk`) and `Eat`
/// (in `Eat`) both change; `Trail` and `Walked` in `Walk`, `Pack` in `Eat`,
/// `Rest` in no group, and an empty group, `Later`, declared between the
/// two others.
fn hike() -> Net {
    let mut b = NetBuilder::new();
    let walk = b.group("Walk");
    let _later = b.group("Later");
    let eat = b.group("Eat");
    let energy = b.place::<u32>("Energy");
    let trail = b.place::<u32>("Trail");
    let pack = b.place::<u32>("Pack");
    let walked = b.place::<u32>("Walked");
    b.in_group(walk, trail);
    b.in_group(eat, pack);
    b.in_group(walk, walked);
    b.seed(energy, [3]);
    b.seed(trail, [1]);
    b.seed(pack, [2]);
    b.transition(
        "Fund Walk",
        (TakeEdge(trail), MutateEdge(energy)),
        (walked,),
        |leg: u32, energy: &mut u32| {
            *energy -= 1;
            (leg,)
        },
    )
    .group(walk);
    b.transition(
        "Eat",
        (TakeEdge(pack), MutateEdge(energy)),
        (),
        |meal: u32, energy: &mut u32| *energy += meal,
    )
    .group(eat);
    b.transition("Rest", (TakeEdge(walked),), (), |_: u32| {});
    b.build().expect("the hike builds")
}

#[test]
fn each_group_is_a_dotted_cluster_in_declared_order() {
    // The nodes in no group first, at the top level, then each group's
    // places and transitions in its cluster; the empty group's cluster
    // holds nothing, which Graphviz does not draw. Every node keeps its id.
    assert_eq!(
        dot(&hike()),
        concat!(
            "digraph \"petritype\" {\n",
            "  rankdir=TB;\n",
            "  node [fontname=\"Helvetica\"];\n",
            "  p0 [shape=oval, style=filled, fillcolor=deepskyblue, color=black, penwidth=1, label=\"Energy\\n3\"];\n",
            "  t2 [shape=box, style=filled, fillcolor=lightgreen, color=black, penwidth=1, label=\"Rest\"];\n",
            "  subgraph cluster_0 {\n",
            "    label=\"Walk\";\n",
            "    style=dotted;\n",
            "    p1 [shape=oval, style=filled, fillcolor=deepskyblue, color=black, penwidth=1, label=\"Trail\\n1\"];\n",
            "    p3 [shape=oval, style=filled, fillcolor=deepskyblue, color=black, penwidth=1, label=\"Walked\"];\n",
            "    t0 [shape=box, style=filled, fillcolor=lightgreen, color=black, penwidth=1, label=\"Fund Walk\"];\n",
            "  }\n",
            "  subgraph cluster_1 {\n",
            "    label=\"Later\";\n",
            "    style=dotted;\n",
            "  }\n",
            "  subgraph cluster_2 {\n",
            "    label=\"Eat\";\n",
            "    style=dotted;\n",
            "    p2 [shape=oval, style=filled, fillcolor=deepskyblue, color=black, penwidth=1, label=\"Pack\\n2\"];\n",
            "    t1 [shape=box, style=filled, fillcolor=lightgreen, color=black, penwidth=1, label=\"Eat\"];\n",
            "  }\n",
            "  p1 -> t0 [color=black, penwidth=1];\n",
            "  p0 -> t0 [dir=both, arrowhead=dot, arrowtail=dot, color=dimgray, penwidth=2];\n",
            "  t0 -> p3 [color=black, penwidth=1];\n",
            "  p2 -> t1 [color=black, penwidth=1];\n",
            "  p0 -> t1 [dir=both, arrowhead=dot, arrowtail=dot, color=dimgray, penwidth=2];\n",
            "  p3 -> t2 [color=black, penwidth=1];\n",
            "}\n",
        ),
    );
}

#[test]
fn a_snapshot_draws_the_groups_as_its_net_does() {
    let net = hike();
    assert_eq!(dot(&net), dot(&net.snapshot().expect("the hike snapshots")));
}

#[test]
fn a_node_in_a_frame_is_drawn_as_it_is_outside_one() {
    // A firing outlined, a switch off, tokens hidden: each node line inside
    // a cluster is the line the same node has in the same net with no
    // groups, indented once more.
    let mut grouped = hike();
    grouped.step().expect("Fund Walk fires");
    let last = grouped.drain_events().next_back().expect("one firing");
    let eat = grouped.transition_named("Eat").unwrap();
    grouped.disable(eat).unwrap();
    let style = Style {
        show_tokens: false,
        highlight: Highlight::from_event(&last),
        ..Style::default()
    };
    let text = dot_with(&grouped, &style);
    assert!(text.contains(
        "    p1 [shape=oval, style=filled, fillcolor=deepskyblue, color=magenta, penwidth=2, label=\"Trail\"];\n"
    ));
    assert!(text.contains(
        "    t0 [shape=box, style=filled, fillcolor=lightgreen, color=magenta, penwidth=3, label=\"Fund Walk\"];\n"
    ));
    assert!(text.contains(
        "    t1 [shape=box, style=\"filled,dashed\", fillcolor=lightgray, color=black, penwidth=1, label=\"Eat\"];\n"
    ));
    assert!(text.contains("  p1 -> t0 [color=magenta, penwidth=2];\n"));
}

#[test]
fn a_group_s_name_is_escaped_for_dot() {
    let mut b = NetBuilder::new();
    let odd = b.group("Walk \"fast\"\\slow\nnow");
    let trail = b.place::<u32>("Trail");
    b.in_group(odd, trail);
    let net = b.build().expect("no character means anything");
    assert!(
        dot(&net).contains("    label=\"Walk \\\"fast\\\"\\\\slow\\nnow\";\n"),
        "{}",
        dot(&net)
    );
}

#[test]
fn a_subnet_place_in_a_group_is_drawn_inside_its_frame() {
    // The world of the subnet tests with its door, its level and its
    // `Enter` in `Walk`: the box is drawn in the frame, its ports are
    // still cells of the box, and an edge on a port still ends at its cell.
    let mut b = NetBuilder::new();
    let walk = b.group("Walk");
    let door = b.place::<u32>("Door");
    let level_3 = b.subnet_place("Level 3", level(&[]));
    let lobby = b.port_of::<u32>(level_3, "Lobby");
    let exit = b.port_of::<u32>(level_3, "Exit Hall");
    let out = b.place::<u32>("Out");
    b.in_group(walk, door);
    b.in_group(walk, level_3);
    b.seed(door, [1]);
    b.transition("Enter", (TakeEdge(door),), (lobby,), |n: u32| (n,))
        .group(walk);
    b.transition("Leave", (TakeEdge(exit),), (out,), |n: u32| (n,));
    let net = b.build().expect("the world builds");

    assert_eq!(
        dot(&net),
        concat!(
            "digraph \"petritype\" {\n",
            "  rankdir=TB;\n",
            "  node [fontname=\"Helvetica\"];\n",
            "  p4 [shape=oval, style=filled, fillcolor=deepskyblue, color=black, penwidth=1, label=\"Out\"];\n",
            "  t1 [shape=box, style=filled, fillcolor=lightgreen, color=black, penwidth=1, label=\"Leave\"];\n",
            "  subgraph cluster_0 {\n",
            "    label=\"Walk\";\n",
            "    style=dotted;\n",
            "    p0 [shape=oval, style=filled, fillcolor=deepskyblue, color=black, penwidth=1, label=\"Door\\n1\"];\n",
            "    p1 [shape=Mrecord, style=filled, fillcolor=lightskyblue, color=black, penwidth=1, label=\"{Level 3|{<port0> Lobby\\n0|<port1> Exit Hall\\n0}}\"];\n",
            "    t0 [shape=box, style=filled, fillcolor=lightgreen, color=black, penwidth=1, label=\"Enter\"];\n",
            "  }\n",
            "  p0 -> t0 [color=black, penwidth=1];\n",
            "  t0 -> p1 [color=black, penwidth=1, headport=port0];\n",
            "  p1 -> t1 [color=black, penwidth=1, tailport=port1];\n",
            "  t1 -> p4 [color=black, penwidth=1];\n",
            "}\n",
        ),
    );
}

#[test]
fn a_subnet_s_own_drawing_has_its_own_groups() {
    let mut b = NetBuilder::new();
    let inside = b.group("Inside");
    let lobby = b.place::<u32>("Lobby");
    let arena = b.place::<u32>("Arena");
    b.port(lobby);
    b.in_group(inside, lobby);
    b.in_group(inside, arena);
    let level = b.build().expect("the level builds");
    let mut b = NetBuilder::new();
    let level_3 = b.subnet_place("Level 3", level);
    let net = b.build().expect("the world builds");

    let outside = dot(&net);
    assert!(!outside.contains("subgraph"), "{outside}");
    let text = dot(net.subnet(level_3.id()).expect("a subnet place"));
    assert!(text.contains(concat!(
        "  subgraph cluster_0 {\n",
        "    label=\"Inside\";\n",
        "    style=dotted;\n",
        "    p0 [shape=oval, style=\"filled,dotted\", fillcolor=deepskyblue, color=black, penwidth=1, label=\"Lobby\"];\n",
        "    p1 [shape=oval",
    )));
}

/// The read edges are dotted, where a snapshot is dashed: a read-where
/// carries its filter and a read-all the hollow head and `*` of an edge that
/// takes every token, zero or more.
#[test]
fn read_edges_are_dotted() {
    let mut b = NetBuilder::new();
    let limits = b.place::<(String, i64)>("Limits");
    let gauges = b.place::<(String, i64)>("Gauges");
    let notes = b.place::<(String, i64)>("Notes");
    b.keys(gauges, ["name"], |(name, _): &(String, i64)| {
        [name.as_str().into()]
    });
    b.transition(
        "Look",
        (
            ReadEdge(limits),
            ReadWhereEdge(gauges, Filter::new().key_in("name", ["Oil"])),
            ReadAllEdge(notes),
        ),
        (),
        |_: &(String, i64), _: &(String, i64), _: Vec<&(String, i64)>| {},
    );
    let text = dot(&b.build().unwrap());
    for line in [
        "  p0 -> t0 [style=dotted, color=dimgray, arrowhead=normal, penwidth=1];\n",
        "  p1 -> t0 [style=dotted, color=dimgray, arrowhead=normal, penwidth=1, label=\"name in {Oil}\"];\n",
        "  p2 -> t0 [style=dotted, color=dimgray, arrowhead=empty, penwidth=1, label=\"*\"];\n",
    ] {
        assert!(text.contains(line), "{line}\n{text}");
    }
}

/// An edge that names the ports it reaches writes them as its label's last
/// line, after a filter or a `*`; and a net drawn on its own, such as the
/// net a token carries, dots its ports as a subnet's drawing does.
#[test]
fn an_edge_writes_the_ports_it_reaches_and_a_net_dots_its_ports() {
    let mut unit = NetBuilder::new();
    let orders_in = unit.place::<String>("Orders In");
    let shots_out = unit.place::<String>("Shots Out");
    unit.place::<String>("Notes");
    unit.port(orders_in);
    unit.port(shots_out);
    let unit = unit.build().unwrap();
    let dotted = dot(&unit);
    assert_eq!(
        dotted.matches("style=\"filled,dotted\"").count(),
        2,
        "the two trays, not the notes: {dotted}"
    );
    assert!(
        lines_for(&dotted, "p2 [")[0].contains("style=filled,"),
        "{dotted}"
    );
    assert!(!dotted.contains("peripheries"), "{dotted}");

    let mut b = NetBuilder::new();
    let orders = b.place::<String>("Orders");
    let field = b.place_nested::<Net>("Field");
    let squads = b.place_nested::<Net>("Squads");
    b.seed(field, [unit]);
    b.transition(
        "Hand Over",
        (
            TakeEdge(orders),
            MutateEdge(field).reaching(["Orders In"]),
            ReadAllEdge(squads).reaching(["Orders In", "Shots Out"]),
        ),
        (),
        |_: String, _: &mut Net, _: Vec<&Net>| {},
    );
    let text = dot(&b.build().unwrap());
    assert!(
        text.contains(
            "  p1 -> t0 [dir=both, arrowhead=dot, arrowtail=dot, color=dimgray, penwidth=2, \
             label=\"reaches Orders In\"];\n"
        ),
        "{text}"
    );
    assert!(
        text.contains("label=\"*\\nreaches Orders In, Shots Out\"];\n"),
        "{text}"
    );
}

// ----- characters Graphviz cannot draw as they are -----------------------------

/// A net whose names hold a tab, a carriage return, a carriage return and a
/// line feed, and a NUL, in every kind of label: a place, a transition, a
/// subnet place and its port, a group and a token.
fn odd_characters() -> Net {
    let mut b = NetBuilder::new();
    let port = b.place::<String>("Gate\tA\0");
    b.port(port);
    let wing = b.build().expect("the wing builds");
    let mut b = NetBuilder::new();
    let notes = b.place::<String>("Notes\r\nKept");
    let out = b.place::<String>("Out\rFront");
    let wing = b.subnet_place("Wing\t1\0", wing);
    let gate = b.port_of::<String>(wing, "Gate\tA\0");
    let frame = b.group("Frame\tOne\0");
    b.in_group(frame, out);
    b.seed(notes, ["a\tb\0c".to_owned()]);
    b.transition(
        "Copy\tIt\0",
        (TakeEdge(notes),),
        (out, gate),
        |s: String| (s.clone(), s),
    );
    b.build().expect("every name has a character")
}

#[test]
fn a_tab_a_carriage_return_and_a_nul_are_drawn_as_graphviz_can() {
    let net = odd_characters();
    let text = dot(&net);
    assert!(!text.contains('\t'), "no tab: {text:?}");
    assert!(!text.contains('\r'), "no carriage return: {text:?}");
    assert!(!text.contains('\0'), "no NUL: {text:?}");
    assert!(text.contains(r#"label="Notes\nKept"#), "{text}");
    assert!(text.contains(r#"label="Out\nFront"];"#), "{text}");
    assert!(text.contains(r#"label="Copy    It"];"#), "{text}");
    assert!(text.contains(r#"label="Frame    One";"#), "{text}");
    assert!(
        text.contains(r#"label="{Wing    1|{<port0> Gate    A\n0}}"];"#),
        "{text}"
    );
    // The names are the net's as they were given.
    assert!(net.place_named("Notes\r\nKept").is_some());
    assert!(net.transition_named("Copy\tIt\0").is_some());
}

/// `dot` draws the text, where a NUL made it fail. Skipped where Graphviz is
/// not installed.
#[test]
fn graphviz_draws_a_net_whose_names_hold_them() {
    let Some(svg) = graphviz_svg(&dot(&odd_characters())) else {
        eprintln!("dot is not installed; skipped");
        return;
    };
    assert!(svg.contains("<svg"));
}

// ----- a source whose ids disagree ---------------------------------------------

/// A net that lists every edge but leaves one place and one transition out
/// of its lists, as a custom source that is wrong about itself might.
struct Forgetful(Net);

impl petritype_dot::DotSource for Forgetful {
    fn places(&self) -> impl Iterator<Item = (petritype::PlaceId, petritype::PlaceInfo<'_>)> {
        self.0.places().filter(|(_, info)| info.name != "Lost")
    }

    fn transitions(&self) -> impl Iterator<Item = (TransitionId, petritype::TransitionInfo<'_>)> {
        self.0.transitions().filter(|(_, info)| info.name != "Gone")
    }

    fn edges(&self) -> impl Iterator<Item = petritype::EdgeInfo> {
        self.0.edges()
    }

    fn groups(&self) -> impl Iterator<Item = &str> {
        self.0.groups()
    }

    fn token_ids(&self, place: petritype::PlaceId) -> impl Iterator<Item = petritype::TokenId> {
        self.0.tokens(place).map(|(id, _)| id)
    }

    fn label(&self, place: petritype::PlaceId, token: petritype::TokenId) -> Option<String> {
        self.0.label(place, token)
    }
}

/// An edge whose place or transition the source does not list is left
/// undrawn, where the renderer panicked on the lookup.
#[test]
fn an_edge_on_a_node_the_source_does_not_list_is_left_undrawn() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let lost = b.place::<u32>("Lost");
    let out = b.place::<u32>("Out");
    b.transition("Keep", (TakeEdge(inbox),), (out,), |n: u32| (n,));
    b.transition("Drop", (TakeEdge(inbox),), (lost,), |n: u32| (n,));
    b.transition("Gone", (TakeEdge(out),), (inbox,), |n: u32| (n,));
    let text = dot_with(&Forgetful(b.build().unwrap()), &Style::default());
    let edges: Vec<&str> = text.lines().filter(|line| line.contains(" -> ")).collect();
    assert_eq!(
        edges,
        [
            "  p0 -> t0 [color=black, penwidth=1];",
            "  t0 -> p1 [color=black, penwidth=1];",
            "  p0 -> t1 [color=black, penwidth=1];",
        ],
        "{text}"
    );
}

/// A net that lists its places, transitions and edges but none of its
/// groups, as a custom source that is wrong about itself might.
struct Groupless(Net);

impl petritype_dot::DotSource for Groupless {
    fn places(&self) -> impl Iterator<Item = (petritype::PlaceId, petritype::PlaceInfo<'_>)> {
        self.0.places()
    }

    fn transitions(&self) -> impl Iterator<Item = (TransitionId, petritype::TransitionInfo<'_>)> {
        self.0.transitions()
    }

    fn edges(&self) -> impl Iterator<Item = petritype::EdgeInfo> {
        self.0.edges()
    }

    fn groups(&self) -> impl Iterator<Item = &str> {
        std::iter::empty()
    }

    fn token_ids(&self, place: petritype::PlaceId) -> impl Iterator<Item = petritype::TokenId> {
        self.0.tokens(place).map(|(id, _)| id)
    }

    fn label(&self, place: petritype::PlaceId, token: petritype::TokenId) -> Option<String> {
        self.0.label(place, token)
    }
}

/// A node whose group the source does not list is drawn among the nodes in
/// no group, where it was left out and its edges drew to a bare Graphviz
/// node of its id.
#[test]
fn a_node_whose_group_the_source_does_not_list_is_drawn_among_the_rest() {
    let net = hike();
    let text = dot(&Groupless(hike()));
    assert!(!text.contains("subgraph"), "{text}");
    let nodes = net.places().count() + net.transitions().count();
    let drawn = text
        .lines()
        .filter(|line| line.starts_with("  p") || line.starts_with("  t"))
        .filter(|line| !line.contains(" -> "))
        .count();
    assert_eq!(drawn, nodes, "every node, once: {text}");
    // The same nodes and edges as the source that lists its groups.
    let listed = dot(&net);
    let edges = |text: &str| -> Vec<String> {
        text.lines()
            .filter(|line| line.contains(" -> "))
            .map(str::to_owned)
            .collect()
    };
    assert_eq!(edges(&text), edges(&listed));
}

#[test]
fn two_styles_compare_with_eq() {
    fn eq<T: Eq>(a: &T, b: &T) -> bool {
        a == b
    }
    let wide = Style {
        rankdir: RankDir::LR,
        ..Style::default()
    };
    assert!(eq(&Style::default(), &Style::default()));
    assert!(!eq(&wide, &Style::default()));
}

// ----- open nets --------------------------------------------------------------------

/// A depot declared open: `Inbox`, a port, in no group; `Shelf` and `Stock`
/// in the group `Store`; `Packer`, a subnet place holding a closed net whose
/// ports are `In` and `Out`; `Send` from the shelf into the packer and
/// `Ship` out of it into `Outbox`. With `open` false, the same net closed.
fn depot(open: bool) -> Net {
    let mut p = NetBuilder::new();
    let into = p.place::<u32>("In");
    let out = p.place::<u32>("Out");
    p.port(into);
    p.port(out);
    p.transition("Pack", (TakeEdge(into),), (out,), |n: u32| (n,));
    let packer = p.build().expect("a closed packer");

    let mut b = NetBuilder::new();
    let store = b.group("Store");
    let inbox = b.place::<u32>("Inbox");
    let shelf = b.place::<u32>("Shelf");
    let outbox = b.place::<u32>("Outbox");
    b.port(inbox);
    b.in_group(store, shelf);
    b.seed(inbox, [1, 2]);
    let packer = b.subnet_place("Packer", packer);
    let into = b.port_of::<u32>(packer, "In");
    let from = b.port_of::<u32>(packer, "Out");
    b.transition("Stock", (TakeEdge(inbox),), (shelf,), |n: u32| (n,))
        .group(store);
    b.transition("Send", (TakeEdge(shelf),), (into,), |n: u32| (n,));
    b.transition("Ship", (TakeEdge(from),), (outbox,), |n: u32| (n,));
    if open {
        b.open();
    }
    b.build().expect("a depot")
}

/// An open net's places are dashed but for its port, which is dotted as a
/// port is in a closed net, and never dashed as well.
#[test]
fn an_open_net_draws_its_places_dashed_and_its_port_and_a_frame_dotted() {
    let net = depot(true);
    assert_eq!(
        dot(&net),
        concat!(
            "digraph \"petritype\" {\n",
            "  rankdir=TB;\n",
            "  node [fontname=\"Helvetica\"];\n",
            "  p0 [shape=oval, style=\"filled,dotted\", fillcolor=deepskyblue, color=black, penwidth=1, label=\"Inbox\\n1\\n2\"];\n",
            "  p2 [shape=oval, style=\"filled,dashed\", fillcolor=deepskyblue, color=black, penwidth=1, label=\"Outbox\"];\n",
            "  p3 [shape=Mrecord, style=filled, fillcolor=lightskyblue, color=black, penwidth=1, label=\"{Packer|{<port0> In\\n0|<port1> Out\\n0}}\"];\n",
            "  t1 [shape=box, style=filled, fillcolor=lightgreen, color=black, penwidth=1, label=\"Send\"];\n",
            "  t2 [shape=box, style=filled, fillcolor=lightgreen, color=black, penwidth=1, label=\"Ship\"];\n",
            "  subgraph cluster_0 {\n",
            "    label=\"Store\";\n",
            "    style=dotted;\n",
            "    p1 [shape=oval, style=\"filled,dashed\", fillcolor=deepskyblue, color=black, penwidth=1, label=\"Shelf\"];\n",
            "    t0 [shape=box, style=filled, fillcolor=lightgreen, color=black, penwidth=1, label=\"Stock\"];\n",
            "  }\n",
            "  p0 -> t0 [color=black, penwidth=1];\n",
            "  t0 -> p1 [color=black, penwidth=1];\n",
            "  p1 -> t1 [color=black, penwidth=1];\n",
            "  t1 -> p3 [color=black, penwidth=1, headport=port0];\n",
            "  p3 -> t2 [color=black, penwidth=1, tailport=port1];\n",
            "  t2 -> p2 [color=black, penwidth=1];\n",
            "}\n",
        )
    );
    assert_eq!(dot(&net), dot(&net.snapshot().expect("a snapshot")));
}

#[test]
fn a_closed_net_differs_from_its_open_twin_only_in_its_places_outlines() {
    let (open, closed) = (dot(&depot(true)), dot(&depot(false)));
    let dashed = "style=\"filled,dashed\", fillcolor=deepskyblue";
    assert_eq!(
        open.matches(dashed).count(),
        2,
        "every place but the port and the subnet place"
    );
    assert_eq!(
        open.replace(dashed, "style=filled, fillcolor=deepskyblue"),
        closed
    );
    assert!(!closed.contains("dashed"));
    // The port, Inbox, is dotted in both, and the closed net's other places
    // are plain.
    for text in [&open, &closed] {
        assert!(
            lines_for(text, "p0 [")[0].contains("style=\"filled,dotted\", fillcolor=deepskyblue"),
            "{text}"
        );
        assert_eq!(
            text.matches("dotted").count(),
            2,
            "the port and the frame: {text}"
        );
        assert!(!text.contains("peripheries"), "{text}");
    }
    assert_eq!(
        closed
            .matches("style=filled, fillcolor=deepskyblue")
            .count(),
        2
    );

    // The closed subnet is drawn closed in its own drawing, its ports dotted.
    let net = depot(true);
    let packer = net.place_named("Packer").expect("the subnet place");
    let inside = dot(net.subnet(packer).expect("a subnet"));
    assert!(!inside.contains("dashed"), "{inside}");
    assert_eq!(
        inside
            .matches(
                "style=\"filled,dotted\", fillcolor=deepskyblue, color=black, penwidth=1, label="
            )
            .count(),
        2,
        "{inside}"
    );
    assert!(!inside.contains("peripheries"), "{inside}");

    // An open net a token carries is drawn dashed on its own.
    let mut w = NetBuilder::new();
    let todo = w.place::<u32>("Todo");
    w.open();
    w.seed(todo, [1]);
    let worker = w.build().expect("an open worker");
    let mut b = NetBuilder::new();
    let yard = b.place_nested::<Net>("Yard");
    b.seed(yard, [worker]);
    let net = b.build().expect("a closed yard");
    assert!(!dot(&net).contains("dashed"));
    let (token, _) = net.tokens(yard.id()).next().expect("the worker");
    let inner = net.nested(yard.id(), token).expect("a nested place")[0].1;
    assert!(dot(inner).contains("p0 [shape=oval, style=\"filled,dashed\""));
}

/// The SVG Graphviz draws of DOT text, or `None` where `dot` is not
/// installed.
fn graphviz_svg(text: &str) -> Option<String> {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let mut child = Command::new("dot")
        .arg("-Tsvg")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    child
        .stdin
        .take()
        .expect("piped")
        .write_all(text.as_bytes())
        .expect("dot reads its input");
    let drawn = child.wait_with_output().expect("dot runs");
    assert!(
        drawn.status.success(),
        "{}",
        String::from_utf8_lossy(&drawn.stderr)
    );
    Some(String::from_utf8_lossy(&drawn.stdout).into_owned())
}

/// What Graphviz makes of the attributes, for the depot open and closed: an
/// open net's places dashed `5,2`, and the frame and the port dotted `1,5`,
/// the port with one outline. Skipped where Graphviz is not installed.
#[test]
fn graphviz_dashes_an_open_net_s_places_longer_than_it_dots_a_frame_and_a_port() {
    // Shelf and Outbox when the depot is open, and none when it is closed.
    for (open, dashed) in [(true, 2), (false, 0)] {
        let Some(svg) = graphviz_svg(&dot(&depot(open))) else {
            eprintln!("dot is not installed; skipped");
            return;
        };
        let count = |element: &str, pattern: &str| {
            let dashes = format!("stroke-dasharray=\"{pattern}\"");
            svg.lines()
                .filter(|line| line.starts_with(element) && line.contains(&dashes))
                .count()
        };
        assert_eq!(count("<ellipse", "5,2"), dashed, "{svg}");
        assert_eq!(count("<polygon", "1,5"), 1, "the frame: {svg}");
        assert_eq!(count("<ellipse", "1,5"), 1, "the port: {svg}");
        // Inbox, `p0`, is drawn with one outline, its dotted ring.
        let inbox: Vec<&str> = svg
            .lines()
            .skip_while(|line| *line != "<title>p0</title>")
            .take_while(|line| *line != "</g>")
            .filter(|line| line.starts_with("<ellipse"))
            .collect();
        assert_eq!(inbox.len(), 1, "one ring: {svg}");
        assert!(inbox[0].contains("stroke-dasharray=\"1,5\""), "{svg}");
    }
}
