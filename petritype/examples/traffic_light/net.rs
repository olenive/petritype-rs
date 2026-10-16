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
//! can `inject` into any of them. One token, `"light"`, stands in `Red`,
//! `Green` or `Amber`, and where it stands is the light's colour; it starts
//! in `Red`. Cars arrive in `Waiting` from outside, by `inject`, and go to
//! `Passed`.
//!
//! - `Turn Green` takes the light from `Red` and *drains* `Waiting` —
//!   every car waiting, zero or more, all consumed — and writes the light to
//!   `Green` and the cars, oldest first, to `Passed`. With nobody waiting it
//!   still fires: [`DrainEdge`] does not require presence, so the light does
//!   not wait for a car.
//! - `Pass on Green` takes the front car of `Waiting` and *reads* `Green`
//!   ([`SnapshotEdge`]: the light is cloned, not taken), and writes the car
//!   to `Passed`. A snapshot needs its place non-empty, so a car goes only
//!   while the light is green; one that arrives at amber or red waits for
//!   the next `Turn Green`.
//! - `Turn Amber` and `Turn Red` move the light on.
//!
//! The light cycles by itself: some light transition is always fireable, so
//! the net never quiesces, as a real junction does not. Every transition
//! takes a token, so none is a source; the light's loop is a cycle, which
//! `assert_acyclic` would refuse, so this net does not ask for it. The
//! transitions are declared in this order on purpose: under the default
//! tie-break, the first declared, a car waiting while the light is green
//! passes before the light turns amber.

use petritype::{
    BuildError, DrainEdge, Net, NetBuilder, PlaceRef, SnapshotEdge, TakeEdge, TransitionId,
};

/// The net, its places and its transitions.
pub struct TrafficLight {
    pub net: Net,
    /// The light, when it is red.
    pub red: PlaceRef<String>,
    /// The light, when it is green.
    pub green: PlaceRef<String>,
    /// The light, when it is amber.
    pub amber: PlaceRef<String>,
    /// Cars waiting at the light, oldest first.
    pub waiting: PlaceRef<String>,
    /// Every car that has gone past the light, in the order they went.
    pub passed: PlaceRef<String>,
    /// `Turn Green`: the light from red to green, and everyone waiting goes.
    pub turn_green: TransitionId,
    /// `Pass on Green`: the front car goes, while the light is green.
    pub pass_on_green: TransitionId,
    /// `Turn Amber`: the light from green to amber.
    pub turn_amber: TransitionId,
    /// `Turn Red`: the light from amber to red.
    pub turn_red: TransitionId,
}

/// `Turn Green`'s body: the light, for `Green`, and every car that waited
/// at red, oldest first, for `Passed` — none when nobody was waiting.
pub fn turn_green(light: String, waiting: Vec<String>) -> (String, Vec<String>) {
    (light, waiting)
}

/// `Pass on Green`'s body: the car goes; the light was only read.
pub fn pass_on_green(car: String, _light: String) -> (String,) {
    (car,)
}

/// `Turn Amber`'s and `Turn Red`'s body: the light moves on unchanged.
pub fn change(light: String) -> (String,) {
    (light,)
}

/// The traffic light as §11 seeds it: the light in `Red`, every other place
/// empty, every transition switched on, no parameters.
pub fn build_traffic_light() -> Result<TrafficLight, BuildError> {
    let mut b = NetBuilder::new();
    let red = b.place::<String>("Red");
    let green = b.place::<String>("Green");
    let amber = b.place::<String>("Amber");
    let waiting = b.place::<String>("Waiting");
    let passed = b.place::<String>("Passed");
    // Every place takes a token injected over the wire, cars into `Waiting`
    // above all.
    for place in [red, green, amber, waiting, passed] {
        b.serializable(place);
    }
    b.seed(red, ["light".to_owned()]);

    let turn_green = b
        .transition(
            "Turn Green",
            (TakeEdge(red), DrainEdge(waiting)),
            (green, passed),
            turn_green,
        )
        .id();
    let pass_on_green = b
        .transition(
            "Pass on Green",
            (TakeEdge(waiting), SnapshotEdge(green)),
            (passed,),
            pass_on_green,
        )
        .id();
    let turn_amber = b
        .transition("Turn Amber", (TakeEdge(green),), (amber,), change)
        .id();
    let turn_red = b
        .transition("Turn Red", (TakeEdge(amber),), (red,), change)
        .id();

    Ok(TrafficLight {
        net: b.build()?,
        red,
        green,
        amber,
        waiting,
        passed,
        turn_green,
        pass_on_green,
        turn_amber,
        turn_red,
    })
}
