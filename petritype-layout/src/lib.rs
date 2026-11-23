//! Where a net's places and transitions go when it is drawn: a layered
//! layout in the manner of Graphviz `dot`, computed from the net's edges.
//!
//! The net is read as a bipartite digraph. Places and transitions are the
//! nodes, each input edge runs place → transition and each output edge
//! transition → place, and the graph is laid out in four passes:
//!
//! 1. **Break cycles.** A depth-first search from the sources, in declaration
//!    order, marks every edge that reaches a node still on the search stack
//!    as a *back-edge*. Back-edges are drawn, but ignored for ranking, which
//!    is what turns a loop such as `Dispatch Truck -> Staging Area` into one
//!    curved arrow instead of a tangle.
//! 2. **Rank.** A node's rank is the longest forward path from a source to
//!    it, so every forward edge points to a strictly higher rank.
//! 3. **Order.** Within a rank, nodes are sorted by the barycentre of their
//!    neighbours in the ranks already placed, sweeping down and then up a few
//!    times to reduce crossings. Ties keep their previous order, so the
//!    result is deterministic.
//! 4. **Coordinates.** Rank along the main axis, order centred along the
//!    cross axis, and the whole drawing centred on the origin.
//!
//! There are no dummy nodes for edges that span several ranks, so such an
//! edge is drawn straight and may pass close to a node in between. That is
//! the main simplification against `dot`, and it costs nothing on the nets
//! this crate has been shown so far.
//!
//! # Groups
//!
//! A net with a few dozen places per subsystem spreads each subsystem over
//! the whole height of the drawing and tangles them. Its definition can
//! gather them into [groups](petritype::NetBuilder::group), named sets of
//! places and transitions that only drawings read. [`Layout::compute`] reads
//! each node's group and the order the groups were declared in off the net,
//! and a renderer that feeds the layout by name gives them with each
//! [`LayoutPlace`] and [`LayoutTransition`]. No name is split or shortened:
//! a node is drawn under its whole name.
//!
//! A group is drawn **open** unless [`Layout::closed`] names it. An open
//! group is laid out on its own, by the four passes above over only the
//! edges *inside* it, with a frame and room along its top for its title.
//! The nodes in no group are laid out together as one more block, with no
//! frame. The block comes first and the open groups follow in declared
//! order, packed in rows with [`Layout::group_gap`] between them. The row
//! width is chosen so the whole comes out as wide as a landscape window will
//! take it: the packing that frames largest on a screen 1.6 times as wide
//! as high wins. A layout with no open group is one drawing, centred on the
//! origin, so a net with no groups lays out exactly as it did before groups
//! came.
//!
//! Edges between blocks take no part in the layout, so they are drawn
//! straight across the gaps, and no edge between blocks is ever a
//! back-edge. [`Layout::group_names`], [`Layout::group_bounds`] and
//! [`Layout::group_of`] say where each group is; the renderer draws a frame
//! and a title from them.
//!
//! A **closed** group is one node of the block: a box with the group's name
//! and the count of the tokens inside, sized for them as a subnet place's
//! box is, which ranks and orders as a place would. An edge between one of
//! its members and a node of the block counts as an edge of the box, and an
//! edge wholly inside the group is not drawn. Every edge in one direction
//! between the box and one node, or between two boxes, is drawn as one
//! *merged arrow*; one that ends in an open group crosses its frame and
//! takes no part in the layout. The members have no node of their own while
//! the group is closed, so the position questions answer `None` for them.
//! [`Layout::group_box`], [`Layout::hidden_in`] and
//! [`Layout::merged_arrows`] say where the box is, which nodes it hides and
//! which arrows stand for which edges. With every group closed the layout is
//! one layered drawing of the boxes and the nodes in no group.
//!
//! # Ghosts
//!
//! A hub place, read or written by dozens of transitions, drags every one
//! of its edges across the drawing and pulls the layout out of shape. List
//! it in [`Layout::ghosting`] (by name, or by a minimum number of edges) and
//! its edges leave the graph the four passes work on. The place stays, with
//! its tokens and its count, but each transition it was joined to gets a
//! small *ghost* of it instead: on the side the ranks come from for an input
//! edge and the other side for an output edge, spread along the transition
//! when there are several, and the edge runs between the two. The layout
//! makes room for them: a transition with many ghosts takes a wider slot in
//! its rank, and the gap to the next rank grows to hold them. An isolated
//! home node lands in the first rank of its block.
//! [`Layout::is_ghosted`], [`Layout::ghosts`] and [`Layout::ghost_position`]
//! say what was ghosted and where.
//!
//! A ghosted place joined to the members of a closed group gets one ghost
//! beside the group's box on each side it needs, whatever the number of
//! members, so that a hub grows no long arrow to a box:
//! [`Layout::group_ghosts`] and [`Layout::group_ghost_position`] say where.
//! A ghosted place hidden in a closed group has no home node and no ghosts.
//! Its edges to the rest of the net are its box's merged arrows, drawn
//! straight, and take no part in the layout, as a ghosted place's edges
//! never do.
//!
//! The gaps within a rank are wide enough for the names drawn beside the
//! nodes. [`Layout::fit_labels`] turns that off.
//!
//! A part of a net, such as what a viewer's filter shows, is laid out on its
//! own with [`Layout::compute_subgraph`]: one drawing, with no frame but
//! those of the groups it was taken for. In a part taken for some groups,
//! the ghosted places of the other groups have no home node: their ghosts
//! stand for them.
//!
//! # Subnet places
//!
//! A [subnet place](petritype::NetBuilder::subnet_place) holds a net of its
//! own, which the net around it reaches through the subnet's port places.
//! The layout is told which places are subnet places and which ports belong
//! to each by the [`PlaceKind`] of each [`LayoutPlace`] it is given:
//! [`Layout::compute`] reads it off the net, and a renderer that feeds the
//! layout by name gives it with each place. A port is named by the pair of
//! its subnet place and its own name, so a port may have the name of any
//! place outside.
//!
//! A subnet place is drawn closed, as a box with rounded corners that holds
//! a small circle for each port, with the place's name as its title and the
//! count of tokens inside under it. It takes part in the four passes as
//! **one node**: an edge to or from one of its ports counts as an edge to or
//! from the box, so the box ranks and orders as a place would. The box is
//! as large as its circles, its title and its count need, and the layout
//! keeps its neighbours as clear of it as of a place: its slot in its rank
//! is wider by as much as the box is wider than a place, and the gaps to
//! the ranks beside it grow so that its transitions are as far from it as
//! from a place.
//!
//! Each port circle sits on the side of the box that faces the transitions
//! it is joined to: the side the ranks come from when most of them are in
//! earlier ranks, the other side when most are in later ones. A port with
//! no such majority, or joined to nothing yet, joins the side most of the
//! box's other ports are on, and the side the ranks come from on a tie. On
//! each side the circles are ordered by where their transitions ended up
//! across the ranks, so that arrows cross as little as that order allows,
//! and ports that join nothing come last, in the order they were given.
//! [`Layout::subnet_box`] says where the box, its title, its count and its
//! circles are; [`Layout::position_of_place`] answers for a port with the
//! centre of its circle, and [`Layout::radius_of_place`] with
//! [`Layout::port_radius`], so an arrow on a port ends at its circle.
//!
//! A subnet place may be in a group. In an open one it is drawn inside the
//! frame, and in a closed one it is hidden with its ports.
//!
//! A net with no subnet places lays out exactly as it did before subnet
//! places came.
//!
//! A renderer gets positions from [`Layout`] as [`Point`]s and frames as
//! [`Bounds`], two small types of this crate's own, and converts them to its
//! own vectors. The crate depends on [`petritype`] alone, and the algorithm
//! can be replaced without a renderer noticing.

#![warn(missing_docs, missing_debug_implementations, unreachable_pub)]
#![forbid(unsafe_code)]

mod geometry;

use std::collections::{BTreeSet, HashMap, HashSet};

pub use geometry::{Bounds, Point};
use petritype::{EdgeInfo, Net, PlaceId, PlaceInfo, TransitionId, TransitionInfo};

/// How many tokens a place draws: one at the centre, a ring of 6 around it
/// and a ring of 12 around that. Further tokens are counted, not drawn.
pub const TOKEN_CAPACITY: usize = 19;

/// How many down-and-up barycentre sweeps the ordering pass makes.
const SWEEPS: usize = 4;

/// The shape of window the packing of groups aims for: width over height.
const LANDSCAPE: f32 = 1.6;
/// Room between a group's nodes and its frame.
const FRAME_PAD: f32 = 24.0;
/// Extra room along the top of a group's frame, for its title.
const TITLE_ROOM: f32 = 26.0;
/// How wide a character of a place's name is drawn, as an estimate: the
/// renderer's 15-point text, in a font whose characters are six tenths of
/// their size wide. An estimate that is too narrow lets two long names
/// side by side touch.
const PLACE_CHAR_WIDTH: f32 = 9.0;
/// The same for a transition's name, drawn in 14-point text.
const TRANSITION_CHAR_WIDTH: f32 = 8.4;
/// The distance between a transition's box and its ghosts: long enough for
/// an arrow.
const GHOST_GAP: f32 = 24.0;
/// The height of a ghost's name, beyond the ghost.
const GHOST_LABEL: f32 = 16.0;
/// The room left between a transition's ghosts and the next rank.
const GHOST_CLEARANCE: f32 = 16.0;
/// The space left between one node's label and the next node, when
/// [`Layout::fit_labels`] spaces them to fit.
const LABEL_GAP: f32 = 14.0;
/// How wide the token count under a place's name is.
const COUNT_WIDTH: f32 = 70.0;
/// The room between a subnet place's box and what it holds. It is also the
/// radius of the box's rounded corners, so nothing inside reaches them.
const BOX_PAD: f32 = 12.0;
/// The height of a subnet place's title, in the renderer's 15-point text.
const BOX_TITLE: f32 = 20.0;
/// The height of the count of tokens inside, under the title.
const BOX_COUNT: f32 = 18.0;
/// The room between the port circles on one side of a box and its title and
/// count.
const BOX_GAP: f32 = 6.0;
/// How wide a port's count, beside its circle, is: three digits of small
/// text.
const PORT_COUNT_WIDTH: f32 = 24.0;
/// The room between a port's circle and its count.
const PORT_COUNT_GAP: f32 = 4.0;
/// The room between one port's count and the next port's circle.
const PORT_SPACING: f32 = 8.0;

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
/// [`compute_named`](Layout::compute_named) and
/// [`compute_subgraph`](Layout::compute_subgraph) take anything that turns
/// into one, so a plain `(PlaceId, String)` is still an ordinary place in no
/// group, and `(PlaceId, PlaceInfo)` from [`Net::places`] carries the kind
/// and the group the net says. The ports of a subnet place are its places of
/// kind [`PlaceKind::Port`], in the order they are given, which should be the
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
/// [`compute_named`](Layout::compute_named) and
/// [`compute_subgraph`](Layout::compute_subgraph) take anything that turns
/// into one, so a plain `(TransitionId, String)` is a transition in no
/// group, and `(TransitionId, TransitionInfo)` from [`Net::transitions`]
/// carries the group the net says.
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
    /// [`Layout::position_of_place`] answers for the subnet place.
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
    /// Its radius: [`Layout::port_radius`].
    pub radius: f32,
    /// The room left beside the circle for the port's count, on the side
    /// toward the middle of the box.
    pub count: Bounds,
}

/// Which side of a subnet place's box a port's circle is on, along the main
/// axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoxSide {
    /// The side the ranks come from: the top, or the left.
    Before,
    /// The side the ranks go to: the bottom, or the right.
    After,
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
    /// ([`Layout::group_ghost_position`]), as a ghosted place's edge is
    /// drawn at its ghost beside the transition.
    Node(LayoutNode),
    /// The box of the closed group of this name ([`Layout::group_box`]).
    Group(String),
}

/// One arrow of the drawing with a closed group's box at one end or at
/// both: it stands for every edge of the net, in one direction, between the
/// box and one node or between two boxes.
///
/// Edges with neither end hidden are drawn as themselves and are not
/// merged; an edge with both ends in one closed group is not drawn at all.
#[derive(Debug, Clone, PartialEq)]
pub struct MergedArrow {
    /// Where it starts.
    pub from: ArrowEnd,
    /// Where it ends.
    pub to: ArrowEnd,
    /// The edges of the net it stands for, each as `(from, to)` with a
    /// port named by the port, as [`Layout::is_back_edge`] names an edge, in
    /// the order the edges were given and without repeats.
    pub edges: Vec<(LayoutNode, LayoutNode)>,
    /// Whether it was set aside to break a cycle, and is therefore drawn
    /// curved, as [`Layout::is_back_edge`] says of an edge. An arrow that
    /// takes no part in the layout (one that crosses a frame, or stands for
    /// ghosted edges alone) never is.
    pub back: bool,
}

/// One group the last computation drew, open or closed.
#[derive(Debug, Clone, PartialEq)]
struct DrawnGroup {
    /// Its name.
    name: String,
    /// Its frame, or its box.
    drawn: Drawn,
}

/// How a group was drawn.
#[derive(Debug, Clone, PartialEq)]
enum Drawn {
    /// Open, in this frame, in world coordinates.
    Open(Bounds),
    /// Closed, as this box.
    Closed(GroupBox),
}

/// How a group with members is to be drawn, before it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// In a frame of its own.
    Open,
    /// As one box among the nodes in no group.
    Closed,
    /// Among the nodes in no group, with no frame: in a part, a group it
    /// was not taken for, or the one it was taken for alone.
    Loose,
}

/// Where a node goes. `drawn` is the group's index among the groups drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Where {
    /// Among the nodes in no group.
    Block,
    /// In the frame of an open group, laid out as block `block`.
    Frame { drawn: usize, block: usize },
    /// Hidden in the box of a closed group, which is node `at`.
    Hidden { drawn: usize, at: usize },
}

/// What an edge of the net is drawn as, once the closed groups are boxes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fate {
    /// Itself: neither end is hidden.
    Plain,
    /// Part of this merged arrow.
    Arrow(usize),
    /// Nothing: both ends are in one closed group.
    Inside,
}

/// What one edge of a block's graph stands for.
#[derive(Debug, Clone, Copy)]
enum Source {
    /// An edge of the net, by its index.
    Edge(usize),
    /// A merged arrow, by its index.
    Arrow(usize),
}

/// The computed layout, and the knobs it was computed with.
///
/// The public fields are configuration. The positions are filled in by
/// [`compute`](Layout::compute) and its siblings, which lay the net out with
/// that configuration and can be called again whenever the net changes.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// Which way the ranks grow.
    pub direction: Direction,
    /// The distance between neighbouring ranks, along the main axis.
    pub rank_sep: f32,
    /// The distance between neighbours within a rank, along the cross axis.
    pub node_sep: f32,
    /// The radius of a place.
    pub place_radius: f32,
    /// The radius of a token.
    pub token_radius: f32,
    /// The width and height of a transition.
    pub transition_size: Point,
    /// The names of the groups to draw closed, each as one box. Every other
    /// group is drawn open, in a frame. Empty by default. A name no node's
    /// group has is ignored.
    pub closed: BTreeSet<String>,
    /// The distance between neighbouring open groups' frames, and between
    /// them and the nodes in no group, both across and down the rows they
    /// are packed in.
    pub group_gap: f32,
    /// Which places are drawn as ghosts beside their transitions. Nothing
    /// by default.
    pub ghosting: Ghosting,
    /// Whether to space the nodes of a rank so that the name drawn beside
    /// each fits between it and its neighbour: the width of a name is
    /// estimated from its length. On by default. Off spaces nodes
    /// [`node_sep`](Layout::node_sep) apart whatever their names, so long
    /// names overlap.
    pub fit_labels: bool,
    /// The radius of a ghost: smaller than a place's.
    pub ghost_radius: f32,
    /// The distance between neighbouring ghosts on one side of a
    /// transition. A transition with many ghosts gets more room in its rank.
    pub ghost_sep: f32,
    /// The radius of a port's circle inside its subnet place's box: a
    /// ghost's by default.
    pub port_radius: f32,
    /// Every node the last [`compute`](Layout::compute) placed.
    nodes: HashMap<LayoutNode, Placement>,
    /// The edges the cycle-breaking pass set aside, as `(from, to)`.
    back_edges: HashSet<(LayoutNode, LayoutNode)>,
    /// How many ranks the last computation produced.
    ranks: usize,
    /// The groups the last computation drew, open or closed, in order.
    groups: Vec<DrawnGroup>,
    /// Which of `groups` each node is in, hidden or not.
    group_index: HashMap<LayoutNode, usize>,
    /// The ghosted places of the last computation.
    ghosted: HashSet<PlaceId>,
    /// Every ghost beside a transition, in declaration order of the edges,
    /// and where it is.
    ghosts: Vec<((PlaceId, TransitionId, GhostSide), Point)>,
    /// Every ghost beside a closed group's box, by the group's index in
    /// `groups`, in declaration order of the edges, and where it is.
    group_ghosts: Vec<((PlaceId, usize, GhostSide), Point)>,
    /// The box of each subnet place of the last computation.
    subnets: HashMap<PlaceId, SubnetBox>,
    /// The centre of each port's circle, for the ports drawn in a box.
    ports: HashMap<PlaceId, Point>,
    /// The arrows of the closed groups' boxes, in the order of their first
    /// edges.
    arrows: Vec<MergedArrow>,
    /// Which of `arrows` each edge of the net is drawn as, by its ends.
    arrow_index: HashMap<(LayoutNode, LayoutNode), usize>,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            direction: Direction::TopDown,
            rank_sep: 150.0,
            node_sep: 180.0,
            place_radius: 40.0,
            token_radius: 6.0,
            transition_size: Point::new(120.0, 50.0),
            closed: BTreeSet::new(),
            group_gap: 80.0,
            ghosting: Ghosting::default(),
            fit_labels: true,
            ghost_radius: 14.0,
            ghost_sep: 80.0,
            port_radius: 14.0,
            nodes: HashMap::new(),
            back_edges: HashSet::new(),
            ranks: 0,
            groups: Vec::new(),
            group_index: HashMap::new(),
            ghosted: HashSet::new(),
            ghosts: Vec::new(),
            group_ghosts: Vec::new(),
            subnets: HashMap::new(),
            ports: HashMap::new(),
            arrows: Vec::new(),
            arrow_index: HashMap::new(),
        }
    }
}

impl Layout {
    /// The default layout, with the ranks growing in `direction` instead.
    ///
    /// The computed half of a `Layout` is private, so struct-update syntax
    /// cannot build one; this, or setting the public fields on a
    /// `Layout::default()`, can.
    pub fn with_direction(mut self, direction: Direction) -> Self {
        self.direction = direction;
        self
    }

    /// The default layout, drawing the groups of these names closed
    /// instead (see [`closed`](Layout::closed)).
    pub fn with_closed<S: Into<String>>(mut self, groups: impl IntoIterator<Item = S>) -> Self {
        self.closed = groups.into_iter().map(Into::into).collect();
        self
    }

    /// The default layout, ghosting places by [`Ghosting`] instead.
    pub fn with_ghosting(mut self, ghosting: Ghosting) -> Self {
        self.ghosting = ghosting;
        self
    }

    /// Lays out every place and transition of `net`, replacing whatever was
    /// computed before and keeping the configuration. The net says which of
    /// its places are subnet places and which are their ports, which group
    /// each node is in, and the order its groups were declared in.
    pub fn compute(&mut self, net: &Net) {
        let groups: Vec<&str> = net.groups().collect();
        self.compute_named(
            net.places().map(LayoutPlace::from),
            net.transitions().map(LayoutTransition::from),
            net.edges(),
            &groups,
        );
    }

    /// Lays out the given places, transitions and edges, replacing whatever
    /// was computed before and keeping the configuration.
    ///
    /// The nodes are taken in the order given, which should be declaration
    /// order: ties in the layout keep it. An edge naming a node not given is
    /// left out. Without names there is nothing to ghost by name, so only
    /// [`Ghosting::min_degree`] applies, every place is an ordinary one, and
    /// every node is in no group: use
    /// [`compute_named`](Layout::compute_named) for those.
    pub fn compute_from(
        &mut self,
        places: impl IntoIterator<Item = PlaceId>,
        transitions: impl IntoIterator<Item = TransitionId>,
        edges: impl IntoIterator<Item = EdgeInfo>,
    ) {
        self.run(
            places
                .into_iter()
                .map(|id| LayoutPlace::new(id, String::new(), PlaceKind::Ordinary))
                .collect(),
            transitions
                .into_iter()
                .map(|id| LayoutTransition::new(id, String::new()))
                .collect(),
            edges.into_iter().collect(),
            None,
            &[],
            None,
        );
    }

    /// Lays out a part of a net on its own, as one drawing with no frame but
    /// those of the groups it was taken for, so there is no packing of what
    /// was left out.
    ///
    /// `edges` are the edges of the part. `whole` are the edges of the net it
    /// was taken from, and decide which places [`Ghosting::min_degree`]
    /// ghosts, so a hub is a hub in every part of the net and not only where
    /// all its edges happen to be shown.
    ///
    /// The places, the transitions and `groups` are given as for
    /// [`compute_named`](Layout::compute_named). `focus` is the set of
    /// groups the part was taken for, if any. A focus of one group is a
    /// plain drawing of it. A focus of two or more keeps their frames, and
    /// the other nodes share one more block, with no frame. Any group of the
    /// part that is not focused is drawn without a frame among the nodes in
    /// no group, or as a box if [`closed`](Layout::closed) names it; a
    /// focused group is always drawn open. A ghosted place outside the focus
    /// keeps its ghosts but gets no home node of its own: the ghosts stand
    /// for it.
    ///
    /// A subnet place in the part is drawn with the circles of those of its
    /// ports that the part holds, and a port without its subnet place is
    /// drawn as an ordinary place.
    pub fn compute_subgraph(
        &mut self,
        places: impl IntoIterator<Item = impl Into<LayoutPlace>>,
        transitions: impl IntoIterator<Item = impl Into<LayoutTransition>>,
        edges: impl IntoIterator<Item = EdgeInfo>,
        whole: impl IntoIterator<Item = EdgeInfo>,
        groups: &[&str],
        focus: Option<&[&str]>,
    ) {
        self.run(
            places.into_iter().map(Into::into).collect(),
            transitions.into_iter().map(Into::into).collect(),
            edges.into_iter().collect(),
            Some(whole.into_iter().collect()),
            groups,
            focus,
        );
    }

    /// [`compute_from`](Layout::compute_from) with each node's name and
    /// group: the names are what the room for labels is estimated from, and
    /// the groups what the layout gathers the nodes by. It needs no [`Net`],
    /// so a renderer can feed the layout through it from whatever source it
    /// reads the net from.
    ///
    /// Each place is a [`LayoutPlace`], or anything that turns into one, and
    /// each transition a [`LayoutTransition`], or anything that turns into
    /// one: a `(PlaceId, String)` is an ordinary place and a
    /// `(TransitionId, String)` a transition, each in no group. A
    /// [`LayoutPlace`] with [`PlaceKind::Subnet`] or [`PlaceKind::Port`]
    /// tells the layout about a subnet place and its ports.
    ///
    /// `groups` is the order of the groups, which should be the order the
    /// net declares them in: the open ones are packed in it. A group a node
    /// names that `groups` does not list comes after those it lists, in the
    /// order of its first node, so `&[]` packs the groups in that order. A
    /// group no node is in has no frame and no box.
    pub fn compute_named(
        &mut self,
        places: impl IntoIterator<Item = impl Into<LayoutPlace>>,
        transitions: impl IntoIterator<Item = impl Into<LayoutTransition>>,
        edges: impl IntoIterator<Item = EdgeInfo>,
        groups: &[&str],
    ) {
        self.run(
            places.into_iter().map(Into::into).collect(),
            transitions.into_iter().map(Into::into).collect(),
            edges.into_iter().collect(),
            None,
            groups,
            None,
        );
    }

    /// The whole computation. `whole` is given for a part of a net, and is
    /// the edges the ghosting degrees are counted over; `order` is the order
    /// of the groups, and `focus` the groups a part was taken for.
    fn run(
        &mut self,
        places: Vec<LayoutPlace>,
        transitions: Vec<LayoutTransition>,
        given: Vec<EdgeInfo>,
        whole: Option<Vec<EdgeInfo>>,
        order: &[&str],
        focus: Option<&[&str]>,
    ) {
        let part = whole.is_some();
        // The groups: those in the order given, then any other a node
        // names, in the order of its first node.
        let mut groups: Vec<String> = Vec::new();
        for &name in order {
            position_or_push(&mut groups, name);
        }
        // A port whose subnet place is given is drawn in that place's box
        // and is no node of its own: the box stands for it in every pass.
        let subnets: HashSet<PlaceId> = places
            .iter()
            .filter(|place| place.kind == PlaceKind::Subnet)
            .map(|place| place.id)
            .collect();
        let mut nodes: Vec<LayoutNode> = Vec::new();
        let mut names: Vec<String> = Vec::new();
        let mut boxed: Vec<bool> = Vec::new();
        // The group of each node, by its index in `groups`.
        let mut member: Vec<Option<usize>> = Vec::new();
        // Each port drawn in a box, in the order given: the port, its name
        // and its subnet place. It goes where its box goes.
        let mut ports: Vec<(PlaceId, String, PlaceId)> = Vec::new();
        for place in places {
            match place.kind {
                PlaceKind::Port(subnet) if subnets.contains(&subnet) => {
                    ports.push((place.id, place.name, subnet));
                }
                kind => {
                    nodes.push(LayoutNode::Place(place.id));
                    names.push(place.name);
                    boxed.push(kind == PlaceKind::Subnet);
                    member.push(place.group.map(|g| position_or_push(&mut groups, &g)));
                }
            }
        }
        for transition in transitions {
            nodes.push(LayoutNode::Transition(transition.id));
            names.push(transition.name);
            boxed.push(false);
            member.push(transition.group.map(|g| position_or_push(&mut groups, &g)));
        }
        let n = nodes.len();
        let mut index: HashMap<LayoutNode, usize> =
            nodes.iter().enumerate().map(|(i, &n)| (n, i)).collect();
        // An edge on a port is an edge on its box.
        for (port, _, subnet) in &ports {
            if let Some(&at) = index.get(&LayoutNode::Place(*subnet)) {
                index.insert(LayoutNode::Place(*port), at);
            }
        }

        let mut edges: Vec<Edge> = Vec::new();
        for edge in given {
            let (place, transition, side) = match edge {
                EdgeInfo::Input {
                    place, transition, ..
                } => (place, transition, GhostSide::Input),
                EdgeInfo::Output {
                    transition, place, ..
                } => (place, transition, GhostSide::Output),
                // `EdgeInfo` is `#[non_exhaustive]`: an unknown direction
                // does not shape the layout.
                _ => continue,
            };
            let (p, t) = (LayoutNode::Place(place), LayoutNode::Transition(transition));
            if let (Some(&p), Some(&t)) = (index.get(&p), index.get(&t)) {
                let (a, b) = match side {
                    GhostSide::Input => (p, t),
                    GhostSide::Output => (t, p),
                };
                edges.push(Edge {
                    a,
                    b,
                    place,
                    transition,
                    side,
                    ghosted: false,
                });
            }
        }

        self.nodes.clear();
        self.back_edges.clear();
        self.groups.clear();
        self.group_index.clear();
        self.ghosted.clear();
        self.ghosts.clear();
        self.group_ghosts.clear();
        self.subnets.clear();
        self.ports.clear();
        self.arrows.clear();
        self.arrow_index.clear();
        self.ranks = 0;

        // Ghosted places: their edges leave the graph the layout works on
        // and become ghosts beside the transitions.
        let mut degree: HashMap<PlaceId, usize> = HashMap::new();
        match &whole {
            Some(whole) => {
                for edge in whole {
                    if let EdgeInfo::Input { place, .. } | EdgeInfo::Output { place, .. } = edge {
                        *degree.entry(*place).or_default() += 1;
                    }
                }
            }
            None => {
                for edge in &edges {
                    *degree.entry(edge.place).or_default() += 1;
                }
            }
        }
        // A subnet place has no edges of its own to ghost; a port in a box
        // is ghosted as any place, and its circle is its home.
        for (i, node) in nodes.iter().enumerate() {
            if let LayoutNode::Place(id) = *node
                && !boxed[i]
                && self
                    .ghosting
                    .applies(&names[i], degree.get(&id).copied().unwrap_or(0))
            {
                self.ghosted.insert(id);
            }
        }
        for (port, name, _) in &ports {
            if self
                .ghosting
                .applies(name, degree.get(port).copied().unwrap_or(0))
            {
                self.ghosted.insert(*port);
            }
        }

        // How each group with members is drawn. A part draws the groups it
        // was taken for open, in frames when there are several, and its
        // other groups closed or among the nodes in no group.
        let mut size = vec![0_usize; groups.len()];
        for &g in member.iter().flatten() {
            size[g] += 1;
        }
        let focused = |g: usize| focus.is_some_and(|f| f.contains(&groups[g].as_str()));
        let several = focus.is_some_and(|f| f.len() > 1);
        let modes: Vec<Option<Mode>> = (0..groups.len())
            .map(|g| {
                (size[g] > 0).then(|| {
                    if focused(g) {
                        if several { Mode::Open } else { Mode::Loose }
                    } else if self.closed.contains(&groups[g]) {
                        Mode::Closed
                    } else if part {
                        Mode::Loose
                    } else {
                        Mode::Open
                    }
                })
            })
            .collect();
        // The groups drawn, open or closed, in order. Each open group is a
        // block of its own after the block of the nodes in no group, and
        // each closed group's box a node after the nodes given.
        let mut placed_group = vec![Where::Block; groups.len()];
        let mut drawn_names: Vec<String> = Vec::new();
        // Whether each drawn group is closed.
        let mut drawn_closed: Vec<bool> = Vec::new();
        // The drawn group of each open group's block, after the first, and
        // of each box.
        let mut block_group: Vec<usize> = Vec::new();
        let mut boxes: Vec<usize> = Vec::new();
        for (g, &mode) in modes.iter().enumerate() {
            let drawn = drawn_names.len();
            placed_group[g] = match mode {
                Some(Mode::Open) => {
                    block_group.push(drawn);
                    Where::Frame {
                        drawn,
                        block: block_group.len(),
                    }
                }
                Some(Mode::Closed) => {
                    boxes.push(drawn);
                    Where::Hidden {
                        drawn,
                        at: n + boxes.len() - 1,
                    }
                }
                Some(Mode::Loose) | None => continue,
            };
            drawn_names.push(groups[g].clone());
            drawn_closed.push(mode == Some(Mode::Closed));
        }
        let blocks = 1 + block_group.len();
        let place: Vec<Where> = member
            .iter()
            .map(|m| m.map_or(Where::Block, |g| placed_group[g]))
            .collect();
        // The closed group hiding a node, and the box's node.
        let hidden = |i: usize| match place[i] {
            Where::Hidden { drawn, at } => Some((drawn, at)),
            _ => None,
        };
        // The block a node, or a box, is laid out in.
        let block_of = |i: usize| match place.get(i) {
            Some(Where::Frame { block, .. }) => *block,
            _ => 0,
        };
        // In a part taken for some groups, a ghosted place of the others
        // that is not hidden is left out as a node, its ghosts standing for
        // it.
        let homeless: Vec<bool> = (0..n)
            .map(|i| {
                focus.is_some()
                    && matches!(nodes[i], LayoutNode::Place(id) if self.ghosted.contains(&id))
                    && hidden(i).is_none()
                    && !member[i].is_some_and(focused)
            })
            .collect();

        // The ghosts of each transition and of each closed group's box, by
        // node index: inputs, outputs. A ghosted place hidden in a closed
        // group has none; one joined to the members of a closed group has
        // one beside the box on each side it needs.
        let mut slots: HashMap<usize, Slots> = HashMap::new();
        for edge in &mut edges {
            edge.ghosted = self.ghosted.contains(&edge.place);
            if !edge.ghosted {
                continue;
            }
            let (at_place, at_transition) = match edge.side {
                GhostSide::Input => (edge.a, edge.b),
                GhostSide::Output => (edge.b, edge.a),
            };
            if hidden(at_place).is_some() {
                continue;
            }
            let holder = match hidden(at_transition) {
                Some((drawn, at)) => {
                    let key = (edge.place, drawn, edge.side);
                    if self.group_ghosts.iter().any(|(k, _)| *k == key) {
                        continue;
                    }
                    self.group_ghosts.push((key, Point::ZERO));
                    at
                }
                None => {
                    let key = (edge.place, edge.transition, edge.side);
                    if self.ghosts.iter().any(|(k, _)| *k == key) {
                        continue;
                    }
                    self.ghosts.push((key, Point::ZERO));
                    at_transition
                }
            };
            let slot = slots.entry(holder).or_default();
            match edge.side {
                GhostSide::Input => slot.inputs.push(edge.place),
                GhostSide::Output => slot.outputs.push(edge.place),
            }
        }

        // What each edge is drawn as. The edges in one direction between a
        // closed group's box and one node, or two boxes, are one merged
        // arrow. It takes part in the layout of the block through any of
        // its edges that is not ghosted, unless it crosses a frame.
        let mut fates: Vec<Fate> = Vec::with_capacity(edges.len());
        let mut arrow_ends: Vec<(usize, usize)> = Vec::new();
        let mut takes_part: Vec<bool> = Vec::new();
        let mut arrow_at: HashMap<(ArrowEnd, ArrowEnd), usize> = HashMap::new();
        for edge in &edges {
            let (hidden_a, hidden_b) = (hidden(edge.a), hidden(edge.b));
            let fate = match (hidden_a, hidden_b) {
                (None, None) => Fate::Plain,
                (Some((x, _)), Some((y, _))) if x == y => Fate::Inside,
                _ => {
                    let (from, to) = edge.ends();
                    let end = |node: LayoutNode, in_box: Option<(usize, usize)>| match in_box {
                        Some((drawn, _)) => ArrowEnd::Group(drawn_names[drawn].clone()),
                        None => ArrowEnd::Node(node),
                    };
                    let key = (end(from, hidden_a), end(to, hidden_b));
                    let k = match arrow_at.get(&key) {
                        Some(&k) => k,
                        None => {
                            let k = self.arrows.len();
                            arrow_at.insert(key.clone(), k);
                            self.arrows.push(MergedArrow {
                                from: key.0,
                                to: key.1,
                                edges: Vec::new(),
                                back: false,
                            });
                            arrow_ends.push((
                                hidden_a.map_or(edge.a, |(_, at)| at),
                                hidden_b.map_or(edge.b, |(_, at)| at),
                            ));
                            takes_part.push(false);
                            k
                        }
                    };
                    if !self.arrows[k].edges.contains(&(from, to)) {
                        self.arrows[k].edges.push((from, to));
                        self.arrow_index.insert((from, to), k);
                    }
                    let (a, b) = arrow_ends[k];
                    if !edge.ghosted && block_of(a) == 0 && block_of(b) == 0 {
                        takes_part[k] = true;
                    }
                    Fate::Arrow(k)
                }
            };
            fates.push(fate);
        }

        // The nodes of each block, in the order given: a closed group's box
        // where its first member would be.
        let mut members: Vec<Vec<usize>> = vec![Vec::new(); blocks];
        let mut boxed_yet: HashSet<usize> = HashSet::new();
        for (i, (&gone, &spot)) in homeless.iter().zip(&place).enumerate() {
            if gone {
                continue;
            }
            match spot {
                Where::Block => members[0].push(i),
                Where::Frame { block, .. } => members[block].push(i),
                Where::Hidden { at, .. } => {
                    if boxed_yet.insert(at) {
                        members[0].push(at);
                    }
                }
            }
        }
        // A node's label is its name, and a box's its group's name.
        let mut labels = names;
        labels.extend(boxes.iter().map(|&drawn| drawn_names[drawn].clone()));

        // Each block on its own, from its own edges.
        let mut arranged: Vec<Arranged> = Vec::with_capacity(blocks);
        // The box of each subnet place and of each closed group, by node
        // index.
        let mut plans: Vec<Option<BoxPlan>> = vec![None; labels.len()];
        for (k, block) in members.iter().enumerate() {
            let local: HashMap<usize, usize> =
                block.iter().enumerate().map(|(l, &i)| (i, l)).collect();
            // The edges inside it, by local index, and what each stands for.
            let mut inside: Vec<(usize, usize)> = Vec::new();
            let mut sources: Vec<Source> = Vec::new();
            let mut linked = vec![false; self.arrows.len()];
            for (e, edge) in edges.iter().enumerate() {
                match fates[e] {
                    Fate::Plain
                        if !edge.ghosted && block_of(edge.a) == k && block_of(edge.b) == k =>
                    {
                        inside.push((local[&edge.a], local[&edge.b]));
                        sources.push(Source::Edge(e));
                    }
                    Fate::Arrow(a) if k == 0 && takes_part[a] && !linked[a] => {
                        linked[a] = true;
                        let (x, y) = arrow_ends[a];
                        inside.push((local[&x], local[&y]));
                        sources.push(Source::Arrow(a));
                    }
                    _ => {}
                }
            }
            let layers = layer(block.len(), &inside);
            self.ranks = self.ranks.max(layers.layers.len());
            for (s, source) in sources.iter().enumerate() {
                if layers.back[s] {
                    match *source {
                        // Named by its own ends, a port's included.
                        Source::Edge(e) => {
                            self.back_edges.insert(edges[e].ends());
                        }
                        Source::Arrow(a) => self.arrows[a].back = true,
                    }
                }
            }
            // Which side of its box each port faces, by the ranks of what it
            // is joined to inside the block.
            let mut rank = vec![0_usize; block.len()];
            for (r, layer) in layers.layers.iter().enumerate() {
                for &l in layer {
                    rank[l] = r;
                }
            }
            for (l, &i) in block.iter().enumerate() {
                if i >= n {
                    // A closed group's box holds its title and count.
                    plans[i] = Some(self.plan_box(&labels[i], &[]));
                    continue;
                }
                let LayoutNode::Place(subnet) = nodes[i] else {
                    continue;
                };
                if !boxed[i] {
                    continue;
                }
                let sides: Vec<(PlaceId, Option<BoxSide>)> = ports
                    .iter()
                    .filter(|(_, _, of)| *of == subnet)
                    .map(|&(port, _, _)| {
                        let own = ArrowEnd::Node(LayoutNode::Place(port));
                        let (mut before, mut after) = (0_usize, 0_usize);
                        for (s, source) in sources.iter().enumerate() {
                            let touches = match *source {
                                Source::Edge(e) => edges[e].place == port,
                                Source::Arrow(a) => {
                                    self.arrows[a].from == own || self.arrows[a].to == own
                                }
                            };
                            if !touches {
                                continue;
                            }
                            // Never the same rank: an edge inside a block
                            // joins two ranks.
                            let (x, y) = inside[s];
                            let other = if x == l { y } else { x };
                            if rank[other] < rank[l] {
                                before += 1;
                            } else {
                                after += 1;
                            }
                        }
                        let side = match before.cmp(&after) {
                            std::cmp::Ordering::Greater => Some(BoxSide::Before),
                            std::cmp::Ordering::Less => Some(BoxSide::After),
                            std::cmp::Ordering::Equal => None,
                        };
                        (port, side)
                    })
                    .collect();
                plans[i] = Some(self.plan_box(&labels[i], &sides));
            }
            let input = Block {
                members: block,
                names: &labels,
                nodes: &nodes,
                slots: &slots,
                inside: &inside,
                plans: &plans,
            };
            arranged.push(self.arrange(&input, &layers));
        }

        // Where each closed group's box ended up, by its place among the
        // boxes, and each open group's frame, by drawn group.
        let mut box_centres: Vec<Option<Point>> = vec![None; boxes.len()];
        let mut frames: Vec<Option<Bounds>> = vec![None; drawn_names.len()];
        if blocks == 1 {
            // One drawing: centre it on the origin, so a camera at rest sees
            // it.
            let only = arranged.pop().unwrap_or_default();
            let points: Vec<Point> = only.placed.iter().map(|&(_, p)| p.position).collect();
            let centre = bounding(&points).map_or(Point::ZERO, |b| b.center());
            for (i, mut placement) in only.placed {
                placement.position -= centre;
                match nodes.get(i) {
                    Some(&node) => {
                        self.nodes.insert(node, placement);
                    }
                    None => box_centres[i - n] = Some(placement.position),
                }
            }
        } else {
            // Pack the blocks that hold anything, the nodes in no group
            // first, then centre the whole on the origin.
            let packed: Vec<usize> = (0..blocks).filter(|&k| !members[k].is_empty()).collect();
            let sizes: Vec<Point> = packed.iter().map(|&k| arranged[k].bounds.size()).collect();
            let corners = pack(&sizes, self.group_gap);
            let mut extent: Option<Bounds> = None;
            let mut shifted: Vec<Bounds> = Vec::with_capacity(packed.len());
            for (&k, corner) in packed.iter().zip(&corners) {
                let a = &arranged[k];
                let shift = *corner - Point::new(a.bounds.min.x, a.bounds.max.y);
                let rect = Bounds::from_corners(a.bounds.min + shift, a.bounds.max + shift);
                extent = Some(extent.map_or(rect, |w| w.union(rect)));
                shifted.push(rect);
            }
            let centre = extent.map_or(Point::ZERO, |w| w.center());
            for (j, &k) in packed.iter().enumerate() {
                let a = std::mem::take(&mut arranged[k]);
                let shift = corners[j] - Point::new(a.bounds.min.x, a.bounds.max.y) - centre;
                for (i, mut placement) in a.placed {
                    placement.position += shift;
                    match nodes.get(i) {
                        Some(&node) => {
                            self.nodes.insert(node, placement);
                        }
                        None => box_centres[i - n] = Some(placement.position),
                    }
                }
                if k > 0 {
                    frames[block_group[k - 1]] = Some(Bounds::from_corners(
                        shifted[j].min - centre,
                        shifted[j].max - centre,
                    ));
                }
            }
        }

        // Which drawn group each node is in, hidden or not; a port is in
        // its subnet place's.
        for (i, &at) in place.iter().enumerate() {
            if let Where::Frame { drawn, .. } | Where::Hidden { drawn, .. } = at {
                self.group_index.insert(nodes[i], drawn);
            }
        }
        for (port, _, _) in &ports {
            if let Some(&i) = index.get(&LayoutNode::Place(*port))
                && let Where::Frame { drawn, .. } | Where::Hidden { drawn, .. } = place[i]
            {
                self.group_index.insert(LayoutNode::Place(*port), drawn);
            }
        }

        // Ghosts go beside their transitions and boxes, wherever those ended
        // up.
        for (&holder, slot) in &slots {
            let (at, half, beside) = match nodes.get(holder) {
                Some(&LayoutNode::Transition(id)) => {
                    let Some(at) = self.position_of(LayoutNode::Transition(id)) else {
                        continue;
                    };
                    (at, self.main_half(self.transition_size), Ok(id))
                }
                Some(LayoutNode::Place(_)) => continue,
                None => {
                    let (Some(at), Some(plan)) = (box_centres[holder - n], &plans[holder]) else {
                        continue;
                    };
                    (at, self.main_half(plan.size), Err(boxes[holder - n]))
                }
            };
            for (side, list) in [
                (GhostSide::Input, &slot.inputs),
                (GhostSide::Output, &slot.outputs),
            ] {
                for (k, &place) in list.iter().enumerate() {
                    let position = at + self.ghost_offset(half, side, k, list.len());
                    let found = match beside {
                        Ok(id) => self
                            .ghosts
                            .iter_mut()
                            .find(|(key, _)| *key == (place, id, side))
                            .map(|(_, at)| at),
                        Err(drawn) => self
                            .group_ghosts
                            .iter_mut()
                            .find(|(key, _)| *key == (place, drawn, side))
                            .map(|(_, at)| at),
                    };
                    if let Some(found) = found {
                        *found = position;
                    }
                }
            }
        }

        // The boxes of the subnet places, and the circles of their ports in
        // the order of what they are joined to, wherever that ended up.
        for (i, plan) in plans.iter().enumerate().take(n) {
            let (Some(plan), LayoutNode::Place(subnet)) = (plan, nodes[i]) else {
                continue;
            };
            let Some(centre) = self.position_of(nodes[i]) else {
                continue;
            };
            let across = |at: Point| match self.direction {
                Direction::TopDown => at.x,
                Direction::LeftRight => -at.y,
            };
            // Where the other end of a port's edge is: its transition, or
            // the box that hides it.
            let other_end = |edge: &Edge| {
                let transition = match edge.side {
                    GhostSide::Input => edge.b,
                    GhostSide::Output => edge.a,
                };
                match place[transition] {
                    Where::Hidden { at, .. } => box_centres[at - n],
                    _ => self.position_of_transition(edge.transition),
                }
            };
            let key = |port: PlaceId| {
                let (sum, count) = edges
                    .iter()
                    .filter(|e| e.place == port && !e.ghosted)
                    .filter_map(other_end)
                    .fold((0.0_f32, 0_u32), |(s, c), at| (s + across(at), c + 1));
                (count > 0).then(|| sum / count as f32)
            };
            // Stable, so ties keep the order given, and the ports joined to
            // nothing come last.
            let sorted = |list: &[PlaceId]| {
                let mut keyed: Vec<(Option<f32>, PlaceId)> =
                    list.iter().map(|&port| (key(port), port)).collect();
                keyed.sort_by(|a, b| match (a.0, b.0) {
                    (Some(x), Some(y)) => x.total_cmp(&y),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                });
                keyed.into_iter().map(|(_, port)| port).collect::<Vec<_>>()
            };
            let (before, after) = (sorted(&plan.before), sorted(&plan.after));
            let drawn = self.place_box(centre, &labels[i], plan, &before, &after);
            for circle in &drawn.ports {
                self.ports.insert(circle.place, circle.centre);
            }
            self.subnets.insert(subnet, drawn);
        }

        // The closed groups' boxes, and the groups in order.
        let mut closed_boxes: Vec<Option<GroupBox>> = vec![None; drawn_names.len()];
        for (j, &drawn) in boxes.iter().enumerate() {
            let i = n + j;
            let plan = plans[i]
                .clone()
                .unwrap_or_else(|| self.plan_box(&labels[i], &[]));
            let centre = box_centres[j].unwrap_or_default();
            let at = self.place_box(centre, &labels[i], &plan, &[], &[]);
            closed_boxes[drawn] = Some(GroupBox {
                centre: at.centre,
                size: at.size,
                corner_radius: at.corner_radius,
                title: at.title,
                count: at.count,
            });
        }
        self.groups = drawn_names
            .into_iter()
            .zip(drawn_closed)
            .enumerate()
            .map(|(d, (name, is_closed))| {
                let drawn = match (is_closed, closed_boxes[d].take()) {
                    (true, Some(at)) => Drawn::Closed(at),
                    _ => Drawn::Open(frames[d].unwrap_or_default()),
                };
                DrawnGroup { name, drawn }
            })
            .collect();
    }

    /// The plan of a subnet place's box titled `title`: which side each
    /// port goes on, and the size that makes room for them. A port with no
    /// side of its own joins the side with more ports, the side the ranks
    /// come from on a tie.
    fn plan_box(&self, title: &str, sides: &[(PlaceId, Option<BoxSide>)]) -> BoxPlan {
        let clear = |side: BoxSide| sides.iter().filter(|(_, s)| *s == Some(side)).count();
        let fallback = if clear(BoxSide::After) > clear(BoxSide::Before) {
            BoxSide::After
        } else {
            BoxSide::Before
        };
        let mut plan = BoxPlan::default();
        for &(port, side) in sides {
            plan.order.push(port);
            match side.unwrap_or(fallback) {
                BoxSide::Before => plan.before.push(port),
                BoxSide::After => plan.after.push(port),
            }
        }
        plan.size = self.box_size(title, plan.before.len(), plan.after.len());
        plan
    }

    /// The width of a port's circle and its count, side by side.
    fn port_cell(&self) -> f32 {
        2.0 * self.port_radius + PORT_COUNT_GAP + PORT_COUNT_WIDTH
    }

    /// The width and height of a subnet place's box titled `title`, with
    /// `before` ports on the side the ranks come from and `after` on the
    /// other. Top down, the ports are rows above and below the title and the
    /// count; left to right, columns beside them.
    fn box_size(&self, title: &str, before: usize, after: usize) -> Point {
        let r = self.port_radius;
        let cell = self.port_cell();
        let text = (title.chars().count() as f32 * PLACE_CHAR_WIDTH).max(COUNT_WIDTH);
        let run = |n: usize, each: f32| {
            if n == 0 {
                0.0
            } else {
                n as f32 * each + (n - 1) as f32 * PORT_SPACING
            }
        };
        match self.direction {
            Direction::TopDown => {
                let side = |n: usize| if n == 0 { 0.0 } else { 2.0 * r + BOX_GAP };
                Point::new(
                    run(before, cell).max(run(after, cell)).max(text) + 2.0 * BOX_PAD,
                    BOX_TITLE + BOX_COUNT + side(before) + side(after) + 2.0 * BOX_PAD,
                )
            }
            Direction::LeftRight => {
                let side = |n: usize| if n == 0 { 0.0 } else { cell + BOX_GAP };
                Point::new(
                    text + side(before) + side(after) + 2.0 * BOX_PAD,
                    run(before, 2.0 * r)
                        .max(run(after, 2.0 * r))
                        .max(BOX_TITLE + BOX_COUNT)
                        + 2.0 * BOX_PAD,
                )
            }
        }
    }

    /// A subnet place's box at `centre`, with the ports of each side in the
    /// order given here.
    fn place_box(
        &self,
        centre: Point,
        title: &str,
        plan: &BoxPlan,
        before: &[PlaceId],
        after: &[PlaceId],
    ) -> SubnetBox {
        let r = self.port_radius;
        let cell = self.port_cell();
        let half = plan.size / 2.0;
        let title_width = title.chars().count() as f32 * PLACE_CHAR_WIDTH;
        // From a circle's centre to the middle of its count's room.
        let beside = r + PORT_COUNT_GAP + PORT_COUNT_WIDTH / 2.0;
        let mut circles: Vec<PortCircle> = Vec::with_capacity(plan.order.len());
        let mut circle = |place: PlaceId, at: Point, count_at: Point| {
            circles.push(PortCircle {
                place,
                centre: at,
                radius: r,
                count: Bounds::from_center_half_size(
                    count_at,
                    Point::new(PORT_COUNT_WIDTH / 2.0, r),
                ),
            });
        };
        let (title_at, count_at) = match self.direction {
            Direction::TopDown => {
                // A row of circles, each with its count to its right,
                // centred across the box.
                let mut row = |list: &[PlaceId], y: f32| {
                    let n = list.len() as f32;
                    let extent = n * cell + (n - 1.0) * PORT_SPACING;
                    let mut x = centre.x - extent / 2.0 + r;
                    for &place in list {
                        circle(place, Point::new(x, y), Point::new(x + beside, y));
                        x += cell + PORT_SPACING;
                    }
                };
                let mut y = centre.y + half.y - BOX_PAD;
                if !before.is_empty() {
                    row(before, y - r);
                    y -= 2.0 * r + BOX_GAP;
                }
                let title_at = Point::new(centre.x, y - BOX_TITLE / 2.0);
                y -= BOX_TITLE;
                let count_at = Point::new(centre.x, y - BOX_COUNT / 2.0);
                y -= BOX_COUNT;
                if !after.is_empty() {
                    row(after, y - BOX_GAP - r);
                }
                (title_at, count_at)
            }
            Direction::LeftRight => {
                // A column of circles at each side, each with its count on
                // the side toward the middle, centred down the box.
                let mut column = |list: &[PlaceId], x: f32, toward: f32| {
                    let n = list.len() as f32;
                    let extent = n * 2.0 * r + (n - 1.0) * PORT_SPACING;
                    let mut y = centre.y + extent / 2.0 - r;
                    for &place in list {
                        circle(place, Point::new(x, y), Point::new(x + toward, y));
                        y -= 2.0 * r + PORT_SPACING;
                    }
                };
                let mut text_left = centre.x - half.x + BOX_PAD;
                if !before.is_empty() {
                    column(before, text_left + r, beside);
                    text_left += cell + BOX_GAP;
                }
                if !after.is_empty() {
                    column(after, centre.x + half.x - BOX_PAD - r, -beside);
                }
                let x = text_left + title_width.max(COUNT_WIDTH) / 2.0;
                (
                    Point::new(x, centre.y + BOX_COUNT / 2.0),
                    Point::new(x, centre.y - BOX_TITLE / 2.0),
                )
            }
        };
        // In the order the ports were given.
        circles.sort_by_key(|c| plan.order.iter().position(|&p| p == c.place));
        SubnetBox {
            centre,
            size: plan.size,
            corner_radius: BOX_PAD,
            title: Bounds::from_center_half_size(
                title_at,
                Point::new(title_width / 2.0, BOX_TITLE / 2.0),
            ),
            count: Bounds::from_center_half_size(
                count_at,
                Point::new(COUNT_WIDTH / 2.0, BOX_COUNT / 2.0),
            ),
            ports: circles,
        }
    }

    /// The clear room a place has between it and a transition in the next
    /// rank, at the usual gap between ranks, which a subnet place's box is
    /// given too.
    fn box_clearance(&self) -> f32 {
        let transition = match self.direction {
            Direction::TopDown => self.transition_size.y / 2.0,
            Direction::LeftRight => self.transition_size.x / 2.0,
        };
        (self.rank_sep - self.place_radius - transition).max(GHOST_CLEARANCE)
    }

    /// Turns one block's layers into coordinates, in a frame of its own,
    /// and works out the box around it.
    fn arrange(&self, block: &Block, layers: &Layers) -> Arranged {
        // The box of a subnet place or a closed group, if the node is one.
        // A closed group's box always has one, and no node of the net.
        let plan_of = |local: usize| block.plans[block.members[local]].as_ref();
        // How far a node reaches before and after its centre along the main
        // axis, ghosts included, and how wide its slot in a rank is.
        let main_half = |local: usize| match plan_of(local) {
            Some(plan) => self.main_half(plan.size),
            None => match block.nodes[block.members[local]] {
                LayoutNode::Place(_) => self.place_radius,
                LayoutNode::Transition(_) => self.main_half(self.transition_size),
            },
        };
        let slot_of = |local: usize| block.slots.get(&block.members[local]);
        let before = |local: usize| match slot_of(local) {
            Some(s) if !s.inputs.is_empty() => main_half(local) + self.ghost_depth(),
            _ => main_half(local),
        };
        let after = |local: usize| match slot_of(local) {
            Some(s) if !s.outputs.is_empty() => main_half(local) + self.ghost_depth(),
            _ => main_half(local),
        };
        let width = |local: usize| {
            let count = slot_of(local).map_or(0, |s| s.inputs.len().max(s.outputs.len()));
            let width = self.node_sep.max(count as f32 * self.ghost_sep);
            match plan_of(local) {
                // A box leaves its neighbours the room a place leaves them.
                Some(plan) => {
                    let across = match self.direction {
                        Direction::TopDown => plan.size.x,
                        Direction::LeftRight => plan.size.y,
                    };
                    width.max(across + (self.node_sep - 2.0 * self.place_radius).max(0.0))
                }
                None => width,
            }
        };
        let has_ghosts = |layer: &[usize]| layer.iter().any(|&l| slot_of(l).is_some());
        let has_box = |layer: &[usize]| layer.iter().any(|&l| plan_of(l).is_some());
        // The box a node needs around `at`: a subnet place's box holds its
        // title and count.
        let reach_of = |local: usize, at: Point| {
            let global = block.members[local];
            match plan_of(local) {
                Some(plan) => Bounds::from_center_half_size(at, plan.size / 2.0),
                None => self.reach(block.nodes[global], &block.names[global], at),
            }
        };
        // How far a node and its label reach to either side across the rank,
        // from its centre.
        let room = |local: usize| {
            let rect = reach_of(local, Point::ZERO);
            match self.direction {
                Direction::TopDown => (-rect.min.x, rect.max.x),
                // Across the rank is down the screen.
                Direction::LeftRight => (rect.max.y, -rect.min.y),
            }
        };
        let before_room = |local: usize| room(local).0;
        let after_room = |local: usize| room(local).1;

        let mut placed = Vec::with_capacity(block.members.len());
        let mut frame: Option<Bounds> = None;
        let mut main = 0.0_f32;
        for (rank, layer) in layers.layers.iter().enumerate() {
            if rank > 0 {
                let above = &layers.layers[rank - 1];
                let mut gap = self.rank_sep;
                let ghosts = has_ghosts(above) || has_ghosts(layer);
                let boxes = has_box(above) || has_box(layer);
                if ghosts || boxes {
                    let need = above.iter().map(|&l| after(l)).fold(0.0, f32::max)
                        + layer.iter().map(|&l| before(l)).fold(0.0, f32::max);
                    if ghosts {
                        gap = gap.max(need + GHOST_CLEARANCE);
                    }
                    if boxes {
                        gap = gap.max(need + self.box_clearance());
                    }
                }
                main += gap;
            }
            // Each node's slot is as wide as it needs; the centres are the
            // slots' centres, and the rank is centred on its middle centre.
            // With `fit_labels` a gap is also at least what the name beside
            // the node needs to its neighbour.
            let mut centres = Vec::with_capacity(layer.len());
            let mut x = 0.0_f32;
            for (order, &local) in layer.iter().enumerate() {
                if order > 0 {
                    let before = layer[order - 1];
                    let mut gap = (width(before) + width(local)) / 2.0;
                    if self.fit_labels {
                        gap = gap.max(after_room(before) + before_room(local) + LABEL_GAP);
                    }
                    x += gap;
                }
                centres.push(x);
            }
            let middle = centres.last().copied().unwrap_or(0.0) / 2.0;
            for (order, &local) in layer.iter().enumerate() {
                let cross = centres[order] - middle;
                let position = match self.direction {
                    Direction::TopDown => Point::new(cross, -main),
                    Direction::LeftRight => Point::new(main, -cross),
                };
                let global = block.members[local];
                let mut reach = reach_of(local, position);
                if let Some(slot) = slot_of(local) {
                    for (side, list) in [
                        (GhostSide::Input, &slot.inputs),
                        (GhostSide::Output, &slot.outputs),
                    ] {
                        for k in 0..list.len() {
                            let offset = self.ghost_offset(main_half(local), side, k, list.len());
                            reach = reach.union(self.ghost_reach(side, position + offset));
                        }
                    }
                }
                frame = Some(frame.map_or(reach, |f| f.union(reach)));
                placed.push((
                    global,
                    Placement {
                        rank,
                        order,
                        position,
                    },
                ));
            }
        }
        let mut bounds = frame.unwrap_or_default();
        // A back-edge bows out to one side: room for its apex.
        let side = self.bow_side();
        let position_of = |local: usize| {
            placed
                .iter()
                .find(|(i, _)| *i == block.members[local])
                .map_or(Point::ZERO, |(_, p)| p.position)
        };
        for (e, &(a, b)) in block.inside.iter().enumerate() {
            if layers.back[e] {
                let (a, b) = (position_of(a), position_of(b));
                bounds = bounds.union_point((a + b) / 2.0 + side * 0.5 * self.bow(a, b));
            }
        }
        Arranged {
            placed,
            bounds: bounds.inflate(FRAME_PAD),
        }
        .with_title_room()
    }

    /// Half of `size` along the main axis.
    fn main_half(&self, size: Point) -> f32 {
        match self.direction {
            Direction::TopDown => size.y / 2.0,
            Direction::LeftRight => size.x / 2.0,
        }
    }

    /// How far the middle of a ghost is from the edge of its transition's
    /// box, along the main axis: a gap long enough for an arrow, and the
    /// ghost's own radius.
    fn ghost_depth(&self) -> f32 {
        GHOST_GAP + self.ghost_radius * 2.0 + GHOST_LABEL
    }

    /// Where the `k`th of `count` ghosts on a side of a transition, or of a
    /// closed group's box, sits relative to its centre: spread across it, at
    /// a distance along the main axis that leaves room for an arrow beyond
    /// `half`, how far it reaches from its centre along that axis.
    fn ghost_offset(&self, half: f32, side: GhostSide, k: usize, count: usize) -> Point {
        let distance = half + GHOST_GAP + self.ghost_radius;
        let main = match side {
            GhostSide::Input => -distance,
            GhostSide::Output => distance,
        };
        let cross = (k as f32 - (count.saturating_sub(1)) as f32 / 2.0) * self.ghost_sep;
        match self.direction {
            Direction::TopDown => Point::new(cross, -main),
            Direction::LeftRight => Point::new(main, -cross),
        }
    }

    /// The box around a ghost centred at `at` and its name.
    fn ghost_reach(&self, side: GhostSide, at: Point) -> Bounds {
        let r = self.ghost_radius;
        let shape = Bounds::from_center_half_size(at, Point::new(r, r));
        let sep = self.ghost_sep / 2.0;
        match self.direction {
            // The name goes beyond the ghost, away from its transition.
            Direction::TopDown => {
                let out = match side {
                    GhostSide::Input => Point::new(0.0, 1.0),
                    GhostSide::Output => Point::new(0.0, -1.0),
                };
                shape.union(Bounds::from_center_half_size(
                    at + out * (r + GHOST_LABEL / 2.0),
                    Point::new(sep, GHOST_LABEL / 2.0),
                ))
            }
            // Ghosts stack vertically and the name goes above.
            Direction::LeftRight => shape.union(Bounds::from_center_half_size(
                at + Point::new(0.0, 1.0) * (r + GHOST_LABEL / 2.0),
                Point::new(sep, GHOST_LABEL / 2.0),
            )),
        }
    }

    /// The box a node needs around `at`: its shape, and the labels the
    /// renderer draws beside it, estimated from the name.
    fn reach(&self, node: LayoutNode, name: &str, at: Point) -> Bounds {
        let chars = name.chars().count() as f32;
        match node {
            LayoutNode::Place(_) => {
                let r = self.place_radius;
                let width = (chars * PLACE_CHAR_WIDTH).max(COUNT_WIDTH);
                let shape = Bounds::from_center_half_size(at, Point::new(r, r));
                match self.direction {
                    // Name and count to the left of the circle.
                    Direction::TopDown => shape.union(Bounds::from_corners(
                        Point::new(at.x - r - 10.0 - width, at.y - 18.0),
                        Point::new(at.x, at.y + 18.0),
                    )),
                    // Name above, count below, both centred.
                    Direction::LeftRight => shape.union(Bounds::from_corners(
                        Point::new(at.x - width / 2.0, at.y - r - 24.0),
                        Point::new(at.x + width / 2.0, at.y + r + 24.0),
                    )),
                }
            }
            LayoutNode::Transition(_) => {
                let half = self.transition_size / 2.0;
                let overflow = (chars * TRANSITION_CHAR_WIDTH / 2.0 - half.x).max(0.0);
                Bounds::from_center_half_size(at, half + Point::new(overflow, 0.0))
            }
        }
    }

    /// The side a back-edge bows out to: the side the ranks do not grow
    /// toward.
    pub fn bow_side(&self) -> Point {
        match self.direction {
            Direction::TopDown => Point::new(1.0, 0.0),
            Direction::LeftRight => Point::new(0.0, -1.0),
        }
    }

    /// How far the control point of a back-edge between `a` and `b` lies
    /// from the midpoint, to the [`bow_side`](Layout::bow_side).
    pub fn bow(&self, a: Point, b: Point) -> f32 {
        0.8 * self.node_sep + 0.2 * a.distance(b)
    }

    /// Where a node was placed, or `None` if the last computation did not
    /// see it. A port drawn in its subnet place's box is no node of the
    /// layout, so it has no rank or order of its own and this is `None` for
    /// it: its box has them, and [`position_of`](Layout::position_of) says
    /// where its circle is. A node hidden in a closed group is no node of
    /// the layout either, and has no placement, no position, no radius and
    /// no token positions: [`hidden_in`](Layout::hidden_in) names the group
    /// whose box stands for it.
    pub fn placement(&self, node: LayoutNode) -> Option<Placement> {
        self.nodes.get(&node).copied()
    }

    /// The centre of a node: for a subnet place, the centre of its box, and
    /// for a port drawn in one, the centre of the port's circle. `None` for
    /// a node hidden in a closed group, or not seen.
    pub fn position_of(&self, node: LayoutNode) -> Option<Point> {
        match self.nodes.get(&node) {
            Some(placement) => Some(placement.position),
            None => match node {
                LayoutNode::Place(place) => self.ports.get(&place).copied(),
                LayoutNode::Transition(_) => None,
            },
        }
    }

    /// The centre of a place: for a subnet place, the centre of its box, and
    /// for a port drawn in one, the centre of the port's circle.
    pub fn position_of_place(&self, place: PlaceId) -> Option<Point> {
        self.position_of(LayoutNode::Place(place))
    }

    /// The radius of a place's circle, where an arrow on it ends:
    /// [`place_radius`](Layout::place_radius) for an ordinary place and
    /// [`port_radius`](Layout::port_radius) for a port drawn in its subnet
    /// place's box. `None` for a subnet place, which is a box
    /// ([`subnet_box`](Layout::subnet_box)), and for a place the last
    /// computation did not place, such as one hidden in a closed group.
    pub fn radius_of_place(&self, place: PlaceId) -> Option<f32> {
        if self.ports.contains_key(&place) {
            Some(self.port_radius)
        } else if self.subnets.contains_key(&place) {
            None
        } else {
            self.nodes
                .contains_key(&LayoutNode::Place(place))
                .then_some(self.place_radius)
        }
    }

    /// Where the last computation drew a subnet place: its box and the
    /// circles of its ports. `None` for any other place.
    ///
    /// ```
    /// use petritype::{NetBuilder, TakeEdge};
    /// use petritype_layout::Layout;
    ///
    /// // A level whose lobby is its port.
    /// let mut b = NetBuilder::new();
    /// let lobby = b.place::<u32>("Lobby");
    /// b.port(lobby);
    /// let level = b.build()?;
    ///
    /// let mut b = NetBuilder::new();
    /// let door = b.place::<u32>("Door");
    /// let level_3 = b.subnet_place("Level 3", level);
    /// let lobby = b.port_of::<u32>(level_3, "Lobby");
    /// b.transition("Enter", (TakeEdge(door),), (lobby,), |n: u32| (n,));
    /// let world = b.build()?;
    ///
    /// let mut layout = Layout::default();
    /// layout.compute(&world);
    /// let drawn = layout.subnet_box(level_3.id()).expect("a subnet place");
    /// assert_eq!(layout.position_of_place(level_3.id()), Some(drawn.centre));
    /// // The arrow from `Enter` ends at the lobby's circle, inside the box.
    /// let circle = layout.position_of_place(lobby.id()).unwrap();
    /// assert!(drawn.bounds().contains(circle));
    /// assert_eq!(layout.radius_of_place(lobby.id()), Some(layout.port_radius));
    /// # Ok::<(), petritype::BuildError>(())
    /// ```
    pub fn subnet_box(&self, place: PlaceId) -> Option<&SubnetBox> {
        self.subnets.get(&place)
    }

    /// The centre of a transition.
    pub fn position_of_transition(&self, transition: TransitionId) -> Option<Point> {
        self.position_of(LayoutNode::Transition(transition))
    }

    /// Where the `index`th token of a place goes, or `None` past
    /// [`TOKEN_CAPACITY`] or for an unknown place.
    pub fn position_of_token(&self, place: PlaceId, index: usize) -> Option<Point> {
        Some(self.position_of_place(place)? + self.token_offset(index)?)
    }

    /// Where the `index`th token sits relative to its place's centre: the
    /// centre first, then a ring of 6 and a ring of 12, each starting at the
    /// top and going clockwise. `None` past [`TOKEN_CAPACITY`].
    pub fn token_offset(&self, index: usize) -> Option<Point> {
        // Neighbours on a ring sit one diameter and a hair apart.
        let ring = 2.0 * self.token_radius + 1.0;
        let (radius, slot, count) = match index {
            0 => return Some(Point::ZERO),
            1..=6 => (ring, index - 1, 6),
            7..TOKEN_CAPACITY => (2.0 * ring, index - 7, 12),
            _ => return None,
        };
        let angle =
            std::f32::consts::FRAC_PI_2 - slot as f32 * std::f32::consts::TAU / count as f32;
        Some(Point::new(angle.cos(), angle.sin()) * radius)
    }

    /// Whether the edge `from -> to` was set aside to break a cycle, and is
    /// therefore drawn curved.
    pub fn is_back_edge(&self, from: LayoutNode, to: LayoutNode) -> bool {
        self.back_edges.contains(&(from, to))
    }

    /// Every back-edge, as `(from, to)`, in no particular order.
    pub fn back_edges(&self) -> impl Iterator<Item = (LayoutNode, LayoutNode)> + '_ {
        self.back_edges.iter().copied()
    }

    /// Whether the last computation ghosted `place`: its edges take no part
    /// in the layout, and run to the ghosts beside its transitions and the
    /// boxes of closed groups instead of to the place itself. A ghosted
    /// place hidden in a closed group has no ghosts, and its edges to the
    /// rest of the net are its box's merged arrows.
    pub fn is_ghosted(&self, place: PlaceId) -> bool {
        self.ghosted.contains(&place)
    }

    /// Every ghost beside a transition of the last computation as
    /// `(place, transition, side)`, in the order the edges were declared.
    /// The ghosts beside closed groups' boxes are
    /// [`group_ghosts`](Layout::group_ghosts).
    pub fn ghosts(&self) -> impl Iterator<Item = (PlaceId, TransitionId, GhostSide)> + '_ {
        self.ghosts.iter().map(|(key, _)| *key)
    }

    /// The centre of the ghost of `place` on the `side` of `transition`,
    /// or `None` if there is no such ghost.
    pub fn ghost_position(
        &self,
        place: PlaceId,
        transition: TransitionId,
        side: GhostSide,
    ) -> Option<Point> {
        self.ghosts
            .iter()
            .find(|(key, _)| *key == (place, transition, side))
            .map(|(_, at)| *at)
    }

    /// Every ghost beside a closed group's box of the last computation as
    /// `(place, group, side)`, in the order the edges were declared: one on
    /// each side the place needs, for all its edges to the group's members.
    /// An input ghost stands for the place read inside the group, on the
    /// side the ranks come from, and an output ghost for the place written
    /// there, on the other side.
    pub fn group_ghosts(&self) -> impl Iterator<Item = (PlaceId, &str, GhostSide)> + '_ {
        self.group_ghosts
            .iter()
            .map(|&((place, group, side), _)| (place, self.groups[group].name.as_str(), side))
    }

    /// The centre of the ghost of `place` on the `side` of the box of the
    /// closed group called `group`, or `None` if there is no such ghost.
    pub fn group_ghost_position(
        &self,
        place: PlaceId,
        group: &str,
        side: GhostSide,
    ) -> Option<Point> {
        self.group_ghosts
            .iter()
            .find(|((p, g, s), _)| *p == place && *s == side && self.groups[*g].name == group)
            .map(|(_, at)| *at)
    }

    /// The names of the groups the last computation drew, open or closed,
    /// in order: the order the groups were given in (for
    /// [`compute`](Layout::compute), the order the net declares them in),
    /// then any other group a node named, in the order of its first node. A
    /// group no node is in has no frame and no box and is not listed, and
    /// neither is a group a [part](Layout::compute_subgraph) draws without a
    /// frame.
    pub fn group_names(&self) -> impl Iterator<Item = &str> + '_ {
        self.groups.iter().map(|g| g.name.as_str())
    }

    /// The frame of an open group, in world coordinates: its nodes, their
    /// labels and ghosts, the curve of its back-edges and a strip along the
    /// top for its title. `None` for a closed group, whose box is
    /// [`group_box`](Layout::group_box), and for a name that is no group
    /// drawn.
    pub fn group_bounds(&self, name: &str) -> Option<Bounds> {
        match self.drawn_group(name)? {
            Drawn::Open(bounds) => Some(*bounds),
            Drawn::Closed(_) => None,
        }
    }

    /// Whether the last computation drew the group called `name` closed,
    /// as one box. False for an open group and for a name that is no group
    /// drawn.
    pub fn is_group_closed(&self, name: &str) -> bool {
        self.group_box(name).is_some()
    }

    /// Where the last computation drew a closed group: its box, and the
    /// room it left for the title and the count. `None` for an open group
    /// and for a name that is no group drawn.
    ///
    /// ```
    /// use petritype::{NetBuilder, TakeEdge};
    /// use petritype_layout::{ArrowEnd, Layout, LayoutNode};
    ///
    /// // `Trail -> Hike -> Summit`, with the hike and the summit in `Walk`.
    /// let mut b = NetBuilder::new();
    /// let walk = b.group("Walk");
    /// let trail = b.place::<u32>("Trail");
    /// let summit = b.place::<u32>("Summit");
    /// b.in_group(walk, summit);
    /// let hike = b
    ///     .transition("Hike", (TakeEdge(trail),), (summit,), |n: u32| (n,))
    ///     .group(walk)
    ///     .id();
    /// let net = b.build()?;
    ///
    /// let mut layout = Layout::default().with_closed(["Walk"]);
    /// layout.compute(&net);
    /// let drawn = layout.group_box("Walk").expect("a closed group");
    /// assert!(drawn.bounds().contains(drawn.title.center()));
    /// // The hike and the summit are hidden in the box, and have no position.
    /// let hike = LayoutNode::Transition(hike);
    /// assert_eq!(layout.hidden_in(hike), Some("Walk"));
    /// assert_eq!(layout.position_of(hike), None);
    /// // The edge from the trail is drawn as an arrow to the box; the one
    /// // from the hike to the summit is inside it, and not drawn.
    /// let arrow = layout
    ///     .merged_arrow_for(LayoutNode::Place(trail.id()), hike)
    ///     .expect("an arrow");
    /// assert_eq!(arrow.to, ArrowEnd::Group("Walk".to_owned()));
    /// assert_eq!(layout.merged_arrows().count(), 1);
    /// # Ok::<(), petritype::BuildError>(())
    /// ```
    pub fn group_box(&self, name: &str) -> Option<&GroupBox> {
        match self.drawn_group(name)? {
            Drawn::Closed(at) => Some(at),
            Drawn::Open(_) => None,
        }
    }

    /// The group a node is drawn under: the open group whose frame holds it,
    /// or the closed group whose box hides it. In a layout of a whole net
    /// that is the group the node is in. `None` for a node drawn among the
    /// nodes in no group (in a [part](Layout::compute_subgraph), the nodes
    /// of the groups it draws without a frame too) and for a node the last
    /// computation did not see. A port of a subnet place is in its subnet
    /// place's group.
    pub fn group_of(&self, node: LayoutNode) -> Option<&str> {
        let &index = self.group_index.get(&node)?;
        Some(self.groups[index].name.as_str())
    }

    /// The closed group whose box hides a node, or `None` for a node drawn
    /// on its own or not seen. A port of a hidden subnet place is hidden
    /// with it. A hidden node has no [`placement`](Layout::placement) and
    /// no position. Its edges to the rest of the net are drawn as the box's
    /// [merged arrows](Layout::merged_arrows), and an edge with both ends in
    /// one closed group is not drawn at all.
    pub fn hidden_in(&self, node: LayoutNode) -> Option<&str> {
        let &index = self.group_index.get(&node)?;
        let group = &self.groups[index];
        matches!(group.drawn, Drawn::Closed(_)).then_some(group.name.as_str())
    }

    /// Every merged arrow of the last computation, in the order of the
    /// first edge each stands for: one per direction between a closed
    /// group's box and one node, or between two boxes. Every edge of the net
    /// with one end hidden, or with its ends hidden in two different closed
    /// groups, is in exactly one of them.
    pub fn merged_arrows(&self) -> impl Iterator<Item = &MergedArrow> + '_ {
        self.arrows.iter()
    }

    /// The merged arrow the edge `from -> to` of the net is drawn as, named
    /// by its own ends as [`is_back_edge`](Layout::is_back_edge) names it,
    /// or `None` for an edge drawn as itself, an edge inside one closed
    /// group, or no edge: what a token crossing that edge travels along.
    pub fn merged_arrow_for(&self, from: LayoutNode, to: LayoutNode) -> Option<&MergedArrow> {
        self.arrow_index.get(&(from, to)).map(|&k| &self.arrows[k])
    }

    /// The group drawn under this name, if any.
    fn drawn_group(&self, name: &str) -> Option<&Drawn> {
        self.groups
            .iter()
            .find(|g| g.name == name)
            .map(|g| &g.drawn)
    }

    /// How many ranks the last computation produced: in a layout with open
    /// groups, the most of any block.
    pub fn rank_count(&self) -> usize {
        self.ranks
    }

    /// Half the width and height of a node's shape: for a subnet place, its
    /// box, and for a port drawn in one, the port's circle. A node hidden in
    /// a closed group has no shape of its own, and this answers as for an
    /// ordinary place or a transition.
    pub fn half_extent(&self, node: LayoutNode) -> Point {
        match node {
            LayoutNode::Place(place) => {
                if let Some(drawn) = self.subnets.get(&place) {
                    drawn.size / 2.0
                } else if self.ports.contains_key(&place) {
                    Point::new(self.port_radius, self.port_radius)
                } else {
                    Point::new(self.place_radius, self.place_radius)
                }
            }
            LayoutNode::Transition(_) => self.transition_size / 2.0,
        }
    }

    /// The box around every node's shape, every closed group's box and
    /// every ghost, or `None` before anything was laid out. Labels and
    /// curved edges are outside it; whoever frames the drawing adds a margin
    /// for them (a layout with open groups has their frames for that).
    pub fn bounds(&self) -> Option<Bounds> {
        let mut out: Option<Bounds> = None;
        for (&node, placement) in &self.nodes {
            let half = self.half_extent(node);
            let rect = Bounds::from_center_half_size(placement.position, half);
            out = Some(match out {
                Some(r) => r.union(rect),
                None => rect,
            });
        }
        for group in &self.groups {
            if let Drawn::Closed(at) = &group.drawn {
                let rect = at.bounds();
                out = Some(out.map_or(rect, |r| r.union(rect)));
            }
        }
        let ghost_centres = self
            .ghosts
            .iter()
            .map(|(_, at)| *at)
            .chain(self.group_ghosts.iter().map(|(_, at)| *at));
        for at in ghost_centres {
            let rect =
                Bounds::from_center_half_size(at, Point::new(self.ghost_radius, self.ghost_radius));
            out = Some(out.map_or(rect, |r| r.union(rect)));
        }
        out
    }
}

/// The smallest box around some points.
fn bounding(points: &[Point]) -> Option<Bounds> {
    let first = *points.first()?;
    Some(
        points
            .iter()
            .fold(Bounds::from_corners(first, first), |r, &p| r.union_point(p)),
    )
}

/// An edge of the graph being laid out.
struct Edge {
    /// Where it starts and ends, by node index.
    a: usize,
    b: usize,
    /// The place it touches, the transition it touches, and which side of
    /// the transition the place is on.
    place: PlaceId,
    transition: TransitionId,
    side: GhostSide,
    /// Whether its place is ghosted, which takes it out of the graph.
    ghosted: bool,
}

impl Edge {
    /// Its ends as nodes, `(from, to)`: a port's own, not its box's.
    fn ends(&self) -> (LayoutNode, LayoutNode) {
        let (place, transition) = (
            LayoutNode::Place(self.place),
            LayoutNode::Transition(self.transition),
        );
        match self.side {
            GhostSide::Input => (place, transition),
            GhostSide::Output => (transition, place),
        }
    }
}

/// A subnet place's box before it is placed: its size, and which ports go
/// on which side.
#[derive(Debug, Clone, Default)]
struct BoxPlan {
    /// The box's width and height.
    size: Point,
    /// Its ports, in the order given.
    order: Vec<PlaceId>,
    /// The ports on the side the ranks come from, in the order given.
    before: Vec<PlaceId>,
    /// The ports on the other side, in the order given.
    after: Vec<PlaceId>,
}

/// The ghosts of one transition: the places, in edge order.
#[derive(Default)]
struct Slots {
    inputs: Vec<PlaceId>,
    outputs: Vec<PlaceId>,
}

/// What [`Layout::arrange`] needs to know about one block: an open group,
/// or the nodes in no group with the closed groups' boxes.
struct Block<'a> {
    /// Its nodes, by index among all nodes: past the nodes given, a closed
    /// group's box.
    members: &'a [usize],
    /// The label of every node and box.
    names: &'a [String],
    /// The nodes given, without the boxes.
    nodes: &'a [LayoutNode],
    slots: &'a HashMap<usize, Slots>,
    /// The edges inside it, by index among `members`.
    inside: &'a [(usize, usize)],
    /// The box of each subnet place and closed group, by index among all
    /// nodes.
    plans: &'a [Option<BoxPlan>],
}

/// One block of nodes, laid out in a frame of its own.
#[derive(Debug, Default)]
struct Arranged {
    /// Each node of the block, by its index among all nodes, and where it
    /// went.
    placed: Vec<(usize, Placement)>,
    /// The box around it, padded for its frame.
    bounds: Bounds,
}

impl Arranged {
    /// The same, with a strip along the top for a group's title. The block
    /// of the nodes in no group has no title but keeps the strip, so that
    /// every block is packed alike.
    fn with_title_room(mut self) -> Self {
        self.bounds.max.y += TITLE_ROOM;
        self
    }
}

/// The position of `name` in `list`, pushed on the end if it is not there.
fn position_or_push(list: &mut Vec<String>, name: &str) -> usize {
    match list.iter().position(|n| n == name) {
        Some(at) => at,
        None => {
            list.push(name.to_owned());
            list.len() - 1
        }
    }
}

/// The rule a [`Layout`] packs its open groups with, after the nodes in no
/// group. It is public so that a picture of the same net in other units,
/// such as a thumbnail's grid of cells, can pack the groups alike.
///
/// Packs boxes of the given sizes in rows, in the order given, with `gap`
/// between neighbours both across and down, and returns the top-left corner
/// of each box, in the same order, in world coordinates (y up). The first
/// row starts at the origin and the rows go down, so no corner has a `y`
/// above zero.
///
/// The row width is the one that makes the whole frame largest on a screen
/// 1.6 times as wide as high, that is, the one minimising the larger of its
/// width over 1.6 and its height, then its area. The candidates are the
/// widths of every run of consecutive boxes, and the first of equal
/// candidates wins, so the result depends on the sizes and the gap alone.
///
/// ```
/// use petritype_layout::{Point, pack};
///
/// // Four unit squares one apart pack two by two.
/// let corners = pack(&[Point::new(1.0, 1.0); 4], 1.0);
/// let want = [(0.0, 0.0), (2.0, 0.0), (0.0, -2.0), (2.0, -2.0)].map(|(x, y)| Point::new(x, y));
/// assert_eq!(corners, want);
/// ```
pub fn pack(sizes: &[Point], gap: f32) -> Vec<Point> {
    let widest = sizes.iter().fold(0.0_f32, |w, s| w.max(s.x));
    let mut best: Option<(f32, f32, Vec<Point>)> = None;
    for start in 0..sizes.len() {
        let mut limit = -gap;
        for size in &sizes[start..] {
            limit += gap + size.x;
            if limit < widest {
                continue;
            }
            let (corners, width, height) = pack_rows(sizes, limit, gap);
            let score = (width / LANDSCAPE).max(height);
            let better = best
                .as_ref()
                .is_none_or(|(s, a, _)| (score, width * height) < (*s, *a));
            if better {
                best = Some((score, width * height, corners));
            }
        }
    }
    best.map_or_else(
        || pack_rows(sizes, widest, gap).0,
        |(_, _, corners)| corners,
    )
}

/// Fills rows no wider than `limit` (a row always takes at least one box),
/// and returns the corners and the width and height of the whole.
fn pack_rows(sizes: &[Point], limit: f32, gap: f32) -> (Vec<Point>, f32, f32) {
    let mut corners = Vec::with_capacity(sizes.len());
    let (mut x, mut y, mut row_height, mut width) = (0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32);
    for size in sizes {
        if x > 0.0 && x + size.x > limit {
            y += row_height + gap;
            x = 0.0;
            row_height = 0.0;
        }
        corners.push(Point::new(x, -y));
        x += size.x + gap;
        width = width.max(x - gap);
        row_height = row_height.max(size.y);
    }
    (corners, width, y + row_height)
}

/// What the layering passes produce, over node indices.
struct Layers {
    /// The nodes of each rank, in order.
    layers: Vec<Vec<usize>>,
    /// For each edge, whether it is a back-edge.
    back: Vec<bool>,
}

/// Cycle breaking, ranking and ordering over a plain digraph of `n` nodes,
/// where a node's index is its declaration order.
fn layer(n: usize, edges: &[(usize, usize)]) -> Layers {
    let mut out_edges: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut has_input = vec![false; n];
    for (e, &(a, b)) in edges.iter().enumerate() {
        out_edges[a].push(e);
        has_input[b] = true;
    }

    // 1. Depth-first search from the sources, then from anything a source
    //    does not reach (a cycle with no way in), in declaration order.
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        New,
        OnStack,
        Done,
    }
    let mut mark = vec![Mark::New; n];
    let mut back = vec![false; edges.len()];
    let mut preorder = Vec::with_capacity(n);
    let roots = (0..n)
        .filter(|&v| !has_input[v])
        .chain(0..n)
        .collect::<Vec<_>>();
    for root in roots {
        if mark[root] != Mark::New {
            continue;
        }
        mark[root] = Mark::OnStack;
        preorder.push(root);
        let mut stack = vec![(root, 0_usize)];
        while let Some((node, next)) = stack.last_mut() {
            let node = *node;
            if let Some(&e) = out_edges[node].get(*next) {
                *next += 1;
                let target = edges[e].1;
                match mark[target] {
                    Mark::New => {
                        mark[target] = Mark::OnStack;
                        preorder.push(target);
                        stack.push((target, 0));
                    }
                    Mark::OnStack => back[e] = true,
                    Mark::Done => {}
                }
            } else {
                mark[node] = Mark::Done;
                stack.pop();
            }
        }
    }

    // 2. Longest path over the forward edges, which form a DAG now.
    let mut indegree = vec![0_usize; n];
    for (e, &(_, b)) in edges.iter().enumerate() {
        if !back[e] {
            indegree[b] += 1;
        }
    }
    let mut rank = vec![0_usize; n];
    let mut queue: std::collections::VecDeque<usize> =
        (0..n).filter(|&v| indegree[v] == 0).collect();
    while let Some(node) = queue.pop_front() {
        for &e in &out_edges[node] {
            if back[e] {
                continue;
            }
            let target = edges[e].1;
            rank[target] = rank[target].max(rank[node] + 1);
            indegree[target] -= 1;
            if indegree[target] == 0 {
                queue.push_back(target);
            }
        }
    }

    // 3. Start from the search's discovery order, then sweep.
    let ranks = rank.iter().max().map_or(0, |&r| r + 1);
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); ranks];
    for &node in &preorder {
        layers[rank[node]].push(node);
    }
    let mut neighbours: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (e, &(a, b)) in edges.iter().enumerate() {
        if !back[e] {
            neighbours[a].push(b);
            neighbours[b].push(a);
        }
    }
    let mut cross = vec![0.0_f32; n];
    for layer in &layers {
        let last = layer.len().saturating_sub(1) as f32;
        for (i, &node) in layer.iter().enumerate() {
            cross[node] = i as f32 - last / 2.0;
        }
    }
    for _ in 0..SWEEPS {
        // Down: each rank by its neighbours above. Up: by those below.
        for (r, layer) in layers.iter_mut().enumerate().skip(1) {
            reorder(layer, &neighbours, &rank, &mut cross, |nr| nr < r);
        }
        for (r, layer) in layers.iter_mut().enumerate().rev().skip(1) {
            reorder(layer, &neighbours, &rank, &mut cross, |nr| nr > r);
        }
    }

    Layers { layers, back }
}

/// Sorts one rank by the barycentre of each node's neighbours in the ranks
/// `side` admits, and updates the cross positions to match. A node with no
/// such neighbour keeps its current position as its key, and ties keep their
/// current order: the sort is stable.
fn reorder(
    layer: &mut [usize],
    neighbours: &[Vec<usize>],
    rank: &[usize],
    cross: &mut [f32],
    side: impl Fn(usize) -> bool,
) {
    let key = |node: usize| {
        let (sum, count) = neighbours[node]
            .iter()
            .filter(|&&m| side(rank[m]))
            .fold((0.0_f32, 0_u32), |(s, c), &m| (s + cross[m], c + 1));
        if count == 0 {
            cross[node]
        } else {
            sum / count as f32
        }
    };
    let mut keyed: Vec<(f32, usize)> = layer.iter().map(|&node| (key(node), node)).collect();
    keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
    let last = layer.len().saturating_sub(1) as f32;
    for (i, (_, node)) in keyed.into_iter().enumerate() {
        layer[i] = node;
        cross[node] = i as f32 - last / 2.0;
    }
}

#[cfg(test)]
mod tests {
    use petritype::{GroupRef, NetBuilder, PlaceRef, TakeEdge};

    use super::*;

    /// `A/In -> A/Go -> A/Out -> B/Go -> B/Out`, and `Lone`: names holding a
    /// `/`, which means nothing, in a net with no groups.
    fn net() -> Net {
        let mut b = NetBuilder::new();
        let a_in = b.place::<u32>("A/In");
        let a_out = b.place::<u32>("A/Out");
        let b_out = b.place::<u32>("B/Out");
        let _lone = b.place::<u32>("Lone");
        b.transition("A/Go", (TakeEdge(a_in),), (a_out,), |n: u32| (n,));
        b.transition("B/Go", (TakeEdge(a_out),), (b_out,), |n: u32| (n,));
        b.build().expect("the net builds")
    }

    /// `In -> Go -> Out` in group `A`, `Out -> Go B -> Out B` with `Go B`
    /// and `Out B` in `B`, and `Lone` in no group. `B` is declared before
    /// `A`, though `A`'s nodes come first.
    fn grouped() -> Net {
        let mut b = NetBuilder::new();
        let group_b = b.group("B");
        let group_a = b.group("A");
        let a_in = b.place::<u32>("In");
        let a_out = b.place::<u32>("Out");
        let b_out = b.place::<u32>("Out B");
        let _lone = b.place::<u32>("Lone");
        b.in_group(group_a, a_in);
        b.in_group(group_a, a_out);
        b.in_group(group_b, b_out);
        b.transition("Go", (TakeEdge(a_in),), (a_out,), |n: u32| (n,))
            .group(group_a);
        b.transition("Go B", (TakeEdge(a_out),), (b_out,), |n: u32| (n,))
            .group(group_b);
        b.build().expect("the net builds")
    }

    fn laid_out_open(net: &Net) -> Layout {
        let mut layout = Layout::default();
        layout.compute(net);
        layout
    }

    #[test]
    fn a_name_is_never_split_and_a_net_with_no_groups_has_no_frames() {
        let net = net();
        let plain = laid_out_open(&net);
        assert_eq!(plain.group_names().count(), 0);
        for (id, _) in net.places() {
            assert_eq!(plain.group_of(LayoutNode::Place(id)), None);
            assert_eq!(plain.hidden_in(LayoutNode::Place(id)), None);
        }
        assert_eq!(plain.merged_arrows().count(), 0);
        // `compute_from` has no names and no groups, whatever is closed.
        let (id, _) = net.places().next().unwrap();
        let mut nameless = Layout::default().with_closed(["A", "B"]);
        nameless.compute_from(
            net.places().map(|(id, _)| id),
            net.transitions().map(|(id, _)| id),
            net.edges(),
        );
        assert_eq!(nameless.group_names().count(), 0);
        assert_eq!(nameless.position_of_place(id), plain.position_of_place(id));
    }

    #[test]
    fn groups_come_in_declared_order_and_hold_their_nodes() {
        let net = grouped();
        let layout = laid_out_open(&net);
        // Declared order, not the order of first use.
        assert_eq!(layout.group_names().collect::<Vec<_>>(), ["B", "A"]);
        for (id, info) in net.places() {
            assert_eq!(layout.group_of(LayoutNode::Place(id)), info.group);
        }
        for (id, info) in net.transitions() {
            assert_eq!(layout.group_of(LayoutNode::Transition(id)), info.group);
        }
        assert!(layout.group_bounds("nope").is_none());
        assert!(!layout.is_group_closed("A"));
    }

    #[test]
    fn each_open_group_is_ranked_by_its_own_edges_and_frames_its_nodes() {
        let net = grouped();
        let layout = laid_out_open(&net);
        for (id, info) in net.places() {
            let node = LayoutNode::Place(id);
            let Some(group) = info.group else {
                continue;
            };
            let frame = layout.group_bounds(group).unwrap();
            assert!(frame.contains(layout.position_of(node).unwrap()));
        }
        // Out -> Go B crosses frames, so Go B is a source of its own group:
        // rank 0, not 2.
        let go_b = net.transition_named("Go B").unwrap();
        assert_eq!(
            layout.placement(LayoutNode::Transition(go_b)).unwrap().rank,
            0
        );
    }

    #[test]
    fn frames_and_the_block_of_the_rest_do_not_overlap_and_keep_the_gap() {
        let net = grouped();
        let layout = laid_out_open(&net);
        let frames: Vec<Bounds> = layout
            .group_names()
            .map(|n| layout.group_bounds(n).unwrap())
            .collect();
        for (i, a) in frames.iter().enumerate() {
            for b in &frames[i + 1..] {
                let apart_x = a.max.x + layout.group_gap - 0.01 <= b.min.x
                    || b.max.x + layout.group_gap - 0.01 <= a.min.x;
                let apart_y = a.max.y + layout.group_gap - 0.01 <= b.min.y
                    || b.max.y + layout.group_gap - 0.01 <= a.min.y;
                assert!(apart_x || apart_y, "{a:?} {b:?}");
            }
        }
        // `Lone` is in no group: no frame, and clear of every frame by the
        // gap and the padding the block keeps around it.
        let lone = LayoutNode::Place(net.place_named("Lone").unwrap());
        assert_eq!(layout.group_of(lone), None);
        let at = Bounds::from_center_half_size(
            layout.position_of(lone).unwrap(),
            layout.half_extent(lone),
        )
        .inflate(layout.group_gap + FRAME_PAD - 0.01);
        for frame in frames {
            assert!(
                at.max.x <= frame.min.x
                    || frame.max.x <= at.min.x
                    || at.max.y <= frame.min.y
                    || frame.max.y <= at.min.y,
                "{at:?} {frame:?}"
            );
        }
    }

    #[test]
    fn packing_is_deterministic_and_tends_to_landscape() {
        // Nine equal boxes pack to three by three or wider, never a column.
        let sizes = vec![Point::new(100.0, 100.0); 9];
        let corners = pack(&sizes, 10.0);
        assert_eq!(corners, pack(&sizes, 10.0));
        let width = corners.iter().map(|c| c.x + 100.0).fold(0.0, f32::max);
        let height = corners.iter().map(|c| 100.0 - c.y).fold(0.0, f32::max);
        assert!(width >= height * 0.9, "{width} x {height}");
        assert!(width <= height * 3.0, "{width} x {height}");
    }

    #[test]
    fn groups_by_name_lay_out_as_the_net_does() {
        let net = grouped();
        let by_net = laid_out_open(&net);
        let mut by_name = Layout::default();
        by_name.compute_named(
            net.places().map(|(id, i)| {
                let place = LayoutPlace::new(id, i.name, PlaceKind::Ordinary);
                match i.group {
                    Some(group) => place.in_group(group),
                    None => place,
                }
            }),
            net.transitions().map(|(id, i)| {
                let transition = LayoutTransition::new(id, i.name);
                match i.group {
                    Some(group) => transition.in_group(group),
                    None => transition,
                }
            }),
            net.edges(),
            &["B", "A"],
        );
        assert_eq!(by_name, by_net);
    }
    /// Two transitions each read the hub `H` and one private place; `Wide`
    /// reads three more hubs.
    fn hubs() -> Net {
        let mut b = NetBuilder::new();
        let h = b.place::<u32>("H");
        let h2 = b.place::<u32>("H2");
        let h3 = b.place::<u32>("H3");
        let x = b.place::<u32>("X");
        let y = b.place::<u32>("Y");
        let out = b.place::<u32>("Out");
        b.transition(
            "T1",
            (TakeEdge(h), TakeEdge(x)),
            (out,),
            |a: u32, _: u32| (a,),
        );
        b.transition(
            "T2",
            (TakeEdge(h), TakeEdge(y)),
            (out,),
            |a: u32, _: u32| (a,),
        );
        b.transition(
            "Wide",
            (TakeEdge(h), TakeEdge(h2), TakeEdge(h3)),
            (out,),
            |a: u32, _: u32, _: u32| (a,),
        );
        b.build().expect("the net builds")
    }

    fn id_of(net: &Net, name: &str) -> PlaceId {
        net.places().find(|(_, info)| info.name == name).unwrap().0
    }

    fn transition_of(net: &Net, name: &str) -> TransitionId {
        net.transitions()
            .find(|(_, info)| info.name == name)
            .unwrap()
            .0
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

    #[test]
    fn a_ghost_sits_beside_its_transition_on_the_side_of_its_edge() {
        let net = hubs();
        let mut layout = Layout::default().with_ghosting(Ghosting::named(["H", "Out"]));
        layout.compute(&net);
        let (h, out) = (id_of(&net, "H"), id_of(&net, "Out"));
        assert!(layout.is_ghosted(h) && !layout.is_ghosted(id_of(&net, "X")));
        let t1 = transition_of(&net, "T1");
        let at = layout.position_of_transition(t1).unwrap();
        let above = layout.ghost_position(h, t1, GhostSide::Input).unwrap();
        let below = layout.ghost_position(out, t1, GhostSide::Output).unwrap();
        // Ranks grow downward: inputs above, outputs below.
        assert!(
            above.y > at.y && below.y < at.y,
            "{above:?} {at:?} {below:?}"
        );
        assert!(layout.ghost_position(h, t1, GhostSide::Output).is_none());
        // One ghost per (place, transition, side): H x 3, Out x 3.
        assert_eq!(layout.ghosts().count(), 6);
        // The home nodes stay.
        assert!(layout.position_of_place(h).is_some());
    }

    #[test]
    fn ghosted_edges_do_not_rank_or_order() {
        let net = hubs();
        let plain = {
            let mut l = Layout::default();
            l.compute(&net);
            l
        };
        let ghosted = {
            let mut l = Layout::default().with_ghosting(Ghosting::named(["H", "H2", "H3"]));
            l.compute(&net);
            l
        };
        let t1 = LayoutNode::Transition(transition_of(&net, "T1"));
        let wide = LayoutNode::Transition(transition_of(&net, "Wide"));
        // Through the hub, T1 is one rank below it; without the hub's
        // edges it is read from X alone, which is a source too.
        assert_eq!(plain.placement(t1).unwrap().rank, 1);
        assert_eq!(ghosted.placement(t1).unwrap().rank, 1);
        // `Wide` reads nothing but hubs: a source of its own.
        assert_eq!(plain.placement(wide).unwrap().rank, 1);
        assert_eq!(ghosted.placement(wide).unwrap().rank, 0);
        // The hub is isolated: rank 0, whatever read it.
        let h = LayoutNode::Place(id_of(&net, "H"));
        assert_eq!(ghosted.placement(h).unwrap().rank, 0);
    }

    #[test]
    fn a_transition_with_many_ghosts_gets_room_for_them() {
        let net = hubs();
        let mut layout = Layout::default().with_ghosting(Ghosting::named(["H", "H2", "H3"]));
        layout.compute(&net);
        let wide = transition_of(&net, "Wide");
        let xs: Vec<f32> = ["H", "H2", "H3"]
            .iter()
            .map(|n| {
                layout
                    .ghost_position(id_of(&net, n), wide, GhostSide::Input)
                    .unwrap()
                    .x
            })
            .collect();
        // Spread across the transition, one `ghost_sep` apart, in order.
        assert!((xs[1] - xs[0] - layout.ghost_sep).abs() < 0.01);
        assert!((xs[2] - xs[1] - layout.ghost_sep).abs() < 0.01);
        // Neighbours in its rank are clear of the whole row.
        let at = layout.position_of_transition(wide).unwrap();
        for other in ["T1", "T2"] {
            let o = layout
                .position_of_transition(transition_of(&net, other))
                .unwrap();
            if (o.y - at.y).abs() < 1.0 {
                assert!((o.x - at.x).abs() >= 3.0 * layout.ghost_sep, "{o:?} {at:?}");
            }
        }
        // And the bounds hold every ghost.
        let bounds = layout.bounds().unwrap();
        for (p, t, s) in layout.ghosts() {
            assert!(bounds.contains(layout.ghost_position(p, t, s).unwrap()));
        }
    }

    #[test]
    fn a_subgraph_is_one_drawing_and_counts_degrees_over_the_whole() {
        let net = grouped();
        let a_out = id_of(&net, "Out");
        let go_b = transition_of(&net, "Go B");
        let b_out = id_of(&net, "Out B");
        let edges: Vec<EdgeInfo> = net
            .edges()
            .filter(|e| matches!(e, EdgeInfo::Input { transition, .. } | EdgeInfo::Output { transition, .. } if *transition == go_b))
            .collect();
        // Out has two edges in the whole net (written by Go, read by Go B)
        // but only one in the part.
        let mut layout = Layout::default().with_ghosting(Ghosting::default().with_min_degree(2));
        layout.compute_subgraph(
            net.places()
                .filter(|(id, _)| [a_out, b_out].contains(id))
                .map(LayoutPlace::from),
            net.transitions()
                .filter(|(id, _)| *id == go_b)
                .map(LayoutTransition::from),
            edges,
            net.edges(),
            &["B", "A"],
            None,
        );
        // A part taken for no group draws no frame, though its nodes are in
        // two groups.
        assert_eq!(layout.group_names().count(), 0);
        assert_eq!(layout.group_of(LayoutNode::Place(b_out)), None);
        assert!(layout.is_ghosted(a_out));
        assert!(!layout.is_ghosted(b_out));
    }

    #[test]
    fn an_empty_net_lays_out_even_with_groups_closed() {
        let net = NetBuilder::new().build().expect("an empty net builds");
        let mut layout = Layout::default().with_closed(["A"]);
        layout.compute(&net);
        assert_eq!(layout.group_names().count(), 0);
        assert!(layout.bounds().is_none());
        layout.compute_named(
            Vec::<LayoutPlace>::new(),
            Vec::<LayoutTransition>::new(),
            [],
            &["A"],
        );
        assert_eq!(layout.group_names().count(), 0);
        layout.compute_subgraph(
            Vec::<LayoutPlace>::new(),
            Vec::<LayoutTransition>::new(),
            [],
            [],
            &[],
            Some(&["A"]),
        );
    }

    #[test]
    fn a_node_is_laid_out_under_its_whole_name() {
        // Two places of the group `Def` whose names begin with `Def/`: no
        // character of a name means anything, so with `fit_labels` the room
        // between them is that of the whole names, and the group is the one
        // declared.
        let mut b = NetBuilder::new();
        let def = b.group("Def");
        let one = b.place::<u32>("Def/Board one");
        let two = b.place::<u32>("Def/Board two");
        let out = b.place::<u32>("Def/Out");
        for p in [one, two, out] {
            b.in_group(def, p);
        }
        b.transition(
            "Def/Play",
            (TakeEdge(one), TakeEdge(two)),
            (out,),
            |a: u32, _: u32| (a,),
        )
        .group(def);
        let net = b.build().expect("the net builds");
        let mut layout = Layout {
            fit_labels: true,
            ..Layout::default()
        };
        layout.compute(&net);
        assert_eq!(layout.group_names().collect::<Vec<_>>(), ["Def"]);
        let a = layout.position_of_place(one.id()).unwrap();
        let b = layout.position_of_place(two.id()).unwrap();
        // The circle to the left, the right one's whole name and count to
        // its left, and the gap between labels.
        let whole = "Def/Board two".len() as f32 * PLACE_CHAR_WIDTH;
        let need = 2.0 * layout.place_radius + 10.0 + whole + LABEL_GAP;
        assert!((b.x - a.x).abs() >= need - 0.01, "{a:?} {b:?}");
        assert!(
            need > layout.node_sep,
            "more than the name without its start"
        );
    }

    /// `Go` reads `In` and the hub `Hub`, and writes `Out`: all in `A` but
    /// the hub, in `B`.
    fn foreign_hub() -> Net {
        let mut b = NetBuilder::new();
        let a = b.group("A");
        let group_b = b.group("B");
        let a_in = b.place::<u32>("In");
        let hub = b.place::<u32>("Hub");
        let a_out = b.place::<u32>("Out");
        b.in_group(a, a_in);
        b.in_group(group_b, hub);
        b.in_group(a, a_out);
        b.transition(
            "Go",
            (TakeEdge(a_in), TakeEdge(hub)),
            (a_out,),
            |n: u32, _: u32| (n,),
        )
        .group(a);
        b.build().expect("the net builds")
    }

    /// Lays out the whole of `net` as a part taken for `focus`.
    fn part_for(layout: &mut Layout, net: &Net, focus: &[&str]) {
        let groups: Vec<&str> = net.groups().collect();
        layout.compute_subgraph(
            net.places().map(LayoutPlace::from),
            net.transitions().map(LayoutTransition::from),
            net.edges(),
            net.edges(),
            &groups,
            Some(focus),
        );
    }

    #[test]
    fn a_focused_part_drops_the_homes_of_other_groups_hubs() {
        let net = foreign_hub();
        let mut layout = Layout::default().with_ghosting(Ghosting::named(["Hub"]));
        part_for(&mut layout, &net, &["A"]);
        let (a_in, hub) = (id_of(&net, "In"), id_of(&net, "Hub"));
        let go = transition_of(&net, "Go");
        // The hub belongs to another group: no home node, but its ghost.
        assert!(layout.position_of_place(hub).is_none());
        assert_eq!(layout.hidden_in(LayoutNode::Place(hub)), None);
        assert!(layout.ghost_position(hub, go, GhostSide::Input).is_some());
        assert!(layout.position_of_place(a_in).is_some());
        // A focus of one is a plain drawing.
        assert_eq!(layout.group_names().count(), 0);
        // A hub of the focused group keeps its home.
        part_for(&mut layout, &net, &["B"]);
        assert!(layout.position_of_place(hub).is_some());
    }

    /// `Go` reads `In`, the hubs `Hub` and `Spare` and the plain `Side`,
    /// and writes `Out`: `In`, `Out` and `Go` in `A`, and each of the others
    /// in a group of its own, `B`, `C` and `D`.
    fn four_groups() -> Net {
        let mut b = NetBuilder::new();
        let [a, group_b, c, d] = ["A", "B", "C", "D"].map(|g| b.group(g));
        let a_in = b.place::<u32>("In");
        let hub = b.place::<u32>("Hub");
        let spare = b.place::<u32>("Spare");
        let side = b.place::<u32>("Side");
        let a_out = b.place::<u32>("Out");
        let put = |b: &mut NetBuilder, g: GroupRef, p: PlaceRef<u32>| b.in_group(g, p);
        put(&mut b, a, a_in);
        put(&mut b, group_b, hub);
        put(&mut b, c, spare);
        put(&mut b, d, side);
        put(&mut b, a, a_out);
        b.transition(
            "Go",
            (
                TakeEdge(a_in),
                TakeEdge(hub),
                TakeEdge(spare),
                TakeEdge(side),
            ),
            (a_out,),
            |n: u32, _: u32, _: u32, _: u32| (n,),
        )
        .group(a);
        b.build().expect("the net builds")
    }

    #[test]
    fn a_part_for_two_groups_keeps_both_frames_and_applies_the_ghosting_rules_to_the_set() {
        let net = four_groups();
        let mut layout = Layout::default().with_ghosting(Ghosting::named(["Hub", "Spare"]));
        part_for(&mut layout, &net, &["A", "B"]);
        let [a_in, hub, spare, side] = ["In", "Hub", "Spare", "Side"].map(|n| id_of(&net, n));
        let go = transition_of(&net, "Go");
        // Each member keeps its frame; the rest share a block with none.
        assert_eq!(layout.group_names().collect::<Vec<_>>(), ["A", "B"]);
        assert!(layout.group_bounds("D").is_none());
        assert_eq!(layout.group_of(LayoutNode::Place(a_in)), Some("A"));
        assert_eq!(layout.group_of(LayoutNode::Place(hub)), Some("B"));
        assert_eq!(layout.group_of(LayoutNode::Place(side)), None);
        // A ghosted hub inside the set keeps its home, one outside has none.
        assert!(layout.position_of_place(hub).is_some());
        assert!(layout.position_of_place(spare).is_none());
        assert!(layout.ghost_position(spare, go, GhostSide::Input).is_some());
        assert!(layout.ghost_position(hub, go, GhostSide::Input).is_some());
        // A plain place of another group is drawn among the rest.
        assert!(layout.position_of_place(side).is_some());
        // Closed, a group of the rest is a box among them; the focused
        // groups stay open whatever is closed.
        layout.closed = ["A", "D"].map(str::to_owned).into();
        part_for(&mut layout, &net, &["A", "B"]);
        assert_eq!(layout.group_names().collect::<Vec<_>>(), ["A", "B", "D"]);
        assert!(layout.group_bounds("A").is_some());
        assert_eq!(layout.hidden_in(LayoutNode::Place(side)), Some("D"));
        assert!(layout.group_box("D").is_some());
    }

    #[test]
    fn fitting_labels_spaces_a_rank_by_the_names_beside_its_nodes() {
        let mut b = NetBuilder::new();
        let long = "A very long place name indeed";
        let p1 = b.place::<u32>(long);
        let p2 = b.place::<u32>(long.to_owned() + "!");
        let out = b.place::<u32>("Out");
        b.transition(
            "T",
            (TakeEdge(p1), TakeEdge(p2)),
            (out,),
            |a: u32, _: u32| (a,),
        );
        let net = b.build().expect("the net builds");
        let gap = |layout: &mut Layout| {
            layout.compute(&net);
            let a = layout.position_of_place(p1.id()).unwrap();
            let b = layout.position_of_place(p2.id()).unwrap();
            (a.x - b.x).abs()
        };
        let mut layout = Layout::default();
        assert!(layout.fit_labels, "names fit by default");
        layout.fit_labels = false;
        assert_eq!(gap(&mut layout), layout.node_sep);
        layout.fit_labels = true;
        // The left node's label sits left of the right node: room for it.
        let needed = long.len() as f32 * PLACE_CHAR_WIDTH;
        assert!(gap(&mut layout) > needed, "{}", gap(&mut layout));
    }

    /// `Queue -> Dispatch -> Staging -> Return -> Queue`: one loop, so one
    /// back-edge, and `Lone` beside it.
    fn a_loop() -> Net {
        let mut b = NetBuilder::new();
        let queue = b.place::<u32>("Queue");
        let staging = b.place::<u32>("Staging Area");
        let _lone = b.place::<u32>("Lone");
        b.transition("Dispatch", (TakeEdge(queue),), (staging,), |n: u32| (n,));
        b.transition("Return", (TakeEdge(staging),), (queue,), |n: u32| (n,));
        b.build().expect("the net builds")
    }

    /// Every number a layout of `net` answers with, in a fixed order: each
    /// place's and each transition's rank, order and centre, each ghost's
    /// centre, each open group's frame, the bounds, the number of ranks and
    /// of back-edges. A node with no placement counts as `-1e9`.
    fn fingerprint(layout: &Layout, net: &Net) -> Vec<f32> {
        let mut out = Vec::new();
        let nodes = net
            .places()
            .map(|(id, _)| LayoutNode::Place(id))
            .chain(net.transitions().map(|(id, _)| LayoutNode::Transition(id)));
        for node in nodes {
            match layout.placement(node) {
                Some(p) => out.extend([p.rank as f32, p.order as f32, p.position.x, p.position.y]),
                None => out.push(-1e9),
            }
        }
        for (p, t, s) in layout.ghosts().collect::<Vec<_>>() {
            let at = layout.ghost_position(p, t, s).unwrap();
            out.extend([at.x, at.y]);
        }
        for name in layout.group_names().collect::<Vec<_>>() {
            if let Some(b) = layout.group_bounds(name) {
                out.extend([b.min.x, b.min.y, b.max.x, b.max.y]);
            }
        }
        if let Some(b) = layout.bounds() {
            out.extend([b.min.x, b.min.y, b.max.x, b.max.y]);
        }
        out.extend([
            layout.rank_count() as f32,
            layout.back_edges().count() as f32,
        ]);
        out
    }

    /// A handful of nets with no subnet places and no groups, each laid out
    /// one way.
    fn pinned() -> Vec<(&'static str, Vec<f32>)> {
        let mut out = Vec::new();
        let mut run = |name: &'static str, net: &Net, mut layout: Layout| {
            layout.compute(net);
            out.push((name, fingerprint(&layout, net)));
        };
        run("plain", &net(), Layout::default());
        run(
            "ghosted",
            &hubs(),
            Layout::default().with_ghosting(Ghosting::named(["H", "H2", "H3"])),
        );
        let mut wide = Layout::default().with_direction(Direction::LeftRight);
        wide.fit_labels = true;
        run("left right, fitted", &hubs(), wide);
        run("a loop", &a_loop(), Layout::default());
        out
    }

    /// What [`pinned`] gave at the commit before subnet places came to the
    /// layout, written down then. The three layouts clustered by name
    /// prefix it also held are pinned in `tests/groups.rs` now, with
    /// declared groups, against what the commit before groups computed.
    const BEFORE: [(&str, &[f32]); 4] = [
        (
            "plain",
            &[
                0.0, 0.0, -90.0, 300.0, 2.0, 0.0, 0.0, 0.0, 4.0, 0.0, 0.0, -300.0, 0.0, 1.0, 90.0,
                300.0, 1.0, 0.0, 0.0, 150.0, 3.0, 0.0, 0.0, -150.0, -130.0, -340.0, 130.0, 340.0,
                5.0, 0.0,
            ],
        ),
        (
            "ghosted",
            &[
                0.0, 0.0, -480.0, 150.0, 0.0, 1.0, -300.0, 150.0, 0.0, 2.0, -120.0, 150.0, 0.0,
                3.0, 60.0, 150.0, 0.0, 5.0, 480.0, 150.0, 2.0, 0.0, 0.0, -150.0, 1.0, 0.0, -90.0,
                0.0, 1.0, 1.0, 90.0, 0.0, 0.0, 4.0, 270.0, 150.0, -90.0, 63.0, 90.0, 63.0, 190.0,
                213.0, 270.0, 213.0, 350.0, 213.0, -520.0, -190.0, 520.0, 227.0, 3.0, 0.0,
            ],
        ),
        (
            "left right, fitted",
            &[
                0.0, 2.0, -150.0, -0.0, 0.0, 0.0, -150.0, 360.0, 0.0, 1.0, -150.0, 180.0, 0.0, 3.0,
                -150.0, -180.0, 0.0, 4.0, -150.0, -360.0, 2.0, 0.0, 150.0, -0.0, 1.0, 1.0, 0.0,
                -0.0, 1.0, 2.0, 0.0, -180.0, 1.0, 0.0, 0.0, 180.0, -190.0, -400.0, 190.0, 400.0,
                3.0, 0.0,
            ],
        ),
        (
            "a loop",
            &[
                0.0, 1.0, 90.0, 225.0, 2.0, 0.0, 0.0, -75.0, 0.0, 0.0, -90.0, 225.0, 1.0, 0.0, 0.0,
                75.0, 3.0, 0.0, 0.0, -225.0, -130.0, -250.0, 130.0, 265.0, 4.0, 1.0,
            ],
        ),
    ];

    #[test]
    fn a_net_with_no_subnet_places_lays_out_to_the_same_numbers_as_before() {
        let now = pinned();
        assert_eq!(now.len(), BEFORE.len());
        for ((name, numbers), (was_name, was)) in now.iter().zip(BEFORE) {
            assert_eq!(*name, was_name);
            assert_eq!(numbers.as_slice(), was, "{name}");
        }
    }

    // ----- subnet places -------------------------------------------------

    /// A subnet whose places are these ports, of `u32`, and one more place
    /// inside that is not a port.
    fn ports_only(ports: &[&str]) -> Net {
        let mut b = NetBuilder::new();
        for name in ports {
            let place = b.place::<u32>(*name);
            b.port(place);
        }
        let _inside = b.place::<u32>("Inside");
        b.build().expect("the subnet builds")
    }

    /// `Door -> Enter -> Level 3 [Lobby | Exit Hall] -> Leave -> Out`: the
    /// world of the spec, a subnet place between two transitions.
    struct World {
        net: Net,
        level: PlaceId,
        lobby: PlaceId,
        exit: PlaceId,
        enter: TransitionId,
        leave: TransitionId,
    }

    fn world() -> World {
        let mut b = NetBuilder::new();
        let door = b.place::<u32>("Door");
        let level = b.subnet_place("Level 3", ports_only(&["Lobby", "Exit Hall"]));
        let lobby = b.port_of::<u32>(level, "Lobby");
        let exit = b.port_of::<u32>(level, "Exit Hall");
        let out = b.place::<u32>("Out");
        let enter = b
            .transition("Enter", (TakeEdge(door),), (lobby,), |n: u32| (n,))
            .id();
        let leave = b
            .transition("Leave", (TakeEdge(exit),), (out,), |n: u32| (n,))
            .id();
        World {
            net: b.build().expect("the world builds"),
            level: level.id(),
            lobby: lobby.id(),
            exit: exit.id(),
            enter,
            leave,
        }
    }

    fn laid_out(net: &Net, layout: Layout) -> Layout {
        let mut layout = layout;
        layout.compute(net);
        layout
    }

    #[test]
    fn a_subnet_place_ranks_as_one_node_between_its_transitions() {
        let w = world();
        let layout = laid_out(&w.net, Layout::default());
        let rank = |node| layout.placement(node).unwrap().rank;
        let door = id_of(&w.net, "Door");
        assert_eq!(rank(LayoutNode::Place(door)), 0);
        assert_eq!(rank(LayoutNode::Transition(w.enter)), 1);
        assert_eq!(rank(LayoutNode::Place(w.level)), 2);
        assert_eq!(rank(LayoutNode::Transition(w.leave)), 3);
        assert_eq!(rank(LayoutNode::Place(id_of(&w.net, "Out"))), 4);
        // The ports are no nodes of their own, but have circles.
        assert!(layout.placement(LayoutNode::Place(w.lobby)).is_none());
        assert!(layout.position_of_place(w.lobby).is_some());
        assert_eq!(layout.rank_count(), 5);
    }

    #[test]
    fn a_port_answers_with_its_circle_and_its_own_radius() {
        let w = world();
        let layout = laid_out(&w.net, Layout::default());
        let drawn = layout.subnet_box(w.level).expect("a box");
        assert_eq!(drawn.ports.len(), 2);
        assert_eq!(drawn.ports[0].place, w.lobby, "in the order given");
        assert_eq!(drawn.ports[1].place, w.exit);
        for circle in &drawn.ports {
            assert_eq!(layout.position_of_place(circle.place), Some(circle.centre));
            assert_eq!(circle.radius, layout.port_radius);
            assert_eq!(
                layout.radius_of_place(circle.place),
                Some(layout.port_radius)
            );
            assert_eq!(
                layout.half_extent(LayoutNode::Place(circle.place)),
                Point::new(layout.port_radius, layout.port_radius)
            );
        }
        assert_eq!(layout.position_of_place(w.level), Some(drawn.centre));
        assert_eq!(layout.radius_of_place(w.level), None, "a box, not a circle");
        assert_eq!(
            layout.half_extent(LayoutNode::Place(w.level)),
            drawn.size / 2.0
        );
        let door = id_of(&w.net, "Door");
        assert_eq!(layout.radius_of_place(door), Some(layout.place_radius));
        assert!(layout.subnet_box(door).is_none());
        // The port's radius is a ghost's.
        assert_eq!(layout.port_radius, layout.ghost_radius);
        // The bounds hold the box.
        let bounds = layout.bounds().unwrap();
        assert!(bounds.contains(drawn.bounds().min) && bounds.contains(drawn.bounds().max));
    }

    #[test]
    fn a_port_faces_the_transitions_it_joins() {
        let w = world();
        let down = laid_out(&w.net, Layout::default());
        let drawn = down.subnet_box(w.level).unwrap();
        let at = |layout: &Layout, place| layout.position_of_place(place).unwrap();
        // Enter is above, so the lobby is on top; Leave below, the exit hall
        // at the bottom.
        assert!(at(&down, w.lobby).y > drawn.centre.y);
        assert!(at(&down, w.exit).y < drawn.centre.y);
        let right = laid_out(
            &w.net,
            Layout::default().with_direction(Direction::LeftRight),
        );
        let drawn = right.subnet_box(w.level).unwrap();
        assert!(at(&right, w.lobby).x < drawn.centre.x);
        assert!(at(&right, w.exit).x > drawn.centre.x);
    }

    /// `P1 -> T1 -> C`, `P2 -> T2 -> B`, `P3 -> T3 -> A`, where `A`, `B`,
    /// `C` are the ports of `Box`, declared in that order, and `U` is a
    /// fourth port nothing uses.
    fn crossing() -> (Net, PlaceId, [PlaceId; 4], [TransitionId; 3]) {
        let mut b = NetBuilder::new();
        let p = ["P1", "P2", "P3"].map(|n| b.place::<u32>(n));
        let subnet = b.subnet_place("Box", ports_only(&["A", "B", "C", "U"]));
        let [a, b_, c, u] = ["A", "B", "C", "U"].map(|n| b.port_of::<u32>(subnet, n));
        let t1 = b
            .transition("T1", (TakeEdge(p[0]),), (c,), |n: u32| (n,))
            .id();
        let t2 = b
            .transition("T2", (TakeEdge(p[1]),), (b_,), |n: u32| (n,))
            .id();
        let t3 = b
            .transition("T3", (TakeEdge(p[2]),), (a,), |n: u32| (n,))
            .id();
        (
            b.build().expect("the net builds"),
            subnet.id(),
            [a.id(), b_.id(), c.id(), u.id()],
            [t1, t2, t3],
        )
    }

    #[test]
    fn port_circles_follow_the_order_of_their_transitions() {
        let (net, subnet, [a, b, c, u], [t1, t2, t3]) = crossing();
        for direction in [Direction::TopDown, Direction::LeftRight] {
            let layout = laid_out(&net, Layout::default().with_direction(direction));
            // Across the ranks: x top down, down the screen left to right.
            let across = |p: Point| match direction {
                Direction::TopDown => p.x,
                Direction::LeftRight => -p.y,
            };
            let t = |id| across(layout.position_of_transition(id).unwrap());
            assert!(t(t1) < t(t2) && t(t2) < t(t3), "{direction:?}");
            let p = |id| across(layout.position_of_place(id).unwrap());
            // C joins T1, B joins T2, A joins T3: so C, B, A; and U, which
            // joins nothing, last, on the same side as the rest.
            assert!(p(c) < p(b) && p(b) < p(a) && p(a) < p(u), "{direction:?}");
            let drawn = layout.subnet_box(subnet).unwrap();
            let ids: Vec<PlaceId> = drawn.ports.iter().map(|c| c.place).collect();
            assert_eq!(ids, [a, b, c, u], "listed in the order given");
            let main = |p: Point| match direction {
                Direction::TopDown => p.y,
                Direction::LeftRight => -p.x,
            };
            for port in [a, b, c, u] {
                let at = layout.position_of_place(port).unwrap();
                assert!(
                    main(at) > main(drawn.centre),
                    "{direction:?}: on the side of T1..T3"
                );
            }
        }
    }

    /// A subnet place with `count` ports, each written by its own transition
    /// above it and read by its own transition below it, as `uses` says:
    /// `(written, read)` for each port.
    fn many(uses: &[(bool, bool)]) -> (Net, PlaceId) {
        let names: Vec<String> = (0..uses.len()).map(|i| format!("Port {i}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut b = NetBuilder::new();
        let subnet = b.subnet_place("A box with a fairly long title", ports_only(&refs));
        for (i, &(written, read)) in uses.iter().enumerate() {
            let port = b.port_of::<u32>(subnet, &names[i]);
            if written {
                let from = b.place::<u32>(format!("From {i}"));
                b.transition(
                    format!("Put {i}"),
                    (TakeEdge(from),),
                    (port,),
                    |n: u32| (n,),
                );
            }
            if read {
                let to = b.place::<u32>(format!("To {i}"));
                b.transition(format!("Take {i}"), (TakeEdge(port),), (to,), |n: u32| (n,));
            }
        }
        (b.build().expect("the net builds"), subnet.id())
    }

    /// Whether two boxes share more than an edge.
    fn overlap(a: Bounds, b: Bounds) -> bool {
        a.min.x < b.max.x - 0.01
            && b.min.x < a.max.x - 0.01
            && a.min.y < b.max.y - 0.01
            && b.min.y < a.max.y - 0.01
    }

    /// Nothing in a box overlaps anything else in it or its corners, and
    /// no other node overlaps the box.
    fn assert_clear(layout: &Layout, net: &Net, subnet: PlaceId) {
        let drawn = layout.subnet_box(subnet).unwrap();
        let frame = drawn.bounds();
        let inner = frame.inflate(-drawn.corner_radius);
        let circle = |c: &PortCircle| {
            Bounds::from_center_half_size(c.centre, Point::new(c.radius, c.radius))
        };
        assert!(inner.contains(drawn.title.min) && inner.contains(drawn.title.max));
        assert!(inner.contains(drawn.count.min) && inner.contains(drawn.count.max));
        assert!(!overlap(drawn.title, drawn.count));
        for (i, a) in drawn.ports.iter().enumerate() {
            // Clear of the rounded corners: inside the box shrunk by them.
            let shape = circle(a);
            assert!(
                inner.contains(shape.min) && inner.contains(shape.max),
                "{a:?}"
            );
            assert!(
                inner.contains(a.count.min) && inner.contains(a.count.max),
                "{a:?}"
            );
            for text in [drawn.title, drawn.count] {
                assert!(!overlap(shape, text) && !overlap(a.count, text), "{a:?}");
            }
            for b in &drawn.ports[i + 1..] {
                assert!(
                    a.centre.distance(b.centre) >= a.radius + b.radius,
                    "{a:?} {b:?}"
                );
                for (x, y) in [
                    (shape, circle(b)),
                    (shape, b.count),
                    (a.count, circle(b)),
                    (a.count, b.count),
                ] {
                    assert!(!overlap(x, y), "{a:?} {b:?}");
                }
            }
        }
        let nodes = net
            .places()
            .map(|(id, _)| LayoutNode::Place(id))
            .chain(net.transitions().map(|(id, _)| LayoutNode::Transition(id)));
        for node in nodes {
            if node == LayoutNode::Place(subnet) {
                continue;
            }
            let Some(placement) = layout.placement(node) else {
                continue;
            };
            let shape = Bounds::from_center_half_size(placement.position, layout.half_extent(node));
            assert!(!overlap(frame, shape), "{node:?} overlaps the box");
        }
    }

    #[test]
    fn nothing_overlaps_in_or_around_a_box_of_any_shape() {
        let shapes: [&[(bool, bool)]; 6] = [
            &[(true, false)],
            &[(true, true); 12],
            &[(true, false), (true, false), (true, false)],
            &[(false, true), (false, true)],
            &[(true, false), (false, true), (true, true), (false, false)],
            &[(false, false)],
        ];
        for uses in shapes {
            let (net, subnet) = many(uses);
            for direction in [Direction::TopDown, Direction::LeftRight] {
                let mut layout = Layout::default().with_direction(direction);
                layout.compute(&net);
                assert_clear(&layout, &net, subnet);
                layout.fit_labels = true;
                layout.compute(&net);
                assert_clear(&layout, &net, subnet);
            }
        }
    }

    #[test]
    fn a_box_is_wide_enough_for_its_circles_and_its_title() {
        let (one_net, one_subnet) = many(&[(true, false)]);
        let one = laid_out(&one_net, Layout::default());
        let (twelve_net, subnet) = many(&[(true, false); 12]);
        let twelve = laid_out(&twelve_net, Layout::default());
        let small = one.subnet_box(one_subnet).unwrap();
        let wide = twelve.subnet_box(subnet).unwrap();
        // Twelve circles in one row, each with its count beside it.
        assert!(wide.size.x >= 12.0 * (2.0 * twelve.port_radius + PORT_COUNT_WIDTH));
        assert!(wide.size.x > small.size.x);
        // A title of 30 characters, in the place's text.
        assert!(small.size.x >= 30.0 * PLACE_CHAR_WIDTH);
        // Top down, one row of ports: the same height for one or twelve.
        assert_eq!(small.size.y, wide.size.y);
        // A port on each side makes it taller.
        let (both, subnet) = many(&[(true, false), (false, true)]);
        let both = laid_out(&both, Layout::default());
        assert!(both.subnet_box(subnet).unwrap().size.y > small.size.y);
        // Left to right the ports are columns: twelve make it tall.
        let tall = laid_out(
            &twelve_net,
            Layout::default().with_direction(Direction::LeftRight),
        );
        let tall = tall.subnet_box(subnet).unwrap();
        assert!(tall.size.y >= 12.0 * 2.0 * twelve.port_radius);
    }

    #[test]
    fn a_box_keeps_its_transitions_as_far_off_as_a_place_does() {
        let (net, subnet) = many(&[(true, true); 12]);
        let layout = laid_out(&net, Layout::default());
        let drawn = layout.subnet_box(subnet).unwrap();
        // Every transition clears the box by at least what a place leaves.
        let clearance = layout.rank_sep - layout.place_radius - layout.transition_size.y / 2.0;
        for (id, _) in net.transitions() {
            let at = layout.position_of_transition(id).unwrap();
            let gap =
                (at.y - drawn.centre.y).abs() - drawn.size.y / 2.0 - layout.transition_size.y / 2.0;
            assert!(gap >= clearance - 0.01, "{gap}");
        }
    }

    #[test]
    fn the_same_net_lays_out_the_same_way_every_time() {
        let (a, _) = many(&[(true, false), (false, true), (true, true), (false, false)]);
        let (b, _) = many(&[(true, false), (false, true), (true, true), (false, false)]);
        for direction in [Direction::TopDown, Direction::LeftRight] {
            let first = laid_out(&a, Layout::default().with_direction(direction));
            for _ in 0..8 {
                assert_eq!(
                    laid_out(&b, Layout::default().with_direction(direction)),
                    first
                );
            }
        }
    }

    #[test]
    fn a_port_with_no_side_of_its_own_joins_the_most_ports() {
        // Two ports written from above, one read from below and above (a
        // tie), one used by nothing.
        let (net, subnet) = many(&[(true, false), (true, false), (true, true), (false, false)]);
        let layout = laid_out(&net, Layout::default());
        let drawn = layout.subnet_box(subnet).unwrap();
        for circle in &drawn.ports {
            assert!(
                circle.centre.y > drawn.centre.y,
                "{circle:?} on top, with the rest"
            );
        }
        // With only ties, the side the ranks come from.
        let (net, subnet) = many(&[(false, false)]);
        let layout = laid_out(&net, Layout::default());
        let drawn = layout.subnet_box(subnet).unwrap();
        assert!(drawn.ports[0].centre.y > drawn.centre.y);
    }

    #[test]
    fn a_loop_through_a_box_is_a_back_edge_named_by_its_port() {
        // `Work` takes from the exit hall and puts back into the lobby.
        let mut b = NetBuilder::new();
        let level = b.subnet_place("Level", ports_only(&["Lobby", "Exit Hall"]));
        let lobby = b.port_of::<u32>(level, "Lobby");
        let exit = b.port_of::<u32>(level, "Exit Hall");
        let work = b
            .transition("Work", (TakeEdge(exit),), (lobby,), |n: u32| (n,))
            .id();
        let net = b.build().expect("the net builds");
        let layout = laid_out(&net, Layout::default());
        let (work, lobby, exit) = (
            LayoutNode::Transition(work),
            LayoutNode::Place(lobby.id()),
            LayoutNode::Place(exit.id()),
        );
        assert_eq!(layout.back_edges().count(), 1);
        assert!(layout.is_back_edge(work, lobby));
        assert!(!layout.is_back_edge(exit, work));
        assert!(!layout.is_back_edge(work, LayoutNode::Place(level.id())));
        assert_eq!(layout.rank_count(), 2);
    }

    #[test]
    fn a_net_laid_out_by_name_with_its_kinds_matches_the_net() {
        let w = world();
        let by_net = laid_out(&w.net, Layout::default());
        let mut by_name = Layout::default();
        by_name.compute_named(
            w.net.places().map(|(id, info)| {
                let kind = match (info.subnet, info.port_of) {
                    (true, _) => PlaceKind::Subnet,
                    (false, Some(subnet)) => PlaceKind::Port(subnet),
                    (false, None) => PlaceKind::Ordinary,
                };
                LayoutPlace::new(id, info.name, kind)
            }),
            w.net.transitions().map(|(id, i)| (id, i.name.to_owned())),
            w.net.edges(),
            &[],
        );
        assert_eq!(by_name, by_net);
        // Without the kinds, the old drawing: every port a place of its own.
        let mut plain = Layout::default();
        plain.compute_named(
            w.net.places().map(|(id, i)| (id, i.name.to_owned())),
            w.net.transitions().map(|(id, i)| (id, i.name.to_owned())),
            w.net.edges(),
            &[],
        );
        assert!(plain.subnet_box(w.level).is_none());
        assert!(plain.placement(LayoutNode::Place(w.lobby)).is_some());
    }

    #[test]
    fn a_port_without_its_subnet_place_in_a_part_is_an_ordinary_place() {
        let w = world();
        let places: Vec<LayoutPlace> = w
            .net
            .places()
            .map(LayoutPlace::from)
            .filter(|p| p.id != w.level)
            .collect();
        let mut layout = Layout::default();
        layout.compute_subgraph(
            places,
            w.net.transitions().map(|(id, i)| (id, i.name.to_owned())),
            w.net.edges(),
            w.net.edges(),
            &[],
            None,
        );
        assert!(layout.subnet_box(w.level).is_none());
        assert!(layout.placement(LayoutNode::Place(w.lobby)).is_some());
        assert_eq!(layout.radius_of_place(w.lobby), Some(layout.place_radius));
    }

    #[test]
    fn a_box_and_its_ports_belong_to_the_box_s_group() {
        // `Door -> Enter -> Level [Lobby]` in the group `Def`, and beside it
        // the group `Atk`, whose `Raid` also writes the lobby.
        let mut b = NetBuilder::new();
        let def = b.group("Def");
        let atk = b.group("Atk");
        let door = b.place::<u32>("Door");
        let level = b.subnet_place("Level", ports_only(&["Lobby"]));
        let lobby = b.port_of::<u32>(level, "Lobby");
        let camp = b.place::<u32>("Camp");
        b.in_group(def, door);
        b.in_group(def, level);
        b.in_group(atk, camp);
        b.transition("Enter", (TakeEdge(door),), (lobby,), |n: u32| (n,))
            .group(def);
        b.transition("Raid", (TakeEdge(camp),), (lobby,), |n: u32| (n,))
            .group(atk);
        let net = b.build().expect("the net builds");
        let layout = laid_out_open(&net);
        assert_eq!(layout.group_names().collect::<Vec<_>>(), ["Def", "Atk"]);
        let (level, lobby) = (level.id(), lobby.id());
        assert_eq!(layout.group_of(LayoutNode::Place(level)), Some("Def"));
        assert_eq!(layout.group_of(LayoutNode::Place(lobby)), Some("Def"));
        let frame = layout.group_bounds("Def").unwrap();
        let drawn = layout.subnet_box(level).unwrap().bounds();
        assert!(frame.contains(drawn.min) && frame.contains(drawn.max));
        let other = layout.group_bounds("Atk").unwrap();
        assert!(!overlap(other, frame));
        // The edge from the other group still ends at the lobby's circle,
        // which faces `Enter`, the transition in its own group.
        assert!(layout.position_of_place(lobby).unwrap().y > drawn.center().y);
        assert_clear(&layout, &net, level);
    }

    #[test]
    fn a_ghosted_port_keeps_its_circle_and_gets_its_ghosts() {
        let w = world();
        let layout = laid_out(
            &w.net,
            Layout::default().with_ghosting(Ghosting::named(["Lobby", "Level 3"])),
        );
        assert!(layout.is_ghosted(w.lobby));
        assert!(
            !layout.is_ghosted(w.level),
            "a subnet place has no edges to ghost"
        );
        assert!(
            layout
                .ghost_position(w.lobby, w.enter, GhostSide::Output)
                .is_some()
        );
        assert!(layout.position_of_place(w.lobby).is_some());
        // Without the lobby's edge, nothing ranks the box after Enter.
        assert_eq!(
            layout.placement(LayoutNode::Place(w.level)).unwrap().rank,
            0
        );
    }

    #[test]
    fn a_box_leaves_its_neighbours_in_its_rank_the_room_a_place_does() {
        let mut b = NetBuilder::new();
        let door = b.place::<u32>("Door");
        let level = b.subnet_place("Level 3", ports_only(&["Lobby"]));
        let lobby = b.port_of::<u32>(level, "Lobby");
        let side = b.place::<u32>("Side");
        b.transition("Enter", (TakeEdge(door),), (lobby, side), |n: u32| (n, n));
        let net = b.build().expect("the net builds");
        let layout = laid_out(&net, Layout::default());
        let drawn = layout.subnet_box(level.id()).unwrap();
        let at = layout.position_of_place(side.id()).unwrap();
        assert_eq!(at.y, drawn.centre.y, "one rank");
        let clear = (at.x - drawn.centre.x).abs() - drawn.size.x / 2.0 - layout.place_radius;
        assert!(
            clear >= layout.node_sep - 2.0 * layout.place_radius - 0.01,
            "{clear}"
        );
        assert_clear(&layout, &net, level.id());
    }
}
