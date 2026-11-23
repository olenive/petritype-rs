//! Integration tests for an input edge that names the ports it reaches in
//! the nets its tokens carry (DESIGN §8.5, 2026-11-23): `Reaching`, made by
//! `.reaching([..])` on every edge kind.
//!
//! A declaration and nothing more: it is carried on the read API, the
//! topology and the snapshot, refused on a place whose tokens carry no nets
//! and for an empty name, and it changes nothing about firing, a name the
//! token's net lacks included.

use petritype::{
    BuildError, DrainEdge, EdgeInfo, Event, Filter, MutateEdge, MutateWhereEdge, Net, NetBuilder,
    PlaceRef, ReadAllEdge, ReadEdge, ReadWhereEdge, SurveyEdge, TakeAllEdge, TakeEdge,
    TakeWhereEdge,
};

/// A unit: `Orders In` and `Shots Out`, both ports, and `Notes`, which is
/// not.
fn unit() -> Net {
    let mut b = NetBuilder::new();
    let orders_in = b.place::<String>("Orders In");
    let shots_out = b.place::<String>("Shots Out");
    b.place::<String>("Notes");
    b.port(orders_in);
    b.port(shots_out);
    b.build().expect("a unit")
}

/// The reaches of every input edge, in edge order.
fn reaches(net: &Net) -> Vec<Vec<String>> {
    net.edges()
        .filter_map(|edge| match edge {
            EdgeInfo::Input { reaches, .. } => Some(reaches),
            _ => None,
        })
        .collect()
}

fn names(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

#[test]
fn every_edge_kind_may_name_the_ports_it_reaches() {
    let mut b = NetBuilder::new();
    let places: Vec<PlaceRef<Net>> = (0..11)
        .map(|i| b.place_nested::<Net>(format!("Field {i}")))
        .collect();
    for &place in &places[4..6] {
        b.keys(place, ["size"], |net: &Net| {
            [petritype::KeyValue::Int(net.places().count() as i64)]
        });
    }
    b.keys(places[9], ["size"], |net: &Net| {
        [petritype::KeyValue::Int(net.places().count() as i64)]
    });
    let any = Filter::new();
    b.transition(
        "Everything",
        (
            TakeEdge(places[0]).reaching(["Orders In"]),
            TakeAllEdge(places[1]).reaching(["Orders In"]),
            MutateEdge(places[2]).reaching(["Orders In", "Shots Out"]),
            DrainEdge(places[3]).reaching(["Shots Out"]),
            TakeWhereEdge(places[4], any.clone()).reaching(["Orders In"]),
            MutateWhereEdge(places[5], any.clone()).reaching(["Orders In"]),
            ReadEdge(places[6]).reaching(["Shots Out"]),
            ReadAllEdge(places[7]).reaching(["Shots Out"]),
            TakeEdge(places[8]),
            ReadWhereEdge(places[9], any).reaching(["Orders In"]),
        ),
        (),
        |_: Net,
         _: Vec<Net>,
         _: &mut Net,
         _: Vec<Net>,
         _: Net,
         _: &mut Net,
         _: &Net,
         _: Vec<&Net>,
         _: Net,
         _: &Net| {},
    );
    let net = b.build().unwrap();
    assert_eq!(
        reaches(&net),
        [
            names(&["Orders In"]),
            names(&["Orders In"]),
            names(&["Orders In", "Shots Out"]),
            names(&["Shots Out"]),
            names(&["Orders In"]),
            names(&["Orders In"]),
            names(&["Shots Out"]),
            names(&["Shots Out"]),
            Vec::new(),
            names(&["Orders In"]),
        ],
        "in declaration order, and none for the edge that names none"
    );
    let (_, info) = net.transitions().next().unwrap();
    assert_eq!(info.reaches.len(), info.inputs.len());
    // A snapshot copies a place of nets by forking its tokens, which takes
    // the `serde` feature.
    #[cfg(feature = "serde")]
    assert_eq!(
        net.snapshot().unwrap().edges().collect::<Vec<_>>(),
        net.edges().collect::<Vec<_>>(),
        "a snapshot carries them too"
    );
    let _ = (SurveyEdge(places[10]).reaching(["x"]), places[10]);
}

/// A declaration changes nothing: a body reaching a port the net has, or
/// naming one it lacks, fires exactly as it would without the names.
#[test]
fn a_reach_changes_nothing_about_firing() {
    /// The events, each as its `Debug` with the inner net's id left out:
    /// every unit built is a new net with an id of its own.
    fn run(reach: &[&str]) -> (Vec<String>, usize) {
        let mut b = NetBuilder::new();
        let orders = b.place::<String>("Orders");
        let field = b.place_nested::<Net>("Field");
        b.seed(field, [unit()]);
        b.seed(orders, ["Hold".to_owned()]);
        let reach: Vec<String> = reach.iter().map(|name| (*name).to_owned()).collect();
        let hand_over = if reach.is_empty() {
            b.transition(
                "Hand Over",
                (TakeEdge(orders), MutateEdge(field)),
                (),
                hand_over,
            )
            .id()
        } else {
            b.transition(
                "Hand Over",
                (TakeEdge(orders), MutateEdge(field).reaching(reach)),
                (),
                hand_over,
            )
            .id()
        };
        let mut net = b.build().unwrap();
        net.drain_events().for_each(drop);
        net.fire(hand_over).unwrap();
        let (token, _) = net.tokens(field.id()).next().unwrap();
        let inside = net.nested(field.id(), token).unwrap()[0].1;
        let count = inside.count(inside.place_named("Orders In").unwrap());
        let events = net
            .drain_events()
            .map(|event: Event| {
                let text = format!("{event:?}");
                match text.find("nested_id: NetId(") {
                    Some(at) => {
                        let close = text[at..].find(')').map_or(text.len(), |end| at + end + 1);
                        format!("{}{}", &text[..at], &text[close..])
                    }
                    None => text,
                }
            })
            .collect();
        (events, count)
    }
    fn hand_over(order: String, unit: &mut Net) {
        let tray = unit.place_ref::<String>("Orders In").unwrap();
        unit.put(tray, order);
    }

    let plain = run(&[]);
    assert_eq!(plain.1, 1);
    assert_eq!(run(&["Orders In"]), plain);
    assert_eq!(
        run(&["Mess Hall"]),
        plain,
        "a port the net lacks is not found out"
    );
}

#[test]
fn a_reach_is_refused_on_a_place_of_no_nets_and_for_an_empty_name() {
    let mut b = NetBuilder::new();
    let orders = b.place::<String>("Orders");
    b.transition(
        "Hand Over",
        (TakeEdge(orders).reaching(["Orders In"]),),
        (),
        |_: String| {},
    );
    let error = b.build().expect_err("orders carry no nets");
    assert!(
        matches!(&error, BuildError::ReachIntoNoNets { transition, place }
            if transition == "Hand Over" && place == "Orders"),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "transition \"Hand Over\" names ports it reaches through place \"Orders\", whose tokens \
         carry no nets"
    );

    let mut b = NetBuilder::new();
    let field = b.place_nested::<Net>("Field");
    b.transition(
        "Look",
        (ReadEdge(field).reaching(["Orders In", ""]),),
        (),
        |_: &Net| {},
    );
    assert!(matches!(
        b.build(),
        Err(BuildError::EmptyReachName { place, .. }) if place == "Field"
    ));
}

/// A field of a struct of inputs may name the ports it reaches.
#[test]
fn a_field_of_a_struct_of_inputs_may_reach() {
    #[derive(petritype::Inputs)]
    struct Handing<'a> {
        order: String,
        unit: &'a mut Net,
    }

    let mut b = NetBuilder::new();
    let orders = b.place::<String>("Orders");
    let field = b.place_nested::<Net>("Field");
    b.transition(
        "Hand Over",
        (HandingEdges {
            order: TakeEdge(orders),
            unit: MutateEdge(field).reaching(["Orders In"]),
        },),
        (),
        |h: Handing<'_>| {
            let tray = h.unit.place_ref::<String>("Orders In").unwrap();
            h.unit.put(tray, h.order);
        },
    );
    let net = b.build().unwrap();
    assert_eq!(reaches(&net), [Vec::new(), names(&["Orders In"])]);
}

#[cfg(feature = "serde")]
mod json {
    use super::*;
    use petritype::serde_json::{self, json};

    /// With `serde` an edge writes `reaches` only when it names some, and the
    /// topology's transitions likewise.
    #[test]
    fn reaches_are_written_only_when_there_are_some() {
        let mut b = NetBuilder::new();
        let orders = b.place::<String>("Orders");
        let field = b.place_nested::<Net>("Field");
        b.transition(
            "Hand Over",
            (TakeEdge(orders), MutateEdge(field).reaching(["Orders In"])),
            (),
            |_: String, _: &mut Net| {},
        );
        let net = b.build().unwrap();
        let topology = serde_json::to_value(net.topology()).unwrap();
        assert_eq!(
            topology["edges"][0],
            json!({"edge": "input", "place": 0, "transition": 0, "kind": "take"})
        );
        assert_eq!(topology["edges"][1]["reaches"], json!(["Orders In"]));
        assert_eq!(
            topology["transitions"][0]["reaches"],
            json!([[], ["Orders In"]])
        );
    }
}
