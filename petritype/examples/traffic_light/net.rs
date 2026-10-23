//! The traffic light net: cars wait at a light that cycles red, green and
//! amber, and all go when it turns green.
//!
//! This is the conformance net `traffic-light` of the shared protocol
//! (`../petritype/PROTOCOL.md` §11), the drain half of the
//! edges-on-empty-places example, split from the shop on 2026-10-16:
//! places, transitions, edges, outputs, controls and styles exactly as the
//! two runtimes agreed them, so that a transcript recorded against the
//! Python runner can replay against this one. It depends on `petritype` and
//! `serde` only, so that a runner's example binary can include this file as
//! it is.
//!
//! Every place is serializable, so that a host can `inject` into any of
//! them. The light is one place, `Light`, holding one token, a [`Colour`]:
//! red, green or amber, whose JSON form is the plain string, `"red"`, as it
//! was when the token was a string. It starts red. Cars are strings; they
//! arrive in `Waiting` from outside and go to `Passed`. The colours go round
//! red, green, amber and red again ([`next_colour`]). Two transitions, one
//! that moves the cars and one that changes the light, declared in this
//! order:
//!
//! - `Go`, guarded on green and someone waiting, *drains* `Waiting` (every
//!   car waiting, oldest first, all consumed) and *reads* `Light`
//!   ([`SnapshotEdge`]: the colour is cloned, not taken), and writes the
//!   cars, oldest first, to `Passed`. The light's token is untouched: it
//!   keeps its id.
//! - `Change`, unguarded, takes the light and writes the next colour back
//!   to `Light`. It is always fireable: the light cycles by itself and the
//!   net never quiesces, as a real junction does not.
//!
//! `Go` needs "someone waiting" in its guard because a [`DrainEdge`]
//! accepts an empty place, and a step fires one transition, under the
//! default tie-break the first declared: without it `Go` would fire at
//! every green taking nothing, and `Change` would never fire. The guard hands the step
//! to `Change` when the queue is empty, so this example never fires a drain
//! on an empty queue (the shop's survey shows the zero case). `Go`'s only
//! consuming edge is the drain, which is what
//! `assert_no_source_transitions` flags; the guard is what makes it safe,
//! so this net does not ask for that check. `Change` takes from `Light` and
//! writes back to it, a loop that `assert_acyclic` would refuse, so this
//! net does not ask for that one either. (On 2026-10-16 the light went from
//! three places to one place, and then to two transitions in all.)
//!
//! What the net declares beside its shape (2026-10-16, net-declared
//! controls), which a viewer draws without knowing the net:
//!
//! - The light's token is coloured as the lamp it lights ([`TokenStyle`]:
//!   red `#ff6347`, green `#32cd32`, amber `#ffa500`), and `Light` is
//!   filled with its token's colour ([`PlaceStyle::fill_token`]).
//! - Two controls, in this order: `car` (`+ Car`), an inject into
//!   `Waiting` given a text box, which adds a car named as typed, or `Car N`
//!   when nothing is typed ([`car_name`], read off the marking its `make` is
//!   handed); and `change` (`Change`), a button that fires `Change`.

use petritype::control::{Control, ControlKind, Value};
use petritype::style::{PlaceStyle, Rgb, Style, TokenStyle};
use petritype::{
    BuildError, DrainEdge, Net, NetBuilder, PlaceRef, SnapshotEdge, TakeEdge, TransitionId,
};
use serde::{Deserialize, Serialize};

/// The light's colour: the one token of `Light`. Written in JSON as the
/// lower-case string, `"red"`, `"green"` or `"amber"`, and shown as that
/// too: `Display` is the name, and `Debug`, which is the token's label on
/// the wire, the name as a string's `Debug` writes it, `"red"`, as it was
/// when the token was a string (and as Python's `'red'` reads).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Colour {
    /// Stop.
    Red,
    /// Go.
    Green,
    /// Stop, if you can.
    Amber,
}

impl Colour {
    /// The colour's name, as JSON writes it.
    fn name(self) -> &'static str {
        match self {
            Colour::Red => "red",
            Colour::Green => "green",
            Colour::Amber => "amber",
        }
    }
}

impl std::fmt::Display for Colour {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl std::fmt::Debug for Colour {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self.name(), f)
    }
}

/// The lamp each colour lights, the colour a viewer draws the token in (the
/// junction notebook's tomato, lime green and orange).
impl TokenStyle for Colour {
    fn style(&self) -> Style {
        Style::default().with_colour(match self {
            Colour::Red => Rgb(0xff, 0x63, 0x47),
            Colour::Green => Rgb(0x32, 0xcd, 0x32),
            Colour::Amber => Rgb(0xff, 0xa5, 0x00),
        })
    }
}

/// The net, its places and its transitions. Its controls are on the net
/// ([`Net::controls`]).
pub struct TrafficLight {
    pub net: Net,
    /// The light: one token, its [`Colour`].
    pub light: PlaceRef<Colour>,
    /// Cars waiting at the light, oldest first.
    pub waiting: PlaceRef<String>,
    /// Every car that has gone past the light, in the order they went.
    pub passed: PlaceRef<String>,
    /// `Go`: everyone waiting goes, while the light is green.
    pub go: TransitionId,
    /// `Change`: the light to its next colour.
    pub change: TransitionId,
}

/// The colour after `colour`: red to green, green to amber, amber to red.
pub fn next_colour(colour: Colour) -> Colour {
    match colour {
        Colour::Red => Colour::Green,
        Colour::Green => Colour::Amber,
        Colour::Amber => Colour::Red,
    }
}

/// The light's colour as `Go`'s guard reads it: the front token of
/// `light`, or `None` while the place is empty.
pub fn colour(net: &Net, light: PlaceRef<Colour>) -> Option<Colour> {
    let (token, _) = net.tokens(light.id()).next()?;
    net.token::<Colour>(light.id(), token).copied()
}

/// The car a press of `+ Car` adds to `Waiting`: `typed` when something was
/// typed, else `Car N`, `N` one more than the cars in `Waiting` and `Passed`
/// together. Read off the marking, with no counter of its own, so that
/// every runtime names it alike.
pub fn car_name(
    net: &Net,
    waiting: PlaceRef<String>,
    passed: PlaceRef<String>,
    typed: &str,
) -> String {
    if typed.is_empty() {
        format!(
            "Car {}",
            net.count(waiting.id()) + net.count(passed.id()) + 1
        )
    } else {
        typed.to_owned()
    }
}

/// `Go`'s body: every car that waited, oldest first, for `Passed`; the
/// light was only read.
pub fn go(cars: Vec<String>, _light: Colour) -> (Vec<String>,) {
    (cars,)
}

/// `Change`'s body: the next colour, for `Light`.
pub fn change(light: Colour) -> (Colour,) {
    (next_colour(light),)
}

/// The traffic light as §11 seeds it: red in `Light`, every other place
/// empty, every transition switched on, no parameters; the light styled,
/// and the two controls.
pub fn build_traffic_light() -> Result<TrafficLight, BuildError> {
    let mut b = NetBuilder::new();
    b.resettable();
    let light = b.place::<Colour>("Light");
    let waiting = b.place::<String>("Waiting");
    let passed = b.place::<String>("Passed");
    // Every place takes a token injected over the wire, cars into `Waiting`
    // above all.
    b.serializable(light);
    b.serializable(waiting);
    b.serializable(passed);
    b.seed(light, [Colour::Red]);
    // The token as the lamp it lights, and the place filled with it.
    b.styled(light);
    b.place_style(light, PlaceStyle::fill_token());

    let go = b
        .transition(
            "Go",
            (DrainEdge(waiting), SnapshotEdge(light)),
            (passed,),
            go,
        )
        .guard(move |net| colour(net, light) == Some(Colour::Green) && net.count(waiting.id()) > 0)
        .id();
    let change = b
        .transition("Change", (TakeEdge(light),), (light,), change)
        .id();

    // An inject into `Waiting` given a text box: `make` is handed the net,
    // so the name of an untyped car is read off the marking. A text control
    // is only ever pressed with text.
    let car = Control::inject("car", "+ Car", waiting, move |value, net| {
        let typed = match value {
            Value::Text(typed) => typed,
            _ => String::new(),
        };
        car_name(net, waiting, passed, &typed)
    })
    .with_kind(ControlKind::Text {
        placeholder: Some("name".to_owned()),
    });
    b.control(car)
        .control(Control::fire("change", "Change", change));

    Ok(TrafficLight {
        net: b.build()?,
        light,
        waiting,
        passed,
        go,
        change,
    })
}
