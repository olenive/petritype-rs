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
//!   switched on, not in flight, every input place non-empty, guard (if any)
//!   saying yes. That is [`Net::fireable`] and [`Net::is_fireable`].
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
//! 1. [`Net::launch`] — take the body out of its slot and take the inputs out
//!    of their places, one argument per input edge ([`consume`]). The tokens
//!    are out of their places the moment this returns, which is what resolves
//!    a conflict between two transitions that wanted the same token, and the
//!    empty body slot is what "in flight" means. The result is a [`Firing`]:
//!    it owns the tokens and the body and borrows nothing of the net.
//! 2. [`Firing::run`] — call the body. A `Firing` is `Send`, so this may
//!    happen on a worker thread, in a task pool, or a frame later.
//! 3. [`Net::distribute`] — put the body back, resolve where every token the
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
//! back, with nothing in between and nothing to hand out — which is why they
//! report one [`Event::Fired`](crate::Event::Fired) where the staged path
//! reports an [`Event::Launched`](crate::Event::Launched) and then an
//! [`Event::Distributed`](crate::Event::Distributed): in the sequential path
//! no time passes between the two (DESIGN §8.1). Both paths run the same
//! code; only the reporting differs.
//!
//! # Inline firings
//!
//! A transition with a [`MutateEdge`](crate::MutateEdge) input is not
//! *detachable*: its body is handed a borrow into a place, so its firing
//! cannot leave the net. [`launch`](Net::launch) therefore runs the body
//! itself and hands back a `Firing` that is already done —
//! [`is_pending`](Firing::is_pending) is false and [`run`](Firing::run) only
//! unwraps it. Everything else is the same, including the events and the trip
//! home through [`distribute`](Net::distribute), so a driver need not care
//! which kind of transition it is holding.
//!
//! # When a firing fails
//!
//! A body fails by returning `Err` or by panicking, and the two arrive the
//! same way (DESIGN §7). Stage two is the only place a body is ever called —
//! [`Firing::run`] for a detachable one, inside [`launch`](Net::launch) for
//! an inline one — and both calls are wrapped in
//! [`catch_unwind`](std::panic::catch_unwind), so a panic becomes a
//! [`BodyPanicked`](crate::BodyPanicked) failure instead of unwinding out
//! through a worker thread with the body in its hand. Stage three then
//! reports it: the body goes back to its slot first of all, nothing is
//! distributed, and [`Event::Failed`] says which tokens the firing had
//! consumed.
//!
//! Those tokens are lost by default — the body owned them, and a failure says
//! nothing about what it did with them first — unless the transition was
//! declared with
//! [`restore_tokens_on_failure`](crate::TransitionBuilder::restore_tokens_on_failure),
//! in which case stage one took a clone of each and stage three puts the
//! clones back at the front of their places, with the ids they had, before it
//! reports anything. A [`MutateEdge`](crate::MutateEdge) mutation is not
//! undone either way: it was made where the token lies, and there is nothing
//! to roll back to.

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::distribution::DistributionError;
use crate::edge::EdgeKind;
use crate::erased::{ErasedBody, ErasedInput, ErasedInputs, ErasedOutcome, OutputTarget};
use crate::error::{BodyPanicked, FiringFailed, NetError};
use crate::event::Event;
use crate::ids::{NetId, PlaceId, TokenId, TransitionId};
use crate::net::Net;
use crate::param::{ParamSlot, param_slot};
use crate::place::{ErasedToken, PlaceMeta, PlaceSlot};
use crate::transition::{ArgSource, TransitionSlot};

/// Why an input place is never empty once stage one starts: being fireable
/// is what was checked.
const NON_EMPTY: &str = "a fireable transition's input places all hold a token";

/// How a net picks which of its fireable transitions fires next.
///
/// The slice is the fireable transitions in declaration order — exactly what
/// [`Net::fireable`] returns — and the answer must be one of them, or `None`
/// for "fire nothing this step". Installed with [`Net::set_selector`]; a net
/// without one uses the default, highest priority first and ties to the
/// transition declared first.
///
/// A selector is given `&Net`, so it may look at the marking, and it is `Fn`
/// rather than `FnMut` because choosing must not change the net. State of its
/// own goes in a captured `Arc`, `Mutex` or atomic — a round-robin selector
/// is the usual reason to want one.
pub type Selector = Box<dyn Fn(&Net, &[TransitionId]) -> Option<TransitionId> + Send + Sync>;

impl Net {
    // ----- fireability ----------------------------------------------------

    /// Every transition that could fire right now, in declaration order.
    ///
    /// A transition is fireable when all four of these hold (DESIGN §4.4,
    /// §6.1):
    ///
    /// - it is switched on — see
    ///   [`enable`](Net::enable) / [`disable`](Net::disable);
    /// - its body is in its slot, i.e. it is not in flight;
    /// - every one of its input places holds at least one token, whatever the
    ///   edge kind: [`SnapshotEdge`](crate::SnapshotEdge) and
    ///   [`MutateEdge`](crate::MutateEdge) consume nothing but still need
    ///   something to read;
    /// - its guard, if it has one, returns true.
    ///
    /// The list is recomputed by a linear scan over the transitions each time
    /// it is asked for, so it is always current and never has to be
    /// invalidated. Guards run during the scan.
    ///
    /// ```
    /// # use petritype::{NetBuilder, TakeEdge};
    /// # let mut b = NetBuilder::new();
    /// # let inbox = b.place::<u32>("Inbox");
    /// # let out = b.place::<u32>("Out");
    /// let double = b
    ///     .transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,))
    ///     .id();
    /// let mut net = b.build()?;
    ///
    /// assert!(net.fireable().is_empty(), "Inbox is empty");
    /// net.put(inbox, 21);
    /// assert_eq!(net.fireable(), [double]);
    /// # Ok::<(), petritype::BuildError>(())
    /// ```
    pub fn fireable(&self) -> Vec<TransitionId> {
        self.transitions
            .iter()
            .enumerate()
            .filter(|(_, slot)| self.slot_is_fireable(slot))
            .map(|(index, _)| TransitionId(index as u32))
            .collect()
    }

    /// Whether this one transition could fire right now, by the rules in
    /// [`fireable`](Net::fireable). An unknown id is not fireable.
    ///
    /// Not to be confused with [`is_enabled`](Net::is_enabled), which is the
    /// on/off switch alone.
    pub fn is_fireable(&self, transition: TransitionId) -> bool {
        self.transitions
            .get(transition.0 as usize)
            .is_some_and(|slot| self.slot_is_fireable(slot))
    }

    /// Every transition whose body is out of its slot, in declaration order.
    ///
    /// A transition is in flight from [`launch`](Net::launch) until the
    /// [`Completed`] it produced comes back to
    /// [`distribute`](Net::distribute). The list is what a visualiser
    /// highlights, and what a driver waits on before it decides that a net is
    /// quiescent: nothing is fireable and nothing is airborne.
    ///
    /// [`step`](Net::step) and [`fire`](Net::fire) never leave anything in
    /// flight — they take all three stages in one call — so this is empty
    /// unless `launch` was used.
    pub fn in_flight(&self) -> Vec<TransitionId> {
        self.transitions
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.body.is_none())
            .map(|(index, _)| TransitionId(index as u32))
            .collect()
    }

    /// Whether this one transition's body is out of its slot. An unknown id
    /// is not in flight.
    ///
    /// An in-flight transition is never [fireable](Net::fireable), whatever
    /// its places hold and whatever its switch says, which is what stops one
    /// transition being launched twice.
    pub fn is_in_flight(&self, transition: TransitionId) -> bool {
        self.transitions
            .get(transition.0 as usize)
            .is_some_and(|slot| slot.body.is_none())
    }

    /// The scan, written once. Takes the slot rather than the id because
    /// [`fireable`](Net::fireable) is already walking the vector.
    fn slot_is_fireable(&self, slot: &TransitionSlot) -> bool {
        slot.enabled
            && slot.body.is_some()
            && slot.inputs.iter().all(|(place, _)| self.count(*place) > 0)
            && match &slot.guard {
                Some(guard) => guard(self),
                None => true,
            }
    }

    /// This transition's priority right now: its priority closure called on
    /// the net, `0.0` if it has none, `0.0` for a `NaN` (so the default
    /// selector never compares against one) and `0.0` for an unknown id.
    ///
    /// Public so that a custom selector can rank by priority the way the
    /// default one does, and so that a diagram can show it.
    pub fn priority_of(&self, transition: TransitionId) -> f64 {
        let Some(slot) = self.transitions.get(transition.0 as usize) else {
            return 0.0;
        };
        let Some(priority) = &slot.priority else {
            return 0.0;
        };
        let score = priority(self);
        if score.is_nan() { 0.0 } else { score }
    }

    // ----- selection ------------------------------------------------------

    /// Replaces how [`step`](Net::step) chooses among the fireable
    /// transitions.
    ///
    /// The default is highest [`priority`](crate::TransitionBuilder::priority)
    /// first, ties to the transition declared first — deterministic, which is
    /// what makes a sequential run replayable. A custom selector sees the
    /// same list, in the same order, and may return `None` to fire nothing.
    ///
    /// ```
    /// # use petritype::{NetBuilder, TakeEdge};
    /// # let mut b = NetBuilder::new();
    /// # let inbox = b.place::<u32>("Inbox");
    /// # let out = b.place::<u32>("Out");
    /// # b.transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,));
    /// # let mut net = b.build()?;
    /// # net.put(inbox, 21);
    /// // Nothing fires while this selector is installed.
    /// net.set_selector(|_net, _fireable| None);
    /// assert_eq!(net.step()?, None);
    /// assert_eq!(net.fireable().len(), 1, "fireable, but not chosen");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn set_selector<F>(&mut self, selector: F)
    where
        F: Fn(&Net, &[TransitionId]) -> Option<TransitionId> + Send + Sync + 'static,
    {
        self.selector = Some(Box::new(selector));
    }

    /// Puts the default selector back: highest
    /// [`priority`](crate::TransitionBuilder::priority) first, ties to the
    /// transition declared first.
    ///
    /// The undo of [`set_selector`](Net::set_selector), and the way back to a
    /// deterministic sequential run after a phase that wanted a different
    /// choice — a round-robin for a demo, a selector that declines while a
    /// user is dragging something. A net that never had one is unchanged.
    ///
    /// ```
    /// # use petritype::{NetBuilder, TakeEdge};
    /// # let mut b = NetBuilder::new();
    /// # let inbox = b.place::<u32>("Inbox");
    /// # let out = b.place::<u32>("Out");
    /// let double = b
    ///     .transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,))
    ///     .id();
    /// let mut net = b.build()?;
    /// net.put(inbox, 21);
    ///
    /// net.set_selector(|_net, _fireable| None);
    /// assert_eq!(net.step()?, None, "fireable, but the selector declined");
    ///
    /// net.clear_selector();
    /// assert_eq!(net.step()?, Some(double), "the default is back");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn clear_selector(&mut self) {
        self.selector = None;
    }

    // ----- firing ---------------------------------------------------------

    /// Fires one transition: scan, select, consume, run, distribute.
    ///
    /// `Ok(Some(t))` says `t` fired. `Ok(None)` says nothing did, either
    /// because nothing was fireable — the net is quiescent — or because the
    /// selector declined. `Err` says the body returned `Err` or panicked: the
    /// net is consistent, the firing's inputs are gone unless the transition
    /// asked to
    /// [restore](crate::TransitionBuilder::restore_tokens_on_failure) them,
    /// and a further `step` works (DESIGN §7).
    ///
    /// One successful firing appends one [`Event::Fired`], preceded by an
    /// [`Event::Mutated`] per [`MutateEdge`](crate::MutateEdge), and adds one
    /// to [`fired_count`](Net::fired_count).
    ///
    /// ```
    /// # use petritype::{Event, NetBuilder, TakeEdge};
    /// # let mut b = NetBuilder::new();
    /// # let inbox = b.place::<u32>("Inbox");
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
    /// and the net consistent — the same failure a body that returned `Err`
    /// would produce. Under `panic = "abort"` there is nothing to catch and
    /// the process dies, as that profile asked.
    pub fn step(&mut self) -> Result<Option<TransitionId>, FiringFailed> {
        let fireable = self.fireable();
        if fireable.is_empty() {
            return Ok(None);
        }

        // Selection reads the net and nothing more, so it is a block of
        // shared borrows that ends before the firing takes `&mut self`.
        let chosen = {
            let net: &Net = self;
            match &net.selector {
                Some(selector) => selector(net, &fireable),
                None => default_selector(net, &fireable),
            }
        };

        let Some(chosen) = chosen else {
            return Ok(None);
        };
        debug_assert!(
            fireable.contains(&chosen),
            "a selector must return one of the transitions it was offered, or None"
        );
        if !fireable.contains(&chosen) {
            return Ok(None);
        }

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
    /// for [`step`](Net::step).
    ///
    /// # Panics
    ///
    /// As [`step`](Net::step): a panicking body is caught and reported as a
    /// failure, not propagated.
    pub fn fire(&mut self, transition: TransitionId) -> Result<(), NetError> {
        let slot = self
            .transitions
            .get(transition.0 as usize)
            .ok_or(NetError::NoSuchTransition { transition })?;
        if !self.slot_is_fireable(slot) {
            return Err(NetError::NotFireable { transition });
        }
        self.fire_now(transition)?;
        Ok(())
    }

    // ----- the three stages -----------------------------------------------

    /// Stage one: consumes this transition's inputs and takes its body out of
    /// the net.
    ///
    /// The tokens leave their places *now*, which is the whole point: a peer
    /// transition that wanted one of them is no longer
    /// [fireable](Net::fireable), so consuming in the launch sweep is what
    /// resolves conflicts, with no locks and no in-flight registry (DESIGN
    /// §6.4). The transition is [in flight](Net::is_in_flight) until the
    /// [`Completed`] comes back to [`distribute`](Net::distribute), and
    /// cannot be launched again meanwhile.
    ///
    /// Emits [`Event::Launched`] naming the tokens that were taken.
    ///
    /// A transition that is not *detachable* — one with a
    /// [`MutateEdge`](crate::MutateEdge) input, whose body borrows a token
    /// where it lies — has its body run inside this call, and the `Firing`
    /// comes back with [`is_pending`](Firing::is_pending) false.
    ///
    /// ```
    /// # use petritype::{NetBuilder, TakeEdge};
    /// # let mut b = NetBuilder::new();
    /// # let inbox = b.place::<u32>("Inbox");
    /// # let out = b.place::<u32>("Out");
    /// let double = b
    ///     .transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,))
    ///     .id();
    /// let mut net = b.build()?;
    /// net.put(inbox, 21);
    ///
    /// let firing = net.launch(double)?;
    /// assert_eq!(net.count(inbox.id()), 0, "consumed at launch");
    /// assert_eq!(net.in_flight(), [double]);
    /// assert!(!net.is_fireable(double), "in flight, so not fireable");
    ///
    /// net.distribute(firing.run())?;
    /// assert!(net.in_flight().is_empty());
    /// assert_eq!(net.fired_count(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// [`NetError::NoSuchTransition`] for an unknown id, and
    /// [`NetError::NotFireable`] for a transition that cannot fire right now
    /// — including one that is already in flight. The net is untouched in
    /// both cases.
    ///
    /// # Panics
    ///
    /// Not for a panicking body, inline firing included: the panic is caught
    /// here, and the `Firing` comes back carrying the failure exactly as it
    /// would carry one a body returned. [`distribute`](Net::distribute) is
    /// where every failure is reported, whichever stage produced it.
    #[must_use = "a Firing that is dropped instead of distributed strands its transition in \
                  flight for good: the body never comes home"]
    pub fn launch(&mut self, transition: TransitionId) -> Result<Firing, NetError> {
        let slot = self
            .transitions
            .get(transition.0 as usize)
            .ok_or(NetError::NoSuchTransition { transition })?;
        if !self.slot_is_fireable(slot) {
            return Err(NetError::NotFireable { transition });
        }
        Ok(self.launch_now(transition, Report::Staged))
    }

    /// Stage three: puts the body back and writes what it produced.
    ///
    /// Atomic: every target place is resolved and checked before any token is
    /// written, so a firing that cannot be applied writes nothing at all. The
    /// body goes home first of all, on the failing path too, so that a
    /// failure never strands the transition in flight.
    ///
    /// Emits the [`Mutated`](Event::Mutated) events of this firing's
    /// [`MutateEdge`](crate::MutateEdge) inputs, then
    /// [`Event::Distributed`] naming the tokens that landed — or
    /// [`Event::Failed`] if the firing failed — and adds one to
    /// [`fired_count`](Net::fired_count) on success. A transition that
    /// [restores its inputs](crate::TransitionBuilder::restore_tokens_on_failure)
    /// has them back in their places before the `Failed` event is appended,
    /// so an observer that re-reads the marking when it sees that event sees
    /// them there.
    ///
    /// # Errors
    ///
    /// [`NetError::ForeignFiring`] if the `Completed` was launched by another
    /// net, [`NetError::NotInFlight`] if this net's transition is not in
    /// flight, and [`NetError::FiringFailed`] if the body returned `Err` or
    /// panicked, or named a place it may not write to. The net is untouched
    /// in the first two cases; in the third it is consistent, with the
    /// firing's inputs gone or restored (DESIGN §7).
    ///
    /// The `Completed` is consumed whichever way this goes, so a rejected one
    /// takes its body with it: the two rejections above are misuse, and the
    /// net they belong to keeps that transition in flight.
    pub fn distribute(&mut self, completed: Completed) -> Result<(), NetError> {
        // The stamp first, exactly as a `PlaceRef` is checked before it is
        // used as an index: a `TransitionId` from another net would otherwise
        // name whatever transition sits at that index here.
        if completed.net != self.id {
            return Err(NetError::ForeignFiring {
                transition: completed.transition,
                from: completed.net,
            });
        }
        if !self.is_in_flight(completed.transition) {
            return Err(NetError::NotInFlight {
                transition: completed.transition,
            });
        }
        self.distribute_completed(completed)?;
        Ok(())
    }

    // ----- the stages, written once ---------------------------------------

    /// One firing, all three stages, with fireability already established.
    fn fire_now(&mut self, transition: TransitionId) -> Result<(), FiringFailed> {
        let completed = self.launch_now(transition, Report::Fired).run();
        self.distribute_completed(completed)
    }

    /// Stage one, with fireability already established.
    ///
    /// `report` is how this firing will describe itself: one
    /// [`Fired`](Event::Fired) at the end for the sequential path, or
    /// [`Launched`](Event::Launched) here and
    /// [`Distributed`](Event::Distributed) at stage three for the staged one.
    /// It travels with the firing, so the two paths differ in nothing but
    /// what they log.
    fn launch_now(&mut self, transition: TransitionId, report: Report) -> Firing {
        let index = transition.0 as usize;
        // Out of its slot until stage three: the body may not borrow the
        // transition list while it runs, and an empty slot is what "in
        // flight" means.
        let mut body = self.transitions[index]
            .body
            .take()
            .expect("a fireable transition has its body in its slot");

        let mut consumed = Vec::new();
        let mut mutated = Vec::new();
        // `Some`, empty, for a transition that restores its inputs: the
        // clones are taken by `consume`, below, before the body sees a thing.
        // `None` for every other transition, which is also what says that
        // there is nothing to put back.
        let mut restore = self.transitions[index]
            .restore_tokens_on_failure
            .then(Vec::new);
        let state = {
            // Field by field, so that the input places can be emptied while
            // the edge list — and the parameter values — are read out of the
            // net beside them.
            let Net {
                places,
                transitions,
                params,
                ..
            } = &mut *self;
            let inputs = &transitions[index].inputs;
            let sources = &transitions[index].args;
            let args = consume(
                places,
                params,
                sources,
                inputs,
                &mut consumed,
                &mut mutated,
                restore.as_mut(),
            );
            if transitions[index].detachable {
                Stage::Airborne { body, args }
            } else {
                // Stage two, here and now: a `MutateEdge` hands the body a
                // borrow into a place, which may neither travel with the
                // `Firing` nor still be alive when stage three writes.
                let args = borrow_mutated(places, sources, inputs, args);
                let outcome = call_body(&mut body, args);
                Stage::Ran { body, outcome }
            }
        };

        if report == Report::Staged {
            self.emit(Event::Launched {
                transition,
                consumed: consumed.clone(),
            });
        }
        Firing {
            net: self.id,
            transition,
            consumed,
            mutated,
            restore,
            report,
            state,
        }
    }

    /// Stage three, with the stamp and the in-flight slot already checked.
    fn distribute_completed(&mut self, completed: Completed) -> Result<(), FiringFailed> {
        let Completed {
            transition,
            consumed,
            mutated,
            restore,
            report,
            body,
            outcome,
            ..
        } = completed;
        // Home before anything else can go wrong, on the failing path too:
        // DESIGN §7 promises a consistent net after a failure, and a body
        // left out of its slot would strand the transition in flight. A body
        // that panicked comes home as well, carrying whatever state the panic
        // left it in.
        self.transitions[transition.0 as usize].body = Some(body);
        self.distribute_outcome(transition, outcome, consumed, mutated, restore, report)
    }

    /// Places what the body produced, or accounts for its failure.
    ///
    /// `consumed` and `mutated` are stage one's records, in edge order, and
    /// `restore` the clones it took if this transition restores its inputs.
    fn distribute_outcome(
        &mut self,
        transition: TransitionId,
        outcome: ErasedOutcome,
        consumed: Vec<(PlaceId, TokenId)>,
        mutated: Vec<(PlaceId, TokenId)>,
        restore: Option<Vec<(PlaceId, TokenId, ErasedToken)>>,
        report: Report,
    ) -> Result<(), FiringFailed> {
        let produced = match outcome {
            ErasedOutcome::Ok(tokens) => tokens,
            ErasedOutcome::Err(error) => {
                // The tokens go back first, so that the marking is already
                // whole when the `Failed` event an observer reads arrives.
                let restored = self.restore_consumed(restore);
                // A body that mutated a token through a `MutateEdge` and then
                // failed has still mutated it — the change went into the
                // place as it was made, and there is nothing to roll back to
                // — so the mutations are reported before the failure.
                self.emit_mutations(&mutated);
                self.emit(Event::Failed {
                    transition,
                    consumed,
                    restored,
                    error: error.to_string(),
                });
                return Err(FiringFailed {
                    transition,
                    error,
                    restored,
                });
            }
        };

        // Atomic, as DESIGN §6.2 asks: every target is resolved and checked
        // in this loop, and nothing is written until all of them have been.
        // Two of the three output forms have nothing left to check — a
        // positional slot's index came from the body's return type, which was
        // matched against the output places at compile time, and a routed
        // payload's from the table resolved when the transition was declared,
        // which `NetBuilder::build` refused the net without — so their target
        // is an index and the bound stays a `debug_assert!`. A
        // [`Distribution`](crate::Distribution) names its places itself, from
        // data, so its targets are checked here and a bad one fails the whole
        // firing before any of its siblings lands.
        let mut resolved = Vec::with_capacity(produced.len());
        for (target, token) in produced {
            let place = match target {
                OutputTarget::Index(index) => {
                    debug_assert!(
                        index < self.transitions[transition.0 as usize].outputs.len(),
                        "output index {index} is past the outputs of transition {transition:?}"
                    );
                    self.transitions[transition.0 as usize].outputs[index]
                }
                OutputTarget::Place(place, net) => {
                    match self.distributed_target(transition, place, net, &token) {
                        Ok(place) => place,
                        Err(error) => {
                            // A firing that cannot be applied fails like any
                            // other (DESIGN §4.5): nothing written, the
                            // consumed tokens lost or restored, the net
                            // consistent.
                            let restored = self.restore_consumed(restore);
                            self.emit_mutations(&mutated);
                            self.emit(Event::Failed {
                                transition,
                                consumed,
                                restored,
                                error: error.to_string(),
                            });
                            return Err(FiringFailed {
                                transition,
                                error: Box::new(error),
                                restored,
                            });
                        }
                    }
                }
            };
            resolved.push((place, token));
        }

        // The firing stood, so the clones a restoring transition took are
        // dropped here, unread: they exist only for the failing path.
        drop(restore);

        let mut landed = Vec::with_capacity(resolved.len());
        for (place, token) in resolved {
            let token = self.push_produced(place, token);
            landed.push((place, token));
        }

        self.emit_mutations(&mutated);
        self.emit(match report {
            // The sequential path: consumed and produced together, because
            // nothing happened between them.
            Report::Fired => Event::Fired {
                transition,
                consumed,
                produced: landed,
            },
            // The staged path already said what was consumed, at launch.
            Report::Staged => Event::Distributed {
                transition,
                produced: landed,
            },
        });
        self.fired_count += 1;
        Ok(())
    }

    /// Checks one pair of a [`Distribution`](crate::Distribution) against this
    /// transition, and answers with the place to write to.
    ///
    /// Three things are asked, in the order they can go wrong:
    ///
    /// - the handle was stamped with *this* net, so that a
    ///   [`PlaceRef`](crate::PlaceRef) from another net is caught rather than
    ///   read as an index into this one;
    /// - the place is one this transition declared as an output, because the
    ///   output tuple is what a diagram draws and a firing may not write
    ///   outside it;
    /// - the token really is of that place's type. The typed API has already
    ///   guaranteed this — [`Distribution::to`](crate::Distribution::to) takes
    ///   a `PlaceRef<T>` and a `T` — so it is a cheap defence against a
    ///   foreign handle whose index happens to coincide, and not a check the
    ///   author is meant to rely on.
    fn distributed_target(
        &self,
        transition: TransitionId,
        place: PlaceId,
        net: NetId,
        token: &ErasedToken,
    ) -> Result<PlaceId, DistributionError> {
        let outputs = &self.transitions[transition.0 as usize].outputs;
        if net != self.id || !outputs.contains(&place) {
            return Err(DistributionError::NotAnOutput { transition, place });
        }
        let meta = &self.places[place.0 as usize].meta;
        // `(**token)` and not `token`: a `Box<dyn Any + ..>` is itself `Any`,
        // so asking the box would answer with the box's own type.
        if (**token).type_id() != meta.type_id {
            return Err(DistributionError::WrongType {
                transition,
                place,
                expected: meta.type_name,
            });
        }
        Ok(place)
    }

    /// Puts a failed firing's consumed tokens back, and says whether there
    /// was a promise to keep.
    ///
    /// `None` is a transition that does not restore, and the answer is false:
    /// its inputs are gone. `Some` is one that does, and the answer is true
    /// however many tokens are in the list — a restoring transition that
    /// consumes nothing (only snapshots and mutating edges) restores an empty
    /// list, and says so.
    ///
    /// The clones go in at the *front*, in the order they were taken, which
    /// is what "the firing is undone" means for a FIFO place: the next firing
    /// sees exactly what this one saw. They keep their original
    /// [`TokenId`]s — the ids were never retired, because the firing they
    /// belonged to never completed — and nothing is minted, so no
    /// [`Added`](Event::Added) is emitted either; the
    /// [`Failed`](Event::Failed) event's `restored` flag is the whole report.
    ///
    /// The list is walked backwards so that each place's tokens end up in
    /// front in the order they left it: the last one taken is pushed first
    /// and ends up behind the ones taken before it. Two input edges never
    /// name one place ([`DuplicateInputPlace`](crate::BuildError::DuplicateInputPlace)),
    /// so the places' groups cannot interleave.
    fn restore_consumed(&mut self, restore: Option<Vec<(PlaceId, TokenId, ErasedToken)>>) -> bool {
        let Some(restore) = restore else {
            return false;
        };
        for (place, token, value) in restore.into_iter().rev() {
            self.places[place.0 as usize]
                .tokens
                .push_front((token, value));
        }
        true
    }

    /// One [`Event::Mutated`] per mutating edge, in edge order.
    fn emit_mutations(&mut self, mutated: &[(PlaceId, TokenId)]) {
        for &(place, token) in mutated {
            self.emit(Event::Mutated { place, token });
        }
    }

    // ----- the switch -----------------------------------------------------

    /// Switches a transition on or off. A disabled transition is never
    /// [fireable](Net::fireable), however full its input places are.
    ///
    /// Emits [`Event::Toggled`] when the switch actually moves, and nothing
    /// at all when it was already where it was asked to go — so a driver that
    /// disables a failing transition on every failure does not flood the log.
    ///
    /// # Errors
    ///
    /// [`NetError::NoSuchTransition`] for an unknown id.
    pub fn set_enabled(&mut self, transition: TransitionId, enabled: bool) -> Result<(), NetError> {
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
/// turn them into outputs, and borrows nothing of the net — so it is `Send`,
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
/// no recovery API in this version, so a driver that can drop a firing —
/// because a worker panicked, or a channel was closed — should treat that as
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
    /// They ride along with the firing — wherever the body runs, they do not
    /// — and stage three either puts them back, if the firing failed, or
    /// drops them. Each carries the id its original had, because a restored
    /// token is the same token: the firing that would have retired the id
    /// never completed.
    restore: Option<Vec<(PlaceId, TokenId, ErasedToken)>>,
    /// Which events this firing reports, decided by the path that launched
    /// it.
    report: Report,
    /// How far along this firing is.
    state: Stage,
}

/// What a [`Firing`] is carrying: work still to do, or an answer already.
enum Stage {
    /// A detachable transition: the body and its owned arguments, waiting for
    /// [`Firing::run`]. The arguments are `'static` — only whole tokens and
    /// clones of them, never a borrow into a place — which is what makes a
    /// `Firing` `Send`.
    Airborne {
        body: ErasedBody,
        args: Vec<ErasedInput<'static>>,
    },
    /// A transition that was not detachable, so
    /// [`launch`](Net::launch) already called its body inline — catching its
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
    /// The transition this firing belongs to — the one that is in flight
    /// until this firing is distributed.
    pub fn transition(&self) -> TransitionId {
        self.transition
    }

    /// Whether [`run`](Firing::run) still has work to do.
    ///
    /// False when [`launch`](Net::launch) ran the body inline, which it does
    /// for a transition that is not detachable: one with a
    /// [`MutateEdge`](crate::MutateEdge) input, whose body borrows a token
    /// where it lies. Such a firing must not be sent to a worker — there is
    /// nothing for the worker to do, and the mutation has already happened —
    /// so a driver sends the pending ones and runs the rest on the spot.
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
        } = self;
        let (body, outcome) = match state {
            Stage::Airborne { mut body, args } => {
                let outcome = call_body(&mut body, args);
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
        }
    }
}

/// A firing whose body has run: stage two done, the outputs and the body
/// waiting to go home.
///
/// `Send`, like the [`Firing`] it came from, so it can be posted back to the
/// thread that holds the net — an `mpsc` channel of `Completed` is what a
/// concurrent driver waits on. Hand it to [`Net::distribute`], which is the
/// only thing that can be done with it: the net is the single writer, and
/// everything this value knows about where the tokens go is the net's to
/// resolve.
///
/// Dropping one strands its transition in flight for good, as dropping a
/// [`Firing`] does.
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
}

impl Completed {
    /// The transition whose body ran — the one that is in flight until this
    /// is distributed.
    pub fn transition(&self) -> TransitionId {
        self.transition
    }

    /// Whether the body produced outputs rather than a failure — a returned
    /// `Err` or a caught panic, which are the same thing by the time they get
    /// here.
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

/// The selector a net uses until [`Net::set_selector`] replaces it: highest
/// priority wins, ties go to the transition declared first.
///
/// `fireable` is in declaration order, and the comparison is strict, so the
/// first transition with the top score is the one kept.
pub(crate) fn default_selector(net: &Net, fireable: &[TransitionId]) -> Option<TransitionId> {
    let mut best: Option<(TransitionId, f64)> = None;
    for &transition in fireable {
        let score = net.priority_of(transition);
        if best.is_none_or(|(_, top)| score > top) {
            best = Some((transition, score));
        }
    }
    best.map(|(transition, _)| transition)
}

/// Stage one: one argument per input, in declaration order, taking out of the
/// places whatever each edge kind takes and off the net whatever each
/// parameter names.
///
/// A free function over the places alone, rather than a method, because that
/// is the shape the staged path needs: consuming is what resolves conflicts
/// between transitions, and it must be doable while the rest of the net is
/// borrowed beside it.
///
/// Every argument it produces is **owned** — `ErasedInput<'static>` — which
/// is what lets a [`Firing`] be `Send` and outlive the call that made it. A
/// [`MutateEdge`](crate::MutateEdge) has no owned argument to give, since its
/// token stays in its place, so it leaves an [`ErasedInput::Taken`] behind
/// and is recorded in `mutated`; [`borrow_mutated`] fills those positions in,
/// for the inline path that is the only one allowed to hold such a borrow.
///
/// `restore` is `Some` for a transition declared with
/// [`restore_tokens_on_failure`](crate::TransitionBuilder::restore_tokens_on_failure),
/// and every token a consuming edge takes is cloned into it, with its id, in
/// the order it was taken. The clone is taken *here*, before the body is
/// given anything, because that is the last moment the token is certainly
/// intact: from the next line on the body owns it.
///
/// A [`Param<T>`](crate::Param) argument is a clone of the net's current
/// value, taken here too and for the same reason (DESIGN §10): the body may
/// run for a long time somewhere else, and a
/// [`set_param`](Net::set_param) meanwhile must not change what it is working
/// with. A parameter consumes nothing, so it is in neither `consumed` nor
/// `restore` and puts nothing in an event.
///
/// `sources` is the tuple order — one entry per argument, saying which input
/// edge or which parameter fills it — because `inputs` is the edges alone.
fn consume(
    places: &mut [PlaceSlot],
    params: &[ParamSlot],
    sources: &[ArgSource],
    inputs: &[(PlaceId, EdgeKind)],
    consumed: &mut Vec<(PlaceId, TokenId)>,
    mutated: &mut Vec<(PlaceId, TokenId)>,
    mut restore: Option<&mut Vec<(PlaceId, TokenId, ErasedToken)>>,
) -> Vec<ErasedInput<'static>> {
    let mut args = Vec::with_capacity(sources.len());

    for source in sources {
        let (place, kind) = match *source {
            ArgSource::Edge(edge) => inputs[edge],
            ArgSource::Param { type_id, .. } => {
                let slot = param_slot(params, type_id)
                    .expect("build refuses a net whose declared parameter has no value");
                args.push(ErasedInput::Token(slot.clone_value()));
                continue;
            }
        };
        let index = place.0 as usize;
        match kind {
            EdgeKind::Take => {
                let slot = &mut places[index];
                let (token, value) = slot.tokens.pop_front().expect(NON_EMPTY);
                if let Some(restore) = restore.as_deref_mut() {
                    restore.push((place, token, restore_clone(&slot.meta, &value)));
                }
                consumed.push((place, token));
                args.push(ErasedInput::Token(value));
            }
            EdgeKind::TakeAll => {
                let slot = &mut places[index];
                let mut values = Vec::with_capacity(slot.tokens.len());
                for (token, value) in slot.tokens.drain(..) {
                    if let Some(restore) = restore.as_deref_mut() {
                        restore.push((place, token, restore_clone(&slot.meta, &value)));
                    }
                    consumed.push((place, token));
                    values.push(value);
                }
                args.push(ErasedInput::Tokens(values));
            }
            EdgeKind::Snapshot => {
                let slot = &places[index];
                let clone = slot
                    .meta
                    .clone
                    .expect("build rejects a snapshot of a place that cannot clone its tokens");
                let (_, value) = slot.tokens.front().expect(NON_EMPTY);
                // The clone is taken now, so the body sees a value that
                // cannot change under it even once firings run elsewhere.
                args.push(ErasedInput::Token(clone(&**value)));
            }
            EdgeKind::Mutate => {
                let (token, _) = places[index].tokens.front().expect(NON_EMPTY);
                mutated.push((place, *token));
                // Overwritten by `borrow_mutated`, which is the only caller
                // allowed to hold the borrow; a body never sees this.
                args.push(ErasedInput::Taken);
            }
        }
    }

    args
}

/// The clone a restoring transition takes of one token it consumes, with that
/// place's cloner.
///
/// [`build`](crate::NetBuilder::build) refuses a transition that restores and
/// consumes a place without a cloner
/// ([`RestoreOfNoClonePlace`](crate::BuildError::RestoreOfNoClonePlace)), so
/// by the time a firing runs the question has been answered.
fn restore_clone(meta: &PlaceMeta, value: &ErasedToken) -> ErasedToken {
    let clone = meta.clone.expect(
        "build rejects restoring a transition that consumes a place which cannot clone its tokens",
    );
    clone(&**value)
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
/// well leave half-updated — that is documented on `BodyPanicked` and is the
/// author's to reason about — and this firing's arguments, which are owned
/// here and dropped with the unwind. The net is not in scope at all on the
/// detachable path, and on the inline path the only borrow into it is the
/// mutating edge's token, whose type cannot make the *net* inconsistent: a
/// half-changed token is a value the author's own code left behind, and the
/// [`Mutated`](Event::Mutated) event reports it either way.
fn call_body(body: &mut ErasedBody, args: Vec<ErasedInput<'_>>) -> ErasedOutcome {
    match catch_unwind(AssertUnwindSafe(move || body(ErasedInputs::new(args)))) {
        Ok(outcome) => outcome,
        Err(payload) => ErasedOutcome::Err(Box::new(BodyPanicked::from_payload(payload))),
    }
}

/// Fills in the arguments of the mutating edges: the front token of each
/// place, borrowed where it lies.
///
/// The second half of stage one, and inline firing's alone. A `&mut` into a
/// place may not be taken while [`consume`] is still draining the other
/// places, and may not outlive the call to the body, so it is taken here,
/// between the two, by the one path that calls the body inside the net.
///
/// `args` comes in owned and goes out borrowing `places` for `'a`: an
/// `ErasedInput<'static>` is an `ErasedInput<'a>` for any shorter `'a`, which
/// is what the annotation below says.
fn borrow_mutated<'a>(
    places: &'a mut [PlaceSlot],
    sources: &[ArgSource],
    inputs: &[(PlaceId, EdgeKind)],
    args: Vec<ErasedInput<'static>>,
) -> Vec<ErasedInput<'a>> {
    let mut args: Vec<ErasedInput<'a>> = args;
    // Where the mutating edges are: the argument each fills and the place it
    // borrows. The position is the argument's, not the edge's, since a
    // parameter may stand between two edges.
    let borrows: Vec<(usize, usize)> = sources
        .iter()
        .enumerate()
        .filter_map(|(position, source)| match *source {
            ArgSource::Edge(edge) => Some((position, inputs[edge])),
            ArgSource::Param { .. } => None,
        })
        .filter(|(_, (_, kind))| *kind == EdgeKind::Mutate)
        .map(|(position, (place, _))| (position, place.0 as usize))
        .collect();
    if borrows.is_empty() {
        return args;
    }

    // `slice::get_disjoint_mut` wants its indices as a fixed-size array and a
    // transition's mutating edges are only counted at run time, so the same
    // job is done with `iter_mut`: its items borrow the slice rather than the
    // iterator, so every borrow this pass takes may be alive at once. That
    // they are disjoint is `BuildError::DuplicateInputPlace`'s doing — no two
    // input edges of one transition name the same place.
    for (index, slot) in places.iter_mut().enumerate() {
        let Some(&(position, _)) = borrows.iter().find(|&&(_, place)| place == index) else {
            continue;
        };
        let (_, value) = slot.tokens.front_mut().expect(NON_EMPTY);
        args[position] = ErasedInput::Borrowed(value);
    }

    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::TakeEdge;
    use crate::erased::ErasedInputs;
    use crate::{Event, NetBuilder};

    /// [`NetError::NotInFlight`] is a guard the safe API cannot reach: a
    /// [`Completed`] is minted only by [`Net::launch`], which leaves its
    /// transition in flight, and [`Net::distribute`] consumes the one it is
    /// given — so there is never a second one to hand over. It is exercised
    /// from in here, where a `Completed` for a transition whose body is back
    /// home can be built by hand.
    #[test]
    fn distributing_a_transition_that_is_not_in_flight_is_refused() {
        let mut b = NetBuilder::new();
        let inbox = b.place::<u32>("Inbox");
        let out = b.place::<u32>("Out");
        b.seed(inbox, [1]);
        let copy = b
            .transition("Copy", (TakeEdge(inbox),), (out,), |n: u32| (n,))
            .id();
        let mut net = b.build().expect("a one-transition net");

        let firing = net.launch(copy).expect("fireable");
        net.distribute(firing.run()).expect("in flight");
        assert!(net.in_flight().is_empty(), "the body is back in its slot");

        let ghost = Completed {
            net: net.id(),
            transition: copy,
            consumed: Vec::new(),
            mutated: Vec::new(),
            restore: None,
            report: Report::Staged,
            body: Box::new(|_: ErasedInputs<'_>| ErasedOutcome::Ok(Vec::new())),
            outcome: ErasedOutcome::Ok(Vec::new()),
        };
        assert!(matches!(
            net.distribute(ghost),
            Err(NetError::NotInFlight { transition }) if transition == copy
        ));

        assert_eq!(net.fired_count(), 1, "the rejected firing changed nothing");
        assert_eq!(net.count(out.id()), 1);
        let events: Vec<Event> = net.drain_events().collect();
        assert!(
            matches!(
                events[..],
                [Event::Launched { .. }, Event::Distributed { .. }]
            ),
            "the one firing that did happen, and nothing since: {events:?}"
        );
    }
}
