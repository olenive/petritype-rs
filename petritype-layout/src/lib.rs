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
//! A renderer gets positions from [`Layout`] as [`Point`]s and frames as
//! [`Bounds`], two small types of this crate's own, and converts them to its
//! own vectors. The crate depends on [`petritype`] alone, and the algorithm
//! can be replaced without a renderer noticing.

#![warn(missing_docs, missing_debug_implementations, unreachable_pub)]
#![forbid(unsafe_code)]

mod geometry;

use std::collections::{HashMap, HashSet};

pub use geometry::{Bounds, Point};
use petritype::{EdgeInfo, Net, PlaceId, TransitionId};

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
            nodes: HashMap::new(),
            back_edges: HashSet::new(),
            ranks: 0,
            clusters: Vec::new(),
            cluster_index: HashMap::new(),
            ghosted: HashSet::new(),
            stripped: HashMap::new(),
            ghosts: Vec::new(),
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
    /// computed before and keeping the configuration.
    pub fn compute(&mut self, net: &Net) {
        self.compute_named(
            net.places().map(|(id, info)| (id, info.name.to_owned())),
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
    /// [`Ghosting::names`] (only [`Ghosting::min_degree`] applies): use
    /// [`compute_named`](Layout::compute_named) for those.
    pub fn compute_from(
        &mut self,
        places: impl IntoIterator<Item = PlaceId>,
        transitions: impl IntoIterator<Item = TransitionId>,
        edges: impl IntoIterator<Item = EdgeInfo>,
    ) {
        self.run(
            places.into_iter().map(|id| (id, String::new())).collect(),
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
    pub fn compute_subgraph(
        &mut self,
        places: impl IntoIterator<Item = (PlaceId, String)>,
        transitions: impl IntoIterator<Item = (TransitionId, String)>,
        edges: impl IntoIterator<Item = EdgeInfo>,
        whole: impl IntoIterator<Item = EdgeInfo>,
        focus: Option<&[&str]>,
    ) {
        self.run(
            places.into_iter().collect(),
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
    pub fn compute_named(
        &mut self,
        places: impl IntoIterator<Item = (PlaceId, String)>,
        transitions: impl IntoIterator<Item = (TransitionId, String)>,
        edges: impl IntoIterator<Item = EdgeInfo>,
    ) {
        self.run(
            places.into_iter().collect(),
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
        places: Vec<(PlaceId, String)>,
        transitions: Vec<(TransitionId, String)>,
        given: Vec<EdgeInfo>,
        whole: Option<Vec<EdgeInfo>>,
        clustered: bool,
        focus: Option<&[&str]>,
    ) {
        let mut nodes: Vec<LayoutNode> = Vec::new();
        let mut names: Vec<String> = Vec::new();
        for (id, name) in places {
            nodes.push(LayoutNode::Place(id));
            names.push(name);
        }
        for (id, name) in transitions {
            nodes.push(LayoutNode::Transition(id));
            names.push(name);
        }
        let index: HashMap<LayoutNode, usize> =
            nodes.iter().enumerate().map(|(i, &n)| (n, i)).collect();

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
        for (i, node) in nodes.iter().enumerate() {
            if let LayoutNode::Place(id) = *node
                && self
                    .ghosting
                    .applies(&names[i], degree.get(&id).copied().unwrap_or(0))
            {
                self.ghosted.insert(id);
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
        for (g, group) in members.iter().enumerate() {
            let local: HashMap<usize, usize> =
                group.iter().enumerate().map(|(l, &i)| (i, l)).collect();
            let inside: Vec<(usize, usize)> = edges
                .iter()
                .filter(|e| !e.ghosted && group_of[e.a] == g && group_of[e.b] == g)
                .map(|e| (local[&e.a], local[&e.b]))
                .collect();
            let layers = layer(group.len(), &inside);
            self.ranks = self.ranks.max(layers.layers.len());
            for (e, &(a, b)) in inside.iter().enumerate() {
                if layers.back[e] {
                    self.back_edges.insert((nodes[group[a]], nodes[group[b]]));
                }
            }
            let input = Group {
                members: group,
                names: &labels,
                nodes: &nodes,
                slots: &slots,
                inside: &inside,
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
    }

    /// Turns one group's layers into coordinates, in a frame of its own,
    /// and works out the box around it.
    fn arrange(&self, group: &Group, layers: &Layers) -> Arranged {
        // How far a node reaches before and after its centre along the main
        // axis, ghosts included, and how wide its slot in a rank is.
        let main_half = |node: LayoutNode| match (node, self.direction) {
            (LayoutNode::Place(_), _) => self.place_radius,
            (LayoutNode::Transition(_), Direction::TopDown) => self.transition_size.y / 2.0,
            (LayoutNode::Transition(_), Direction::LeftRight) => self.transition_size.x / 2.0,
        };
        let slot_of = |local: usize| group.slots.get(&group.members[local]);
        let before = |local: usize| match slot_of(local) {
            Some(s) if !s.inputs.is_empty() => {
                main_half(group.nodes[group.members[local]]) + self.ghost_depth()
            }
            _ => main_half(group.nodes[group.members[local]]),
        };
        let after = |local: usize| match slot_of(local) {
            Some(s) if !s.outputs.is_empty() => {
                main_half(group.nodes[group.members[local]]) + self.ghost_depth()
            }
            _ => main_half(group.nodes[group.members[local]]),
        };
        let width = |local: usize| {
            let count = slot_of(local).map_or(0, |s| s.inputs.len().max(s.outputs.len()));
            self.node_sep.max(count as f32 * self.ghost_sep)
        };
        let has_ghosts = |layer: &[usize]| layer.iter().any(|&l| slot_of(l).is_some());
        // How far a node and its label reach to either side across the rank,
        // from its centre.
        let room = |local: usize| {
            let global = group.members[local];
            let rect = self.reach(group.nodes[global], &group.names[global], Point::ZERO);
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
                if has_ghosts(above) || has_ghosts(layer) {
                    let need = above.iter().map(|&l| after(l)).fold(0.0, f32::max)
                        + layer.iter().map(|&l| before(l)).fold(0.0, f32::max)
                        + GHOST_CLEARANCE;
                    gap = gap.max(need);
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
                let mut reach = self.reach(group.nodes[global], &group.names[global], position);
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
    /// see it.
    pub fn placement(&self, node: LayoutNode) -> Option<Placement> {
        self.nodes.get(&node).copied()
    }

    /// The centre of a node.
    pub fn position_of(&self, node: LayoutNode) -> Option<Point> {
        self.nodes.get(&node).map(|p| p.position)
    }

    /// The centre of a place.
    pub fn position_of_place(&self, place: PlaceId) -> Option<Point> {
        self.position_of(LayoutNode::Place(place))
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
    /// round: `None` for the unnamed cluster.)
    pub fn cluster_of(&self, node: LayoutNode) -> Option<&str> {
        let &index = self.cluster_index.get(&node)?;
        Some(self.clusters[index].name.as_str())
    }

    /// How many ranks the last computation produced: in a clustered layout,
    /// the most of any cluster.
    pub fn rank_count(&self) -> usize {
        self.ranks
    }

    /// Half the width and height of a node's shape.
    pub fn half_extent(&self, node: LayoutNode) -> Point {
        match node {
            LayoutNode::Place(_) => Point::new(self.place_radius, self.place_radius),
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
        layout.compute_named([], [], []);
        layout.compute_subgraph([], [], [], [], Some(&["A"]));
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
}
