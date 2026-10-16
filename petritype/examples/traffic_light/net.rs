//! The traffic light net: cars wait at a light that cycles red, green and
//! amber, and all go when it turns green.
//!
//! This is the conformance net `traffic-light` of the shared protocol
//! (`../petritype/PROTOCOL.md` §11), the drain half of the
//! edges-on-empty-places example, split from the shop on 2026-10-16:
//! places, transitions, edges and outputs exactly as the two runtimes agreed
//! them, so that a transcript recorded against the Python runner can replay
//! against this one. It depends on `petritype` only, so that a runner's
//! example binary can include this file as it is.
//!
//! Every place holds strings; every place is serializable, so that a host
//! can `inject` into any of them. The light is one place, `Light`, holding
//! one token whose value is the colour, `"red"`, `"green"` or `"amber"`; it
//! starts red. Cars arrive in `Waiting` from outside, by `inject`, and go to
//! `Passed`. The colours go round red, green, amber and red again
//! ([`next_colour`]).
//!
//! - `Turn Green`, guarded on red, takes the light and *drains* `Waiting` —
//!   every car waiting, zero or more, all consumed — and writes the next
//!   colour, green, back to `Light` and the cars, oldest first, to `Passed`.
//!   With nobody waiting it still fires: [`DrainEdge`] does not require
//!   presence, so the light does not wait for a car.
//! - `Pass on Green`, guarded on green, takes the front car of `Waiting` and
//!   *reads* `Light` ([`SnapshotEdge`]: the colour is cloned, not taken),
//!   and writes the car to `Passed`. A car that arrives at amber or red
//!   waits for the next `Turn Green`.
//! - `Change`, guarded on anything but red, takes the light and writes the
//!   next colour back: green to amber, amber to red.
//!
//! The light has two transitions, not one, because a drain is
//! unconditional: whatever transition carries it takes every car waiting
//! whenever it fires, so it must be the one that fires only as the light
//! goes green, and `Change` does the other two steps. `Pass on Green` is
//! guarded as well as reading the light because `Light` is never empty, so
//! the read alone would let a car through at red; the read hands the colour
//! to the body and shows the dependency in a drawing, and the guard is what
//! holds the car back. (At first the light was three places, `Red`, `Green`
//! and `Amber`, with one token standing in one of them and four transitions
//! moving it round; it became one place the same day.)
//!
//! The light cycles by itself: some light transition is always fireable, so
//! the net never quiesces, as a real junction does not. Every transition
//! takes a token, so none is a source; `Turn Green` and `Change` each take
//! from `Light` and write back to it, a loop that `assert_acyclic` would
//! refuse, so this net does not ask for it. The transitions are declared in
//! this order on purpose: under the default tie-break, the first declared, a
//! car waiting while the light is green passes before the light changes.

use petritype::{
    BuildError, DrainEdge, Net, NetBuilder, PlaceRef, SnapshotEdge, TakeEdge, TransitionId,
};

/// The net, its places and its transitions.
pub struct TrafficLight {
    pub net: Net,
    /// The light: one token, its colour, `"red"`, `"green"` or `"amber"`.
    pub light: PlaceRef<String>,
    /// Cars waiting at the light, oldest first.
    pub waiting: PlaceRef<String>,
    /// Every car that has gone past the light, in the order they went.
    pub passed: PlaceRef<String>,
    /// `Turn Green`: the light from red to green, and everyone waiting goes.
    pub turn_green: TransitionId,
    /// `Pass on Green`: the front car goes, while the light is green.
    pub pass_on_green: TransitionId,
    /// `Change`: the light from green to amber, or from amber to red.
    pub change: TransitionId,
}

/// The colour after `colour`: red to green, green to amber, amber to red.
/// Anything else goes to red too, so a light given a colour it does not
/// know is set right by its next `Change`.
pub fn next_colour(colour: &str) -> &'static str {
    match colour {
        "red" => "green",
        "green" => "amber",
        _ => "red",
    }
}

/// The light's colour as the guards read it: the front token of `light`,
/// or `None` while the place is empty.
pub fn colour(net: &Net, light: PlaceRef<String>) -> Option<&str> {
    let (token, _) = net.tokens(light.id()).next()?;
    net.token::<String>(light.id(), token).map(String::as_str)
}

/// `Turn Green`'s body: the next colour, green, for `Light`, and every car
/// that waited at red, oldest first, for `Passed` — none when nobody was
/// waiting.
pub fn turn_green(light: String, cars: Vec<String>) -> (String, Vec<String>) {
    (next_colour(&light).to_owned(), cars)
}

/// `Pass on Green`'s body: the car goes; the light was only read.
pub fn pass_on_green(car: String, _light: String) -> (String,) {
    (car,)
}

/// `Change`'s body: the next colour, for `Light`.
pub fn change(light: String) -> (String,) {
    (next_colour(&light).to_owned(),)
}

/// The traffic light as §11 seeds it: `"red"` in `Light`, every other place
/// empty, every transition switched on, no parameters.
pub fn build_traffic_light() -> Result<TrafficLight, BuildError> {
    let mut b = NetBuilder::new();
    let light = b.place::<String>("Light");
    let waiting = b.place::<String>("Waiting");
    let passed = b.place::<String>("Passed");
    // Every place takes a token injected over the wire, cars into `Waiting`
    // above all.
    for place in [light, waiting, passed] {
        b.serializable(place);
    }
    b.seed(light, ["red".to_owned()]);

    let turn_green = b
        .transition(
            "Turn Green",
            (TakeEdge(light), DrainEdge(waiting)),
            (light, passed),
            turn_green,
        )
        .guard(move |net| colour(net, light) == Some("red"))
        .id();
    let pass_on_green = b
        .transition(
            "Pass on Green",
            (TakeEdge(waiting), SnapshotEdge(light)),
            (passed,),
            pass_on_green,
        )
        .guard(move |net| colour(net, light) == Some("green"))
        .id();
    let change = b
        .transition("Change", (TakeEdge(light),), (light,), change)
        .guard(move |net| colour(net, light).is_some_and(|colour| colour != "red"))
        .id();

    Ok(TrafficLight {
        net: b.build()?,
        light,
        waiting,
        passed,
        turn_green,
        pass_on_green,
        change,
    })
}
