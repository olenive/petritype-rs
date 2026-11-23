//! Groups in the layout (`dev-docs/group-spec.md` §4): open groups against
//! the clusters by name prefix they replace, nets with no groups unchanged
//! to the number, and closed groups.
//!
//! The pinned numbers at the bottom are what the layout computed at
//! eb323d3, the commit before groups came to it, for the same nets: then
//! with each node's group as the prefix of its name, `Group/Name`, and
//! clustering by that prefix; now with declared groups and the bare names.
//!
//! The one change since: the width reckoned for a subnet place's title, where a
//! place's name is now reckoned nine units a character and was eight.

use petritype::{EdgeInfo, GroupRef, Net, NetBuilder, PlaceId, PlaceRef, TakeEdge, TransitionId};
use petritype_layout::{
    ArrowEnd, Bounds, Direction, GhostSide, Ghosting, Layout, LayoutNode, LayoutPlace,
    LayoutTransition, Point,
};

// ----- nets with no groups ----------------------------------------------

/// `A/In -> A/Go -> A/Out -> B/Go -> B/Out`, and `Lone`: names that hold a
/// `/`, which means nothing.
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

/// `Queue -> Dispatch -> Staging -> Return -> Queue`, and `Lone`.
fn a_loop() -> Net {
    let mut b = NetBuilder::new();
    let queue = b.place::<u32>("Queue");
    let staging = b.place::<u32>("Staging Area");
    let _lone = b.place::<u32>("Lone");
    b.transition("Dispatch", (TakeEdge(queue),), (staging,), |n: u32| (n,));
    b.transition("Return", (TakeEdge(staging),), (queue,), |n: u32| (n,));
    b.build().expect("the net builds")
}

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

/// `Door -> Enter -> Level 3 [Lobby | Exit Hall] -> Leave -> Out`.
fn world() -> Net {
    let mut b = NetBuilder::new();
    let door = b.place::<u32>("Door");
    let level = b.subnet_place("Level 3", ports_only(&["Lobby", "Exit Hall"]));
    let lobby = b.port_of::<u32>(level, "Lobby");
    let exit = b.port_of::<u32>(level, "Exit Hall");
    let out = b.place::<u32>("Out");
    b.transition("Enter", (TakeEdge(door),), (lobby,), |n: u32| (n,));
    b.transition("Leave", (TakeEdge(exit),), (out,), |n: u32| (n,));
    b.build().expect("the world builds")
}

/// `P1 -> T1 -> C`, `P2 -> T2 -> B`, `P3 -> T3 -> A`, the ports of `Box`,
/// and `U`, a fourth port nothing uses.
fn crossing() -> Net {
    let mut b = NetBuilder::new();
    let p = ["P1", "P2", "P3"].map(|n| b.place::<u32>(n));
    let subnet = b.subnet_place("Box", ports_only(&["A", "B", "C", "U"]));
    let [a, b_, c, _u] = ["A", "B", "C", "U"].map(|n| b.port_of::<u32>(subnet, n));
    b.transition("T1", (TakeEdge(p[0]),), (c,), |n: u32| (n,));
    b.transition("T2", (TakeEdge(p[1]),), (b_,), |n: u32| (n,));
    b.transition("T3", (TakeEdge(p[2]),), (a,), |n: u32| (n,));
    b.build().expect("the net builds")
}

/// A subnet place whose ports are each written from above, read from
/// below, both or neither, as `uses` says.
fn many(uses: &[(bool, bool)]) -> Net {
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
    b.build().expect("the net builds")
}

// ----- nets built two ways: a name prefix then, a declared group now -----

/// Builds a net either way: with each node's group as the prefix of its
/// name, as the nets were clustered at eb323d3, or with declared groups and
/// the bare names. A group is declared at its first use, which is also the
/// order the clusters came in.
struct Two {
    b: NetBuilder,
    declared: bool,
    groups: Vec<(String, GroupRef)>,
}

impl Two {
    fn new(declared: bool) -> Self {
        Two {
            b: NetBuilder::new(),
            declared,
            groups: Vec::new(),
        }
    }

    fn name(&self, group: &str, name: &str) -> String {
        if self.declared {
            name.to_owned()
        } else {
            format!("{group}/{name}")
        }
    }

    fn group(&mut self, group: &str) -> GroupRef {
        if let Some((_, g)) = self.groups.iter().find(|(n, _)| n == group) {
            return *g;
        }
        let g = self.b.group(group);
        self.groups.push((group.to_owned(), g));
        g
    }

    fn place(&mut self, group: &str, name: &str) -> PlaceRef<u32> {
        let full = self.name(group, name);
        let place = self.b.place::<u32>(full);
        if self.declared {
            let g = self.group(group);
            self.b.in_group(g, place);
        }
        place
    }

    /// A transition with one input and one output.
    fn pipe(&mut self, group: &str, name: &str, from: PlaceRef<u32>, to: PlaceRef<u32>) {
        let full = self.name(group, name);
        let g = self.declared.then(|| self.group(group));
        let mut t = self
            .b
            .transition(full, (TakeEdge(from),), (to,), |n: u32| (n,));
        if let Some(g) = g {
            t.group(g);
        }
    }

    fn build(self) -> Net {
        self.b.build().expect("the net builds")
    }
}

/// `A/In -> A/Start -> A/Mid -> B/Finish -> B/Out`, and `B/Side`.
fn two_groups(declared: bool) -> Net {
    let mut h = Two::new(declared);
    let a_in = h.place("A", "In");
    let a_mid = h.place("A", "Mid");
    let b_out = h.place("B", "Out");
    let _side = h.place("B", "Side");
    h.pipe("A", "Start", a_in, a_mid);
    h.pipe("B", "Finish", a_mid, b_out);
    h.build()
}

/// `A/Go` reads `A/In`, `B/Hub`, `C/Spare` and `D/Side`, and writes `A/Out`.
fn four_groups(declared: bool) -> Net {
    let mut h = Two::new(declared);
    let a_in = h.place("A", "In");
    let hub = h.place("B", "Hub");
    let spare = h.place("C", "Spare");
    let side = h.place("D", "Side");
    let a_out = h.place("A", "Out");
    let full = h.name("A", "Go");
    let g = h.declared.then(|| h.group("A"));
    let mut t = h.b.transition(
        full,
        (
            TakeEdge(a_in),
            TakeEdge(hub),
            TakeEdge(spare),
            TakeEdge(side),
        ),
        (a_out,),
        |n: u32, _: u32, _: u32, _: u32| (n,),
    );
    if let Some(g) = g {
        t.group(g);
    }
    h.build()
}

/// `L/Queue -> L/Dispatch -> L/Staging Area -> L/Return -> L/Queue`, and
/// `M/Lone`.
fn grouped_loop(declared: bool) -> Net {
    let mut h = Two::new(declared);
    let queue = h.place("L", "Queue");
    let staging = h.place("L", "Staging Area");
    let _lone = h.place("M", "Lone");
    h.pipe("L", "Dispatch", queue, staging);
    h.pipe("L", "Return", staging, queue);
    h.build()
}

/// `Def/Door -> Def/Enter -> Def/Level [Lobby]`, and `Atk/Camp -> Atk/Raid
/// -> Lobby`.
fn grouped_subnet(declared: bool) -> Net {
    let mut h = Two::new(declared);
    let door = h.place("Def", "Door");
    let full = h.name("Def", "Level");
    let level = h.b.subnet_place(full, ports_only(&["Lobby"]));
    if declared {
        let g = h.group("Def");
        h.b.in_group(g, level);
    }
    let lobby = h.b.port_of::<u32>(level, "Lobby");
    let camp = h.place("Atk", "Camp");
    h.pipe("Def", "Enter", door, lobby);
    h.pipe("Atk", "Raid", camp, lobby);
    h.build()
}

/// `A/In -> A/Run -> A/Out -> B/Go -> B/End`, and `Lone` in no group.
fn mixed(declared: bool) -> Net {
    let mut h = Two::new(declared);
    let a_in = h.place("A", "In");
    let a_out = h.place("A", "Out");
    let b_end = h.place("B", "End");
    let _lone = h.b.place::<u32>("Lone");
    h.pipe("A", "Run", a_in, a_out);
    h.pipe("B", "Go", a_out, b_end);
    h.build()
}

// ----- the numbers -------------------------------------------------------

/// What a layout answered at eb323d3, for one net: each place's and then
/// each transition's `[rank, order, x, y, frame]` (a port's rank and order
/// `-1`, a node with no position at `-1e9`, the frame its index among
/// `frames` or `-1`), each ghost's `[place, transition, side, x, y]`, the
/// frames by name and bounds, each subnet box's numbers in a row, the
/// bounds, the number of ranks and every back-edge.
struct Pinned {
    name: &'static str,
    nodes: &'static [[f32; 5]],
    ghosts: &'static [[f32; 5]],
    frames: &'static [&'static str],
    frame_bounds: &'static [[f32; 4]],
    boxes: &'static [f32],
    bounds: &'static [f32],
    ranks: usize,
    back: &'static [[f32; 2]],
}

/// The same numbers, from a layout now.
#[derive(Debug, PartialEq)]
struct Numbers {
    nodes: Vec<[f32; 5]>,
    ghosts: Vec<[f32; 5]>,
    frames: Vec<String>,
    frame_bounds: Vec<[f32; 4]>,
    boxes: Vec<f32>,
    bounds: Vec<f32>,
    ranks: usize,
    back: Vec<[f32; 2]>,
}

impl Pinned {
    fn numbers(&self) -> Numbers {
        Numbers {
            nodes: self.nodes.to_vec(),
            ghosts: self.ghosts.to_vec(),
            frames: self.frames.iter().map(|f| (*f).to_owned()).collect(),
            frame_bounds: self.frame_bounds.to_vec(),
            boxes: self.boxes.to_vec(),
            bounds: self.bounds.to_vec(),
            ranks: self.ranks,
            back: self.back.to_vec(),
        }
    }
}

fn code(node: LayoutNode) -> f32 {
    match node {
        LayoutNode::Place(p) => p.index() as f32,
        LayoutNode::Transition(t) => 1000.0 + t.index() as f32,
    }
}

fn b4(b: Bounds) -> [f32; 4] {
    [b.min.x, b.min.y, b.max.x, b.max.y]
}

fn nodes_of(net: &Net) -> Vec<LayoutNode> {
    net.places()
        .map(|(id, _)| LayoutNode::Place(id))
        .chain(net.transitions().map(|(id, _)| LayoutNode::Transition(id)))
        .collect()
}

fn numbers(layout: &Layout, net: &Net) -> Numbers {
    let frames: Vec<String> = layout.group_names().map(str::to_owned).collect();
    let nodes = nodes_of(net)
        .into_iter()
        .map(|node| {
            let c = layout
                .group_of(node)
                .and_then(|n| frames.iter().position(|f| f == n))
                .map_or(-1.0, |i| i as f32);
            match layout.placement(node) {
                Some(p) => [p.rank as f32, p.order as f32, p.position.x, p.position.y, c],
                None => match layout.position_of(node) {
                    Some(at) => [-1.0, -1.0, at.x, at.y, c],
                    None => [-1.0, -1.0, -1e9, -1e9, c],
                },
            }
        })
        .collect();
    let ghosts = layout
        .ghosts()
        .map(|(p, t, s)| {
            let at = layout.ghost_position(p, t, s).unwrap();
            let side = if s == GhostSide::Input { 0.0 } else { 1.0 };
            [p.index() as f32, t.index() as f32, side, at.x, at.y]
        })
        .collect();
    let frame_bounds = frames
        .iter()
        .filter_map(|f| layout.group_bounds(f))
        .map(b4)
        .collect();
    let mut boxes = Vec::new();
    for (id, _) in net.places() {
        if let Some(drawn) = layout.subnet_box(id) {
            boxes.extend([
                id.index() as f32,
                drawn.centre.x,
                drawn.centre.y,
                drawn.size.x,
                drawn.size.y,
                drawn.corner_radius,
            ]);
            boxes.extend(b4(drawn.title).into_iter().chain(b4(drawn.count)));
            for c in &drawn.ports {
                boxes.extend([c.place.index() as f32, c.centre.x, c.centre.y, c.radius]);
                boxes.extend(b4(c.count));
            }
        }
    }
    let mut back: Vec<[f32; 2]> = layout
        .back_edges()
        .map(|(a, b)| [code(a), code(b)])
        .collect();
    back.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Numbers {
        nodes,
        ghosts,
        frames,
        frame_bounds,
        boxes,
        bounds: layout.bounds().map(|b| b4(b).to_vec()).unwrap_or_default(),
        ranks: layout.rank_count(),
        back,
    }
}

fn whole(name: &'static str, net: &Net, mut layout: Layout) -> (&'static str, Numbers) {
    layout.compute(net);
    (name, numbers(&layout, net))
}

/// The edges of these transitions.
fn of(net: &Net, transitions: &[TransitionId]) -> Vec<EdgeInfo> {
    net.edges()
        .filter(|e| match e {
            EdgeInfo::Input { transition, .. } | EdgeInfo::Output { transition, .. } => {
                transitions.contains(transition)
            }
            _ => false,
        })
        .collect()
}

fn lr() -> Layout {
    Layout::default().with_direction(Direction::LeftRight)
}

fn fitted(mut layout: Layout) -> Layout {
    layout.fit_labels = true;
    layout
}

/// [`NO_GROUPS`], laid out now.
fn no_groups() -> Vec<(&'static str, Numbers)> {
    let uses = [(true, false), (false, true), (true, true), (false, false)];
    let mut out = vec![
        whole("plain", &net(), Layout::default()),
        whole(
            "ghosted",
            &hubs(),
            Layout::default().with_ghosting(Ghosting::named(["H", "H2", "H3"])),
        ),
        whole("left right, fitted", &hubs(), fitted(lr())),
        whole("a loop", &a_loop(), Layout::default()),
        whole("a loop, left right", &a_loop(), lr()),
        whole(
            "hubs by degree, fitted",
            &hubs(),
            fitted(Layout::default().with_ghosting(Ghosting::default().with_min_degree(3))),
        ),
        whole("world", &world(), Layout::default()),
        whole(
            "world, left right, fitted, lobby ghosted",
            &world(),
            fitted(lr().with_ghosting(Ghosting::named(["Lobby"]))),
        ),
        whole("crossing", &crossing(), Layout::default()),
        whole("crossing, left right", &crossing(), lr()),
        whole("many, fitted", &many(&uses), fitted(Layout::default())),
        whole("many, left right", &many(&uses), lr()),
    ];
    {
        let net = hubs();
        let t1 = net.transition_named("T1").unwrap();
        let wide = net.transition_named("Wide").unwrap();
        let mut layout = Layout::default().with_ghosting(Ghosting::default().with_min_degree(3));
        layout.compute_subgraph(
            net.places()
                .filter(|(_, i)| i.name != "Y")
                .map(|(id, i)| (id, i.name.to_owned())),
            net.transitions()
                .filter(|(id, _)| [t1, wide].contains(id))
                .map(|(id, i)| (id, i.name.to_owned())),
            of(&net, &[t1, wide]),
            net.edges(),
            &[],
            None,
        );
        out.push(("a part of the hubs", numbers(&layout, &net)));
    }
    {
        let net = world();
        let enter = net.transition_named("Enter").unwrap();
        let mut layout = Layout::default();
        layout.compute_subgraph(
            net.places().map(LayoutPlace::from),
            net.transitions()
                .filter(|(id, _)| *id == enter)
                .map(LayoutTransition::from),
            of(&net, &[enter]),
            net.edges(),
            &[],
            None,
        );
        out.push(("a part of the world", numbers(&layout, &net)));
    }
    out
}

/// A part of `net`, for the groups in `focus`, holding the places `keep`
/// says and every transition, with the net's groups in their order.
fn part_for(net: &Net, layout: &mut Layout, focus: &[&str], keep: impl Fn(Option<&str>) -> bool) {
    let groups: Vec<&str> = net.groups().collect();
    layout.compute_subgraph(
        net.places()
            .filter(|(_, i)| keep(i.group))
            .map(LayoutPlace::from),
        net.transitions().map(LayoutTransition::from),
        net.edges(),
        net.edges(),
        &groups,
        Some(focus),
    );
}

/// [`EVERY_NODE_GROUPED`], laid out now with declared groups, all open.
fn every_node_grouped() -> Vec<(&'static str, Numbers)> {
    let mut out = vec![
        whole("two groups", &two_groups(true), Layout::default()),
        whole("two groups, left right", &two_groups(true), lr()),
        whole(
            "four groups, ghosted",
            &four_groups(true),
            Layout::default().with_ghosting(Ghosting::named(["Hub", "Spare"])),
        ),
        whole("a loop in a group, left right", &grouped_loop(true), lr()),
        whole(
            "a subnet in a group",
            &grouped_subnet(true),
            Layout::default(),
        ),
        whole(
            "a subnet in a group, left right, lobby ghosted",
            &grouped_subnet(true),
            lr().with_ghosting(Ghosting::named(["Lobby"])),
        ),
    ];
    let net = four_groups(true);
    let mut layout = Layout::default().with_ghosting(Ghosting::named(["Hub"]));
    part_for(&net, &mut layout, &["A", "B"], |g| {
        matches!(g, Some("A" | "B"))
    });
    out.push(("a part for two groups", numbers(&layout, &net)));
    part_for(&net, &mut layout, &["A"], |_| true);
    out.push(("a part for one group", numbers(&layout, &net)));
    out
}

/// [`SOME_IN_NONE`], laid out now with declared groups, all open.
fn some_in_none() -> Vec<(&'static str, Numbers)> {
    let mut out = vec![
        whole("mixed", &mixed(true), Layout::default()),
        whole("mixed, left right, fitted", &mixed(true), fitted(lr())),
    ];
    let net = four_groups(true);
    let mut layout = lr().with_ghosting(Ghosting::named(["Hub", "Spare"]));
    part_for(&net, &mut layout, &["A", "B"], |_| true);
    out.push((
        "a part for two groups, with the rest",
        numbers(&layout, &net),
    ));
    out
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.01
}

#[test]
fn a_net_with_no_groups_lays_out_to_the_same_numbers_as_before_groups() {
    let now = no_groups();
    assert_eq!(now.len(), NO_GROUPS.len());
    for ((name, numbers), was) in now.iter().zip(NO_GROUPS) {
        assert_eq!(*name, was.name);
        assert_eq!(*numbers, was.numbers(), "{name}");
    }
}

#[test]
fn open_groups_lay_out_to_the_numbers_of_the_clusters_they_replace() {
    // Every node is in a group, so there is no block of nodes in none, and
    // the labels are not fitted, so the names' lengths matter only to the
    // frames, which the bare names give as the shortened ones did.
    let now = every_node_grouped();
    assert_eq!(now.len(), EVERY_NODE_GROUPED.len());
    for ((name, numbers), was) in now.iter().zip(EVERY_NODE_GROUPED) {
        assert_eq!(*name, was.name);
        assert_eq!(*numbers, was.numbers(), "{name}");
    }
}

#[test]
fn with_nodes_in_no_group_each_block_is_laid_out_as_its_cluster_was_and_the_block_is_packed_first()
{
    // The frames keep their sizes and every node its place in its frame;
    // what moves is where the frames go, since the nodes in no group are
    // now packed first, where the cluster of the names with no prefix was
    // packed at its first node. Those nodes keep their places among
    // themselves, and have no frame.
    let now = some_in_none();
    assert_eq!(now.len(), SOME_IN_NONE.len());
    for ((name, now), was) in now.iter().zip(SOME_IN_NONE) {
        assert_eq!(*name, was.name);
        assert_eq!(now.ranks, was.ranks, "{name}");
        assert_eq!(now.nodes.len(), was.nodes.len(), "{name}");
        let unnamed = was.frames.iter().position(|f| f.is_empty());
        // In the cluster of the names with no prefix, or, in a part, among
        // the placed nodes of no cluster.
        let in_block = |n: &[f32; 5]| match unnamed {
            Some(k) => n[4] == k as f32,
            None => n[4] < 0.0 && n[2] > -1e8,
        };
        // Each named frame: the same size, and its nodes at the same places
        // in it.
        for (k, frame) in was.frames.iter().enumerate() {
            if frame.is_empty() {
                continue;
            }
            let old = was.frame_bounds[k];
            let j = now
                .frames
                .iter()
                .position(|f| f == frame)
                .unwrap_or_else(|| panic!("{name}: {frame}"));
            let new = now.frame_bounds[j];
            assert!(close(old[2] - old[0], new[2] - new[0]), "{name}: {frame}");
            assert!(close(old[3] - old[1], new[3] - new[1]), "{name}: {frame}");
            for (o, n) in was.nodes.iter().zip(&now.nodes) {
                if o[4] != k as f32 {
                    continue;
                }
                assert_eq!((o[0], o[1]), (n[0], n[1]), "{name}: rank and order");
                assert_eq!(n[4], j as f32, "{name}: in {frame}");
                assert!(close(o[2] - old[0], n[2] - new[0]), "{name}: {o:?} {n:?}");
                assert!(close(o[3] - old[1], n[3] - new[1]), "{name}: {o:?} {n:?}");
            }
        }
        // The block: no frame, and the same places among its nodes.
        let block: Vec<usize> = (0..was.nodes.len())
            .filter(|&i| in_block(&was.nodes[i]))
            .collect();
        assert!(!block.is_empty(), "{name}");
        let (first_was, first_now) = (was.nodes[block[0]], now.nodes[block[0]]);
        for &i in &block {
            let (o, n) = (was.nodes[i], now.nodes[i]);
            assert_eq!(n[4], -1.0, "{name}: no frame");
            assert_eq!((o[0], o[1]), (n[0], n[1]), "{name}: rank and order");
            assert!(close(o[2] - first_was[2], n[2] - first_now[2]), "{name}");
            assert!(close(o[3] - first_was[3], n[3] - first_now[3]), "{name}");
        }
        // The nodes with no home stay without.
        for (o, n) in was.nodes.iter().zip(&now.nodes) {
            assert_eq!(o[2] < -1e8, n[2] < -1e8, "{name}");
        }
        // Packed first: every frame is to the right of the block or below it.
        let right = block
            .iter()
            .map(|&i| now.nodes[i][2])
            .fold(f32::MIN, f32::max);
        let bottom = block
            .iter()
            .map(|&i| now.nodes[i][3])
            .fold(f32::MAX, f32::min);
        for f in &now.frame_bounds {
            assert!(f[0] > right || f[3] < bottom, "{name}: {f:?}");
        }
        assert_eq!(
            now.frames,
            was.frames
                .iter()
                .filter(|f| !f.is_empty())
                .map(|f| (*f).to_owned())
                .collect::<Vec<_>>(),
            "{name}"
        );
    }
}

// ----- closed groups -------------------------------------------------------

fn pass(n: u32) -> (u32,) {
    (n,)
}

/// `Start -> Set Off -> [Walk: Trail -> Hike -> Summit -> Descend] ->
/// [Eat: Pantry -> Cook -> Fed] -> Finish -> End`: `Descend` is in `Walk`
/// and writes `Pantry`, in `Eat`; `Start`, `Set Off`, `Finish` and `End` are
/// in no group.
struct Camp {
    net: Net,
    start: PlaceId,
    trail: PlaceId,
    summit: PlaceId,
    pantry: PlaceId,
    fed: PlaceId,
    end: PlaceId,
    set_off: TransitionId,
    hike: TransitionId,
    descend: TransitionId,
    cook: TransitionId,
    finish: TransitionId,
}

fn camp() -> Camp {
    let mut b = NetBuilder::new();
    let walk = b.group("Walk");
    let eat = b.group("Eat");
    let start = b.place::<u32>("Start");
    let trail = b.place::<u32>("Trail");
    let summit = b.place::<u32>("Summit");
    let pantry = b.place::<u32>("Pantry");
    let fed = b.place::<u32>("Fed");
    let end = b.place::<u32>("End");
    b.in_group(walk, trail);
    b.in_group(walk, summit);
    b.in_group(eat, pantry);
    b.in_group(eat, fed);
    let set_off = b
        .transition("Set Off", (TakeEdge(start),), (trail,), pass)
        .id();
    let hike = b
        .transition("Hike", (TakeEdge(trail),), (summit,), pass)
        .group(walk)
        .id();
    let descend = b
        .transition("Descend", (TakeEdge(summit),), (pantry,), pass)
        .group(walk)
        .id();
    let cook = b
        .transition("Cook", (TakeEdge(pantry),), (fed,), pass)
        .group(eat)
        .id();
    let finish = b.transition("Finish", (TakeEdge(fed),), (end,), pass).id();
    Camp {
        net: b.build().expect("the camp builds"),
        start: start.id(),
        trail: trail.id(),
        summit: summit.id(),
        pantry: pantry.id(),
        fed: fed.id(),
        end: end.id(),
        set_off,
        hike,
        descend,
        cook,
        finish,
    }
}

fn laid_out(net: &Net, layout: Layout) -> Layout {
    let mut layout = layout;
    layout.compute(net);
    layout
}

fn place(id: PlaceId) -> LayoutNode {
    LayoutNode::Place(id)
}

fn transition(id: TransitionId) -> LayoutNode {
    LayoutNode::Transition(id)
}

fn group(name: &str) -> ArrowEnd {
    ArrowEnd::Group(name.to_owned())
}

/// Whether two boxes share more than an edge.
fn overlap(a: Bounds, b: Bounds) -> bool {
    a.min.x < b.max.x - 0.01
        && b.min.x < a.max.x - 0.01
        && a.min.y < b.max.y - 0.01
        && b.min.y < a.max.y - 0.01
}

fn contains(outer: Bounds, inner: Bounds) -> bool {
    outer.contains(inner.min) && outer.contains(inner.max)
}

/// Nothing drawn overlaps anything else: no two nodes, boxes of closed
/// groups or frames of open ones, and a frame holds exactly its own nodes.
/// Every node is either placed or hidden in a closed group.
fn assert_clear(layout: &Layout, net: &Net) {
    // Each node and box: its shape, and the open group it is drawn in.
    let mut shapes: Vec<(String, Bounds, Option<String>)> = Vec::new();
    for node in nodes_of(net) {
        let Some(placement) = layout.placement(node) else {
            let port =
                matches!(node, LayoutNode::Place(p) if layout.position_of_place(p).is_some());
            assert!(
                port || layout.hidden_in(node).is_some(),
                "{node:?} is neither placed nor hidden"
            );
            continue;
        };
        assert!(layout.hidden_in(node).is_none(), "{node:?}");
        let shape = Bounds::from_center_half_size(placement.position, layout.half_extent(node));
        let frame = layout
            .group_of(node)
            .filter(|g| layout.group_bounds(g).is_some());
        shapes.push((format!("{node:?}"), shape, frame.map(str::to_owned)));
    }
    let names: Vec<String> = layout.group_names().map(str::to_owned).collect();
    for name in &names {
        if let Some(drawn) = layout.group_box(name) {
            assert!(layout.group_bounds(name).is_none());
            let inner = drawn.bounds().inflate(-drawn.corner_radius);
            assert!(
                contains(inner, drawn.title) && contains(inner, drawn.count),
                "{name}"
            );
            assert!(!overlap(drawn.title, drawn.count), "{name}");
            shapes.push((format!("box {name}"), drawn.bounds(), None));
        }
    }
    for (i, (a, ra, _)) in shapes.iter().enumerate() {
        for (b, rb, _) in &shapes[i + 1..] {
            assert!(!overlap(*ra, *rb), "{a} overlaps {b}");
        }
    }
    let frames: Vec<(&String, Bounds)> = names
        .iter()
        .filter_map(|n| Some((n, layout.group_bounds(n)?)))
        .collect();
    for (i, (a, fa)) in frames.iter().enumerate() {
        for (b, fb) in &frames[i + 1..] {
            assert!(!overlap(*fa, *fb), "the frames of {a} and {b} overlap");
        }
        for (what, shape, frame) in &shapes {
            if frame.as_ref() == Some(*a) {
                assert!(contains(*fa, *shape), "{what} is outside {a}'s frame");
            } else {
                assert!(!overlap(*fa, *shape), "{what} overlaps {a}'s frame");
            }
        }
    }
}

#[test]
fn a_closed_group_is_one_node_that_ranks_between_its_neighbours() {
    let c = camp();
    let layout = laid_out(&c.net, Layout::default().with_closed(["Walk", "Eat"]));
    // Start, Set Off, Walk, Eat, Finish, End: one rank each, top down.
    assert_eq!(layout.rank_count(), 6);
    let y = |node| layout.position_of(node).unwrap().y;
    let walk = layout.group_box("Walk").expect("closed").centre;
    let eat = layout.group_box("Eat").expect("closed").centre;
    assert!(y(place(c.start)) > y(transition(c.set_off)));
    assert!(y(transition(c.set_off)) > walk.y);
    assert!(walk.y > eat.y);
    assert!(eat.y > y(transition(c.finish)));
    assert!(y(transition(c.finish)) > y(place(c.end)));
    assert_eq!(layout.placement(transition(c.finish)).unwrap().rank, 4);
    // The members are hidden and have no position of their own.
    for (node, name) in [
        (place(c.trail), "Walk"),
        (place(c.summit), "Walk"),
        (transition(c.hike), "Walk"),
        (transition(c.descend), "Walk"),
        (place(c.pantry), "Eat"),
        (place(c.fed), "Eat"),
        (transition(c.cook), "Eat"),
    ] {
        assert_eq!(layout.hidden_in(node), Some(name), "{node:?}");
        assert_eq!(layout.group_of(node), Some(name), "{node:?}");
        assert!(layout.placement(node).is_none() && layout.position_of(node).is_none());
    }
    assert_eq!(layout.radius_of_place(c.trail), None);
    assert_eq!(layout.position_of_token(c.trail, 0), None);
    assert_eq!(layout.hidden_in(place(c.start)), None);
    assert_eq!(layout.group_of(place(c.start)), None);
    assert!(layout.is_group_closed("Walk") && layout.is_group_closed("Eat"));
    assert!(layout.group_bounds("Walk").is_none());
    assert_clear(&layout, &c.net);
}

#[test]
fn with_every_group_closed_the_layout_is_one_drawing_of_boxes_and_the_rest() {
    let c = camp();
    for direction in [Direction::TopDown, Direction::LeftRight] {
        let layout = laid_out(
            &c.net,
            Layout::default()
                .with_direction(direction)
                .with_closed(["Walk", "Eat"]),
        );
        assert_eq!(layout.group_names().collect::<Vec<_>>(), ["Walk", "Eat"]);
        assert!(
            layout
                .group_names()
                .all(|g| layout.group_bounds(g).is_none())
        );
        // One drawing, centred on the origin as a net with no groups is.
        let centres: Vec<Point> = nodes_of(&c.net)
            .into_iter()
            .filter_map(|n| layout.position_of(n))
            .chain(["Walk", "Eat"].map(|g| layout.group_box(g).unwrap().centre))
            .collect();
        let lo = centres.iter().fold(Point::new(f32::MAX, f32::MAX), |m, p| {
            Point::new(m.x.min(p.x), m.y.min(p.y))
        });
        let hi = centres.iter().fold(Point::new(f32::MIN, f32::MIN), |m, p| {
            Point::new(m.x.max(p.x), m.y.max(p.y))
        });
        assert!(
            close(lo.x + hi.x, 0.0) && close(lo.y + hi.y, 0.0),
            "{direction:?}"
        );
        let bounds = layout.bounds().unwrap();
        for g in ["Walk", "Eat"] {
            assert!(contains(bounds, layout.group_box(g).unwrap().bounds()));
        }
        assert_clear(&layout, &c.net);
    }
}

#[test]
fn edges_of_a_closed_group_are_merged_arrows_and_those_inside_are_not_drawn() {
    let c = camp();
    let layout = laid_out(&c.net, Layout::default().with_closed(["Walk", "Eat"]));
    let arrows: Vec<_> = layout.merged_arrows().collect();
    assert_eq!(arrows.len(), 3, "{arrows:?}");
    // Into a box, between two boxes, and out of a box.
    assert_eq!(arrows[0].from, ArrowEnd::Node(transition(c.set_off)));
    assert_eq!(arrows[0].to, group("Walk"));
    assert_eq!(arrows[0].edges, [(transition(c.set_off), place(c.trail))]);
    assert_eq!(
        (&arrows[1].from, &arrows[1].to),
        (&group("Walk"), &group("Eat"))
    );
    assert_eq!(arrows[1].edges, [(transition(c.descend), place(c.pantry))]);
    assert_eq!(arrows[2].from, group("Eat"));
    assert_eq!(arrows[2].to, ArrowEnd::Node(transition(c.finish)));
    assert!(arrows.iter().all(|a| !a.back));
    // Each edge with a hidden end finds its arrow; one inside a box, or with
    // no hidden end, finds none.
    let arrow = layout
        .merged_arrow_for(transition(c.descend), place(c.pantry))
        .unwrap();
    assert_eq!(arrow, arrows[1]);
    assert!(
        layout
            .merged_arrow_for(place(c.trail), transition(c.hike))
            .is_none()
    );
    assert!(
        layout
            .merged_arrow_for(place(c.start), transition(c.set_off))
            .is_none()
    );
    assert!(layout.back_edges().next().is_none());
}

/// `Map -> Plan`, which writes both places of `Walk`; `Look` reads both
/// and writes `Photo`; `Hike`, in `Walk`, takes the trail to the summit.
fn plan_and_look() -> (Net, [PlaceId; 4], [TransitionId; 3]) {
    let mut b = NetBuilder::new();
    let walk = b.group("Walk");
    let map = b.place::<u32>("Map");
    let trail = b.place::<u32>("Trail");
    let summit = b.place::<u32>("Summit");
    let photo = b.place::<u32>("Photo");
    b.in_group(walk, trail);
    b.in_group(walk, summit);
    let plan = b
        .transition("Plan", (TakeEdge(map),), (trail, summit), |n: u32| (n, n))
        .id();
    let hike = b
        .transition("Hike", (TakeEdge(trail),), (summit,), pass)
        .group(walk)
        .id();
    let look = b
        .transition(
            "Look",
            (TakeEdge(trail), TakeEdge(summit)),
            (photo,),
            |a: u32, _: u32| (a,),
        )
        .id();
    (
        b.build().expect("the net builds"),
        [map.id(), trail.id(), summit.id(), photo.id()],
        [plan, hike, look],
    )
}

#[test]
fn several_edges_between_a_box_and_one_node_are_one_arrow_in_each_direction() {
    let (net, [map, trail, summit, photo], [plan, _, look]) = plan_and_look();
    let layout = laid_out(&net, Layout::default().with_closed(["Walk"]));
    let arrows: Vec<_> = layout.merged_arrows().collect();
    assert_eq!(arrows.len(), 2, "{arrows:?}");
    assert_eq!(arrows[0].from, ArrowEnd::Node(transition(plan)));
    assert_eq!(arrows[0].to, group("Walk"));
    assert_eq!(
        arrows[0].edges,
        [
            (transition(plan), place(trail)),
            (transition(plan), place(summit))
        ]
    );
    assert_eq!(arrows[1].from, group("Walk"));
    assert_eq!(arrows[1].to, ArrowEnd::Node(transition(look)));
    assert_eq!(
        arrows[1].edges,
        [
            (place(trail), transition(look)),
            (place(summit), transition(look))
        ]
    );
    // Map, Plan, Walk, Look, Photo: the box ranks as one node.
    let rank = |node| layout.placement(node).unwrap().rank;
    assert_eq!(
        [map, photo].map(|p| rank(place(p))),
        [0, 4],
        "the arrows rank the box"
    );
    assert_eq!(rank(transition(look)), 3);
    assert_clear(&layout, &net);
}

#[test]
fn a_loop_through_a_closed_group_has_one_back_arrow() {
    // `Rest` takes from the summit and puts back on the trail.
    let mut b = NetBuilder::new();
    let walk = b.group("Walk");
    let trail = b.place::<u32>("Trail");
    let summit = b.place::<u32>("Summit");
    b.in_group(walk, trail);
    b.in_group(walk, summit);
    b.transition("Hike", (TakeEdge(trail),), (summit,), pass)
        .group(walk);
    let rest = b
        .transition("Rest", (TakeEdge(summit),), (trail,), pass)
        .id();
    let net = b.build().expect("the net builds");
    let layout = laid_out(&net, Layout::default().with_closed(["Walk"]));
    let arrows: Vec<_> = layout.merged_arrows().collect();
    assert_eq!(arrows.len(), 2);
    assert_eq!(arrows.iter().filter(|a| a.back).count(), 1, "{arrows:?}");
    let back = arrows.iter().find(|a| a.back).unwrap();
    assert_eq!(back.from, ArrowEnd::Node(transition(rest)));
    assert_eq!(back.to, group("Walk"));
    assert_eq!(layout.rank_count(), 2);
    // A back arrow's curve is the one an edge would have, between the ends.
    let at = layout.position_of(transition(rest)).unwrap();
    let centre = layout.group_box("Walk").unwrap().centre;
    assert!(layout.bow(at, centre) > 0.0);
}

#[test]
fn a_closed_box_is_sized_for_its_name_and_its_count() {
    let mut b = NetBuilder::new();
    let short = b.group("Eat");
    let long = b.group("A group with a rather long name");
    let a = b.place::<u32>("A");
    let z = b.place::<u32>("Z");
    b.in_group(short, a);
    b.in_group(long, z);
    let net = b.build().expect("the net builds");
    for direction in [Direction::TopDown, Direction::LeftRight] {
        let layout = laid_out(
            &net,
            Layout::default()
                .with_direction(direction)
                .with_closed(["Eat", "A group with a rather long name"]),
        );
        let small = layout.group_box("Eat").unwrap();
        let wide = layout.group_box("A group with a rather long name").unwrap();
        assert!(wide.size.x > small.size.x, "{direction:?}");
        assert_eq!(wide.size.y, small.size.y, "{direction:?}");
        for drawn in [small, wide] {
            assert_eq!(drawn.corner_radius, 12.0);
            assert!(drawn.title.size().x < drawn.size.x - 2.0 * drawn.corner_radius + 0.01);
            assert!(
                drawn.count.max.y <= drawn.title.min.y + 0.01,
                "the count under the title"
            );
        }
        // The title has room for every character, as a subnet place's does.
        assert!(wide.title.size().x > small.title.size().x * 5.0);
        assert_clear(&layout, &net);
    }
}

#[test]
fn some_groups_open_some_closed_and_some_nodes_in_none() {
    let c = camp();
    for direction in [Direction::TopDown, Direction::LeftRight] {
        for fit in [false, true] {
            let mut layout = Layout::default()
                .with_direction(direction)
                .with_closed(["Walk"]);
            layout.fit_labels = fit;
            layout.compute(&c.net);
            assert_eq!(layout.group_names().collect::<Vec<_>>(), ["Walk", "Eat"]);
            assert!(layout.group_box("Walk").is_some() && layout.group_bounds("Walk").is_none());
            assert!(layout.group_bounds("Eat").is_some() && layout.group_box("Eat").is_none());
            assert_eq!(layout.group_of(place(c.pantry)), Some("Eat"));
            assert_eq!(layout.hidden_in(place(c.pantry)), None);
            // The arrow from the box to a node of an open group crosses its
            // frame: drawn straight, taking no part in the layout.
            let arrow = layout
                .merged_arrow_for(transition(c.descend), place(c.pantry))
                .unwrap();
            assert_eq!(arrow.from, group("Walk"));
            assert_eq!(arrow.to, ArrowEnd::Node(place(c.pantry)));
            assert!(!arrow.back);
            // Pantry is a source of its own frame.
            assert_eq!(layout.placement(place(c.pantry)).unwrap().rank, 0);
            assert_clear(&layout, &c.net);
        }
    }
}

#[test]
fn a_subnet_place_in_a_group_is_framed_when_open_and_hidden_with_its_ports_when_closed() {
    let net = grouped_subnet(true);
    let level = net.place_named("Level").unwrap();
    let lobby = net
        .places()
        .find(|(_, i)| i.port_of == Some(level))
        .unwrap()
        .0;
    let raid = net.transition_named("Raid").unwrap();
    let open = laid_out(&net, Layout::default());
    let frame = open.group_bounds("Def").unwrap();
    assert!(contains(frame, open.subnet_box(level).unwrap().bounds()));
    assert_eq!(open.group_of(place(lobby)), Some("Def"));
    assert!(open.merged_arrows().next().is_none());
    assert_clear(&open, &net);

    let closed = laid_out(&net, Layout::default().with_closed(["Def"]));
    assert_eq!(closed.hidden_in(place(level)), Some("Def"));
    assert_eq!(closed.hidden_in(place(lobby)), Some("Def"));
    assert_eq!(closed.group_of(place(lobby)), Some("Def"));
    assert!(closed.subnet_box(level).is_none());
    assert!(closed.position_of_place(lobby).is_none());
    assert_eq!(closed.radius_of_place(lobby), None);
    // The raid, in the open group `Atk`, writes the lobby: an arrow from its
    // frame to the box, named by the port.
    let arrow = closed
        .merged_arrow_for(transition(raid), place(lobby))
        .unwrap();
    assert_eq!(arrow.to, group("Def"));
    assert_eq!(arrow.edges, [(transition(raid), place(lobby))]);
    assert_clear(&closed, &net);
}

#[test]
fn a_port_faces_a_closed_box_it_is_joined_to() {
    // `[Gate: Door -> Enter] -> Level [Lobby | Exit Hall] -> Leave -> Out`.
    let mut b = NetBuilder::new();
    let gate = b.group("Gate");
    let door = b.place::<u32>("Door");
    b.in_group(gate, door);
    let level = b.subnet_place("Level", ports_only(&["Lobby", "Exit Hall"]));
    let lobby = b.port_of::<u32>(level, "Lobby");
    let exit = b.port_of::<u32>(level, "Exit Hall");
    let out = b.place::<u32>("Out");
    b.transition("Enter", (TakeEdge(door),), (lobby,), pass)
        .group(gate);
    b.transition("Leave", (TakeEdge(exit),), (out,), pass);
    let net = b.build().expect("the net builds");
    for direction in [Direction::TopDown, Direction::LeftRight] {
        let layout = laid_out(
            &net,
            Layout::default()
                .with_direction(direction)
                .with_closed(["Gate"]),
        );
        let main = |p: Point| match direction {
            Direction::TopDown => p.y,
            Direction::LeftRight => -p.x,
        };
        let drawn = layout.subnet_box(level.id()).unwrap();
        let gate = layout.group_box("Gate").unwrap();
        assert!(main(gate.centre) > main(drawn.centre), "{direction:?}");
        let at = |port: PlaceRef<u32>| main(layout.position_of_place(port.id()).unwrap());
        assert!(
            at(lobby) > main(drawn.centre),
            "{direction:?}: toward the gate"
        );
        assert!(at(exit) < main(drawn.centre), "{direction:?}");
        assert_clear(&layout, &net);
    }
}

/// `Energy`, in no group, is read by `Hike` and `Climb` in `Walk` and read
/// and written by `Cook` in `Eat`; `Water`, in `Walk`, is read by `Drink`,
/// in no group, which fills `Sated`.
fn hub() -> (Net, [PlaceId; 3], [TransitionId; 4]) {
    let mut b = NetBuilder::new();
    let walk = b.group("Walk");
    let eat = b.group("Eat");
    let energy = b.place::<u32>("Energy");
    let trail = b.place::<u32>("Trail");
    let summit = b.place::<u32>("Summit");
    let water = b.place::<u32>("Water");
    let pantry = b.place::<u32>("Pantry");
    let fed = b.place::<u32>("Fed");
    let sated = b.place::<u32>("Sated");
    for p in [trail, summit, water] {
        b.in_group(walk, p);
    }
    for p in [pantry, fed] {
        b.in_group(eat, p);
    }
    let two = |a: u32, _: u32| (a,);
    let hike = b
        .transition("Hike", (TakeEdge(trail), TakeEdge(energy)), (summit,), two)
        .group(walk)
        .id();
    let climb = b
        .transition("Climb", (TakeEdge(summit), TakeEdge(energy)), (trail,), two)
        .group(walk)
        .id();
    let cook = b
        .transition(
            "Cook",
            (TakeEdge(pantry), TakeEdge(energy)),
            (fed, energy),
            |a: u32, e: u32| (a, e),
        )
        .group(eat)
        .id();
    let drink = b
        .transition("Drink", (TakeEdge(water),), (sated,), pass)
        .id();
    (
        b.build().expect("the net builds"),
        [energy.id(), water.id(), sated.id()],
        [hike, climb, cook, drink],
    )
}

#[test]
fn a_ghosted_place_has_one_ghost_beside_a_closed_box_on_each_side_it_needs() {
    let (net, [energy, water, _], [hike, climb, cook, drink]) = hub();
    let ghosting = Ghosting::named(["Energy", "Water"]);
    // Open, a ghost beside each transition, as ever.
    let open = laid_out(&net, Layout::default().with_ghosting(ghosting.clone()));
    assert_eq!(open.ghosts().count(), 5);
    assert_eq!(open.group_ghosts().count(), 0);
    assert!(
        open.ghost_position(water, drink, GhostSide::Input)
            .is_some()
    );

    for direction in [Direction::TopDown, Direction::LeftRight] {
        let layout = laid_out(
            &net,
            Layout::default()
                .with_direction(direction)
                .with_ghosting(ghosting.clone())
                .with_closed(["Walk", "Eat"]),
        );
        // One ghost beside `Walk` for both its readers, and one on each side
        // of `Eat`; none beside a hidden transition.
        assert_eq!(
            layout.group_ghosts().collect::<Vec<_>>(),
            [
                (energy, "Walk", GhostSide::Input),
                (energy, "Eat", GhostSide::Input),
                (energy, "Eat", GhostSide::Output),
            ]
        );
        assert_eq!(layout.ghosts().count(), 0);
        for t in [hike, climb, cook] {
            assert!(layout.ghost_position(energy, t, GhostSide::Input).is_none());
        }
        // The ghost is beside the box, on the side the ranks come from for
        // an input and the other side for an output.
        let main = |p: Point| match direction {
            Direction::TopDown => p.y,
            Direction::LeftRight => -p.x,
        };
        let half = |s: Point| match direction {
            Direction::TopDown => s.y / 2.0,
            Direction::LeftRight => s.x / 2.0,
        };
        for (g, side) in [
            ("Walk", GhostSide::Input),
            ("Eat", GhostSide::Input),
            ("Eat", GhostSide::Output),
        ] {
            let drawn = layout.group_box(g).unwrap();
            let at = layout.group_ghost_position(energy, g, side).unwrap();
            let off = main(at) - main(drawn.centre);
            match side {
                GhostSide::Input => assert!(off > half(drawn.size), "{direction:?} {g}"),
                GhostSide::Output => assert!(-off > half(drawn.size), "{direction:?} {g}"),
            }
            assert!(layout.bounds().unwrap().contains(at));
        }
        assert!(
            layout
                .group_ghost_position(energy, "Walk", GhostSide::Output)
                .is_none()
        );
        // The hub keeps its home, and its arrows to the boxes are drawn from
        // the ghosts: they take no part in the layout.
        assert!(layout.position_of_place(energy).is_some());
        let to_walk = layout
            .merged_arrow_for(place(energy), transition(hike))
            .unwrap();
        assert_eq!(to_walk.from, ArrowEnd::Node(place(energy)));
        assert_eq!(to_walk.to, group("Walk"));
        assert_eq!(
            to_walk.edges,
            [
                (place(energy), transition(hike)),
                (place(energy), transition(climb))
            ]
        );
        assert!(!to_walk.back);
        assert_eq!(layout.placement(place(energy)).unwrap().rank, 0);
        // A ghosted place hidden in a closed group has no home and no
        // ghosts; its edge out of the group is the box's arrow.
        assert!(layout.is_ghosted(water));
        assert_eq!(layout.hidden_in(place(water)), Some("Walk"));
        assert!(layout.position_of_place(water).is_none());
        assert!(
            layout
                .ghost_position(water, drink, GhostSide::Input)
                .is_none()
        );
        let out = layout
            .merged_arrow_for(place(water), transition(drink))
            .unwrap();
        assert_eq!(out.from, group("Walk"));
        assert!(!out.back);
        // Drink reads nothing else: a source of the block, as it would be
        // with the water's edge ghosted and the group open.
        assert_eq!(layout.placement(transition(drink)).unwrap().rank, 0);
        assert_clear(&layout, &net);
    }
}

#[test]
fn a_part_keeps_its_focus_open_and_closes_the_rest_as_told() {
    let c = camp();
    let groups: Vec<&str> = c.net.groups().collect();
    let mut layout = Layout::default().with_closed(["Walk", "Eat"]);
    let run = |layout: &mut Layout, focus: Option<&[&str]>| {
        layout.compute_subgraph(
            c.net.places().map(LayoutPlace::from),
            c.net.transitions().map(LayoutTransition::from),
            c.net.edges(),
            c.net.edges(),
            &groups,
            focus,
        );
    };
    // A focus of one: a plain drawing of it, never closed, while the other
    // group stays closed.
    run(&mut layout, Some(&["Walk"]));
    assert_eq!(layout.group_names().collect::<Vec<_>>(), ["Eat"]);
    assert!(layout.is_group_closed("Eat"));
    assert_eq!(layout.group_of(place(c.trail)), None, "no frame");
    assert!(layout.position_of_place(c.trail).is_some());
    assert_eq!(layout.hidden_in(place(c.pantry)), Some("Eat"));
    assert_clear(&layout, &c.net);
    // A focus of two keeps both frames, closed or not.
    run(&mut layout, Some(&["Walk", "Eat"]));
    assert_eq!(layout.group_names().collect::<Vec<_>>(), ["Walk", "Eat"]);
    assert!(layout.group_bounds("Walk").is_some() && layout.group_bounds("Eat").is_some());
    assert_eq!(layout.group_of(place(c.start)), None);
    assert!(layout.merged_arrows().next().is_none());
    assert_clear(&layout, &c.net);
    // No focus: the closed groups are boxes, and an open one has no frame.
    layout.closed = ["Eat".to_owned()].into();
    run(&mut layout, None);
    assert_eq!(layout.group_names().collect::<Vec<_>>(), ["Eat"]);
    assert_eq!(layout.group_of(place(c.trail)), None);
    assert!(layout.position_of_place(c.trail).is_some());
    assert_clear(&layout, &c.net);
}

#[test]
fn an_empty_group_has_no_frame_and_a_group_left_out_of_the_order_comes_last() {
    let mut b = NetBuilder::new();
    let _empty = b.group("Empty");
    let late = b.group("Late");
    let early = b.group("Early");
    let x = b.place::<u32>("X");
    let y = b.place::<u32>("Y");
    b.in_group(early, x);
    b.in_group(late, y);
    let net = b.build().expect("the net builds");
    let layout = laid_out(&net, Layout::default().with_closed(["Empty"]));
    // Declared order, not the order of first use; the empty group is not
    // drawn, open or closed.
    assert_eq!(layout.group_names().collect::<Vec<_>>(), ["Late", "Early"]);
    assert!(layout.group_bounds("Empty").is_none() && layout.group_box("Empty").is_none());
    assert!(!layout.is_group_closed("Empty"));
    // By name, with an order that leaves `Early` out: it comes after the
    // listed ones.
    let mut named = Layout::default();
    named.compute_named(
        net.places().map(LayoutPlace::from),
        Vec::<LayoutTransition>::new(),
        net.edges(),
        &["Late"],
    );
    assert_eq!(named.group_names().collect::<Vec<_>>(), ["Late", "Early"]);
    // With no order, the order of the first nodes.
    named.compute_named(
        net.places().map(LayoutPlace::from),
        Vec::<LayoutTransition>::new(),
        net.edges(),
        &[],
    );
    assert_eq!(named.group_names().collect::<Vec<_>>(), ["Early", "Late"]);
}

#[test]
fn a_closed_name_no_node_uses_changes_nothing() {
    let c = camp();
    let plain = laid_out(&c.net, Layout::default());
    let mut told = laid_out(&c.net, Layout::default().with_closed(["Nowhere"]));
    told.closed.clear();
    assert_eq!(told, plain);
}

#[test]
fn the_same_net_with_closed_groups_lays_out_the_same_way_every_time() {
    let (net, _, _) = hub();
    let layout = || {
        Layout::default()
            .with_ghosting(Ghosting::named(["Energy", "Water"]))
            .with_closed(["Walk"])
    };
    let first = laid_out(&net, layout());
    for _ in 0..8 {
        assert_eq!(laid_out(&hub().0, layout()), first);
    }
}

#[test]
fn no_node_box_or_frame_overlaps_in_a_net_of_every_kind() {
    // Two open groups, one holding a subnet place, two closed ones, a hub in
    // no group read in three of them, and an arrow between two boxes.
    let mut b = NetBuilder::new();
    let [a, bb, c, d] = ["A", "B", "C", "D"].map(|g| b.group(g));
    let hub = b.place::<u32>("Hub");
    let start = b.place::<u32>("Start");
    let a1 = b.place::<u32>("A one");
    let level = b.subnet_place("Level", ports_only(&["Lobby", "Exit Hall"]));
    let lobby = b.port_of::<u32>(level, "Lobby");
    let exit = b.port_of::<u32>(level, "Exit Hall");
    let b1 = b.place::<u32>("B one");
    let b2 = b.place::<u32>("B two");
    let c1 = b.place::<u32>("C one");
    let d1 = b.place::<u32>("D one");
    let end = b.place::<u32>("End");
    b.in_group(a, a1);
    b.in_group(a, level);
    b.in_group(bb, b1);
    b.in_group(bb, b2);
    b.in_group(c, c1);
    b.in_group(d, d1);
    let two = |x: u32, _: u32| (x,);
    b.transition("Go", (TakeEdge(start),), (a1,), pass);
    b.transition("Enter", (TakeEdge(a1),), (lobby,), pass)
        .group(a);
    b.transition("Leave", (TakeEdge(exit), TakeEdge(hub)), (b1,), two)
        .group(a);
    b.transition("Move", (TakeEdge(b1), TakeEdge(hub)), (b2,), two)
        .group(bb);
    b.transition("Pass", (TakeEdge(b2),), (d1, c1), |x: u32| (x, x))
        .group(bb);
    b.transition("Use", (TakeEdge(c1), TakeEdge(hub)), (end,), two)
        .group(c);
    b.transition("Store", (TakeEdge(d1),), (end,), pass)
        .group(d);
    let net = b.build().expect("the net builds");
    let pass_on = transition(net.transition_named("Pass").unwrap());
    let sets: [&[&str]; 4] = [&[], &["B", "D"], &["A", "B", "C", "D"], &["A", "C"]];
    for direction in [Direction::TopDown, Direction::LeftRight] {
        for fit in [false, true] {
            for closed in sets {
                let mut layout = Layout::default()
                    .with_direction(direction)
                    .with_ghosting(Ghosting::named(["Hub"]))
                    .with_closed(closed.iter().copied());
                layout.fit_labels = fit;
                layout.compute(&net);
                assert_clear(&layout, &net);
                if closed.contains(&"B") && closed.contains(&"D") {
                    let arrow = layout.merged_arrow_for(pass_on, place(d1.id())).unwrap();
                    assert_eq!((&arrow.from, &arrow.to), (&group("B"), &group("D")));
                }
            }
        }
    }
}

// ----- the numbers at eb323d3 ---------------------------------------------

/// The nets with no groups, laid out at eb323d3.
const NO_GROUPS: &[Pinned] = &[
    Pinned {
        name: "plain",
        nodes: &[
            [0.0, 0.0, -90.0, 300.0, -1.0],
            [2.0, 0.0, 0.0, 0.0, -1.0],
            [4.0, 0.0, 0.0, -300.0, -1.0],
            [0.0, 1.0, 90.0, 300.0, -1.0],
            [1.0, 0.0, 0.0, 150.0, -1.0],
            [3.0, 0.0, 0.0, -150.0, -1.0],
        ],
        ghosts: &[],
        frames: &[],
        frame_bounds: &[],
        boxes: &[],
        bounds: &[-130.0, -340.0, 130.0, 340.0],
        ranks: 5,
        back: &[],
    },
    Pinned {
        name: "ghosted",
        nodes: &[
            [0.0, 0.0, -480.0, 150.0, -1.0],
            [0.0, 1.0, -300.0, 150.0, -1.0],
            [0.0, 2.0, -120.0, 150.0, -1.0],
            [0.0, 3.0, 60.0, 150.0, -1.0],
            [0.0, 5.0, 480.0, 150.0, -1.0],
            [2.0, 0.0, 0.0, -150.0, -1.0],
            [1.0, 0.0, -90.0, 0.0, -1.0],
            [1.0, 1.0, 90.0, 0.0, -1.0],
            [0.0, 4.0, 270.0, 150.0, -1.0],
        ],
        ghosts: &[
            [0.0, 0.0, 0.0, -90.0, 63.0],
            [0.0, 1.0, 0.0, 90.0, 63.0],
            [0.0, 2.0, 0.0, 190.0, 213.0],
            [1.0, 2.0, 0.0, 270.0, 213.0],
            [2.0, 2.0, 0.0, 350.0, 213.0],
        ],
        frames: &[],
        frame_bounds: &[],
        boxes: &[],
        bounds: &[-520.0, -190.0, 520.0, 227.0],
        ranks: 3,
        back: &[],
    },
    Pinned {
        name: "left right, fitted",
        nodes: &[
            [0.0, 2.0, -150.0, -0.0, -1.0],
            [0.0, 0.0, -150.0, 360.0, -1.0],
            [0.0, 1.0, -150.0, 180.0, -1.0],
            [0.0, 3.0, -150.0, -180.0, -1.0],
            [0.0, 4.0, -150.0, -360.0, -1.0],
            [2.0, 0.0, 150.0, -0.0, -1.0],
            [1.0, 1.0, 0.0, -0.0, -1.0],
            [1.0, 2.0, 0.0, -180.0, -1.0],
            [1.0, 0.0, 0.0, 180.0, -1.0],
        ],
        ghosts: &[],
        frames: &[],
        frame_bounds: &[],
        boxes: &[],
        bounds: &[-190.0, -400.0, 190.0, 400.0],
        ranks: 3,
        back: &[],
    },
    Pinned {
        name: "a loop",
        nodes: &[
            [0.0, 1.0, 90.0, 225.0, -1.0],
            [2.0, 0.0, 0.0, -75.0, -1.0],
            [0.0, 0.0, -90.0, 225.0, -1.0],
            [1.0, 0.0, 0.0, 75.0, -1.0],
            [3.0, 0.0, 0.0, -225.0, -1.0],
        ],
        ghosts: &[],
        frames: &[],
        frame_bounds: &[],
        boxes: &[],
        bounds: &[-130.0, -250.0, 130.0, 265.0],
        ranks: 4,
        back: &[[1001.0, 0.0]],
    },
    Pinned {
        name: "a loop, left right",
        nodes: &[
            [0.0, 1.0, -225.0, -90.0, -1.0],
            [2.0, 0.0, 75.0, -0.0, -1.0],
            [0.0, 0.0, -225.0, 90.0, -1.0],
            [1.0, 0.0, -75.0, -0.0, -1.0],
            [3.0, 0.0, 225.0, -0.0, -1.0],
        ],
        ghosts: &[],
        frames: &[],
        frame_bounds: &[],
        boxes: &[],
        bounds: &[-265.0, -130.0, 285.0, 130.0],
        ranks: 4,
        back: &[[1001.0, 0.0]],
    },
    Pinned {
        name: "hubs by degree, fitted",
        nodes: &[
            [0.0, 0.0, -450.0, 75.0, -1.0],
            [0.0, 1.0, -270.0, 75.0, -1.0],
            [0.0, 2.0, -90.0, 75.0, -1.0],
            [0.0, 3.0, 90.0, 75.0, -1.0],
            [0.0, 4.0, 270.0, 75.0, -1.0],
            [0.0, 5.0, 450.0, 75.0, -1.0],
            [1.0, 1.0, 0.0, -75.0, -1.0],
            [1.0, 2.0, 180.0, -75.0, -1.0],
            [1.0, 0.0, -180.0, -75.0, -1.0],
        ],
        ghosts: &[
            [0.0, 0.0, 0.0, 0.0, -12.0],
            [5.0, 0.0, 1.0, 0.0, -138.0],
            [0.0, 1.0, 0.0, 180.0, -12.0],
            [5.0, 1.0, 1.0, 180.0, -138.0],
            [0.0, 2.0, 0.0, -180.0, -12.0],
            [5.0, 2.0, 1.0, -180.0, -138.0],
        ],
        frames: &[],
        frame_bounds: &[],
        boxes: &[],
        bounds: &[-490.0, -152.0, 490.0, 115.0],
        ranks: 2,
        back: &[],
    },
    Pinned {
        name: "world",
        nodes: &[
            [0.0, 0.0, 0.0, 325.0, -1.0],
            [2.0, 0.0, 0.0, 0.0, -1.0],
            [-1.0, -1.0, -14.0, 39.0, -1.0],
            [-1.0, -1.0, -14.0, -39.0, -1.0],
            [4.0, 0.0, 0.0, -325.0, -1.0],
            [1.0, 0.0, 0.0, 175.0, -1.0],
            [3.0, 0.0, 0.0, -175.0, -1.0],
        ],
        ghosts: &[],
        frames: &[],
        frame_bounds: &[],
        boxes: &[
            1.0, 0.0, 0.0, 94.0, 130.0, 12.0, -31.5, -1.0, 31.5, 19.0, -35.0, -19.0, 35.0, -1.0,
            2.0, -14.0, 39.0, 14.0, 4.0, 25.0, 28.0, 53.0, 3.0, -14.0, -39.0, 14.0, 4.0, -53.0,
            28.0, -25.0,
        ],
        bounds: &[-60.0, -365.0, 60.0, 365.0],
        ranks: 5,
        back: &[],
    },
    Pinned {
        name: "world, left right, fitted, lobby ghosted",
        nodes: &[
            [0.0, 0.0, -186.0, 92.0, -1.0],
            [0.0, 1.0, -186.0, -92.0, -1.0],
            [-1.0, -1.0, -134.0, -110.0, -1.0],
            [-1.0, -1.0, -134.0, -74.0, -1.0],
            [2.0, 0.0, 186.0, -0.0, -1.0],
            [1.0, 0.0, 2.0, 90.0, -1.0],
            [1.0, 1.0, 2.0, -90.0, -1.0],
        ],
        ghosts: &[[2.0, 0.0, 1.0, 100.0, 90.0]],
        frames: &[],
        frame_bounds: &[],
        boxes: &[
            1.0, -186.0, -92.0, 156.0, 88.0, 12.0, -248.5, -93.0, -185.5, -73.0, -252.0, -111.0,
            -182.0, -93.0, 2.0, -134.0, -110.0, 14.0, -176.0, -124.0, -152.0, -96.0, 3.0, -134.0,
            -74.0, 14.0, -176.0, -88.0, -152.0, -60.0,
        ],
        bounds: &[-264.0, -136.0, 226.0, 132.0],
        ranks: 3,
        back: &[],
    },
    Pinned {
        name: "crossing",
        nodes: &[
            [0.0, 0.0, -180.0, 154.0, -1.0],
            [0.0, 1.0, 0.0, 154.0, -1.0],
            [0.0, 2.0, 180.0, 154.0, -1.0],
            [2.0, 0.0, 0.0, -154.0, -1.0],
            [-1.0, -1.0, 18.0, -132.0, -1.0],
            [-1.0, -1.0, -46.0, -132.0, -1.0],
            [-1.0, -1.0, -110.0, -132.0, -1.0],
            [-1.0, -1.0, 82.0, -132.0, -1.0],
            [1.0, 0.0, -180.0, 4.0, -1.0],
            [1.0, 1.0, 0.0, 4.0, -1.0],
            [1.0, 2.0, 180.0, 4.0, -1.0],
        ],
        ghosts: &[],
        frames: &[],
        frame_bounds: &[],
        boxes: &[
            3.0, 0.0, -154.0, 272.0, 96.0, 12.0, -13.5, -172.0, 13.5, -152.0, -35.0, -190.0, 35.0,
            -172.0, 4.0, 18.0, -132.0, 14.0, 36.0, -146.0, 60.0, -118.0, 5.0, -46.0, -132.0, 14.0,
            -28.0, -146.0, -4.0, -118.0, 6.0, -110.0, -132.0, 14.0, -92.0, -146.0, -68.0, -118.0,
            7.0, 82.0, -132.0, 14.0, 100.0, -146.0, 124.0, -118.0,
        ],
        bounds: &[-240.0, -202.0, 240.0, 194.0],
        ranks: 3,
        back: &[],
    },
    Pinned {
        name: "crossing, left right",
        nodes: &[
            [0.0, 0.0, -169.0, 180.0, -1.0],
            [0.0, 1.0, -169.0, -0.0, -1.0],
            [0.0, 2.0, -169.0, -180.0, -1.0],
            [2.0, 0.0, 169.0, -0.0, -1.0],
            [-1.0, -1.0, 117.0, -18.0, -1.0],
            [-1.0, -1.0, 117.0, 18.0, -1.0],
            [-1.0, -1.0, 117.0, 54.0, -1.0],
            [-1.0, -1.0, 117.0, -54.0, -1.0],
            [1.0, 0.0, -19.0, 180.0, -1.0],
            [1.0, 1.0, -19.0, -0.0, -1.0],
            [1.0, 2.0, -19.0, -180.0, -1.0],
        ],
        ghosts: &[],
        frames: &[],
        frame_bounds: &[],
        boxes: &[
            3.0, 169.0, -0.0, 156.0, 160.0, 12.0, 186.5, -1.0, 213.5, 19.0, 165.0, -19.0, 235.0,
            -1.0, 4.0, 117.0, -18.0, 14.0, 135.0, -32.0, 159.0, -4.0, 5.0, 117.0, 18.0, 14.0,
            135.0, 4.0, 159.0, 32.0, 6.0, 117.0, 54.0, 14.0, 135.0, 40.0, 159.0, 68.0, 7.0, 117.0,
            -54.0, 14.0, 135.0, -68.0, 159.0, -40.0,
        ],
        bounds: &[-209.0, -220.0, 247.0, 220.0],
        ranks: 3,
        back: &[],
    },
    Pinned {
        name: "many, fitted",
        nodes: &[
            [2.0, 0.0, 0.0, 0.0, -1.0],
            [-1.0, -1.0, -78.0, 39.0, -1.0],
            [-1.0, -1.0, -14.0, -39.0, -1.0],
            [-1.0, -1.0, -14.0, 39.0, -1.0],
            [-1.0, -1.0, 50.0, 39.0, -1.0],
            [0.0, 0.0, -90.0, 325.0, -1.0],
            [4.0, 0.0, -90.0, -325.0, -1.0],
            [0.0, 1.0, 90.0, 325.0, -1.0],
            [4.0, 1.0, 90.0, -325.0, -1.0],
            [1.0, 0.0, -90.0, 175.0, -1.0],
            [3.0, 0.0, -90.0, -175.0, -1.0],
            [1.0, 1.0, 90.0, 175.0, -1.0],
            [3.0, 1.0, 90.0, -175.0, -1.0],
        ],
        ghosts: &[],
        frames: &[],
        frame_bounds: &[],
        boxes: &[
            0.0, 0.0, 0.0, 294.0, 130.0, 12.0, -135.0, -1.0, 135.0, 19.0, -35.0, -19.0, 35.0, -1.0,
            1.0, -78.0, 39.0, 14.0, -60.0, 25.0, -36.0, 53.0, 2.0, -14.0, -39.0, 14.0, 4.0, -53.0,
            28.0, -25.0, 3.0, -14.0, 39.0, 14.0, 4.0, 25.0, 28.0, 53.0, 4.0, 50.0, 39.0, 14.0,
            68.0, 25.0, 92.0, 53.0,
        ],
        bounds: &[-150.0, -365.0, 150.0, 365.0],
        ranks: 5,
        back: &[],
    },
    Pinned {
        name: "many, left right",
        nodes: &[
            [2.0, 0.0, 0.0, -0.0, -1.0],
            [-1.0, -1.0, -183.0, 36.0, -1.0],
            [-1.0, -1.0, 183.0, 0.0, -1.0],
            [-1.0, -1.0, -183.0, 0.0, -1.0],
            [-1.0, -1.0, -183.0, -36.0, -1.0],
            [0.0, 0.0, -469.0, 90.0, -1.0],
            [4.0, 0.0, 469.0, 90.0, -1.0],
            [0.0, 1.0, -469.0, -90.0, -1.0],
            [4.0, 1.0, 469.0, -90.0, -1.0],
            [1.0, 0.0, -319.0, 90.0, -1.0],
            [3.0, 0.0, 319.0, 90.0, -1.0],
            [1.0, 1.0, -319.0, -90.0, -1.0],
            [3.0, 1.0, 319.0, -90.0, -1.0],
        ],
        ghosts: &[],
        frames: &[],
        frame_bounds: &[],
        boxes: &[
            0.0, 0.0, -0.0, 418.0, 124.0, 12.0, -135.0, -1.0, 135.0, 19.0, -35.0, -19.0, 35.0,
            -1.0, 1.0, -183.0, 36.0, 14.0, -165.0, 22.0, -141.0, 50.0, 2.0, 183.0, 0.0, 14.0,
            141.0, -14.0, 165.0, 14.0, 3.0, -183.0, 0.0, 14.0, -165.0, -14.0, -141.0, 14.0, 4.0,
            -183.0, -36.0, 14.0, -165.0, -50.0, -141.0, -22.0,
        ],
        bounds: &[-509.0, -130.0, 509.0, 130.0],
        ranks: 5,
        back: &[],
    },
    Pinned {
        name: "a part of the hubs",
        nodes: &[
            [0.0, 0.0, -360.0, 75.0, -1.0],
            [0.0, 1.0, -180.0, 75.0, -1.0],
            [0.0, 2.0, 0.0, 75.0, -1.0],
            [0.0, 3.0, 180.0, 75.0, -1.0],
            [-1.0, -1.0, -1e9, -1e9, -1.0],
            [0.0, 4.0, 360.0, 75.0, -1.0],
            [1.0, 1.0, 90.0, -75.0, -1.0],
            [-1.0, -1.0, -1e9, -1e9, -1.0],
            [1.0, 0.0, -90.0, -75.0, -1.0],
        ],
        ghosts: &[
            [0.0, 0.0, 0.0, 90.0, -12.0],
            [5.0, 0.0, 1.0, 90.0, -138.0],
            [0.0, 2.0, 0.0, -90.0, -12.0],
            [5.0, 2.0, 1.0, -90.0, -138.0],
        ],
        frames: &[],
        frame_bounds: &[],
        boxes: &[],
        bounds: &[-400.0, -152.0, 400.0, 115.0],
        ranks: 2,
        back: &[],
    },
    Pinned {
        name: "a part of the world",
        nodes: &[
            [0.0, 0.0, -90.0, 154.0, -1.0],
            [2.0, 0.0, 0.0, -154.0, -1.0],
            [-1.0, -1.0, -46.0, -132.0, -1.0],
            [-1.0, -1.0, 18.0, -132.0, -1.0],
            [0.0, 1.0, 90.0, 154.0, -1.0],
            [1.0, 0.0, 0.0, 4.0, -1.0],
            [-1.0, -1.0, -1000000000.0, -1000000000.0, -1.0],
        ],
        ghosts: &[],
        frames: &[],
        frame_bounds: &[],
        boxes: &[
            1.0, 0.0, -154.0, 144.0, 96.0, 12.0, -31.5, -172.0, 31.5, -152.0, -35.0, -190.0, 35.0,
            -172.0, 2.0, -46.0, -132.0, 14.0, -28.0, -146.0, -4.0, -118.0, 3.0, 18.0, -132.0, 14.0,
            36.0, -146.0, 60.0, -118.0,
        ],
        bounds: &[-130.0, -202.0, 130.0, 194.0],
        ranks: 3,
        back: &[],
    },
];

/// The nets whose every node has a group, clustered by name prefix with
/// labels not fitted, laid out at eb323d3 under the names `Group/Name`.
const EVERY_NODE_GROUPED: &[Pinned] = &[
    Pinned {
        name: "two groups",
        nodes: &[
            [0.0, 0.0, -214.0, 137.0, 0.0],
            [2.0, 0.0, -214.0, -163.0, 0.0],
            [1.0, 0.0, 184.0, -13.0, 1.0],
            [0.0, 0.0, 94.0, 137.0, 1.0],
            [1.0, 0.0, -214.0, -13.0, 0.0],
            [0.0, 1.0, 274.0, 137.0, 1.0],
        ],
        ghosts: &[],
        frames: &["A", "B"],
        frame_bounds: &[
            [-358.0, -227.0, -130.0, 227.0],
            [-50.0, -77.0, 358.0, 227.0],
        ],
        boxes: &[],
        bounds: &[-274.0, -203.0, 334.0, 177.0],
        ranks: 3,
        back: &[],
    },
    Pinned {
        name: "two groups, left right",
        nodes: &[
            [0.0, 0.0, -339.0, 57.5, 0.0],
            [2.0, 0.0, -39.0, 57.5, 0.0],
            [1.0, 0.0, 339.0, -32.5, 1.0],
            [0.0, 0.0, 189.0, 57.5, 1.0],
            [1.0, 0.0, -189.0, 57.5, 0.0],
            [0.0, 1.0, 189.0, -122.5, 1.0],
        ],
        ghosts: &[],
        frames: &["A", "B"],
        frame_bounds: &[[-403.0, -30.5, 25.0, 171.5], [105.0, -171.5, 403.0, 171.5]],
        boxes: &[],
        bounds: &[-379.0, -147.5, 379.0, 97.5],
        ranks: 3,
        back: &[],
    },
    Pinned {
        name: "four groups, ghosted",
        nodes: &[
            [0.0, 0.0, -124.0, 254.0, 0.0],
            [0.0, 0.0, 204.0, 254.0, 1.0],
            [0.0, 0.0, -124.0, -280.0, 2.0],
            [0.0, 0.0, 164.0, -280.0, 3.0],
            [2.0, 0.0, -124.0, -46.0, 0.0],
            [1.0, 0.0, -124.0, 104.0, 0.0],
        ],
        ghosts: &[
            [1.0, 0.0, 0.0, -164.0, 167.0],
            [2.0, 0.0, 0.0, -84.0, 167.0],
        ],
        frames: &["A", "B", "C", "D"],
        frame_bounds: &[
            [-268.0, -110.0, -20.0, 344.0],
            [60.0, 190.0, 268.0, 344.0],
            [-268.0, -344.0, -60.0, -190.0],
            [20.0, -344.0, 228.0, -190.0],
        ],
        boxes: &[],
        bounds: &[-184.0, -320.0, 244.0, 294.0],
        ranks: 3,
        back: &[],
    },
    Pinned {
        name: "a loop in a group, left right",
        nodes: &[
            [0.0, 0.0, -339.0, 13.5, 0.0],
            [2.0, 0.0, -39.0, 13.5, 0.0],
            [0.0, 0.0, 339.0, 13.5, 1.0],
            [1.0, 0.0, -189.0, 13.5, 0.0],
            [3.0, 0.0, 111.0, 13.5, 0.0],
        ],
        ghosts: &[],
        frames: &["L", "M"],
        frame_bounds: &[[-403.0, -127.5, 195.0, 127.5], [275.0, -74.5, 403.0, 127.5]],
        boxes: &[],
        bounds: &[-379.0, -26.5, 379.0, 53.5],
        ranks: 4,
        back: &[[1001.0, 0.0]],
    },
    Pinned {
        name: "a subnet in a group",
        nodes: &[
            [0.0, 0.0, -124.0, 145.0, 0.0],
            [2.0, 0.0, -124.0, -163.0, 0.0],
            [-1.0, -1.0, -138.0, -141.0, 0.0],
            [0.0, 0.0, 184.0, 145.0, 1.0],
            [1.0, 0.0, -124.0, -5.0, 0.0],
            [1.0, 0.0, 184.0, -5.0, 1.0],
        ],
        ghosts: &[],
        frames: &["Def", "Atk"],
        frame_bounds: &[[-268.0, -235.0, -40.0, 235.0], [40.0, -54.0, 268.0, 235.0]],
        boxes: &[
            1.0, -124.0, -163.0, 94.0, 96.0, 12.0, -146.5, -181.0, -101.5, -161.0, -159.0, -199.0,
            -89.0, -181.0, 2.0, -138.0, -141.0, 14.0, -120.0, -155.0, -96.0, -127.0,
        ],
        bounds: &[-184.0, -211.0, 244.0, 185.0],
        ranks: 3,
        back: &[],
    },
    Pinned {
        name: "a subnet in a group, left right, lobby ghosted",
        nodes: &[
            [0.0, 0.0, -352.0, 60.5, 0.0],
            [0.0, 1.0, -352.0, -119.5, 0.0],
            [-1.0, -1.0, -404.0, -119.5, 0.0],
            [0.0, 0.0, 142.0, 60.5, 1.0],
            [1.0, 0.0, -164.0, -29.5, 0.0],
            [1.0, 0.0, 292.0, 60.5, 1.0],
        ],
        ghosts: &[[2.0, 0.0, 1.0, -66.0, -29.5], [2.0, 1.0, 1.0, 390.0, 60.5]],
        frames: &["Def", "Atk"],
        frame_bounds: &[[-454.0, -174.5, -2.0, 174.5], [78.0, -27.5, 454.0, 174.5]],
        boxes: &[
            1.0, -352.0, -119.5, 156.0, 62.0, 12.0, -343.5, -120.5, -298.5, -100.5, -356.0, -138.5,
            -286.0, -120.5, 2.0, -404.0, -119.5, 14.0, -386.0, -133.5, -362.0, -105.5,
        ],
        bounds: &[-430.0, -150.5, 404.0, 100.5],
        ranks: 2,
        back: &[],
    },
    Pinned {
        name: "a part for two groups",
        nodes: &[
            [0.0, 0.0, -114.0, 137.0, 0.0],
            [0.0, 0.0, 194.0, 137.0, 1.0],
            [-1.0, -1.0, -1e9, -1e9, -1.0],
            [-1.0, -1.0, -1e9, -1e9, -1.0],
            [2.0, 0.0, -114.0, -163.0, 0.0],
            [1.0, 0.0, -114.0, -13.0, 0.0],
        ],
        ghosts: &[[1.0, 0.0, 0.0, -114.0, 50.0]],
        frames: &["A", "B"],
        frame_bounds: &[[-258.0, -227.0, -30.0, 227.0], [50.0, 73.0, 258.0, 227.0]],
        boxes: &[],
        bounds: &[-174.0, -203.0, 234.0, 177.0],
        ranks: 3,
        back: &[],
    },
    Pinned {
        name: "a part for one group",
        nodes: &[
            [0.0, 0.0, -180.0, 150.0, -1.0],
            [-1.0, -1.0, -1e9, -1e9, -1.0],
            [0.0, 1.0, 0.0, 150.0, -1.0],
            [0.0, 2.0, 180.0, 150.0, -1.0],
            [2.0, 0.0, 0.0, -150.0, -1.0],
            [1.0, 0.0, 0.0, 0.0, -1.0],
        ],
        ghosts: &[[1.0, 0.0, 0.0, 0.0, 63.0]],
        frames: &[],
        frame_bounds: &[],
        boxes: &[],
        bounds: &[-220.0, -190.0, 220.0, 190.0],
        ranks: 3,
        back: &[],
    },
];

/// The nets with some nodes in no group, clustered by name prefix, laid
/// out at eb323d3 under the names `Group/Name`.
const SOME_IN_NONE: &[Pinned] = &[
    Pinned {
        name: "mixed",
        nodes: &[
            [0.0, 0.0, -268.0, 137.0, 0.0],
            [2.0, 0.0, -268.0, -163.0, 0.0],
            [1.0, 0.0, 40.0, 2.0, 1.0],
            [0.0, 0.0, 348.0, 137.0, 2.0],
            [1.0, 0.0, -268.0, -13.0, 0.0],
            [0.0, 0.0, 40.0, 152.0, 1.0],
        ],
        ghosts: &[],
        frames: &["A", "B", ""],
        frame_bounds: &[
            [-412.0, -227.0, -184.0, 227.0],
            [-104.0, -62.0, 124.0, 227.0],
            [204.0, 73.0, 412.0, 227.0],
        ],
        boxes: &[],
        bounds: &[-328.0, -203.0, 388.0, 177.0],
        ranks: 3,
        back: &[],
    },
    Pinned {
        name: "mixed, left right, fitted",
        nodes: &[
            [0.0, 0.0, -189.0, 128.0, 0.0],
            [2.0, 0.0, 111.0, 128.0, 0.0],
            [1.0, 0.0, -19.0, -154.0, 1.0],
            [0.0, 0.0, 189.0, -154.0, 2.0],
            [1.0, 0.0, -39.0, 128.0, 0.0],
            [0.0, 0.0, -169.0, -154.0, 1.0],
        ],
        ghosts: &[],
        frames: &["A", "B", ""],
        frame_bounds: &[
            [-253.0, 40.0, 175.0, 242.0],
            [-253.0, -242.0, 45.0, -40.0],
            [125.0, -242.0, 253.0, -40.0],
        ],
        boxes: &[],
        bounds: &[-229.0, -194.0, 229.0, 168.0],
        ranks: 3,
        back: &[],
    },
    Pinned {
        name: "a part for two groups, with the rest",
        nodes: &[
            [0.0, 0.0, -167.0, 125.0, 0.0],
            [0.0, 0.0, -167.0, -157.0, 1.0],
            [-1.0, -1.0, -1e9, -1e9, -1.0],
            [0.0, 0.0, 41.0, -157.0, -1.0],
            [2.0, 0.0, 167.0, 125.0, 0.0],
            [1.0, 0.0, 17.0, 125.0, 0.0],
        ],
        ghosts: &[[1.0, 0.0, 0.0, -81.0, 165.0], [2.0, 0.0, 0.0, -81.0, 85.0]],
        frames: &["A", "B"],
        frame_bounds: &[
            [-231.0, 37.0, 231.0, 245.0],
            [-231.0, -245.0, -103.0, -43.0],
        ],
        boxes: &[],
        bounds: &[-207.0, -197.0, 207.0, 179.0],
        ranks: 3,
        back: &[],
    },
];
