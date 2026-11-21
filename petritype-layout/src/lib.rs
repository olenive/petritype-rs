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
//! # Clusters
//!
//! A net with a few dozen places per subsystem spreads each subsystem over
//! the whole height of the drawing and tangles them. Set
//! [`Layout::clustering`] to [`Clustering::NamePrefix`] and the nodes are
//! grouped by the text of their name before a separator (`Def` in
//! `Def/Board`); a name without the separator joins one unnamed cluster.
//! Each cluster is then laid out on its own, by the four passes above over
//! only the edges *inside* it, and the finished clusters are packed in rows,
//! in the order their first node was declared, with [`Layout::cluster_gap`]
//! between them. The row width is chosen so the whole comes out as wide as a
//! landscape window will take it: the packing that frames largest on a
//! screen 1.6 times as wide as high wins.
//!
//! Edges between clusters take no part in the layout, so they are drawn
//! straight across the gaps, and no edge between clusters is ever a
//! back-edge. [`Layout::cluster_names`], [`Layout::cluster_bounds`] and
//! [`Layout::cluster_of`] say where each cluster is; the renderer draws a
//! frame and a title from them.
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
//! home node lands in the first rank of its cluster.
//! [`Layout::is_ghosted`], [`Layout::ghosts`] and [`Layout::ghost_position`]
//! say what was ghosted and where.
//!
//! In a clustered layout a node is drawn under its name minus its own
//! cluster's prefix ([`Layout::display_name`]), and
//! [`Layout::fit_labels`] can make the gaps within a rank wide enough for
//! the names. In a part of a net taken for one cluster, the ghosted places of
//! other clusters have no home node: their ghosts stand for them.
//!
//! A part of a net, such as what a viewer's filter shows, is laid out on its
//! own with [`Layout::compute_subgraph`]: one drawing, never clustered.
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

use std::collections::{HashMap, HashSet};

pub use geometry::{Bounds, Point};
use petritype::{EdgeInfo, Net, PlaceId, PlaceInfo, TransitionId};

/// How many tokens a place draws: one at the centre, a ring of 6 around it
/// and a ring of 12 around that. Further tokens are counted, not drawn.
pub const TOKEN_CAPACITY: usize = 19;

/// How many down-and-up barycentre sweeps the ordering pass makes.
const SWEEPS: usize = 4;

/// The shape of window the cluster packing aims for: width over height.
const LANDSCAPE: f32 = 1.6;
/// Room between a cluster's nodes and its frame.
const CLUSTER_PAD: f32 = 24.0;
/// Extra room along the top of a cluster's frame, for its title.
const TITLE_ROOM: f32 = 26.0;
/// How wide a character of a place's name is drawn, as an estimate: the
/// renderer's 15-point text.
const PLACE_CHAR_WIDTH: f32 = 8.0;
/// The same for a transition's name, drawn in 14-point text.
const TRANSITION_CHAR_WIDTH: f32 = 7.5;
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

/// How a layout groups its nodes into clusters.
///
/// Each cluster is laid out on its own and the clusters are packed in rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Clustering {
    /// One drawing, no clusters: the default.
    #[default]
    None,
    /// Cluster by the text of a node's name before the first occurrence of
    /// this separator: with `'/'`, `Def/Board` and `Def/Fortress` share the
    /// cluster `Def`. A name without the separator, or beginning with it,
    /// belongs to the one unnamed cluster, whose name is `""`.
    NamePrefix(char),
}

impl Clustering {
    /// The prefix of `name` that names its cluster, or `None` if it has none
    /// and so belongs to the unnamed cluster (or this is
    /// [`Clustering::None`]). [`Layout::cluster_of`] answers for a laid-out
    /// node, and gives `Some("")` for the unnamed cluster.
    pub fn prefix_of<'a>(&self, name: &'a str) -> Option<&'a str> {
        match *self {
            Clustering::None => None,
            Clustering::NamePrefix(separator) => name
                .split_once(separator)
                .map(|(prefix, _)| prefix)
                .filter(|prefix| !prefix.is_empty()),
        }
    }
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

/// One place handed to the layout: its id, its name and its [`PlaceKind`].
///
/// [`compute_named`](Layout::compute_named) and
/// [`compute_subgraph`](Layout::compute_subgraph) take anything that turns
/// into one, so a plain `(PlaceId, String)` is still an ordinary place, and
/// `(PlaceId, PlaceInfo)` from [`Net::places`] carries the kind the net says.
/// The ports of a subnet place are its places of kind [`PlaceKind::Port`], in
/// the order they are given, which should be the order the net declares them.
///
/// ```
/// use petritype::PlaceId;
/// use petritype_layout::{LayoutPlace, PlaceKind};
///
/// let level = PlaceId::new(1);
/// let places = [
///     LayoutPlace::from((PlaceId::new(0), "Door".to_owned())),
///     LayoutPlace::new(level, "Level 3", PlaceKind::Subnet),
///     LayoutPlace::new(PlaceId::new(2), "Lobby", PlaceKind::Port(level)),
/// ];
/// assert_eq!(places[0].kind, PlaceKind::Ordinary);
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
}

impl LayoutPlace {
    /// A place of this kind.
    pub fn new(id: PlaceId, name: impl Into<String>, kind: PlaceKind) -> Self {
        Self {
            id,
            name: name.into(),
            kind,
        }
    }
}

/// An ordinary place.
impl From<(PlaceId, String)> for LayoutPlace {
    fn from((id, name): (PlaceId, String)) -> Self {
        Self::new(id, name, PlaceKind::Ordinary)
    }
}

/// A place as the net describes it: a subnet place, a port of one, or an
/// ordinary place.
impl From<(PlaceId, PlaceInfo<'_>)> for LayoutPlace {
    fn from((id, info): (PlaceId, PlaceInfo<'_>)) -> Self {
        let kind = match (info.subnet, info.port_of) {
            (true, _) => PlaceKind::Subnet,
            (false, Some(subnet)) => PlaceKind::Port(subnet),
            (false, None) => PlaceKind::Ordinary,
        };
        Self::new(id, info.name, kind)
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

/// One cluster the last computation made.
#[derive(Debug, Clone, PartialEq)]
struct Cluster {
    /// Its name, `""` for the unnamed one.
    name: String,
    /// Its frame, in world coordinates.
    bounds: Bounds,
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
    /// How nodes are grouped into clusters. [`Clustering::None`] by
    /// default, which lays the whole net out as one drawing.
    pub clustering: Clustering,
    /// The distance between neighbouring clusters' frames, both across and
    /// down the rows they are packed in.
    pub cluster_gap: f32,
    /// Which places are drawn as ghosts beside their transitions. Nothing
    /// by default.
    pub ghosting: Ghosting,
    /// Whether to space the nodes of a rank so that the name drawn beside
    /// each fits between it and its neighbour: the width of a name is
    /// estimated from its length. Off by default, which spaces nodes
    /// [`node_sep`](Layout::node_sep) apart whatever their names.
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
    /// The clusters the last computation made, in packing order.
    clusters: Vec<Cluster>,
    /// Which of `clusters` each node is in.
    cluster_index: HashMap<LayoutNode, usize>,
    /// The ghosted places of the last computation.
    ghosted: HashSet<PlaceId>,
    /// How many bytes of its name the node's own cluster prefix takes up,
    /// for the nodes drawn with a short name.
    stripped: HashMap<LayoutNode, usize>,
    /// Every ghost, in declaration order of the edges, and where it is.
    ghosts: Vec<((PlaceId, TransitionId, GhostSide), Point)>,
    /// The box of each subnet place of the last computation.
    subnets: HashMap<PlaceId, SubnetBox>,
    /// The centre of each port's circle, for the ports drawn in a box.
    ports: HashMap<PlaceId, Point>,
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
            clustering: Clustering::None,
            cluster_gap: 80.0,
            ghosting: Ghosting::default(),
            fit_labels: false,
            ghost_radius: 14.0,
            ghost_sep: 80.0,
            port_radius: 14.0,
            nodes: HashMap::new(),
            back_edges: HashSet::new(),
            ranks: 0,
            clusters: Vec::new(),
            cluster_index: HashMap::new(),
            ghosted: HashSet::new(),
            stripped: HashMap::new(),
            ghosts: Vec::new(),
            subnets: HashMap::new(),
            ports: HashMap::new(),
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

    /// The default layout, grouping nodes by [`Clustering`] instead.
    pub fn with_clustering(mut self, clustering: Clustering) -> Self {
        self.clustering = clustering;
        self
    }

    /// The default layout, ghosting places by [`Ghosting`] instead.
    pub fn with_ghosting(mut self, ghosting: Ghosting) -> Self {
        self.ghosting = ghosting;
        self
    }

    /// Lays out every place and transition of `net`, replacing whatever was
    /// computed before and keeping the configuration. The net says which of
    /// its places are subnet places and which are their ports.
    pub fn compute(&mut self, net: &Net) {
        self.compute_named(
            net.places().map(LayoutPlace::from),
            net.transitions()
                .map(|(id, info)| (id, info.name.to_owned())),
            net.edges(),
        );
    }

    /// Lays out the given places, transitions and edges, replacing whatever
    /// was computed before and keeping the configuration.
    ///
    /// The nodes are taken in the order given, which should be declaration
    /// order: ties in the layout keep it. An edge naming a node not given is
    /// left out. Without names there is nothing to cluster by or to ghost by
    /// name, so this ignores [`clustering`](Layout::clustering) and
    /// [`Ghosting::names`] (only [`Ghosting::min_degree`] applies), and every
    /// place is an ordinary one: use [`compute_named`](Layout::compute_named)
    /// for those.
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
                .map(|id| (id, String::new()))
                .collect(),
            edges.into_iter().collect(),
            None,
            false,
            None,
        );
    }

    /// Lays out a part of a net on its own, as one drawing: never in
    /// [clusters](Clustering), whatever [`clustering`](Layout::clustering)
    /// says, so there is no packing of what was left out.
    ///
    /// `edges` are the edges of the part. `whole` are the edges of the net it
    /// was taken from, and decide which places [`Ghosting::min_degree`]
    /// ghosts, so a hub is a hub in every part of the net and not only where
    /// all its edges happen to be shown.
    ///
    /// `focus` is the set of clusters the part was taken for, if any. With
    /// [`clustering`](Layout::clustering) on, the nodes of those clusters are
    /// drawn with their short names (see [`display_name`](Layout::display_name)),
    /// and a ghosted place of another cluster keeps its ghosts but gets no
    /// home node of its own: the ghosts stand for it. A focus of two or more
    /// clusters is laid out clustered, so each keeps its frame; the nodes of
    /// other clusters share one more block, with no frame
    /// ([`cluster_of`](Layout::cluster_of) is `None` for them). A focus of
    /// one is a plain drawing.
    ///
    /// The places are given as for [`compute_named`](Layout::compute_named).
    /// A subnet place in the part is drawn with the circles of those of its
    /// ports that the part holds, and a port without its subnet place is
    /// drawn as an ordinary place.
    pub fn compute_subgraph(
        &mut self,
        places: impl IntoIterator<Item = impl Into<LayoutPlace>>,
        transitions: impl IntoIterator<Item = (TransitionId, String)>,
        edges: impl IntoIterator<Item = EdgeInfo>,
        whole: impl IntoIterator<Item = EdgeInfo>,
        focus: Option<&[&str]>,
    ) {
        self.run(
            places.into_iter().map(Into::into).collect(),
            transitions.into_iter().collect(),
            edges.into_iter().collect(),
            Some(whole.into_iter().collect()),
            focus.is_some_and(|f| f.len() > 1),
            focus,
        );
    }

    /// [`compute_from`](Layout::compute_from) with each node's name, which
    /// is what [`clustering`](Layout::clustering) groups by and what the
    /// room for labels is estimated from. It needs no [`Net`], so a renderer
    /// can feed the layout through it from whatever source it reads the net
    /// from.
    ///
    /// Each place is a [`LayoutPlace`], or anything that turns into one: a
    /// `(PlaceId, String)` is an ordinary place, and a [`LayoutPlace`] with
    /// [`PlaceKind::Subnet`] or [`PlaceKind::Port`] tells the layout about a
    /// subnet place and its ports.
    pub fn compute_named(
        &mut self,
        places: impl IntoIterator<Item = impl Into<LayoutPlace>>,
        transitions: impl IntoIterator<Item = (TransitionId, String)>,
        edges: impl IntoIterator<Item = EdgeInfo>,
    ) {
        self.run(
            places.into_iter().map(Into::into).collect(),
            transitions.into_iter().collect(),
            edges.into_iter().collect(),
            None,
            true,
            None,
        );
    }

    /// The whole computation. `clustered` says whether the names are to be
    /// clustered by (when [`clustering`](Layout::clustering) is on), and
    /// `whole` the edges the ghosting degrees are counted over, if not the
    /// given ones.
    fn run(
        &mut self,
        places: Vec<LayoutPlace>,
        transitions: Vec<(TransitionId, String)>,
        given: Vec<EdgeInfo>,
        whole: Option<Vec<EdgeInfo>>,
        clustered: bool,
        focus: Option<&[&str]>,
    ) {
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
        // Each port drawn in a box, in the order given: the port, its name
        // and its subnet place.
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
                }
            }
        }
        for (id, name) in transitions {
            nodes.push(LayoutNode::Transition(id));
            names.push(name);
            boxed.push(false);
        }
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
        self.clusters.clear();
        self.cluster_index.clear();
        self.ghosted.clear();
        self.stripped.clear();
        self.ghosts.clear();
        self.subnets.clear();
        self.ports.clear();
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
        // The ghosts of each transition, by node index: inputs, outputs.
        let mut slots: HashMap<usize, Slots> = HashMap::new();
        for edge in &mut edges {
            edge.ghosted = self.ghosted.contains(&edge.place);
            if !edge.ghosted {
                continue;
            }
            let key = (edge.place, edge.transition, edge.side);
            if !self.ghosts.iter().any(|(k, _)| *k == key) {
                self.ghosts.push((key, Point::ZERO));
                let transition = match edge.side {
                    GhostSide::Input => edge.b,
                    GhostSide::Output => edge.a,
                };
                let slot = slots.entry(transition).or_default();
                match edge.side {
                    GhostSide::Input => slot.inputs.push(edge.place),
                    GhostSide::Output => slot.outputs.push(edge.place),
                }
            }
        }

        // Which cluster each node is in, in order of first appearance. With
        // no clustering there is one group and no cluster.
        // With nothing to lay out there is nothing to cluster.
        let clustered = clustered && self.clustering != Clustering::None && !nodes.is_empty();
        let mut cluster_names: Vec<Option<String>> = Vec::new();
        let mut group_of = vec![0_usize; nodes.len()];
        if clustered {
            // In a part, the nodes outside the focus share one frameless group.
            let mut seen: HashMap<Option<&str>, usize> = HashMap::new();
            for (i, name) in names.iter().enumerate() {
                let key = self.clustering.prefix_of(name).unwrap_or("");
                let key = match focus {
                    Some(focus) if !focus.contains(&key) => None,
                    _ => Some(key),
                };
                group_of[i] = *seen.entry(key).or_insert_with(|| {
                    cluster_names.push(key.map(str::to_owned));
                    cluster_names.len() - 1
                });
            }
        }
        let groups = cluster_names.len().max(1);

        // A node is drawn with its name minus the prefix of its own cluster:
        // in every cluster of a clustered layout, and in the focused one of
        // a part. A ghosted place of another cluster is left out of a part
        // as a node, its ghosts standing for it.
        let separator = match self.clustering {
            Clustering::NamePrefix(separator) => Some(separator),
            Clustering::None => None,
        };
        let mut labels: Vec<String> = names.clone();
        let mut homeless = vec![false; nodes.len()];
        if let Some(separator) = separator {
            for (i, name) in names.iter().enumerate() {
                let prefix = self.clustering.prefix_of(name);
                let own = match focus {
                    Some(focus) => prefix.filter(|p| focus.contains(p)),
                    None if clustered => prefix,
                    None => None,
                };
                if let Some(prefix) = own {
                    let cut = prefix.len() + separator.len_utf8();
                    labels[i] = name[cut..].to_owned();
                    self.stripped.insert(nodes[i], cut);
                }
                if let (Some(focus), LayoutNode::Place(id)) = (focus, nodes[i])
                    && self.ghosted.contains(&id)
                    && !focus.contains(&prefix.unwrap_or(""))
                {
                    homeless[i] = true;
                }
            }
        }
        let mut members: Vec<Vec<usize>> = vec![Vec::new(); groups];
        for (i, &g) in group_of.iter().enumerate() {
            if !homeless[i] {
                members[g].push(i);
            }
        }

        // Each group on its own, from its own edges.
        let mut arranged: Vec<Arranged> = Vec::with_capacity(groups);
        // The box of each subnet place, by node index.
        let mut plans: Vec<Option<BoxPlan>> = vec![None; nodes.len()];
        for (g, group) in members.iter().enumerate() {
            let local: HashMap<usize, usize> =
                group.iter().enumerate().map(|(l, &i)| (i, l)).collect();
            // The edges inside it, by index among all edges.
            let within: Vec<usize> = (0..edges.len())
                .filter(|&e| {
                    let e = &edges[e];
                    !e.ghosted && group_of[e.a] == g && group_of[e.b] == g
                })
                .collect();
            let inside: Vec<(usize, usize)> = within
                .iter()
                .map(|&e| (local[&edges[e].a], local[&edges[e].b]))
                .collect();
            let layers = layer(group.len(), &inside);
            self.ranks = self.ranks.max(layers.layers.len());
            for (k, &e) in within.iter().enumerate() {
                if layers.back[k] {
                    // Named by its own ends, a port's included.
                    self.back_edges.insert(edges[e].ends());
                }
            }
            // Which side of its box each port faces, by the ranks of the
            // transitions it is joined to inside the group.
            let mut rank = vec![0_usize; group.len()];
            for (r, layer) in layers.layers.iter().enumerate() {
                for &l in layer {
                    rank[l] = r;
                }
            }
            for (l, &i) in group.iter().enumerate() {
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
                        let (mut before, mut after) = (0_usize, 0_usize);
                        for &e in &within {
                            let edge = &edges[e];
                            if edge.place != port {
                                continue;
                            }
                            let transition = match edge.side {
                                GhostSide::Input => edge.b,
                                GhostSide::Output => edge.a,
                            };
                            // Never the same rank: an edge inside a group
                            // joins two ranks.
                            if rank[local[&transition]] < rank[l] {
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
            let input = Group {
                members: group,
                names: &labels,
                nodes: &nodes,
                slots: &slots,
                inside: &inside,
                plans: &plans,
            };
            arranged.push(self.arrange(&input, &layers));
        }

        if !clustered {
            // Centre the drawing on the origin, so a camera at rest sees it.
            let only = arranged.pop().unwrap_or_default();
            let points: Vec<Point> = only.placed.iter().map(|&(_, p)| p.position).collect();
            let centre = bounding(&points).map_or(Point::ZERO, |b| b.center());
            for (i, mut placement) in only.placed {
                placement.position -= centre;
                self.nodes.insert(nodes[i], placement);
            }
        } else {
            // Pack the clusters, then centre the whole on the origin.
            let sizes: Vec<Point> = arranged.iter().map(|a| a.bounds.size()).collect();
            let corners = pack(&sizes, self.cluster_gap);
            let mut whole: Option<Bounds> = None;
            let mut shifted: Vec<Bounds> = Vec::with_capacity(groups);
            for (a, corner) in arranged.iter().zip(&corners) {
                let shift = *corner - Point::new(a.bounds.min.x, a.bounds.max.y);
                let rect = Bounds::from_corners(a.bounds.min + shift, a.bounds.max + shift);
                whole = Some(whole.map_or(rect, |w| w.union(rect)));
                shifted.push(rect);
            }
            let centre = whole.map_or(Point::ZERO, |w| w.center());
            for (g, a) in arranged.into_iter().enumerate() {
                let shift = corners[g] - Point::new(a.bounds.min.x, a.bounds.max.y) - centre;
                for (i, mut placement) in a.placed {
                    placement.position += shift;
                    if cluster_names[g].is_some() {
                        self.cluster_index.insert(nodes[i], self.clusters.len());
                    }
                    self.nodes.insert(nodes[i], placement);
                }
                if let Some(name) = &cluster_names[g] {
                    self.clusters.push(Cluster {
                        name: name.clone(),
                        bounds: Bounds::from_corners(
                            shifted[g].min - centre,
                            shifted[g].max - centre,
                        ),
                    });
                }
            }
        }

        // Ghosts go beside their transitions, wherever those ended up.
        for (&transition, slot) in &slots {
            let LayoutNode::Transition(id) = nodes[transition] else {
                continue;
            };
            let Some(at) = self.position_of(nodes[transition]) else {
                continue;
            };
            for (side, list) in [
                (GhostSide::Input, &slot.inputs),
                (GhostSide::Output, &slot.outputs),
            ] {
                for (k, &place) in list.iter().enumerate() {
                    let offset = self.ghost_offset(side, k, list.len());
                    if let Some((_, position)) = self
                        .ghosts
                        .iter_mut()
                        .find(|(key, _)| *key == (place, id, side))
                    {
                        *position = at + offset;
                    }
                }
            }
        }

        // The boxes of the subnet places, and the circles of their ports in
        // the order of the transitions they are joined to, wherever those
        // ended up.
        for (i, plan) in plans.iter().enumerate() {
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
            let key = |port: PlaceId| {
                let (sum, count) = edges
                    .iter()
                    .filter(|e| e.place == port && !e.ghosted)
                    .filter_map(|e| self.position_of_transition(e.transition))
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
            let cluster = self.cluster_index.get(&nodes[i]).copied();
            for circle in &drawn.ports {
                self.ports.insert(circle.place, circle.centre);
                if let Some(cluster) = cluster {
                    self.cluster_index
                        .insert(LayoutNode::Place(circle.place), cluster);
                }
            }
            self.subnets.insert(subnet, drawn);
        }
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

    /// Turns one group's layers into coordinates, in a frame of its own,
    /// and works out the box around it.
    fn arrange(&self, group: &Group, layers: &Layers) -> Arranged {
        // A subnet place's box, if the node is one.
        let plan_of = |local: usize| group.plans[group.members[local]].as_ref();
        // How far a node reaches before and after its centre along the main
        // axis, ghosts included, and how wide its slot in a rank is.
        let main_half = |local: usize| match (
            plan_of(local),
            group.nodes[group.members[local]],
            self.direction,
        ) {
            (Some(plan), _, Direction::TopDown) => plan.size.y / 2.0,
            (Some(plan), _, Direction::LeftRight) => plan.size.x / 2.0,
            (None, LayoutNode::Place(_), _) => self.place_radius,
            (None, LayoutNode::Transition(_), Direction::TopDown) => self.transition_size.y / 2.0,
            (None, LayoutNode::Transition(_), Direction::LeftRight) => self.transition_size.x / 2.0,
        };
        let slot_of = |local: usize| group.slots.get(&group.members[local]);
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
            let global = group.members[local];
            match plan_of(local) {
                Some(plan) => Bounds::from_center_half_size(at, plan.size / 2.0),
                None => self.reach(group.nodes[global], &group.names[global], at),
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

        let mut placed = Vec::with_capacity(group.members.len());
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
                let global = group.members[local];
                let mut reach = reach_of(local, position);
                if let Some(slot) = slot_of(local) {
                    for (side, list) in [
                        (GhostSide::Input, &slot.inputs),
                        (GhostSide::Output, &slot.outputs),
                    ] {
                        for k in 0..list.len() {
                            let at = position + self.ghost_offset(side, k, list.len());
                            reach = reach.union(self.ghost_reach(side, at));
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
                .find(|(i, _)| *i == group.members[local])
                .map_or(Point::ZERO, |(_, p)| p.position)
        };
        for (e, &(a, b)) in group.inside.iter().enumerate() {
            if layers.back[e] {
                let (a, b) = (position_of(a), position_of(b));
                bounds = bounds.union_point((a + b) / 2.0 + side * 0.5 * self.bow(a, b));
            }
        }
        Arranged {
            placed,
            bounds: bounds.inflate(CLUSTER_PAD),
        }
        .with_title_room()
    }

    /// How far the middle of a ghost is from the edge of its transition's
    /// box, along the main axis: a gap long enough for an arrow, and the
    /// ghost's own radius.
    fn ghost_depth(&self) -> f32 {
        GHOST_GAP + self.ghost_radius * 2.0 + GHOST_LABEL
    }

    /// Where the `k`th of `count` ghosts on a side of a transition sits,
    /// relative to the transition's centre: spread across the transition,
    /// at a distance along the main axis that leaves room for an arrow.
    fn ghost_offset(&self, side: GhostSide, k: usize, count: usize) -> Point {
        let half = match self.direction {
            Direction::TopDown => self.transition_size.y / 2.0,
            Direction::LeftRight => self.transition_size.x / 2.0,
        };
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
    /// where its circle is.
    pub fn placement(&self, node: LayoutNode) -> Option<Placement> {
        self.nodes.get(&node).copied()
    }

    /// The centre of a node: for a subnet place, the centre of its box, and
    /// for a port drawn in one, the centre of the port's circle.
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
    /// computation did not place.
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

    /// The name to draw beside a node, given its full name: without the
    /// prefix of its own cluster where there is one, in a clustered layout
    /// and for the focused cluster of a
    /// [part](Layout::compute_subgraph). Ghosts and the places of other
    /// clusters keep their full names. Anything that names the node in full,
    /// such as a tooltip, shows `name` as given.
    pub fn display_name<'a>(&self, node: LayoutNode, name: &'a str) -> &'a str {
        match self.stripped.get(&node) {
            Some(&cut) => name.get(cut..).unwrap_or(name),
            None => name,
        }
    }

    /// Whether the last computation drew `place` as ghosts: its edges run
    /// to the ghosts beside its transitions and not to the place itself.
    pub fn is_ghosted(&self, place: PlaceId) -> bool {
        self.ghosted.contains(&place)
    }

    /// Every ghost of the last computation as `(place, transition, side)`,
    /// in the order the edges were declared.
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

    /// The names of the clusters the last computation made, in the order
    /// they were packed: that of the first node declared in each. Empty
    /// without [`Clustering`]; `""` is the unnamed cluster of the names that
    /// have no prefix.
    pub fn cluster_names(&self) -> impl Iterator<Item = &str> + '_ {
        self.clusters.iter().map(|c| c.name.as_str())
    }

    /// The frame of a cluster, in world coordinates: its nodes, their
    /// labels, the curve of its back-edges and a strip along the top for its
    /// title. `None` for a name that is not a cluster.
    pub fn cluster_bounds(&self, name: &str) -> Option<Bounds> {
        self.clusters
            .iter()
            .find(|c| c.name == name)
            .map(|c| c.bounds)
    }

    /// The cluster a node is in: its name, `Some("")` for the unnamed
    /// cluster, or `None` without [`Clustering`] or for a node the last
    /// computation did not see. ([`Clustering::prefix_of`] is the other way
    /// round: `None` for the unnamed cluster.) A port drawn in its subnet
    /// place's box is in the box's cluster, whatever its own name.
    pub fn cluster_of(&self, node: LayoutNode) -> Option<&str> {
        let &index = self.cluster_index.get(&node)?;
        Some(self.clusters[index].name.as_str())
    }

    /// How many ranks the last computation produced: in a clustered layout,
    /// the most of any cluster.
    pub fn rank_count(&self) -> usize {
        self.ranks
    }

    /// Half the width and height of a node's shape: for a subnet place, its
    /// box, and for a port drawn in one, the port's circle.
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

    /// The box around every node's shape and every ghost, or `None` before
    /// anything was laid out. Labels and curved edges are outside it;
    /// whoever frames the drawing adds a margin for them (a clustered layout
    /// has the frames of its clusters for that).
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
        for (_, at) in &self.ghosts {
            let rect = Bounds::from_center_half_size(
                *at,
                Point::new(self.ghost_radius, self.ghost_radius),
            );
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

/// What [`Layout::arrange`] needs to know about one cluster.
struct Group<'a> {
    /// Its nodes, by index among all nodes.
    members: &'a [usize],
    names: &'a [String],
    nodes: &'a [LayoutNode],
    slots: &'a HashMap<usize, Slots>,
    /// The edges inside it, by index among `members`.
    inside: &'a [(usize, usize)],
    /// The box of each subnet place, by index among all nodes.
    plans: &'a [Option<BoxPlan>],
}

/// One group of nodes, laid out in a frame of its own.
#[derive(Debug, Default)]
struct Arranged {
    /// Each node of the group, by its index among all nodes, and where it
    /// went.
    placed: Vec<(usize, Placement)>,
    /// The box around it, padded for its frame.
    bounds: Bounds,
}

impl Arranged {
    /// The same, with a strip along the top for the cluster's title.
    fn with_title_room(mut self) -> Self {
        self.bounds.max.y += TITLE_ROOM;
        self
    }
}

/// The rule a clustered [`Layout`] packs its clusters with. It is public so
/// that a picture of the same net in other units, such as a thumbnail's grid
/// of cells, can pack the clusters alike.
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
    use petritype::{NetBuilder, TakeEdge};

    use super::*;

    /// `A/In -> A/Go -> A/Out -> B/Go -> B/Out`, and `Lone`, which has no
    /// prefix. The net is built so the unnamed cluster is declared last.
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

    fn clustered(net: &Net) -> Layout {
        let mut layout = Layout::default().with_clustering(Clustering::NamePrefix('/'));
        layout.compute(net);
        layout
    }

    #[test]
    fn a_prefix_names_its_cluster_and_the_rest_share_an_unnamed_one() {
        let c = Clustering::NamePrefix('/');
        assert_eq!(c.prefix_of("Def/Board"), Some("Def"));
        assert_eq!(c.prefix_of("Def/Sub/Board"), Some("Def"));
        assert_eq!(c.prefix_of("Board"), None);
        assert_eq!(c.prefix_of("/Board"), None);
        assert_eq!(Clustering::None.prefix_of("Def/Board"), None);
    }

    #[test]
    fn clusters_come_in_order_of_first_declaration_and_hold_their_nodes() {
        let net = net();
        let layout = clustered(&net);
        // Places are declared before transitions: A, B, then the unnamed.
        assert_eq!(layout.cluster_names().collect::<Vec<_>>(), ["A", "B", ""]);
        for (id, info) in net.places() {
            let want = info.name.split_once('/').map_or("", |(p, _)| p);
            assert_eq!(layout.cluster_of(LayoutNode::Place(id)), Some(want));
        }
        assert!(layout.cluster_bounds("nope").is_none());
    }

    #[test]
    fn each_cluster_is_ranked_by_its_own_edges_and_frames_its_nodes() {
        let net = net();
        let layout = clustered(&net);
        for (id, _) in net.places() {
            let node = LayoutNode::Place(id);
            let frame = layout
                .cluster_bounds(layout.cluster_of(node).unwrap())
                .unwrap();
            assert!(frame.contains(layout.position_of(node).unwrap()));
        }
        // A/Out -> B/Go crosses clusters, so B/Go is a source of its own
        // cluster: rank 0, not 2.
        let b_go = net
            .transitions()
            .find(|(_, info)| info.name == "B/Go")
            .unwrap()
            .0;
        assert_eq!(
            layout.placement(LayoutNode::Transition(b_go)).unwrap().rank,
            0
        );
    }

    #[test]
    fn frames_do_not_overlap_and_keep_the_gap() {
        let net = net();
        let layout = clustered(&net);
        let frames: Vec<Bounds> = layout
            .cluster_names()
            .map(|n| layout.cluster_bounds(n).unwrap())
            .collect();
        for (i, a) in frames.iter().enumerate() {
            for b in &frames[i + 1..] {
                let apart_x = a.max.x + layout.cluster_gap - 0.01 <= b.min.x
                    || b.max.x + layout.cluster_gap - 0.01 <= a.min.x;
                let apart_y = a.max.y + layout.cluster_gap - 0.01 <= b.min.y
                    || b.max.y + layout.cluster_gap - 0.01 <= a.min.y;
                assert!(apart_x || apart_y, "{a:?} {b:?}");
            }
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
    fn without_clustering_nothing_changes() {
        let net = net();
        let mut plain = Layout::default();
        plain.compute(&net);
        assert_eq!(plain.cluster_names().count(), 0);
        let (id, _) = net.places().next().unwrap();
        assert_eq!(plain.cluster_of(LayoutNode::Place(id)), None);
        // `compute_from` has no names, so it never clusters.
        let mut nameless = Layout::default().with_clustering(Clustering::NamePrefix('/'));
        nameless.compute_from(
            net.places().map(|(id, _)| id),
            net.transitions().map(|(id, _)| id),
            net.edges(),
        );
        assert_eq!(nameless.cluster_names().count(), 0);
        assert_eq!(nameless.position_of_place(id), plain.position_of_place(id));
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
        let net = net();
        let a_out = id_of(&net, "A/Out");
        let b_go = transition_of(&net, "B/Go");
        let b_out = id_of(&net, "B/Out");
        let names = |places: &[PlaceId]| -> Vec<(PlaceId, String)> {
            places
                .iter()
                .map(|&p| {
                    (
                        p,
                        net.places()
                            .find(|(id, _)| *id == p)
                            .unwrap()
                            .1
                            .name
                            .to_owned(),
                    )
                })
                .collect()
        };
        let edges: Vec<EdgeInfo> = net
            .edges()
            .filter(|e| matches!(e, EdgeInfo::Input { transition, .. } | EdgeInfo::Output { transition, .. } if *transition == b_go))
            .collect();
        // A/Out has two edges in the whole net (written by A/Go, read by
        // B/Go) but only one in the part.
        let mut layout = Layout::default()
            .with_clustering(Clustering::NamePrefix('/'))
            .with_ghosting(Ghosting::default().with_min_degree(2));
        layout.compute_subgraph(
            names(&[a_out, b_out]),
            [(b_go, "B/Go".to_owned())],
            edges,
            net.edges(),
            None,
        );
        assert_eq!(layout.cluster_names().count(), 0);
        assert!(layout.is_ghosted(a_out));
        assert!(!layout.is_ghosted(b_out));
    }

    #[test]
    fn an_empty_net_lays_out_even_when_clustered() {
        let net = NetBuilder::new().build().expect("an empty net builds");
        let mut layout = Layout::default().with_clustering(Clustering::NamePrefix('/'));
        layout.compute(&net);
        assert_eq!(layout.cluster_names().count(), 0);
        assert!(layout.bounds().is_none());
        layout.compute_named(Vec::<LayoutPlace>::new(), [], []);
        layout.compute_subgraph(Vec::<LayoutPlace>::new(), [], [], [], Some(&["A"]));
    }

    #[test]
    fn nodes_are_drawn_without_the_prefix_of_their_own_cluster() {
        let net = net();
        let layout = clustered(&net);
        let a_in = LayoutNode::Place(id_of(&net, "A/In"));
        assert_eq!(layout.display_name(a_in, "A/In"), "In");
        let lone = LayoutNode::Place(id_of(&net, "Lone"));
        assert_eq!(layout.display_name(lone, "Lone"), "Lone");
        // Without clustering, nothing is shortened.
        let mut plain = Layout::default();
        plain.compute(&net);
        assert_eq!(plain.display_name(a_in, "A/In"), "A/In");
    }

    /// `A/Go` reads `A/In` and the hub `B/Hub`, and writes `A/Out`.
    fn foreign_hub() -> Net {
        let mut b = NetBuilder::new();
        let a_in = b.place::<u32>("A/In");
        let hub = b.place::<u32>("B/Hub");
        let a_out = b.place::<u32>("A/Out");
        b.transition(
            "A/Go",
            (TakeEdge(a_in), TakeEdge(hub)),
            (a_out,),
            |n: u32, _: u32| (n,),
        );
        b.build().expect("the net builds")
    }

    #[test]
    fn a_focused_part_shortens_its_own_names_and_drops_foreign_hub_homes() {
        let net = foreign_hub();
        let mut layout = Layout::default()
            .with_clustering(Clustering::NamePrefix('/'))
            .with_ghosting(Ghosting::named(["B/Hub"]));
        let names = |net: &Net| -> Vec<(PlaceId, String)> {
            net.places()
                .map(|(id, info)| (id, info.name.to_owned()))
                .collect()
        };
        layout.compute_subgraph(
            names(&net),
            net.transitions().map(|(id, i)| (id, i.name.to_owned())),
            net.edges(),
            net.edges(),
            Some(&["A"]),
        );
        let (a_in, hub) = (id_of(&net, "A/In"), id_of(&net, "B/Hub"));
        let go = transition_of(&net, "A/Go");
        // The hub belongs to another cluster: no home node, but its ghost.
        assert!(layout.position_of_place(hub).is_none());
        assert!(layout.ghost_position(hub, go, GhostSide::Input).is_some());
        assert!(layout.position_of_place(a_in).is_some());
        assert_eq!(layout.display_name(LayoutNode::Place(a_in), "A/In"), "In");
        assert_eq!(
            layout.display_name(LayoutNode::Place(hub), "B/Hub"),
            "B/Hub"
        );
        // A hub of the focused cluster keeps its home.
        layout.compute_subgraph(
            names(&net),
            net.transitions().map(|(id, i)| (id, i.name.to_owned())),
            net.edges(),
            net.edges(),
            Some(&["B"]),
        );
        assert!(layout.position_of_place(hub).is_some());
    }

    /// `A/Go` reads `A/In`, the hubs `B/Hub` and `C/Hub` and the plain
    /// `D/Side`, and writes `A/Out`.
    fn four_clusters() -> Net {
        let mut b = NetBuilder::new();
        let a_in = b.place::<u32>("A/In");
        let b_hub = b.place::<u32>("B/Hub");
        let c_hub = b.place::<u32>("C/Hub");
        let d_side = b.place::<u32>("D/Side");
        let a_out = b.place::<u32>("A/Out");
        b.transition(
            "A/Go",
            (
                TakeEdge(a_in),
                TakeEdge(b_hub),
                TakeEdge(c_hub),
                TakeEdge(d_side),
            ),
            (a_out,),
            |n: u32, _: u32, _: u32, _: u32| (n,),
        );
        b.build().expect("the net builds")
    }

    #[test]
    fn a_part_for_two_clusters_keeps_both_frames_and_applies_the_ghosting_rules_to_the_set() {
        let net = four_clusters();
        let mut layout = Layout::default()
            .with_clustering(Clustering::NamePrefix('/'))
            .with_ghosting(Ghosting::named(["B/Hub", "C/Hub"]));
        layout.compute_subgraph(
            net.places().map(|(id, i)| (id, i.name.to_owned())),
            net.transitions().map(|(id, i)| (id, i.name.to_owned())),
            net.edges(),
            net.edges(),
            Some(&["A", "B"]),
        );
        let [a_in, b_hub, c_hub, d_side] =
            ["A/In", "B/Hub", "C/Hub", "D/Side"].map(|n| id_of(&net, n));
        let go = transition_of(&net, "A/Go");
        // Each member keeps its frame; the foreign block has none.
        assert_eq!(layout.cluster_names().collect::<Vec<_>>(), ["A", "B"]);
        assert!(layout.cluster_bounds("D").is_none());
        assert_eq!(layout.cluster_of(LayoutNode::Place(a_in)), Some("A"));
        assert_eq!(layout.cluster_of(LayoutNode::Place(b_hub)), Some("B"));
        assert_eq!(layout.cluster_of(LayoutNode::Place(d_side)), None);
        // A ghosted hub inside the set keeps its home, one outside has none.
        assert!(layout.position_of_place(b_hub).is_some());
        assert!(layout.position_of_place(c_hub).is_none());
        assert!(layout.ghost_position(c_hub, go, GhostSide::Input).is_some());
        assert!(layout.ghost_position(b_hub, go, GhostSide::Input).is_some());
        // A plain foreign place is drawn, under its full name.
        assert!(layout.position_of_place(d_side).is_some());
        let shown = |p, n| layout.display_name(LayoutNode::Place(p), n);
        assert_eq!(shown(a_in, "A/In"), "In");
        assert_eq!(shown(b_hub, "B/Hub"), "Hub");
        assert_eq!(shown(c_hub, "C/Hub"), "C/Hub");
        assert_eq!(shown(d_side, "D/Side"), "D/Side");
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
    /// centre, each cluster's frame, the bounds, the number of ranks and of
    /// back-edges. A node with no placement counts as `-1e9`.
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
        for name in layout.cluster_names().collect::<Vec<_>>() {
            let b = layout.cluster_bounds(name).unwrap();
            out.extend([b.min.x, b.min.y, b.max.x, b.max.y]);
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

    /// A handful of nets with no subnet places, each laid out one way.
    fn pinned() -> Vec<(&'static str, Vec<f32>)> {
        let mut out = Vec::new();
        let mut run = |name: &'static str, net: &Net, mut layout: Layout| {
            layout.compute(net);
            out.push((name, fingerprint(&layout, net)));
        };
        run("plain", &net(), Layout::default());
        run("clustered", &net(), clustered(&net()));
        run(
            "ghosted",
            &hubs(),
            Layout::default().with_ghosting(Ghosting::named(["H", "H2", "H3"])),
        );
        let mut wide = Layout::default().with_direction(Direction::LeftRight);
        wide.fit_labels = true;
        run("left right, fitted", &hubs(), wide);
        run("a loop", &a_loop(), Layout::default());
        run(
            "a loop, left right, clustered",
            &a_loop(),
            Layout::default()
                .with_direction(Direction::LeftRight)
                .with_clustering(Clustering::NamePrefix('/')),
        );
        // A part for two clusters, ghosted, left to right.
        let net = four_clusters();
        let mut part = Layout::default()
            .with_direction(Direction::LeftRight)
            .with_clustering(Clustering::NamePrefix('/'))
            .with_ghosting(Ghosting::named(["B/Hub", "C/Hub"]));
        part.compute_subgraph(
            net.places().map(|(id, i)| (id, i.name.to_owned())),
            net.transitions().map(|(id, i)| (id, i.name.to_owned())),
            net.edges(),
            net.edges(),
            Some(&["A", "B"]),
        );
        out.push(("a part", fingerprint(&part, &net)));
        out
    }

    /// What [`pinned`] gave at the commit before subnet places came to the
    /// layout, written down then.
    const BEFORE: [(&str, &[f32]); 7] = [
        (
            "plain",
            &[
                0.0, 0.0, -90.0, 300.0, 2.0, 0.0, 0.0, 0.0, 4.0, 0.0, 0.0, -300.0, 0.0, 1.0, 90.0,
                300.0, 1.0, 0.0, 0.0, 150.0, 3.0, 0.0, 0.0, -150.0, -130.0, -340.0, 130.0, 340.0,
                5.0, 0.0,
            ],
        ),
        (
            "clustered",
            &[
                0.0, 0.0, -268.0, 137.0, 2.0, 0.0, -268.0, -163.0, 1.0, 0.0, 40.0, 2.0, 0.0, 0.0,
                348.0, 137.0, 1.0, 0.0, -268.0, -13.0, 0.0, 0.0, 40.0, 152.0, -412.0, -227.0,
                -184.0, 227.0, -104.0, -62.0, 124.0, 227.0, 204.0, 73.0, 412.0, 227.0, -328.0,
                -203.0, 388.0, 177.0, 3.0, 0.0,
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
        (
            "a loop, left right, clustered",
            &[
                0.0, 1.0, -235.0, -98.55441, 2.0, 0.0, 65.0, -8.554413, 0.0, 0.0, -235.0, 81.44559,
                1.0, 0.0, -85.0, -8.554413, 3.0, 0.0, 215.0, -8.554413, -299.0, -195.44559, 299.0,
                195.44559, -275.0, -138.55441, 275.0, 121.44559, 4.0, 1.0,
            ],
        ),
        (
            "a part",
            &[
                0.0, 0.0, -167.0, 125.0, 0.0, 0.0, -167.0, -157.0, -1e9, 0.0, 0.0, 41.0, -157.0,
                2.0, 0.0, 167.0, 125.0, 1.0, 0.0, 17.0, 125.0, -81.0, 165.0, -81.0, 85.0, -231.0,
                37.0, 231.0, 245.0, -231.0, -245.0, -103.0, -43.0, -207.0, -197.0, 207.0, 179.0,
                3.0, 0.0,
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
        );
        assert_eq!(by_name, by_net);
        // Without the kinds, the old drawing: every port a place of its own.
        let mut plain = Layout::default();
        plain.compute_named(
            w.net.places().map(|(id, i)| (id, i.name.to_owned())),
            w.net.transitions().map(|(id, i)| (id, i.name.to_owned())),
            w.net.edges(),
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
            None,
        );
        assert!(layout.subnet_box(w.level).is_none());
        assert!(layout.placement(LayoutNode::Place(w.lobby)).is_some());
        assert_eq!(layout.radius_of_place(w.lobby), Some(layout.place_radius));
    }

    #[test]
    fn a_box_and_its_ports_belong_to_the_box_s_cluster() {
        // `Def/Door -> Def/Enter -> Def/Level [Lobby]`, and beside it the
        // cluster `Atk`, whose `Atk/Raid` also writes the lobby.
        let mut b = NetBuilder::new();
        let door = b.place::<u32>("Def/Door");
        let level = b.subnet_place("Def/Level", ports_only(&["Lobby"]));
        let lobby = b.port_of::<u32>(level, "Lobby");
        let camp = b.place::<u32>("Atk/Camp");
        b.transition("Def/Enter", (TakeEdge(door),), (lobby,), |n: u32| (n,));
        b.transition("Atk/Raid", (TakeEdge(camp),), (lobby,), |n: u32| (n,));
        let net = b.build().expect("the net builds");
        let layout = clustered(&net);
        assert_eq!(layout.cluster_names().collect::<Vec<_>>(), ["Def", "Atk"]);
        let (level, lobby) = (level.id(), lobby.id());
        assert_eq!(layout.cluster_of(LayoutNode::Place(level)), Some("Def"));
        assert_eq!(layout.cluster_of(LayoutNode::Place(lobby)), Some("Def"));
        let frame = layout.cluster_bounds("Def").unwrap();
        let drawn = layout.subnet_box(level).unwrap().bounds();
        assert!(frame.contains(drawn.min) && frame.contains(drawn.max));
        let other = layout.cluster_bounds("Atk").unwrap();
        assert!(!overlap(other, frame));
        // The edge from the other cluster still ends at the lobby's circle,
        // which faces `Def/Enter`, the transition in its own cluster.
        assert!(layout.position_of_place(lobby).unwrap().y > drawn.center().y);
        assert_eq!(
            layout.display_name(LayoutNode::Place(lobby), "Lobby"),
            "Lobby"
        );
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
