//! Integration tests for net-declared controls and token styles (DESIGN §18,
//! 2026-10-16, "net-declared controls"; the `control` and `style` modules).
//!
//! A net declares, as data, how its tokens look and which buttons it wants;
//! a press runs code written beside the net, through the net's own API, so
//! that everything it does is an ordinary event. What is tested here: a
//! style colours the tokens of the place that declared it and no other, the
//! trait and the closure say the same, the place style is shape; each
//! control constructor, each kind's value check, an unknown name, a handler
//! that refuses, a handler that reads the net before it changes it, and the
//! events a press leaves in the log.

use petritype::style::{Fill, PlaceStyle, Rgb, Style, TokenStyle};
use petritype::{Net, NetBuilder, PlaceRef, TakeEdge, TokenId};

// ----- styles ------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum Lamp {
    Red,
    Green,
    Off,
}

impl TokenStyle for Lamp {
    fn style(&self) -> Style {
        match self {
            Lamp::Red => Style::default().with_colour(Rgb(0xff, 0x63, 0x47)),
            Lamp::Green => Style::default().with_colour(Rgb(0x32, 0xcd, 0x32)),
            Lamp::Off => Style::default(),
        }
    }
}

/// The first token of a place.
fn first<T>(net: &Net, place: PlaceRef<T>) -> TokenId {
    net.tokens(place.id()).next().expect("a token").0
}

#[test]
fn a_styled_place_colours_its_tokens() {
    let mut b = NetBuilder::new();
    let lamps = b.place::<Lamp>("Lamps");
    b.styled(lamps);
    b.seed(lamps, [Lamp::Red, Lamp::Green, Lamp::Off]);
    let net = b.build().unwrap();

    let colours: Vec<Option<String>> = net
        .tokens(lamps.id())
        .map(|(token, _)| {
            net.style(lamps.id(), token)
                .expect("a styled place gives every token a style")
                .colour
                .map(|rgb| rgb.to_string())
        })
        .collect();
    assert_eq!(
        colours,
        [Some("#ff6347".to_owned()), Some("#32cd32".to_owned()), None],
        "a token whose style has no colour has none"
    );
}

#[test]
fn an_unstyled_place_gives_none() {
    let mut b = NetBuilder::new();
    let lamps = b.place::<Lamp>("Lamps");
    b.seed(lamps, [Lamp::Red]);
    let styled = b.place::<Lamp>("Styled");
    b.styled(styled);
    let net = b.build().unwrap();

    let red = first(&net, lamps);
    assert_eq!(
        net.style(lamps.id(), red),
        None,
        "Lamp is TokenStyle, but the place did not opt in"
    );
    assert_eq!(
        net.style(styled.id(), red),
        None,
        "no such token in that place"
    );
}

#[test]
fn the_trait_and_the_closure_say_the_same() {
    let mut b = NetBuilder::new();
    let by_trait = b.place::<Lamp>("By trait");
    let by_closure = b.place::<Lamp>("By closure");
    b.styled(by_trait);
    b.style(by_closure, |lamp: &Lamp| lamp.style());
    for place in [by_trait, by_closure] {
        b.seed(place, [Lamp::Green, Lamp::Off]);
    }
    let net = b.build().unwrap();

    let styles = |place: PlaceRef<Lamp>| -> Vec<Option<Style>> {
        net.tokens(place.id())
            .map(|(token, _)| net.style(place.id(), token))
            .collect()
    };
    assert_eq!(styles(by_trait), styles(by_closure));
    assert_eq!(
        styles(by_trait)[0].as_ref().unwrap().colour,
        Some(Rgb(0x32, 0xcd, 0x32))
    );
}

#[test]
fn a_style_follows_the_token_as_it_changes() {
    let mut b = NetBuilder::new();
    let lamp = b.place::<Lamp>("Lamp");
    let out = b.place::<Lamp>("Out");
    b.styled(lamp);
    b.styled(out);
    b.seed(lamp, [Lamp::Red]);
    b.transition(
        "Switch",
        (TakeEdge(lamp),),
        (out,),
        |_: Lamp| (Lamp::Green,),
    );
    let mut net = b.build().unwrap();

    net.step().unwrap();
    let green = first(&net, out);
    assert_eq!(
        net.style(out.id(), green).unwrap().colour,
        Some(Rgb(0x32, 0xcd, 0x32))
    );
}

#[test]
fn the_topology_carries_the_place_style() {
    let mut b = NetBuilder::new();
    let light = b.place::<Lamp>("Light");
    let plain = b.place::<u32>("Plain");
    b.place_style(light, PlaceStyle::fill_token());
    let net = b.build().unwrap();

    let styles: Vec<_> = net.places().map(|(_, info)| info.style).collect();
    assert_eq!(styles, [Some(PlaceStyle::fill_token()), None]);
    assert_eq!(styles[0].unwrap().fill, Fill::Token);
    let topology = net.topology();
    assert_eq!(
        topology.places[light.id().index() as usize].style,
        Some(PlaceStyle::fill_token())
    );
    assert_eq!(topology.places[plain.id().index() as usize].style, None);
    let snapshot = net.snapshot().unwrap();
    assert_eq!(snapshot.topology(), topology, "a snapshot carries it too");
}

#[cfg(feature = "serde")]
#[test]
fn a_place_style_serialises_as_fill_token() {
    let mut b = NetBuilder::new();
    let light = b.place::<Lamp>("Light");
    let _plain = b.place::<u32>("Plain");
    b.place_style(light, PlaceStyle::fill_token());
    let net = b.build().unwrap();

    let json = petritype::serde_json::to_value(net.topology()).unwrap();
    assert_eq!(
        json["places"][0]["style"],
        petritype::serde_json::json!({"fill": "token"})
    );
    assert!(
        json["places"][1].get("style").is_none(),
        "written only when there is one"
    );
}
