//! No name is the empty string (since 2026-11-28): the build refuses one
//! for a place of every kind, a subnet place, a key, a transition, a group,
//! a control, a parameter's wire name and a template, with
//! `BuildError::EmptyName` saying which, and a keys declaration with no key
//! at all with `BuildError::NoKeys`. A name may be anything else, a space
//! included. Two empty names keep their meaning: a bare net carried as a
//! token is its own unnamed inner net, and a control that acts on no node
//! says so with `None`.

use petritype::control::{Control, Pressed};
use petritype::{BuildError, Named, Net, NetBuilder, TakeEdge};

fn empty(b: NetBuilder) -> Named {
    match b.build() {
        Err(BuildError::EmptyName { what }) => what,
        other => panic!("expected EmptyName, got {other:?}"),
    }
}

fn noop(_: &mut Net, _: petritype::control::Value) -> Result<Pressed, String> {
    Ok(Pressed::Applied)
}

#[test]
fn a_place_of_every_kind_is_refused_the_empty_name() {
    let mut b = NetBuilder::new();
    b.place::<u32>("");
    assert_eq!(empty(b), Named::Place);
    let mut b = NetBuilder::new();
    b.place_no_clone::<u32>("");
    assert_eq!(empty(b), Named::Place);
    let mut b = NetBuilder::new();
    b.place_nested::<Net>("");
    assert_eq!(empty(b), Named::Place);
    let mut b = NetBuilder::new();
    b.subnet_place("", NetBuilder::new().build().unwrap());
    assert_eq!(empty(b), Named::SubnetPlace);

    // Two places named "" are refused for the name, not as a duplicate.
    let mut b = NetBuilder::new();
    b.place::<u32>("");
    b.place::<u32>("");
    let error = b.build().unwrap_err();
    assert_eq!(
        error.to_string(),
        "a place is named by the empty string: a name has at least one character"
    );
}

#[test]
fn a_key_is_refused_the_empty_name_and_a_declaration_no_key() {
    let mut b = NetBuilder::new();
    let orders = b.place::<String>("Orders");
    b.keys(orders, ["unit", ""], |o: &String| {
        [o.as_str().into(), o.as_str().into()]
    });
    assert_eq!(
        empty(b),
        Named::Key {
            place: "Orders".into()
        }
    );

    let mut b = NetBuilder::new();
    let orders = b.place::<String>("Orders");
    b.keys(orders, [], |_: &String| []);
    let error = b.build().unwrap_err();
    assert!(
        matches!(&error, BuildError::NoKeys { place } if place == "Orders"),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "place \"Orders\" declares its keys with no key in them"
    );
}

#[test]
fn a_transition_a_group_and_a_control_are_refused_the_empty_name() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    b.transition("", (TakeEdge(inbox),), (), |_: u32| {});
    assert_eq!(empty(b), Named::Transition);

    let mut b = NetBuilder::new();
    b.group("");
    assert_eq!(empty(b), Named::Group);

    let mut b = NetBuilder::new();
    b.control(Control::button("", "Press", noop));
    assert_eq!(empty(b), Named::Control);
}

#[test]
fn a_name_of_one_character_or_a_space_is_a_name() {
    let mut b = NetBuilder::new();
    let place = b.place::<String>(" ");
    b.keys(place, ["k"], |s: &String| [s.as_str().into()]);
    b.transition("T", (TakeEdge(place),), (), |_: String| {});
    b.group(" ");
    b.control(Control::button("c", "", noop));
    b.build()
        .expect("every name has a character; a label may be empty");
}

#[test]
fn the_two_empty_names_with_a_meaning_are_kept() {
    let inner = NetBuilder::new().build().unwrap();
    let mut b = NetBuilder::new();
    let yard = b.place_nested::<Net>("Yard");
    b.seed(yard, [inner]);
    b.control(Control::button("noop", "No-op", noop));
    let net = b.build().expect("a bare net is its own unnamed inner net");
    let (token, _) = net.tokens(yard.id()).next().unwrap();
    assert_eq!(net.nested(yard.id(), token).unwrap()[0].0, "");
    assert_eq!(net.controls()[0].node(), None, "a control on no node");
}

#[cfg(feature = "serde")]
mod with_serde {
    use super::*;
    use petritype::serde_json::json;
    use petritype::{Params, TemplateError, Templates};

    #[derive(Clone, serde::Serialize, serde::Deserialize)]
    struct Limit(u32);

    #[test]
    fn a_parameter_s_wire_name_is_refused_the_empty_string() {
        let mut b = NetBuilder::new();
        b.param(Limit(3));
        b.serializable_param::<Limit>("");
        assert!(matches!(
            empty(b),
            Named::Parameter { type_name } if type_name.ends_with("Limit")
        ));
    }

    #[test]
    fn a_template_is_refused_the_empty_name() {
        let mut b = NetBuilder::new();
        b.template("", |_: &Params| NetBuilder::new().build());
        assert_eq!(empty(b), Named::Template);

        let mut templates = Templates::new();
        templates.register("", |_: &Params| NetBuilder::new().build());
        let mut b = NetBuilder::new();
        b.templates(&templates);
        assert_eq!(empty(b), Named::Template);

        assert!(matches!(
            templates.build("", &Params::new()),
            Err(TemplateError::EmptyName)
        ));
        let saved = json!({"template": "", "marking": {}, "params": {}, "enabled": {}});
        let restored = templates.restore(saved);
        assert!(restored.is_err(), "no net is rebuilt under the empty name");
    }
}
