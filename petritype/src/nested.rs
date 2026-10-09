//! Nets as tokens: a token that owns Petri nets of its own (DESIGN §6.6).
//!
//! A vehicle moving between the stations of a city has a life of its own
//! inside — passengers sit down, stand, go to the door — and that life is a
//! net too. Here the vehicle is a token of the *outer* net, and the net it
//! carries is an *inner* net. The outer net moves the token like any other;
//! what is new is that the engine knows the inner net is there:
//!
//! - **Declared.** A token type says which nets it owns by implementing
//!   [`Nested`], and a place declared with
//!   [`NetBuilder::place_nested`](crate::NetBuilder::place_nested) holds such
//!   tokens. [`Net::nested`] reads a token's inner nets back.
//! - **Run.** [`Net::step_deep`] fires the outer net first and, when it has
//!   nothing to fire, the first inner net that has, in place order, then
//!   token order, then the order [`Nested::nets`] lists them.
//!   [`drivers::run`](crate::drivers::run) steps that way, so running a net
//!   to quiescence runs its inner nets too. A
//!   [passive](crate::NetBuilder::passive) place is skipped: its inner nets
//!   fire only when an outer body steps them.
//! - **Reached.** A [`MutateEdge`](crate::MutateEdge) body is handed the
//!   token itself, and so the inner net, with its whole API; a guard reads
//!   inner markings with [`Net::nested`]; a caller that is not a body uses
//!   [`Net::nested_scope`].
//! - **Observed.** Whatever an inner net logs ends up in the outer log,
//!   wrapped in [`Event::Nested`] with the place and token it happened in,
//!   at a moment fixed by the rules on [`Event::Nested`] — *lifted*, in this
//!   crate's word. Nothing is left behind for a driver to collect.
//!
//! The coupling goes one way. An inner net never sees the outer one: its
//! guards and bodies are its own, and the outer net reaches in only through
//! the token it holds. There are no shared places.

use std::any::Any;
use std::fmt;

use crate::error::FiringFailed;
#[cfg(feature = "serde")]
use crate::error::NetError;
use crate::event::Event;
use crate::ids::{NetId, PlaceId, TokenId, TransitionId};
use crate::net::Net;
use crate::place::{ErasedToken, PlaceSlot};

/// A token type that owns Petri nets of its own, each under a name.
///
/// Implement it for a token that carries an inner net — a vehicle with a
/// cabin, a character with a body — and declare the place that holds such
/// tokens with
/// [`NetBuilder::place_nested`](crate::NetBuilder::place_nested). The engine
/// then runs the inner nets ([`Net::step_deep`]), lets the outer net read
/// them ([`Net::nested`]) and reports what happens inside them in the outer
/// log ([`Event::Nested`]).
///
/// ```
/// use petritype::{Nested, Net};
///
/// #[derive(Debug)]
/// struct Vehicle {
///     name: String,
///     inner: Net,
/// }
///
/// impl Nested for Vehicle {
///     fn nets(&self) -> Vec<(&str, &Net)> {
///         vec![("cabin", &self.inner)]
///     }
///     fn nets_mut(&mut self) -> Vec<(&str, &mut Net)> {
///         vec![("cabin", &mut self.inner)]
///     }
/// }
/// ```
///
/// The two methods must agree: the same nets, under the same names, in the
/// same order — the order is the order the engine steps them in, and the
/// name is what [`Event::Nested::net`](Event::Nested) and
/// [`FiringFailed::path`] call each one. Names need not be unique, but a
/// reader that addresses a net by name finds the first.
///
/// [`Net`] implements it as one unnamed inner net, `("", net)`, so a place of
/// bare nets is a place of nested tokens with no wrapper type at all.
///
/// # Copies
///
/// A `Net` cannot be cloned — its bodies are closures — so neither can a
/// token that owns one. With the `serde` feature the trait has a third
/// method, `fork`, a fallible copy: the plain fields cloned, each inner net
/// through `Net::fork`, which rebuilds it from the template that built it.
/// It is what [`Net::snapshot`] copies the tokens of a nested place with.
/// The default refuses (`NetError::NotForkable`), so a type that never needs
/// a snapshot writes nothing; one that does writes three lines:
///
/// ```
/// # #[cfg(feature = "serde")] {
/// # use petritype::{Nested, Net, NetError};
/// # #[derive(Debug)] struct Vehicle { name: String, inner: Net }
/// impl Nested for Vehicle {
/// #   fn nets(&self) -> Vec<(&str, &Net)> { vec![("cabin", &self.inner)] }
/// #   fn nets_mut(&mut self) -> Vec<(&str, &mut Net)> { vec![("cabin", &mut self.inner)] }
///     // nets and nets_mut as above, and:
///     fn fork(&self) -> Result<Self, NetError> {
///         Ok(Vehicle { name: self.name.clone(), inner: self.inner.fork()? })
///     }
/// }
/// # }
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not say which nets it owns, so it cannot be the token of a nested place",
    label = "not `Nested`",
    note = "implement `petritype::Nested` for `{Self}`: `nets` and `nets_mut` list the inner nets \
            a token owns, each under a name",
    note = "a token that owns no net goes in an ordinary `place`"
)]
pub trait Nested: Send + Sync + 'static {
    /// The inner nets this token owns, each with a name, in a fixed order.
    fn nets(&self) -> Vec<(&str, &Net)>;

    /// The same nets, mutably, under the same names and in the same order as
    /// [`nets`](Nested::nets).
    fn nets_mut(&mut self) -> Vec<(&str, &mut Net)>;

    /// A copy of this token, its inner nets rebuilt from their templates
    /// with [`Net::fork`]: what [`Net::snapshot`] copies a nested place's
    /// tokens with (DESIGN §6.6).
    ///
    /// # Errors
    ///
    /// By default [`NetError::NotForkable`] naming the type: a token type
    /// says how to copy itself or it is not copied. An implementation
    /// passes on whatever its nets' `fork` returns.
    #[cfg(feature = "serde")]
    fn fork(&self) -> Result<Self, NetError>
    where
        Self: Sized,
    {
        Err(NetError::NotForkable {
            type_name: std::any::type_name::<Self>(),
        })
    }
}

/// A net is a token that owns one inner net, itself, with no name: `""`.
impl Nested for Net {
    fn nets(&self) -> Vec<(&str, &Net)> {
        vec![("", self)]
    }

    fn nets_mut(&mut self) -> Vec<(&str, &mut Net)> {
        vec![("", self)]
    }

    /// [`Net::fork`].
    #[cfg(feature = "serde")]
    fn fork(&self) -> Result<Self, NetError> {
        Net::fork(self)
    }
}

/// Lists the inner nets of an erased token. See [`nets_of`].
pub(crate) type NetsFn = fn(&dyn Any) -> Vec<(&str, &Net)>;

/// Lists the inner nets of an erased token, mutably. See [`nets_mut_of`].
pub(crate) type NetsMutFn = fn(&mut dyn Any) -> Vec<(&str, &mut Net)>;

/// Copies an erased token through its type's [`Nested::fork`]. See
/// [`fork_of`].
#[cfg(feature = "serde")]
pub(crate) type ForkFn = fn(&dyn Any) -> Result<ErasedToken, NetError>;

/// The [`Nested`] half of a place's vtable: `T::nets` and `T::nets_mut`,
/// monomorphised by
/// [`place_nested`](crate::NetBuilder::place_nested) where `T` is known and
/// kept as plain `fn` pointers, as `debug` and `clone` are (DESIGN §4.2).
#[derive(Clone, Copy)]
pub(crate) struct NestedVtable {
    /// `nets_of::<T>`.
    pub(crate) nets: NetsFn,
    /// `nets_mut_of::<T>`.
    pub(crate) nets_mut: NetsMutFn,
    /// `fork_of::<T>`: the cloner a nested place has instead of `clone`,
    /// fallible because a fork rebuilds nets from templates.
    #[cfg(feature = "serde")]
    pub(crate) fork: ForkFn,
}

impl NestedVtable {
    /// The vtable for tokens of type `T`.
    pub(crate) fn of<T: Nested>() -> NestedVtable {
        NestedVtable {
            nets: nets_of::<T> as NetsFn,
            nets_mut: nets_mut_of::<T> as NetsMutFn,
            #[cfg(feature = "serde")]
            fork: fork_of::<T> as ForkFn,
        }
    }
}

impl fmt::Debug for NestedVtable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NestedVtable")
    }
}

/// `nets_of::<T>` lists an erased token's inner nets with `T`'s [`Nested`].
fn nets_of<T: Nested>(value: &dyn Any) -> Vec<(&str, &Net)> {
    value
        .downcast_ref::<T>()
        .expect("token in a place is always of the place's type")
        .nets()
}

/// `nets_mut_of::<T>` is [`nets_of`], mutably.
fn nets_mut_of<T: Nested>(value: &mut dyn Any) -> Vec<(&str, &mut Net)> {
    value
        .downcast_mut::<T>()
        .expect("token in a place is always of the place's type")
        .nets_mut()
}

/// `fork_of::<T>` copies an erased token with `T`'s [`Nested::fork`],
/// erasing the copy again.
#[cfg(feature = "serde")]
fn fork_of<T: Nested>(value: &dyn Any) -> Result<ErasedToken, NetError> {
    value
        .downcast_ref::<T>()
        .expect("token in a place is always of the place's type")
        .fork()
        .map(|copy| Box::new(copy) as ErasedToken)
}

/// The inner nets of one token of a nested place, read through the place's
/// vtable: [`Net::nested`] and [`Snapshot::nested`](crate::Snapshot::nested)
/// both ask this.
pub(crate) fn nested_in(
    places: &[impl std::borrow::Borrow<PlaceSlot>],
    place: PlaceId,
    token: TokenId,
) -> Option<Vec<(&str, &Net)>> {
    let slot = places.get(place.0 as usize)?.borrow();
    let nested = slot.meta.nested?;
    let index = slot.position(token)?;
    Some((nested.nets)(&*slot.tokens[index].1))
}

/// Empties the logs of one token's inner nets, wrapping every event for the
/// outer log under the place and the token id the token has now.
///
/// The one place an inner event is *lifted*. Every rule on
/// [`Event::Nested`] is a choice of when to call this; the wrapping is
/// always the same.
pub(crate) fn lift_from(
    nested: NestedVtable,
    value: &mut ErasedToken,
    place: PlaceId,
    token: TokenId,
) -> Vec<Event> {
    let mut lifted = Vec::new();
    for (name, inner) in (nested.nets_mut)(&mut **value) {
        let nested_id = inner.id();
        lifted.extend(inner.drain_events().map(|event| Event::Nested {
            place,
            token,
            net: name.to_owned(),
            nested_id,
            event: Box::new(event),
        }));
    }
    lifted
}

/// Empties the logs of one token's inner nets and throws the events away.
///
/// For the seeds of a nested place: seeding is setup, not change (DESIGN
/// §4.1), and an inner net set up before it was seeded starts where its
/// owner starts, with nothing to report.
pub(crate) fn forget_inner_events(nested: NestedVtable, value: &mut ErasedToken) {
    for (_, inner) in (nested.nets_mut)(&mut **value) {
        inner.events.clear();
    }
}

/// What one [`Net::step_deep`] fired, and where.
///
/// [`Here`](DeepStep::Here) is a transition of the net that was stepped;
/// [`Inside`](DeepStep::Inside) is a firing inside one of its tokens, with
/// the place and token it happened in and, recursively, what fired in there.
/// The outer net's log says the same thing as an [`Event::Nested`], with the
/// same place, token, name and id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeepStep {
    /// A transition of this net fired.
    Here {
        /// The transition that fired.
        transition: TransitionId,
    },
    /// Something fired inside an inner net of one of this net's tokens.
    Inside {
        /// The nested place the token is in.
        place: PlaceId,
        /// The token, by the id it has in that place.
        token: TokenId,
        /// The inner net's name, as [`Nested::nets`] gave it.
        net: String,
        /// The inner net's own [`NetId`]: the id that stays the same when
        /// the token moves and its [`TokenId`] does not.
        nested_id: NetId,
        /// What fired in there.
        step: Box<DeepStep>,
    },
}

impl DeepStep {
    /// The transition that fired, at whatever depth: the innermost
    /// [`Here`](DeepStep::Here). Its id is an id of the innermost net.
    pub fn transition(&self) -> TransitionId {
        match self {
            DeepStep::Here { transition } => *transition,
            DeepStep::Inside { step, .. } => step.transition(),
        }
    }
}

impl Net {
    /// The inner nets of one token of a nested place, each with its name, in
    /// the order [`Nested::nets`] lists them.
    ///
    /// `None` if the place is not nested
    /// ([`place_nested`](crate::NetBuilder::place_nested)), or does not
    /// exist, or holds no such token. This is how a
    /// [guard](crate::TransitionBuilder::guard) reads inside a token — the
    /// front one is what a [`MutateEdge`](crate::MutateEdge) would be handed:
    ///
    /// ```
    /// use petritype::{NetBuilder, TakeEdge};
    ///
    /// // An inner net with one token in it.
    /// let mut inner = NetBuilder::new();
    /// let cargo = inner.place::<u32>("Cargo");
    /// inner.seed(cargo, [7]);
    /// let inner = inner.build()?;
    ///
    /// let mut b = NetBuilder::new();
    /// let yard = b.place_nested::<petritype::Net>("Yard");
    /// let gone = b.place_nested::<petritype::Net>("Gone");
    /// b.seed(yard, [inner]);
    /// b.transition("Ship", (TakeEdge(yard),), (gone,), |net: petritype::Net| (net,))
    ///     .guard(move |net| {
    ///         let Some((token, _)) = net.tokens(yard.id()).next() else { return false };
    ///         net.nested(yard.id(), token)
    ///             .is_some_and(|nets| nets.iter().any(|(_, inner)| inner.count(cargo.id()) > 0))
    ///     });
    /// let net = b.build()?;
    ///
    /// let (token, _) = net.tokens(yard.id()).next().unwrap();
    /// let nets = net.nested(yard.id(), token).expect("a nested place");
    /// assert_eq!(nets.len(), 1);
    /// assert_eq!(nets[0].0, "", "a bare net is its own unnamed inner net");
    /// assert_eq!(nets[0].1.count(cargo.id()), 1);
    /// assert!(!net.fireable().is_empty(), "the guard read the cargo");
    /// # Ok::<(), petritype::BuildError>(())
    /// ```
    pub fn nested(&self, place: PlaceId, token: TokenId) -> Option<Vec<(&str, &Net)>> {
        nested_in(&self.places, place, token)
    }

    /// Calls `f` with the inner nets of one token of a nested place, mutably,
    /// and lifts whatever they logged into this net's log when it returns.
    ///
    /// The reach for a caller that is not a body — a driver that injects
    /// into an inner net, a test that sets one up, an inspector applying an
    /// edit — with the same reporting a body gets: each event `f` caused
    /// inside comes out here as an [`Event::Nested`], under this place and
    /// token, and an `f` that changes nothing emits nothing. Deeper levels
    /// are reached by calling `nested_scope` on an inner net inside `f`;
    /// each level lifts into the one above it as its call returns.
    ///
    /// `None`, with `f` not called, if the place is not nested or holds no
    /// such token.
    ///
    /// ```
    /// use petritype::{Event, NetBuilder};
    ///
    /// let mut inner = NetBuilder::new();
    /// let cargo = inner.place::<u32>("Cargo");
    /// let inner = inner.build()?;
    ///
    /// let mut b = NetBuilder::new();
    /// let yard = b.place_nested::<petritype::Net>("Yard");
    /// b.seed(yard, [inner]);
    /// let mut net = b.build()?;
    /// let (token, _) = net.tokens(yard.id()).next().unwrap();
    ///
    /// let added = net
    ///     .nested_scope(yard.id(), token, |nets| nets[0].1.put(cargo, 7))
    ///     .expect("a nested place");
    /// let events: Vec<Event> = net.drain_events().collect();
    /// assert!(matches!(
    ///     &events[..],
    ///     [Event::Nested { place, event, .. }]
    ///         if *place == yard.id() && **event == Event::Added { place: cargo.id(), token: added }
    /// ));
    /// # Ok::<(), petritype::BuildError>(())
    /// ```
    pub fn nested_scope<R>(
        &mut self,
        place: PlaceId,
        token: TokenId,
        f: impl FnOnce(&mut [(&str, &mut Net)]) -> R,
    ) -> Option<R> {
        let (result, lifted) = {
            let slot = self.places.get_mut(place.0 as usize)?;
            let nested = slot.meta.nested?;
            let index = slot.position(token)?;
            let value = &mut slot.tokens[index].1;
            let result = f(&mut (nested.nets_mut)(&mut **value));
            (result, lift_from(nested, value, place, token))
        };
        self.emit_all(lifted);
        Some(result)
    }

    /// Fires one transition, here or in an inner net: this net first, and
    /// when it has nothing to fire, the first inner net that has.
    ///
    /// [`step`](Net::step) is tried first, selector and all. When it fires
    /// nothing, the inner nets are walked — nested places in place order,
    /// [passive](crate::NetBuilder::passive) ones skipped, each place's
    /// tokens oldest first, each token's nets in [`Nested::nets`] order — and
    /// `step_deep` is called on each in turn, so the walk goes as deep as
    /// the nesting does. The first firing found is the step. Called in a
    /// loop, which is what [`drivers::run`](crate::drivers::run) does, this
    /// is DESIGN §6.6's policy: the outer net until it is quiet, then the
    /// inner nets in place order and token order, repeated until everything
    /// is quiet. Deterministic, given the selectors.
    ///
    /// An inner firing's events are in this net's log, as
    /// [`Event::Nested`], by the time this returns.
    ///
    /// ```
    /// use petritype::{DeepStep, NetBuilder, TakeEdge};
    ///
    /// let mut inner = NetBuilder::new();
    /// let todo = inner.place::<u32>("Todo");
    /// let done = inner.place::<u32>("Done");
    /// inner.seed(todo, [1]);
    /// let work = inner.transition("Work", (TakeEdge(todo),), (done,), |n: u32| (n,)).id();
    /// let inner = inner.build()?;
    ///
    /// let mut b = NetBuilder::new();
    /// let desk = b.place_nested::<petritype::Net>("Desk");
    /// b.seed(desk, [inner]);
    /// let mut net = b.build()?;
    ///
    /// assert_eq!(net.step()?, None, "nothing fires in the outer net itself");
    /// let step = net.step_deep()?.expect("the inner net had work");
    /// assert!(matches!(&step, DeepStep::Inside { place, .. } if *place == desk.id()));
    /// assert_eq!(step.transition(), work);
    /// assert!(net.is_quiescent_deep());
    /// assert_eq!(net.step_deep()?, None);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// The [`FiringFailed`] of a failed firing, wherever it was. One that
    /// failed inside says where in [`path`](FiringFailed::path), outer to
    /// inner; its transition id is the inner net's. The failure's own
    /// [`Failed`](Event::Failed) event is lifted, like any other.
    pub fn step_deep(&mut self) -> Result<Option<DeepStep>, FiringFailed> {
        if let Some(transition) = self.step()? {
            return Ok(Some(DeepStep::Here { transition }));
        }
        self.step_inside()
    }

    /// Whether nothing can happen here or below: nothing fireable and nothing
    /// in flight in this net (DESIGN §3), and every inner net of every
    /// token in a non-[passive](crate::NetBuilder::passive) nested place
    /// quiescent in the same deep sense.
    ///
    /// [`fireable`](Net::fireable) stays about this net alone. A passive
    /// place's inner nets are left out because nothing but an outer body
    /// will ever step them.
    pub fn is_quiescent_deep(&self) -> bool {
        self.fireable().is_empty()
            && self.in_flight().is_empty()
            && self
                .active_inner_nets()
                .all(|(_, _, _, inner)| inner.is_quiescent_deep())
    }

    /// The inner half of [`step_deep`](Net::step_deep): one firing in the
    /// first inner net that has one, without asking this net's own selector
    /// first.
    ///
    /// For a caller that has already asked it. A driver that runs this net's
    /// own firings in stages — [`select`](Net::select), then
    /// [`launch`](Net::launch) with the body on a thread of its own, then
    /// [`distribute`](Net::distribute) — and wants the inner nets when that
    /// selector picks nothing calls this, where
    /// [`step_deep`](Net::step_deep) would ask the selector a second time
    /// and fire inline whatever it chose. The concurrent driver does exactly
    /// that when its launch sweep finds nothing out here, and so does the
    /// protocol runner (`petritype-runner`) for `step` and a running net.
    ///
    /// The walk and the lifting are `step_deep`'s: nested places in place
    /// order, [passive](crate::NetBuilder::passive) ones skipped, tokens
    /// oldest first, each token's nets in [`Nested::nets`] order, `step_deep`
    /// on each, the first firing found is the step, and its events are in
    /// this net's log, as [`Event::Nested`], by the time this returns. `None`
    /// when no inner net has anything to fire; this net's own fireable
    /// transitions are never fired here, at this level.
    ///
    /// ```
    /// use petritype::{DeepStep, NetBuilder, TakeEdge};
    ///
    /// let mut inner = NetBuilder::new();
    /// let todo = inner.place::<u32>("Todo");
    /// let done = inner.place::<u32>("Done");
    /// inner.seed(todo, [1]);
    /// inner.transition("Work", (TakeEdge(todo),), (done,), |n: u32| (n,));
    /// let inner = inner.build()?;
    ///
    /// let mut b = NetBuilder::new();
    /// let desk = b.place_nested::<petritype::Net>("Desk");
    /// let inbox = b.place::<u32>("Inbox");
    /// let outbox = b.place::<u32>("Outbox");
    /// b.seed(desk, [inner]);
    /// b.seed(inbox, [7]);
    /// b.transition("Forward", (TakeEdge(inbox),), (outbox,), |n: u32| (n,));
    /// let mut net = b.build()?;
    ///
    /// // `Forward` is fireable out here, and still the inner net goes first.
    /// let step = net.step_inside()?.expect("the inner net had work");
    /// assert!(matches!(step, DeepStep::Inside { place, .. } if place == desk.id()));
    /// assert_eq!(net.count(inbox.id()), 1, "the outer net was not fired");
    /// assert_eq!(net.step_inside()?, None, "and the inner net is done");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// The inner failure's [`FiringFailed`], as from `step_deep`, its
    /// [`path`](FiringFailed::path) saying where.
    pub fn step_inside(&mut self) -> Result<Option<DeepStep>, FiringFailed> {
        for index in 0..self.places.len() {
            let meta = &self.places[index].meta;
            let Some(nested) = meta.nested else {
                continue;
            };
            if meta.passive {
                continue;
            }
            let place = PlaceId(index as u32);
            for position in 0..self.places[index].tokens.len() {
                let (token, value) = &mut self.places[index].tokens[position];
                let token = *token;
                // The first inner net that fires, with what it logged; the
                // borrow of the token ends with this block, so that the
                // events can then be written to this net's own log.
                let found =
                    (nested.nets_mut)(&mut **value)
                        .into_iter()
                        .find_map(|(name, inner)| {
                            let result = inner.step_deep();
                            if matches!(result, Ok(None)) {
                                return None;
                            }
                            let nested_id = inner.id();
                            let events: Vec<Event> = inner.drain_events().collect();
                            Some((name.to_owned(), nested_id, events, result))
                        });
                let Some((name, nested_id, events, result)) = found else {
                    continue;
                };
                // Rule 1: lifted right after the inner firing, under the
                // place and token the token sits in.
                for event in events {
                    self.emit(Event::Nested {
                        place,
                        token,
                        net: name.clone(),
                        nested_id,
                        event: Box::new(event),
                    });
                }
                return match result {
                    Ok(Some(step)) => Ok(Some(DeepStep::Inside {
                        place,
                        token,
                        net: name,
                        nested_id,
                        step: Box::new(step),
                    })),
                    Err(mut failed) => {
                        failed.path.insert(0, (place, token, name));
                        Err(failed)
                    }
                    Ok(None) => unreachable!("only a firing or a failure is kept"),
                };
            }
        }
        Ok(None)
    }

    // ----- internals ------------------------------------------------------

    /// The first transition anywhere in the nesting that is fireable, with
    /// its name: this net's first, then the inner nets' in
    /// [`step_deep`](Net::step_deep) order. Selectors are not consulted.
    ///
    /// What the drivers' fuse blames when it trips: the transition that
    /// would have been attempted next.
    pub(crate) fn first_fireable_deep(&self) -> Option<(TransitionId, String)> {
        if let Some(&transition) = self.fireable().first() {
            return Some((transition, self.transition_name(transition)));
        }
        self.active_inner_nets()
            .find_map(|(_, _, _, inner)| inner.first_fireable_deep())
    }

    /// Every inner net that [`step_deep`](Net::step_deep) would walk, in the
    /// order it walks them: nested places in place order, passive ones
    /// skipped, tokens oldest first, nets in [`Nested::nets`] order.
    fn active_inner_nets(&self) -> impl Iterator<Item = (PlaceId, TokenId, &str, &Net)> {
        self.places
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| match slot.meta.nested {
                Some(nested) if !slot.meta.passive => Some((PlaceId(index as u32), slot, nested)),
                _ => None,
            })
            .flat_map(|(place, slot, nested)| {
                slot.tokens.iter().flat_map(move |(token, value)| {
                    let value: &dyn Any = &**value;
                    (nested.nets)(value)
                        .into_iter()
                        .map(move |(name, inner)| (place, *token, name, inner))
                })
            })
    }

    /// Lifts whatever the inner nets of one token logged into this net's
    /// log, under the place and the token id it has now. A place that is not
    /// nested, or a token that is not there, lifts nothing.
    pub(crate) fn lift(&mut self, place: PlaceId, token: TokenId) {
        let lifted = {
            let Some(slot) = self.places.get_mut(place.0 as usize) else {
                return;
            };
            let Some(nested) = slot.meta.nested else {
                return;
            };
            let Some(index) = slot.position(token) else {
                return;
            };
            lift_from(nested, &mut slot.tokens[index].1, place, token)
        };
        self.emit_all(lifted);
    }

    /// [`emit`](Net::emit), once per event and in order: every lifted event
    /// is a change here too, and moves
    /// [`state_counter`](Net::state_counter) by one.
    pub(crate) fn emit_all(&mut self, events: Vec<Event>) {
        for event in events {
            self.emit(event);
        }
    }
}
