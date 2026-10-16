//! [`Topology`]: the shape of a net, owned.

use crate::edge::{EdgeInfo, EdgeKind};
use crate::ids::{PlaceId, TransitionId};
use crate::keys::Filter;
use crate::net::PlaceInfo;
use crate::style::PlaceStyle;
use crate::transition::TransitionInfo;

/// The shape of a net at one moment, owned: its places, its transitions and
/// the edges between them, from [`Net::topology`](crate::Net::topology) or
/// [`Snapshot::topology`](crate::Snapshot::topology).
///
/// # What it is for
///
/// A render thread, or a runner that serves a net to a viewer, needs the
/// wiring to lay out a diagram, and needs it without holding a borrow of the
/// net the driver is writing to. This is that wiring, `Send + Sync + Clone`
/// and `'static`. With the `serde` feature it is `Serialize`, for logs and
/// tests.
///
/// # What it is not
///
/// - Not state. There are no token counts, no
///   [`enabled`](crate::TransitionInfo::enabled) switches and no
///   [`in_flight`](crate::TransitionInfo::in_flight) marks: those change as
///   the net runs, and live on the [`Net`](crate::Net) and on a
///   [`Snapshot`](crate::Snapshot). Putting a token into a place leaves the
///   topology equal to what it was.
/// - Not a snapshot. A [`Snapshot`](crate::Snapshot) is the marking and the
///   switches as well, and owns the tokens; a topology is a few small
///   vectors of names and ids.
/// - Not the wire protocol. It speaks in the core's ids, [`PlaceId`] and
///   [`TransitionId`]; a protocol keyed by names lives with whatever speaks
///   it, outside this crate.
///
/// The [`NetId`](crate::NetId) of the net is deliberately absent: it is
/// process-local and has no serialised form.
///
/// # Staleness
///
/// [`topology_counter`](Topology::topology_counter) is
/// [`Net::topology_counter`](crate::Net::topology_counter) at the moment the
/// copy was taken. A reader that compares the two knows whether its copy is
/// stale without comparing the copy itself.
///
/// # Ids and indices
///
/// `places[i].id` is `PlaceId(i)` and `transitions[i].id` is
/// `TransitionId(i)`: each vector is in id order and the index *is* the id.
/// The ids are kept explicitly anyway, so that a serialised form describes
/// itself.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[non_exhaustive]
pub struct Topology {
    /// Every place, in id order: [`Net::places`](crate::Net::places) without
    /// the counts.
    pub places: Vec<TopologyPlace>,
    /// Every transition, in id order:
    /// [`Net::transitions`](crate::Net::transitions) without the switches.
    pub transitions: Vec<TopologyTransition>,
    /// Every edge, in the order [`Net::edges`](crate::Net::edges) gives
    /// them.
    pub edges: Vec<EdgeInfo>,
    /// [`Net::topology_counter`](crate::Net::topology_counter) when this was
    /// taken.
    pub topology_counter: u64,
}

/// One place of a [`Topology`]: what [`PlaceInfo`] reports, owned, minus the
/// count.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[non_exhaustive]
pub struct TopologyPlace {
    /// The place's id, equal to its index in [`Topology::places`].
    pub id: PlaceId,
    /// The place's name, unique within the net.
    pub name: String,
    /// The Rust type of its tokens, `std::any::type_name::<T>()`.
    pub type_name: &'static str,
    /// [`PlaceInfo::serializable`]: whether the place was marked
    /// serializable, and so has a JSON view of its tokens.
    pub serializable: bool,
    /// [`PlaceInfo::nested`]: whether the place's tokens carry nets of their
    /// own. With `serde`, written only when true, so that the shape of a net
    /// without nesting is what it always was.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "std::ops::Not::not"))]
    pub nested: bool,
    /// [`PlaceInfo::passive`]: whether those inner nets are left to an outer
    /// body. Written only when true, like `nested`.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "std::ops::Not::not"))]
    pub passive: bool,
    /// [`PlaceInfo::key_names`]: the keys the place derives from its tokens,
    /// in declaration order, empty for none (DESIGN §8.3). With `serde`,
    /// written only when there are some, like `nested`.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Vec::is_empty"))]
    pub key_names: Vec<&'static str>,
    /// [`PlaceInfo::style`]: how the place itself is drawn. With `serde`,
    /// written only when there is one, as `{"fill": "token"}`.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub style: Option<PlaceStyle>,
}

/// One transition of a [`Topology`]: what [`TransitionInfo`] reports, owned,
/// minus the [`enabled`](TransitionInfo::enabled) and
/// [`in_flight`](TransitionInfo::in_flight) switches, which are state.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[non_exhaustive]
pub struct TopologyTransition {
    /// The transition's id, equal to its index in
    /// [`Topology::transitions`].
    pub id: TransitionId,
    /// The transition's name, unique within the net.
    pub name: String,
    /// Input places and how each is read, in declaration order:
    /// [`TransitionInfo::inputs`].
    pub inputs: Vec<(PlaceId, EdgeKind)>,
    /// The filter of each input edge, `None` for all but the keyed ones:
    /// [`TransitionInfo::filters`] (slice 19). With `serde`, written only
    /// when some input is keyed, so that the shape of a net without keyed
    /// edges is what it always was.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "no_filters"))]
    pub filters: Vec<Option<Filter>>,
    /// The parameter types it declared, by name:
    /// [`TransitionInfo::params`].
    pub params: Vec<&'static str>,
    /// Output places, in declaration order: [`TransitionInfo::outputs`].
    pub outputs: Vec<PlaceId>,
    /// Whether the body returns its outputs by position:
    /// [`TransitionInfo::positional`]. With `serde`, written only when
    /// false, so that the shape of a net of positional bodies is what it
    /// always was.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "is_true"))]
    pub positional: bool,
    /// Whether a firing can leave the net: [`TransitionInfo::detachable`].
    /// Derived from the edge kinds, so it is shape, not state.
    pub detachable: bool,
}

impl Topology {
    /// Builds a topology from a read API — the one constructor behind
    /// [`Net::topology`](crate::Net::topology) and
    /// [`Snapshot::topology`](crate::Snapshot::topology), so the two cannot
    /// disagree.
    pub(crate) fn from_read_api<'a>(
        places: impl Iterator<Item = (PlaceId, PlaceInfo<'a>)>,
        transitions: impl Iterator<Item = (TransitionId, TransitionInfo<'a>)>,
        edges: impl Iterator<Item = EdgeInfo>,
        topology_counter: u64,
    ) -> Topology {
        Topology {
            places: places
                .map(|(id, info)| TopologyPlace {
                    id,
                    name: info.name.to_owned(),
                    type_name: info.type_name,
                    serializable: info.serializable,
                    nested: info.nested,
                    passive: info.passive,
                    key_names: info.key_names.to_vec(),
                    style: info.style,
                })
                .collect(),
            transitions: transitions
                .map(|(id, info)| TopologyTransition {
                    id,
                    name: info.name.to_owned(),
                    inputs: info.inputs.to_vec(),
                    filters: info.filters.to_vec(),
                    params: info.params.to_vec(),
                    outputs: info.outputs.to_vec(),
                    positional: info.positional,
                    detachable: info.detachable,
                })
                .collect(),
            edges: edges.collect(),
            topology_counter,
        }
    }
}

/// For `skip_serializing_if`, which hands the field over by reference.
#[cfg(feature = "serde")]
fn is_true(value: &bool) -> bool {
    *value
}

/// For `skip_serializing_if`: no input of the transition is keyed.
#[cfg(feature = "serde")]
fn no_filters(filters: &[Option<Filter>]) -> bool {
    filters.iter().all(Option::is_none)
}
