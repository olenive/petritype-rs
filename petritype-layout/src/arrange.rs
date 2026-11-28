//! One block's layers turned into coordinates, and the room each node, its
//! label and its ghosts take.

use std::collections::HashMap;

use petritype::PlaceId;

use crate::boxes::BoxPlan;
use crate::layer::Layers;
use crate::{
    Bounds, COUNT_WIDTH, Direction, FRAME_PAD, GHOST_CLEARANCE, GhostSide, LABEL_GAP, Layout,
    LayoutNode, PLACE_CHAR_WIDTH, Placement, Point,
};

/// Extra room along the top of a group's frame, for its title.
const TITLE_ROOM: f32 = 26.0;
/// The same for a transition's name, drawn in 14-point text.
const TRANSITION_CHAR_WIDTH: f32 = 8.4;
/// The distance between a transition's box and its ghosts: long enough for
/// an arrow.
const GHOST_GAP: f32 = 24.0;
/// The height of a ghost's name, beyond the ghost.
const GHOST_LABEL: f32 = 16.0;

impl Layout {
    /// Turns one block's layers into coordinates, in a frame of its own,
    /// and works out the box around it.
    pub(crate) fn arrange(&self, block: &Block, layers: &Layers) -> Arranged {
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
    pub(crate) fn main_half(&self, size: Point) -> f32 {
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
    pub(crate) fn ghost_offset(&self, half: f32, side: GhostSide, k: usize, count: usize) -> Point {
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
}

/// The ghosts of one transition: the places, in edge order.
#[derive(Default)]
pub(crate) struct Slots {
    pub(crate) inputs: Vec<PlaceId>,
    pub(crate) outputs: Vec<PlaceId>,
}

/// What [`Layout::arrange`] needs to know about one block: an open group,
/// or the nodes in no group with the closed groups' boxes.
pub(crate) struct Block<'a> {
    /// Its nodes, by index among all nodes: past the nodes given, a closed
    /// group's box.
    pub(crate) members: &'a [usize],
    /// The label of every node and box.
    pub(crate) names: &'a [String],
    /// The nodes given, without the boxes.
    pub(crate) nodes: &'a [LayoutNode],
    pub(crate) slots: &'a HashMap<usize, Slots>,
    /// The edges inside it, by index among `members`.
    pub(crate) inside: &'a [(usize, usize)],
    /// The box of each subnet place and closed group, by index among all
    /// nodes.
    pub(crate) plans: &'a [Option<BoxPlan>],
}

/// One block of nodes, laid out in a frame of its own.
#[derive(Debug, Default)]
pub(crate) struct Arranged {
    /// Each node of the block, by its index among all nodes, and where it
    /// went.
    pub(crate) placed: Vec<(usize, Placement)>,
    /// The box around it, padded for its frame.
    pub(crate) bounds: Bounds,
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
