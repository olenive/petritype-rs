//! Nets as tokens: a token that owns Petri nets of its own (DESIGN §6.6).
//!
//! A vehicle moving between the stations of a city has a life of its own
//! inside (passengers sit down, stand, go to the door), and that life is a
//! net too. Here the vehicle is a token of the *outer* net, and the net it
//! carries is an *inner* net. The outer net moves the token like any other;
//! what is new is that the engine knows the inner net is there:
//!
//! - **Declared.** A token type says which nets it owns by implementing
//!   [`Nested`], and a place declared with
//!   [`NetBuilder::place_nested`](crate::NetBuilder::place_nested) holds such
//!   tokens. [`Net::nested`] reads a token's inner nets back.
//! - **Run.** [`Net::step_deep`] fires one transition, here or in an inner
//!   net, in the order each nested place's [`Stepping`] rule gives: the
//!   inner nets of an inner-first place (the default) before the outer net,
//!   those of a take-turns place in turn with it, those of an outer-first
//!   place once the outer net has nothing to fire, and those of a passive
//!   place never, since only an outer body steps them.
//!   [`drivers::run`](crate::drivers::run) steps that way, so running a net
//!   to quiescence runs its inner nets too; its concurrent mode lets the
//!   inner nets a rule lets go launch together, beside the outer net.
//! - **Watched.** An inner firing runs its body inside the deep step, so
//!   [`Net::step_deep_staged_with`] takes an observer that is told of each
//!   such body, as an [`InnerBody`], just before it runs: how a driver that
//!   reports what is running names a body below the net it stepped.
//! - **Reached.** A [`MutateEdge`](crate::MutateEdge) body is handed the
//!   token itself, and so the inner net, with its whole API; a
//!   [`ReadEdge`](crate::ReadEdge) body is lent it to look at; a guard reads
//!   inner markings with [`Net::nested`]; a caller that is not a body uses
//!   [`Net::nested_scope`].
//! - **Observed.** Whatever an inner net logs ends up in the outer log,
//!   wrapped in [`Event::Nested`] with the place and token it happened in,
//!   at a moment fixed by the rules on [`Event::Nested`]: *lifted*, in this
//!   crate's word. Nothing is left behind for a driver to collect.
//!
//! The coupling goes one way. An inner net never sees the outer one: its
//! guards and bodies are its own, and the outer net reaches in only through
//! the token it holds. A net carried by a token shares no places with the
//! net around it.
//!
//! A *subnet place*
//! ([`NetBuilder::subnet_place`](crate::NetBuilder::subnet_place)) is the
//! other way to nest: a place that holds one net, a subnet, for its net's
//! whole life. The subnet is just as isolated, except at its *ports*, the
//! places its definition marks with [`NetBuilder::port`](crate::NetBuilder::port),
//! which the net around it uses as its own places. In a deep step a subnet
//! is one net at its place's position, under its place's [`Stepping`] rule,
//! and what it logs is lifted as [`Event::Subnet`]. [`Net::subnet`] reads
//! one and [`Net::subnet_scope`] reaches into it.

use std::any::Any;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

use crate::closed;
use crate::error::{NetError, PathStep, StepError};
use crate::event::Event;
use crate::ids::{NetId, PlaceId, TokenId, TransitionId};
use crate::index::KeyIndex;
use crate::net::Net;
use crate::place::{ErasedToken, PlaceSlot};

/// A token type that owns Petri nets of its own, each under a name.
///
/// Implement it for a token that carries an inner net (a vehicle with a
/// cabin, a character with a body), and declare the place that holds such
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
/// same order. The order is the order the engine steps them in, and the
/// name is what [`Event::Nested::net`](Event::Nested) and
/// [`FiringFailed::path`](crate::FiringFailed::path) call each one. Names
/// need not be unique, but a reader that addresses a net by name finds the
/// first. The engine tells two nets of one name apart by their ids: the
/// drivers find the net a firing failed in, or lands in, by
/// [`FiringFailed::nested_ids`](crate::FiringFailed::nested_ids) and by the
/// id at each level of the way down (since 0.4.0).
///
/// [`Net`] implements it as one unnamed inner net, `("", net)`, so a place of
/// bare nets is a place of nested tokens with no wrapper type at all.
///
/// # Copies
///
/// A `Net` cannot be cloned (its bodies are closures), so neither can a
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
    /// tokens with.
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
    let slot = crate::net::slot_of(places, place)?;
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
        lifted.extend(inner.take_events().map(|event| Event::Nested {
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
/// the place and token it happened in and, recursively, what fired in there,
/// and [`Subnet`](DeepStep::Subnet) one inside the subnet of a subnet place.
/// The outer net's log says the same thing as an [`Event::Nested`] or an
/// [`Event::Subnet`], with the same place, token, name and id.
///
/// The enum is `#[non_exhaustive]`: match with a `_` arm, or ask
/// [`transition`](DeepStep::transition) for the innermost transition.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
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
    /// Something fired inside the subnet of one of this net's subnet
    /// places ([`NetBuilder::subnet_place`](crate::NetBuilder::subnet_place)).
    Subnet {
        /// The subnet place.
        place: PlaceId,
        /// The subnet's own [`NetId`].
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
            DeepStep::Inside { step, .. } | DeepStep::Subnet { step, .. } => step.transition(),
        }
    }
}

/// How the nets in a place take their turns beside the net around them: one
/// rule per place that holds nets, set when the net is built and changed
/// with [`Net::set_stepping`].
///
/// One deep step ([`Net::step_deep`]) of a net fires at most one transition,
/// in that net or anywhere below it. It tries three bands in this order and
/// stops at the first firing, or failure, it comes to:
///
/// 1. every net in a place whose rule is
///    [`InnerFirst`](Stepping::InnerFirst), in walk order: a deep step of
///    that net;
/// 2. the *ring*: the net itself and every net in a place whose rule is
///    [`TakeTurns`](Stepping::TakeTurns), in walk order, starting after the
///    member that fired last and going round once. The net's own turn is one
///    firing chosen by its own [selector](crate::Selector); any other
///    member's turn is a deep step of that net;
/// 3. every net in a place whose rule is
///    [`OuterFirst`](Stepping::OuterFirst), in walk order: a deep step of
///    that net.
///
/// A [`Passive`](Stepping::Passive) place is in no band. *Walk order* is
/// places in declaration order, a place's tokens oldest first, a token's
/// nets in [`Nested::nets`] order; a
/// [subnet place](crate::NetBuilder::subnet_place) counts as one net at its
/// place's position. A member's turn is a deep step of its
/// own, so every level follows the rules of its own places, all the way
/// down, and one deep step still fires at most one transition in the whole
/// tree, whatever the rules.
///
/// The ring's *cursor* is the member that fired last: the net's own
/// [`NetId`], or a take-turns net's nested id. A turn in the ring moves it,
/// a failed firing as well as one that landed; a firing in the other two
/// bands leaves it where it is. Each net keeps its own, in memory only, so
/// the round starts at the net itself in a fresh net, one restored from JSON
/// or forked (`Net::fork`, with the `serde` feature), and when the member it
/// names has gone, its token consumed or moved to a place with another rule.
///
/// The rule is part of the net's definition: set with
/// [`NetBuilder::stepping`](crate::NetBuilder::stepping), changed with
/// [`Net::set_stepping`], read with [`Net::stepping`] and
/// [`PlaceInfo::stepping`](crate::PlaceInfo::stepping). A fork copies it and
/// a saved net does not carry it. [`Net::is_quiescent_deep`] depends only on
/// which places are passive. The sequential driver
/// ([`drivers`](crate::drivers)) and the protocol runner step in the same
/// order. The concurrent driver keeps each rule's meaning and lets the nets
/// a band lets go launch together, at any depth (since 0.4.0): every
/// inner-first net, until each is at rest; then the net's own sweep and
/// every take-turns net, each in every pass; then, once those are at rest
/// and nothing of the net's own is in flight, every outer-first net. With
/// the `serde` feature it
/// is written as the protocol names it: `"inner_first"`, `"take_turns"`,
/// `"outer_first"` or `"passive"`.
///
/// ```
/// use petritype::{DeepStep, NetBuilder, Stepping, TakeEdge};
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
/// b.stepping(desk, Stepping::TakeTurns);
/// let mut net = b.build()?;
///
/// assert!(matches!(net.step_deep()?, Some(DeepStep::Here { .. })), "the outer net's turn");
/// assert!(matches!(net.step_deep()?, Some(DeepStep::Inside { .. })), "the inner net's");
/// assert!(matches!(net.step_deep()?, Some(DeepStep::Here { .. })), "and round again");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize),
    serde(rename_all = "snake_case")
)]
#[non_exhaustive]
pub enum Stepping {
    /// The nets in this place fire until they are at rest before the net
    /// around them fires: every deep step goes to them first. The default.
    /// A net here that never comes to rest keeps the net around it from
    /// firing at all; a place for such a net takes turns, or is passive and
    /// stepped by a body.
    #[default]
    InnerFirst,
    /// The net around them and each net in this place fire in turn, one
    /// firing each, round the ring, so no live member waits longer than one
    /// round however busy the others are.
    TakeTurns,
    /// The nets in this place fire only when the net around them and its
    /// turn-takers have nothing to fire, and nothing of the net around them
    /// is in flight. An outer net that can always fire starves them, so a
    /// net meant to run this way gives its transitions guards that wait on
    /// what is inside, as the `transit` example's city waits on where each
    /// vehicle's doors are.
    OuterFirst,
    /// Deep stepping never enters this place, and its nets do not count
    /// against [`Net::is_quiescent_deep`]: an outer body steps them, through
    /// a [`MutateEdge`](crate::MutateEdge) on the place
    /// (`vehicle.inner.step()`, or [`drivers::run`](crate::drivers::run) on
    /// it), or the program does, with [`Net::nested_scope`]. What they do is
    /// reported in the outer log as for any reach inside.
    Passive,
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
    /// An inner net fired, inline, as in [`Net::step_deep`]: a
    /// [`DeepStep::Inside`] or a [`DeepStep::Subnet`], never a
    /// [`DeepStep::Here`], its events already in this net's log.
    Inside(DeepStep),
}

/// An inner body that a deep step is about to run inline: what
/// [`Net::step_deep_staged_with`] tells its observer, once for each such
/// body, just before it runs.
///
/// It names the firing as the step names it once the body has run: the
/// transition, in the net it is in, and the way down to that net from the
/// net that was stepped, one level per [`PathStep`], each with the nested id
/// of the net it leads to. `path` is the
/// [`FiringFailed::path`](crate::FiringFailed::path) a failure of this body
/// comes back with, and the last of `nested_ids` is the nested id
/// of the innermost [`DeepStep::Inside`] or [`DeepStep::Subnet`] of the
/// step's result. A firing of the net that was stepped is never an inner
/// body, so `path` is never empty.
///
/// The struct is `#[non_exhaustive]`: read its fields, and build none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct InnerBody<'a> {
    /// The transition whose body is about to run, by its id in the net it
    /// is in.
    pub transition: TransitionId,
    /// That transition's name in its net.
    pub name: &'a str,
    /// The way down from the net that was stepped, outer to inner.
    pub path: &'a [PathStep],
    /// The nested id of the net each step of `path` leads to, in the same
    /// order.
    pub nested_ids: &'a [NetId],
}

impl InnerBody<'_> {
    /// The nested id of the net the transition is in: the last of
    /// [`nested_ids`](InnerBody::nested_ids).
    pub fn nested_id(&self) -> NetId {
        *self
            .nested_ids
            .last()
            .expect("an inner body is at least one level down")
    }
}

/// What a watched deep step carries down the levels: the observer, and the
/// way down to the net whose turn it is, a level added as a member's turn
/// goes in and taken off as it comes out.
struct Watch<'w> {
    before_body: &'w mut dyn FnMut(&InnerBody<'_>),
    path: Vec<PathStep>,
    nested_ids: Vec<NetId>,
}

impl Watch<'_> {
    /// Tells the observer that `transition`, called `name`, of the net this
    /// watch is down at, is about to run its body.
    fn before_body(&mut self, transition: TransitionId, name: &str) {
        debug_assert!(
            !self.path.is_empty(),
            "only an inner body is told of, and an inner body is at least one level down"
        );
        (self.before_body)(&InnerBody {
            transition,
            name,
            path: &self.path,
            nested_ids: &self.nested_ids,
        });
    }

    /// The turn of the net one level down, through `step` to the net
    /// `nested_id`: `turn` with that level on the way down for as long as
    /// it runs.
    fn down<R>(
        &mut self,
        step: PathStep,
        nested_id: NetId,
        turn: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.path.push(step);
        self.nested_ids.push(nested_id);
        let result = turn(self);
        self.path.pop();
        self.nested_ids.pop();
        result
    }
}

/// One of the two parts of a net's round on either side of its own turn
/// ([`Stepping`]): what the staged step, the drivers and the fuse take in
/// turn around the net's own choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Part {
    /// The turns before the net's own: its inner-first places, then the
    /// ring's members after the cursor.
    Ahead,
    /// The turns after it: the rest of the ring, then the outer-first
    /// places.
    Behind,
}

/// One net in a place of a net, as a deep step of that net comes to it: a
/// *member* of its round.
///
/// Where it is, by position, holds for the deep step it was found in:
/// nothing out here moves until something fires, and the step ends there.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Member {
    /// Its place's index.
    pub(crate) place: usize,
    /// Where in the place it is.
    pub(crate) at: At,
    /// Its own id, the nested id: what the ring's cursor records.
    pub(crate) id: NetId,
    /// Its place's rule.
    pub(crate) stepping: Stepping,
}

/// Where a [`Member`] is in its place.
#[derive(Debug, Clone, Copy)]
pub(crate) enum At {
    /// One of the nets of a token.
    Token {
        /// Where the token stands in the place, oldest first.
        position: usize,
        /// The token's id, which a concurrent pass finds it by once the
        /// place around it has moved.
        token: TokenId,
        /// Which of the token's nets it is, in [`Nested::nets`] order.
        net: usize,
    },
    /// The place's subnet.
    Subnet,
}

impl Net {
    /// The inner nets of one token of a nested place, each with its name, in
    /// the order [`Nested::nets`] lists them.
    ///
    /// `None` if the place is not nested
    /// ([`place_nested`](crate::NetBuilder::place_nested)), or does not
    /// exist, or holds no such token. This is how a
    /// [guard](crate::TransitionBuilder::guard) reads inside a token. The
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
    /// The reach for a caller that is not a body (a driver that injects
    /// into an inner net, a test that sets one up, an inspector applying an
    /// edit), with the same reporting a body gets: each event `f` caused
    /// inside comes out here as an [`Event::Nested`], under this place and
    /// token, and an `f` that changes nothing emits nothing. Deeper levels
    /// are reached by calling `nested_scope` on an inner net inside `f`;
    /// each level lifts into the one above it as its call returns.
    ///
    /// `None`, with `f` not called, if the place is not nested or holds no
    /// such token.
    ///
    /// Each net it lends is under its own rule: unless it is declared open
    /// it is [closed](Net::is_closed), and `f`, being outside it, writes
    /// tokens only at its ports. Called on a closed net, or on one lent
    /// from inside a closed net, it lends them *shielded*: they take no
    /// token and no import from `f` at all, ports included, since from
    /// outside a closed net nothing inside it is written
    /// ([`ClosedRefusal::Around`](crate::ClosedRefusal::Around)); called
    /// on a window, the subnet behind it shields them when it is closed.
    /// Called by a handler of this net's own control, it shields nothing by
    /// this net, the handler being inside it. Called on a net that a body
    /// holds as a token, it lends the nets with the body's mark for `f`'s
    /// time, so that their knobs and firing by name are refused to the body
    /// too, at any depth, and a closed one takes no token from it. Whatever
    /// `f` does, panics included, the marks and the shield are gone when
    /// the call ends.
    ///
    /// A panic of `f` goes on to the caller after the scope has done what
    /// it does when `f` returns (since 2026-11-28): the token is indexed
    /// again and its nets' events are lifted, so what `f` did before it
    /// panicked is in the key index and in this net's log.
    ///
    /// ```
    /// use petritype::{Event, NetBuilder};
    ///
    /// let mut inner = NetBuilder::new();
    /// let cargo = inner.place::<u32>("Cargo");
    /// inner.port(cargo);
    /// let inner = inner.build()?;
    ///
    /// let mut b = NetBuilder::new();
    /// let yard = b.place_nested::<petritype::Net>("Yard");
    /// b.seed(yard, [inner]);
    /// b.open(); // a closed yard would shield the nets inside it
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
        let window = self.places.get(place.0 as usize)?.meta.window;
        // A body that holds this net holds the nets it lends too, and a
        // closed net shields them, for the closure's time (DESIGN §6.9).
        let lending = self.lending(window);
        let (result, lifted) = {
            let (holder, at) = self.holder_mut(place);
            let slot = &mut holder.places[at.0 as usize];
            let nested = slot.meta.nested?;
            let index = slot.position(token)?;
            // A key may read the inner nets: the token's keys as they are
            // before `f`, for the index to follow what it changed (DESIGN
            // §8.3).
            let before = KeyIndex::before(&slot.meta, &*slot.tokens[index].1);
            let value = &mut slot.tokens[index].1;
            let found = lending.lend_all(nested, value);
            let result = catch_unwind(AssertUnwindSafe(|| f(&mut (nested.nets_mut)(&mut **value))));
            closed::Lending::restore_all(nested, &mut slot.tokens[index].1, found);
            // What `f` did before a panic is done all the same, so the
            // token is indexed and its nets' events lifted on that path too
            // (since 2026-11-28), and the panic goes on after them.
            holder
                .index
                .changed(&slot.meta, at, token, before, &*slot.tokens[index].1);
            let value = &mut slot.tokens[index].1;
            (result, lift_from(nested, value, place, token))
        };
        drop(lending);
        self.emit_all(lifted);
        Some(result.unwrap_or_else(|payload| resume_unwind(payload)))
    }

    /// The subnet of a subnet place
    /// ([`subnet_place`](crate::NetBuilder::subnet_place)), to read: its
    /// places, its marking, and its ports as the subnet sees them, which is
    /// as this net sees them through
    /// [`port_of`](crate::NetBuilder::port_of)'s handles.
    ///
    /// `None` for a place that holds no subnet, or that this net does not
    /// have.
    ///
    /// ```
    /// use petritype::NetBuilder;
    ///
    /// let mut b = NetBuilder::new();
    /// let lobby = b.place::<String>("Lobby");
    /// b.port(lobby);
    /// let level = b.build()?;
    ///
    /// let mut b = NetBuilder::new();
    /// let level_3 = b.subnet_place("Level 3", level);
    /// let door = b.port_of::<String>(level_3, "Lobby");
    /// b.open(); // so that the program may put a hero at the door
    /// let mut world = b.build()?;
    ///
    /// let ada = world.put(door, "Ada".to_owned());
    /// let inside = world.subnet(level_3.id()).expect("a subnet place");
    /// assert_eq!(inside.tokens(lobby.id()).next().map(|(id, _)| id), Some(ada), "one queue, one id");
    /// # Ok::<(), petritype::BuildError>(())
    /// ```
    pub fn subnet(&self, place: PlaceId) -> Option<&Net> {
        self.places
            .get(place.0 as usize)?
            .subnet
            .as_deref()
            .map(|subnet| &subnet.net)
    }

    /// Calls `f` with the subnet of a subnet place, mutably, and lifts
    /// whatever it logged into this net's log when it returns, each event
    /// wrapped in an [`Event::Subnet`].
    ///
    /// The reach for the program into a subnet, to step it or change it, as
    /// [`nested_scope`](Net::nested_scope) is into a token's nets. The
    /// subnet's own handles work inside `f`, its ports included, and deeper
    /// levels are reached by calling `subnet_scope` or `nested_scope` on the
    /// subnet; each level lifts into the one above it as its call returns.
    ///
    /// The subnet is under its own rule: it is [closed](Net::is_closed)
    /// unless it declares itself open, and `f`, being outside it, writes
    /// tokens only at its ports. Called on a closed net, or on one lent
    /// from inside a closed net, it lends the subnet shielded, as
    /// [`nested_scope`](Net::nested_scope) does: it takes no token and no
    /// import from `f`, ports included. Its parameters, switches and other
    /// knobs stay the program's. Called on a net that a body holds as a
    /// token, it lends the subnet with the body's mark for `f`'s time, as
    /// `nested_scope` does, and a closed subnet takes no token from the
    /// body.
    ///
    /// ```
    /// use petritype::{Event, NetBuilder};
    ///
    /// let mut b = NetBuilder::new();
    /// let lobby = b.place::<String>("Lobby");
    /// b.port(lobby);
    /// let level = b.build()?;
    ///
    /// let mut b = NetBuilder::new();
    /// let level_3 = b.subnet_place("Level 3", level);
    /// let door = b.port_of::<String>(level_3, "Lobby");
    /// b.open(); // a closed world would shield its levels from the program
    /// let mut world = b.build()?;
    ///
    /// let ada = world.subnet_scope(level_3.id(), |level| level.put(lobby, "Ada".to_owned()))?;
    /// assert_eq!(world.count(door.id()), 1, "the same queue, seen from out here");
    /// assert!(matches!(
    ///     &world.drain_events().collect::<Vec<_>>()[..],
    ///     [Event::Subnet { place, event, .. }]
    ///         if *place == level_3.id() && **event == Event::Added { place: lobby.id(), token: ada }
    /// ));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// With `f` not called: [`NetError::NoSuchPlace`] for a place this net
    /// does not have; [`NetError::NotSubnet`] for one that holds no subnet;
    /// and [`NetError::InFlight`] while a transition of this net with an
    /// edge on one of the subnet's ports is in flight, naming the first:
    /// the subnet is held still until that firing lands, as it is in a deep
    /// step, so that one firing happens at a time across the levels.
    ///
    /// # Panics
    ///
    /// If `f` puts another net in the subnet's place: a subnet place holds
    /// the one subnet it was built with for its net's whole life, and this
    /// net's handles to its ports name that subnet's places.
    ///
    /// A panic of `f` goes on to the caller after the scope has done what
    /// it does when `f` returns (since 2026-11-28): the marks and the
    /// shield are gone, the subnet is checked, and its events are lifted,
    /// so what `f` did before it panicked is in this net's log. An `f` that
    /// panicked after putting another net in place panics with the message
    /// above instead.
    pub fn subnet_scope<R>(
        &mut self,
        place: PlaceId,
        f: impl FnOnce(&mut Net) -> R,
    ) -> Result<R, NetError> {
        let slot = self
            .places
            .get(place.0 as usize)
            .ok_or(NetError::NoSuchPlace { place })?;
        let Some(subnet) = slot.subnet.as_deref() else {
            return Err(NetError::NotSubnet {
                place,
                name: slot.meta.name.clone(),
            });
        };
        if let Some(transition) = self.reached_in_flight(subnet) {
            return Err(NetError::InFlight {
                transition,
                name: self.transition_name(transition),
                path: Vec::new(),
            });
        }
        // A body that holds this net holds its subnet too, and a closed net
        // shields it, for the closure's time (DESIGN §6.9).
        let lending = self.lending(None);
        let subnet = &mut self.places[place.0 as usize]
            .subnet
            .as_deref_mut()
            .expect("the place was just found to hold a subnet")
            .net;
        let nested_id = subnet.id();
        let found = lending.lend(subnet);
        let result = catch_unwind(AssertUnwindSafe(|| f(&mut *subnet)));
        closed::Lending::restore(subnet, found);
        drop(lending);
        // What `f` did before a panic is done all the same: the subnet is
        // checked and its events lifted on that path too (since
        // 2026-11-28), and the panic goes on after them.
        assert_eq!(
            subnet.id(),
            nested_id,
            "subnet_scope: the closure put another net in the place of the subnet of {place:?}, \
             which holds its subnet for its net's whole life",
        );
        let lifted: Vec<Event> = subnet
            .take_events()
            .map(|event| Event::Subnet {
                place,
                nested_id,
                event: Box::new(event),
            })
            .collect();
        self.emit_all(lifted);
        Ok(result.unwrap_or_else(|payload| resume_unwind(payload)))
    }

    /// Fires one transition, here or in an inner net, in the order the
    /// [`Stepping`] rules of this net's nested places give: the inner nets
    /// of inner-first places (the default), then this net and the inner nets
    /// of take-turns places in turn, then the inner nets of outer-first
    /// places. Passive places are left alone. An inner net's turn is a deep
    /// step of that net, so each level follows the rules of its own places,
    /// and the first firing found is the step: at most one transition fires,
    /// in the whole tree.
    ///
    /// This net's own turn is [`step`](Net::step), selector and all. Called
    /// in a loop, which is what [`drivers::run`](crate::drivers::run) does,
    /// it runs the net and everything in it until all of it is at rest.
    /// Deterministic, given the selectors.
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
    /// let inbox = b.place::<u32>("Inbox");
    /// let outbox = b.place::<u32>("Outbox");
    /// b.seed(desk, [inner]);
    /// b.seed(inbox, [7]);
    /// let forward = b.transition("Forward", (TakeEdge(inbox),), (outbox,), |n: u32| (n,)).id();
    /// let mut net = b.build()?;
    ///
    /// // `Desk` is inner first, the default: its net goes before `Forward`.
    /// let step = net.step_deep()?.expect("the inner net had work");
    /// assert!(matches!(&step, DeepStep::Inside { place, .. } if *place == desk.id()));
    /// assert_eq!(step.transition(), work);
    /// assert_eq!(net.step_deep()?, Some(DeepStep::Here { transition: forward }));
    /// assert!(net.is_quiescent_deep());
    /// assert_eq!(net.step_deep()?, None);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// [`StepError::FiringFailed`] with the
    /// [`FiringFailed`](crate::FiringFailed) of a failed firing, wherever it
    /// was. One that failed inside says where in
    /// [`path`](crate::FiringFailed::path), outer to inner; its transition
    /// id is the inner net's. The failure's own [`Failed`](Event::Failed)
    /// event is lifted, like any other. [`StepError::SelectionFailed`] when
    /// a net whose turn it was could not choose (since 2026-11-28), as
    /// [`choose`](Net::choose) says, its
    /// [`path`](crate::SelectionFailed::path) saying which net; nothing
    /// fired.
    pub fn step_deep(&mut self) -> Result<Option<DeepStep>, StepError> {
        self.step_deep_watched(None)
    }

    /// [`step_deep`](Net::step_deep), telling `watch`, when there is one, of
    /// this net's own firing just before its body runs: a net that is
    /// stepped with a watch is always one level down or more.
    fn step_deep_watched(
        &mut self,
        mut watch: Option<&mut Watch<'_>>,
    ) -> Result<Option<DeepStep>, StepError> {
        Ok(match self.staged(watch.as_deref_mut())? {
            None => None,
            Some(StagedStep::Inside(step)) => Some(step),
            Some(StagedStep::Chosen(transition)) => {
                if let Some(watch) = watch {
                    watch.before_body(transition, &self.transitions[transition.0 as usize].name);
                }
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
    /// For a driver whose outer bodies run elsewhere ([`launch`](Net::launch)
    /// here, [`run`](crate::Firing::run) on a thread of its own,
    /// [`distribute`](Net::distribute) here) and whose inner firings are
    /// inline, as the protocol runner (`petritype-runner`) does for `step`
    /// and a running net. A deep step has three parts, and this takes all
    /// three in order: the turns ahead of this net's own (the inner-first
    /// places, then the ring's members before this net), this net's own
    /// choice, and the turns behind it (the ring's members after this net,
    /// then the outer-first places); see [`Stepping`]. The choice counts,
    /// for the [round robin](crate::TieBreak::RoundRobin) as
    /// [`choose`](Net::choose)'s does and for the ring as this net's turn, so
    /// launch it.
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
    /// An inner failure's [`FiringFailed`](crate::FiringFailed), as from
    /// `step_deep`, its [`path`](crate::FiringFailed::path) saying where.
    /// Nothing of this net's own fires here, so nothing of its own fails. A
    /// choice that cannot be made, this net's own or an inner net's, as from
    /// `step_deep` ([`StepError::SelectionFailed`], since 2026-11-28).
    ///
    /// [`StepError::HeldByBody`] when a body of the net around this one
    /// holds it, nothing stepped (since 0.4.0; from 2026-11-28 until then
    /// the step panicked with the same message): the step leaves a choice
    /// to [`launch`](Net::launch) and [`distribute`](Net::distribute), which
    /// are the program's on a net a body holds, so the body's firing fails
    /// with [`NetError::HeldByBody`](crate::NetError::HeldByBody) too. A
    /// body steps a net it holds with [`step_deep`](Net::step_deep), or
    /// drives it with [`drivers::run`](crate::drivers::run) in either mode.
    pub fn step_deep_staged(&mut self) -> Result<Option<StagedStep>, StepError> {
        self.admit_step(closed::STAGE_STEP)?;
        self.staged(None)
    }

    /// [`step_deep_staged`](Net::step_deep_staged), with `before_body`
    /// called just before an inner body runs, to say which: for a driver
    /// that reports what is running while it runs.
    ///
    /// The step is `step_deep_staged`'s in every way: the same choice, the
    /// same firing, the same result and the same events, whatever
    /// `before_body` does. When the step fires inside an inner net, which
    /// runs the body inline before this returns, `before_body` is called
    /// once, before that body starts, with an [`InnerBody`] naming the
    /// transition and the way down to its net; when the body has run, the
    /// step comes back as `StagedStep::Inside`, or as the body's failure,
    /// as it would have. It is not called when the turn falls to this net
    /// (`StagedStep::Chosen`: nothing of this net's own fires here) or when
    /// nothing fires, so it is called once or not at all.
    ///
    /// It is told of the bodies the deep step runs and of no others. A body
    /// that steps the nets inside the tokens it holds (a
    /// [`MutateEdge`](crate::MutateEdge) body calling `vehicle.inner.step()`)
    /// fires those as part of its own run, and they are not told of, nor is
    /// anything a control's handler fires during
    /// [`Net::press`](crate::Net::press), which takes no observer. The step
    /// reads no clock for it: a caller that wants to know when the body
    /// started reads its own clock in `before_body`.
    ///
    /// ```
    /// use petritype::{NetBuilder, PathStep, StagedStep, TakeEdge};
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
    /// let (token, _) = net.tokens(desk.id()).next().unwrap();
    /// let cabin = net.nested(desk.id(), token).unwrap()[0].1.id();
    ///
    /// let mut told = Vec::new();
    /// let step = net.step_deep_staged_with(|body| {
    ///     told.push((body.name.to_owned(), body.path.to_vec(), body.nested_id()));
    /// })?;
    /// assert!(matches!(step, Some(StagedStep::Inside(ref s)) if s.transition() == work));
    /// let way_down = vec![PathStep::Token { place: desk.id(), token, net: String::new() }];
    /// assert_eq!(told, [("Work".to_owned(), way_down, cabin)], "told once, before Work ran");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// As [`step_deep_staged`](Net::step_deep_staged)'s: an inner failure's
    /// [`FiringFailed`](crate::FiringFailed), its
    /// [`path`](crate::FiringFailed::path) the `path` that `before_body` was
    /// given for the body that failed, or a choice that cannot be made
    /// ([`StepError::SelectionFailed`]). [`StepError::HeldByBody`] when a
    /// body of the net around this one holds it, as for `step_deep_staged`,
    /// `before_body` never called.
    pub fn step_deep_staged_with(
        &mut self,
        mut before_body: impl FnMut(&InnerBody<'_>),
    ) -> Result<Option<StagedStep>, StepError> {
        self.admit_step(closed::STAGE_STEP)?;
        let mut watch = Watch {
            before_body: &mut before_body,
            path: Vec::new(),
            nested_ids: Vec::new(),
        };
        self.staged(Some(&mut watch))
    }

    /// The three parts of a staged deep step, with the watch, when there is
    /// one, carried into every member's turn.
    fn staged(
        &mut self,
        mut watch: Option<&mut Watch<'_>>,
    ) -> Result<Option<StagedStep>, StepError> {
        // One walk for both parts: a turn that fires nothing changes
        // nothing, and neither does a choice of none, so the members found
        // for the turns ahead are where they were for the turns behind.
        let members = self.members();
        if let Some(step) = self.take_turn_among(&members, Part::Ahead, watch.as_deref_mut())? {
            return Ok(Some(StagedStep::Inside(step)));
        }
        if let Some(transition) = self.choose_now()? {
            self.took_own_turn();
            return Ok(Some(StagedStep::Chosen(transition)));
        }
        Ok(self
            .take_turn_among(&members, Part::Behind, watch)?
            .map(StagedStep::Inside))
    }

    /// Changes the [`Stepping`] rule of a place that holds nets. Takes effect
    /// from the next deep step, and leaves the ring's cursor where it is.
    ///
    /// ```
    /// use petritype::{NetBuilder, Stepping};
    ///
    /// let mut b = NetBuilder::new();
    /// let desk = b.place_nested::<petritype::Net>("Desk");
    /// let inbox = b.place::<u32>("Inbox");
    /// let mut net = b.build()?;
    ///
    /// assert_eq!(net.stepping(desk.id()), Some(Stepping::InnerFirst), "the default");
    /// net.set_stepping(desk.id(), Stepping::Passive)?;
    /// assert_eq!(net.stepping(desk.id()), Some(Stepping::Passive));
    /// assert!(net.set_stepping(inbox.id(), Stepping::Passive).is_err(), "Inbox holds no nets");
    /// assert_eq!(net.stepping(inbox.id()), None);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// [`NetError::NoSuchPlace`] for a place this net does not have, and
    /// [`NetError::NotNested`] for one that holds no nets, which has no rule
    /// to change. [`NetError::HeldByBody`] when a body of the net around
    /// this one holds it: its stepping rules are the program's.
    pub fn set_stepping(&mut self, place: PlaceId, stepping: Stepping) -> Result<(), NetError> {
        self.admit_knob(closed::SET_STEPPING)?;
        let slot = self
            .places
            .get_mut(place.0 as usize)
            .ok_or(NetError::NoSuchPlace { place })?;
        if !slot.meta.holds_nets() {
            return Err(NetError::NotNested {
                place,
                name: slot.meta.name.clone(),
            });
        }
        slot.meta.stepping = stepping;
        Ok(())
    }

    /// The [`Stepping`] rule of a place that holds nets, as built or as last
    /// set with [`set_stepping`](Net::set_stepping); `None` for a place that
    /// holds none, or that this net does not have.
    pub fn stepping(&self, place: PlaceId) -> Option<Stepping> {
        let meta = &self.places.get(place.0 as usize)?.meta;
        meta.holds_nets().then_some(meta.stepping)
    }

    /// Whether nothing can happen here or below: nothing fireable and nothing
    /// in flight in this net, and every inner net of every token in a
    /// nested place, and the subnet of every subnet place, that is not
    /// [passive](Stepping::Passive) quiescent in the same deep sense.
    ///
    /// [`fireable`](Net::fireable) stays about this net alone. A passive
    /// place's inner nets are left out because nothing but an outer body
    /// will ever step them. The other three rules decide only the order of
    /// the firings, so they count alike here.
    pub fn is_quiescent_deep(&self) -> bool {
        self.fireable().is_empty()
            && self.in_flight().is_empty()
            && self
                .members()
                .into_iter()
                .all(|member| self.member(member).is_none_or(Net::is_quiescent_deep))
    }

    /// A deep step without this net's own turn: the turns ahead of it and
    /// the turns behind it, in that order ([`Stepping`]), and never one of
    /// this net's own transitions.
    ///
    /// For a caller that has already asked this net's selector. A driver
    /// that runs this net's own firings in stages ([`select`](Net::select),
    /// then [`launch`](Net::launch) with the body on a thread of its own,
    /// then [`distribute`](Net::distribute)) and wants the inner nets when
    /// that selector picks nothing calls this, where
    /// [`step_deep`](Net::step_deep) would ask the selector a second time
    /// and fire inline whatever it chose. The turns are
    /// [`step_deep_staged`](Net::step_deep_staged)'s, and a turn in the ring
    /// moves its cursor as it does there; a driver that wants this net's own
    /// choice in its place in the round steps with `step_deep_staged`
    /// instead, as the protocol runner (`petritype-runner`) does.
    ///
    /// The lifting is `step_deep`'s: the first firing found is the step, and
    /// its events are in this net's log, as [`Event::Nested`], by the time
    /// this returns. `None` when no inner net that has a turn has anything
    /// to fire.
    ///
    /// ```
    /// use petritype::{DeepStep, NetBuilder, Stepping, TakeEdge};
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
    /// b.stepping(desk, Stepping::OuterFirst);
    /// let mut net = b.build()?;
    ///
    /// // `Forward` is fireable out here, and `Desk` is outer first, and
    /// // still the inner net goes: this net's own turn is not taken.
    /// let step = net.step_inside()?.expect("the inner net had work");
    /// assert!(matches!(step, DeepStep::Inside { place, .. } if place == desk.id()));
    /// assert_eq!(net.count(inbox.id()), 1, "the outer net was not fired");
    /// assert_eq!(net.step_inside()?, None, "and the inner net is done");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// The inner failure's [`FiringFailed`](crate::FiringFailed), as from
    /// `step_deep`, its [`path`](crate::FiringFailed::path) saying where, or
    /// an inner net's choice that cannot be made
    /// ([`StepError::SelectionFailed`]).
    ///
    /// [`StepError::HeldByBody`] when a body of the net around this one
    /// holds it, nothing stepped (since 0.4.0; until then the step
    /// panicked with the same message): a step that leaves out the net's
    /// own turn is the program's, and the body's firing fails with
    /// [`NetError::HeldByBody`] too. A body steps a net it holds with
    /// [`step`](Net::step) or [`step_deep`](Net::step_deep).
    pub fn step_inside(&mut self) -> Result<Option<DeepStep>, StepError> {
        self.admit_step(closed::STEP_INSIDE)?;
        if let Some(step) = self.take_turn(Part::Ahead)? {
            return Ok(Some(step));
        }
        self.take_turn(Part::Behind)
    }

    // ----- internals ------------------------------------------------------

    /// Records that this net took its own turn in the ring, so that the
    /// next round starts after it. [`step_deep_staged`](Net::step_deep_staged)
    /// calls it for a choice, and the concurrent driver for a launch sweep
    /// that launched something, which is that driver's way of taking this
    /// net's turn.
    pub(crate) fn took_own_turn(&mut self) {
        self.ring_cursor = Some(self.id);
    }

    /// One firing among the turns of one part of this net's round, tried in
    /// order: the first member whose deep step fires, or fails, is the step,
    /// and a member of the ring that does becomes the cursor. A member that
    /// could not choose is the step too, its [`StepError::SelectionFailed`]
    /// the result, and the cursor stays: it fired nothing. `None` when none
    /// of them has anything to fire.
    fn take_turn(&mut self, part: Part) -> Result<Option<DeepStep>, StepError> {
        self.take_turn_watched(part, None)
    }

    /// [`take_turn`](Net::take_turn), with the watch, when there is one,
    /// carried into the member's turn.
    fn take_turn_watched(
        &mut self,
        part: Part,
        watch: Option<&mut Watch<'_>>,
    ) -> Result<Option<DeepStep>, StepError> {
        let members = self.members();
        self.take_turn_among(&members, part, watch)
    }

    /// [`take_turn_watched`](Net::take_turn_watched), among `members`, this
    /// net's [`members`](Net::members) as they stand.
    fn take_turn_among(
        &mut self,
        members: &[Member],
        part: Part,
        mut watch: Option<&mut Watch<'_>>,
    ) -> Result<Option<DeepStep>, StepError> {
        for member in self.turns_among(members, part) {
            let Some(result) = self.member_turn(member, watch.as_deref_mut()) else {
                continue;
            };
            let fired = !matches!(result, Err(StepError::SelectionFailed(_)));
            if fired && member.stepping == Stepping::TakeTurns {
                self.ring_cursor = Some(member.id);
            }
            return result.map(Some);
        }
        Ok(None)
    }

    /// The members whose turn is in `part` of this net's round, in the order
    /// they are tried ([`Stepping`]).
    ///
    /// The ring is this net and its take-turns members, in walk order, and
    /// the round enters it after the cursor: after this net, every
    /// take-turns member comes before this net's own turn; after the member
    /// at `k`, those after `k` come before it and those up to `k` after it;
    /// with no cursor, or one that names no member, all of them come after
    /// it. The outer-first members follow, while nothing of this net is in
    /// flight: until a body out here lands, this net may still have
    /// something to fire.
    fn turns(&self, part: Part) -> Vec<Member> {
        self.turns_among(&self.members(), part)
    }

    /// [`turns`](Net::turns), among `members`, this net's
    /// [`members`](Net::members) as they stand.
    fn turns_among(&self, members: &[Member], part: Part) -> Vec<Member> {
        let band = |stepping: Stepping| {
            members
                .iter()
                .copied()
                .filter(move |member| member.stepping == stepping)
        };
        let ring: Vec<Member> = band(Stepping::TakeTurns).collect();
        let start = self.ring_start(&ring);
        // A subnet held still by a firing in flight keeps its place in the
        // ring, so the cursor finds it, but takes no turn.
        let free = |member: &Member| !self.held(member);
        match part {
            Part::Ahead => band(Stepping::InnerFirst)
                .chain(ring[start..].iter().copied())
                .filter(free)
                .collect(),
            Part::Behind => {
                let settled = self.in_flight().is_empty();
                ring[..start]
                    .iter()
                    .copied()
                    .chain(band(Stepping::OuterFirst).filter(|_| settled))
                    .filter(free)
                    .collect()
            }
        }
    }

    /// Where the round enters the ring, the take-turns members in walk
    /// order: at 0 after this net, after the member the cursor names, and
    /// past the end, so that every member comes after this net's own turn,
    /// with no cursor or one that names no member.
    pub(crate) fn ring_start(&self, ring: &[Member]) -> usize {
        match self.ring_cursor {
            Some(cursor) if cursor == self.id => 0,
            Some(cursor) => ring
                .iter()
                .position(|member| member.id == cursor)
                .map_or(ring.len(), |at| at + 1),
            None => ring.len(),
        }
    }

    /// Every net in a place of this net that holds nets and is not passive,
    /// in walk order (places in declaration order, a place's tokens oldest
    /// first, a token's nets in [`Nested::nets`] order, a subnet place's
    /// subnet at its place), each with where it is and its place's rule. The
    /// one walk every part of deep stepping goes through.
    pub(crate) fn members(&self) -> Vec<Member> {
        let mut members = Vec::new();
        if !self.holds_nets {
            return members;
        }
        for (place, slot) in self.places.iter().enumerate() {
            let meta = &slot.meta;
            if meta.nested.is_none() && !meta.subnet {
                continue;
            }
            // A window's tokens are its subnet's to step, so only a place
            // that holds nets of this net's own counts.
            let stepping = meta.stepping;
            if meta.window.is_some() || stepping == Stepping::Passive {
                continue;
            }
            // A subnet place: one net, at its place's position.
            if let Some(subnet) = &slot.subnet {
                members.push(Member {
                    place,
                    at: At::Subnet,
                    id: subnet.net.id,
                    stepping,
                });
                continue;
            }
            let Some(nested) = slot.meta.nested else {
                continue;
            };
            for (position, (token, value)) in slot.tokens.iter().enumerate() {
                let value: &dyn Any = &**value;
                for (net, (_, inner)) in (nested.nets)(value).into_iter().enumerate() {
                    members.push(Member {
                        place,
                        at: At::Token {
                            position,
                            token: *token,
                            net,
                        },
                        id: inner.id,
                        stepping,
                    });
                }
            }
        }
        members
    }

    /// Whether a member is held still: a subnet that one of this net's
    /// transitions in flight reaches through a port. It takes no turn until
    /// that firing lands, so that one firing happens at a time across the
    /// levels.
    pub(crate) fn held(&self, member: &Member) -> bool {
        let At::Subnet = member.at else {
            return false;
        };
        self.places[member.place]
            .subnet
            .as_deref()
            .is_some_and(|subnet| self.reached_in_flight(subnet).is_some())
    }

    /// The first transition in flight that has an edge on one of this
    /// subnet's ports, if any.
    fn reached_in_flight(&self, subnet: &crate::place::Subnet) -> Option<TransitionId> {
        subnet
            .through
            .iter()
            .copied()
            .find(|transition| self.transitions[transition.0 as usize].body.is_none())
    }

    /// The net a member names, to read.
    fn member(&self, member: Member) -> Option<&Net> {
        let slot = &self.places[member.place];
        let At::Token { position, net, .. } = member.at else {
            return slot.subnet.as_deref().map(|subnet| &subnet.net);
        };
        let nested = slot.meta.nested?;
        let (_, value) = slot.tokens.get(position)?;
        let value: &dyn Any = &**value;
        (nested.nets)(value)
            .into_iter()
            .nth(net)
            .map(|(_, inner)| inner)
    }

    /// A member's turn: a deep step of its net. If that fired or failed,
    /// what the net logged is lifted under the place and token it sits in
    /// (rule 1), and the step is wrapped, or the error's
    /// [`path`](StepError::path) filled in, one level. `None` if it fired
    /// nothing. A watch goes down with the turn, one level further.
    fn member_turn(
        &mut self,
        member: Member,
        watch: Option<&mut Watch<'_>>,
    ) -> Option<Result<DeepStep, StepError>> {
        let place = PlaceId(member.place as u32);
        let At::Token { position, net, .. } = member.at else {
            return self.subnet_turn(place, watch);
        };
        let slot = &mut self.places[member.place];
        let nested = slot.meta.nested?;
        let (token, value) = slot.tokens.get_mut(position)?;
        let token = *token;
        // The member's deep step, with what it logged; the borrow of the
        // token ends with this block, so that the events can then be written
        // to this net's own log.
        let (name, nested_id, events, result) = {
            let (name, inner) = (nested.nets_mut)(&mut **value).into_iter().nth(net)?;
            debug_assert_eq!(
                inner.id(),
                member.id,
                "a member stays put within a deep step"
            );
            // A panic of the inner net's own code (a guard, a selector, a
            // key extractor) is caught here and goes on once what the net
            // logged before it is lifted (since 0.4.0; until then it was
            // left in the inner log).
            let stepped = catch_unwind(AssertUnwindSafe(|| match watch {
                None => inner.step_deep_watched(None),
                Some(watch) => {
                    let step = PathStep::Token {
                        place,
                        token,
                        net: name.to_owned(),
                    };
                    watch.down(step, member.id, |watch| {
                        inner.step_deep_watched(Some(watch))
                    })
                }
            }));
            let result = match stepped {
                Ok(Ok(None)) => return None,
                Ok(Ok(Some(step))) => Ok(Ok(step)),
                Ok(Err(failed)) => Ok(Err(failed)),
                Err(panic) => Err(panic),
            };
            let events: Vec<Event> = inner.take_events().collect();
            (name.to_owned(), inner.id(), events, result)
        };
        // Rule 1: lifted right after the inner firing, under the place and
        // token the token sits in; before the token is indexed again, so
        // that a key extractor that panics there loses none of it (since
        // 0.4.0).
        for event in events {
            self.emit(Event::Nested {
                place,
                token,
                net: name.clone(),
                nested_id,
                event: Box::new(event),
            });
        }
        // An inner net fired, and a key may read it: the token is indexed
        // again as it is now (DESIGN §8.3). Its keys are not read before
        // the turn: most turns tried fire nothing, and reading each tried
        // member's keys costs more than finding the old entries of the one
        // that fired in the index (`KeyIndex::forget`).
        let slot = &self.places[member.place];
        if slot.meta.keys.is_some() {
            self.index
                .forget(&slot.meta, place, token, &*slot.tokens[position].1);
        }
        let result = result.unwrap_or_else(|panic| resume_unwind(panic));
        Some(match result {
            Ok(step) => Ok(DeepStep::Inside {
                place,
                token,
                net: name,
                nested_id,
                step: Box::new(step),
            }),
            Err(failed) => Err(failed.one_level_out(
                PathStep::Token {
                    place,
                    token,
                    net: name,
                },
                nested_id,
            )),
        })
    }

    /// A subnet's turn: a deep step of the subnet of the subnet place at
    /// `place`, what it logged lifted into this log as [`Event::Subnet`]
    /// right after, and the step wrapped, or the error's
    /// [`path`](StepError::path) filled in, one level. `None` if it fired
    /// nothing.
    ///
    /// The subnet's ports are indexed by the subnet itself, so nothing of
    /// this net's index follows what fired in there. A watch goes down with
    /// the turn, one level further.
    fn subnet_turn(
        &mut self,
        place: PlaceId,
        watch: Option<&mut Watch<'_>>,
    ) -> Option<Result<DeepStep, StepError>> {
        let subnet = &mut self.places[place.0 as usize].subnet.as_deref_mut()?.net;
        // A panic in there goes on once what the subnet logged is lifted, as
        // for a token's net (since 0.4.0).
        let stepped = catch_unwind(AssertUnwindSafe(|| match watch {
            None => subnet.step_deep_watched(None),
            Some(watch) => {
                let id = subnet.id();
                watch.down(PathStep::Subnet { place }, id, |watch| {
                    subnet.step_deep_watched(Some(watch))
                })
            }
        }));
        let result = match stepped {
            Ok(Ok(None)) => return None,
            Ok(Ok(Some(step))) => Ok(Ok(step)),
            Ok(Err(failed)) => Ok(Err(failed)),
            Err(panic) => Err(panic),
        };
        let nested_id = subnet.id();
        let events: Vec<Event> = subnet.take_events().collect();
        for event in events {
            self.emit(Event::Subnet {
                place,
                nested_id,
                event: Box::new(event),
            });
        }
        let result = result.unwrap_or_else(|panic| resume_unwind(panic));
        Some(match result {
            Ok(step) => Ok(DeepStep::Subnet {
                place,
                nested_id,
                step: Box::new(step),
            }),
            Err(failed) => Err(failed.one_level_out(PathStep::Subnet { place }, nested_id)),
        })
    }

    /// The first transition anywhere in the nesting that is fireable, with
    /// its name, in the order the next [`step_deep`](Net::step_deep) would
    /// try the nets: the turns ahead of this net's own, each member's own
    /// first in the same order one level down, then this net's, then the
    /// turns behind it. Selectors are not consulted.
    ///
    /// What the drivers' fuse blames when it trips: the transition that
    /// would have been attempted next.
    pub(crate) fn first_fireable_deep(&self) -> Option<(TransitionId, String)> {
        self.first_fireable_in(Part::Ahead)
            .or_else(|| {
                self.fireable()
                    .first()
                    .map(|&transition| (transition, self.transition_name(transition)))
            })
            .or_else(|| self.first_fireable_in(Part::Behind))
    }

    /// [`first_fireable_deep`](Net::first_fireable_deep) of the members
    /// whose turn is in one part of this net's round, in the order they are
    /// tried, this net's own transitions left out: the parts on either side
    /// of this net's own in what the sequential driver's fuse blames.
    fn first_fireable_in(&self, part: Part) -> Option<(TransitionId, String)> {
        self.turns(part)
            .into_iter()
            .find_map(|member| self.member(member)?.first_fireable_deep())
    }

    /// Lifts whatever the inner nets of one token logged into this net's
    /// log, under the place and the token id it has now. A place that is not
    /// nested, or a token that is not there, lifts nothing.
    pub(crate) fn lift(&mut self, place: PlaceId, token: TokenId) {
        let lifted = {
            // A window carries its port's vtable, so this is asked here
            // before the token is looked for where it is.
            let Some(nested) = self
                .places
                .get(place.0 as usize)
                .and_then(|slot| slot.meta.nested)
            else {
                return;
            };
            let (holder, at) = self.holder_mut(place);
            let slot = &mut holder.places[at.0 as usize];
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
