//! What a net reads once and keeps follows what changes it: the value of a
//! parameter that a `{key, param}` clause reads, kept from the first time a
//! clause asks until the value is set again (since 2026-11-28), and whether
//! the net holds nets at all, which a deep step asks of a flat net (since
//! 2026-11-28). Each test here reads first, so that what is kept is there,
//! then changes the net, and asks again.

use petritype::{NetBuilder, Stepping, TakeEdge};

/// A worker: `Todo -Done->`, with `n` to do, its `Todo` a port.
fn worker(n: u32) -> petritype::Net {
    let mut b = NetBuilder::new();
    let todo = b.place::<u32>("Todo");
    b.port(todo);
    b.seed(todo, 0..n);
    b.transition("Done", (TakeEdge(todo),), (), |_: u32| {});
    b.build().expect("a worker")
}

/// A net whose nested place is empty when it is built is stepped into
/// once a token brings a net there: that it holds nets is a fact of its
/// places, not of its tokens.
#[test]
fn a_nested_place_empty_at_build_is_stepped_into_once_a_net_arrives() {
    let mut b = NetBuilder::new();
    let desk = b.place_nested::<petritype::Net>("Desk");
    b.open();
    let mut net = b.build().expect("an empty desk");
    assert_eq!(net.step_deep().unwrap(), None, "nothing to step");
    assert!(net.is_quiescent_deep());

    net.put(desk, worker(2));
    assert!(!net.is_quiescent_deep(), "the worker has work");
    assert!(net.step_deep().unwrap().is_some(), "the worker's turn");
    assert!(net.step_deep().unwrap().is_some());
    assert_eq!(net.step_deep().unwrap(), None, "done");
}

/// A passive place's nets are stepped once its rule says they may be: the
/// rule is read at each step, whatever it was when the net was built.
#[test]
fn a_passive_place_made_inner_first_later_is_stepped_into() {
    let mut b = NetBuilder::new();
    let desk = b.place_nested::<petritype::Net>("Desk");
    b.stepping(desk, Stepping::Passive);
    b.seed(desk, [worker(1)]);
    let mut net = b.build().expect("a passive desk");
    assert_eq!(
        net.step_deep().unwrap(),
        None,
        "a passive place is left alone"
    );

    net.set_stepping(desk.id(), Stepping::InnerFirst).unwrap();
    assert!(net.step_deep().unwrap().is_some(), "now its net has a turn");
}

#[cfg(feature = "serde")]
mod params {
    use petritype::{
        Filter, JsonCommand, KeyValue, Net, NetBuilder, Params, PlaceRef, TakeWhereEdge, Templates,
        TransitionId,
    };
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Animal {
        species: String,
    }

    fn animal(species: &str) -> Animal {
        Animal {
            species: species.to_owned(),
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct Season(String);

    /// A field of animals keyed by species, and `Take`, which takes one of
    /// the species the parameter `season` names, `"Fox"` as built.
    struct Zoo {
        net: Net,
        field: PlaceRef<Animal>,
        take: TransitionId,
    }

    fn zoo_net() -> Result<Net, petritype::BuildError> {
        let mut b = NetBuilder::new();
        let field = b.place::<Animal>("Field");
        let out = b.place::<Animal>("Out");
        b.serializable(field);
        b.serializable(out);
        b.port(field);
        b.keys(field, ["species"], |a: &Animal| [a.species.as_str().into()]);
        b.param(Season("Fox".to_owned()));
        b.serializable_param::<Season>("season");
        b.seed(field, [animal("Owl")]);
        b.transition(
            "Take",
            (TakeWhereEdge(
                field,
                Filter::new().key_is_param("species", "season"),
            ),),
            (out,),
            |a: Animal| (a,),
        );
        b.build()
    }

    fn zoo() -> Zoo {
        let net = zoo_net().expect("a zoo");
        let field = net.place_ref::<Animal>("Field").expect("a field");
        let take = net.transition_named("Take").expect("a take");
        Zoo { net, field, take }
    }

    fn in_season() -> Filter {
        Filter::new().key_is_param("species", "season")
    }

    /// Read as a fox, the parameter is read again once it is set, by the
    /// typed call and by a JSON command alike, for the keyed edge and for a
    /// query.
    #[test]
    fn a_param_clause_reads_the_value_set_after_it_was_read() {
        let Zoo {
            mut net,
            field,
            take,
        } = zoo();
        assert!(net.fireable().is_empty(), "no fox in the field");
        assert!(net.query_ids(&in_season()).unwrap().is_empty());

        net.set_param(Season("Owl".to_owned())).unwrap();
        assert_eq!(net.fireable(), [take], "the owl is in season now");
        let (owl, _) = net.tokens(field.id()).next().unwrap();
        assert_eq!(net.query_ids(&in_season()).unwrap(), [(field.id(), owl)]);

        net.apply_json(JsonCommand::SetParam {
            param: "season".to_owned(),
            value: serde_json::json!("Fox"),
        })
        .unwrap();
        assert!(net.fireable().is_empty(), "back to foxes, by JSON");
        net.put(field, animal("Fox"));
        assert_eq!(net.step().unwrap(), Some(take));
        assert_eq!(net.count(field.id()), 1, "the fox was taken, the owl stays");
        assert_eq!(
            net.tokens(field.id()).next().map(|(token, _)| token),
            Some(owl)
        );
    }

    /// A fork reads its own parameter, given the original's value, and the
    /// original goes on reading its own. The factory asks its net what is
    /// fireable before it hands it over, so the fork has read its parameter
    /// as built before the original's value is set on it.
    #[test]
    fn a_fork_reads_its_own_parameter() {
        let mut templates = Templates::new();
        templates.register("Zoo", |_: &Params| {
            let net = zoo_net()?;
            assert!(net.fireable().is_empty(), "read as a fox, as built");
            Ok::<_, petritype::BuildError>(net)
        });
        let mut net = templates.build("Zoo", &Params::new()).unwrap();
        let take = net.transition_named("Take").unwrap();
        assert!(net.fireable().is_empty(), "read as a fox");

        net.set_param(Season("Owl".to_owned())).unwrap();
        let mut fork = net.fork().unwrap();
        assert_eq!(fork.fireable(), [take], "the fork has the owl's season");

        fork.set_param(Season("Fox".to_owned())).unwrap();
        assert!(fork.fireable().is_empty());
        assert_eq!(net.fireable(), [take], "the original keeps its own");
    }

    /// A subnet's parameter that an import puts back is read again.
    #[test]
    fn a_subnet_parameter_an_import_puts_back_is_read_again() {
        let mut templates = Templates::new();
        templates.register("Zoo", |_: &Params| zoo_net());
        let mut b = NetBuilder::new();
        let pen = b.subnet_place("Pen", templates.build("Zoo", &Params::new()).unwrap());
        b.templates(&templates);
        let mut world = b.build().unwrap();
        let saved = world.export_marking().unwrap();

        let fireable = |world: &mut Net| {
            world
                .subnet_scope(pen.id(), |zoo| zoo.fireable().len())
                .unwrap()
        };
        assert_eq!(fireable(&mut world), 0, "read as a fox");
        world
            .subnet_scope(pen.id(), |zoo| zoo.set_param(Season("Owl".to_owned())))
            .unwrap()
            .unwrap();
        assert_eq!(fireable(&mut world), 1, "read as an owl");

        world.import_marking(&saved).unwrap();
        assert_eq!(fireable(&mut world), 0, "the import put the fox back");
    }

    /// A snapshot reads the parameter as it was when it was taken, whatever
    /// the net reads after it, and so does a clone of the snapshot.
    #[test]
    fn a_snapshot_reads_the_parameter_as_it_was() {
        let Zoo { mut net, field, .. } = zoo();
        let (owl, _) = net.tokens(field.id()).next().unwrap();
        net.set_param(Season("Owl".to_owned())).unwrap();
        let taken = net.snapshot().unwrap();
        assert_eq!(taken.query_ids(&in_season()).unwrap(), [(field.id(), owl)]);

        net.set_param(Season("Fox".to_owned())).unwrap();
        assert!(net.query_ids(&in_season()).unwrap().is_empty());
        assert_eq!(taken.query_ids(&in_season()).unwrap(), [(field.id(), owl)]);
        assert_eq!(
            taken.clone().query_ids(&in_season()).unwrap(),
            [(field.id(), owl)]
        );
        assert_eq!(
            taken.distinct_values("species", None),
            [KeyValue::from("Owl")]
        );
    }

    /// A keyed edge on a place for a subnet's port reads the parameter of
    /// its own net, the net around the subnet, after that one changes.
    #[test]
    fn a_window_takes_by_its_own_nets_parameter() {
        let mut b = NetBuilder::new();
        let pen = b.subnet_place("Pen", zoo_net().unwrap());
        let gate = b.port_of::<Animal>(pen, "Field");
        let shed = b.place::<Animal>("Shed");
        b.param(Season("Owl".to_owned()));
        b.serializable_param::<Season>("season");
        b.transition(
            "Fetch",
            (TakeWhereEdge(
                gate,
                Filter::new().key_is_param("species", "season"),
            ),),
            (shed,),
            |a: Animal| (a,),
        );
        let mut world = b.build().unwrap();
        let fetch = world.transition_named("Fetch").unwrap();
        assert_eq!(
            world.fireable(),
            [fetch],
            "an owl at the gate, owls in season out here"
        );

        world.set_param(Season("Fox".to_owned())).unwrap();
        assert!(world.fireable().is_empty(), "foxes now, out here");
    }
}
