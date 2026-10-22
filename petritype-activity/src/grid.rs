//! [`Grid`]: where each node's cell goes in a thumbnail, from the ranks and
//! orders of a whole-net layout.

use std::collections::HashMap;

use petritype::{PlaceId, Topology, TransitionId};
use petritype_layout::{Direction, Layout, LayoutNode, Point, pack};

/// One node's square in a thumbnail.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct Cell {
    /// Its column, from 0 at the left.
    pub column: usize,
    /// Its row, from 0 at the top.
    pub row: usize,
    /// The node it stands for.
    pub node: LayoutNode,
    /// The node's full name, from the topology.
    pub name: String,
}

/// Where the cells of a thumbnail go: one per place and transition, from
/// the ranks and orders of a [`Layout`], never from its coordinates.
///
/// Within a cluster, or the whole net when there are no clusters, a node's
/// cell is its rank along the main axis and its order along the cross axis,
/// in the layout's [`Direction`]: for [`TopDown`](Direction::TopDown) the
/// rank is the row and the order the column, and for
/// [`LeftRight`](Direction::LeftRight) the other way round, so the thumbnail
/// keeps the drawing's orientation. A rank shorter than the longest in its
/// cluster is centred across it in whole cells, rounding toward the start,
/// as the drawing centres it. Clusters are packed in rows, in the
/// order the layout gives them, with a gap of one cell, by the rule the
/// layout packs them with ([`pack`]) in cell units. Ghosts are drawing aids
/// and get no cell.
///
/// The grid is meant for a layout of the whole net, which is the overview
/// whatever a view's filter shows. It is deterministic because the layout
/// is. Positions with no node are empty: a thumbnail draws them in the page
/// colour.
///
/// With the `serde` feature it is written as its columns, its rows and its
/// cells, and reading it back refuses a cell outside the grid or on another
/// cell's position.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(into = "GridData", try_from = "GridData")
)]
pub struct Grid {
    columns: usize,
    rows: usize,
    /// In topology order: the places, then the transitions.
    cells: Vec<Cell>,
    /// Row-major, `columns * rows` long: which of `cells` is there.
    at: Vec<Option<usize>>,
    by_node: HashMap<LayoutNode, usize>,
}

impl Grid {
    /// The grid of the net `topology` describes, from a `layout` computed
    /// over that same net, whole.
    ///
    /// The layout holds ids and placements but no names, so the topology
    /// gives the nodes, in declaration order, and the name each cell
    /// carries. A node the layout did not place gets no cell; the layout
    /// of a filtered part would leave some out, which is why the grid wants
    /// the whole net's.
    pub fn from_layout(layout: &Layout, topology: &Topology) -> Grid {
        Grid::from_layout_named(
            layout,
            topology.places.iter().map(|p| (p.id, p.name.clone())),
            topology.transitions.iter().map(|t| (t.id, t.name.clone())),
        )
    }

    /// The same as [`from_layout`](Grid::from_layout), with the nodes given
    /// by id and name rather than read off a [`Topology`]: for a renderer
    /// that reads the net from a source that has none, such as a mirror of
    /// a net in another process, as
    /// [`Layout::compute_named`](petritype_layout::Layout::compute_named)
    /// takes them. The places first and then the transitions, each in
    /// declaration order.
    pub fn from_layout_named(
        layout: &Layout,
        places: impl IntoIterator<Item = (PlaceId, String)>,
        transitions: impl IntoIterator<Item = (TransitionId, String)>,
    ) -> Grid {
        let nodes = places
            .into_iter()
            .map(|(id, name)| (LayoutNode::Place(id), name))
            .chain(
                transitions
                    .into_iter()
                    .map(|(id, name)| (LayoutNode::Transition(id), name)),
            );

        // The groups packed: each cluster in the layout's order, then one
        // for any node in no cluster, which is every node of an unclustered
        // layout.
        let clusters: Vec<&str> = layout.cluster_names().collect();
        let group_of = |node: LayoutNode| {
            layout
                .cluster_of(node)
                .and_then(|name| clusters.iter().position(|c| *c == name))
                .unwrap_or(clusters.len())
        };
        // Each node's group, rank and order, and each rank's length across:
        // its largest order plus one.
        let mut placed: Vec<(usize, usize, usize, LayoutNode, String)> = Vec::new();
        let mut rank_length: HashMap<(usize, usize), usize> = HashMap::new();
        for (node, name) in nodes {
            let Some(placement) = layout.placement(node) else {
                continue;
            };
            let group = group_of(node);
            let length = rank_length.entry((group, placement.rank)).or_default();
            *length = (*length).max(placement.order + 1);
            placed.push((group, placement.rank, placement.order, node, name));
        }
        // Each group's extent across: its longest rank.
        let mut across = vec![0_usize; clusters.len() + 1];
        for (&(group, _), &length) in &rank_length {
            across[group] = across[group].max(length);
        }

        // Within its group, a node's cell is its rank along the main axis and
        // its order along the cross axis, each rank centred in whole cells
        // within the group's longest, as the drawing centres it.
        let mut sizes = vec![(0_usize, 0_usize); clusters.len() + 1];
        let mut local: Vec<(usize, usize, usize, LayoutNode, String)> = Vec::new();
        for (group, rank, order, node, name) in placed {
            let order = order + (across[group] - rank_length[&(group, rank)]) / 2;
            let (column, row) = match layout.direction {
                Direction::TopDown => (order, rank),
                Direction::LeftRight => (rank, order),
            };
            sizes[group].0 = sizes[group].0.max(column + 1);
            sizes[group].1 = sizes[group].1.max(row + 1);
            local.push((group, column, row, node, name));
        }

        // Only the groups with nodes take room.
        let present: Vec<usize> = (0..sizes.len()).filter(|&g| sizes[g].0 > 0).collect();
        let boxes: Vec<Point> = present
            .iter()
            .map(|&g| Point::new(sizes[g].0 as f32, sizes[g].1 as f32))
            .collect();
        let mut offset = vec![(0_usize, 0_usize); sizes.len()];
        for (&g, corner) in present.iter().zip(pack(&boxes, 1.0)) {
            // Corners are whole cells, the first row at y = 0 and the rest
            // below it.
            offset[g] = (corner.x.round() as usize, (-corner.y).round() as usize);
        }

        let cells: Vec<Cell> = local
            .into_iter()
            .map(|(group, column, row, node, name)| Cell {
                column: offset[group].0 + column,
                row: offset[group].1 + row,
                node,
                name,
            })
            .collect();
        let columns = cells.iter().map(|c| c.column + 1).max().unwrap_or(0);
        let rows = cells.iter().map(|c| c.row + 1).max().unwrap_or(0);
        Grid::of_cells(columns, rows, cells)
            .expect("every cell is inside, and alone on its position")
    }

    /// The grid of `cells` in `columns` by `rows`, or why there is none: a
    /// cell outside it, or two on one position or for one node.
    fn of_cells(columns: usize, rows: usize, cells: Vec<Cell>) -> Result<Grid, String> {
        let mut at = vec![None; columns * rows];
        let mut by_node = HashMap::with_capacity(cells.len());
        for (i, cell) in cells.iter().enumerate() {
            if cell.column >= columns || cell.row >= rows {
                return Err(format!(
                    "cell {} at ({}, {}) is outside a grid of {columns} by {rows}",
                    cell.name, cell.column, cell.row
                ));
            }
            let slot = &mut at[cell.row * columns + cell.column];
            if slot.replace(i).is_some() || by_node.insert(cell.node, i).is_some() {
                return Err(format!(
                    "cell {} at ({}, {}) shares its position or its node",
                    cell.name, cell.column, cell.row
                ));
            }
        }
        Ok(Grid {
            columns,
            rows,
            cells,
            at,
            by_node,
        })
    }

    /// How many columns wide the grid is.
    pub fn columns(&self) -> usize {
        self.columns
    }

    /// How many rows high the grid is.
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Whether the grid has no cells at all: an empty net.
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Every node's cell, the places first and then the transitions, each in
    /// declaration order.
    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    /// The cell at a column and row, or `None` for an empty position or one
    /// outside the grid: what a click on the thumbnail lands on.
    pub fn at(&self, column: usize, row: usize) -> Option<&Cell> {
        if column >= self.columns || row >= self.rows {
            return None;
        }
        self.at[row * self.columns + column].map(|i| &self.cells[i])
    }

    /// A node's cell, or `None` for a node the layout did not place.
    pub fn cell_of(&self, node: LayoutNode) -> Option<&Cell> {
        self.by_node.get(&node).map(|&i| &self.cells[i])
    }
}

/// A grid as written: its size and its cells, from which the lookups are
/// built again on reading.
#[cfg(feature = "serde")]
#[derive(serde::Serialize, serde::Deserialize)]
struct GridData {
    columns: usize,
    rows: usize,
    cells: Vec<Cell>,
}

#[cfg(feature = "serde")]
impl From<Grid> for GridData {
    fn from(grid: Grid) -> Self {
        GridData {
            columns: grid.columns,
            rows: grid.rows,
            cells: grid.cells,
        }
    }
}

#[cfg(feature = "serde")]
impl TryFrom<GridData> for Grid {
    type Error = String;

    fn try_from(data: GridData) -> Result<Self, Self::Error> {
        Grid::of_cells(data.columns, data.rows, data.cells)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use petritype::{Net, NetBuilder, PlaceId, TakeEdge, TransitionId};
    use petritype_layout::{Clustering, Ghosting};

    use super::*;

    /// Two clusters and a hub: `A/In -> A/Go -> A/Out`, `B/In -> B/Go ->
    /// B/Mid -> B/Next -> B/Out`, and `Hub`, read by both `Go`s.
    fn clustered() -> Net {
        let mut b = NetBuilder::new();
        let a_in = b.place::<u32>("A/In");
        let a_out = b.place::<u32>("A/Out");
        let b_in = b.place::<u32>("B/In");
        let b_mid = b.place::<u32>("B/Mid");
        let b_out = b.place::<u32>("B/Out");
        let hub = b.place::<u32>("Hub");
        b.transition(
            "A/Go",
            (TakeEdge(a_in), TakeEdge(hub)),
            (a_out,),
            |n: u32, _: u32| (n,),
        );
        b.transition(
            "B/Go",
            (TakeEdge(b_in), TakeEdge(hub)),
            (b_mid,),
            |n: u32, _: u32| (n,),
        );
        b.transition("B/Next", (TakeEdge(b_mid),), (b_out,), |n: u32| (n,));
        b.build().expect("the net builds")
    }

    fn grid(net: &Net, layout: Layout) -> Grid {
        let mut layout = layout;
        layout.compute(net);
        Grid::from_layout(&layout, &net.topology())
    }

    fn clustered_layout() -> Layout {
        Layout::default()
            .with_clustering(Clustering::NamePrefix('/'))
            .with_ghosting(Ghosting::named(["Hub"]))
    }

    #[test]
    fn a_clustered_net_gives_the_same_grid_every_time() {
        let net = clustered();
        let first = grid(&net, clustered_layout());
        assert_eq!(first, grid(&net, clustered_layout()));
        assert_eq!(first, grid(&clustered(), clustered_layout()));
    }

    #[test]
    fn every_node_has_one_cell_and_no_two_share_one() {
        let net = clustered();
        let grid = grid(&net, clustered_layout());
        let nodes = net.places().count() + net.transitions().count();
        assert_eq!(grid.cells().len(), nodes);
        let positions: HashSet<(usize, usize)> =
            grid.cells().iter().map(|c| (c.column, c.row)).collect();
        assert_eq!(positions.len(), nodes, "{:?}", grid.cells());
        for cell in grid.cells() {
            assert!(cell.column < grid.columns() && cell.row < grid.rows());
            assert_eq!(grid.at(cell.column, cell.row), Some(cell));
            assert_eq!(grid.cell_of(cell.node), Some(cell));
        }
        let filled = (0..grid.rows())
            .flat_map(|r| (0..grid.columns()).map(move |c| (c, r)))
            .filter(|&(c, r)| grid.at(c, r).is_some())
            .count();
        assert_eq!(filled, nodes);
        assert_eq!(grid.at(grid.columns(), 0), None);
    }

    #[test]
    fn a_grid_from_names_is_the_grid_from_the_topology() {
        let net = clustered();
        let mut layout = clustered_layout();
        layout.compute(&net);
        let named = Grid::from_layout_named(
            &layout,
            net.places().map(|(id, info)| (id, info.name.to_owned())),
            net.transitions()
                .map(|(id, info)| (id, info.name.to_owned())),
        );
        assert_eq!(named, Grid::from_layout(&layout, &net.topology()));
    }

    #[test]
    fn ghosts_get_no_cell_and_their_home_keeps_one() {
        let net = clustered();
        let mut layout = clustered_layout();
        layout.compute(&net);
        assert!(layout.ghosts().count() > 0, "the hub is ghosted");
        let grid = Grid::from_layout(&layout, &net.topology());
        let hub = net.place_named("Hub").unwrap();
        let homes: Vec<&Cell> = grid
            .cells()
            .iter()
            .filter(|c| c.node == LayoutNode::Place(hub))
            .collect();
        assert_eq!(homes.len(), 1);
        assert_eq!(homes[0].name, "Hub");
        let nodes = net.places().count() + net.transitions().count();
        assert_eq!(grid.cells().len(), nodes, "a cell per node, none per ghost");
    }

    #[test]
    fn clusters_are_packed_apart_by_a_gap_of_one_cell() {
        let net = clustered();
        let grid = grid(&net, clustered_layout());
        let mut boxes: HashMap<&str, (usize, usize, usize, usize)> = HashMap::new();
        for cell in grid.cells() {
            let cluster = cell.name.split_once('/').map_or("", |(c, _)| c);
            let b = boxes
                .entry(cluster)
                .or_insert((cell.column, cell.row, cell.column, cell.row));
            *b = (
                b.0.min(cell.column),
                b.1.min(cell.row),
                b.2.max(cell.column),
                b.3.max(cell.row),
            );
        }
        assert_eq!(boxes.len(), 3, "A, B and the unnamed");
        let list: Vec<_> = boxes.values().collect();
        for (i, a) in list.iter().enumerate() {
            for b in &list[i + 1..] {
                let apart_x = a.2 + 1 < b.0 || b.2 + 1 < a.0;
                let apart_y = a.3 + 1 < b.1 || b.3 + 1 < a.1;
                assert!(apart_x || apart_y, "{a:?} {b:?}");
            }
        }
    }

    /// `P0 -> T0 -> P1 -> T1 -> P2`.
    fn chain() -> Net {
        let mut b = NetBuilder::new();
        let p0 = b.place::<u32>("P0");
        let p1 = b.place::<u32>("P1");
        let p2 = b.place::<u32>("P2");
        b.transition("T0", (TakeEdge(p0),), (p1,), |n: u32| (n,));
        b.transition("T1", (TakeEdge(p1),), (p2,), |n: u32| (n,));
        b.build().expect("the net builds")
    }

    #[test]
    fn the_orientation_follows_the_layout_direction() {
        let net = chain();
        let down = grid(&net, Layout::default());
        assert_eq!((down.columns(), down.rows()), (1, 5));
        let across = grid(&net, Layout::default().with_direction(Direction::LeftRight));
        assert_eq!((across.columns(), across.rows()), (5, 1));
        let order = [
            LayoutNode::Place(PlaceId::new(0)),
            LayoutNode::Transition(TransitionId::new(0)),
            LayoutNode::Place(PlaceId::new(1)),
            LayoutNode::Transition(TransitionId::new(1)),
            LayoutNode::Place(PlaceId::new(2)),
        ];
        for (rank, node) in order.into_iter().enumerate() {
            assert_eq!(
                down.cell_of(node).map(|c| (c.column, c.row)),
                Some((0, rank))
            );
            assert_eq!(
                across.cell_of(node).map(|c| (c.column, c.row)),
                Some((rank, 0))
            );
        }
    }

    #[test]
    fn within_a_rank_the_order_runs_across_the_main_axis() {
        // Two sources feed one transition: rank 0 holds both, in order.
        let mut b = NetBuilder::new();
        let left = b.place::<u32>("Left");
        let right = b.place::<u32>("Right");
        let out = b.place::<u32>("Out");
        b.transition(
            "Join",
            (TakeEdge(left), TakeEdge(right)),
            (out,),
            |a: u32, _: u32| (a,),
        );
        let net = b.build().expect("the net builds");
        let down = grid(&net, Layout::default());
        assert_eq!((down.columns(), down.rows()), (2, 3));
        let across = grid(&net, Layout::default().with_direction(Direction::LeftRight));
        assert_eq!((across.columns(), across.rows()), (3, 2));
        let (l, r) = (LayoutNode::Place(left.id()), LayoutNode::Place(right.id()));
        let at = |g: &Grid, n| g.cell_of(n).map(|c| (c.column, c.row)).unwrap();
        assert_eq!(at(&down, l).1, at(&down, r).1, "one row");
        assert_eq!(at(&across, l).0, at(&across, r).0, "one column");
    }

    /// `width` sources in rank 0, and a transition for each `share` of them
    /// in rank 1, each writing to a place of its own in rank 2.
    fn fan_in(width: usize, share: usize) -> (Net, Vec<TransitionId>) {
        let mut b = NetBuilder::new();
        let sources: Vec<_> = (0..width)
            .map(|i| b.place::<u32>(&format!("S{i}")))
            .collect();
        let mut joins = Vec::new();
        for (j, group) in sources.chunks(share).enumerate() {
            let out = b.place::<u32>(&format!("Out{j}"));
            let name = format!("Join{j}");
            let join = match group {
                [a] => b.transition(&name, (TakeEdge(*a),), (out,), |n: u32| (n,)),
                [a, c] => b.transition(
                    &name,
                    (TakeEdge(*a), TakeEdge(*c)),
                    (out,),
                    |n: u32, _: u32| (n,),
                ),
                [a, c, d] => b.transition(
                    &name,
                    (TakeEdge(*a), TakeEdge(*c), TakeEdge(*d)),
                    (out,),
                    |n: u32, _: u32, _: u32| (n,),
                ),
                _ => unreachable!("one to three inputs"),
            };
            joins.push(join.id());
        }
        (b.build().expect("the net builds"), joins)
    }

    #[test]
    fn a_short_rank_is_centred_in_whole_cells() {
        // Ranks of 3 and 1: the lone node in column 1.
        let (net, joins) = fan_in(3, 3);
        let down = grid(&net, Layout::default());
        assert_eq!((down.columns(), down.rows()), (3, 3));
        let join = LayoutNode::Transition(joins[0]);
        assert_eq!(down.cell_of(join).map(|c| (c.column, c.row)), Some((1, 1)));
        let out = LayoutNode::Place(net.place_named("Out0").unwrap());
        assert_eq!(down.cell_of(out).map(|c| (c.column, c.row)), Some((1, 2)));
        // Across, the same in rows.
        let across = grid(&net, Layout::default().with_direction(Direction::LeftRight));
        assert_eq!(
            across.cell_of(join).map(|c| (c.column, c.row)),
            Some((1, 1))
        );

        // Ranks of 4 and 2: the pair in columns 1 and 2.
        let (net, joins) = fan_in(4, 2);
        let down = grid(&net, Layout::default());
        assert_eq!(down.columns(), 4);
        let mut columns: Vec<usize> = joins
            .iter()
            .map(|&t| down.cell_of(LayoutNode::Transition(t)).unwrap().column)
            .collect();
        columns.sort_unstable();
        assert_eq!(columns, [1, 2]);
        // Every cell inside the grid, none sharing a position, each found.
        let positions: HashSet<(usize, usize)> =
            down.cells().iter().map(|c| (c.column, c.row)).collect();
        assert_eq!(positions.len(), down.cells().len());
        for cell in down.cells() {
            assert!(cell.column < down.columns() && cell.row < down.rows());
            assert_eq!(down.at(cell.column, cell.row), Some(cell));
        }
    }

    #[test]
    fn an_empty_net_has_an_empty_grid() {
        let net = NetBuilder::new().build().expect("an empty net builds");
        let grid = grid(&net, clustered_layout());
        assert!(grid.is_empty());
        assert_eq!((grid.columns(), grid.rows()), (0, 0));
        assert_eq!(grid.at(0, 0), None);
    }
}
