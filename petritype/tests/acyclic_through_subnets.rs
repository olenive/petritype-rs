//! The acyclic check follows a subnet place through (since 2026-11-28): a
//! token put at one of its ports goes on from every port its subnet's own
//! token flow leads it to, at any depth, so a cycle that leaves the net at
//! one port and comes back at another is found, and so is one inside the
//! subnet that comes back to the port it left. A cycle wholly inside a
//! subnet, through none of its ports, is the subnet's own: its definition
//! asks for the check. The Python library walks the same way.

use petritype::{BuildError, DrainEdge, Net, NetBuilder, SnapshotEdge, SubnetRef, TakeEdge};

/// A subnet `In -Pass-> Out`, both of them ports.
fn pipe() -> Net {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("In");
    let out = b.place::<u32>("Out");
    b.port(inbox);
    b.port(out);
    b.transition("Pass", (TakeEdge(inbox),), (out,), |n: u32| (n,));
    b.build().expect("a pipe")
}

/// A subnet whose way from `In` to `Out` goes through a pipe of its own:
/// `In -Down-> Inner/In`, then `Inner/Out -Up-> Out`.
fn deeper_pipe() -> Net {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("In");
    let out = b.place::<u32>("Out");
    b.port(inbox);
    b.port(out);
    let inner = b.subnet_place("Inner", pipe());
    let inner_in = b.port_of::<u32>(inner, "In");
    let inner_out = b.port_of::<u32>(inner, "Out");
    b.transition("Down", (TakeEdge(inbox),), (inner_in,), |n: u32| (n,));
    b.transition("Up", (TakeEdge(inner_out),), (out,), |n: u32| (n,));
    b.build().expect("a deeper pipe")
}

/// `Back` takes from the subnet's `Out` and puts into its `In`.
fn back_through(b: &mut NetBuilder, name: &str, subnet: Net) -> SubnetRef {
    let s = b.subnet_place(name, subnet);
    let inbox = b.port_of::<u32>(s, "In");
    let out = b.port_of::<u32>(s, "Out");
    b.transition(
        format!("Back {name}"),
        (TakeEdge(out),),
        (inbox,),
        |n: u32| (n,),
    );
    s
}

fn cycle(b: NetBuilder) -> Vec<String> {
    match b.build() {
        Err(BuildError::Cycle { path }) => path,
        other => panic!("expected a cycle, got {other:?}"),
    }
}

#[test]
fn a_cycle_through_a_subnet_is_found_and_named() {
    for subnet in [pipe(), deeper_pipe()] {
        let mut b = NetBuilder::new();
        b.assert_acyclic();
        back_through(&mut b, "S", subnet);
        let path = cycle(b);
        assert_eq!(
            path,
            [
                "Back S",
                "S/In",
                "(inside subnet place \"S\")",
                "S/Out",
                "Back S"
            ]
        );
        assert_eq!(
            BuildError::Cycle { path }.to_string(),
            "the net's token flow has a cycle: Back S -> S/In -> (inside subnet place \"S\") -> \
             S/Out -> Back S"
        );
    }
}

#[test]
fn a_cycle_through_two_subnets_is_found() {
    let mut b = NetBuilder::new();
    b.assert_acyclic();
    let a = b.subnet_place("A", pipe());
    let c = b.subnet_place("B", deeper_pipe());
    let (a_in, a_out) = (b.port_of::<u32>(a, "In"), b.port_of::<u32>(a, "Out"));
    let (c_in, c_out) = (b.port_of::<u32>(c, "In"), b.port_of::<u32>(c, "Out"));
    b.transition("Over", (TakeEdge(a_out),), (c_in,), |n: u32| (n,));
    b.transition("Back", (TakeEdge(c_out),), (a_in,), |n: u32| (n,));
    let path = cycle(b);
    assert_eq!(
        path,
        [
            "Over",
            "B/In",
            "(inside subnet place \"B\")",
            "B/Out",
            "Back",
            "A/In",
            "(inside subnet place \"A\")",
            "A/Out",
            "Over"
        ]
    );
}

#[test]
fn a_pipeline_through_a_subnet_passes() {
    let mut b = NetBuilder::new();
    b.assert_acyclic();
    let start = b.place::<u32>("Start");
    let end = b.place::<u32>("End");
    b.seed(start, [1]);
    let s = b.subnet_place("S", deeper_pipe());
    let (s_in, s_out) = (b.port_of::<u32>(s, "In"), b.port_of::<u32>(s, "Out"));
    b.transition("Enter", (TakeEdge(start),), (s_in,), |n: u32| (n,));
    b.transition("Leave", (TakeEdge(s_out),), (end,), |n: u32| (n,));
    let mut net = b
        .build()
        .expect("no cycle: in at one end, out at the other");
    while net.step_deep().expect("nothing fails").is_some() {}
    assert_eq!(net.count(end.id()), 1);
}

#[test]
fn a_way_back_inside_the_subnet_to_the_port_it_left_is_a_cycle() {
    let mut inner = NetBuilder::new();
    let inbox = inner.place::<u32>("In");
    let middle = inner.place::<u32>("Middle");
    inner.port(inbox);
    inner.transition("Out", (TakeEdge(inbox),), (middle,), |n: u32| (n,));
    inner.transition("Back", (TakeEdge(middle),), (inbox,), |n: u32| (n,));
    let inner = inner
        .build()
        .expect("the subnet does not ask for the check");

    let mut b = NetBuilder::new();
    b.assert_acyclic();
    b.subnet_place("S", inner);
    assert_eq!(cycle(b), ["S/In", "(inside subnet place \"S\")", "S/In"]);
}

#[test]
fn a_drain_or_a_snapshot_inside_the_subnet_leads_nowhere() {
    // As in the net around: neither moves a token for certain.
    let drained = {
        let mut b = NetBuilder::new();
        let inbox = b.place::<u32>("In");
        let out = b.place::<u32>("Out");
        b.port(inbox);
        b.port(out);
        b.transition("Pass", (DrainEdge(inbox),), (out,), |all: Vec<u32>| {
            (all.len() as u32,)
        });
        b.build().expect("a drain")
    };
    let copied = {
        let mut b = NetBuilder::new();
        let inbox = b.place::<u32>("In");
        let out = b.place::<u32>("Out");
        b.port(inbox);
        b.port(out);
        b.transition("Pass", (SnapshotEdge(inbox),), (out,), |n: u32| (n,));
        b.build().expect("a snapshot")
    };
    for subnet in [drained, copied] {
        let mut b = NetBuilder::new();
        b.assert_acyclic();
        back_through(&mut b, "S", subnet);
        b.build().expect("no token is sure to go round");
    }
}

#[test]
fn a_cycle_inside_a_subnet_through_none_of_its_ports_is_the_subnet_s_own() {
    let looped = |acyclic: bool| {
        let mut b = NetBuilder::new();
        if acyclic {
            b.assert_acyclic();
        }
        let inbox = b.place::<u32>("In");
        let one = b.place::<u32>("P1");
        let two = b.place::<u32>("P2");
        b.port(inbox);
        b.transition("T1", (TakeEdge(one),), (two,), |n: u32| (n,));
        b.transition("T2", (TakeEdge(two),), (one,), |n: u32| (n,));
        b.build()
    };
    assert!(matches!(
        looped(true),
        Err(BuildError::Cycle { path }) if path == ["T1", "P2", "T2", "P1", "T1"]
    ));
    let mut b = NetBuilder::new();
    b.assert_acyclic();
    b.subnet_place("S", looped(false).expect("not asked"));
    b.build()
        .expect("the subnet's own cycle, which its definition did not ask about");
}

#[test]
fn a_cycle_of_the_net_s_own_on_a_port_names_the_port_by_its_subnet_place() {
    let mut b = NetBuilder::new();
    b.assert_acyclic();
    let s = b.subnet_place("S", pipe());
    let s_out = b.port_of::<u32>(s, "Out");
    b.transition("Again", (TakeEdge(s_out),), (s_out,), |n: u32| (n,));
    assert_eq!(cycle(b), ["Again", "S/Out", "Again"]);
}
