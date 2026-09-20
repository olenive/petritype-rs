//! External input with the types forgotten: [`Command`] and [`Net::apply`].
//!
//! The module is private and everything public in it is re-exported from the
//! crate root, so the narrative lives on [`Command`] itself rather than here.

use std::any::Any;
use std::fmt;

use crate::error::CommandError;
use crate::ids::{PlaceId, PlaceRef, TokenId, TransitionId};
use crate::net::Net;
use crate::place::erase_all;

/// The type name to blame in a [`NetError::TypeMismatch`](crate::NetError::TypeMismatch)
/// raised on this route.
///
/// An erased token knows its [`TypeId`](std::any::TypeId) — that is what the
/// check compares — but a `TypeId` cannot be turned back into a name, and the
/// value has forgotten the `T` whose `type_name` the typed methods report. A
/// literal is more honest than a guess: the place's own `expected` is the
/// useful half of the message either way.
pub(crate) const ERASED: &str = "<erased>";

/// An instruction for a net, with the token types forgotten: input that
/// crosses a thread, or comes from a source that never knew the types
/// (DESIGN §9).
///
/// In-process code does not need this. A caller holding `&mut Net` and a
/// [`PlaceRef<T>`] has [`Net::put`], [`Net::extend`], [`Net::set_tokens`],
/// [`Net::remove`] and [`Net::replace`], all of which are checked when they
/// compile. `Command` is for the input that *cannot* be: a value sent down a
/// channel from a UI thread, a line read from a file, a message from a
/// scripting layer. Such a sender has no `&mut Net` to call a method on, and
/// often no `T` either, so the instruction has to become data first and be
/// carried out later, by whoever does hold the net.
///
/// Being data is the whole of it. A `Command` owns its tokens as
/// `Box<dyn Any + Send + Sync>`, so it is `Send + Sync` and crosses a thread
/// boundary; [`Net::apply`] is where the type comes back, by comparing each
/// token's [`TypeId`](std::any::TypeId) with the one its place recorded — the
/// one runtime type check in the engine (DESIGN §9), and the reason nothing
/// else in the crate needs one.
///
/// It is not `Clone` and not `PartialEq`: an erased token can be neither
/// cloned nor compared without the vtable its place holds. Its `Debug` prints
/// how *many* tokens a batch has rather than what they are, for the same
/// reason.
///
/// The enum is `#[non_exhaustive]`: match with a `_` arm. Parameters are the
/// next slice, and will add a `SetParam` variant here.
///
/// # Typed on the way out, erased on the wire
///
/// The constructors — [`put`](Command::put), [`extend`](Command::extend),
/// [`set_tokens`](Command::set_tokens), [`replace`](Command::replace) — take
/// a `PlaceRef<T>` and `T`s, so an author on the sending side still gets the
/// compile error for putting a `String` in a place of `u32`. Only the value
/// that travels is erased. The variants are public too, for a sender that
/// genuinely has no type to offer — a script, or a decoder — and pays the
/// runtime check instead.
///
/// A `Command` carries a [`PlaceId`], never the [`NetId`](crate::NetId) stamp
/// a `PlaceRef` has. The stamp is process-local and means nothing at the far
/// end of a channel or a file, so carrying it would be a promise the wire
/// cannot keep; the `TypeId` check in `apply` is what protects the net
/// instead. The consequence is worth stating plainly: a command built for one
/// net and applied to another lands on whatever place has that index there,
/// and succeeds if the types happen to agree. Send commands to the net they
/// were made for.
///
/// # Atomic
///
/// A rejected command changes nothing and emits nothing. An
/// [`Extend`](Command::Extend) whose third token is of the wrong type adds
/// none of the three, and a [`SetTokens`](Command::SetTokens) that is
/// rejected does not clear the place on its way out: every token is checked
/// before any of them is written.
///
/// An accepted command is otherwise exactly the typed method of the same
/// name, down to the events: the typed methods and `apply` share one erased
/// insertion path, so the two cannot drift.
///
/// # Channels are the caller's concern
///
/// The crate has no inbox type and no channel of its own. A driver drains
/// whatever the caller uses from the `each_tick` hook of
/// [`run_indefinitely`](crate::drivers::run_indefinitely), which is called
/// once a tick with the same `&mut Net` the loop holds:
///
/// ```
/// # use std::sync::mpsc;
/// # use petritype::{Command, Net};
/// # fn drain(net: &mut Net, inbox: &mpsc::Receiver<Command>) {
/// for command in inbox.try_iter() {
///     if let Err(error) = net.apply(command) {
///         eprintln!("rejected: {error}");
///     }
/// }
/// # }
/// ```
///
/// # Example
///
/// ```
/// use petritype::{Command, Event, NetBuilder, TakeEdge};
///
/// let mut b = NetBuilder::new();
/// let inbox = b.place::<u32>("Inbox");
/// let out = b.place::<u32>("Out");
/// let double = b
///     .transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,))
///     .id();
/// let mut net = b.build()?;
///
/// // What the UI thread sent; what the driver's thread does with it.
/// net.apply(Command::put(inbox, 7))?;
/// assert_eq!(net.count(inbox.id()), 1);
///
/// let token = net.tokens(inbox.id()).next().expect("one token").0;
/// assert_eq!(
///     net.drain_events().collect::<Vec<_>>(),
///     [Event::Added { place: inbox.id(), token }],
/// );
///
/// // The switch travels the same way.
/// net.apply(Command::disable(double))?;
/// assert!(!net.is_enabled(double));
/// assert_eq!(net.step()?, None, "switched off");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # The JSON form
///
/// With the `serde` feature, `JsonCommand` is the same instruction with the
/// ids resolved to names and the tokens as `serde_json::Value`s — what a UI
/// that never saw the Rust types can send:
///
/// ```json
/// {"command": "extend", "place": "Inbox", "tokens": [1, 2, 3]}
/// {"command": "set_tokens", "place": "Inbox", "tokens": []}
/// {"command": "remove", "place": "Inbox", "token": 4}
/// {"command": "replace", "place": "Inbox", "token": 4, "value": 9}
/// {"command": "disable", "transition": "Double"}
/// ```
///
/// Places and transitions travel by name, because a name is what an author, a
/// saved file or a config knows, and it survives a change in declaration
/// order; tokens travel by [`TokenId`], because a token has no name and the
/// id is what the event log already gave the sender. `Command::from_json`
/// resolves the names and decodes the tokens against a net without changing
/// it, `Command::to_json` goes the other way, and `Net::apply_json` is the
/// two steps in one call. Both directions need the place to be marked
/// `NetBuilder::serializable`.
// slice 11: SetParam { value: Box<dyn Any + Send + Sync> }
#[non_exhaustive]
pub enum Command {
    /// Append tokens to a place, in order: [`Net::extend`] erased.
    Extend {
        /// The place to append to.
        place: PlaceId,
        /// The tokens, each of which must be of the place's type. Either all
        /// of them are added or none is.
        tokens: Vec<Box<dyn Any + Send + Sync>>,
    },
    /// Replace everything in a place; an empty `tokens` clears it:
    /// [`Net::set_tokens`] erased.
    SetTokens {
        /// The place to overwrite.
        place: PlaceId,
        /// The new contents, each of which must be of the place's type. The
        /// old tokens are removed only once all of these have been checked.
        tokens: Vec<Box<dyn Any + Send + Sync>>,
    },
    /// Remove one token by id: [`Net::remove`].
    Remove {
        /// The place holding it.
        place: PlaceId,
        /// The token to remove. Its id retires with it.
        token: TokenId,
    },
    /// Overwrite one token in place, keeping its id: [`Net::replace`]
    /// erased.
    Replace {
        /// The place holding it.
        place: PlaceId,
        /// The token to overwrite.
        token: TokenId,
        /// Its new value, which must be of the place's type.
        value: Box<dyn Any + Send + Sync>,
    },
    /// Switch a transition on: [`Net::enable`].
    Enable {
        /// The transition to switch on.
        transition: TransitionId,
    },
    /// Switch a transition off: [`Net::disable`].
    Disable {
        /// The transition to switch off.
        transition: TransitionId,
    },
}

impl Command {
    /// Appends tokens to a place, in iteration order.
    ///
    /// ```
    /// # use petritype::{Command, NetBuilder};
    /// # let mut b = NetBuilder::new();
    /// # let inbox = b.place::<u32>("Inbox");
    /// # let mut net = b.build()?;
    /// net.apply(Command::extend(inbox, [1, 2, 3]))?;
    /// assert_eq!(net.count(inbox.id()), 3);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn extend<T: Send + Sync + 'static>(
        place: PlaceRef<T>,
        tokens: impl IntoIterator<Item = T>,
    ) -> Command {
        Command::Extend {
            place: place.id(),
            tokens: erase_all(tokens),
        }
    }

    /// Appends one token to a place. Sugar for an [`extend`](Command::extend)
    /// of one, as [`Net::put`] is for [`Net::extend`].
    pub fn put<T: Send + Sync + 'static>(place: PlaceRef<T>, token: T) -> Command {
        Command::extend(place, [token])
    }

    /// Replaces everything in a place. An empty iterator clears it.
    pub fn set_tokens<T: Send + Sync + 'static>(
        place: PlaceRef<T>,
        tokens: impl IntoIterator<Item = T>,
    ) -> Command {
        Command::SetTokens {
            place: place.id(),
            tokens: erase_all(tokens),
        }
    }

    /// Removes one token by id.
    ///
    /// The only constructor that takes a [`PlaceId`] rather than a
    /// `PlaceRef<T>`, because removal does not care what the place holds —
    /// exactly as [`Net::remove`] does not.
    pub fn remove(place: PlaceId, token: TokenId) -> Command {
        Command::Remove { place, token }
    }

    /// Overwrites one token, keeping its id and its place in the queue.
    pub fn replace<T: Send + Sync + 'static>(
        place: PlaceRef<T>,
        token: TokenId,
        value: T,
    ) -> Command {
        Command::Replace {
            place: place.id(),
            token,
            value: Box::new(value),
        }
    }

    /// Switches a transition on.
    pub fn enable(transition: TransitionId) -> Command {
        Command::Enable { transition }
    }

    /// Switches a transition off.
    pub fn disable(transition: TransitionId) -> Command {
        Command::Disable { transition }
    }
}

impl fmt::Debug for Command {
    /// Prints the variant, its ids, and how many tokens a batch carries.
    ///
    /// An erased token has no `Debug` of its own — the formatter for it lives
    /// in the place it is destined for, which a `Command` does not have a
    /// handle on. The count is the part a reader of a log or a channel dump
    /// actually needs; [`Net::label`] renders a token once it has landed.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Command::Extend { place, tokens } => f
                .debug_struct("Extend")
                .field("place", place)
                .field("tokens", &tokens.len())
                .finish(),
            Command::SetTokens { place, tokens } => f
                .debug_struct("SetTokens")
                .field("place", place)
                .field("tokens", &tokens.len())
                .finish(),
            Command::Remove { place, token } => f
                .debug_struct("Remove")
                .field("place", place)
                .field("token", token)
                .finish(),
            Command::Replace { place, token, .. } => f
                .debug_struct("Replace")
                .field("place", place)
                .field("token", token)
                .finish_non_exhaustive(),
            Command::Enable { transition } => f
                .debug_struct("Enable")
                .field("transition", transition)
                .finish(),
            Command::Disable { transition } => f
                .debug_struct("Disable")
                .field("transition", transition)
                .finish(),
        }
    }
}

impl Net {
    /// Carries out one erased [`Command`]: the far end of a channel from a UI
    /// thread, or of a decoder (DESIGN §9).
    ///
    /// Each variant is the typed method of the same name with the type
    /// checked at run time instead of at compile time, and emits exactly what
    /// that method emits: an [`Added`](crate::Event::Added) per token for
    /// [`Extend`](Command::Extend), a [`Removed`](crate::Event::Removed) per
    /// old token and then an `Added` per new one for
    /// [`SetTokens`](Command::SetTokens), a `Removed` for
    /// [`Remove`](Command::Remove), a [`Mutated`](crate::Event::Mutated) for
    /// [`Replace`](Command::Replace), and a
    /// [`Toggled`](crate::Event::Toggled) for an
    /// [`Enable`](Command::Enable) or [`Disable`](Command::Disable) that
    /// actually moves the switch. [`state_counter`](Net::state_counter)
    /// therefore moves by the number of events, and by nothing else: `apply`
    /// has no tick of its own.
    ///
    /// Commands may be applied whenever the caller holds `&mut Net`,
    /// including while firings are in flight.
    ///
    /// # Errors
    ///
    /// [`CommandError::Net`], wrapping
    /// [`NoSuchPlace`](crate::NetError::NoSuchPlace),
    /// [`NoSuchTransition`](crate::NetError::NoSuchTransition),
    /// [`NoSuchToken`](crate::NetError::NoSuchToken) or
    /// [`TypeMismatch`](crate::NetError::TypeMismatch). **The net is
    /// untouched and silent in every case**: a batch is checked token by
    /// token before any of it is written, so a rejected command is one an
    /// observer never hears about.
    ///
    /// ```
    /// # use petritype::{Command, CommandError, NetBuilder, NetError};
    /// # use std::any::Any;
    /// # let mut b = NetBuilder::new();
    /// # let inbox = b.place::<u32>("Inbox");
    /// # let mut net = b.build()?;
    /// // A batch from an untyped sender, the second token of it wrong.
    /// let tokens: Vec<Box<dyn Any + Send + Sync>> = vec![Box::new(1_u32), Box::new("two")];
    /// let rejected = net.apply(Command::Extend { place: inbox.id(), tokens });
    ///
    /// assert!(matches!(rejected, Err(CommandError::Net(NetError::TypeMismatch { .. }))));
    /// assert_eq!(net.count(inbox.id()), 0, "not even the first token");
    /// assert_eq!(net.state_counter(), 0);
    /// assert_eq!(net.drain_events().count(), 0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn apply(&mut self, command: Command) -> Result<(), CommandError> {
        match command {
            Command::Extend { place, tokens } => {
                self.extend_erased(place, tokens, ERASED)?;
            }
            Command::SetTokens { place, tokens } => {
                self.set_erased(place, tokens, ERASED)?;
            }
            Command::Remove { place, token } => self.remove(place, token)?,
            Command::Replace {
                place,
                token,
                value,
            } => self.replace_erased(place, token, value, ERASED)?,
            Command::Enable { transition } => self.set_enabled(transition, true)?,
            Command::Disable { transition } => self.set_enabled(transition, false)?,
        }
        Ok(())
    }
}

/// One instruction in the form a wire, a file or an untyped UI can carry:
/// names instead of ids, and `serde_json::Value`s instead of tokens.
///
/// The shape is documented, and tested, under
/// [The JSON form](Command#the-json-form): an internally tagged object whose
/// `"command"` field is the variant in `snake_case`. It is ordinary data —
/// `Clone`, `PartialEq`, `Serialize`,
/// `Deserialize` — because nothing in it is erased yet;
/// [`Command::from_json`] is what turns it into the erased form a net can
/// apply, and the place it names must be marked
/// [`serializable`](crate::NetBuilder::serializable) for that to be possible.
#[cfg(feature = "serde")]
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
#[non_exhaustive]
pub enum JsonCommand {
    /// Append tokens to a place: [`Command::Extend`].
    Extend {
        /// The place's name, as [`Net::place_named`] resolves it.
        place: String,
        /// The tokens, each parsed as the place's type.
        tokens: Vec<serde_json::Value>,
    },
    /// Replace everything in a place; `[]` clears it:
    /// [`Command::SetTokens`].
    SetTokens {
        /// The place's name.
        place: String,
        /// The new contents, each parsed as the place's type.
        tokens: Vec<serde_json::Value>,
    },
    /// Remove one token by id: [`Command::Remove`].
    Remove {
        /// The place's name.
        place: String,
        /// The token's id, a bare number, as the event log reported it.
        token: TokenId,
    },
    /// Overwrite one token, keeping its id: [`Command::Replace`].
    Replace {
        /// The place's name.
        place: String,
        /// The token's id.
        token: TokenId,
        /// Its new value, parsed as the place's type.
        value: serde_json::Value,
    },
    /// Switch a transition on: [`Command::Enable`].
    Enable {
        /// The transition's name, as [`Net::transition_named`] resolves it.
        transition: String,
    },
    /// Switch a transition off: [`Command::Disable`].
    Disable {
        /// The transition's name.
        transition: String,
    },
}

#[cfg(feature = "serde")]
impl Command {
    /// Resolves a [`JsonCommand`]'s names and decodes its tokens against
    /// `net`, producing the erased command [`apply`](Net::apply) takes.
    ///
    /// The net is read, never changed: this is the decode half of
    /// [`apply_json`](Net::apply_json), separate so that a caller can decode
    /// a file of commands once and apply them later, or report a bad one
    /// without having touched the net at all.
    ///
    /// # Errors
    ///
    /// [`CommandError::NoSuchPlaceName`] or
    /// [`CommandError::NoSuchTransitionName`] for a name the net does not
    /// have, [`CommandError::NotSerializable`] for a place with no decoder
    /// recorded, and [`CommandError::InvalidToken`] for a value that does not
    /// parse as the place's type — the first one that fails, with nothing
    /// half-decoded kept.
    pub fn from_json(net: &Net, command: JsonCommand) -> Result<Command, CommandError> {
        match command {
            JsonCommand::Extend { place, tokens } => {
                let place = place_id(net, &place)?;
                Ok(Command::Extend {
                    place,
                    tokens: decode_all(net, place, tokens)?,
                })
            }
            JsonCommand::SetTokens { place, tokens } => {
                let place = place_id(net, &place)?;
                Ok(Command::SetTokens {
                    place,
                    tokens: decode_all(net, place, tokens)?,
                })
            }
            JsonCommand::Remove { place, token } => Ok(Command::Remove {
                place: place_id(net, &place)?,
                token,
            }),
            JsonCommand::Replace {
                place,
                token,
                value,
            } => {
                let place = place_id(net, &place)?;
                let mut decoded = decode_all(net, place, vec![value])?;
                Ok(Command::Replace {
                    place,
                    token,
                    value: decoded.pop().expect("one value in, one value out"),
                })
            }
            JsonCommand::Enable { transition } => Ok(Command::Enable {
                transition: transition_id(net, &transition)?,
            }),
            JsonCommand::Disable { transition } => Ok(Command::Disable {
                transition: transition_id(net, &transition)?,
            }),
        }
    }

    /// This command in the form a wire carries, with the ids `net` gave it
    /// turned back into names and its tokens encoded by their places.
    ///
    /// The net is read, never changed. The inverse of
    /// [`from_json`](Command::from_json), for a caller that builds commands
    /// typed and sends them somewhere that has no types — a log, a replay
    /// file, a browser.
    ///
    /// # Errors
    ///
    /// [`CommandError::NotSerializable`] for a place with no encoder
    /// recorded; [`CommandError::Net`] wrapping
    /// [`NoSuchPlace`](crate::NetError::NoSuchPlace) or
    /// [`NoSuchTransition`](crate::NetError::NoSuchTransition) for an id this
    /// net does not have — which is what a command built for another net
    /// usually looks like from here. A token whose type is not the place's is
    /// [`TypeMismatch`](crate::NetError::TypeMismatch) rather than a `null`
    /// in the output: the place's encoder would quietly produce one, and a
    /// silently wrong wire message is worse than a rejected one.
    pub fn to_json(&self, net: &Net) -> Result<JsonCommand, CommandError> {
        match self {
            Command::Extend { place, tokens } => Ok(JsonCommand::Extend {
                place: place_name(net, *place)?,
                tokens: encode_all(net, *place, tokens)?,
            }),
            Command::SetTokens { place, tokens } => Ok(JsonCommand::SetTokens {
                place: place_name(net, *place)?,
                tokens: encode_all(net, *place, tokens)?,
            }),
            Command::Remove { place, token } => Ok(JsonCommand::Remove {
                place: place_name(net, *place)?,
                token: *token,
            }),
            Command::Replace {
                place,
                token,
                value,
            } => Ok(JsonCommand::Replace {
                place: place_name(net, *place)?,
                token: *token,
                value: encode_all(net, *place, std::slice::from_ref(value))?
                    .pop()
                    .expect("one value in, one value out"),
            }),
            Command::Enable { transition } => Ok(JsonCommand::Enable {
                transition: transition_name(net, *transition)?,
            }),
            Command::Disable { transition } => Ok(JsonCommand::Disable {
                transition: transition_name(net, *transition)?,
            }),
        }
    }
}

#[cfg(feature = "serde")]
impl Net {
    /// [`Command::from_json`] and then [`apply`](Net::apply): one JSON
    /// instruction, carried out.
    ///
    /// Decoding happens first and in full, so a command rejected for a bad
    /// name or an unparsable token leaves the net exactly as `apply` would
    /// have: untouched and silent.
    ///
    /// # Errors
    ///
    /// Whatever [`Command::from_json`] or [`apply`](Net::apply) returns.
    pub fn apply_json(&mut self, command: JsonCommand) -> Result<(), CommandError> {
        let command = Command::from_json(self, command)?;
        self.apply(command)
    }
}

/// The id of a named place, or [`CommandError::NoSuchPlaceName`].
#[cfg(feature = "serde")]
fn place_id(net: &Net, name: &str) -> Result<PlaceId, CommandError> {
    net.place_named(name)
        .ok_or_else(|| CommandError::NoSuchPlaceName {
            name: name.to_owned(),
        })
}

/// The id of a named transition, or [`CommandError::NoSuchTransitionName`].
#[cfg(feature = "serde")]
fn transition_id(net: &Net, name: &str) -> Result<TransitionId, CommandError> {
    net.transition_named(name)
        .ok_or_else(|| CommandError::NoSuchTransitionName {
            name: name.to_owned(),
        })
}

/// The name of a place, or [`NetError::NoSuchPlace`](crate::NetError::NoSuchPlace).
#[cfg(feature = "serde")]
fn place_name(net: &Net, place: PlaceId) -> Result<String, CommandError> {
    net.places
        .get(place.0 as usize)
        .map(|slot| slot.meta.name.clone())
        .ok_or_else(|| CommandError::Net(crate::NetError::NoSuchPlace { place }))
}

/// The name of a transition, or
/// [`NetError::NoSuchTransition`](crate::NetError::NoSuchTransition).
#[cfg(feature = "serde")]
fn transition_name(net: &Net, transition: TransitionId) -> Result<String, CommandError> {
    net.transitions
        .get(transition.0 as usize)
        .map(|slot| slot.name.clone())
        .ok_or_else(|| CommandError::Net(crate::NetError::NoSuchTransition { transition }))
}

/// Parses a batch of wire values as the place's type, with the place's
/// recorded `from_json`.
///
/// `place` has already been resolved from a name, so it indexes this net.
#[cfg(feature = "serde")]
fn decode_all(
    net: &Net,
    place: PlaceId,
    values: Vec<serde_json::Value>,
) -> Result<Vec<Box<dyn Any + Send + Sync>>, CommandError> {
    let slot = &net.places[place.0 as usize];
    let from_json = slot
        .meta
        .from_json
        .ok_or(CommandError::NotSerializable { place })?;
    values
        .into_iter()
        .map(|value| {
            from_json(value).map_err(|source| CommandError::InvalidToken {
                place,
                expected: slot.meta.type_name,
                source,
            })
        })
        .collect()
}

/// Renders a batch of erased tokens as the place's type would, with the
/// place's recorded `to_json`.
///
/// The [`TypeId`](std::any::TypeId) check is this function's own: `to_json`
/// answers a failed downcast with `Null`, which on a wire is indistinguishable
/// from a token that really is null.
#[cfg(feature = "serde")]
fn encode_all(
    net: &Net,
    place: PlaceId,
    tokens: &[Box<dyn Any + Send + Sync>],
) -> Result<Vec<serde_json::Value>, CommandError> {
    let slot = net
        .places
        .get(place.0 as usize)
        .ok_or(crate::NetError::NoSuchPlace { place })?;
    let to_json = slot
        .meta
        .to_json
        .ok_or(CommandError::NotSerializable { place })?;
    tokens
        .iter()
        .map(|token| {
            if (**token).type_id() == slot.meta.type_id {
                Ok(to_json(&**token))
            } else {
                Err(CommandError::from(crate::NetError::TypeMismatch {
                    place,
                    expected: slot.meta.type_name,
                    found: ERASED,
                }))
            }
        })
        .collect()
}
