//! The core's errors mapped onto the protocol's closed set of codes
//! (DESIGN §19.3.7): one function, a test per arm.

use petritype::{CommandError, NetError};

use crate::protocol::ErrorCode;
use crate::registry::LoadError;
use crate::translate::FilterError;

/// An error from the core or a factory, borrowed, for [`code_of`].
#[derive(Debug, Clone, Copy)]
pub(crate) enum CoreError<'a> {
    /// From `apply_json` and friends.
    Command(&'a CommandError),
    /// From the net's own methods.
    Net(&'a NetError),
    /// From a factory.
    Load(&'a LoadError),
    /// From reading a request's filter (PROTOCOL.md §13.2).
    Filter(&'a FilterError),
}

/// The protocol code for a core or factory error. The message sent with it is
/// the error's `Display`.
pub(crate) fn code_of(error: CoreError<'_>) -> ErrorCode {
    match error {
        CoreError::Command(error) => match error {
            CommandError::Net(error) => code_of(CoreError::Net(error)),
            CommandError::NoSuchPlaceName { .. } => ErrorCode::UnknownPlace,
            CommandError::NoSuchTransitionName { .. } => ErrorCode::UnknownTransition,
            CommandError::NotSerializable { .. } => ErrorCode::NotSerializable,
            // Also a nested token whose saved net the core refuses
            // (PROTOCOL.md §12.5): an unknown template, a parameter the
            // template does not declare, an unknown inner place, an inner
            // token that does not decode. The core reports each through the
            // token's `Deserialize`, so each is the token that did not
            // decode.
            CommandError::InvalidToken { .. } => ErrorCode::InvalidToken,
            // A name no parameter of the net goes by: the runtime can set
            // parameters (§6.4), the request named one that is not there.
            // `unsupported` until 2026-10-09, when the shared transcript
            // `05_nested_nets.json` pinned `invalid_params`.
            CommandError::NoSuchParamName { .. } => ErrorCode::InvalidParams,
            // A parameter type with no wire name cannot cross the wire at
            // all: this runtime cannot do it.
            CommandError::ParamNotSerializable { .. } => ErrorCode::Unsupported,
            // The value is a request parameter that did not decode, not a
            // token: `invalid_params`.
            CommandError::InvalidParam { .. } => ErrorCode::InvalidParams,
            _ => ErrorCode::Internal,
        },
        CoreError::Net(error) => match error {
            // Also a filter whose `places` names a place the net does not
            // have, from `Net::query` and `Net::query_ids` (PROTOCOL.md
            // §13.2).
            NetError::NoSuchPlace { .. } => ErrorCode::UnknownPlace,
            NetError::NoSuchTransition { .. } => ErrorCode::UnknownTransition,
            NetError::NoSuchToken { .. } => ErrorCode::UnknownToken,
            NetError::NoSuchParam { .. } => ErrorCode::Unsupported,
            _ => ErrorCode::Internal,
        },
        CoreError::Load(error) => match error {
            LoadError::InvalidParams(_) => ErrorCode::InvalidParams,
            LoadError::Build(_) | LoadError::Other(_) => ErrorCode::LoadFailed,
        },
        CoreError::Filter(error) => match error {
            // A clause with neither `values` nor `param`, a field a filter
            // does not have, a value that is not a string, an integer or a
            // boolean: the request's parameters are wrong.
            FilterError::Shape(_) => ErrorCode::InvalidParams,
            FilterError::UnknownPlace(_) => ErrorCode::UnknownPlace,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use petritype::{JsonCommand, Net, NetBuilder, PlaceId, TokenId, TransitionId};
    use serde_json::json;

    fn place(n: u32) -> PlaceId {
        serde_json::from_value(json!(n)).unwrap()
    }

    fn command(error: CommandError) -> ErrorCode {
        code_of(CoreError::Command(&error))
    }

    fn net(error: NetError) -> ErrorCode {
        code_of(CoreError::Net(&error))
    }

    fn load(error: LoadError) -> ErrorCode {
        code_of(CoreError::Load(&error))
    }

    /// A net with one serializable place, one plain place and one
    /// serializable parameter, for errors produced the honest way.
    fn sample() -> Net {
        #[derive(Clone, serde::Serialize, serde::Deserialize)]
        struct Limit(u32);
        let mut b = NetBuilder::new();
        let numbers = b.place::<u32>("Numbers");
        b.place::<String>("Notes");
        b.serializable(numbers);
        b.param(Limit(1));
        b.serializable_param::<Limit>("limit");
        b.build().unwrap()
    }

    fn apply(command: JsonCommand) -> CommandError {
        sample().apply_json(command).unwrap_err()
    }

    #[test]
    fn unknown_place_name() {
        let e = apply(JsonCommand::Extend {
            place: "Nope".into(),
            tokens: vec![json!(1)],
        });
        assert_eq!(command(e), ErrorCode::UnknownPlace);
    }

    #[test]
    fn unknown_transition_name() {
        let e = apply(JsonCommand::Enable {
            transition: "Nope".into(),
        });
        assert_eq!(command(e), ErrorCode::UnknownTransition);
    }

    #[test]
    fn not_serializable() {
        let e = apply(JsonCommand::Extend {
            place: "Notes".into(),
            tokens: vec![json!("x")],
        });
        assert_eq!(command(e), ErrorCode::NotSerializable);
    }

    #[test]
    fn invalid_token() {
        let e = apply(JsonCommand::Extend {
            place: "Numbers".into(),
            tokens: vec![json!("seven")],
        });
        assert_eq!(command(e), ErrorCode::InvalidToken);
    }

    #[test]
    fn no_such_param_name_is_invalid_params() {
        let e = apply(JsonCommand::SetParam {
            param: "speed".into(),
            value: json!(1),
        });
        assert!(matches!(e, CommandError::NoSuchParamName { .. }));
        assert_eq!(command(e), ErrorCode::InvalidParams);
    }

    /// A place of bare nets, serializable, with one template, `Box`: an
    /// `Items` place of numbers and a `size` parameter.
    fn yard() -> Net {
        #[derive(Clone, serde::Serialize, serde::Deserialize)]
        struct Size(u32);
        let mut templates = petritype::Templates::new();
        templates.register("Box", |_: &petritype::Params| {
            let mut b = NetBuilder::new();
            let items = b.place::<u32>("Items");
            b.serializable(items);
            b.param(Size(1));
            b.serializable_param::<Size>("size");
            b.build()
        });
        let mut b = NetBuilder::new();
        b.templates(&templates);
        let yard = b.place_nested::<Net>("Yard");
        b.serializable(yard);
        b.build().unwrap()
    }

    fn inject_net(saved: serde_json::Value) -> Result<(), CommandError> {
        yard().apply_json(JsonCommand::Extend {
            place: "Yard".into(),
            tokens: vec![saved],
        })
    }

    #[test]
    fn a_nested_token_the_core_refuses_is_invalid_token() {
        assert!(inject_net(json!({"template": "Box", "marking": {"Items": [1]}})).is_ok());
        for (saved, why) in [
            (
                json!({"template": "Crate", "marking": {}}),
                "an unknown template",
            ),
            (
                json!({"template": "Box", "marking": {}, "params": {"weight": 3}}),
                "a parameter the template does not declare",
            ),
            (
                json!({"template": "Box", "marking": {}, "params": {"size": "big"}}),
                "a parameter value that does not decode",
            ),
            (
                json!({"template": "Box", "marking": {"Shelf": []}}),
                "an unknown inner place",
            ),
            (
                json!({"template": "Box", "marking": {"Items": ["one"]}}),
                "an inner token that does not decode",
            ),
            (
                json!({"template": "Box", "marking": {}, "enabled": {"Pack": true}}),
                "an unknown inner transition",
            ),
        ] {
            let e = inject_net(saved).expect_err(why);
            assert!(matches!(e, CommandError::InvalidToken { .. }), "{why}: {e}");
            assert_eq!(command(e), ErrorCode::InvalidToken, "{why}");
        }
    }

    #[test]
    fn param_not_serializable_is_unsupported() {
        let e = CommandError::ParamNotSerializable { type_name: "Limit" };
        assert_eq!(command(e), ErrorCode::Unsupported);
    }

    #[test]
    fn invalid_param_value_is_invalid_params() {
        let e = apply(JsonCommand::SetParam {
            param: "limit".into(),
            value: json!("lots"),
        });
        assert_eq!(command(e), ErrorCode::InvalidParams);
    }

    #[test]
    fn a_wrapped_net_error_maps_as_the_net_error() {
        let e = apply(JsonCommand::Remove {
            place: "Numbers".into(),
            token: serde_json::from_value::<TokenId>(json!(99)).unwrap(),
        });
        assert!(matches!(e, CommandError::Net(NetError::NoSuchToken { .. })));
        assert_eq!(command(e), ErrorCode::UnknownToken);
    }

    #[test]
    fn net_no_such_place() {
        assert_eq!(
            net(NetError::NoSuchPlace { place: place(9) }),
            ErrorCode::UnknownPlace
        );
    }

    #[test]
    fn net_no_such_transition() {
        let transition: TransitionId = serde_json::from_value(json!(9)).unwrap();
        assert_eq!(
            net(NetError::NoSuchTransition { transition }),
            ErrorCode::UnknownTransition
        );
    }

    #[test]
    fn net_no_such_token() {
        let mut n = sample();
        let token = serde_json::from_value::<TokenId>(json!(5)).unwrap();
        let e = n.remove(place(0), token).unwrap_err();
        assert_eq!(net(e), ErrorCode::UnknownToken);
    }

    #[test]
    fn net_no_such_param() {
        let e = NetError::NoSuchParam { type_name: "X" };
        assert_eq!(net(e), ErrorCode::Unsupported);
    }

    #[test]
    fn anything_else_is_internal() {
        let mut n = sample();
        let unknown = serde_json::from_value::<TransitionId>(json!(0)).unwrap();
        // No transitions at all, so launching one is a NoSuchTransition; a
        // not-fireable transition is the "anything else" case.
        assert_eq!(
            net(n.launch(unknown).unwrap_err()),
            ErrorCode::UnknownTransition
        );
        let e = NetError::NoClonePlace {
            place: place(0),
            name: "Numbers".into(),
        };
        assert_eq!(net(e), ErrorCode::Internal);
        let e = CommandError::Net(NetError::NoClonePlace {
            place: place(0),
            name: "Numbers".into(),
        });
        assert_eq!(command(e), ErrorCode::Internal);
    }

    fn filter(value: serde_json::Value) -> Result<petritype::Filter, FilterError> {
        crate::translate::filter(&value, &sample())
    }

    #[test]
    fn a_filter_of_the_wrong_shape_is_invalid_params() {
        assert!(filter(json!({"clauses": [{"key": "n", "values": [1]}]})).is_ok());
        for (shape, why) in [
            (json!([1]), "not an object"),
            (
                json!({"clauses": [{"key": "n"}]}),
                "neither values nor param",
            ),
            (
                json!({"clauses": [{"key": "n", "values": [1], "param": "limit"}]}),
                "both values and param",
            ),
            (
                json!({"clauses": [{"key": "n", "values": []}]}),
                "no values",
            ),
            (
                json!({"clauses": [{"key": "n", "values": [1.5]}]}),
                "a float",
            ),
            (json!({"clauses": [{"values": [1]}]}), "no key"),
            (json!({"clauses": {"key": "n"}}), "clauses not a list"),
            (json!({"places": "Numbers"}), "places not a list"),
            (json!({"prefix": 3}), "prefix not a string"),
            (json!({"where": []}), "a field a filter has not"),
        ] {
            let error = filter(shape).expect_err(why);
            assert!(matches!(error, FilterError::Shape(_)), "{why}: {error}");
            assert_eq!(
                code_of(CoreError::Filter(&error)),
                ErrorCode::InvalidParams,
                "{why}"
            );
        }
    }

    #[test]
    fn a_filter_naming_a_place_the_net_lacks_is_unknown_place() {
        let error = filter(json!({"places": ["Numbers", "Nowhere"]})).unwrap_err();
        assert_eq!(error, FilterError::UnknownPlace("Nowhere".into()));
        assert_eq!(code_of(CoreError::Filter(&error)), ErrorCode::UnknownPlace);
        // The core's own answer to a place id it does not have, from a
        // query: the same code.
        let error = sample()
            .query_ids(&petritype::Filter::new().in_places([place(7)]))
            .unwrap_err();
        assert_eq!(net(error), ErrorCode::UnknownPlace);
    }

    #[test]
    fn load_errors() {
        assert_eq!(
            load(LoadError::InvalidParams("x".into())),
            ErrorCode::InvalidParams
        );
        assert_eq!(load(LoadError::Other("x".into())), ErrorCode::LoadFailed);
        let mut b = NetBuilder::new();
        b.place::<u32>("A");
        b.place::<u32>("A");
        let e = LoadError::from(b.build().unwrap_err());
        assert_eq!(load(e), ErrorCode::LoadFailed);
    }
}
