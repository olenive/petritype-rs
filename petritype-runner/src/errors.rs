//! The core's errors mapped onto the protocol's closed set of codes
//! (DESIGN §19.3.7): one function, a test per arm.

use petritype::{CommandError, NetError};

use crate::protocol::ErrorCode;
use crate::registry::LoadError;

/// An error from the core or a factory, borrowed, for [`code_of`].
#[derive(Debug, Clone, Copy)]
pub(crate) enum CoreError<'a> {
    /// From `apply_json` and friends.
    Command(&'a CommandError),
    /// From the net's own methods.
    Net(&'a NetError),
    /// From a factory.
    Load(&'a LoadError),
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
            CommandError::InvalidToken { .. } => ErrorCode::InvalidToken,
            // A parameter the net registered no wire name for is one this
            // runtime cannot set: the protocol's `unsupported`.
            CommandError::NoSuchParamName { .. } | CommandError::ParamNotSerializable { .. } => {
                ErrorCode::Unsupported
            }
            // The value is a request parameter that did not decode, not a
            // token: `invalid_params`.
            CommandError::InvalidParam { .. } => ErrorCode::InvalidParams,
            _ => ErrorCode::Internal,
        },
        CoreError::Net(error) => match error {
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
    fn no_such_param_name_is_unsupported() {
        let e = apply(JsonCommand::SetParam {
            param: "speed".into(),
            value: json!(1),
        });
        assert_eq!(command(e), ErrorCode::Unsupported);
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
