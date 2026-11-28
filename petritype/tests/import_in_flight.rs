//! A marking is not imported under a firing in flight (since 2026-11-28):
//! `Net::import_marking` refuses with `NetError::InFlight`, and changes
//! nothing, while a firing of the net is airborne, or one of a subnet of it
//! at any depth, since an import fills each subnet where it stands and the
//! landing would put the firing's outputs into the marking imported. A net
//! a token carries is replaced by an import, not filled, so a firing of it
//! is not asked about. A firing of the net around a subnet on one of its
//! ports keeps the subnet out of reach, so no import into it meets one.

#![cfg(feature = "serde")]

use std::error::Error;

use petritype::{
    CommandError, Marking, Net, NetBuilder, NetError, Params, PathStep, TakeEdge, Templates,
};

type BoxedError = Box<dyn Error + Send + Sync>;

/// `Desk`: `Todo`, its port, and `Done`, with `Work` between them.
fn desk(_: &Params) -> Result<Net, BoxedError> {
    let mut b = NetBuilder::new();
    let todo = b.place::<u32>("Todo");
    let done = b.place::<u32>("Done");
    b.serializable(todo);
    b.serializable(done);
    b.port(todo);
    b.transition("Work", (TakeEdge(todo),), (done,), |n: u32| (n,));
    Ok(b.build()?)
}

/// `Office`: an `Inbox`, its port, and the subnet place `Desk`, with `Hand
/// In` from the inbox to the desk's `Todo`; the desk starts with one job.
fn office(templates: &Templates) -> Result<Net, BoxedError> {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    b.serializable(inbox);
    b.port(inbox);
    let desk = b.subnet_place("Desk", templates.build("Desk", &Params::new())?);
    let todo = b.port_of::<u32>(desk, "Todo");
    b.seed(todo, [7]);
    b.seed(inbox, [8]);
    b.transition("Hand In", (TakeEdge(inbox),), (todo,), |n: u32| (n,));
    Ok(b.build()?)
}

/// `Tower`: the subnet place `Office`, an office one level down.
fn tower(templates: &Templates) -> Result<Net, BoxedError> {
    let mut b = NetBuilder::new();
    b.subnet_place("Office", templates.build("Office", &Params::new())?);
    Ok(b.build()?)
}

fn templates() -> Templates {
    let mut templates = Templates::new();
    templates.register("Desk", desk);
    let inner = templates.clone();
    templates.register("Office", move |_: &Params| office(&inner));
    let inner = templates.clone();
    templates.register("Tower", move |_: &Params| tower(&inner));
    templates
}

fn built(name: &str) -> Net {
    templates()
        .build(name, &Params::new())
        .expect("the template builds")
}

/// The refusal, if `import` was refused for a firing in flight.
fn in_flight(import: Result<(), CommandError>) -> (String, Vec<PathStep>) {
    match import {
        Err(CommandError::Net(NetError::InFlight { name, path, .. })) => (name, path),
        other => panic!("expected InFlight, got {other:?}"),
    }
}

#[test]
fn an_import_waits_for_a_firing_of_the_net_s_own() {
    let mut office = built("Office");
    let hand_in = office.transition_named("Hand In").unwrap();
    let before = office.export_marking().unwrap();
    let firing = office.launch(hand_in).unwrap();
    office.drain_events().for_each(drop);

    let refused = office.import_marking(&Marking::new());
    assert_eq!(in_flight(refused), ("Hand In".to_owned(), vec![]));
    assert_eq!(office.drain_events().count(), 0, "nothing told");
    assert!(office.is_in_flight(hand_in), "still airborne");

    office.distribute(firing.run()).unwrap();
    office.import_marking(&before).expect("landed, so imported");
    assert_eq!(office.export_marking().unwrap(), before);
}

#[test]
fn an_import_waits_for_a_firing_inside_a_subnet() {
    let mut office = built("Office");
    let desk = office.place_named("Desk").unwrap();
    let before = office.export_marking().unwrap();
    let firing = office
        .subnet_scope(desk, |desk| {
            let work = desk.transition_named("Work").unwrap();
            desk.launch(work)
        })
        .unwrap()
        .unwrap();
    office.drain_events().for_each(drop);
    let midway = office.export_marking();

    let refused = office.import_marking(&Marking::new());
    let (name, path) = in_flight(refused);
    assert_eq!(name, "Work");
    assert_eq!(path, [PathStep::Subnet { place: desk }]);
    assert_eq!(
        CommandError::Net(NetError::InFlight {
            transition: office
                .subnet(desk)
                .unwrap()
                .transition_named("Work")
                .unwrap(),
            name,
            path,
        })
        .to_string(),
        format!("transition \"Work\" (TransitionId(0)) inside {desk:?} is in flight"),
    );
    assert_eq!(office.drain_events().count(), 0, "nothing told");
    assert!(
        midway.is_err(),
        "a save refuses under the same firing, as the import does"
    );

    office
        .subnet_scope(desk, |desk| desk.distribute(firing.run()))
        .unwrap()
        .unwrap();
    office.import_marking(&before).expect("landed, so imported");
    assert_eq!(office.export_marking().unwrap(), before);
}

#[test]
fn an_import_waits_for_a_firing_two_subnets_down() {
    let mut tower = built("Tower");
    let office_at = tower.place_named("Office").unwrap();
    let firing = tower
        .subnet_scope(office_at, |office| {
            let desk = office.place_named("Desk").unwrap();
            office
                .subnet_scope(desk, |desk| {
                    let work = desk.transition_named("Work").unwrap();
                    desk.launch(work)
                })
                .map(|launched| (desk, launched))
        })
        .unwrap()
        .unwrap();
    let (desk, firing) = (firing.0, firing.1.unwrap());

    let (name, path) = in_flight(tower.import_marking(&Marking::new()));
    assert_eq!(name, "Work");
    assert_eq!(
        path,
        [
            PathStep::Subnet { place: office_at },
            PathStep::Subnet { place: desk },
        ]
    );

    // The office in between refuses for the desk below it, one level up.
    let (_, path) = in_flight(
        tower
            .subnet_scope(office_at, |office| office.import_marking(&Marking::new()))
            .unwrap(),
    );
    assert_eq!(path, [PathStep::Subnet { place: desk }]);

    tower
        .subnet_scope(office_at, |office| {
            office.subnet_scope(desk, |desk| desk.distribute(firing.run()))
        })
        .unwrap()
        .unwrap()
        .unwrap();
    tower
        .import_marking(&Marking::new())
        .expect("landed, so imported");
}

#[test]
fn a_subnet_under_a_firing_on_its_ports_is_out_of_reach_of_an_import() {
    let mut office = built("Office");
    let desk = office.place_named("Desk").unwrap();
    let hand_in = office.transition_named("Hand In").unwrap();
    let firing = office.launch(hand_in).unwrap();

    // The scope is refused before the closure could import anything.
    let reached = office.subnet_scope(desk, |desk| desk.import_marking(&Marking::new()));
    assert!(matches!(
        reached,
        Err(NetError::InFlight { transition, .. }) if transition == hand_in
    ));
    office.distribute(firing.run()).unwrap();
    assert!(
        office
            .subnet_scope(desk, |desk| desk.import_marking(&Marking::new()))
            .is_ok(),
        "landed: the scope opens, and the import is the closed net's to refuse or take"
    );
}

#[test]
fn a_firing_of_a_net_a_token_carries_does_not_hold_an_import_back() {
    let mut inner = NetBuilder::new();
    let todo = inner.place::<u32>("Todo");
    let done = inner.place::<u32>("Done");
    inner.seed(todo, [1]);
    let work = inner
        .transition("Work", (TakeEdge(todo),), (done,), |n: u32| (n,))
        .id();
    inner.open();
    let inner = inner.build().unwrap();

    let mut b = NetBuilder::new();
    let yard = b.place_nested::<Net>("Yard");
    b.serializable(yard);
    b.seed(yard, [inner]);
    b.open();
    let mut net = b.build().unwrap();
    let (token, _) = net.tokens(yard.id()).next().unwrap();
    let firing = net
        .nested_scope(yard.id(), token, |nets| nets[0].1.launch(work))
        .unwrap()
        .unwrap();

    // The token and its net are replaced, not filled: the firing cannot
    // land in the marking imported.
    net.import_marking(&Marking::new())
        .expect("a token's net is replaced by an import");
    assert_eq!(net.count(yard.id()), 0);
    drop(firing);
}
