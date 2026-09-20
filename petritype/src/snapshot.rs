//! [`Net::snapshot`]: a consistent, owned copy of everything a reader can see.

use std::any::Any;
use std::fmt;

use crate::error::NetError;
use crate::ids::{NetId, PlaceId, TokenId, TransitionId};
#[cfg(feature = "serde")]
use crate::net::json_in;
use crate::net::{
    Net, PlaceInfo, count_in, label_in, place_named_in, places_of, token_in, tokens_in,
};
use crate::param::{ParamSlot, param_of};
use crate::place::PlaceSlot;
use crate::transition::TransitionInfo;

impl Net {
    /// A consistent, owned copy of everything a reader can see: the marking,
    /// the switches, the in-flight set, the parameters and the counters
    /// (DESIGN §8.2).
    ///
    /// Taking one changes nothing: no event is emitted and no counter moves,
    /// including [`state_counter`](Net::state_counter) — reading is not a
    /// change.
    ///
    /// ```
    /// # use petritype::{NetBuilder, TakeEdge};
    /// # let mut b = NetBuilder::new();
    /// # let inbox = b.place::<u32>("Inbox");
    /// # let out = b.place::<u32>("Out");
    /// # b.seed(inbox, [21]);
    /// # b.transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,));
    /// # let mut net = b.build()?;
    /// let before = net.snapshot()?;
    /// net.step()?;
    ///
    /// assert_eq!(before.count(inbox.id()), 1, "the copy did not move with the net");
    /// assert_eq!(net.count(inbox.id()), 0);
    /// assert_eq!(before.fired_count(), 0);
    /// assert_eq!(net.fired_count(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// [`NetError::NoClonePlace`] if any place was declared with
    /// [`place_no_clone`](crate::NetBuilder::place_no_clone): its tokens
    /// cannot be copied, and half a state is no state. Nothing is cloned
    /// before every place has been checked, so a refusal costs nothing.
    pub fn snapshot(&self) -> Result<Snapshot, NetError> {
        // Checked first, all of it, so that a net with a `no_clone` place in
        // the middle does not pay for copying the places before it.
        if let Some((index, slot)) = self
            .places
            .iter()
            .enumerate()
            .find(|(_, slot)| slot.meta.clone.is_none())
        {
            return Err(NetError::NoClonePlace {
                place: PlaceId(index as u32),
                name: slot.meta.name.clone(),
            });
        }

        let places = self
            .places
            .iter()
            .map(|slot| {
                slot.try_clone()
                    .expect("every place was just checked for a cloner")
            })
            .collect();
        let transitions = self
            .transitions
            .iter()
            .map(|slot| SnapshotTransition {
                name: slot.name.clone(),
                inputs: slot.inputs.clone(),
                params: slot.params.clone(),
                outputs: slot.outputs.clone(),
                enabled: slot.enabled,
                // An empty body slot is the in-flight mark, here as in the
                // net: what the snapshot keeps is the answer, since the body
                // itself is exactly what cannot be copied.
                in_flight: slot.body.is_none(),
                detachable: slot.detachable,
            })
            .collect();

        Ok(Snapshot {
            id: self.id,
            places,
            transitions,
            params: self.params.clone(),
            label_limit: self.label_limit,
            topology_counter: self.topology_counter,
            state_counter: self.state_counter,
            fired_count: self.fired_count,
        })
    }
}

/// One transition inside a [`Snapshot`]: everything
/// [`TransitionInfo`] reports, owned.
///
/// The topology comes along with the state — it is a few small vectors, and a
/// renderer holding a snapshot can then draw the whole net from it rather
/// than needing the `Net` for the wiring and the snapshot for the marking.
struct SnapshotTransition {
    name: String,
    inputs: Vec<(PlaceId, crate::EdgeKind)>,
    params: Vec<&'static str>,
    outputs: Vec<PlaceId>,
    enabled: bool,
    in_flight: bool,
    detachable: bool,
}

/// The state of a net at one moment, owned and independent of it.
///
/// # Why it exists
///
/// A driver holds `&mut Net` and is the only writer. Anything else that wants
/// to look at the net — a renderer on another thread, an inspector, a test
/// comparing one moment with another — either borrows the net and blocks the
/// driver, or takes a copy. This is the copy: `Send + Sync + Clone`, owning
/// its tokens and its parameter values, answering the same questions as
/// [`Net`] in the same words ([`count`](Snapshot::count),
/// [`tokens`](Snapshot::tokens), [`label`](Snapshot::label),
/// [`param`](Snapshot::param), the counters) and going stale the moment the
/// net moves on — which is the point: it is a *moment*, and the counters on
/// it say which one.
///
/// # Why it is not a `Net`
///
/// DESIGN §8.2 wrote `snapshot() -> Result<Net, NetError>`, a deep copy of
/// the net. That cannot be built. A transition's body is a
/// `Box<dyn FnMut ..>`, which has no `Clone`, and the body of a transition
/// that is [in flight](Net::in_flight) is not in the net at all — it is
/// travelling with a [`Firing`](crate::Firing) somewhere else. So a snapshot
/// is the net's *state*, not the net. A what-if copy that can be run is a
/// second `build()` from the builder that declared the first.
///
/// # What it holds
///
/// The marking with the tokens' ids and their places' metadata, so labels and
/// JSON read exactly as they do on the net; each transition's name, wiring,
/// switch, in-flight mark, detachability and declared parameter types; the
/// parameter *values*, cloned; the label limit; and the three counters.
///
/// What is **not** here is the fireable set. Fireability depends on guards,
/// which are closures over the live net (`Fn(&Net) -> bool`): they cannot be
/// cloned and cannot be asked a question about a snapshot. Ask
/// [`Net::fireable`](Net::fireable) of the net itself. For the same reason
/// there is no `step`, no `priority_of` and no selector here: a snapshot is
/// something to read, not something to run.
///
/// `Debug` prints place names with counts and transition names — never a
/// token's contents in bulk, and never a parameter value, which has no
/// `Debug` bound at all (DESIGN §10).
pub struct Snapshot {
    id: NetId,
    places: Vec<PlaceSlot>,
    transitions: Vec<SnapshotTransition>,
    params: Vec<ParamSlot>,
    label_limit: usize,
    topology_counter: u64,
    state_counter: u64,
    fired_count: u64,
}

impl Snapshot {
    /// The id of the net this is a snapshot of, as [`Net::id`] reports it.
    ///
    /// Carried so that a reader holding snapshots of two nets can tell them
    /// apart, and so that a [`PlaceId`] from a snapshot is known to mean a
    /// place of that net.
    pub fn id(&self) -> NetId {
        self.id
    }

    /// Every place, in id order, with its name, token type and token count as
    /// they were: [`Net::places`].
    pub fn places(&self) -> impl Iterator<Item = (PlaceId, PlaceInfo<'_>)> {
        places_of(&self.places)
    }

    /// Every transition, in declaration order: [`Net::transitions`].
    ///
    /// The same [`TransitionInfo`] the net hands out, down to
    /// [`in_flight`](TransitionInfo::in_flight), which is what was airborne
    /// when the snapshot was taken.
    pub fn transitions(&self) -> impl Iterator<Item = (TransitionId, TransitionInfo<'_>)> {
        self.transitions.iter().enumerate().map(|(index, slot)| {
            // The index is where the id came from in the first place.
            let id = TransitionId(index as u32);
            let info = TransitionInfo {
                name: &slot.name,
                inputs: &slot.inputs,
                params: &slot.params,
                outputs: &slot.outputs,
                enabled: slot.enabled,
                in_flight: slot.in_flight,
                detachable: slot.detachable,
            };
            (id, info)
        })
    }

    /// The id of the place with this name: [`Net::place_named`].
    pub fn place_named(&self, name: &str) -> Option<PlaceId> {
        place_named_in(&self.places, name)
    }

    /// The id of the transition with this name: [`Net::transition_named`].
    pub fn transition_named(&self, name: &str) -> Option<TransitionId> {
        self.transitions
            .iter()
            .position(|slot| slot.name == name)
            .map(|index| TransitionId(index as u32))
    }

    /// How many tokens a place held: [`Net::count`].
    pub fn count(&self, place: PlaceId) -> usize {
        count_in(&self.places, place)
    }

    /// The tokens of a place, oldest first: [`Net::tokens`].
    pub fn tokens(&self, place: PlaceId) -> impl Iterator<Item = (TokenId, &dyn Any)> {
        tokens_in(&self.places, place)
    }

    /// One token of a place, downcast to `T`: [`Net::token`].
    pub fn token<T: 'static>(&self, place: PlaceId, token: TokenId) -> Option<&T> {
        token_in::<T>(&self.places, place, token)
    }

    /// One token rendered with its type's `Debug`: [`Net::label`].
    ///
    /// Truncated to the net's own
    /// [`label_limit`](Net::label_limit), which the snapshot carried along so
    /// that a label reads the same either side of the copy.
    pub fn label(&self, place: PlaceId, token: TokenId) -> Option<String> {
        label_in(&self.places, place, token, self.label_limit)
    }

    /// One token as JSON: [`Net::json`], for a place marked
    /// [`serializable`](crate::NetBuilder::serializable).
    #[cfg(feature = "serde")]
    pub fn json(&self, place: PlaceId, token: TokenId) -> Option<serde_json::Value> {
        json_in(&self.places, place, token)
    }

    /// The maximum number of characters [`label`](Snapshot::label) returns:
    /// [`Net::label_limit`].
    pub fn label_limit(&self) -> usize {
        self.label_limit
    }

    /// Whether a transition's switch was on: [`Net::is_enabled`]. An unknown
    /// id is not enabled.
    pub fn is_enabled(&self, transition: TransitionId) -> bool {
        self.transitions
            .get(transition.0 as usize)
            .is_some_and(|slot| slot.enabled)
    }

    /// Whether a transition's body was out of its slot:
    /// [`Net::is_in_flight`]. An unknown id is not in flight.
    pub fn is_in_flight(&self, transition: TransitionId) -> bool {
        self.transitions
            .get(transition.0 as usize)
            .is_some_and(|slot| slot.in_flight)
    }

    /// Every transition that was in flight, in declaration order:
    /// [`Net::in_flight`].
    pub fn in_flight(&self) -> Vec<TransitionId> {
        self.transitions
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.in_flight)
            .map(|(index, _)| TransitionId(index as u32))
            .collect()
    }

    /// The value the parameter of type `T` had: [`Net::param`].
    ///
    /// The snapshot owns its own copy, so a later
    /// [`set_param`](Net::set_param) on the net does not reach it.
    pub fn param<T: 'static>(&self) -> Option<&T> {
        param_of::<T>(&self.params)
    }

    /// The net's [`topology_counter`](Net::topology_counter) at that moment.
    pub fn topology_counter(&self) -> u64 {
        self.topology_counter
    }

    /// The net's [`state_counter`](Net::state_counter) at that moment — which
    /// is what says *which* moment this is, and what a view compares against
    /// to know whether it needs to redraw.
    pub fn state_counter(&self) -> u64 {
        self.state_counter
    }

    /// The net's [`fired_count`](Net::fired_count) at that moment.
    pub fn fired_count(&self) -> u64 {
        self.fired_count
    }
}

/// Cloning re-clones every token through its place's recorded cloner, and
/// every parameter through its own: the copy is as independent of this
/// snapshot as this snapshot is of the net.
impl Clone for Snapshot {
    fn clone(&self) -> Self {
        Snapshot {
            id: self.id,
            places: self
                .places
                .iter()
                .map(|slot| {
                    slot.try_clone()
                        .expect("a snapshot's places all cloned once already")
                })
                .collect(),
            transitions: self
                .transitions
                .iter()
                .map(|slot| SnapshotTransition {
                    name: slot.name.clone(),
                    inputs: slot.inputs.clone(),
                    params: slot.params.clone(),
                    outputs: slot.outputs.clone(),
                    enabled: slot.enabled,
                    in_flight: slot.in_flight,
                    detachable: slot.detachable,
                })
                .collect(),
            params: self.params.clone(),
            label_limit: self.label_limit,
            topology_counter: self.topology_counter,
            state_counter: self.state_counter,
            fired_count: self.fired_count,
        }
    }
}

impl fmt::Debug for Snapshot {
    /// Place names with their counts, transition names, parameter type names
    /// and the counters. Never a token's contents — there may be thousands —
    /// and never a parameter's value, which has no `Debug` to print.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Snapshot")
            .field("id", &self.id)
            .field(
                "places",
                &self
                    .places
                    .iter()
                    .map(|slot| (slot.meta.name.as_str(), slot.tokens.len()))
                    .collect::<Vec<_>>(),
            )
            .field(
                "transitions",
                &self
                    .transitions
                    .iter()
                    .map(|slot| slot.name.as_str())
                    .collect::<Vec<_>>(),
            )
            .field("params", &self.params)
            .field("state_counter", &self.state_counter)
            .field("topology_counter", &self.topology_counter)
            .field("fired_count", &self.fired_count)
            .finish()
    }
}

impl fmt::Debug for SnapshotTransition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SnapshotTransition")
            .field("name", &self.name)
            .field("inputs", &self.inputs)
            .field("params", &self.params)
            .field("outputs", &self.outputs)
            .field("enabled", &self.enabled)
            .field("in_flight", &self.in_flight)
            .field("detachable", &self.detachable)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use crate::{NetBuilder, Snapshot, TakeEdge};

    /// The whole point of the type: it can be handed to another thread and
    /// shared there.
    #[test]
    fn a_snapshot_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Snapshot>();
    }

    #[test]
    fn a_snapshot_is_independent_of_the_net_it_came_from() {
        let mut b = NetBuilder::new();
        let inbox = b.place::<u32>("Inbox");
        let out = b.place::<u32>("Out");
        b.seed(inbox, [1]);
        b.transition("Copy", (TakeEdge(inbox),), (out,), |n: u32| (n,));
        let mut net = b.build().expect("a one-transition net");

        let taken = net.snapshot().expect("every place can clone");
        net.put(inbox, 2);

        assert_eq!(taken.count(inbox.id()), 1);
        assert_eq!(net.count(inbox.id()), 2);
        assert_eq!(taken.state_counter(), 0);
        assert_eq!(net.state_counter(), 1);
    }
}
