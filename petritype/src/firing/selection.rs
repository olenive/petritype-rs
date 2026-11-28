//! Which transitions could fire, and which one does: fireability, the
//! selector, the tie-break and the choice a step makes.

use crate::closed::{CHOOSE, CLEAR_SELECTOR, SET_SELECTOR, SET_TIE_BREAK};
use crate::deep::Busy;
use crate::error::{NetError, SelectionFailed, SelectionFault, StepError};
use crate::ids::TransitionId;
use crate::index::first_free_match;
use crate::net::{Net, slot_of};
use crate::pairing::pair;
use crate::sharing::share;
use crate::transition::TransitionSlot;

/// How a fireability check of the concurrent driver takes the busy tokens
/// of a place that a take-all or a drain takes every token of.
#[derive(Debug, Clone, Copy)]
enum Every {
    /// The transition is not fireable while one of them is busy.
    Waits,
    /// As if none of them were busy: whether the transition would be
    /// fireable but for them.
    Overlooked,
}

/// How a net picks which of its fireable transitions fires next.
///
/// The slice is the fireable transitions in declaration order, exactly what
/// [`Net::fireable`] returns, and the answer is one of them, or `None` for
/// "fire nothing this step". Any other answer is an error, the
/// [`SelectionFailed`] of [`SelectionFault::NotOffered`], for whatever
/// asked: the step, [`Net::select`] or [`Net::choose`] (since 2026-11-28;
/// until then nothing fired). Installed with [`Net::set_selector`]; a net
/// without one uses the default, highest priority first and ties to the
/// transition declared first.
///
/// A selector is given `&Net`, so it may look at the marking, and it is `Fn`
/// rather than `FnMut` because choosing must not change the net. State of its
/// own goes in a captured `Arc`, `Mutex` or atomic. (Turn order needs none:
/// it is the default selector's [`TieBreak::RoundRobin`].)
pub type Selector = Box<dyn Fn(&Net, &[TransitionId]) -> Option<TransitionId> + Send + Sync>;

/// How the default selector breaks a tie between fireable transitions of
/// equal [`priority`](crate::TransitionBuilder::priority).
///
/// The choice shapes how a pipeline drains, and neither shape is more
/// correct than the other. Places are FIFO, so in a chain
/// `A -> First -> B -> Second -> C` with several tokens in `A`:
///
/// - [`FirstDeclared`](TieBreak::FirstDeclared) keeps choosing `First` while
///   `A` has anything in it, so the tokens move in **batches**: every one
///   through stage one before any reaches stage two.
/// - [`LastDeclared`](TieBreak::LastDeclared) prefers the stage furthest
///   along, so each token **flows** all the way through before the next one
///   enters.
/// - [`RoundRobin`](TieBreak::RoundRobin) takes the transitions in turn, so
///   the stages **alternate**, `First`, `Second`, `First`, `Second`, for as
///   long as both can fire.
///
/// It is a setting rather than a rule for that reason, set with
/// [`NetBuilder::tie_break`](crate::NetBuilder::tie_break) or
/// [`Net::set_tie_break`]. The default is `FirstDeclared`, which is the
/// Python library's definition-order rule and what every net built before
/// this setting existed already did. A custom selector installed with
/// [`Net::set_selector`] makes its own choice and ignores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum TieBreak {
    /// Ties go to the transition declared first: batch order.
    #[default]
    FirstDeclared,
    /// Ties go to the transition declared last: flow order.
    LastDeclared,
    /// Ties go to the first transition declared *after* the one the default
    /// selector chose last, wrapping round to the first declared when none
    /// follows: turn order. Python petritype's
    /// `tie_break="round_robin"`.
    ///
    /// That last choice is the *cursor*, one per net, inner nets included.
    /// It moves whenever a choice of the default selector is taken to be
    /// fired (by [`Net::step`] and [`Net::choose`], and so by
    /// [`Net::step_deep`], the drivers and the protocol runner) under any
    /// tie-break, though only this one reads it. Each launch of a
    /// concurrent sweep is such a choice (since 2026-11-28; until then a
    /// sweep neither read nor moved it). [`Net::select`] reads it without
    /// moving it, and a custom selector and [`Net::fire`] neither read nor
    /// move it. With no cursor yet the
    /// first declared wins: in a fresh net, and in one restored from JSON or
    /// forked (`Net::fork`, with the `serde` feature), since the cursor is
    /// kept in memory only and a restored net starts its round afresh. A
    /// switched-off transition is not fireable, so it is never a
    /// candidate.
    RoundRobin,
}

impl Net {
    // ----- fireability ----------------------------------------------------

    /// Every transition that could fire right now, in declaration order.
    ///
    /// A transition is fireable when all four of these hold:
    ///
    /// - it is switched on (see
    ///   [`enable`](Net::enable) / [`disable`](Net::disable));
    /// - its body is in its slot, i.e. it is not in flight;
    /// - every one of its input places holds at least one token, whatever the
    ///   edge kind: [`SnapshotEdge`](crate::SnapshotEdge),
    ///   [`MutateEdge`](crate::MutateEdge) and [`ReadEdge`](crate::ReadEdge)
    ///   consume nothing but still need something to read; and a keyed edge
    ///   ([`TakeWhereEdge`](crate::TakeWhereEdge),
    ///   [`MutateWhereEdge`](crate::MutateWhereEdge),
    ///   [`ReadWhereEdge`](crate::ReadWhereEdge)) needs a token its
    ///   filter accepts, asked of the key index, a
    ///   [`Clause::IsParam`](crate::Clause::IsParam) reading the parameter as
    ///   it is now. A keyed edge bound to another input by a
    ///   [`Clause::IsInput`](crate::Clause::IsInput) needs, with that input,
    ///   a pair: a token of the input that has a match, the oldest such
    ///   token tried first, one index lookup per token tried. Two or more
    ///   take edges on one place (since 0.4.0) need a token each, all
    ///   different: the edges choose in declaration order, each the first
    ///   token no edge before it chose, a [`TakeEdge`](crate::TakeEdge) the
    ///   front one left and a [`TakeWhereEdge`](crate::TakeWhereEdge) the
    ///   first left that its filter accepts, and none gives its token up for
    ///   a later one. The exceptions
    ///   are the edges that do not
    ///   [require presence](crate::EdgeKind::requires_presence),
    ///   [`DrainEdge`](crate::DrainEdge), [`SurveyEdge`](crate::SurveyEdge)
    ///   and [`ReadAllEdge`](crate::ReadAllEdge), which take, copy or lend
    ///   every token there is, zero or more, and are not asked at all;
    /// - its guard, if it has one, returns true. It is asked last, so a guard
    ///   on a keyed transition knows a token is there to be had.
    ///
    /// The list is recomputed by a linear scan over the transitions each time
    /// it is asked for, so it is always current and never has to be
    /// invalidated. Guards run during the scan.
    ///
    /// ```
    /// # use petritype::{NetBuilder, TakeEdge};
    /// # let mut b = NetBuilder::new();
    /// # let inbox = b.place::<u32>("Inbox");
    /// # b.port(inbox);
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
    /// [`Completed`](crate::Completed) it produced comes back to
    /// [`distribute`](Net::distribute). The list is what a visualiser
    /// highlights, and what a driver waits on before it decides that a net is
    /// quiescent: nothing is fireable and nothing is airborne.
    ///
    /// [`step`](Net::step) and [`fire`](Net::fire) never leave anything in
    /// flight (they take all three stages in one call), so this is empty
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
    pub(super) fn slot_is_fireable(&self, slot: &TransitionSlot) -> bool {
        self.slot_is_fireable_with(slot, None)
    }

    /// [`fireable`](Net::fireable) for the concurrent driver, which minds
    /// the tokens `busy` holds in every edge that takes or changes a token
    /// (DESIGN §13, since 0.4.0): a take or a mutating edge passes them
    /// over, so a transition that could only use busy tokens is not fireable
    /// for now, and a take-all or a drain takes every token of its place, so
    /// its transition is not fireable while any of them is busy. With `None`
    /// it is `fireable`.
    pub(crate) fn fireable_with(&self, busy: Option<Busy<'_>>) -> Vec<TransitionId> {
        self.transitions
            .iter()
            .enumerate()
            .filter(|(_, slot)| self.slot_is_fireable_with(slot, busy))
            .map(|(index, _)| TransitionId(index as u32))
            .collect()
    }

    /// The transitions that wait for the busy tokens of a place they take
    /// every token of (DESIGN §13, since 0.4.0): each has a take-all or a
    /// drain edge on a place where `busy` holds a token, and would be
    /// fireable but for those tokens, every other condition met: its switch,
    /// its guard and its other inputs, which pass over their busy tokens as
    /// [`fireable_with`](Net::fireable_with) has them do. What the
    /// concurrent driver holds a place still for.
    pub(crate) fn waiting_with(&self, busy: Busy<'_>) -> Vec<TransitionId> {
        let blocked = |slot: &TransitionSlot| {
            slot.inputs.iter().any(|&(place, kind)| {
                kind.takes_every()
                    && slot_of(&self.places, place)
                        .is_some_and(|held| Busy::any_in(Busy::for_edge(Some(busy), kind), held))
            })
        };
        self.transitions
            .iter()
            .enumerate()
            .filter(|(_, slot)| {
                blocked(slot) && self.slot_is_fireable_as(slot, Some(busy), Every::Overlooked)
            })
            .map(|(index, _)| TransitionId(index as u32))
            .collect()
    }

    /// [`slot_is_fireable`](Net::slot_is_fireable), passing over the tokens
    /// `busy` holds as [`fireable_with`](Net::fireable_with) says.
    pub(super) fn slot_is_fireable_with(
        &self,
        slot: &TransitionSlot,
        busy: Option<Busy<'_>>,
    ) -> bool {
        self.slot_is_fireable_as(slot, busy, Every::Waits)
    }

    /// [`slot_is_fireable_with`](Net::slot_is_fireable_with), with `every`
    /// saying whether a take-all or a drain waits for the busy tokens of its
    /// place, or overlooks them, as [`waiting_with`](Net::waiting_with)
    /// asks.
    fn slot_is_fireable_as(
        &self,
        slot: &TransitionSlot,
        busy: Option<Busy<'_>>,
        every: Every,
    ) -> bool {
        slot.enabled
            && slot.body.is_some()
            && slot.inputs.iter().zip(&slot.filters).enumerate().all(
                |(edge, (&(place, kind), filter))| {
                    let busy = match every {
                        Every::Overlooked if kind.takes_every() => None,
                        _ => Busy::for_edge(busy, kind),
                    };
                    match filter {
                        // A take-all or a drain takes every token there is,
                        // so it waits while one of them is busy.
                        _ if kind.takes_every()
                            && slot_of(&self.places, place)
                                .is_some_and(|held| Busy::any_in(busy, held)) =>
                        {
                            false
                        }
                        // A drain or a survey takes whatever is there,
                        // nothing included: its place cannot hold the
                        // transition back.
                        _ if !kind.requires_presence() => true,
                        // A source or a bound edge is asked with its pair,
                        // and the takes on a shared place together, below.
                        _ if slot.is_paired(edge) => true,
                        _ if slot.is_shared(edge) => true,
                        None if busy.is_none() => self.count(place) > 0,
                        None => slot_of(&self.places, place)
                            .and_then(|held| Busy::first_free(busy, held))
                            .is_some(),
                        Some(filter) => first_free_match(
                            &self.places,
                            &self.params,
                            &self.index,
                            place,
                            filter,
                            busy,
                        )
                        .is_some(),
                    }
                },
            )
            && slot.bindings.iter().all(|binding| {
                pair(&self.places, &self.params, &self.index, slot, binding, busy).is_some()
            })
            && (slot.shared.is_empty()
                || share(&self.places, &self.params, &self.index, slot, busy).is_some())
            && match &slot.guard {
                Some(guard) => guard(self),
                None => true,
            }
    }

    /// This transition's priority right now: its priority closure called on
    /// the net, `0.0` if it has none and `0.0` for an unknown id. An
    /// infinity is a priority like any other and ranks as itself.
    ///
    /// Public so that a custom selector can rank by priority the way the
    /// default one does, and so that a diagram can show it.
    ///
    /// ```
    /// use petritype::{NetBuilder, SelectionFault, TakeEdge};
    ///
    /// let mut b = NetBuilder::new();
    /// let inbox = b.place::<u32>("Inbox");
    /// let out = b.place::<u32>("Out");
    /// let urgent = b
    ///     .transition("Urgent", (TakeEdge(inbox),), (out,), |n: u32| (n,))
    ///     .priority(|_| f64::INFINITY)
    ///     .id();
    /// let broken = b
    ///     .transition("Broken", (TakeEdge(inbox),), (out,), |n: u32| (n,))
    ///     .priority(|_| f64::NAN)
    ///     .id();
    /// let net = b.build()?;
    ///
    /// assert_eq!(net.priority_of(urgent)?, f64::INFINITY);
    /// let failed = net.priority_of(broken).unwrap_err();
    /// let SelectionFault::NanPriority { transition, .. } = failed.why else {
    ///     panic!("a NaN priority");
    /// };
    /// assert_eq!(transition, broken);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// [`SelectionFailed`] with [`SelectionFault::NanPriority`] when the
    /// closure returns `NaN` (since 2026-11-28; until then it counted as
    /// `0.0`): a `NaN` ranks neither above nor below any other priority,
    /// so the default selector fails with the same error when it meets one.
    pub fn priority_of(&self, transition: TransitionId) -> Result<f64, SelectionFailed> {
        let Some(slot) = self.transitions.get(transition.0 as usize) else {
            return Ok(0.0);
        };
        let Some(priority) = &slot.priority else {
            return Ok(0.0);
        };
        let score = priority(self);
        if score.is_nan() {
            return Err(SelectionFailed::new(SelectionFault::NanPriority {
                transition,
                name: slot.name.clone(),
            }));
        }
        Ok(score)
    }

    // ----- selection ------------------------------------------------------

    /// Replaces how [`step`](Net::step) chooses among the fireable
    /// transitions.
    ///
    /// The default is highest [`priority`](crate::TransitionBuilder::priority)
    /// first, ties to the transition declared first. That is deterministic,
    /// which is what makes a sequential run replayable. A custom selector
    /// sees the same list, in the same order, and may return `None` to fire
    /// nothing.
    ///
    /// ```
    /// # use petritype::{NetBuilder, TakeEdge};
    /// # let mut b = NetBuilder::new();
    /// # let inbox = b.place::<u32>("Inbox");
    /// # b.port(inbox);
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
    ///
    /// # Panics
    ///
    /// When a body of the net around this one holds it: its selector is the
    /// program's, and the body's firing fails with [`NetError::HeldByBody`].
    /// [`try_set_selector`](Net::try_set_selector) returns the error
    /// instead.
    pub fn set_selector<F>(&mut self, selector: F)
    where
        F: Fn(&Net, &[TransitionId]) -> Option<TransitionId> + Send + Sync + 'static,
    {
        self.admit_knob_or_panic(SET_SELECTOR);
        self.selector = Some(Box::new(selector));
    }

    /// [`set_selector`](Net::set_selector) for a caller that handles a
    /// refusal (since 0.4.0), as [`try_put`](Net::try_put) is
    /// [`put`](Net::put)'s: the selector installed, or the error, the net
    /// unchanged.
    ///
    /// ```
    /// use petritype::NetBuilder;
    ///
    /// let mut net = NetBuilder::new().build()?;
    /// net.try_set_selector(|_net, _fireable| None)?;
    /// net.try_clear_selector()?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// [`NetError::HeldByBody`] when a body of the net around this one
    /// holds it, the selector being the program's; the body's firing fails
    /// with it too, whether or not the body passes it on.
    pub fn try_set_selector<F>(&mut self, selector: F) -> Result<(), NetError>
    where
        F: Fn(&Net, &[TransitionId]) -> Option<TransitionId> + Send + Sync + 'static,
    {
        self.admit_knob(SET_SELECTOR)?;
        self.selector = Some(Box::new(selector));
        Ok(())
    }

    /// Puts the default selector back: highest
    /// [`priority`](crate::TransitionBuilder::priority) first, ties broken by
    /// the net's [`TieBreak`] (the transition declared first, unless set
    /// otherwise).
    ///
    /// The undo of [`set_selector`](Net::set_selector), and the way back to a
    /// deterministic sequential run after a phase that wanted a different
    /// choice (a rule of the caller's own for a demo, a selector that
    /// declines while a user is dragging something). A net that never had one
    /// is unchanged, and the [round robin](TieBreak::RoundRobin)'s cursor
    /// is where the default selector last left it.
    ///
    /// ```
    /// # use petritype::{NetBuilder, TakeEdge};
    /// # let mut b = NetBuilder::new();
    /// # let inbox = b.place::<u32>("Inbox");
    /// # b.port(inbox);
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
    ///
    /// # Panics
    ///
    /// As [`set_selector`](Net::set_selector), for a body that holds the
    /// net. [`try_clear_selector`](Net::try_clear_selector) returns the
    /// error instead.
    pub fn clear_selector(&mut self) {
        self.admit_knob_or_panic(CLEAR_SELECTOR);
        self.selector = None;
    }

    /// [`clear_selector`](Net::clear_selector) for a caller that handles a
    /// refusal (since 0.4.0): the default selector back, or the error, the
    /// net unchanged.
    ///
    /// # Errors
    ///
    /// [`NetError::HeldByBody`] when a body of the net around this one
    /// holds it, as for [`try_set_selector`](Net::try_set_selector).
    pub fn try_clear_selector(&mut self) -> Result<(), NetError> {
        self.admit_knob(CLEAR_SELECTOR)?;
        self.selector = None;
        Ok(())
    }

    /// The transition [`step`](Net::step) would fire right now, chosen for a
    /// firing the caller makes itself: [`select`](Net::select), and the
    /// choice counts.
    ///
    /// It counts for the [round robin](TieBreak::RoundRobin): when the
    /// default selector made the choice, its cursor moves to it, as `step`
    /// would move it, so the next choice is the next in turn. `select` reads
    /// the cursor and never moves it, which is why a driver that fires this
    /// net's own transitions in stages (the choice made here, then
    /// [`launch`](Net::launch), the body on a thread of its own, then
    /// [`distribute`](Net::distribute)) asks here and not there. A custom
    /// selector is asked exactly as `select` asks it, and nothing moves.
    /// `Ok(None)` when nothing is fireable or when the selector declines.
    ///
    /// ```
    /// # use petritype::{NetBuilder, TakeEdge, TieBreak};
    /// # let mut b = NetBuilder::new();
    /// # let inbox = b.place::<u32>("Inbox");
    /// # let out = b.place::<u32>("Out");
    /// b.seed(inbox, [1, 2, 3, 4]);
    /// let first = b.transition("First", (TakeEdge(inbox),), (out,), |n: u32| (n,)).id();
    /// let second = b.transition("Second", (TakeEdge(inbox),), (out,), |n: u32| (n,)).id();
    /// b.tie_break(TieBreak::RoundRobin);
    /// let mut net = b.build()?;
    ///
    /// assert_eq!(net.select()?, Some(first), "no cursor yet: the first declared");
    /// assert_eq!(net.select()?, Some(first), "and asking moves nothing");
    /// let chosen = net.choose()?.expect("both are fireable");
    /// assert_eq!(chosen, first);
    /// let firing = net.launch(chosen)?;
    /// net.distribute(firing.run())?;
    /// assert_eq!(net.select()?, Some(second), "the next in turn");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// [`StepError::SelectionFailed`] when the choice cannot be made (since
    /// 0.4.0): the default selector met a fireable transition whose
    /// priority is `NaN` ([`SelectionFault::NanPriority`]), or the custom
    /// selector answered with a transition it was not offered
    /// ([`SelectionFault::NotOffered`]). Nothing moves, the cursor
    /// included. Until then a `NaN` counted as `0.0` and such an answer was
    /// `None`.
    ///
    /// [`StepError::HeldByBody`] when a body of the net around this one
    /// holds it (since 0.4.0; until then a body could choose): the cursor
    /// would move with no firing, and the tie-break is the program's.
    /// Nothing moves, and the body's firing fails with
    /// [`NetError::HeldByBody`] when it returns, whether or not the body
    /// passed this on, as for a staged step. A step the body makes still
    /// moves the cursor, and [`select`](Net::select), which moves nothing,
    /// reads it.
    pub fn choose(&mut self) -> Result<Option<TransitionId>, StepError> {
        self.admit_step(CHOOSE)?;
        Ok(self.choose_now()?)
    }

    /// [`choose`](Net::choose) without the check: the choice of a step and
    /// of a deep step, which a body may make on a net it holds.
    pub(crate) fn choose_now(&mut self) -> Result<Option<TransitionId>, SelectionFailed> {
        // Selection reads the net and nothing more, so it ends before the
        // cursor is written, and a failed one writes nothing.
        let Some(chosen) = self.select_from(&self.fireable())? else {
            return Ok(None);
        };
        self.count_choice(chosen);
        Ok(Some(chosen))
    }

    /// The choice of one launch of a concurrent sweep: the installed
    /// selector's, or the default rule's, among `offered`, the transitions
    /// fireable now that the sweep has not launched yet, in declaration
    /// order. Nothing moves; [`count_choice`](Net::count_choice) moves the
    /// cursor once the choice is launched.
    pub(crate) fn select_among(
        &self,
        offered: &[TransitionId],
    ) -> Result<Option<TransitionId>, SelectionFailed> {
        self.select_from(offered)
    }

    /// A choice taken to be fired: the [round robin](TieBreak::RoundRobin)'s
    /// cursor moves to it when the default selector made it, under any
    /// tie-break. A custom selector's choice moves nothing.
    pub(crate) fn count_choice(&mut self, chosen: TransitionId) {
        if self.selector.is_none() {
            self.last_pick = Some(chosen);
        }
    }

    /// Changes how the default selector breaks ties between transitions of
    /// equal priority; see [`TieBreak`] for what each choice does to a
    /// pipeline. Takes effect from the next choice, and leaves the
    /// [round robin](TieBreak::RoundRobin)'s cursor where it is. A custom
    /// selector installed with [`set_selector`](Net::set_selector) ignores
    /// it.
    ///
    /// # Panics
    ///
    /// When a body of the net around this one holds it: its tie-break is
    /// the program's, and the body's firing fails with
    /// [`NetError::HeldByBody`]. [`try_set_tie_break`](Net::try_set_tie_break)
    /// returns the error instead.
    pub fn set_tie_break(&mut self, tie_break: TieBreak) {
        self.admit_knob_or_panic(SET_TIE_BREAK);
        self.tie_break = tie_break;
    }

    /// [`set_tie_break`](Net::set_tie_break) for a caller that handles a
    /// refusal (since 0.4.0): the tie-break set, or the error, the net
    /// unchanged.
    ///
    /// ```
    /// use petritype::{NetBuilder, TieBreak};
    ///
    /// let mut net = NetBuilder::new().build()?;
    /// net.try_set_tie_break(TieBreak::RoundRobin)?;
    /// assert_eq!(net.tie_break(), TieBreak::RoundRobin);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// [`NetError::HeldByBody`] when a body of the net around this one
    /// holds it, the tie-break being the program's; the body's firing fails
    /// with it too, whether or not the body passes it on.
    pub fn try_set_tie_break(&mut self, tie_break: TieBreak) -> Result<(), NetError> {
        self.admit_knob(SET_TIE_BREAK)?;
        self.tie_break = tie_break;
        Ok(())
    }

    /// How the default selector breaks ties: as built, or as last set with
    /// [`set_tie_break`](Net::set_tie_break).
    pub fn tie_break(&self) -> TieBreak {
        self.tie_break
    }

    /// The transition [`step`](Net::step) would fire right now, without
    /// firing it: the installed selector's choice from
    /// [`fireable`](Net::fireable), or the default rule's if none is
    /// installed.
    ///
    /// This is the net's own choice, offered to whoever needs it before the
    /// firing rather than after: a visualiser that highlights the next
    /// transition, or a driver that stages the firing itself so that its
    /// body can run elsewhere. Either would otherwise have to re-implement
    /// the rule, and a custom selector, being a closure inside the net,
    /// cannot be re-implemented at all.
    ///
    /// `Ok(None)` when nothing is fireable, or when the selector declines.
    /// Guards and the selector run, exactly as for `step`; nothing in the
    /// net is changed, not even the [round robin](TieBreak::RoundRobin)'s
    /// cursor, so a driver that fires the choice in stages asks
    /// [`choose`](Net::choose) instead. A custom selector that keeps state
    /// of its own counts every call as a choice, so ask it once per firing.
    ///
    /// ```
    /// # use petritype::{NetBuilder, TakeEdge};
    /// # let mut b = NetBuilder::new();
    /// # let inbox = b.place::<u32>("Inbox");
    /// # b.port(inbox);
    /// # let out = b.place::<u32>("Out");
    /// let double = b
    ///     .transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,))
    ///     .id();
    /// let mut net = b.build()?;
    /// assert_eq!(net.select()?, None, "nothing to fire");
    ///
    /// net.put(inbox, 21);
    /// let chosen = net.select()?.expect("Double is fireable");
    /// assert_eq!(chosen, double);
    /// assert_eq!(net.fired_count(), 0, "choosing fires nothing");
    ///
    /// let firing = net.launch(chosen)?;
    /// net.distribute(firing.run())?;
    /// assert_eq!(net.fired_count(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// As [`choose`](Net::choose) (since 2026-11-28): [`SelectionFailed`]
    /// for a `NaN` priority the default selector met, or a custom
    /// selector's answer that is not among the fireable transitions.
    pub fn select(&self) -> Result<Option<TransitionId>, SelectionFailed> {
        self.select_from(&self.fireable())
    }

    /// The choice itself, written once: the installed selector, or the
    /// default rule, over a fireable list the caller already has, and the
    /// check that the answer is one of them.
    fn select_from(
        &self,
        fireable: &[TransitionId],
    ) -> Result<Option<TransitionId>, SelectionFailed> {
        if fireable.is_empty() {
            return Ok(None);
        }
        let Some(selector) = &self.selector else {
            return default_selector(self, fireable);
        };
        let Some(chosen) = selector(self, fireable) else {
            return Ok(None);
        };
        if fireable.contains(&chosen) {
            return Ok(Some(chosen));
        }
        Err(SelectionFailed::new(SelectionFault::NotOffered {
            chosen,
            name: self
                .transitions
                .get(chosen.0 as usize)
                .map(|slot| slot.name.clone()),
            offered: fireable
                .iter()
                .map(|&transition| (transition, self.transition_name(transition)))
                .collect(),
        }))
    }
}

/// The selector a net uses until [`Net::set_selector`] replaces it: highest
/// priority wins, ties go by the net's [`TieBreak`].
///
/// `fireable` is in declaration order. A strict comparison keeps the first
/// transition with the top score in the order it is read, so the first two
/// tie-breaks are nothing more than which way the list is read, as in the
/// Python library; the third is [`round_robin`].
///
/// Every priority is asked, in the order the list is read, and the first
/// `NaN` fails the choice: it ranks neither above nor below any other. No
/// priority is `NaN` past that point, so the comparisons are total, an
/// infinity ranking as itself.
pub(crate) fn default_selector(
    net: &Net,
    fireable: &[TransitionId],
) -> Result<Option<TransitionId>, SelectionFailed> {
    if net.tie_break == TieBreak::RoundRobin {
        return round_robin(net, fireable);
    }
    let mut best: Option<(TransitionId, f64)> = None;
    let mut read = |transition: TransitionId| -> Result<(), SelectionFailed> {
        let score = net.priority_of(transition)?;
        if best.is_none_or(|(_, top)| score > top) {
            best = Some((transition, score));
        }
        Ok(())
    };
    if net.tie_break == TieBreak::LastDeclared {
        fireable
            .iter()
            .rev()
            .try_for_each(|&transition| read(transition))?;
    } else {
        fireable
            .iter()
            .try_for_each(|&transition| read(transition))?;
    }
    Ok(best.map(|(transition, _)| transition))
}

/// [`TieBreak::RoundRobin`]: of the fireable transitions with the top score,
/// in declaration order, the first declared after the cursor, else the first.
///
/// A [`TransitionId`] is a declaration index, so "declared after" is a
/// comparison of ids.
fn round_robin(
    net: &Net,
    fireable: &[TransitionId],
) -> Result<Option<TransitionId>, SelectionFailed> {
    let scored = fireable
        .iter()
        .map(|&transition| Ok((transition, net.priority_of(transition)?)))
        .collect::<Result<Vec<(TransitionId, f64)>, SelectionFailed>>()?;
    let top = scored
        .iter()
        .map(|&(_, score)| score)
        .fold(f64::NEG_INFINITY, f64::max);
    let mut tied = scored
        .iter()
        .filter(|&&(_, score)| score == top)
        .map(|&(transition, _)| transition);
    let after = net
        .last_pick
        .and_then(|cursor| tied.clone().find(|transition| transition.0 > cursor.0));
    Ok(after.or_else(|| tied.next()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NetBuilder;
    use crate::edge::TakeEdge;

    /// A chain `A -> First -> B -> Second -> C` holding three tokens, with
    /// both transitions at the default priority.
    fn pipeline(tie_break: Option<TieBreak>) -> (Net, TransitionId, TransitionId) {
        let mut b = NetBuilder::new();
        let a = b.place::<u32>("A");
        let mid = b.place::<u32>("B");
        let c = b.place::<u32>("C");
        b.seed(a, [1, 2, 3]);
        let first = b
            .transition("First", (TakeEdge(a),), (mid,), |n: u32| (n,))
            .id();
        let second = b
            .transition("Second", (TakeEdge(mid),), (c,), |n: u32| (n,))
            .id();
        if let Some(tie_break) = tie_break {
            b.tie_break(tie_break);
        }
        (b.build().expect("a two-stage pipeline"), first, second)
    }

    /// Every firing `step` makes until the net is quiescent, in order.
    fn run_order(net: &mut Net) -> Vec<TransitionId> {
        std::iter::from_fn(|| net.step().expect("nothing fails")).collect()
    }

    #[test]
    fn the_default_tie_break_drains_a_pipeline_in_batches() {
        let (mut net, first, second) = pipeline(None);
        assert_eq!(net.tie_break(), TieBreak::FirstDeclared);
        assert_eq!(
            run_order(&mut net),
            [first, first, first, second, second, second],
            "every token through stage one before any reaches stage two"
        );
    }

    #[test]
    fn last_declared_lets_each_token_flow_through() {
        let (mut net, first, second) = pipeline(Some(TieBreak::LastDeclared));
        assert_eq!(net.tie_break(), TieBreak::LastDeclared);
        assert_eq!(
            run_order(&mut net),
            [first, second, first, second, first, second],
            "one token all the way through before the next enters"
        );

        // Set on the net, it takes effect from the next choice; and a tie is
        // only a tie between equal priorities.
        let (mut net, first, _) = pipeline(None);
        net.step().expect("First fires");
        net.set_tie_break(TieBreak::LastDeclared);
        assert_ne!(
            net.select().unwrap(),
            Some(first),
            "Second is now preferred"
        );
        let (mut net, first, _) = pipeline(Some(TieBreak::LastDeclared));
        net.step().expect("First fires");
        net.transitions[first.0 as usize].priority = Some(Box::new(|_: &Net| 1.0));
        assert_eq!(
            net.select().unwrap(),
            Some(first),
            "priority beats the tie-break"
        );
    }

    #[test]
    fn a_custom_selector_ignores_the_tie_break() {
        let (mut net, first, _) = pipeline(Some(TieBreak::LastDeclared));
        net.step().expect("First fires, B now has a token");
        net.set_selector(|_net, fireable| fireable.first().copied());
        assert_eq!(
            net.select().unwrap(),
            Some(first),
            "the selector's own rule"
        );
    }

    /// [`Net::select`] asks the *installed* selector, so a custom rule that
    /// no caller could copy (here a round-robin with state of its own) is
    /// what it reports, one choice per firing.
    #[test]
    fn select_honours_a_custom_selector_and_is_none_when_quiescent() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let mut b = NetBuilder::new();
        let inbox = b.place::<u32>("Inbox");
        let out = b.place::<u32>("Out");
        b.seed(inbox, [1, 2, 3, 4]);
        let first = b
            .transition("First", (TakeEdge(inbox),), (out,), |n: u32| (n,))
            .id();
        let second = b
            .transition("Second", (TakeEdge(inbox),), (out,), |n: u32| (n,))
            .id();
        let mut net = b.build().expect("a two-transition net");

        assert_eq!(
            net.select().unwrap(),
            Some(first),
            "the default: declared first"
        );

        let turn = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&turn);
        net.set_selector(move |_net, fireable| {
            let i = counter.load(Ordering::SeqCst);
            fireable.get(i % fireable.len()).copied()
        });

        let mut chosen = Vec::new();
        while let Some(next) = net.select().unwrap() {
            assert_eq!(
                net.select().unwrap(),
                Some(next),
                "asking twice changes nothing"
            );
            net.fire(next).expect("the selector chose a fireable one");
            turn.fetch_add(1, Ordering::SeqCst);
            chosen.push(next);
        }
        assert_eq!(chosen, [first, second, first, second], "round-robin");
        assert_eq!(net.count(out.id()), 4);
        assert_eq!(net.select().unwrap(), None, "quiescent");
    }
}
