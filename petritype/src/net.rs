//! The net itself: token storage, the read API and external input.
//!
//! Firing is the other half of `Net`, and lives in [`crate::firing`]: which
//! transitions could fire, which one does, and what one firing changes.

use std::any::{Any, TypeId, type_name};
use std::fmt;

use crate::command::ERASED;
use crate::edge::EdgeInfo;
use crate::error::NetError;
use crate::event::Event;
use crate::firing::Selector;
use crate::ids::{NetId, PlaceId, PlaceRef, TokenId, TransitionId, foreign_place};
use crate::param::{ParamSlot, param_of, param_slot_index};
use crate::place::{ErasedToken, PlaceSlot, erase_all};
use crate::transition::{TransitionInfo, TransitionSlot};

/// A built net: places, their tokens, and the log of what has changed.
///
/// A net is created by [`NetBuilder::build`](crate::NetBuilder::build) and
/// changed only through `&mut self` methods, so there is exactly one writer at
/// a time. An observer holds the same `&mut Net` between steps: it reads the
/// marking with [`places`](Net::places) / [`tokens`](Net::tokens) /
/// [`label`](Net::label) and follows changes with
/// [`drain_events`](Net::drain_events).
pub struct Net {
    /// Stamped into every [`PlaceRef`] this net's builder handed out, and
    /// compared before any of them is used as an index.
    pub(crate) id: NetId,
    /// Indexed by [`PlaceId`]; ids are never reused, so this vector only grows.
    pub(crate) places: Vec<PlaceSlot>,
    /// Indexed by [`TransitionId`], in declaration order.
    pub(crate) transitions: Vec<TransitionSlot>,
    /// The parameter values, one per type, in the order the builder first
    /// heard of each type (DESIGN §10).
    ///
    /// A short linear list keyed by [`TypeId`] rather than a map: a net has a
    /// handful of parameters at most, and a scan over that many `TypeId`s
    /// beats hashing one. It never grows or shrinks after
    /// [`build`](crate::NetBuilder::build) — only the values are replaced,
    /// by [`set_param`](Net::set_param).
    pub(crate) params: Vec<ParamSlot>,
    /// The id the next token will get. Monotonic for the life of the net.
    pub(crate) next_token_id: u64,
    /// Bumped by connectivity changes only. Nothing in this version bumps it.
    pub(crate) topology_counter: u64,
    /// Bumped once per state-changing event.
    pub(crate) state_counter: u64,
    /// Completed firings. A statistic, not a change counter.
    pub(crate) fired_count: u64,
    /// How [`step`](Net::step) chooses among the fireable transitions, or
    /// `None` for the default (highest priority, ties to the transition
    /// declared first). Set by [`set_selector`](Net::set_selector).
    pub(crate) selector: Option<Selector>,
    /// Maximum number of characters in a [`label`](Net::label).
    pub(crate) label_limit: usize,
    /// Appended to by the net, drained by the driver.
    pub(crate) events: Vec<Event>,
}

/// The stand-in name for an id that indexes nothing, in the message of an
/// error that carries both a name and an id.
///
/// Unreachable in practice — an id only reaches an error message after the
/// net has established that it is one of its own — and written like the
/// engine's other stand-ins (`"<erased>"` for a type a `dyn Any` cannot name)
/// so that a reader can tell at a glance that it is not a name an author
/// wrote.
pub(crate) const UNKNOWN_NAME: &str = "<unknown>";

/// A view of one place, for building and refreshing a diagram.
///
/// Returned by value from [`Net::places`]: the net stores the pieces (a name,
/// a type name, a queue) and this struct is assembled on demand rather than
/// kept alongside them, so the count can never go stale. The name is borrowed
/// from the net, so nothing is allocated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct PlaceInfo<'a> {
    /// The place's name, unique within the net.
    pub name: &'a str,
    /// The Rust type of the tokens this place holds,
    /// `std::any::type_name::<T>()`.
    pub type_name: &'static str,
    /// How many tokens the place holds right now.
    pub count: usize,
}

impl Net {
    // ----- read API -------------------------------------------------------

    /// This net's id, the one every [`PlaceRef`] it handed out carries.
    ///
    /// Two nets built from two builders never share an id, so a handle can
    /// always say which net it means — which is what makes composition, where
    /// an inner net is the body of an outer net's transition, safe to write.
    pub fn id(&self) -> NetId {
        self.id
    }

    /// Every place, in id order, with its name, token type and token count.
    ///
    /// ```
    /// # let mut b = petritype::NetBuilder::new();
    /// # let inbox = b.place::<u32>("Inbox");
    /// # b.seed(inbox, [1, 2]);
    /// # let net = b.build().unwrap();
    /// let names: Vec<_> = net.places().map(|(_, info)| info.name).collect();
    /// assert_eq!(names, ["Inbox"]);
    /// ```
    pub fn places(&self) -> impl Iterator<Item = (PlaceId, PlaceInfo<'_>)> {
        places_of(&self.places)
    }

    /// Every transition, in declaration order, with its name, its input
    /// edges and its output places.
    ///
    /// ```
    /// # use petritype::{EdgeKind, NetBuilder, TakeEdge};
    /// # let mut b = NetBuilder::new();
    /// # let inbox = b.place::<u32>("Inbox");
    /// # let out = b.place::<u32>("Out");
    /// # b.transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,));
    /// # let net = b.build().unwrap();
    /// let (id, info) = net.transitions().next().unwrap();
    /// assert_eq!(info.name, "Double");
    /// assert_eq!(info.inputs, [(inbox.id(), EdgeKind::Take)]);
    /// assert!(info.params.is_empty(), "no parameters declared");
    /// assert_eq!(info.outputs, [out.id()]);
    /// assert!(info.enabled, "the switch is on");
    /// assert!(!info.in_flight, "nothing has been launched");
    /// assert!(info.detachable, "no mutating edge, so it may run off the net");
    /// assert_eq!(net.transition_named("Double"), Some(id));
    /// ```
    pub fn transitions(&self) -> impl Iterator<Item = (TransitionId, TransitionInfo<'_>)> {
        self.transitions.iter().enumerate().map(|(index, slot)| {
            // The index is where the id came from in the first place.
            let id = TransitionId(index as u32);
            let info = TransitionInfo {
                name: &slot.name,
                inputs: &slot.inputs,
                params: &slot.params,
                outputs: &slot.outputs,
                enabled: slot.enabled,
                // An empty body slot is the in-flight mark: there is no
                // register of airborne firings to consult.
                in_flight: slot.body.is_none(),
                detachable: slot.detachable,
            };
            (id, info)
        })
    }

    /// Every edge of the net, as a diagram draws it: transition by
    /// transition in declaration order, each one's input edges in
    /// declaration order and then its output edges.
    ///
    /// The same wiring [`transitions`](Net::transitions) reports, one edge at
    /// a time and with both endpoints named, which is the form a layout or a
    /// DOT file is built from. It is topology, so it only changes when
    /// [`topology_counter`](Net::topology_counter) does — never in this
    /// version, where a net is wired once and then run.
    ///
    /// ```
    /// # use petritype::{EdgeInfo, EdgeKind, NetBuilder, SnapshotEdge, TakeEdge};
    /// # #[derive(Debug, Clone)] struct Parcel { id: u32, weight: f32 }
    /// # #[derive(Debug, Clone)] struct Config { max_weight: f32 }
    /// # let mut b = NetBuilder::new();
    /// let inbox = b.place::<Parcel>("Inbox");
    /// let heavy = b.place::<Parcel>("Heavy");
    /// let config = b.place::<Config>("Config");
    /// let weigh = b
    ///     .transition(
    ///         "Weigh",
    ///         (TakeEdge(inbox), SnapshotEdge(config)),
    ///         (heavy,),
    ///         |parcel: Parcel, cfg: Config| (Some(parcel).filter(|p| p.weight > cfg.max_weight),),
    ///     )
    ///     .id();
    /// let net = b.build()?;
    ///
    /// assert_eq!(
    ///     net.edges().collect::<Vec<_>>(),
    ///     [
    ///         EdgeInfo::Input { place: inbox.id(), transition: weigh, kind: EdgeKind::Take },
    ///         EdgeInfo::Input { place: config.id(), transition: weigh, kind: EdgeKind::Snapshot },
    ///         EdgeInfo::Output { transition: weigh, place: heavy.id(), slot: 0 },
    ///     ],
    /// );
    /// # Ok::<(), petritype::BuildError>(())
    /// ```
    ///
    /// A place nothing reads or writes appears in no edge; it is still a
    /// node, and [`places`](Net::places) is where a diagram gets its nodes.
    pub fn edges(&self) -> impl Iterator<Item = EdgeInfo> {
        self.transitions
            .iter()
            .enumerate()
            .flat_map(|(index, slot)| {
                // The index is where the id came from in the first place.
                let transition = TransitionId(index as u32);
                let inputs = slot
                    .inputs
                    .iter()
                    .map(move |&(place, kind)| EdgeInfo::Input {
                        place,
                        transition,
                        kind,
                    });
                let outputs =
                    slot.outputs
                        .iter()
                        .enumerate()
                        .map(move |(slot, &place)| EdgeInfo::Output {
                            transition,
                            place,
                            slot,
                        });
                inputs.chain(outputs)
            })
    }

    /// The id of the transition with this name, if the net has one. Names are
    /// unique: [`build`](crate::NetBuilder::build) rejects duplicates.
    pub fn transition_named(&self, name: &str) -> Option<TransitionId> {
        self.transitions
            .iter()
            .position(|slot| slot.name == name)
            .map(|index| TransitionId(index as u32))
    }

    /// The id of the place with this name, if the net has one. Names are
    /// unique: [`build`](crate::NetBuilder::build) rejects duplicates.
    pub fn place_named(&self, name: &str) -> Option<PlaceId> {
        place_named_in(&self.places, name)
    }

    /// How many tokens a place holds. An unknown place counts zero, so this is
    /// usable straight inside a guard.
    pub fn count(&self, place: PlaceId) -> usize {
        count_in(&self.places, place)
    }

    /// The tokens of a place, oldest first — the order in which they will be
    /// consumed.
    ///
    /// The values are erased; use [`token`](Net::token) when the concrete type
    /// is known, [`label`](Net::label) for a rendering, or downcast the
    /// `&dyn Any` yourself. An unknown place yields nothing.
    pub fn tokens(&self, place: PlaceId) -> impl Iterator<Item = (TokenId, &dyn Any)> {
        tokens_in(&self.places, place)
    }

    /// One token of a place, downcast to `T`.
    ///
    /// `None` if the place or the token does not exist, or if the place holds
    /// some other type — the typed API cannot produce that last case, but a
    /// widget that was told a type by a human can ask for the wrong one.
    pub fn token<T: 'static>(&self, place: PlaceId, token: TokenId) -> Option<&T> {
        token_in::<T>(&self.places, place, token)
    }

    /// A token rendered with its type's `Debug`, for a diagram.
    ///
    /// Truncated to [`label_limit`](Net::label_limit) characters, on a
    /// character boundary, with `"..."` appended when anything was cut.
    /// `None` if the place or the token does not exist.
    pub fn label(&self, place: PlaceId, token: TokenId) -> Option<String> {
        label_in(&self.places, place, token, self.label_limit)
    }

    /// A token rendered as JSON, for an inspector that wants structure rather
    /// than a `Debug` string.
    ///
    /// `Some` only for a place marked
    /// [`serializable`](crate::NetBuilder::serializable); an ordinary place
    /// has no serialiser recorded and returns `None`, as does an unknown
    /// place or token.
    #[cfg(feature = "serde")]
    pub fn json(&self, place: PlaceId, token: TokenId) -> Option<serde_json::Value> {
        json_in(&self.places, place, token)
    }

    /// The current value of the parameter of type `T`, or `None` if this net
    /// has no parameter of that type (DESIGN §10).
    ///
    /// The borrow is the net's own value, not a clone: a body is handed a
    /// copy because it may run elsewhere, but a reader holding `&Net` can
    /// simply look. That is what makes a parameter usable from a
    /// [guard](crate::TransitionBuilder::guard), which is asked with the
    /// whole net in hand:
    ///
    /// ```
    /// # use petritype::{NetBuilder, TakeEdge};
    /// #[derive(Clone)]
    /// struct Paused(bool);
    ///
    /// let mut b = NetBuilder::new();
    /// let inbox = b.place::<u32>("Inbox");
    /// let out = b.place::<u32>("Out");
    /// b.seed(inbox, [1]);
    /// b.param(Paused(true));
    /// let double = b
    ///     .transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,))
    ///     .guard(|net| !net.param::<Paused>().is_some_and(|p| p.0))
    ///     .id();
    /// let mut net = b.build()?;
    ///
    /// assert!(!net.is_fireable(double), "paused");
    /// net.set_param(Paused(false))?;
    /// assert!(net.is_fireable(double));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn param<T: 'static>(&self) -> Option<&T> {
        param_of::<T>(&self.params)
    }

    /// Increments when the net's connectivity changes. Nothing in this
    /// version changes connectivity after `build`, so it stays at zero; a
    /// view that lays out nodes and edges can already watch it.
    pub fn topology_counter(&self) -> u64 {
        self.topology_counter
    }

    /// A change tick for observers: it increments once per state-changing
    /// event emitted after the net was built. A view that has seen counter
    /// *n* and sees *n* again needs no refresh.
    ///
    /// A freshly built net is at zero however it was seeded — the initial
    /// marking is setup, not a change. It is also not a firing count; that is
    /// [`fired_count`](Net::fired_count).
    pub fn state_counter(&self) -> u64 {
        self.state_counter
    }

    /// How many firings have completed — the statistic
    /// [`state_counter`](Net::state_counter) is deliberately not. Always zero
    /// until transitions arrive.
    pub fn fired_count(&self) -> u64 {
        self.fired_count
    }

    /// The maximum number of characters [`label`](Net::label) returns before
    /// truncating, as set by
    /// [`NetBuilder::label_limit`](crate::NetBuilder::label_limit).
    pub fn label_limit(&self) -> usize {
        self.label_limit
    }

    // ----- events ---------------------------------------------------------

    /// Takes the events logged since the last drain, in order.
    ///
    /// The log is unbounded until drained, so a long-running driver should
    /// drain every step even if it ignores what it gets.
    ///
    /// ```
    /// # use petritype::{Event, NetBuilder};
    /// # let mut b = NetBuilder::new();
    /// # let inbox = b.place::<u32>("Inbox");
    /// # let mut net = b.build().unwrap();
    /// let token = net.put(inbox, 7);
    /// let events: Vec<Event> = net.drain_events().collect();
    /// assert_eq!(events, [Event::Added { place: inbox.id(), token }]);
    /// assert_eq!(net.drain_events().count(), 0);
    /// ```
    pub fn drain_events(&mut self) -> std::vec::Drain<'_, Event> {
        self.events.drain(..)
    }

    // ----- external input -------------------------------------------------

    /// Appends tokens to a place, in iteration order, and returns their new
    /// ids.
    ///
    /// Emits one [`Added`](Event::Added) per token and bumps
    /// [`state_counter`](Net::state_counter) once per event.
    ///
    /// The erased [`Command::Extend`](crate::Command::Extend) is the same
    /// call with the type forgotten, and goes through the same internal
    /// `extend_erased` as this one, so the two cannot drift.
    ///
    /// # Panics
    ///
    /// If `place` does not belong to this net — a `PlaceRef` is only valid
    /// for the net built from the builder that handed it out.
    pub fn extend<T, I>(&mut self, place: PlaceRef<T>, tokens: I) -> Vec<TokenId>
    where
        T: Send + Sync + 'static,
        I: IntoIterator<Item = T>,
    {
        self.checked_index(place, "extend");
        self.extend_erased(place.id(), erase_all(tokens), type_name::<T>())
            .expect("a stamped handle indexes a place of its own type")
    }

    /// Appends one token to a place and returns its new id. Sugar for an
    /// [`extend`](Net::extend) of one.
    ///
    /// # Panics
    ///
    /// If `place` does not belong to this net.
    pub fn put<T: Send + Sync + 'static>(&mut self, place: PlaceRef<T>, token: T) -> TokenId {
        self.push_typed(place, token, "put")
    }

    /// Replaces everything in a place. An empty iterator clears it.
    ///
    /// Emits a [`Removed`](Event::Removed) for every token that was there,
    /// then an [`Added`](Event::Added) for every new one: the old tokens are
    /// gone and their ids retire, the new ones are new tokens with new ids,
    /// and a view replaying the log ends up in the same state as the net.
    ///
    /// The erased [`Command::SetTokens`](crate::Command::SetTokens) is the
    /// same call with the type forgotten, and goes through the same internal
    /// `set_erased` as this one, so the two cannot drift.
    ///
    /// # Panics
    ///
    /// If `place` does not belong to this net.
    pub fn set_tokens<T, I>(&mut self, place: PlaceRef<T>, tokens: I)
    where
        T: Send + Sync + 'static,
        I: IntoIterator<Item = T>,
    {
        // Checked before the clear, not by the insertion that follows it: a
        // foreign handle must not empty a place of this net on its way to
        // the panic.
        self.checked_index(place, "set_tokens");
        self.set_erased(place.id(), erase_all(tokens), type_name::<T>())
            .expect("a stamped handle indexes a place of its own type");
    }

    /// Removes one token by id.
    ///
    /// Emits [`Removed`](Event::Removed). The id retires with the token and is
    /// never handed out again. Takes a [`PlaceId`] rather than a `PlaceRef`
    /// because removal does not care about the type, and an inspector usually
    /// only has the erased id.
    ///
    /// # Errors
    ///
    /// [`NetError::NoSuchPlace`] or [`NetError::NoSuchToken`]; the net is
    /// unchanged in both cases.
    pub fn remove(&mut self, place: PlaceId, token: TokenId) -> Result<(), NetError> {
        {
            let slot = self
                .places
                .get_mut(place.0 as usize)
                .ok_or(NetError::NoSuchPlace { place })?;
            let index = slot.position(token).ok_or_else(|| NetError::NoSuchToken {
                place,
                name: slot.meta.name.clone(),
                token,
            })?;
            slot.tokens.remove(index);
        }
        self.emit(Event::Removed { place, token });
        Ok(())
    }

    /// Overwrites one token in place, keeping its id and its position in the
    /// queue.
    ///
    /// Emits [`Mutated`](Event::Mutated). This is the edit an inspector makes:
    /// the token is the same token as far as any view is concerned, so a view
    /// refreshes its label instead of destroying and recreating it.
    ///
    /// # Errors
    ///
    /// [`NetError::NoSuchPlace`] if the handle is from another net, or
    /// [`NetError::NoSuchToken`]; the net is unchanged in both cases. Unlike
    /// the infallible half of this API, `replace` has somewhere to report a
    /// foreign handle, so it does. ([`NetError::TypeMismatch`] is kept as a
    /// defensive check: a stamped handle always indexes a place of its own
    /// type, and the erased command route is where a type can really be
    /// wrong.)
    pub fn replace<T: Send + Sync + 'static>(
        &mut self,
        place: PlaceRef<T>,
        token: TokenId,
        value: T,
    ) -> Result<(), NetError> {
        if place.net() != self.id {
            return Err(NetError::NoSuchPlace { place: place.id() });
        }
        self.replace_erased(place.id(), token, Box::new(value), type_name::<T>())
    }

    /// Replaces the value of the parameter of type `T` (DESIGN §10).
    ///
    /// Emits [`ParamChanged`](Event::ParamChanged) and so bumps
    /// [`state_counter`](Net::state_counter) once. The type name is all the
    /// event carries: a parameter has no `Debug` bound, deliberately, so that
    /// a secret cannot leak into a log or a label.
    ///
    /// **Atomic between steps.** A firing that has already been
    /// [launched](Net::launch) keeps the clone it was given at launch, so a
    /// body in flight is never handed a value that changes under it; the next
    /// launch sees the new one. Parameters do not affect
    /// [fireability](Net::fireable) — every parameter has a value from the
    /// moment the net is built — so replacing one enables nothing and
    /// disables nothing by itself, unless a guard reads it.
    ///
    /// ```
    /// # use petritype::{Event, NetBuilder, Param, TakeEdge};
    /// # #[derive(Clone)] struct Scale(u32);
    /// # let mut b = NetBuilder::new();
    /// # let inbox = b.place::<u32>("Inbox");
    /// # let out = b.place::<u32>("Out");
    /// # b.seed(inbox, [1, 1]);
    /// # b.param(Scale(2));
    /// # b.transition("Scale", (TakeEdge(inbox), Param::<Scale>::new()), (out,), |n: u32, s: Scale| (n * s.0,));
    /// # let mut net = b.build()?;
    /// net.step()?;
    /// net.set_param(Scale(10))?;
    /// net.step()?;
    ///
    /// let values: Vec<u32> = net
    ///     .tokens(out.id())
    ///     .map(|(_, v)| *v.downcast_ref::<u32>().unwrap())
    ///     .collect();
    /// assert_eq!(values, [2, 10], "the second firing saw the new value");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// [`NetError::NoSuchParam`] if this net has no parameter of type `T`.
    /// Parameters are fixed at [`build`](crate::NetBuilder::build): this adds
    /// none, and the net is unchanged and silent when it refuses.
    pub fn set_param<T: Clone + Send + Sync + 'static>(
        &mut self,
        value: T,
    ) -> Result<(), NetError> {
        if param_slot_index(&self.params, TypeId::of::<T>()).is_none() {
            return Err(NetError::NoSuchParam {
                type_name: type_name::<T>(),
            });
        }
        self.set_param_erased(Box::new(value))
    }

    // ----- internals ------------------------------------------------------

    /// Hands out the next token id. Ids are never reused.
    fn mint_token_id(&mut self) -> TokenId {
        let id = TokenId(self.next_token_id);
        self.next_token_id += 1;
        id
    }

    /// Appends an event and counts it as a state change. Every event the net
    /// emits is one, which is what makes
    /// [`state_counter`](Net::state_counter) a count of events: one firing
    /// bumps it once for its [`Fired`](Event::Fired) or
    /// [`Failed`](Event::Failed), plus once per
    /// [`Mutated`](Event::Mutated) a mutating edge produced.
    pub(crate) fn emit(&mut self, event: Event) {
        self.events.push(event);
        self.state_counter += 1;
    }

    /// The name of one of this net's places, for the message of an error
    /// that also carries its id.
    ///
    /// An id that indexes no place cannot reach this: every caller has
    /// already established that the place is one of this net's. But an error
    /// path is the wrong place to panic, so an unknown id gets
    /// [`UNKNOWN_NAME`] rather than an index out of bounds.
    pub(crate) fn place_name(&self, place: PlaceId) -> String {
        self.places
            .get(place.0 as usize)
            .map_or_else(|| String::from(UNKNOWN_NAME), |slot| slot.meta.name.clone())
    }

    /// The name of one of this net's transitions, for the message of an
    /// error that also carries its id. As [`place_name`](Net::place_name),
    /// stand-in included.
    pub(crate) fn transition_name(&self, transition: TransitionId) -> String {
        self.transitions
            .get(transition.0 as usize)
            .map_or_else(|| String::from(UNKNOWN_NAME), |slot| slot.name.clone())
    }

    /// The one insertion path: mint an id, check the type, push the token.
    ///
    /// Everything that adds a token — the typed methods here, the seeds in
    /// [`build`](crate::NetBuilder::build), the erased
    /// [`Command`](crate::Command) form and the output side of a firing —
    /// goes through this function, so
    /// id minting and the type check are written once. It deliberately does
    /// *not* log: the caller decides whether the insertion is a change an
    /// observer should hear about ([`push_erased`](Net::push_erased)) or part
    /// of setting the net up ([`build`](crate::NetBuilder::build)).
    ///
    /// `found` is the type name to blame in a [`NetError::TypeMismatch`]: the
    /// typed callers pass `type_name::<T>()`, an erased caller passes whatever
    /// it knows.
    pub(crate) fn insert_erased(
        &mut self,
        place: PlaceId,
        value: ErasedToken,
        found: &'static str,
    ) -> Result<TokenId, NetError> {
        let index = place.0 as usize;
        let slot = self
            .places
            .get(index)
            .ok_or(NetError::NoSuchPlace { place })?;
        // `(*value).type_id()` and not `value.type_id()`: the latter would ask
        // the `Box` for *its* type, since `Box<dyn Any + ..>` is itself `Any`.
        if (*value).type_id() != slot.meta.type_id {
            return Err(NetError::TypeMismatch {
                place,
                name: slot.meta.name.clone(),
                expected: slot.meta.type_name,
                found,
            });
        }
        let token = self.mint_token_id();
        self.places[index].tokens.push_back((token, value));
        Ok(token)
    }

    /// [`insert_erased`](Net::insert_erased) plus the [`Added`](Event::Added)
    /// event: an insertion that happens to a *running* net, which an observer
    /// has to hear about.
    pub(crate) fn push_erased(
        &mut self,
        place: PlaceId,
        value: ErasedToken,
        found: &'static str,
    ) -> Result<TokenId, NetError> {
        let token = self.insert_erased(place, value, found)?;
        self.emit(Event::Added { place, token });
        Ok(token)
    }

    /// The output side of a firing: mint an id and push a token whose type
    /// was checked when the transition was declared.
    ///
    /// [`insert_erased`](Net::insert_erased) without the two things stage
    /// three does not need. There is nothing to blame in a type mismatch,
    /// because the body's return type was matched against the output place's
    /// type at compile time; and there is no [`Added`](Event::Added), because
    /// a firing reports every token it produced together in one
    /// [`Fired`](Event::Fired). Insertion from outside the net
    /// ([`push_erased`](Net::push_erased)) and the builder's seeds are
    /// unaffected.
    pub(crate) fn push_produced(&mut self, place: PlaceId, value: ErasedToken) -> TokenId {
        self.insert_erased(place, value, "<produced>")
            .expect("a body's return type is matched against its output places when declared")
    }

    /// Checks a whole batch before any of it is written: the place exists,
    /// and every token in it has the place's type.
    ///
    /// This is what makes an [`Extend`](crate::Command::Extend) or a
    /// [`SetTokens`](crate::Command::SetTokens) all-or-nothing. Without it a
    /// batch whose third token is of the wrong type would leave the first two
    /// behind, and an observer would have seen two
    /// [`Added`](Event::Added) events for a command that was rejected.
    fn check_tokens(
        &self,
        place: PlaceId,
        tokens: &[ErasedToken],
        found: &'static str,
    ) -> Result<(), NetError> {
        let slot = self
            .places
            .get(place.0 as usize)
            .ok_or(NetError::NoSuchPlace { place })?;
        for token in tokens {
            // `(**token).type_id()`: the inner `dyn Any`'s type, not the
            // `Box`'s. See [`insert_erased`](Net::insert_erased).
            if (**token).type_id() != slot.meta.type_id {
                return Err(NetError::TypeMismatch {
                    place,
                    name: slot.meta.name.clone(),
                    expected: slot.meta.type_name,
                    found,
                });
            }
        }
        Ok(())
    }

    /// The shared body of [`extend`](Net::extend) and
    /// [`Command::Extend`](crate::Command::Extend): check the whole batch,
    /// then push every token with its [`Added`](Event::Added).
    pub(crate) fn extend_erased(
        &mut self,
        place: PlaceId,
        tokens: Vec<ErasedToken>,
        found: &'static str,
    ) -> Result<Vec<TokenId>, NetError> {
        self.check_tokens(place, &tokens, found)?;
        Ok(tokens
            .into_iter()
            .map(|token| {
                self.push_erased(place, token, found)
                    .expect("the batch was checked against this place above")
            })
            .collect())
    }

    /// The shared body of [`set_tokens`](Net::set_tokens) and
    /// [`Command::SetTokens`](crate::Command::SetTokens): check the whole
    /// batch, *then* clear and push.
    ///
    /// The check comes first so that a rejected batch does not empty the
    /// place on its way out.
    pub(crate) fn set_erased(
        &mut self,
        place: PlaceId,
        tokens: Vec<ErasedToken>,
        found: &'static str,
    ) -> Result<Vec<TokenId>, NetError> {
        self.check_tokens(place, &tokens, found)?;
        self.clear(place);
        self.extend_erased(place, tokens, found)
    }

    /// The shared body of [`replace`](Net::replace) and
    /// [`Command::Replace`](crate::Command::Replace): type first, then the
    /// token, then the write and its [`Mutated`](Event::Mutated).
    pub(crate) fn replace_erased(
        &mut self,
        place: PlaceId,
        token: TokenId,
        value: ErasedToken,
        found: &'static str,
    ) -> Result<(), NetError> {
        {
            let slot = self
                .places
                .get_mut(place.0 as usize)
                .ok_or(NetError::NoSuchPlace { place })?;
            if (*value).type_id() != slot.meta.type_id {
                return Err(NetError::TypeMismatch {
                    place,
                    name: slot.meta.name.clone(),
                    expected: slot.meta.type_name,
                    found,
                });
            }
            let index = slot.position(token).ok_or_else(|| NetError::NoSuchToken {
                place,
                name: slot.meta.name.clone(),
                token,
            })?;
            slot.tokens[index].1 = value;
        }
        self.emit(Event::Mutated { place, token });
        Ok(())
    }

    /// The shared body of [`set_param`](Net::set_param) and
    /// [`Command::SetParam`](crate::Command::SetParam): find the parameter by
    /// the value's own type, replace it, say so.
    ///
    /// The lookup is by [`TypeId`], which an erased value knows, so the two
    /// routes are the same code; only the blame differs when there is no such
    /// parameter, because the typed caller can name the type and this one
    /// cannot. The [`ParamChanged`](Event::ParamChanged) event carries the
    /// *net's* name for the type, which is the real one either way.
    pub(crate) fn set_param_erased(&mut self, value: ErasedToken) -> Result<(), NetError> {
        // `(*value).type_id()` and not `value.type_id()`: the latter would
        // ask the `Box` for *its* type, as on the token path.
        let type_id = (*value).type_id();
        let Some(index) = param_slot_index(&self.params, type_id) else {
            return Err(NetError::NoSuchParam { type_name: ERASED });
        };
        self.params[index].value = value;
        let type_name = self.params[index].type_name;
        self.emit(Event::ParamChanged { type_name });
        Ok(())
    }

    /// `push_erased` for a caller that still knows `T`, where the only way to
    /// fail is to use a handle from another net.
    fn push_typed<T: Send + Sync + 'static>(
        &mut self,
        place: PlaceRef<T>,
        value: T,
        method: &str,
    ) -> TokenId {
        self.checked_index(place, method);
        self.push_erased(place.id(), Box::new(value), type_name::<T>())
            .expect("a stamped handle indexes a place of its own type")
    }

    /// The index of a handle in this net, or a panic naming both nets.
    ///
    /// The infallible half of the API — [`extend`](Net::extend),
    /// [`put`](Net::put), [`set_tokens`](Net::set_tokens) — has nowhere to
    /// report a foreign handle, and silently writing to whatever place sits
    /// at that index in *this* net would be the worse answer. The fallible
    /// half ([`remove`](Net::remove), [`replace`](Net::replace)) returns
    /// [`NetError::NoSuchPlace`] instead.
    fn checked_index<T>(&self, place: PlaceRef<T>, method: &str) -> usize {
        assert!(
            place.net() == self.id,
            "{}",
            foreign_place(method, place.id(), place.net(), self.id)
        );
        place.id().0 as usize
    }

    /// Removes every token of a place, emitting one
    /// [`Removed`](Event::Removed) each. An unknown place is a no-op.
    fn clear(&mut self, place: PlaceId) {
        let Some(slot) = self.places.get_mut(place.0 as usize) else {
            return;
        };
        let removed: Vec<TokenId> = slot.tokens.drain(..).map(|(id, _)| id).collect();
        for token in removed {
            self.emit(Event::Removed { place, token });
        }
    }
}

impl fmt::Debug for Net {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Net")
            .field("id", &self.id)
            .field("places", &self.places)
            .field("transitions", &self.transitions)
            // Type names only: a parameter value is never printed (DESIGN
            // §10), and `ParamSlot`'s own `Debug` is what says so.
            .field("params", &self.params)
            .field("state_counter", &self.state_counter)
            .field("topology_counter", &self.topology_counter)
            .field("fired_count", &self.fired_count)
            // A selector is a closure: all a reader can be told is whether
            // the net has one of its own.
            .field("custom_selector", &self.selector.is_some())
            .field("pending_events", &self.events.len())
            .finish()
    }
}

// ----- the read API, over places alone -------------------------------------
//
// A [`Snapshot`](crate::Snapshot) is the net's state without its bodies, and
// a reader must be able to ask it the same questions in the same words. These
// functions are those questions, written once over a plain slice of places,
// and both `Net` and `Snapshot` are thin typed wrappers around them — which
// is also why a snapshot's labels and JSON come out exactly as the net's do:
// it carries the same [`PlaceMeta`](crate::place::PlaceMeta) vtables.

/// Every place, in id order, with its name, token type and live count.
pub(crate) fn places_of(places: &[PlaceSlot]) -> impl Iterator<Item = (PlaceId, PlaceInfo<'_>)> {
    places.iter().enumerate().map(|(index, slot)| {
        // The index is where the id came from in the first place.
        let id = PlaceId(index as u32);
        let info = PlaceInfo {
            name: &slot.meta.name,
            type_name: slot.meta.type_name,
            count: slot.tokens.len(),
        };
        (id, info)
    })
}

/// The id of the place with this name, if there is one.
pub(crate) fn place_named_in(places: &[PlaceSlot], name: &str) -> Option<PlaceId> {
    places
        .iter()
        .position(|slot| slot.meta.name == name)
        .map(|index| PlaceId(index as u32))
}

/// How many tokens a place holds; an unknown place counts zero.
pub(crate) fn count_in(places: &[PlaceSlot], place: PlaceId) -> usize {
    places
        .get(place.0 as usize)
        .map_or(0, |slot| slot.tokens.len())
}

/// The tokens of a place, oldest first and erased.
pub(crate) fn tokens_in(
    places: &[PlaceSlot],
    place: PlaceId,
) -> impl Iterator<Item = (TokenId, &dyn Any)> {
    places.get(place.0 as usize).into_iter().flat_map(|slot| {
        slot.tokens.iter().map(|(id, value)| {
            // `dyn Any + Send + Sync` unsizes to plain `dyn Any`; the
            // auto traits matter to the net, not to a reader.
            let value: &dyn Any = &**value;
            (*id, value)
        })
    })
}

/// One token of a place, downcast to `T`.
pub(crate) fn token_in<T: 'static>(
    places: &[PlaceSlot],
    place: PlaceId,
    token: TokenId,
) -> Option<&T> {
    let slot = places.get(place.0 as usize)?;
    let index = slot.position(token)?;
    slot.tokens[index].1.downcast_ref::<T>()
}

/// One token rendered with its type's `Debug`, truncated to `limit`.
pub(crate) fn label_in(
    places: &[PlaceSlot],
    place: PlaceId,
    token: TokenId,
    limit: usize,
) -> Option<String> {
    let slot = places.get(place.0 as usize)?;
    let index = slot.position(token)?;
    let text = (slot.meta.debug)(&*slot.tokens[index].1);
    Some(truncate_label(text, limit))
}

/// One token as JSON, for a place that recorded an encoder.
#[cfg(feature = "serde")]
pub(crate) fn json_in(
    places: &[PlaceSlot],
    place: PlaceId,
    token: TokenId,
) -> Option<serde_json::Value> {
    let slot = places.get(place.0 as usize)?;
    let to_json = slot.meta.to_json?;
    let index = slot.position(token)?;
    Some(to_json(&*slot.tokens[index].1))
}

/// Cuts `text` to `limit` characters, on a character boundary, appending
/// `"..."` if anything was cut.
///
/// `char_indices().nth(limit)` is both the test and the cut point: it yields
/// `Some` exactly when there is a `limit + 1`-th character, and its byte
/// offset is where the first `limit` characters end.
fn truncate_label(mut text: String, limit: usize) -> String {
    if let Some((byte, _)) = text.char_indices().nth(limit) {
        text.truncate(byte);
        text.push_str("...");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::truncate_label;

    #[test]
    fn short_labels_are_untouched() {
        assert_eq!(truncate_label("abc".to_owned(), 3), "abc");
    }

    #[test]
    fn long_labels_are_cut_and_marked() {
        assert_eq!(truncate_label("abcd".to_owned(), 3), "abc...");
    }

    #[test]
    fn multibyte_labels_are_cut_on_a_character_boundary() {
        // Four characters, eight bytes: a naive byte truncation at 3 would
        // split a character and panic.
        let cut = truncate_label("日本語x".to_owned(), 3);
        assert_eq!(cut, "日本語...");
    }

    #[test]
    fn a_zero_limit_keeps_nothing() {
        assert_eq!(truncate_label("abc".to_owned(), 0), "...");
    }
}
