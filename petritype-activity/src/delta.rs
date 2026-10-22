//! [`Delta`]: what the activity model applied over a stretch of time, as
//! counts per node, for a recorder to bin and a replay to apply again.

use std::collections::BTreeMap;

use petritype::{PlaceId, TransitionId};

/// What an [`Activity`](crate::Activity) applied since its account was last
/// taken: how many firings, whether the staleness dim was lifted, and per
/// node the hits it took and the state it was left in.
///
/// The model keeps this account as it goes
/// ([`take_delta`](crate::Activity::take_delta)), so its rules are written
/// once: one ember hit per event, a pulse hit per token, a launch's flow
/// held to its distribute, the distribute's smaller hit. A replay applies a
/// delta with [`apply_delta`](crate::Activity::apply_delta) under its own
/// settings. A node appears when it was touched, or, for a transition, while
/// it was in flight; an empty delta is `Delta::default()` and costs almost
/// nothing.
///
/// With the `serde` feature it is `Serialize` and `Deserialize`, the nodes
/// keyed by their ids and the fields at zero left out.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct Delta {
    /// How many fires and launches were seen: how far the firings clock
    /// moved.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "is_zero"))]
    pub firings: u32,
    /// Whether a fire, launch, distribute or failure lifted the staleness
    /// dim.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "is_false"))]
    pub lifted: bool,
    /// Every place touched, by id.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "BTreeMap::is_empty"))]
    pub places: BTreeMap<PlaceId, PlaceDelta>,
    /// Every transition touched or in flight, by id.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "BTreeMap::is_empty"))]
    pub transitions: BTreeMap<TransitionId, TransitionDelta>,
}

/// One place's part of a [`Delta`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct PlaceDelta {
    /// Pulse hits: one per token taken, added or changed in place.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "is_zero"))]
    pub pulse_hits: u32,
    /// Ember hits: one per event that touched the place.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "is_zero"))]
    pub ember_hits: u32,
    /// The tokens the flow counted, added less taken, as the model applied
    /// them: a launch's take shows here when its distribute lands, and a
    /// token changed in place not at all.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "is_zero_i64"))]
    pub balance: i64,
    /// The standing count at the end of the delta.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "is_zero_usize"))]
    pub count: usize,
}

/// One transition's part of a [`Delta`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct TransitionDelta {
    /// Full hits: one per fire or launch, and one for a failure with no
    /// launch before it, which is a firing of its own.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "is_zero"))]
    pub fires: u32,
    /// Distributes, each a hit of
    /// [`distribute_hit`](crate::ActivitySettings::distribute_hit) times the
    /// jump.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "is_zero"))]
    pub distributes: u32,
    /// Failures, each setting the failure level to 1.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "is_zero"))]
    pub failures: u32,
    /// The wall time spent in flight, in milliseconds, as
    /// [`tick`](crate::Activity::tick) counted it.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "is_zero_f64"))]
    pub in_flight_ms: f64,
    /// Whether the transition was in flight at the end of the delta.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "is_false"))]
    pub in_flight: bool,
}

impl Delta {
    /// Adds a `later` delta to this one, as if the two had been one: counts,
    /// balances and time in flight add, each node's count and in-flight
    /// state take the later value, `lifted` is either's and the firings
    /// add. A recorder merges a frame into its open bin this way, and
    /// [`coarsen`](crate::Record::coarsen) merges bins.
    pub fn absorb(&mut self, later: &Delta) {
        self.firings = self.firings.saturating_add(later.firings);
        self.lifted |= later.lifted;
        for (&place, l) in &later.places {
            let e = self.places.entry(place).or_default();
            e.pulse_hits = e.pulse_hits.saturating_add(l.pulse_hits);
            e.ember_hits = e.ember_hits.saturating_add(l.ember_hits);
            e.balance = e.balance.saturating_add(l.balance);
            e.count = l.count;
        }
        for (&transition, l) in &later.transitions {
            let e = self.transitions.entry(transition).or_default();
            e.fires = e.fires.saturating_add(l.fires);
            e.distributes = e.distributes.saturating_add(l.distributes);
            e.failures = e.failures.saturating_add(l.failures);
            e.in_flight_ms += l.in_flight_ms;
            e.in_flight = l.in_flight;
        }
    }

    /// Whether nothing happened: no firing, no lift and no node touched.
    pub fn is_empty(&self) -> bool {
        self.firings == 0 && !self.lifted && self.places.is_empty() && self.transitions.is_empty()
    }

    /// Gives `later` the standing state this delta ends in for every node
    /// `later` does not mention: each place's count and each transition's
    /// in-flight state, with no hit. A ring that drops its oldest bin
    /// carries its state into the next this way, so that a replay of what
    /// is kept starts from the counts that stood.
    pub(crate) fn carry_into(&self, later: &mut Delta) {
        for (&place, e) in &self.places {
            later.places.entry(place).or_insert(PlaceDelta {
                count: e.count,
                ..PlaceDelta::default()
            });
        }
        for (&transition, e) in &self.transitions {
            later
                .transitions
                .entry(transition)
                .or_insert(TransitionDelta {
                    in_flight: e.in_flight,
                    ..TransitionDelta::default()
                });
        }
    }
}

#[cfg(feature = "serde")]
fn is_zero(n: &u32) -> bool {
    *n == 0
}

#[cfg(feature = "serde")]
fn is_zero_usize(n: &usize) -> bool {
    *n == 0
}

#[cfg(feature = "serde")]
fn is_zero_i64(n: &i64) -> bool {
    *n == 0
}

#[cfg(feature = "serde")]
fn is_zero_f64(n: &f64) -> bool {
    *n == 0.0
}

#[cfg(feature = "serde")]
fn is_false(b: &bool) -> bool {
    !*b
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: PlaceId = PlaceId::new(0);
    const Q: PlaceId = PlaceId::new(1);
    const T: TransitionId = TransitionId::new(0);

    #[test]
    fn absorbing_adds_the_counts_and_keeps_the_later_state() {
        let mut first = Delta {
            firings: 1,
            ..Delta::default()
        };
        first.places.insert(
            P,
            PlaceDelta {
                pulse_hits: 2,
                ember_hits: 1,
                balance: -2,
                count: 3,
            },
        );
        first.transitions.insert(
            T,
            TransitionDelta {
                fires: 1,
                in_flight_ms: 40.0,
                in_flight: true,
                ..TransitionDelta::default()
            },
        );
        let mut later = Delta {
            firings: 2,
            lifted: true,
            ..Delta::default()
        };
        later.places.insert(
            P,
            PlaceDelta {
                pulse_hits: 1,
                ember_hits: 1,
                balance: 1,
                count: 4,
            },
        );
        later.places.insert(
            Q,
            PlaceDelta {
                count: 1,
                ..PlaceDelta::default()
            },
        );
        later.transitions.insert(
            T,
            TransitionDelta {
                distributes: 1,
                failures: 1,
                in_flight_ms: 10.0,
                in_flight: false,
                ..TransitionDelta::default()
            },
        );
        first.absorb(&later);
        assert_eq!((first.firings, first.lifted), (3, true));
        assert_eq!(
            first.places[&P],
            PlaceDelta {
                pulse_hits: 3,
                ember_hits: 2,
                balance: -1,
                count: 4,
            }
        );
        assert_eq!(first.places[&Q].count, 1);
        assert_eq!(
            first.transitions[&T],
            TransitionDelta {
                fires: 1,
                distributes: 1,
                failures: 1,
                in_flight_ms: 50.0,
                in_flight: false,
            }
        );
        assert!(Delta::default().is_empty());
        assert!(!first.is_empty());
    }

    #[test]
    fn carrying_gives_the_standing_state_and_no_hit() {
        let mut old = Delta::default();
        old.places.insert(
            P,
            PlaceDelta {
                pulse_hits: 5,
                ember_hits: 1,
                balance: 2,
                count: 2,
            },
        );
        old.places.insert(
            Q,
            PlaceDelta {
                count: 7,
                ..PlaceDelta::default()
            },
        );
        old.transitions.insert(
            T,
            TransitionDelta {
                fires: 1,
                in_flight: true,
                ..TransitionDelta::default()
            },
        );
        let mut next = Delta::default();
        next.places.insert(
            Q,
            PlaceDelta {
                pulse_hits: 1,
                ember_hits: 1,
                balance: 1,
                count: 8,
            },
        );
        old.carry_into(&mut next);
        assert_eq!(
            next.places[&P],
            PlaceDelta {
                count: 2,
                ..PlaceDelta::default()
            }
        );
        assert_eq!(next.places[&Q].count, 8, "the later count stands");
        assert_eq!(
            next.transitions[&T],
            TransitionDelta {
                in_flight: true,
                ..TransitionDelta::default()
            }
        );
        assert_eq!(next.firings, 0);
    }
}
