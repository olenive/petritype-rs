//! What a layout is given and what it gives back: the nodes, their kinds
//! and groups, and where it put them.

use petritype::{PlaceId, PlaceInfo, TransitionId, TransitionInfo};

use crate::{Bounds, Point};

/// Which way the ranks grow.
///
/// With the `serde` feature it is written in snake case, `"top_down"` or
/// `"left_right"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum Direction {
    /// Rank 0 at the top, growing downward. It is the default, because a
    /// pipeline read top to bottom fits a landscape window as a single column.
    #[default]
    TopDown,
    /// Rank 0 on the left, growing rightward.
    LeftRight,
}

/// One node of the layout graph.
///
/// With the `serde` feature it is written as its kind in snake case and its
/// id as a bare number, `{"place": 3}` or `{"transition": 0}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum LayoutNode {
    /// A place.
    Place(PlaceId),
    /// A transition.
    Transition(TransitionId),
}

/// Where the layout put one node.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// The node's rank: 0 for a source, otherwise one more than the highest
    /// rank among its forward predecessors.
    pub rank: usize,
    /// Its position within the rank, from 0.
    pub order: usize,
    /// Its centre, in world coordinates.
    pub position: Point,
}

/// Which places a layout ghosts: draws their edges to a small copy of the
/// place beside each transition instead of across the drawing.
///
/// A ghosted place keeps one home node, which holds its tokens and shows its
/// count, but no edge is drawn to it. Each transition it is joined to gets a
/// ghost of it instead, an input ghost on the side the ranks come from for an
/// input edge and an output ghost on the other side for an output edge, and
/// the edge runs between the ghost and the transition. Ghosts and their edges
/// play no part in ranking and ordering, so a hub joined to half the net no
/// longer pulls it out of shape. Nothing is ghosted by default.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Ghosting {
    /// The names of the places to ghost.
    pub names: Vec<String>,
    /// Ghost every place with at least this many edges, counting inputs and
    /// outputs, whatever its name. `None` ghosts by name only.
    pub min_degree: Option<usize>,
}

impl Ghosting {
    /// Ghosts the places with these names.
    pub fn named<S: Into<String>>(names: impl IntoIterator<Item = S>) -> Self {
        Self {
            names: names.into_iter().map(Into::into).collect(),
            min_degree: None,
        }
    }

    /// The same, and every place with at least `degree` edges too.
    pub fn with_min_degree(mut self, degree: usize) -> Self {
        self.min_degree = Some(degree);
        self
    }

    /// Whether a place called `name` with `degree` edges is ghosted.
    pub fn applies(&self, name: &str, degree: usize) -> bool {
        self.names.iter().any(|n| n == name) || self.min_degree.is_some_and(|min| degree >= min)
    }
}

/// Which side of its transition a ghost sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GhostSide {
    /// The side the ranks come from: the ghost of a place the transition
    /// reads.
    Input,
    /// The side the ranks go to: the ghost of a place the transition writes.
    Output,
}

/// What kind of place a [`LayoutPlace`] is, which decides how it is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PlaceKind {
    /// A place that holds tokens, drawn as a circle: the default.
    #[default]
    Ordinary,
    /// A [subnet place](petritype::NetBuilder::subnet_place): one node of the
    /// layout, drawn closed as a box that holds the circles of its ports.
    Subnet,
    /// A port of the subnet place with this id, as the net around the subnet
    /// sees it: a circle inside that place's box, where the edges on the port
    /// end. A port whose subnet place is not among the places given is laid
    /// out as an ordinary place.
    Port(PlaceId),
}

/// One place handed to the layout: its id, its name, its [`PlaceKind`] and
/// its group.
///
/// [`compute_named`](crate::Layout::compute_named) and
/// [`compute_subgraph`](crate::Layout::compute_subgraph) take anything that
/// turns into one, so a plain `(PlaceId, String)` is still an ordinary place
/// in no group, and `(PlaceId, PlaceInfo)` from
/// [`Net::places`](petritype::Net::places) carries the kind and the group
/// the net says. The ports of a subnet place are its places of kind
/// [`PlaceKind::Port`], in the order they are given, which should be the
/// order the net declares them.
///
/// ```
/// use petritype::PlaceId;
/// use petritype_layout::{LayoutPlace, PlaceKind};
///
/// let level = PlaceId::new(1);
/// let places = [
///     LayoutPlace::from((PlaceId::new(0), "Door".to_owned())).in_group("Dungeon"),
///     LayoutPlace::new(level, "Level 3", PlaceKind::Subnet).in_group("Dungeon"),
///     LayoutPlace::new(PlaceId::new(2), "Lobby", PlaceKind::Port(level)),
/// ];
/// assert_eq!(places[0].kind, PlaceKind::Ordinary);
/// assert_eq!(places[1].group.as_deref(), Some("Dungeon"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LayoutPlace {
    /// The place, as the edges name it.
    pub id: PlaceId,
    /// Its name: for a port, the port's own name, which need be unique only
    /// among its subnet place's ports.
    pub name: String,
    /// What kind of place it is.
    pub kind: PlaceKind,
    /// The name of the [group](petritype::NetBuilder::group) it is in, or
    /// `None` for a place in no group. A port drawn in its subnet place's
    /// box goes where the box goes, so its own is not read, and the net
    /// gives it none.
    pub group: Option<String>,
}

impl LayoutPlace {
    /// A place of this kind, in no group.
    pub fn new(id: PlaceId, name: impl Into<String>, kind: PlaceKind) -> Self {
        Self {
            id,
            name: name.into(),
            kind,
            group: None,
        }
    }

    /// The same place, in the group of this name.
    pub fn in_group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }
}

/// An ordinary place in no group.
impl From<(PlaceId, String)> for LayoutPlace {
    fn from((id, name): (PlaceId, String)) -> Self {
        Self::new(id, name, PlaceKind::Ordinary)
    }
}

/// A place as the net describes it: a subnet place, a port of one, or an
/// ordinary place, in the group the net puts it in.
impl From<(PlaceId, PlaceInfo<'_>)> for LayoutPlace {
    fn from((id, info): (PlaceId, PlaceInfo<'_>)) -> Self {
        let kind = match (info.subnet, info.port_of) {
            (true, _) => PlaceKind::Subnet,
            (false, Some(subnet)) => PlaceKind::Port(subnet),
            (false, None) => PlaceKind::Ordinary,
        };
        Self {
            group: info.group.map(str::to_owned),
            ..Self::new(id, info.name, kind)
        }
    }
}

/// One transition handed to the layout: its id, its name and its group.
///
/// [`compute_named`](crate::Layout::compute_named) and
/// [`compute_subgraph`](crate::Layout::compute_subgraph) take anything that
/// turns into one, so a plain `(TransitionId, String)` is a transition in no
/// group, and `(TransitionId, TransitionInfo)` from
/// [`Net::transitions`](petritype::Net::transitions) carries the group the
/// net says.
///
/// ```
/// use petritype::TransitionId;
/// use petritype_layout::LayoutTransition;
///
/// let fund = LayoutTransition::new(TransitionId::new(0), "Fund Walk").in_group("Walk");
/// let plain = LayoutTransition::from((TransitionId::new(1), "Rest".to_owned()));
/// assert_eq!(fund.group.as_deref(), Some("Walk"));
/// assert_eq!(plain.group, None);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LayoutTransition {
    /// The transition, as the edges name it.
    pub id: TransitionId,
    /// Its name.
    pub name: String,
    /// The name of the [group](petritype::NetBuilder::group) it is in, or
    /// `None` for a transition in no group.
    pub group: Option<String>,
}

impl LayoutTransition {
    /// A transition in no group.
    pub fn new(id: TransitionId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            group: None,
        }
    }

    /// The same transition, in the group of this name.
    pub fn in_group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }
}

/// A transition in no group.
impl From<(TransitionId, String)> for LayoutTransition {
    fn from((id, name): (TransitionId, String)) -> Self {
        Self::new(id, name)
    }
}

/// A transition as the net describes it, in the group the net puts it in.
impl From<(TransitionId, TransitionInfo<'_>)> for LayoutTransition {
    fn from((id, info): (TransitionId, TransitionInfo<'_>)) -> Self {
        Self {
            group: info.group.map(str::to_owned),
            ..Self::new(id, info.name)
        }
    }
}

/// Where the layout put a closed subnet place: its box, the room it left
/// inside for the title and the count, and the circle of each port.
///
/// The box is drawn with rounded corners of
/// [`corner_radius`](SubnetBox::corner_radius); nothing inside reaches them.
#[derive(Debug, Clone, PartialEq)]
pub struct SubnetBox {
    /// The centre of the box, which is also what
    /// [`Layout::position_of_place`](crate::Layout::position_of_place)
    /// answers for the subnet place.
    pub centre: Point,
    /// Its width and height.
    pub size: Point,
    /// The radius of its rounded corners.
    pub corner_radius: f32,
    /// The room left for the place's name, centred across the box; its width
    /// is estimated from the name's length, as a place's label is.
    pub title: Bounds,
    /// The room left for the count of tokens inside, under the title.
    pub count: Bounds,
    /// The circle of each port, in the order the ports were given.
    pub ports: Vec<PortCircle>,
}

impl SubnetBox {
    /// The box as [`Bounds`], in world coordinates.
    pub fn bounds(&self) -> Bounds {
        Bounds::from_center_half_size(self.centre, self.size / 2.0)
    }
}

/// One port's circle inside its subnet place's box.
#[derive(Debug, Clone, PartialEq)]
pub struct PortCircle {
    /// The port, by the id the outer net's edges name it with.
    pub place: PlaceId,
    /// The centre of its circle.
    pub centre: Point,
    /// Its radius: [`Layout::port_radius`](crate::Layout::port_radius).
    pub radius: f32,
    /// The room left beside the circle for the port's count, on the side
    /// toward the middle of the box.
    pub count: Bounds,
}

/// Where the layout put a closed group: one box, and the room it left
/// inside for the group's name and for the count of the tokens in the
/// group's places.
///
/// It is sized as a [`SubnetBox`] with no ports is, from the length of the
/// name, and drawn with rounded corners of
/// [`corner_radius`](GroupBox::corner_radius); nothing inside reaches them.
/// It has no port circles: every arrow on it ends at its edge.
#[derive(Debug, Clone, PartialEq)]
pub struct GroupBox {
    /// The centre of the box.
    pub centre: Point,
    /// Its width and height.
    pub size: Point,
    /// The radius of its rounded corners.
    pub corner_radius: f32,
    /// The room left for the group's name, centred across the box; its
    /// width is estimated from the name's length, as a place's label is.
    pub title: Bounds,
    /// The room left for the count of tokens inside, under the title.
    pub count: Bounds,
}

impl GroupBox {
    /// The box as [`Bounds`], in world coordinates.
    pub fn bounds(&self) -> Bounds {
        Bounds::from_center_half_size(self.centre, self.size / 2.0)
    }
}

/// One end of a [`MergedArrow`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ArrowEnd {
    /// A node drawn on its own: a place, a port's circle in its subnet
    /// place's box, or a transition. A ghosted place's end is drawn at its
    /// ghost beside the box at the other end
    /// ([`Layout::group_ghost_position`](crate::Layout::group_ghost_position)),
    /// as a ghosted place's edge is drawn at its ghost beside the
    /// transition.
    Node(LayoutNode),
    /// The box of the closed group of this name
    /// ([`Layout::group_box`](crate::Layout::group_box)).
    Group(String),
}

/// One arrow of the drawing with a closed group's box at one end or at
/// both: it stands for every edge of the net, in one direction, between the
/// box and one node or between two boxes.
///
/// Edges with neither end hidden are drawn as themselves and are not
/// merged; an edge with both ends in one closed group is not drawn at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedArrow {
    /// Where it starts.
    pub from: ArrowEnd,
    /// Where it ends.
    pub to: ArrowEnd,
    /// The edges of the net it stands for, each as `(from, to)` with a port
    /// named by the port, as
    /// [`Layout::is_back_edge`](crate::Layout::is_back_edge) names an edge,
    /// in the order the edges were given and without repeats.
    pub edges: Vec<(LayoutNode, LayoutNode)>,
    /// Whether it was set aside to break a cycle, and is therefore drawn
    /// curved, as [`Layout::is_back_edge`](crate::Layout::is_back_edge) says
    /// of an edge. An arrow that takes no part in the layout (one that
    /// crosses a frame, or stands for ghosted edges alone) never is.
    pub back: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_merged_arrow_is_eq() {
        fn eq<T: Eq>() {}
        eq::<MergedArrow>();
    }

    #[test]
    fn ghosting_applies_by_name_or_by_degree() {
        let by_name = Ghosting::named(["H"]);
        assert!(by_name.applies("H", 1));
        assert!(!by_name.applies("X", 99));
        let by_degree = Ghosting::default().with_min_degree(3);
        assert!(by_degree.applies("X", 3));
        assert!(!by_degree.applies("X", 2));
        assert!(!Ghosting::default().applies("X", 99));
    }
}
