//! The cycle check [`assert_acyclic`](NetBuilder::assert_acyclic) asks
//! [`build`](NetBuilder::build) for: the net's token flow, followed
//! through its subnet places, and the walk that finds a cycle in it.

use crate::ids::PlaceId;
use crate::net::Net;
use crate::place::PlaceSlot;
use crate::transition::TransitionSlot;

use super::NetBuilder;

/// One node of the token-flow graph
/// [`assert_acyclic`](NetBuilder::assert_acyclic) walks.
///
/// The two kinds are numbered separately, so a node is a kind and an index:
/// names are unique per kind, and a place and a transition may share one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Node {
    /// The place with this index.
    Place(u32),
    /// The transition with this index.
    Transition(u32),
}

/// How far the walk has got with a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Colour {
    /// Not reached yet.
    Unseen,
    /// On the path being walked right now: reaching it again closes a cycle.
    OnPath,
    /// Walked, along with everything reachable from it, and no cycle found
    /// through it. Reaching it again is a second way to the same place, not
    /// a loop.
    Done,
}

/// A net's token flow, as [`assert_acyclic`](NetBuilder::assert_acyclic)
/// walks it: from a transition to its output places, from a place to the
/// transitions that always take a token from it, and from a port of a
/// subnet place to every port of the same subnet place that the subnet's
/// own flow leads a token to, at any depth ([`port_paths`]).
struct Flow {
    /// Per place, the transitions that always take a token from it, in
    /// declaration order. Non-consuming edges are not token flow, and a
    /// drain, which may take nothing, is no part of the proof
    /// (`assert_acyclic`'s docs).
    consumers: Vec<Vec<u32>>,
    /// Per place, the places a token put there goes on to through a subnet:
    /// for the place a net has for a port of one of its subnet places, the
    /// places it has for the ports the subnet leads it to, in the subnet's
    /// place order. Empty for every other place.
    through: Vec<Vec<u32>>,
}

impl Flow {
    /// The flow of a net with these places and transitions: a builder's, or
    /// a built subnet's.
    fn of(places: &[PlaceSlot], transitions: &[TransitionSlot]) -> Flow {
        let mut consumers: Vec<Vec<u32>> = vec![Vec::new(); places.len()];
        for (index, slot) in transitions.iter().enumerate() {
            for (place, kind) in &slot.inputs {
                if kind.always_consumes() {
                    consumers[place.0 as usize].push(index as u32);
                }
            }
        }
        let mut through: Vec<Vec<u32>> = vec![Vec::new(); places.len()];
        for (at, slot) in places.iter().enumerate() {
            let Some(subnet) = slot.subnet.as_deref() else {
                continue;
            };
            // This net's place for each port of the subnet, by the port's
            // id there: one for every port.
            let window = |port: PlaceId| {
                places.iter().position(|slot| {
                    slot.meta
                        .window
                        .is_some_and(|window| window.subnet.0 as usize == at && window.port == port)
                })
            };
            for (from, to) in port_paths(&subnet.net) {
                let Some(from) = window(from) else {
                    continue;
                };
                through[from].extend(to.into_iter().filter_map(window).map(|to| to as u32));
            }
        }
        Flow { consumers, through }
    }

    /// Where the `index`-th edge out of `node` leads, or `None` once they are
    /// exhausted: a transition's output places; or the transitions that
    /// consume a place and then the places a subnet leads it to, each in
    /// declaration order.
    fn successor(&self, transitions: &[TransitionSlot], node: Node, index: usize) -> Option<Node> {
        match node {
            Node::Transition(id) => transitions[id as usize]
                .outputs
                .get(index)
                .map(|place| Node::Place(place.0)),
            Node::Place(id) => {
                let consumers = &self.consumers[id as usize];
                match consumers.get(index) {
                    Some(transition) => Some(Node::Transition(*transition)),
                    None => self.through[id as usize]
                        .get(index - consumers.len())
                        .map(|place| Node::Place(*place)),
                }
            }
        }
    }
}

/// For each port of a built net, by its id, the ports a token put there
/// can reach through the net's token flow, in place order: its own subnets'
/// ports included, at any depth ([`Flow`]), and the port itself among them
/// when a path leads back to it. What the net around it walks through the
/// net's subnet place (since 2026-11-28).
fn port_paths(net: &Net) -> Vec<(PlaceId, Vec<PlaceId>)> {
    let flow = Flow::of(&net.places, &net.transitions);
    let ports: Vec<PlaceId> = net
        .places
        .iter()
        .enumerate()
        .filter(|(_, slot)| slot.meta.port)
        .map(|(at, _)| PlaceId(at as u32))
        .collect();
    ports
        .iter()
        .map(|&port| {
            let mut seen_places = vec![false; net.places.len()];
            let mut seen_transitions = vec![false; net.transitions.len()];
            // From the port's successors, so that the port itself is
            // reached only by a way back to it.
            let mut stack: Vec<Node> = Vec::new();
            let mut index = 0;
            while let Some(next) = flow.successor(&net.transitions, Node::Place(port.0), index) {
                stack.push(next);
                index += 1;
            }
            while let Some(node) = stack.pop() {
                let seen = match node {
                    Node::Place(id) => &mut seen_places[id as usize],
                    Node::Transition(id) => &mut seen_transitions[id as usize],
                };
                if std::mem::replace(seen, true) {
                    continue;
                }
                let mut index = 0;
                while let Some(next) = flow.successor(&net.transitions, node, index) {
                    stack.push(next);
                    index += 1;
                }
            }
            let reached = ports
                .iter()
                .copied()
                .filter(|other| seen_places[other.0 as usize])
                .collect();
            (port, reached)
        })
        .collect()
}

/// The colour of a node, from the two vectors the walk keeps.
fn colour(node: Node, places: &[Colour], transitions: &[Colour]) -> Colour {
    match node {
        Node::Place(id) => places[id as usize],
        Node::Transition(id) => transitions[id as usize],
    }
}

/// Paints a node in whichever of the two vectors it belongs to.
fn set_colour(node: Node, places: &mut [Colour], transitions: &mut [Colour], colour: Colour) {
    match node {
        Node::Place(id) => places[id as usize] = colour,
        Node::Transition(id) => transitions[id as usize] = colour,
    }
}

impl NetBuilder {
    /// The first cycle in the net's token flow, as the names of its nodes
    /// from the node the walk closed on round to itself, or `None` if there
    /// is none.
    ///
    /// A depth-first walk with three colours (unseen, on the path, done),
    /// where meeting a node that is on the path is a cycle. The stack is
    /// explicit rather than the call stack, so that a long pipeline cannot
    /// overflow it. Every transition is a root, in declaration order, and
    /// then every place: a cycle in this net's own flow alternates places
    /// and transitions, so it contains a transition, but one that goes
    /// through a subnet place may go from port to port and back with no
    /// transition of this net on it. A node already walked was walked with
    /// everything reachable from it.
    pub(super) fn find_cycle(&self) -> Option<Vec<String>> {
        let flow = Flow::of(&self.places, &self.transitions);

        // One colour per node, in two vectors because the two kinds are
        // numbered separately.
        let mut places = vec![Colour::Unseen; self.places.len()];
        let mut transitions = vec![Colour::Unseen; self.transitions.len()];
        let roots = (0..self.transitions.len() as u32)
            .map(Node::Transition)
            .chain((0..self.places.len() as u32).map(Node::Place));
        for root in roots {
            if colour(root, &places, &transitions) != Colour::Unseen {
                continue;
            }
            set_colour(root, &mut places, &mut transitions, Colour::OnPath);
            // The path being walked, each node with how many of its
            // successors have been followed so far.
            let mut path = vec![(root, 0_usize)];
            while let Some(&(node, followed)) = path.last() {
                path.last_mut().expect("the path is not empty here").1 += 1;
                let Some(next) = flow.successor(&self.transitions, node, followed) else {
                    // Every way out of this node led nowhere: it is not on
                    // any cycle, and nothing need come back to it.
                    set_colour(node, &mut places, &mut transitions, Colour::Done);
                    path.pop();
                    continue;
                };
                match colour(next, &places, &transitions) {
                    Colour::OnPath => {
                        let from = path
                            .iter()
                            .position(|(node, _)| *node == next)
                            .expect("a node on the path is in the path");
                        let cycle: Vec<Node> = path[from..]
                            .iter()
                            .map(|(node, _)| *node)
                            .chain(std::iter::once(next))
                            .collect();
                        return Some(self.cycle_names(&cycle));
                    }
                    Colour::Done => {}
                    Colour::Unseen => {
                        set_colour(next, &mut places, &mut transitions, Colour::OnPath);
                        path.push((next, 0));
                    }
                }
            }
        }
        None
    }

    /// A cycle's nodes by name, with the way through a subnet place, from
    /// one of its ports to another, shown between the two as
    /// `(inside subnet place "S")`.
    fn cycle_names(&self, cycle: &[Node]) -> Vec<String> {
        let mut names = vec![self.node_name(cycle[0])];
        for pair in cycle.windows(2) {
            // Two places in a row: the second is a port the first leads to
            // through their subnet place.
            if let [Node::Place(_), Node::Place(to)] = *pair
                && let Some(window) = self.places[to as usize].meta.window
            {
                names.push(format!(
                    "(inside subnet place {:?})",
                    self.places[window.subnet.0 as usize].meta.name
                ));
            }
            names.push(self.node_name(pair[1]));
        }
        names
    }

    /// What to call a node in a reported cycle. Names are unique per kind,
    /// and a place and a transition may share one, so a path can name the
    /// same word twice without meaning the same node. A port of a subnet
    /// place is named by both, `Level 3/Lobby`, as a drawing names it.
    pub(super) fn node_name(&self, node: Node) -> String {
        match node {
            Node::Place(id) => {
                let meta = &self.places[id as usize].meta;
                match meta.window {
                    Some(window) => format!(
                        "{}/{}",
                        self.places[window.subnet.0 as usize].meta.name, meta.name
                    ),
                    None => meta.name.clone(),
                }
            }
            Node::Transition(id) => self.transitions[id as usize].name.clone(),
        }
    }
}
