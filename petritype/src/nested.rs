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
//!   token order, then the order [`Nested::nets`] lists them; or, under
//!   [`DeepPolicy::Interleaved`], every net in turn.
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
use std::ops::Range;

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

/// No inner net's turn: an empty range of places in walk order.
const NO_TURN: Range<usize> = 0..0;

/// Every inner net's turn, however many there are.
const EVERY_TURN: Range<usize> = 0..usize::MAX;

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

/// How [`Net::step_deep`] shares its firings between a net and the inner
/// nets its tokens carry (DESIGN §6.6).
///
/// The nets form a cycle in *walk order*: the net itself, then its inner
/// nets — nested places in place order,
/// [passive](crate::NetBuilder::passive) ones skipped, each place's tokens
/// oldest first, each token's nets in [`Nested::nets`] order, a net's own
/// inner nets before the next token. A deep step fires once, in the first
/// net it comes to that has a firing, chosen by that net's own selector; the
/// policy says where it starts.
///
/// - [`OuterFirst`](DeepPolicy::OuterFirst), the default, starts at the net
///   itself every time, so an outer net that can always fire **starves** its
///   inner nets. A net meant to run this way gives its outer transitions
///   guards that wait on what is inside, as the `transit` example's city
///   waits on where each vehicle's doors are.
/// - [`Interleaved`](DeepPolicy::Interleaved) starts at the net after the
///   one the previous deep step fired in and goes round once, so no live
///   net waits longer than **one round**, however busy the others are: the
///   policy for a net that cannot give its outer transitions such guards.
///
/// Interleaved's *cursor* is the net that fired last: this net's own
/// [`NetId`], or an inner net's nested id, recorded by every deep step under
/// either policy, a failed firing as well as one that landed, and read under
/// this one alone. Where there is none — a
/// fresh net, one restored from JSON or forked (`Net::fork`, with the
/// `serde` feature), since the cursor is kept in memory only, or when that
/// net has gone, its token consumed or moved to a passive place — the round
/// starts at this net.
///
/// The policy of the net that is stepped governs the whole walk below it;
/// an inner net's own policy counts only when it is stepped deep itself, by
/// an outer body. Quiescence ([`Net::is_quiescent_deep`]) does not depend
/// on it. Set with
/// [`NetBuilder::deep_policy`](crate::NetBuilder::deep_policy) or
/// [`Net::set_deep_policy`]; the drivers ([`drivers`](crate::drivers)) and
/// the protocol runner honour it.
///
/// ```
/// use petritype::{DeepPolicy, DeepStep, NetBuilder, TakeEdge};
///
/// // An inner net with one thing to do.
/// let mut inner = NetBuilder::new();
/// let todo = inner.place::<u32>("Todo");
/// let done = inner.place::<u32>("Done");
/// inner.seed(todo, [1]);
/// inner.transition("Work", (TakeEdge(todo),), (done,), |n: u32| (n,));
/// let inner = inner.build()?;
///
/// // An outer net that can always fire: `Spin` puts back what it takes.
/// let mut b = NetBuilder::new();
/// let wheel = b.place::<u32>("Wheel");
/// let desk = b.place_nested::<petritype::Net>("Desk");
/// b.seed(wheel, [0]);
/// b.seed(desk, [inner]);
/// b.transition("Spin", (TakeEdge(wheel),), (wheel,), |n: u32| (n + 1,));
/// b.deep_policy(DeepPolicy::Interleaved);
/// let mut net = b.build()?;
///
/// assert!(matches!(net.step_deep()?, Some(DeepStep::Here { .. })), "the outer net's turn");
/// assert!(matches!(net.step_deep()?, Some(DeepStep::Inside { .. })), "the inner net's");
/// assert!(matches!(net.step_deep()?, Some(DeepStep::Here { .. })), "and round again");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum DeepPolicy {
    /// This net's own selector first; when it chooses nothing, the first
    /// inner net in walk order that has a firing. Python petritype's
    /// `deep_policy="outer_first"`, and every net's policy before this
    /// setting existed.
    #[default]
    OuterFirst,
    /// Every net in turn: from the net after the one that fired last, round
    /// the cycle once, to the first that has a firing. Python petritype's
    /// `deep_policy="interleaved"`.
    Interleaved,
}

/// What one [`Net::step_deep_staged`] did: chose one of this net's own
/// transitions for the caller to fire, or fired inside.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StagedStep {
    /// It was this net's turn, and its selector chose this transition, which
    /// has **not** fired: the caller [launches](Net::launch) it and
    /// [distributes](Net::distribute) what comes back. The choice counted,
    /// as [`Net::choose`]'s does.
    Chosen(TransitionId),
    /// An inner net fired, inline, as in [`Net::step_deep`]: always a
    /// [`DeepStep::Inside`], its events already in this net's log.
    Inside(DeepStep),
}

impl Net {
    /// The inner nets of one token of a nested place, each with its name, in
    /// the order [`Nested::nets`] lists them.
    ///
    /// `None` if the place is not nested
    /// ([`place_nested`](crate::NetBuilder::place_nested)), or does not
    /// exist, or holds no such token. This is how a
    /// [guard](crate::TransitionBuilder::guard) reads inside a token — the
    /// front one is what a [`MutateEdge`](crate::MutateEdge) would be handed,
    /// and [`first_match`](Net::first_match) names the one a
    /// [`MutateWhereEdge`](crate::MutateWhereEdge) would:
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
            // A key may read the inner nets: the token is indexed again as
            // it is now (DESIGN §8.3).
            if slot.meta.keys.is_some() {
                self.index
                    .forget(&slot.meta, place, token, &*slot.tokens[index].1);
            }
            let value = &mut slot.tokens[index].1;
            (result, lift_from(nested, value, place, token))
        };
        self.emit_all(lifted);
        Some(result)
    }

    /// Fires one transition, here or in an inner net: this net first, and
    /// when it has nothing to fire, the first inner net that has — or, under
    /// [`DeepPolicy::Interleaved`], the next net in turn that has.
    ///
    /// Under the default [`DeepPolicy::OuterFirst`], [`step`](Net::step) is
    /// tried first, selector and all. When it fires nothing, the inner nets
    /// are walked — nested places in place order,
    /// [passive](crate::NetBuilder::passive) ones skipped, each place's
    /// tokens oldest first, each token's nets in [`Nested::nets`] order, a
    /// net's own inner nets before the next token — and each is stepped by
    /// its own selector, so the walk goes as deep as the nesting does. The
    /// first firing found is the step. Called in a loop, which is what
    /// [`drivers::run`](crate::drivers::run) does, this is DESIGN §6.6's
    /// policy: the outer net until it is quiet, then the inner nets in place
    /// order and token order, repeated until everything is quiet. Under
    /// `Interleaved` the walk is the same cycle, started after the net that
    /// fired last ([`DeepPolicy`]). Deterministic, given the selectors.
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
        Ok(match self.step_deep_staged()? {
            None => None,
            Some(StagedStep::Inside(step)) => Some(step),
            Some(StagedStep::Chosen(transition)) => {
                self.fire_now(transition)?;
                Some(DeepStep::Here { transition })
            }
        })
    }

    /// One deep step for a caller that fires this net's own transitions in
    /// stages: [`step_deep`](Net::step_deep), except that when the turn
    /// falls to this net and its selector chooses, the choice is handed back
    /// rather than fired.
    ///
    /// For a driver whose outer bodies run elsewhere — [`launch`](Net::launch)
    /// here, [`run`](crate::Firing::run) on a thread of its own,
    /// [`distribute`](Net::distribute) here — and whose inner firings are
    /// inline, as the protocol runner (`petritype-runner`) does for `step`
    /// and a running net. It honours the [`DeepPolicy`] where
    /// [`select`](Net::select) followed by [`step_inside`](Net::step_inside)
    /// cannot: under [`DeepPolicy::Interleaved`] the inner nets whose turn
    /// comes before this net's are tried first. The choice counts, for the
    /// [round robin](crate::TieBreak::RoundRobin) as
    /// [`choose`](Net::choose)'s does and for the deep policy as a firing
    /// here, so launch it.
    ///
    /// ```
    /// use petritype::{NetBuilder, StagedStep, TakeEdge};
    ///
    /// let mut b = NetBuilder::new();
    /// let inbox = b.place::<u32>("Inbox");
    /// let outbox = b.place::<u32>("Outbox");
    /// b.seed(inbox, [7]);
    /// let forward = b.transition("Forward", (TakeEdge(inbox),), (outbox,), |n: u32| (n,)).id();
    /// let mut net = b.build()?;
    ///
    /// let Some(StagedStep::Chosen(chosen)) = net.step_deep_staged()? else {
    ///     panic!("the outer net's own turn");
    /// };
    /// assert_eq!(chosen, forward);
    /// let firing = net.launch(chosen)?;            // the body may go anywhere now
    /// net.distribute(firing.run())?;
    /// assert_eq!(net.step_deep_staged()?, None, "quiescent, deep");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// An inner failure's [`FiringFailed`], as from `step_deep`, its
    /// [`path`](FiringFailed::path) saying where. Nothing of this net's own
    /// fires here, so nothing of its own fails.
    pub fn step_deep_staged(&mut self) -> Result<Option<StagedStep>, FiringFailed> {
        let (ahead, behind) = self.round();
        if let Some(step) = self.inner_turn(ahead)? {
            return Ok(Some(StagedStep::Inside(step)));
        }
        if let Some(transition) = self.choose() {
            self.deep_cursor = Some(self.id);
            return Ok(Some(StagedStep::Chosen(transition)));
        }
        Ok(self.inner_turn(behind)?.map(StagedStep::Inside))
    }

    /// Changes how [`step_deep`](Net::step_deep) shares its firings between
    /// this net and its inner nets; see [`DeepPolicy`]. Takes effect from
    /// the next deep step, and leaves the cursor where it is.
    pub fn set_deep_policy(&mut self, policy: DeepPolicy) {
        self.deep_policy = policy;
    }

    /// How [`step_deep`](Net::step_deep) shares its firings: as built, or as
    /// last set with [`set_deep_policy`](Net::set_deep_policy).
    pub fn deep_policy(&self) -> DeepPolicy {
        self.deep_policy
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

    /// The inner half of [`step_deep`](Net::step_deep) under
    /// [`DeepPolicy::OuterFirst`]: one firing in the first inner net that
    /// has one, without asking this net's own selector first.
    ///
    /// For a caller that has already asked it. A driver that runs this net's
    /// own firings in stages — [`select`](Net::select), then
    /// [`launch`](Net::launch) with the body on a thread of its own, then
    /// [`distribute`](Net::distribute) — and wants the inner nets when that
    /// selector picks nothing calls this, where
    /// [`step_deep`](Net::step_deep) would ask the selector a second time
    /// and fire inline whatever it chose. It always walks from the start,
    /// whatever the net's [`DeepPolicy`], and moves no cursor; a driver that
    /// honours the policy steps with
    /// [`step_deep_staged`](Net::step_deep_staged) instead, as the
    /// protocol runner (`petritype-runner`) does.
    ///
    /// The walk and the lifting are `step_deep`'s: nested places in place
    /// order, [passive](crate::NetBuilder::passive) ones skipped, tokens
    /// oldest first, each token's nets in [`Nested::nets`] order, each net
    /// stepped by its own selector and then its own inner nets walked, the
    /// first firing found is the step, and its events are in this net's
    /// log, as [`Event::Nested`], by the time this returns. `None` when no
    /// inner net has anything to fire; this net's own fireable transitions
    /// are never fired here, at this level.
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
        match self.turn_inside(&EVERY_TURN, &mut 0) {
            None => Ok(None),
            Some((_, result)) => result.map(Some),
        }
    }

    // ----- internals ------------------------------------------------------

    /// Where this net's round stands: the inner nets, by their place in
    /// walk order (counted from 0, this net itself not counted), whose turn
    /// comes before this net's own and after it (DESIGN §6.6).
    ///
    /// Under [`DeepPolicy::OuterFirst`] none come before and all after.
    /// Under [`DeepPolicy::Interleaved`] the round starts after the cursor:
    /// after this net, every inner net comes before it; after the inner net
    /// at `k`, those after `k` come before it and those up to `k` after;
    /// with no cursor, or one that names no net in the walk, the round
    /// starts here.
    pub(crate) fn round(&self) -> (Range<usize>, Range<usize>) {
        match self.deep_policy {
            DeepPolicy::OuterFirst => (NO_TURN, EVERY_TURN),
            DeepPolicy::Interleaved => match self.deep_cursor {
                Some(cursor) if cursor == self.id => (EVERY_TURN, NO_TURN),
                Some(cursor) => match self.walk_position(cursor) {
                    Some(at) => (at + 1..usize::MAX, 0..at + 1),
                    None => (NO_TURN, EVERY_TURN),
                },
                None => (NO_TURN, EVERY_TURN),
            },
        }
    }

    /// One inner firing among the inner nets whose place in walk order is in
    /// `turns`, the net it happened in recorded as the deep cursor.
    pub(crate) fn inner_turn(
        &mut self,
        turns: Range<usize>,
    ) -> Result<Option<DeepStep>, FiringFailed> {
        if turns.is_empty() {
            return Ok(None);
        }
        let Some((fired_in, result)) = self.turn_inside(&turns, &mut 0) else {
            return Ok(None);
        };
        self.deep_cursor = Some(fired_in);
        result.map(Some)
    }

    /// Where `wanted` is in walk order among the active inner nets, at any
    /// depth, or `None` if it is not there.
    fn walk_position(&self, wanted: NetId) -> Option<usize> {
        let mut at = 0;
        self.find_in_walk(wanted, &mut at).then_some(at)
    }

    /// [`walk_position`](Net::walk_position), one level: `at` counts the
    /// nets passed so far.
    fn find_in_walk(&self, wanted: NetId, at: &mut usize) -> bool {
        for (_, _, _, inner) in self.active_inner_nets() {
            if inner.id == wanted {
                return true;
            }
            *at += 1;
            if inner.find_in_walk(wanted, at) {
                return true;
            }
        }
        false
    }

    /// The walk itself: the active inner nets in walk order, numbered from
    /// `*next`, each one whose number is in `turns` stepped by its own
    /// selector and then its own inner nets walked; the first firing, or
    /// failure, ends it. Returns the id of the net it happened in and the
    /// step or the failure, wrapped and lifted level by level on the way
    /// up: the events under the place and token the token sits in (rule 1),
    /// a failure's [`path`](FiringFailed::path) filled in.
    fn turn_inside(
        &mut self,
        turns: &Range<usize>,
        next: &mut usize,
    ) -> Option<(NetId, Result<DeepStep, FiringFailed>)> {
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
                if *next >= turns.end {
                    // Every net from here on is past the last turn asked for.
                    return None;
                }
                let (token, value) = &mut self.places[index].tokens[position];
                let token = *token;
                // The first inner net that fires, with what it logged; the
                // borrow of the token ends with this block, so that the
                // events can then be written to this net's own log.
                let found =
                    (nested.nets_mut)(&mut **value)
                        .into_iter()
                        .find_map(|(name, inner)| {
                            let turn = *next;
                            *next += 1;
                            let here = if turns.contains(&turn) {
                                match inner.step() {
                                    Ok(None) => None,
                                    Ok(Some(transition)) => {
                                        Some((inner.id(), Ok(DeepStep::Here { transition })))
                                    }
                                    Err(failed) => Some((inner.id(), Err(failed))),
                                }
                            } else {
                                None
                            };
                            let (fired_in, result) = match here {
                                Some(found) => found,
                                None => inner.turn_inside(turns, next)?,
                            };
                            let nested_id = inner.id();
                            let events: Vec<Event> = inner.drain_events().collect();
                            Some((name.to_owned(), nested_id, events, fired_in, result))
                        });
                let Some((name, nested_id, events, fired_in, result)) = found else {
                    continue;
                };
                // An inner net fired, and a key may read it: the token is
                // indexed again as it is now (DESIGN §8.3).
                let slot = &self.places[index];
                if slot.meta.keys.is_some() {
                    self.index
                        .forget(&slot.meta, place, token, &*slot.tokens[position].1);
                }
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
                let result = match result {
                    Ok(step) => Ok(DeepStep::Inside {
                        place,
                        token,
                        net: name,
                        nested_id,
                        step: Box::new(step),
                    }),
                    Err(mut failed) => {
                        failed.path.insert(0, (place, token, name));
                        Err(failed)
                    }
                };
                return Some((fired_in, result));
            }
        }
        None
    }

    /// The first transition anywhere in the nesting that is fireable, with
    /// its name, in the order the next [`step_deep`](Net::step_deep) would
    /// try the nets: under [`DeepPolicy::OuterFirst`] this net's first, then
    /// the inner nets' in walk order; under [`DeepPolicy::Interleaved`] the
    /// round from the cursor. Selectors are not consulted.
    ///
    /// What the drivers' fuse blames when it trips: the transition that
    /// would have been attempted next.
    pub(crate) fn first_fireable_deep(&self) -> Option<(TransitionId, String)> {
        let first_of = |net: &Net| {
            net.fireable()
                .first()
                .map(|&transition| (transition, net.transition_name(transition)))
        };
        let (ahead, behind) = self.round();
        let mut walk = Vec::new();
        self.collect_walk(&mut walk);
        let in_turns = |turns: &Range<usize>| {
            walk.iter()
                .enumerate()
                .filter(|(turn, _)| turns.contains(turn))
                .find_map(|(_, net)| first_of(net))
        };
        in_turns(&ahead)
            .or_else(|| first_of(self))
            .or_else(|| in_turns(&behind))
    }

    /// Every active inner net at every depth, in walk order.
    fn collect_walk<'a>(&'a self, walk: &mut Vec<&'a Net>) {
        for (_, _, _, inner) in self.active_inner_nets() {
            walk.push(inner);
            inner.collect_walk(walk);
        }
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
