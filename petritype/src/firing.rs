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
//! # One firing
//!
//! [`Net::step`] scans for the fireable transitions, asks the selector which
//! one to fire, and fires it inline in three stages:
//!
//! 1. **consume** — take the body out of its slot and take the inputs out of
//!    their places, one argument per input edge ([`consume`]);
//! 2. **run** — call the body with them;
//! 3. **distribute** — mint an id for every token the body returned, push it
//!    into its output place, put the body back, and log what happened
//!    ([`Net::distribute`]).
//!
//! They are three steps of one function here because sequential firing has
//! nothing to do between them. The split is kept so that the concurrent
//! slice can publish stage 1 and stage 3 as `launch` and `distribute` with a
//! `Firing` in between, without rewriting what each stage does.

use crate::edge::EdgeKind;
use crate::erased::{ErasedInput, ErasedInputs, ErasedOutcome};
use crate::error::{FiringFailed, NetError};
use crate::event::Event;
use crate::ids::{PlaceId, TokenId, TransitionId};
use crate::net::Net;
use crate::place::PlaceSlot;
use crate::transition::TransitionSlot;

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

    // ----- firing ---------------------------------------------------------

    /// Fires one transition: scan, select, consume, run, distribute.
    ///
    /// `Ok(Some(t))` says `t` fired. `Ok(None)` says nothing did, either
    /// because nothing was fireable — the net is quiescent — or because the
    /// selector declined. `Err` says the body returned `Err`: the net is
    /// consistent, the firing's inputs are gone, and a further `step` works
    /// (DESIGN §7).
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
    /// A panicking body is not caught yet: the panic unwinds through `step`,
    /// and because the body was taken out of its slot before the call, the
    /// transition stays in flight and is never fireable again. The slice that
    /// adds `catch_unwind` turns that into an ordinary
    /// [`FiringFailed`] with the body back in place.
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
    /// the net is untouched in both cases. A body that returns `Err` gives
    /// [`NetError::FiringFailed`], with the same consequences as for
    /// [`step`](Net::step).
    ///
    /// # Panics
    ///
    /// As [`step`](Net::step): a panicking body is not caught yet.
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

    /// One firing, all three stages, with fireability already established.
    fn fire_now(&mut self, transition: TransitionId) -> Result<(), FiringFailed> {
        let index = transition.0 as usize;
        // Out of its slot for the duration of the call: the body may not
        // borrow the transition list while it runs, and an empty slot is what
        // "in flight" means.
        let mut body = self.transitions[index]
            .body
            .take()
            .expect("a fireable transition has its body in its slot");

        let mut consumed = Vec::new();
        let mut mutated = Vec::new();
        let outcome = {
            // Field by field, so that the input places can be emptied while
            // the edge list is read out of the transition beside them.
            let Net {
                places,
                transitions,
                ..
            } = &mut *self;
            // Stage 1, and stage 2 in the same scope: a `MutateEdge` hands
            // the body a borrow into a place, which has to outlive the call
            // and end before stage 3 writes anything.
            let args = consume(
                places,
                &transitions[index].inputs,
                &mut consumed,
                &mut mutated,
            );
            body(ErasedInputs::new(args))
        };

        // Home before anything else can go wrong, on the failing path too:
        // DESIGN §7 promises a consistent net after a failure, and a body
        // left out of its slot would strand the transition in flight.
        self.transitions[index].body = Some(body);
        self.distribute(transition, outcome, consumed, mutated)
    }

    /// Stage 3: place what the body produced, or account for its failure.
    ///
    /// `consumed` and `mutated` are stage one's records, in edge order.
    fn distribute(
        &mut self,
        transition: TransitionId,
        outcome: ErasedOutcome,
        consumed: Vec<(PlaceId, TokenId)>,
        mutated: Vec<(PlaceId, TokenId)>,
    ) -> Result<(), FiringFailed> {
        let produced = match outcome {
            ErasedOutcome::Ok(tokens) => tokens,
            ErasedOutcome::Err(error) => {
                // A body that mutated a token through a `MutateEdge` and then
                // failed has still mutated it — the change went into the
                // place as it was made, and there is nothing to roll back to
                // — so the mutations are reported before the failure.
                self.emit_mutations(&mutated);
                self.emit(Event::Failed {
                    transition,
                    lost: consumed,
                    restored: false,
                    error: error.to_string(),
                });
                return Err(FiringFailed {
                    transition,
                    error,
                    restored: false,
                });
            }
        };

        // Distribution cannot fail here, so DESIGN §6.2's "every target place
        // is checked before any token is written" holds trivially: every
        // index came from the body's return type, which the builder matched
        // against the declared output places at compile time. A routed body
        // is no different by the time its tokens arrive here — its indices
        // come from the table resolved when it was declared, and
        // `NetBuilder::build` refuses a net whose routing does not resolve —
        // and the `Distribution` form will resolve its own before this point
        // too, so the check stays a `debug_assert!`.
        let mut landed = Vec::with_capacity(produced.len());
        for (index, token) in produced {
            debug_assert!(
                index < self.transitions[transition.0 as usize].outputs.len(),
                "output index {index} is past the outputs of transition {transition:?}"
            );
            let place = self.transitions[transition.0 as usize].outputs[index];
            let token = self.push_produced(place, token);
            landed.push((place, token));
        }

        self.emit_mutations(&mutated);
        self.emit(Event::Fired {
            transition,
            consumed,
            produced: landed,
        });
        self.fired_count += 1;
        Ok(())
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

/// Stage 1: one argument per input edge, in declaration order, taking out of
/// the places whatever each edge kind takes.
///
/// A free function over the places alone, rather than a method, because that
/// is the shape the concurrent slice needs: consuming is what resolves
/// conflicts between transitions, and it must be doable while the rest of the
/// net is borrowed elsewhere.
fn consume<'a>(
    places: &'a mut [PlaceSlot],
    inputs: &[(PlaceId, EdgeKind)],
    consumed: &mut Vec<(PlaceId, TokenId)>,
    mutated: &mut Vec<(PlaceId, TokenId)>,
) -> Vec<ErasedInput<'a>> {
    let mut args = Vec::with_capacity(inputs.len());
    // Where the mutating edges are: the argument each fills and the place it
    // borrows. They are filled in a second pass, because a `&mut` into a
    // place may not be held while the pass that drains the other places is
    // still running.
    let mut borrows: Vec<(usize, usize)> = Vec::new();

    for (position, &(place, kind)) in inputs.iter().enumerate() {
        let index = place.0 as usize;
        match kind {
            EdgeKind::Take => {
                let (token, value) = places[index].tokens.pop_front().expect(NON_EMPTY);
                consumed.push((place, token));
                args.push(ErasedInput::Token(value));
            }
            EdgeKind::TakeAll => {
                let slot = &mut places[index];
                let mut values = Vec::with_capacity(slot.tokens.len());
                for (token, value) in slot.tokens.drain(..) {
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
                borrows.push((position, index));
                // Overwritten by the pass below; a body never sees it.
                args.push(ErasedInput::Taken);
            }
        }
    }

    if !borrows.is_empty() {
        // `slice::get_disjoint_mut` wants its indices as a fixed-size array
        // and a transition's mutating edges are only counted at run time, so
        // the same job is done with `iter_mut`: its items borrow the slice
        // rather than the iterator, so every borrow this pass takes may be
        // alive at once. That they are disjoint is
        // `BuildError::DuplicateInputPlace`'s doing — no two input edges of
        // one transition name the same place.
        for (index, slot) in places.iter_mut().enumerate() {
            let Some(&(position, _)) = borrows.iter().find(|&&(_, place)| place == index) else {
                continue;
            };
            let (_, value) = slot.tokens.front_mut().expect(NON_EMPTY);
            args[position] = ErasedInput::Borrowed(value);
        }
    }

    args
}
