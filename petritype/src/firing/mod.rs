//! Firing: which transitions could fire, which one does, and the three
//! stages one firing goes through.
//!
//! # Fireable, and the switch called `enabled`
//!
//! Two different questions are easy to confuse, so they have two names here:
//!
//! - **enabled** is the author's on/off switch,
//!   [`TransitionInfo::enabled`](crate::TransitionInfo::enabled), flipped by
//!   [`Net::enable`] and [`Net::disable`] and reported by
//!   [`Event::Toggled`](crate::Event::Toggled). A switched-off transition is
//!   ignored however full its places are.
//! - **fireable** is whether this transition could fire *right now*:
//!   switched on, not in flight, every input place non-empty (holding a
//!   token its filter accepts, for a keyed edge; a token for each edge,
//!   all different, for take edges that share a place; anything at all,
//!   nothing included, for a [`DrainEdge`](crate::DrainEdge) or a
//!   [`SurveyEdge`](crate::SurveyEdge)), guard (if any) saying yes. That is
//!   [`Net::fireable`] and [`Net::is_fireable`].
//!
//! Classical Petri net writing calls the second one "enabled" (and DESIGN §3
//! still does); this crate needs the word for the switch, which is the one an
//! author sets and a visualiser draws.
//!
//! # One firing, in three stages
//!
//! Every firing goes through the same three stages, and each of them is
//! public, because the middle one may happen somewhere else entirely:
//!
//! 1. [`Net::launch`]: take the body out of its slot and take the inputs out
//!    of their places, one argument per input edge ([`consume`]). The tokens
//!    are out of their places the moment this returns, which is what resolves
//!    a conflict between two transitions that wanted the same token, and the
//!    empty body slot is what "in flight" means. The result is a [`Firing`]:
//!    it owns the tokens and the body and borrows nothing of the net.
//! 2. [`Firing::run`]: call the body. A `Firing` is `Send`, so this may
//!    happen on a worker thread, in a task pool, or a frame later.
//! 3. [`Net::distribute`]: put the body back, resolve where every token the
//!    body returned belongs, mint an id for each, push them into their output
//!    places, and log what happened. Resolution happens before any write, so
//!    a [`Distribution`](crate::Distribution) that names a place this
//!    transition does not own fails the firing without leaving half of it
//!    behind.
//!
//! The net is the only writer throughout: stages one and three take
//! `&mut Net`, and the stage between them has no net to write to.
//!
//! [`Net::step`] and [`Net::fire`] are those same three stages run back to
//! back, with nothing in between and nothing to hand out, which is why they
//! report one [`Event::Fired`](crate::Event::Fired) where the staged path
//! reports an [`Event::Launched`](crate::Event::Launched) and then an
//! [`Event::Distributed`](crate::Event::Distributed): in the sequential path
//! no time passes between the two (DESIGN §8.1). Both paths run the same
//! code; only the reporting differs.
//!
//! # Inline firings
//!
//! A transition with a mutating or a read edge, such as a
//! [`MutateEdge`](crate::MutateEdge) or a [`ReadEdge`](crate::ReadEdge), is
//! not *detachable*: its body is handed a borrow into a place, so its firing
//! cannot leave the net. [`launch`](Net::launch) therefore runs the body
//! itself and hands back a `Firing` that is already done:
//! [`is_pending`](Firing::is_pending) is false and [`run`](Firing::run) only
//! unwraps it. Everything else is the same, including the events and the trip
//! home through [`distribute`](Net::distribute), so a driver need not care
//! which kind of transition it is holding.
//!
//! # When a firing fails
//!
//! A body fails by returning `Err` or by panicking, and the two arrive the
//! same way (DESIGN §7). Stage two is the only place a body is ever called
//! ([`Firing::run`] for a detachable one, inside [`launch`](Net::launch) for
//! an inline one), and both calls are wrapped in
//! [`catch_unwind`](std::panic::catch_unwind), so a panic becomes a
//! [`BodyPanicked`](crate::BodyPanicked) failure instead of unwinding out
//! through a worker thread with the body in its hand. Stage three then
//! reports it: the body goes back to its slot first of all, nothing is
//! distributed, and [`Event::Failed`] says which tokens the firing had
//! consumed.
//!
//! Those tokens are lost by default (the body owned them, and a failure says
//! nothing about what it did with them first), unless the transition was
//! declared with
//! [`restore_tokens_on_failure`](crate::TransitionBuilder::restore_tokens_on_failure),
//! in which case stage one took a clone of each and stage three puts the
//! clones back at the front of their places, with the ids they had, before it
//! reports anything. A [`MutateEdge`](crate::MutateEdge) mutation is not
//! undone either way: it was made where the token lies, and there is nothing
//! to roll back to.

mod distribute;
mod launch;
mod selection;

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use crate::closed::{FIRE, Hold, SWITCH};
use crate::erased::{ErasedBody, ErasedInput, ErasedInputs, ErasedOutcome};
use crate::error::{BodyPanicked, FiringFailed, NetError, StepError};
use crate::event::Event;
use crate::ids::{NetId, PlaceId, TokenId, TransitionId};
use crate::net::Net;
use crate::place::ErasedToken;

pub use selection::{Selector, TieBreak};

impl Net {
    // ----- firing ---------------------------------------------------------

    /// Fires one transition: scan, select, consume, run, distribute.
    ///
    /// `Ok(Some(t))` says `t` fired. `Ok(None)` says nothing did, either
    /// because nothing was fireable (the net is quiescent) or because the
    /// selector declined. `Err` says one of two things
    /// ([`StepError`]). [`StepError::FiringFailed`]: the body returned `Err`
    /// or panicked; the net is consistent, the firing's inputs are gone
    /// unless the transition asked to
    /// [restore](crate::TransitionBuilder::restore_tokens_on_failure) them,
    /// and a further `step` works.
    /// [`StepError::SelectionFailed`]: the choice could not be made, as
    /// [`choose`](Net::choose) says, and nothing fired or changed.
    ///
    /// One successful firing appends one [`Event::Fired`], preceded by an
    /// [`Event::Mutated`] per [`MutateEdge`](crate::MutateEdge) or
    /// [`MutateWhereEdge`](crate::MutateWhereEdge), and adds one
    /// to [`fired_count`](Net::fired_count). The choice is made as
    /// [`choose`](Net::choose) makes it, so it moves the
    /// [round robin](TieBreak::RoundRobin)'s cursor.
    ///
    /// ```
    /// # use petritype::{Event, NetBuilder, TakeEdge};
    /// # let mut b = NetBuilder::new();
    /// # let inbox = b.place::<u32>("Inbox");
    /// # b.port(inbox);
    /// # let out = b.place::<u32>("Out");
    /// let double = b
    ///     .transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,))
    ///     .id();
    /// let mut net = b.build()?;
    /// let token = net.put(inbox, 21);
    ///
    /// assert_eq!(net.step()?, Some(double));
    /// assert_eq!(net.count(inbox.id()), 0);
    /// assert_eq!(net.fired_count(), 1);
    /// assert_eq!(net.step()?, None, "nothing left to fire");
    ///
    /// let produced = net.tokens(out.id()).next().unwrap().0;
    /// assert_eq!(
    ///     net.drain_events().last(),
    ///     Some(Event::Fired {
    ///         transition: double,
    ///         consumed: vec![(inbox.id(), token)],
    ///         produced: vec![(out.id(), produced)],
    ///     }),
    /// );
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Panics
    ///
    /// Not for a panicking body: the panic is caught and returned as a
    /// [`FiringFailed`] whose error is a
    /// [`BodyPanicked`](crate::BodyPanicked), with the body back in its slot
    /// and the net consistent: the same failure a body that returned `Err`
    /// would produce. Under `panic = "abort"` there is nothing to catch and
    /// the process dies, as that profile asked.
    ///
    /// A panic of the net's own code, a guard, a priority or the selector
    /// while the choice is made, or what [`launch`](Net::launch) and
    /// [`distribute`](Net::distribute) run (a key extractor, a token's or a
    /// parameter's `Clone`, [`Nested::nets_mut`](crate::Nested::nets_mut)),
    /// goes on to the caller, and leaves the net as those two say for their
    /// stages: a panic before the firing takes its tokens moves nothing.
    pub fn step(&mut self) -> Result<Option<TransitionId>, StepError> {
        let Some(chosen) = self.choose_now()? else {
            return Ok(None);
        };
        self.fire_now(chosen)?;
        Ok(Some(chosen))
    }

    /// Fires one named transition, whatever the selector would have chosen.
    ///
    /// This is how a test drives a net one firing at a time, and how a
    /// visualiser fires the transition a user clicked on. The transition must
    /// be fireable: `fire` never waits and never half-fires.
    ///
    /// # Errors
    ///
    /// [`NetError::NoSuchTransition`] for an unknown id and
    /// [`NetError::NotFireable`] for a transition that cannot fire right now;
    /// the net is untouched in both cases. A body that returns `Err` or
    /// panics gives [`NetError::FiringFailed`], with the same consequences as
    /// for [`step`](Net::step). [`NetError::HeldByBody`], the net untouched,
    /// when a body of the net around this one holds it: firing by name is
    /// the program's, and the body steps the net instead.
    ///
    /// # Panics
    ///
    /// As [`step`](Net::step): a panicking body is caught and reported as a
    /// failure, not propagated.
    pub fn fire(&mut self, transition: TransitionId) -> Result<(), NetError> {
        self.admit_knob(FIRE)?;
        let slot = self
            .transitions
            .get(transition.0 as usize)
            .ok_or(NetError::NoSuchTransition { transition })?;
        if !self.slot_is_fireable(slot) {
            return Err(NetError::NotFireable {
                transition,
                name: slot.name.clone(),
            });
        }
        self.fire_now(transition)?;
        Ok(())
    }

    // ----- the stages, written once ---------------------------------------

    /// One firing, all three stages, with fireability already established.
    pub(crate) fn fire_now(&mut self, transition: TransitionId) -> Result<(), FiringFailed> {
        let completed = self.launch_now(transition, Report::Fired, None).run();
        self.distribute_completed(completed)
    }

    // ----- the switch -----------------------------------------------------

    /// Switches a transition on or off. A disabled transition is never
    /// [fireable](Net::fireable), however full its input places are.
    ///
    /// Emits [`Event::Toggled`] when the switch actually moves, and nothing
    /// at all when it was already where it was asked to go, so a driver that
    /// disables a failing transition on every failure does not flood the log.
    ///
    /// # Errors
    ///
    /// [`NetError::NoSuchTransition`] for an unknown id.
    /// [`NetError::HeldByBody`] when a body of the net around this one holds
    /// it: its switches are the program's.
    pub fn set_enabled(&mut self, transition: TransitionId, enabled: bool) -> Result<(), NetError> {
        self.admit_knob(SWITCH)?;
        let slot = self
            .transitions
            .get_mut(transition.0 as usize)
            .ok_or(NetError::NoSuchTransition { transition })?;
        if slot.enabled == enabled {
            return Ok(());
        }
        slot.enabled = enabled;
        self.emit(Event::Toggled {
            transition,
            enabled,
        });
        Ok(())
    }

    /// Switches a transition on: [`set_enabled(t, true)`](Net::set_enabled).
    pub fn enable(&mut self, transition: TransitionId) -> Result<(), NetError> {
        self.set_enabled(transition, true)
    }

    /// Switches a transition off: [`set_enabled(t, false)`](Net::set_enabled).
    ///
    /// ```
    /// # use petritype::{Event, NetBuilder, TakeEdge};
    /// # let mut b = NetBuilder::new();
    /// # let inbox = b.place::<u32>("Inbox");
    /// # b.port(inbox);
    /// # let out = b.place::<u32>("Out");
    /// # let double = b.transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,)).id();
    /// # let mut net = b.build()?;
    /// # net.put(inbox, 21);
    /// net.disable(double)?;
    /// assert!(!net.is_fireable(double), "switched off");
    /// assert!(!net.is_enabled(double));
    /// assert_eq!(net.step()?, None);
    ///
    /// net.enable(double)?;
    /// assert_eq!(net.step()?, Some(double));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn disable(&mut self, transition: TransitionId) -> Result<(), NetError> {
        self.set_enabled(transition, false)
    }

    /// Whether a transition is switched on. An unknown id is not.
    ///
    /// The switch only: a switched-on transition with empty input places is
    /// still `is_enabled`. See [`is_fireable`](Net::is_fireable) for whether
    /// it could fire.
    pub fn is_enabled(&self, transition: TransitionId) -> bool {
        self.transitions
            .get(transition.0 as usize)
            .is_some_and(|slot| slot.enabled)
    }
}

/// A firing that has left the net: stage one done, the body in hand.
///
/// A `Firing` owns the tokens its transition consumed and the body that will
/// turn them into outputs, and borrows nothing of the net, so it is `Send`,
/// and stage two may happen on a worker thread, in a task pool, or a frame
/// later while the net goes on firing other transitions. The net meanwhile
/// knows only that this transition's body slot is empty:
/// [`Net::is_in_flight`].
///
/// [`run`](Firing::run) turns it into a [`Completed`], which
/// [`Net::distribute`] hands back to the net.
///
/// # The body has to come home
///
/// Dropping a `Firing`, or the [`Completed`] it becomes, **strands its
/// transition in flight for good**: the body is inside the value that was
/// dropped, the slot stays empty, and the transition is never
/// [fireable](Net::fireable) again. The consumed tokens go with it. There is
/// no recovery API in this version, so a driver that can drop a firing
/// (because a worker panicked, or a channel was closed) should treat that as
/// a failure of the whole run.
#[must_use = "a Firing that is dropped instead of run and distributed strands its transition in \
              flight for good: the body never comes home"]
pub struct Firing {
    /// The net that launched this firing, checked by
    /// [`distribute`](Net::distribute) before anything else.
    net: NetId,
    /// The transition whose body is in here.
    transition: TransitionId,
    /// What stage one took out of the input places, in edge order.
    consumed: Vec<(PlaceId, TokenId)>,
    /// The tokens a [`MutateEdge`](crate::MutateEdge) borrowed, in edge
    /// order. They never left their places; stage three reports them.
    mutated: Vec<(PlaceId, TokenId)>,
    /// The clones stage one took of the consumed tokens, in the order it took
    /// them, for a transition declared with
    /// [`restore_tokens_on_failure`](crate::TransitionBuilder::restore_tokens_on_failure);
    /// `None` for every other transition.
    ///
    /// They ride along with the firing wherever its body runs, never handed to
    /// the body, and stage three either puts them back, if the firing failed,
    /// or drops them. Each carries the id its original had, because a restored
    /// token is the same token: the firing that would have retired the id
    /// never completed.
    restore: Option<Vec<(PlaceId, TokenId, ErasedToken)>>,
    /// Which events this firing reports, decided by the path that launched
    /// it.
    report: Report,
    /// How far along this firing is.
    state: Stage,
    /// This firing's mark, when it handed or lent its body a token's net:
    /// every such net holds a `Weak` to it, which lapses when this is
    /// dropped at the end of stage three (DESIGN §6.9).
    hold: Option<Arc<Hold>>,
}

/// What a [`Firing`] is carrying: work still to do, or an answer already.
enum Stage {
    /// A detachable transition: the body and its owned arguments, waiting for
    /// [`Firing::run`]. The arguments are `'static` (only whole tokens and
    /// clones of them, never a borrow into a place), which is what makes a
    /// `Firing` `Send`.
    Airborne {
        body: ErasedBody,
        args: Vec<ErasedInput<'static>>,
    },
    /// A transition that was not detachable, so
    /// [`launch`](Net::launch) already called its body inline, catching its
    /// panic, if it had one, exactly as [`Firing::run`] would have; `run`
    /// only unwraps this.
    Ran {
        body: ErasedBody,
        outcome: ErasedOutcome,
    },
}

/// Which events one firing reports: the difference between the sequential
/// path and the staged one, and the only difference between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Report {
    /// [`step`](Net::step) and [`fire`](Net::fire): one
    /// [`Event::Fired`](crate::Event::Fired) at the end.
    Fired,
    /// [`launch`](Net::launch) and [`distribute`](Net::distribute):
    /// [`Launched`](crate::Event::Launched) when the tokens leave and
    /// [`Distributed`](crate::Event::Distributed) when they land, because
    /// time passes in between.
    Staged,
}

impl Firing {
    /// The transition this firing belongs to, the one that is in flight
    /// until this firing is distributed.
    pub fn transition(&self) -> TransitionId {
        self.transition
    }

    /// Whether [`run`](Firing::run) still has work to do.
    ///
    /// False when [`launch`](Net::launch) ran the body inline, which it does
    /// for a transition that is not
    /// [detachable](crate::TransitionInfo::detachable): one with a mutating
    /// or a read edge, whose body borrows a token where it lies. Such a
    /// firing must not be sent to a worker (there is nothing for the worker
    /// to do, and the body has already run), so a driver sends the pending
    /// ones and runs the rest on the spot.
    pub fn is_pending(&self) -> bool {
        matches!(self.state, Stage::Airborne { .. })
    }

    /// Stage two: calls the body.
    ///
    /// This is the one stage with no net in sight, which is exactly why it is
    /// its own step: it may run anywhere, for as long as it likes. A firing
    /// that was already run inline is unwrapped unchanged.
    ///
    /// A panicking body does not panic this call: the body runs inside
    /// [`catch_unwind`](std::panic::catch_unwind), and a panic becomes a
    /// [`BodyPanicked`](crate::BodyPanicked) failure carried home like any
    /// other. That matters most exactly here, where the body may be on a
    /// worker thread: an escaping panic would kill the thread and take the
    /// body with it, and the transition would be in flight for good. The
    /// panic message still reaches stderr through the panic hook, which this
    /// crate does not touch.
    ///
    /// # Panics
    ///
    /// Not for a panicking body, as above. Under `panic = "abort"` there is
    /// nothing to catch and the process dies, as that profile asked.
    pub fn run(self) -> Completed {
        let Firing {
            net,
            transition,
            consumed,
            mutated,
            restore,
            report,
            state,
            hold,
        } = self;
        let (body, outcome) = match state {
            Stage::Airborne { mut body, args } => {
                let outcome = call_body(&mut body, args, hold.as_deref());
                (body, outcome)
            }
            Stage::Ran { body, outcome } => (body, outcome),
        };
        Completed {
            net,
            transition,
            consumed,
            mutated,
            restore,
            report,
            body,
            outcome,
            hold,
        }
    }
}

/// A firing whose body has run: stage two done, the outputs and the body
/// waiting to go home.
///
/// `Send`, like the [`Firing`] it came from, so it can be posted back to the
/// thread that holds the net. An `mpsc` channel of `Completed` is what a
/// concurrent driver waits on. Hand it to [`Net::distribute`], which is the
/// only thing that can be done with it: the net is the single writer, and
/// everything this value knows about where the tokens go is the net's to
/// resolve.
///
/// Dropping one strands its transition in flight for good, as dropping a
/// [`Firing`] does. A `distribute` that refuses one, handed to a net that
/// did not launch it say, gives it back in its [`DistributeError`](crate::DistributeError) (since
/// 0.4.0), to be handed to the net that did.
#[must_use = "a Completed that is dropped instead of distributed strands its transition in \
              flight for good: the body never comes home"]
pub struct Completed {
    /// The net that launched this firing.
    net: NetId,
    /// The transition whose body ran.
    transition: TransitionId,
    /// What stage one took out of the input places, in edge order.
    consumed: Vec<(PlaceId, TokenId)>,
    /// The tokens a [`MutateEdge`](crate::MutateEdge) borrowed, in edge
    /// order.
    mutated: Vec<(PlaceId, TokenId)>,
    /// The clones of the consumed tokens, for a transition that restores them
    /// on failure; `None` for every other transition. See
    /// [`Firing`](Firing#structfield.restore).
    restore: Option<Vec<(PlaceId, TokenId, ErasedToken)>>,
    /// Which events this firing reports.
    report: Report,
    /// On its way back to its slot.
    body: ErasedBody,
    /// What the body produced, or how it failed.
    outcome: ErasedOutcome,
    /// The firing's mark, kept until stage three ends. See
    /// [`Firing`](Firing#structfield.hold).
    hold: Option<Arc<Hold>>,
}

impl Completed {
    /// The transition whose body ran, the one that is in flight until this
    /// is distributed.
    pub fn transition(&self) -> TransitionId {
        self.transition
    }

    /// Whether the body produced outputs rather than a failure (a returned
    /// `Err` or a caught panic, which are the same thing by the time they get
    /// here).
    ///
    /// A driver may look before it distributes, to decide what to do about a
    /// failure. It still has to distribute: that is what puts the body back,
    /// puts the consumed tokens back if the transition asked for it, and logs
    /// the [`Failed`](crate::Event::Failed) event, and it is where the
    /// failure is reported as [`FiringFailed`].
    pub fn succeeded(&self) -> bool {
        matches!(self.outcome, ErasedOutcome::Ok(_))
    }
}

impl fmt::Debug for Firing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Firing")
            .field("transition", &self.transition)
            .field("net", &self.net)
            .field("consumed", &self.consumed)
            // The body is a closure and the outcome may hold tokens of any
            // type: how far along the firing is, is what a reader can use.
            .field("pending", &self.is_pending())
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for Completed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Completed")
            .field("transition", &self.transition)
            .field("net", &self.net)
            .field("consumed", &self.consumed)
            .field("succeeded", &self.succeeded())
            .finish_non_exhaustive()
    }
}

/// Stage two, wherever it happens: call the body, and turn a panic into a
/// failure instead of letting it out.
///
/// The one place a body is ever called, so the one place the catch belongs.
/// A panic becomes [`BodyPanicked`](crate::BodyPanicked) and travels on as an
/// ordinary failure: the body comes home, the net stays consistent, and a
/// worker thread running this survives to report it (DESIGN §7).
///
/// [`AssertUnwindSafe`] is the honest answer rather than a dodge. What the
/// call touches is the body, which is `FnMut` and whose own state a panic may
/// well leave half-updated (that is documented on `BodyPanicked` and is the
/// author's to reason about), and this firing's arguments, which are owned
/// here and dropped with the unwind. The net is not in scope at all on the
/// detachable path, and on the inline path the only borrow into it is the
/// mutating edge's token, whose type cannot make the *net* inconsistent: a
/// half-changed token is a value the author's own code left behind, and the
/// [`Mutated`](Event::Mutated) event reports it either way.
///
/// `hold` is this firing's mark, when it lent or handed its body a token's
/// net. A net that refused the body a write or a knob kept the refusal
/// there, and the firing fails with the first one, whatever the body made
/// of it: passed it on, dropped the `Err`, or caught the panic of a method
/// that has no `Result` (DESIGN §6.9). A panic of such a method carries
/// the refusal's message, and the refusal itself is what the firing fails
/// with, not a [`BodyPanicked`](crate::BodyPanicked).
fn call_body(
    body: &mut ErasedBody,
    args: Vec<ErasedInput<'_>>,
    hold: Option<&Hold>,
) -> ErasedOutcome {
    let outcome = match catch_unwind(AssertUnwindSafe(move || body(ErasedInputs::new(args)))) {
        Ok(outcome) => outcome,
        Err(payload) => ErasedOutcome::Err(Box::new(BodyPanicked::from_payload(payload))),
    };
    match hold.and_then(Hold::take_refusal) {
        Some(refusal) => ErasedOutcome::Err(Box::new(refusal)),
        None => outcome,
    }
}
