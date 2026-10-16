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
//! ([`next_colour`]). Two transitions, one that moves the cars and one that
//! changes the light, declared in this order:
//!
//! - `Go`, guarded on green and someone waiting, *drains* `Waiting` — every
//!   car waiting, oldest first, all consumed — and *reads* `Light`
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
    /// `Go`: everyone waiting goes, while the light is green.
    pub go: TransitionId,
    /// `Change`: the light to its next colour.
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

/// The light's colour as `Go`'s guard reads it: the front token of
/// `light`, or `None` while the place is empty.
pub fn colour(net: &Net, light: PlaceRef<String>) -> Option<&str> {
    let (token, _) = net.tokens(light.id()).next()?;
    net.token::<String>(light.id(), token).map(String::as_str)
}

/// `Go`'s body: every car that waited, oldest first, for `Passed`; the
/// light was only read.
pub fn go(cars: Vec<String>, _light: String) -> (Vec<String>,) {
    (cars,)
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

    let go = b
        .transition(
            "Go",
            (DrainEdge(waiting), SnapshotEdge(light)),
            (passed,),
            go,
        )
        .guard(move |net| colour(net, light) == Some("green") && net.count(waiting.id()) > 0)
        .id();
    let change = b
        .transition("Change", (TakeEdge(light),), (light,), change)
        .id();

    Ok(TrafficLight {
        net: b.build()?,
        light,
        waiting,
        passed,
        go,
        change,
    })
}
