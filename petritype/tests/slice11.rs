//! Integration tests for the eleventh slice: `Param<T>` inputs, `set_param`
//! and the `SetParam` command, and `Net::snapshot`.
//!
//! Two features that both answer "what can a reader hold?". A **parameter** is
//! a value the net carries for its transitions rather than a token that flows
//! between them (DESIGN §10): declared as an input, handed to the body as a
//! clone, replaceable between steps, and deliberately invisible to everything
//! that is about places — edges, fireability, the marking. A **snapshot** is
//! the net's state, owned and independent (DESIGN §8.2), which is what a
//! renderer on another thread or a test comparing two moments needs.
//!
//! So most of what is tested here is what parameters are *not*: not edges, not
//! tokens, not part of fireability, never consumed, and never printed.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use petritype::drivers::{self, Limits, Mode, OnFailure};
use petritype::{
    BuildError, Command, CommandError, Event, Net, NetBuilder, NetError, Param, PlaceId, PlaceRef,
    SnapshotEdge, TakeEdge, TransitionId,
};

/// A parameter type with no `Debug`, as a secret would be.
#[derive(Clone, PartialEq)]
struct ApiToken(String);

#[derive(Clone, PartialEq, Debug)]
struct Scale(u32);

#[derive(Clone, PartialEq, Debug)]
struct Cfg(u32);

/// Reads a place back as plain values, oldest first.
fn values<T: Clone + Send + Sync + 'static>(net: &Net, place: PlaceId) -> Vec<T> {
    net.tokens(place)
        .map(|(_, value)| {
            value
                .downcast_ref::<T>()
                .expect("the place holds this type")
                .clone()
        })
        .collect()
}

/// `Urls -> Fetch -> Pages`, where `Fetch` needs an `ApiToken` it is not told
/// about through any place.
fn token_net() -> (Net, PlaceRef<String>, PlaceRef<String>, TransitionId) {
    let mut b = NetBuilder::new();
    let urls = b.place::<String>("Urls");
    let pages = b.place::<String>("Pages");
    b.param(ApiToken(String::from("first")));
    let fetch = b
        .transition(
            "Fetch",
            (TakeEdge(urls), Param::<ApiToken>::new()),
            (pages,),
            |url: String, token: ApiToken| (format!("{url}/{}", token.0),),
        )
        .id();
    let net = b.build().expect("a well-formed net");
    (net, urls, pages, fetch)
}

// ----- the body gets the value --------------------------------------------

#[test]
fn a_parameter_value_reaches_the_body() {
    let (mut net, urls, pages, fetch) = token_net();
    net.put(urls, String::from("a"));

    assert_eq!(net.step().expect("a working body"), Some(fetch));
    assert_eq!(values::<String>(&net, pages.id()), ["a/first"]);
}

#[test]
fn arguments_follow_the_order_of_the_input_tuple() {
    let mut b = NetBuilder::new();
    let numbers = b.place::<u32>("Numbers");
    let out = b.place::<String>("Out");
    b.seed(numbers, [7]);
    b.param(Cfg(1));
    b.param(Scale(10));
    // A parameter before the edge, another after it: the body's parameters
    // are the tuple, in the order it was written.
    b.transition(
        "Mix",
        (
            Param::<Cfg>::new(),
            TakeEdge(numbers),
            Param::<Scale>::new(),
        ),
        (out,),
        |cfg: Cfg, n: u32, scale: Scale| (format!("{}:{}:{}", cfg.0, n, scale.0),),
    );
    let mut net = b.build().expect("a well-formed net");

    net.step().expect("a working body");
    assert_eq!(values::<String>(&net, out.id()), ["1:7:10"]);
}

#[test]
fn a_declared_parameter_with_no_value_is_refused() {
    let mut b = NetBuilder::new();
    let urls = b.place::<String>("Urls");
    let pages = b.place::<String>("Pages");
    // Everything else about the net is fine; only the value is missing.
    b.transition(
        "Fetch",
        (TakeEdge(urls), Param::<ApiToken>::new()),
        (pages,),
        |url: String, token: ApiToken| (format!("{url}/{}", token.0),),
    );

    let error = b.build().expect_err("no value for ApiToken");
    assert!(
        matches!(
            error,
            BuildError::MissingParam { ref transition, type_name }
                if transition == "Fetch" && type_name.ends_with("ApiToken")
        ),
        "{error:?}",
    );
}

#[test]
fn a_registered_parameter_no_transition_declares_is_fine() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.param(Scale(3));
    b.transition("Copy", (TakeEdge(inbox),), (out,), |n: u32| (n,));

    let net = b
        .build()
        .expect("a value nobody asked for is still a value");
    assert_eq!(net.param::<Scale>(), Some(&Scale(3)));
}

// ----- changing and reading -----------------------------------------------

#[test]
fn set_param_replaces_the_value_for_the_next_firing() {
    let (mut net, urls, pages, _) = token_net();
    net.put(urls, String::from("a"));
    net.step().expect("a working body");
    let before = net.state_counter();
    net.drain_events().count();

    net.set_param(ApiToken(String::from("second")))
        .expect("a registered parameter type");

    assert_eq!(net.state_counter(), before + 1, "one event, one tick");
    let events: Vec<Event> = net.drain_events().collect();
    assert!(
        matches!(
            events[..],
            [Event::ParamChanged { type_name }] if type_name.ends_with("ApiToken")
        ),
        "{events:?}",
    );

    net.put(urls, String::from("b"));
    net.step().expect("a working body");
    assert_eq!(values::<String>(&net, pages.id()), ["a/first", "b/second"]);
}

#[test]
fn set_param_of_an_unregistered_type_is_refused() {
    let (mut net, _, _, _) = token_net();

    let error = net.set_param(Scale(2)).expect_err("no Scale parameter");
    assert!(
        matches!(error, NetError::NoSuchParam { type_name } if type_name.ends_with("Scale")),
        "{error:?}",
    );
    assert_eq!(net.state_counter(), 0, "a refusal changes nothing");
    assert_eq!(net.drain_events().count(), 0);
}

#[test]
fn param_reads_the_current_value_and_nothing_else() {
    let (mut net, _, _, _) = token_net();

    assert!(net.param::<ApiToken>().is_some_and(|t| t.0 == "first"));
    assert_eq!(net.param::<Scale>(), None, "not a parameter of this net");

    net.set_param(ApiToken(String::from("second")))
        .expect("registered");
    assert!(net.param::<ApiToken>().is_some_and(|t| t.0 == "second"));
}

#[test]
fn two_newtypes_over_one_type_are_two_parameters() {
    #[derive(Clone, PartialEq, Debug)]
    struct GithubToken(String);
    #[derive(Clone, PartialEq, Debug)]
    struct OpenAiToken(String);

    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<String>("Out");
    b.seed(inbox, [1]);
    b.param(GithubToken(String::from("gh")));
    b.param(OpenAiToken(String::from("oa")));
    b.transition(
        "Both",
        (
            TakeEdge(inbox),
            Param::<GithubToken>::new(),
            Param::<OpenAiToken>::new(),
        ),
        (out,),
        |_: u32, gh: GithubToken, oa: OpenAiToken| (format!("{}+{}", gh.0, oa.0),),
    );
    let mut net = b.build().expect("two distinct parameter types");

    net.step().expect("a working body");
    assert_eq!(values::<String>(&net, out.id()), ["gh+oa"]);
    assert_eq!(
        net.param::<GithubToken>(),
        Some(&GithubToken(String::from("gh")))
    );

    net.set_param(GithubToken(String::from("gh2")))
        .expect("registered");
    assert_eq!(
        net.param::<OpenAiToken>(),
        Some(&OpenAiToken(String::from("oa"))),
        "the other one is untouched",
    );
}

// ----- not an edge ---------------------------------------------------------

#[test]
fn the_read_api_lists_parameters_apart_from_edges() {
    let mut b = NetBuilder::new();
    let urls = b.place::<String>("Urls");
    let config = b.place::<u32>("Config");
    let pages = b.place::<String>("Pages");
    b.param(Cfg(1));
    b.param(Scale(2));
    b.transition(
        "Fetch",
        (
            TakeEdge(urls),
            Param::<Cfg>::new(),
            SnapshotEdge(config),
            Param::<Scale>::new(),
        ),
        (pages,),
        |url: String, _: Cfg, _: u32, _: Scale| (url,),
    );
    let net = b.build().expect("a well-formed net");

    let (_, info) = net.transitions().next().expect("one transition");
    assert_eq!(
        info.inputs,
        [
            (urls.id(), petritype::EdgeKind::Take),
            (config.id(), petritype::EdgeKind::Snapshot),
        ],
        "the edges, in order, and only the edges",
    );
    assert_eq!(info.params.len(), 2, "the parameters, in order");
    assert!(info.params[0].ends_with("Cfg"));
    assert!(info.params[1].ends_with("Scale"));

    // A parameter is no node and no edge, so a diagram never draws one.
    assert_eq!(
        net.edges().count(),
        3,
        "two input edges and one output edge"
    );
}

#[test]
fn parameters_do_not_affect_fireability() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let out = b.place::<u32>("Out");
    b.param(Scale(2));
    let scale = b
        .transition(
            "Scale",
            (TakeEdge(inbox), Param::<Scale>::new()),
            (out,),
            |n: u32, s: Scale| (n * s.0,),
        )
        .id();
    let mut net = b.build().expect("a well-formed net");

    assert!(!net.is_fireable(scale), "the input place is empty");
    net.put(inbox, 1);
    assert!(
        net.is_fireable(scale),
        "a parameter never withholds a firing"
    );

    net.disable(scale).expect("a transition of this net");
    assert!(!net.is_fireable(scale), "the switch still works");
    net.enable(scale).expect("a transition of this net");
    assert!(net.is_fireable(scale));

    // And replacing one neither enables nor disables anything.
    net.set_param(Scale(5)).expect("registered");
    assert!(net.is_fireable(scale));
}

/// A mutating edge is filled in after the other inputs have been taken, by
/// argument position — so a parameter standing in front of one must not shift
/// the borrow onto the wrong argument. Such a transition runs inline, which is
/// the other half of what this checks: a parameter does not make a firing
/// detachable, and does not stop it being so either.
#[test]
fn a_mutating_edge_after_a_parameter_still_borrows_its_own_token() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let live = b.place::<u32>("Live");
    b.seed(inbox, [1]);
    b.seed(live, [100]);
    b.param(Scale(3));
    let bump = b
        .transition(
            "Bump",
            (
                Param::<Scale>::new(),
                TakeEdge(inbox),
                petritype::MutateEdge(live),
            ),
            (),
            |scale: Scale, n: u32, live: &mut u32| {
                *live += n * scale.0;
            },
        )
        .id();
    let mut net = b.build().expect("a well-formed net");

    let (_, info) = net.transitions().next().expect("one transition");
    assert!(
        !info.detachable,
        "a mutating edge ties the firing to the net"
    );
    assert_eq!(info.params.len(), 1);

    net.step().expect("a working body");
    let token = net.tokens(live.id()).next().expect("the live token").0;
    assert_eq!(net.token::<u32>(live.id(), token), Some(&103));
    assert_eq!(net.count(inbox.id()), 0);

    // The mutation is reported for the token it was made on, and the
    // parameter is reported not at all.
    let events: Vec<Event> = net.drain_events().collect();
    assert!(
        matches!(
            events[..],
            [
                Event::Mutated { place, token: mutated },
                Event::Fired { transition, .. }
            ] if place == live.id() && mutated == token && transition == bump
        ),
        "{events:?}",
    );
}

#[test]
fn a_transition_whose_only_input_is_a_parameter_is_a_source() {
    let mut b = NetBuilder::new();
    b.assert_no_source_transitions();
    let out = b.place::<u32>("Out");
    b.param(Scale(2));
    b.transition("Spawn", (Param::<Scale>::new(),), (out,), |s: Scale| (s.0,));

    let error = b.build().expect_err("a parameter consumes nothing");
    assert!(
        matches!(error, BuildError::SourceTransition { ref transition } if transition == "Spawn"),
        "{error:?}",
    );
}

#[test]
fn a_firing_neither_consumes_nor_reports_a_parameter() {
    let (mut net, urls, _, fetch) = token_net();
    net.put(urls, String::from("a"));
    net.drain_events().count();

    net.step().expect("a working body");

    let events: Vec<Event> = net.drain_events().collect();
    match &events[..] {
        [
            Event::Fired {
                transition,
                consumed,
                produced,
            },
        ] => {
            assert_eq!(*transition, fetch);
            assert_eq!(consumed.len(), 1, "the url, and nothing for the parameter");
            assert_eq!(consumed[0].0, urls.id());
            assert_eq!(produced.len(), 1);
        }
        other => panic!("one Fired and no ParamChanged: {other:?}"),
    }
}

// ----- when the clone is taken --------------------------------------------

#[test]
fn the_clone_a_body_gets_is_taken_at_launch() {
    let (mut net, urls, pages, fetch) = token_net();
    net.put(urls, String::from("a"));

    // Stage one takes the value; the body is airborne with its own copy.
    let firing = net.launch(fetch).expect("fireable");
    net.set_param(ApiToken(String::from("second")))
        .expect("registered");
    net.distribute(firing.run()).expect("in flight");

    assert_eq!(
        values::<String>(&net, pages.id()),
        ["a/first"],
        "the firing kept the value it was launched with",
    );

    // The next launch sees the new one.
    net.put(urls, String::from("b"));
    net.step().expect("a working body");
    assert_eq!(values::<String>(&net, pages.id()), ["a/first", "b/second"]);
}

#[test]
fn a_concurrent_run_hands_every_body_the_value() {
    let seen = Arc::new(AtomicU32::new(0));
    let mut b = NetBuilder::new();
    let left = b.place::<u32>("Left");
    let right = b.place::<u32>("Right");
    let out = b.place::<u32>("Out");
    b.seed(left, [1]);
    b.seed(right, [2]);
    b.param(Scale(10));

    for (name, place) in [("Left scale", left), ("Right scale", right)] {
        let seen = Arc::clone(&seen);
        b.transition(
            name,
            (TakeEdge(place), Param::<Scale>::new()),
            (out,),
            move |n: u32, s: Scale| {
                seen.fetch_add(s.0, Ordering::Relaxed);
                (n * s.0,)
            },
        );
    }
    let mut net = b.build().expect("a well-formed net");

    let summary = drivers::run(
        &mut net,
        Mode::Concurrent,
        Limits::default(),
        OnFailure::Stop,
    )
    .expect("nothing fails");
    assert!(summary.quiesced);
    assert_eq!(summary.fired_count, 2);
    assert_eq!(
        seen.load(Ordering::Relaxed),
        20,
        "both bodies saw Scale(10)"
    );

    let mut produced = values::<u32>(&net, out.id());
    produced.sort();
    assert_eq!(produced, [10, 20]);
}

// ----- the erased command -------------------------------------------------

#[test]
fn a_set_param_command_replaces_the_value() {
    let (mut net, urls, pages, _) = token_net();

    net.apply(Command::set_param(ApiToken(String::from("second"))))
        .expect("a registered parameter type");
    assert_eq!(net.state_counter(), 1);
    let events: Vec<Event> = net.drain_events().collect();
    assert!(
        matches!(
            events[..],
            [Event::ParamChanged { type_name }] if type_name.ends_with("ApiToken")
        ),
        "{events:?}",
    );

    net.put(urls, String::from("a"));
    net.step().expect("a working body");
    assert_eq!(values::<String>(&net, pages.id()), ["a/second"]);
}

#[test]
fn a_set_param_command_of_an_unregistered_type_is_refused() {
    let (mut net, _, _, _) = token_net();

    let error = net
        .apply(Command::set_param(Scale(2)))
        .expect_err("no Scale parameter");
    // Erased: the type cannot be named on the way in, only blamed.
    assert!(
        matches!(
            error,
            CommandError::Net(NetError::NoSuchParam { type_name }) if type_name == "<erased>"
        ),
        "{error:?}",
    );
    assert_eq!(net.state_counter(), 0, "a rejected command changes nothing");
    assert_eq!(net.drain_events().count(), 0);
}

#[test]
fn the_debug_of_a_set_param_command_shows_no_value() {
    let command = Command::set_param(ApiToken(String::from("s3cret")));
    let text = format!("{command:?}");

    assert!(text.starts_with("SetParam"), "{text}");
    assert!(
        !text.contains("s3cret"),
        "a parameter is never printed: {text}"
    );
}

// ----- snapshots ----------------------------------------------------------

#[test]
fn a_snapshot_copies_the_marking_and_then_goes_its_own_way() {
    let (mut net, urls, pages, fetch) = token_net();
    net.put(urls, String::from("a"));
    let token = net.tokens(urls.id()).next().expect("one token").0;

    let taken = net.snapshot().expect("every place can clone");
    assert_eq!(taken.count(urls.id()), 1);
    assert_eq!(taken.count(pages.id()), 0);
    assert_eq!(
        taken
            .tokens(urls.id())
            .map(|(id, _)| id)
            .collect::<Vec<_>>(),
        [token],
        "the same token ids, not new ones",
    );
    assert_eq!(
        taken.token::<String>(urls.id(), token),
        Some(&String::from("a"))
    );
    assert_eq!(net.label(urls.id(), token), taken.label(urls.id(), token));
    assert_eq!(taken.place_named("Urls"), Some(urls.id()));
    assert_eq!(taken.transition_named("Fetch"), Some(fetch));

    // The net moves on; the snapshot does not.
    net.put(urls, String::from("b"));
    net.step().expect("a working body");
    let page = net.tokens(pages.id()).next().expect("a page").0;
    net.remove(pages.id(), page).expect("a token of this place");
    assert_eq!(taken.count(urls.id()), 1);
    assert_eq!(taken.count(pages.id()), 0);
    assert_eq!(taken.fired_count(), 0);
    assert_eq!(net.fired_count(), 1);

    // And the snapshot cannot be changed at all: it has no `&mut` API, so
    // nothing done to it can reach the net either.
    assert_eq!(net.count(urls.id()), 1, "\"b\" was consumed, not \"a\"");
}

#[test]
fn a_snapshot_is_send_sync_and_clone() {
    fn assert_send_sync_clone<T: Send + Sync + Clone>() {}
    assert_send_sync_clone::<petritype::Snapshot>();

    let (mut net, urls, _, _) = token_net();
    net.put(urls, String::from("a"));
    let taken = net.snapshot().expect("every place can clone");
    let copy = taken.clone();

    let token = copy.tokens(urls.id()).next().expect("one token").0;
    assert_eq!(copy.count(urls.id()), taken.count(urls.id()));
    assert_eq!(copy.label(urls.id(), token), taken.label(urls.id(), token));

    // It really can cross a thread.
    let over_there = std::thread::spawn(move || copy.count(urls.id()))
        .join()
        .expect("nothing panics");
    assert_eq!(over_there, 1);
}

#[test]
fn a_place_that_cannot_clone_refuses_the_snapshot() {
    let mut b = NetBuilder::new();
    let _fine = b.place::<u32>("Fine");
    let handles = b.place_no_clone::<String>("Handles");
    b.seed(handles, [String::from("a socket, say")]);
    let net = b.build().expect("a well-formed net");

    let error = net.snapshot().expect_err("Handles cannot clone its tokens");
    assert!(
        matches!(error, NetError::NoClonePlace { place, .. } if place == handles.id()),
        "{error:?}",
    );
    assert_eq!(
        error.to_string(),
        format!(
            "place \"Handles\" ({:?}) cannot clone its tokens",
            handles.id()
        ),
        "the name is for the reader, the id for a program",
    );
}

#[test]
fn a_snapshot_copies_the_switches_the_flight_and_the_counters() {
    let (mut net, urls, _, fetch) = token_net();
    net.put(urls, String::from("a"));
    net.put(urls, String::from("b"));
    net.step().expect("a working body");
    net.disable(fetch).expect("a transition of this net");
    net.enable(fetch).expect("a transition of this net");
    net.set_param(ApiToken(String::from("second")))
        .expect("registered");

    // Taken with the transition airborne.
    let firing = net.launch(fetch).expect("fireable");
    let taken = net.snapshot().expect("every place can clone");

    assert!(taken.is_in_flight(fetch), "it was in flight at the time");
    assert_eq!(taken.in_flight(), [fetch]);
    assert!(taken.is_enabled(fetch));
    assert_eq!(taken.fired_count(), 1);
    assert_eq!(taken.state_counter(), net.state_counter());
    assert_eq!(taken.topology_counter(), 0);
    assert_eq!(taken.id(), net.id());
    assert!(taken.param::<ApiToken>().is_some_and(|t| t.0 == "second"));

    // The net lands the firing and moves its parameter on; the snapshot is
    // still the moment it was taken.
    net.distribute(firing.run()).expect("in flight");
    net.set_param(ApiToken(String::from("third")))
        .expect("registered");
    assert!(taken.is_in_flight(fetch));
    assert!(taken.param::<ApiToken>().is_some_and(|t| t.0 == "second"));
    assert!(net.in_flight().is_empty());
}

#[test]
fn a_snapshot_labels_tokens_as_its_net_would() {
    let mut b = NetBuilder::new();
    b.label_limit(4);
    let notes = b.place::<String>("Notes");
    b.seed(notes, [String::from("a long enough note")]);
    let net = b.build().expect("a well-formed net");
    let token = net.tokens(notes.id()).next().expect("one token").0;

    let taken = net.snapshot().expect("every place can clone");
    assert_eq!(taken.label_limit(), 4);
    assert_eq!(taken.label(notes.id(), token), net.label(notes.id(), token));
    assert_eq!(
        taken.label(notes.id(), token),
        Some(String::from("\"a l...")),
        "the net's own limit, truncation and all",
    );
}

#[test]
fn a_snapshot_reports_the_same_transitions() {
    let mut b = NetBuilder::new();
    let urls = b.place::<String>("Urls");
    let pages = b.place::<String>("Pages");
    b.param(Cfg(1));
    b.transition(
        "Fetch",
        (TakeEdge(urls), Param::<Cfg>::new()),
        (pages,),
        |url: String, _: Cfg| (url,),
    );
    b.transition("Idle", (), (), || {});
    let mut net = b.build().expect("a well-formed net");
    net.disable(net.transition_named("Idle").expect("declared"))
        .expect("a transition of this net");

    let taken = net.snapshot().expect("every place can clone");
    let from_net: Vec<_> = net
        .transitions()
        .map(|(id, info)| (id, info.name.to_owned(), info.enabled, info.params.to_vec()))
        .collect();
    let from_snapshot: Vec<_> = taken
        .transitions()
        .map(|(id, info)| (id, info.name.to_owned(), info.enabled, info.params.to_vec()))
        .collect();
    assert_eq!(from_net, from_snapshot);
    assert_eq!(from_snapshot[0].3.len(), 1, "Fetch declares one parameter");

    // The wiring comes along too, so a renderer needs only the snapshot.
    let (_, info) = taken.transitions().next().expect("a first transition");
    assert_eq!(info.inputs, [(urls.id(), petritype::EdgeKind::Take)]);
    assert_eq!(info.outputs, [pages.id()]);
    assert!(info.detachable);
}

#[test]
fn snapshotting_says_nothing_and_counts_nothing() {
    let (mut net, urls, _, _) = token_net();
    net.put(urls, String::from("a"));
    net.drain_events().count();
    let before = net.state_counter();

    let _first = net.snapshot().expect("every place can clone");
    let _second = net.snapshot().expect("every place can clone");

    assert_eq!(net.state_counter(), before, "reading is not a change");
    assert_eq!(net.topology_counter(), 0);
    assert_eq!(net.drain_events().count(), 0);
}

// ----- the JSON route -----------------------------------------------------

#[cfg(feature = "serde")]
mod json {
    use super::*;

    use petritype::JsonCommand;
    use petritype::serde_json::{self, json};

    /// `ApiToken` travels under a name of its own; `Scale` is a parameter
    /// that was never marked, and so cannot travel at all.
    #[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    struct WireToken(String);

    fn wire_net() -> (Net, PlaceRef<String>, PlaceRef<String>) {
        let mut b = NetBuilder::new();
        let urls = b.place::<String>("Urls");
        let pages = b.place::<String>("Pages");
        b.serializable(pages);
        b.param(WireToken(String::from("first")));
        b.param(Scale(2));
        b.serializable_param::<WireToken>("api_token");
        b.transition(
            "Fetch",
            (TakeEdge(urls), Param::<WireToken>::new()),
            (pages,),
            |url: String, token: WireToken| (format!("{url}/{}", token.0),),
        );
        let net = b.build().expect("a well-formed net");
        (net, urls, pages)
    }

    #[test]
    fn a_json_set_param_replaces_the_value() {
        let (mut net, urls, pages) = wire_net();

        net.apply_json(JsonCommand::SetParam {
            param: String::from("api_token"),
            value: json!("second"),
        })
        .expect("a marked parameter");

        assert!(net.param::<WireToken>().is_some_and(|t| t.0 == "second"));
        net.put(urls, String::from("a"));
        net.step().expect("a working body");
        assert_eq!(values::<String>(&net, pages.id()), ["a/second"]);
    }

    #[test]
    fn an_unknown_parameter_name_is_refused() {
        let (mut net, _, _) = wire_net();

        let error = net
            .apply_json(JsonCommand::SetParam {
                param: String::from("nope"),
                value: json!("second"),
            })
            .expect_err("no parameter of that name");
        assert!(
            matches!(error, CommandError::NoSuchParamName { ref name } if name == "nope"),
            "{error:?}",
        );
        assert!(net.param::<WireToken>().is_some_and(|t| t.0 == "first"));
        assert_eq!(net.state_counter(), 0);
    }

    #[test]
    fn a_value_of_the_wrong_shape_is_refused() {
        let (mut net, _, _) = wire_net();

        let error = net
            .apply_json(JsonCommand::SetParam {
                param: String::from("api_token"),
                value: json!(7),
            })
            .expect_err("a WireToken is a string");
        assert!(
            matches!(error, CommandError::InvalidParam { ref param, .. } if param == "api_token"),
            "{error:?}",
        );
        assert!(net.param::<WireToken>().is_some_and(|t| t.0 == "first"));
    }

    #[test]
    fn a_set_param_command_goes_out_as_json_under_its_name() {
        let (net, _, _) = wire_net();
        let command = Command::set_param(WireToken(String::from("secret")));

        let wire = command.to_json(&net).expect("a marked parameter");
        assert_eq!(
            wire,
            JsonCommand::SetParam {
                param: String::from("api_token"),
                value: json!("secret"),
            },
        );
        assert_eq!(
            serde_json::to_string(&wire).expect("plain data"),
            r#"{"command":"set_param","param":"api_token","value":"secret"}"#,
        );
    }

    #[test]
    fn an_unmarked_parameter_cannot_be_encoded() {
        let (net, _, _) = wire_net();

        // A parameter of this net, but without a wire name.
        let error = Command::set_param(Scale(3))
            .to_json(&net)
            .expect_err("Scale was never marked");
        assert!(
            matches!(
                error,
                CommandError::ParamNotSerializable { type_name } if type_name.ends_with("Scale")
            ),
            "{error:?}",
        );

        // Not a parameter of this net at all: erased, so unnameable.
        let error = Command::set_param(Cfg(1))
            .to_json(&net)
            .expect_err("Cfg is not a parameter here");
        assert!(
            matches!(
                error,
                CommandError::ParamNotSerializable { type_name } if type_name == "<erased>"
            ),
            "{error:?}",
        );
    }

    #[test]
    fn two_parameters_with_one_wire_name_are_refused() {
        let mut b = NetBuilder::new();
        b.param(WireToken(String::from("a")));
        b.param(OtherToken(String::from("b")));
        b.serializable_param::<WireToken>("api_token");
        b.serializable_param::<OtherToken>("api_token");

        let error = b.build().expect_err("one name, two types");
        assert!(
            matches!(error, BuildError::DuplicateParamName { ref name } if name == "api_token"),
            "{error:?}",
        );
    }

    #[derive(Clone, serde::Serialize, serde::Deserialize)]
    struct OtherToken(String);

    #[test]
    fn a_snapshot_renders_tokens_as_json_like_its_net() {
        let (mut net, urls, pages) = wire_net();
        net.put(urls, String::from("a"));
        net.step().expect("a working body");
        let token = net.tokens(pages.id()).next().expect("one page").0;

        let taken = net.snapshot().expect("every place can clone");
        assert_eq!(taken.json(pages.id(), token), Some(json!("a/first")));
        assert_eq!(taken.json(pages.id(), token), net.json(pages.id(), token));
        assert_eq!(
            taken.json(urls.id(), token),
            None,
            "Urls is not serialisable"
        );
    }
}
