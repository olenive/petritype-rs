//! A subnet place's box: which side each port goes on, its size, and
//! where its title, its count and its circles go.

use petritype::PlaceId;

use crate::{
    Bounds, COUNT_WIDTH, Direction, GHOST_CLEARANCE, Layout, PLACE_CHAR_WIDTH, Point, PortCircle,
    SubnetBox,
};

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

/// Which side of a subnet place's box a port's circle is on, along the main
/// axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoxSide {
    /// The side the ranks come from: the top, or the left.
    Before,
    /// The side the ranks go to: the bottom, or the right.
    After,
}

impl Layout {
    /// The plan of a subnet place's box titled `title`: which side each
    /// port goes on, and the size that makes room for them. A port with no
    /// side of its own joins the side with more ports, the side the ranks
    /// come from on a tie.
    pub(crate) fn plan_box(&self, title: &str, sides: &[(PlaceId, Option<BoxSide>)]) -> BoxPlan {
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
    pub(crate) fn place_box(
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
    pub(crate) fn box_clearance(&self) -> f32 {
        let transition = match self.direction {
            Direction::TopDown => self.transition_size.y / 2.0,
            Direction::LeftRight => self.transition_size.x / 2.0,
        };
        (self.rank_sep - self.place_radius - transition).max(GHOST_CLEARANCE)
    }
}

/// A subnet place's box before it is placed: its size, and which ports go
/// on which side.
#[derive(Debug, Clone, Default)]
pub(crate) struct BoxPlan {
    /// The box's width and height.
    pub(crate) size: Point,
    /// Its ports, in the order given.
    order: Vec<PlaceId>,
    /// The ports on the side the ranks come from, in the order given.
    pub(crate) before: Vec<PlaceId>,
    /// The ports on the other side, in the order given.
    pub(crate) after: Vec<PlaceId>,
}

#[cfg(test)]
mod tests {
    use petritype::{Net, NetBuilder, TakeEdge, TransitionId};

    use super::*;
    use crate::tests::{id_of, laid_out, laid_out_open};
    use crate::{GhostSide, Ghosting, LayoutNode, LayoutPlace, PlaceKind};

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
