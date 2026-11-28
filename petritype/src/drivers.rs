//! Loops that drive a net: run it to quiescence, or run it on a clock.
//!
//! A driver is *thin*. It owns no state of its own and adds no
//! semantics: it is a loop over the stages [`Net`] already exposes
//! ([`step_deep`](Net::step_deep) for the sequential one,
//! [`launch`](Net::launch) / [`Firing::run`] /
//! [`distribute`](Net::distribute) for the concurrent one), plus a decision
//! about when to stop and what to do about a failure. Everything it knows is
//! in its arguments and everything it did is in the [`RunSummary`] it
//! returns, so two calls in a row continue each other exactly as far as the
//! net's own marking says they do.
//!
//! ```
//! use petritype::drivers::{self, Limits, Mode, OnFailure};
//! use petritype::{NetBuilder, TakeEdge};
//!
//! let mut b = NetBuilder::new();
//! let inbox = b.place::<u32>("Inbox");
//! let middle = b.place::<u32>("Middle");
//! let out = b.place::<u32>("Out");
//! b.seed(inbox, [1, 2, 3]);
//! b.transition("Double", (TakeEdge(inbox),), (middle,), |n: u32| (n * 2,));
//! b.transition("AddOne", (TakeEdge(middle),), (out,), |n: u32| (n + 1,));
//! let mut net = b.build()?;
//!
//! // Three tokens down a two-stage chain: six firings, then nothing left.
//! let summary = drivers::run(&mut net, Mode::Sequential, Limits::default(), OnFailure::Stop)?;
//! assert!(summary.quiesced);
//! assert_eq!((summary.fired_count, summary.failed_count), (6, 0));
//! assert_eq!(net.count(out.id()), 3);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # The two modes
//!
//! [`Mode::Sequential`] is [`Net::step_deep`] in a loop: one transition at a
//! time, chosen by the net's [`Selector`](crate::Selector), each firing
//! complete before the next begins, and, if its tokens carry nets of their
//! own ([`Nested`](crate::Nested)), the inner nets' firings in the order
//! their places' [`Stepping`](crate::Stepping) rules give. It is
//! **deterministic given the selectors**, which is what makes a sequential
//! run the thing to write a
//! test or a replay against, and what makes [`Net::set_selector`] a way to
//! shape a whole run rather than one step.
//!
//! [`Mode::Concurrent`] takes the net's own turn as a **launch sweep**: it
//! [`launch`](Net::launch)es one transition after another, for as long as
//! any is fireable, each detachable body on a worker thread, then
//! distributes the completions as they arrive over an [`mpsc`] channel.
//! Consuming the inputs synchronously in the sweep is what resolves
//! conflicts: a peer that wanted one of those tokens is simply not fireable
//! any more, and no lock is involved. Three consequences are
//! worth saying out loud:
//!
//! - **Each launch is the net's own choice**, made as a sequential step
//!   makes it (since 2026-11-28; until then the sweep launched in
//!   declaration order and the selector was not asked): the custom
//!   [`Selector`](crate::Selector) if one is set, else the highest
//!   [`priority`](crate::TransitionBuilder::priority) with ties by the
//!   [`TieBreak`](crate::TieBreak), the round robin's cursor moving with
//!   each launch. It chooses among the transitions fireable now that the
//!   sweep has not launched yet, and after each launch it chooses afresh,
//!   its guards and priorities asked again, until nothing is left to launch
//!   or the selector declines. So each choice is the one a sequential step
//!   would make on the marking as the sweep has left it, in which the bodies
//!   launched so far have taken their inputs and not yet landed, and a net
//!   that sets no selector, no priority and the default tie-break launches
//!   in declaration order. A choice that cannot be made stops the run as it
//!   does in sequential mode.
//! - **Order is wall-clock order.** Completions are distributed as they come
//!   back, so the event log and the final token ids depend on how long the
//!   bodies took. One flight per transition at a time, though: a transition
//!   in flight is not fireable, so it is never launched twice over.
//! - **The threads are bounded** (since 2026-11-28; until then each launch
//!   had a thread of its own). The bodies run on a pool of worker threads
//!   inside a [`std::thread::scope`], at most
//!   [`available_parallelism`](std::thread::available_parallelism) of them
//!   for [`Mode::Concurrent`] and at most `workers` for
//!   [`Mode::ConcurrentWith`], the bodies of the nets inside the net
//!   included ([below](self#nets-inside-the-net)). A worker is spawned only when a body is
//!   handed over, no worker is free to take it, and the cap is not
//!   reached, so a call that never has more than two bodies airborne
//!   spawns two threads at most, and one whose firings all run inline
//!   spawns none. A body handed over while every worker is busy waits for
//!   one, and every worker is joined before the driver returns. A body
//!   that waits for another body to start therefore needs a cap that lets
//!   both run.
//!
//! A transition that is not
//! [detachable](crate::TransitionInfo::detachable) (one with a mutating or a
//! read edge, such as a [`MutateEdge`](crate::MutateEdge), whose body borrows
//! a token where it lies) has already run by the time
//! [`launch`](Net::launch) returns, so
//! the concurrent driver distributes it on the spot. It has had its turn in
//! that sweep and is not offered again until the next one, and the driver
//! passes again at once, without waiting for a body in flight to land,
//! rather than calling the net quiescent: the marking moved.
//!
//! # Nets inside the net
//!
//! The nets inside the net, a token's nets and a subnet place's subnet, at
//! any depth, launch beside it (since 0.4.0; until then they fired one
//! transition at a time, inline on the driver's thread, between sweeps).
//! Each time round its loop the concurrent driver makes one **pass** over
//! the whole tree, in which every net takes its round as its places'
//! [`Stepping`](crate::Stepping) rules order it, and each net's own turn in
//! it is a launch sweep of its own transitions, every launch its own
//! choice, every detachable body handed to the call's one pool of workers.
//! The rules keep their meaning; what is new is that the nets a rule lets
//! go go together:
//!
//! - the nets of the **inner-first** places launch first, all of them, and
//!   the rest of the round waits until every one is at rest, having
//!   nothing to launch and nothing in flight, so that the net around them
//!   fires only with the inside at rest;
//! - the **ring** comes next: the net's own sweep and a round of every net
//!   of the take-turns places, each in every pass, from where the ring's
//!   cursor says, so that none waits for another to come to rest;
//! - the nets of the **outer-first** places launch, all of them, once every
//!   net of the ring is at rest and nothing of the net's own is in flight;
//! - the nets of **passive** places, and the nets a body holds, are never
//!   stepped by the driver.
//!
//! A firing in flight inside lands in the net that launched it, found by
//! the way down to it, each net on the way by its id, so two nets of one
//! name in one token are told apart; what each net on the way logged is
//! lifted into the net around it as it lands. A token whose nets have a
//! firing in flight is **busy** until every one has landed. An edge that
//! takes one token or lends one to change ([`TakeEdge`](crate::TakeEdge),
//! [`TakeWhereEdge`](crate::TakeWhereEdge),
//! [`MutateEdge`](crate::MutateEdge),
//! [`MutateWhereEdge`](crate::MutateWhereEdge)) passes it over, as if it
//! were not in its place, and a transition that could only use busy tokens
//! is not fireable for now. A [`TakeAllEdge`](crate::TakeAllEdge) or a
//! [`DrainEdge`](crate::DrainEdge) takes every token of its place, as it is
//! drawn, so its transition is not fireable while any of them is busy, and
//! fires once they have all landed. Meanwhile it **holds its place**: when
//! the transition would be fireable but for those busy tokens, its switch,
//! its guard and its other inputs all letting it, the driver launches
//! nothing new inside the nets that place's tokens carry, at any depth,
//! until the transition has had its turn, so that what is in flight lands
//! and the place comes to rest. The transition is then offered to the net's
//! ordinary choice among whatever else is fireable. Once it has been
//! offered, chosen or not, or once it is no longer fireable otherwise, it
//! holds the place no more and the nets go on, so a selector that never
//! chooses it slows those nets but never stops them for good. Only the
//! nets of that place are held. A read edge and a guard see a busy token
//! as it stands, the firing in flight and all, as a guard sees its own net
//! while one of its own firings is in flight. A subnet is held still while
//! a firing of the net around it on one of its ports is in flight, as in a
//! deep step.
//!
//! The order of the inner nets is therefore wall-clock order too, within
//! what the rules allow; only the sequential driver promises an order. On a
//! net whose firings do not compete, the sequential and the concurrent
//! drivers fire the same transitions and leave the same marking.
//!
//! # Limits: pacing and the fuse
//!
//! [`Limits`] holds two caps, and both count **launches** (attempts,
//! including ones that go on to fail, inside inner nets as well as in the net
//! itself) rather than successes.
//!
//! - [`stop_after_n_firings`](Limits::stop_after_n_firings) is *pacing*: the
//!   driver stops launching and returns `Ok`, with
//!   [`quiesced`](RunSummary::quiesced) false to say that work may remain.
//!   Call again to continue; nothing resumes by itself, because everything
//!   that survives the call is in the net.
//! - [`error_after_n_firings`](Limits::error_after_n_firings) is a *fuse*:
//!   the driver returns [`DriverError::TooManyFirings`] **before** the
//!   offending launch consumes anything. It is the guard against a net that
//!   will not stop, most sharply against a transition that
//!   [restores its inputs](crate::TransitionBuilder::restore_tokens_on_failure)
//!   and fails every time, which would otherwise put its tokens back and try
//!   again for ever.
//!
//! Both are **per call**. This is a deliberate difference from Python
//! petritype, where the fuse lived on a `RunContext` shared between calls:
//! here `Limits` is a plain `Copy` argument, so there is no hidden state to
//! carry a half-spent fuse from one run into the next. A budget that spans
//! calls is the caller's to keep.
//!
//! ```
//! # use petritype::drivers::{self, Limits, Mode, OnFailure};
//! # use petritype::{NetBuilder, TakeEdge};
//! # let mut b = NetBuilder::new();
//! # let inbox = b.place::<u32>("Inbox");
//! # let middle = b.place::<u32>("Middle");
//! # let out = b.place::<u32>("Out");
//! # b.seed(inbox, [1, 2, 3]);
//! # b.transition("Double", (TakeEdge(inbox),), (middle,), |n: u32| (n * 2,));
//! # b.transition("AddOne", (TakeEdge(middle),), (out,), |n: u32| (n + 1,));
//! # let mut net = b.build()?;
//! let paced = Limits {
//!     stop_after_n_firings: Some(2),
//!     ..Limits::default()
//! };
//! let first = drivers::run(&mut net, Mode::Sequential, paced, OnFailure::Stop)?;
//! assert!(!first.quiesced, "a limit ended the call, not the net");
//! assert_eq!(first.fired_count, 2);
//!
//! // The net kept everything; a second call finishes the job.
//! let rest = drivers::run(&mut net, Mode::Sequential, Limits::default(), OnFailure::Stop)?;
//! assert!(rest.quiesced);
//! assert_eq!(rest.fired_count, 4);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Failure
//!
//! A firing fails when its body returns `Err`, panics, or names a place it
//! may not write to. The net is consistent either way by the time
//! the driver hears about it, so the only open question is whether the loop
//! goes on, and [`OnFailure`] answers it: [`Stop`](OnFailure::Stop) ends the
//! run and reports the *first* failure, [`Disable`](OnFailure::Disable)
//! switches the offending transition off and keeps going, and
//! [`Continue`](OnFailure::Continue) keeps going as if nothing had happened.
//!
//! A choice the net cannot make is not a failed firing: a priority that is
//! `NaN`, or a custom selector that names a transition it was not offered
//! ([`SelectionFailed`]), fires nothing that could be switched off or
//! passed over, so the run stops with [`DriverError::SelectionFailed`]
//! whatever [`OnFailure`] says (since 2026-11-28).
//!
//! The driver emits no events of its own. Everything an observer sees comes
//! from the net: one [`Failed`](crate::Event::Failed) per failed firing, and
//! a [`Toggled`](crate::Event::Toggled) for each transition
//! [`Disable`](OnFailure::Disable) switches off.
//!
//! # A body that drives a net it holds
//!
//! A body may drive a net it holds as a token, in either mode, with the
//! same result, as it may step one: the driver launches what
//! the net's own rules make fireable and chooses nothing by name. A run
//! with [`OnFailure::Disable`] is refused before anything fires, with
//! [`DriverError::HeldByBody`], since a switch of a net a body holds is the
//! program's; the body's firing fails with
//! [`NetError::HeldByBody`] when it returns. Driven concurrently, the nets
//! inside the held net launch beside it as they do under the program, on
//! the pool of the body's own call.
//!
//! # Nothing is left in flight
//!
//! A driver never returns while a firing is airborne, not even on the way out
//! of an error. A [`Firing`] or
//! [`Completed`] that is dropped instead of distributed
//! takes its transition's body with it and **strands that transition in
//! flight for good** (see the `#[must_use]` notes on both types). So the
//! concurrent driver, once it has decided to stop, stops *launching* and then
//! waits for every completion it is still owed, distributing each one
//! (failures among them are counted in
//! [`failed_count`](RunSummary::failed_count) and logged, but the error that
//! is returned stays the first one), in whichever net inside it launched
//! it. `net.in_flight()` is empty whenever one of these functions returns,
//! and so is the `in_flight()` of every net inside it the driver launched
//! in.
//!
//! That holds for a panic too (since 2026-11-28). A panic of the net's own
//! code on the driver's thread, a guard, a priority, a selector, a key
//! extractor or `Clone` met in a launch or a landing, in the net or in a
//! net inside it, or the `each_tick` of [`run_indefinitely`], stops the
//! launching as an error does: the concurrent driver waits for every
//! firing airborne and distributes it, and the panic goes on to the caller
//! after that, with every worker joined and what each net inside had
//! logged lifted into the net around it. A body's own panic is a failure, never a panic of the driver
//! ([`Firing::run`] catches it).

use std::any::Any;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::num::NonZeroUsize;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SendError, Sender};
use std::sync::{Mutex, PoisonError};
use std::thread::{self, Scope};
use std::time::{Duration, Instant};

use crate::closed::DISABLE_ON_FAILURE;
use crate::deep::{Busy, Holds, Level, Turn, Way};
use crate::error::{DistributeError, FiringFailed, NetError, PathStep, SelectionFailed, StepError};
use crate::firing::{Completed, Firing};
use crate::ids::{NetId, TransitionId};
use crate::nested::{DeepStep, Member};
use crate::net::Net;

/// How a driver fires: one at a time through the selector, or every fireable
/// transition launched at once, each launch chosen by the selector.
///
/// The default is [`Sequential`](Mode::Sequential), which is the
/// reproducible one.
///
/// ```
/// use std::num::NonZeroUsize;
///
/// use petritype::drivers::{self, Limits, Mode, OnFailure};
/// use petritype::{NetBuilder, TakeEdge};
///
/// let mut b = NetBuilder::new();
/// let inbox = b.place::<u32>("Inbox");
/// let out = b.place::<u32>("Out");
/// b.seed(inbox, [1, 2]);
/// b.transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,));
/// b.transition("Triple", (TakeEdge(inbox),), (out,), |n: u32| (n * 3,));
/// let mut net = b.build()?;
///
/// // Both bodies are airborne at once, and one worker runs them in turn.
/// let one = Mode::ConcurrentWith { workers: NonZeroUsize::MIN };
/// let summary = drivers::run(&mut net, one, Limits::default(), OnFailure::Stop)?;
/// assert_eq!(summary.fired_count, 2);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// [`Net::step_deep`] in a loop: one firing at a time, in the net or in
    /// one of its inner nets, in the order the
    /// [`Stepping`](crate::Stepping) rules give; the net's own turn is its
    /// [`Selector`](crate::Selector)'s choice, fired to completion before the
    /// loop asks again. Deterministic given the selectors.
    #[default]
    Sequential,
    /// Launch everything fireable, one launch after another, each the
    /// net's own choice as a step would make it among what is still
    /// fireable, and run each detachable body on a worker thread inside a
    /// [`std::thread::scope`]: at most
    /// [`available_parallelism`](std::thread::available_parallelism) of
    /// them (one when that is unknown), spawned as the bodies need them.
    /// Completion order is wall-clock order. The nets inside the net, at
    /// any depth, launch beside it in the same way, onto the same workers,
    /// as the [`Stepping`](crate::Stepping) rules let them go (since 0.4.0;
    /// until then they fired inline, one firing at a time, between the
    /// sweeps); see [the module docs](self#nets-inside-the-net).
    Concurrent,
    /// [`Concurrent`](Mode::Concurrent), with at most `workers` worker
    /// threads (since 2026-11-28): the cap for a program that runs several
    /// nets side by side, or that wants the bodies to queue rather than
    /// fill the machine. With one worker the bodies run one at a time, off
    /// the driver's thread, while the driver goes on launching. The cap is
    /// for every body of the call, the nets inside the net included.
    ConcurrentWith {
        /// The most worker threads the call runs bodies on at once.
        workers: NonZeroUsize,
    },
}

/// Limits on one call of [`run`]. Both count launches, not successes.
///
/// `Default` is no limit at all: run until the net quiesces. See the
/// [module docs](self#limits-pacing-and-the-fuse) for why a limit is per call
/// rather than per net.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Limits {
    /// Pacing: stop launching once this many firings have been attempted in
    /// this call, and return `Ok` with [`RunSummary::quiesced`] false.
    ///
    /// The net keeps whatever is left, so calling [`run`] again continues
    /// from exactly here. In [`Mode::Concurrent`] the cap is on *launches*:
    /// firings already airborne are waited for and distributed before the
    /// call returns.
    pub stop_after_n_firings: Option<u64>,
    /// The fuse: once this many firings have been attempted in this call, the
    /// next attempt is [`DriverError::TooManyFirings`] instead, raised before
    /// that launch consumes anything.
    ///
    /// `Some(0)` trips on the first fireable transition. A net that quiesces
    /// at exactly the limit does not trip at all, because nothing attempted
    /// to fire past it.
    pub error_after_n_firings: Option<u64>,
}

/// What a driver does when a firing fails.
///
/// A failure is a body that returned `Err`, a body that panicked (caught, and
/// reported as [`BodyPanicked`](crate::BodyPanicked)), or a
/// [`Distribution`](crate::Distribution) naming a place the transition may
/// not write to. The net is consistent in all three cases before the driver
/// sees it, so this is only about the loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnFailure {
    /// Stop launching and return [`DriverError::FiringFailed`] carrying the
    /// **first** failure of the call.
    ///
    /// In [`Mode::Concurrent`] the firings already airborne are still waited
    /// for and distributed first (see
    /// [the module docs](self#nothing-is-left-in-flight)), so later failures
    /// may be in the event log and in
    /// [`failed_count`](RunSummary::failed_count) even though the error
    /// reports the first.
    #[default]
    Stop,
    /// Switch the failing transition off with [`Net::disable`], which
    /// appends a [`Toggled`](crate::Event::Toggled) event, and keep going.
    ///
    /// The way to let the rest of a net make progress around one broken
    /// transition, and the way to make a failing transition stop being
    /// fireable, so that the run can still reach quiescence.
    Disable,
    /// Keep going as if nothing had happened: the failure is in the event log
    /// and in [`failed_count`](RunSummary::failed_count), and nowhere else.
    ///
    /// Sound when a failure consumes its inputs, since the marking then
    /// shrinks with every attempt. Combined with
    /// [`restore_tokens_on_failure`](crate::TransitionBuilder::restore_tokens_on_failure)
    /// it will retry for ever, so pair it with
    /// [`error_after_n_firings`](Limits::error_after_n_firings).
    Continue,
}

/// What one call of [`run`] or [`run_indefinitely`] did.
///
/// Counts are for **this call**: `fired_count` is the delta of
/// [`Net::fired_count`] across it plus the firings it made inside inner nets,
/// and `failed_count` counts the failures the driver handled, wherever they
/// were. `Default` is the zero summary of a call that did nothing
/// and found nothing to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RunSummary {
    /// True: there was nothing left to fire when the call returned. The net
    /// is quiescent and nothing is in flight, inner nets included
    /// ([`Net::is_quiescent_deep`]).
    ///
    /// False: a limit or, for [`run_indefinitely`], the `stop` flag ended the
    /// call, and work may remain. Note the asymmetry: a call capped by
    /// [`stop_after_n_firings`](Limits::stop_after_n_firings) reports false
    /// even if the net happens to have nothing left, because the driver
    /// stopped before it could find out. The summary carried by a
    /// [`DriverError`] is always false.
    pub quiesced: bool,
    /// Firings that completed successfully during this call, in the net and
    /// in its inner nets.
    pub fired_count: u64,
    /// Firings that failed during this call, whatever [`OnFailure`] then did
    /// about them. Each one is an [`Event::Failed`](crate::Event::Failed) in
    /// the net's log.
    pub failed_count: u64,
}

/// A run that ended badly: the fuse blew, a firing failed under
/// [`OnFailure::Stop`], a net could not choose what to fire, or a body asked
/// [`OnFailure::Disable`] of a net it holds.
///
/// The first three carry the [`RunSummary`] of the interrupted call, because
/// what the run managed to do before it stopped is exactly as interesting as
/// why it stopped. None leaves anything in flight.
///
/// # Names and ids
///
/// A driver error escapes the scope the net lives in even more readily than
/// the rest: `drivers::run(&mut net, ..)?` in `main` prints the message and
/// exits the process, and by then `TransitionId(2)` says nothing to the
/// person reading the log. So the transition is named as well as numbered:
/// here in [`TooManyFirings`](DriverError::TooManyFirings), and inside the
/// [`FiringFailed`] that [`FiringFailed`](DriverError::FiringFailed) carries.
/// The ids stay as the fields a program matches on.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DriverError {
    /// [`error_after_n_firings`](Limits::error_after_n_firings) was spent and
    /// a transition attempted another firing.
    ///
    /// Raised *before* that launch consumes anything, so the named transition
    /// did not fire and the marking still holds its inputs. The net is
    /// otherwise exactly as the firings that did happen left it.
    #[error(
        "the fuse of {limit} firings is spent, and transition {name:?} ({transition:?}) \
         attempted another"
    )]
    TooManyFirings {
        /// The limit that was set.
        limit: u64,
        /// The transition that was about to be launched, and was not.
        transition: TransitionId,
        /// That transition's name, for the message; the id is the field to
        /// match on.
        name: String,
        /// What the interrupted call had done by then.
        summary: RunSummary,
    },
    /// A firing failed while the driver was running under
    /// [`OnFailure::Stop`].
    ///
    /// The first failure of the call, even in [`Mode::Concurrent`] where
    /// firings that were already airborne are distributed on the way out and
    /// may fail too; [`summary`](DriverError::FiringFailed::summary) counts
    /// all of them.
    #[error("the run stopped on a failed firing: {error}")]
    FiringFailed {
        /// What the interrupted call had done by then.
        summary: RunSummary,
        /// The failure itself, which names the transition, the error the body
        /// produced, and whether the consumed tokens were restored.
        #[source]
        error: FiringFailed,
    },
    /// A net whose turn it was could not choose what to fire (since
    /// 2026-11-28): a priority that is `NaN`, or a custom selector's answer
    /// that is not among the fireable transitions. See [`SelectionFailed`].
    ///
    /// Whatever [`OnFailure`] says, since nothing fired that could be
    /// switched off or passed over: the run stops launching, and in
    /// [`Mode::Concurrent`] the firings already airborne are distributed
    /// first. Nothing of the failed choice is in the marking, the log or
    /// [`failed_count`](RunSummary::failed_count).
    #[error("the run stopped on a choice it could not make: {error}")]
    SelectionFailed {
        /// What the interrupted call had done by then.
        summary: RunSummary,
        /// The choice, which says why and in which net.
        #[source]
        error: SelectionFailed,
    },
    /// A body of the net around this one holds it, and asked for a run with
    /// [`OnFailure::Disable`], which would switch its failing transitions
    /// off (since 2026-11-28). A switch of a net a body holds is the
    /// program's, as [`Net::disable`] is.
    ///
    /// Refused before anything fires, the net unchanged, in either mode and
    /// by [`run`] and [`run_indefinitely`] alike. The body's firing fails
    /// with [`NetError::HeldByBody`] when it returns, whether or not the
    /// body passed this on. Drive the net with [`OnFailure::Stop`] or
    /// [`OnFailure::Continue`] instead.
    #[error("{}", crate::error::held_message(.net, DISABLE_ON_FAILURE))]
    HeldByBody {
        /// The net, by the template that built it, or by its id
        /// (`NetId(7)`) when no template did.
        net: String,
    },
}

/// Drives a net until it has nothing left to fire, or until a limit stops it.
///
/// The workhorse: it subsumes Python petritype's `execute_graph`,
/// `Runner.run` and `run_to_completion`, which differed only in their limits
/// and their return type. `mode` picks the loop, `limits` says
/// when to give up, and `on_failure` says what a failed firing means for the
/// rest of the run. See the [module docs](self) for all three.
///
/// A returned [`RunSummary`] with [`quiesced`](RunSummary::quiesced) true is
/// the net's own finish line: nothing is fireable and nothing is in flight,
/// in the net or in the inner nets its tokens carry.
/// False means a limit ended the call and another call continues it.
///
/// ```
/// use petritype::drivers::{self, Limits, Mode, OnFailure};
/// use petritype::{NetBuilder, TakeEdge};
///
/// let mut b = NetBuilder::new();
/// let left = b.place::<u32>("Left");
/// let right = b.place::<u32>("Right");
/// let out = b.place::<u32>("Out");
/// b.seed(left, [1]);
/// b.seed(right, [10]);
/// b.transition("Double", (TakeEdge(left),), (out,), |n: u32| (n * 2,));
/// b.transition("Halve", (TakeEdge(right),), (out,), |n: u32| (n / 2,));
/// let mut net = b.build()?;
///
/// // Two independent transitions, so both bodies run at once, each on a
/// // worker thread. Which token lands first is wall-clock order.
/// let summary = drivers::run(&mut net, Mode::Concurrent, Limits::default(), OnFailure::Stop)?;
/// assert!(summary.quiesced);
/// assert_eq!(summary.fired_count, 2);
/// assert!(net.in_flight().is_empty(), "a driver never returns mid-flight");
/// assert_eq!(net.count(out.id()), 2);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Errors
///
/// [`DriverError::TooManyFirings`] when the fuse is spent and something still
/// wants to fire, [`DriverError::FiringFailed`] when a firing fails under
/// [`OnFailure::Stop`], and [`DriverError::SelectionFailed`] when a net whose
/// turn it was could not choose, under any [`OnFailure`]. Each carries a
/// [`RunSummary`] of what the call managed first, and each leaves the net
/// consistent with nothing in flight.
/// [`DriverError::HeldByBody`], before anything fires, when a body that
/// holds the net asks for [`OnFailure::Disable`].
///
/// # Panics
///
/// Not for a panicking body: [`Firing::run`] catches it
/// and it arrives as an ordinary failure, so a worker thread never dies of
/// one. A panic of the net's own code on the driver's thread (a guard, a
/// priority, a selector, a key extractor, a token's or a parameter's
/// `Clone`, [`Nested::nets_mut`](crate::Nested::nets_mut)), in the net or in
/// a net inside it, goes on to the caller: in [`Mode::Sequential`] at once,
/// as from [`Net::step_deep`], and in [`Mode::Concurrent`] once every firing
/// airborne has landed (see
/// [the module docs](self#nothing-is-left-in-flight)).
pub fn run(
    net: &mut Net,
    mode: Mode,
    limits: Limits,
    on_failure: OnFailure,
) -> Result<RunSummary, DriverError> {
    admit(net, on_failure)?;
    let mut tally = Tally::new(net);
    match mode {
        Mode::Sequential => run_sequential(net, limits, on_failure, &mut tally),
        Mode::Concurrent => run_concurrent(net, limits, on_failure, &mut tally, None),
        Mode::ConcurrentWith { workers } => {
            run_concurrent(net, limits, on_failure, &mut tally, Some(workers))
        }
    }
}

/// Drives a net on a clock until `stop` is set: the real-time regime.
///
/// The difference from [`run`] is the whole point: this loop **never returns
/// because nothing is fireable**. An idle tick means the net is waiting
/// (for input from outside, or for a time-based guard), not that it is
/// finished. It ends when `stop` is set, which is checked at the top of
/// every tick, after `each_tick` and again before every launch, so a stop
/// is acted on promptly rather than at the end of the sweep. The loop reads
/// it with [`Ordering::Acquire`], so a thread that sets it with `Release`
/// has what it wrote before seen by the loop.
///
/// A tick is: call `each_tick`, fire, then wait for `tick`. The wait is a
/// whole `tick` counted from the end of the firing, not the rest of a tick
/// counted from its start, so a tick lasts `tick` and what `each_tick` and
/// the firing took besides. In [`Mode::Sequential`] "fire" is one
/// [`step_deep`](Net::step_deep) (one firing per tick, which is what an
/// animation wants, one firing per frame), and the wait is a
/// [`sleep`](std::thread::sleep). In [`Mode::Concurrent`] it is one pass
/// over the net and the nets inside it, each launching what its rules let
/// go ([the module docs](self#nets-inside-the-net)); and the wait is a
/// [`recv_timeout`](std::sync::mpsc::Receiver::recv_timeout) on the
/// completion channel until `tick` has passed, so completions are
/// distributed the moment they arrive instead of at the next tick. A
/// landing does not end the wait early: the next pass comes when `tick`
/// has passed. A stop set during the wait is seen when the wait ends, or,
/// in [`Mode::Concurrent`], as soon as a completion lands. A tick too long
/// for the clock to reach its end, such as [`Duration::MAX`], never ends:
/// a concurrent one lands completions as they come and sees a stop only
/// after one lands, so with nothing in flight it waits for good, as a
/// sequential tick sleeps that long.
///
/// There are no [`Limits`]: an indefinite run has no natural firing budget,
/// and the caller already holds the `stop` flag. `on_failure` works as it
/// does for [`run`], including [`Stop`](OnFailure::Stop), which drains what
/// is in flight and then returns the failure.
///
/// # `each_tick`, instead of an inbox
///
/// `each_tick(&mut Net)` is called once at the start of every tick, before
/// the firing, and it is the only hook this loop has. Drain your own channel
/// in it, apply commands, poll a UI, top up a source place, whatever the
/// caller's side of the loop is. An `inbox: Option<Receiver<Command>>`
/// parameter would have done less: a closure is strictly more general, and
/// it keeps the driver channel-agnostic, the channels between a UI thread
/// and the driver being the caller's concern. The net is the single writer
/// either way, since `each_tick` is called by this loop and holds the same
/// `&mut Net`.
///
/// ```
/// use std::sync::atomic::{AtomicBool, Ordering};
/// use std::time::Duration;
///
/// use petritype::drivers::{self, Mode, OnFailure};
/// use petritype::{NetBuilder, TakeEdge};
///
/// let mut b = NetBuilder::new();
/// let inbox = b.place::<u32>("Inbox");
/// let out = b.place::<u32>("Out");
/// b.port(inbox);
/// b.transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,));
/// let mut net = b.build()?;
///
/// // The caller's own end condition, written into `each_tick`: feed the net
/// // three tokens, one per tick, then ask the loop to stop.
/// let stop = AtomicBool::new(false);
/// let mut sent = 0_u32;
/// let summary = drivers::run_indefinitely(
///     &mut net,
///     Mode::Sequential,
///     Duration::from_millis(1),
///     &stop,
///     OnFailure::Stop,
///     |net| {
///         if sent < 3 {
///             net.put(inbox, sent);
///             sent += 1;
///         } else if net.fireable().is_empty() {
///             stop.store(true, Ordering::Release);
///         }
///     },
/// )?;
/// assert_eq!(summary.fired_count, 3);
/// assert!(summary.quiesced, "nothing was fireable when the stop was seen");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Errors
///
/// [`DriverError::FiringFailed`] when a firing fails under
/// [`OnFailure::Stop`], [`DriverError::SelectionFailed`] when a net could
/// not choose, and [`DriverError::HeldByBody`] as for [`run`]. The
/// fuse has no place here, so [`DriverError::TooManyFirings`] is never
/// returned.
///
/// # Panics
///
/// Not for a panicking body, as for [`run`]. A panic of `each_tick`, or of
/// the net's own code on the driver's thread, goes on to the caller as for
/// [`run`], in [`Mode::Concurrent`] once every firing airborne has landed.
pub fn run_indefinitely<F>(
    net: &mut Net,
    mode: Mode,
    tick: Duration,
    stop: &AtomicBool,
    on_failure: OnFailure,
    each_tick: F,
) -> Result<RunSummary, DriverError>
where
    F: FnMut(&mut Net),
{
    admit(net, on_failure)?;
    let mut tally = Tally::new(net);
    let clock = Clock { tick, stop };
    match mode {
        Mode::Sequential => tick_sequential(net, clock, on_failure, each_tick, &mut tally),
        Mode::Concurrent => tick_concurrent(net, clock, on_failure, each_tick, &mut tally, None),
        Mode::ConcurrentWith { workers } => {
            tick_concurrent(net, clock, on_failure, each_tick, &mut tally, Some(workers))
        }
    }
}

/// What [`run_indefinitely`] runs by: the length of a tick and the flag
/// that ends the call.
#[derive(Clone, Copy)]
struct Clock<'a> {
    tick: Duration,
    stop: &'a AtomicBool,
}

impl Clock<'_> {
    /// Whether the caller has asked the loop to stop, read with
    /// [`Ordering::Acquire`].
    fn stopped(self) -> bool {
        self.stop.load(Ordering::Acquire)
    }
}

// ----- the loops -----------------------------------------------------------

/// [`Mode::Sequential`] for [`run`]: stop check, fuse check, one
/// [`step_deep`](Net::step_deep), repeat.
fn run_sequential(
    net: &mut Net,
    limits: Limits,
    on_failure: OnFailure,
    tally: &mut Tally,
) -> Result<RunSummary, DriverError> {
    loop {
        if tally.capped(limits) {
            return Ok(tally.summary(net, false));
        }
        // The selector is deliberately not consulted: an enabled net with a
        // spent fuse counts as attempting to fire, as in Python's
        // `_check_fuse`, and so does an inner net with something fireable.
        // Nothing fireable anywhere falls through to the `step_deep` below,
        // which reports the quiescence.
        if let Some(limit) = limits.error_after_n_firings
            && tally.launched >= limit
            && let Some((transition, name)) = net.first_fireable_deep()
        {
            return Err(DriverError::TooManyFirings {
                limit,
                transition,
                name,
                summary: tally.summary(net, false),
            });
        }
        match net.step_deep() {
            Ok(Some(step)) => tally.landed(&step),
            // Nothing fireable, or the selector declined: either way this
            // driver would do nothing more.
            Ok(None) => return Ok(tally.summary(net, true)),
            Err(error) => step_failed(net, error, on_failure, tally)?,
        }
    }
}

/// [`Mode::Concurrent`] for [`run`]: pass, wait, land, repeat.
///
/// The passes are [`run_turns`], inside a [`thread::scope`] that holds the
/// [`Crew`]. However they end, the firings still airborne, in the net and
/// in the nets inside it, are landed before the call returns: when the net
/// is at rest there are none; when a limit, the fuse, a failure or a choice
/// that cannot be made said to stop launching, they are waited for and
/// distributed and then the call reports; and when the net's own code
/// panicked on this thread, they are waited for and distributed and then
/// the panic goes on.
fn run_concurrent(
    net: &mut Net,
    limits: Limits,
    on_failure: OnFailure,
    tally: &mut Tally,
    workers: Option<NonZeroUsize>,
) -> Result<RunSummary, DriverError> {
    let (quiesced, ending) = with_crew(
        net,
        on_failure,
        tally,
        workers,
        |net, crew, tally, ending| run_turns(net, crew, limits, on_failure, tally, ending),
    );

    if let Some((limit, transition, name)) = ending.fuse {
        return Err(DriverError::TooManyFirings {
            limit,
            transition,
            name,
            summary: tally.summary(net, false),
        });
    }
    if let Some(error) = ending.first_failure {
        return Err(stopped_by(error, tally.summary(net, false)));
    }
    Ok(tally.summary(net, quiesced))
}

/// The passes of a concurrent [`run`], one time round the loop each: a pass
/// over the net and every net inside it ([`Pass::round`]), then a wait for
/// what lands.
///
/// True when the net came to rest: a pass launched nothing anywhere and
/// nothing is airborne. False when a limit, the fuse, a failure or a choice
/// that cannot be made said to stop launching, which `ending` records; what
/// is still airborne is the caller's to land.
fn run_turns(
    net: &mut Net,
    crew: &mut Crew<'_, '_>,
    limits: Limits,
    on_failure: OnFailure,
    tally: &mut Tally,
    ending: &mut Ending,
) -> bool {
    loop {
        if tally.capped(limits) {
            ending.capped = true;
        }
        let swept = if ending.is_clear() {
            let mut pass = Pass {
                crew: &mut *crew,
                limits,
                clock: None,
                on_failure,
                tally: &mut *tally,
                ending: &mut *ending,
            };
            pass.round(net, &mut Way::new())
        } else {
            Swept::default()
        };
        if !ending.is_clear() {
            return false;
        }
        crew.holds.prune(net);
        if swept.launched == 0 && crew.airborne() == 0 {
            if swept.held {
                // Nets held still took no round in this pass, and the hold
                // has lapsed or lapses at its net's next sweep, with nothing
                // in flight to wait for: pass again.
                continue;
            }
            // Every net had its turn, and none had a firing.
            return true;
        }
        if crew.airborne() == 0 {
            // Inline firings only, and they moved the marking: pass again
            // before calling the net quiescent.
            continue;
        }
        if !swept.inline {
            // Only bodies airborne can change anything now: wait for one.
            let (way, done) = crew.recv();
            land(net, way, done, on_failure, tally, &mut ending.first_failure);
        }
        // Whatever else has come back meanwhile, without waiting: after an
        // inline firing, which moved the marking, the next pass comes at
        // once.
        while let Some((way, done)) = crew.try_recv() {
            land(net, way, done, on_failure, tally, &mut ending.first_failure);
        }
    }
}

/// [`Mode::Sequential`] for [`run_indefinitely`]: one firing per tick, then
/// sleep out the rest of it.
fn tick_sequential<F>(
    net: &mut Net,
    clock: Clock<'_>,
    on_failure: OnFailure,
    mut each_tick: F,
    tally: &mut Tally,
) -> Result<RunSummary, DriverError>
where
    F: FnMut(&mut Net),
{
    while !clock.stopped() {
        each_tick(net);
        if clock.stopped() {
            break;
        }
        match net.step_deep() {
            Ok(Some(step)) => tally.landed(&step),
            // An idle tick is not a finish line here: the net is waiting.
            Ok(None) => {}
            Err(error) => step_failed(net, error, on_failure, tally)?,
        }
        thread::sleep(clock.tick);
    }
    let quiesced = net.is_quiescent_deep();
    Ok(tally.summary(net, quiesced))
}

/// [`Mode::Concurrent`] for [`run_indefinitely`]: one pass per tick, and the
/// wait on the completion channel *is* the tick's wait, a whole `tick` from
/// the end of the pass.
///
/// The ticks are [`tick_turns`], inside a [`thread::scope`] that holds the
/// [`Crew`], and the firings still airborne are landed however they end, a
/// panic of `each_tick` or of the net's own code included, as for
/// [`run_concurrent`].
fn tick_concurrent<F>(
    net: &mut Net,
    clock: Clock<'_>,
    on_failure: OnFailure,
    mut each_tick: F,
    tally: &mut Tally,
    workers: Option<NonZeroUsize>,
) -> Result<RunSummary, DriverError>
where
    F: FnMut(&mut Net),
{
    let ((), ending) = with_crew(
        net,
        on_failure,
        tally,
        workers,
        |net, crew, tally, ending| {
            tick_turns(net, crew, clock, on_failure, &mut each_tick, tally, ending);
        },
    );

    if let Some(error) = ending.first_failure {
        return Err(stopped_by(error, tally.summary(net, false)));
    }
    let quiesced = net.is_quiescent_deep();
    Ok(tally.summary(net, quiesced))
}

/// The ticks of a concurrent [`run_indefinitely`], until `stop` is set or a
/// failure under [`OnFailure::Stop`], or a choice that cannot be made, ends
/// them, which `ending` records. What is still airborne is the caller's to
/// land.
fn tick_turns<F>(
    net: &mut Net,
    crew: &mut Crew<'_, '_>,
    clock: Clock<'_>,
    on_failure: OnFailure,
    each_tick: &mut F,
    tally: &mut Tally,
    ending: &mut Ending,
) where
    F: FnMut(&mut Net),
{
    while !clock.stopped() && ending.first_failure.is_none() {
        each_tick(net);
        if clock.stopped() {
            break;
        }
        // One pass a tick, over the net and every net inside it.
        let mut pass = Pass {
            crew: &mut *crew,
            limits: Limits::default(),
            clock: Some(clock),
            on_failure,
            tally: &mut *tally,
            ending: &mut *ending,
        };
        pass.round(net, &mut Way::new());
        if ending.first_failure.is_some() {
            break;
        }
        crew.holds.prune(net);

        // The tick's wait, a whole `tick` from here, spent on the channel
        // rather than asleep: a completion is distributed the moment it
        // lands. A tick too long to end on the clock has no deadline, and the
        // wait is for the next completion, as long as it takes.
        let deadline = Instant::now().checked_add(clock.tick);
        loop {
            let left = deadline.map_or(Duration::MAX, |deadline| {
                deadline.saturating_duration_since(Instant::now())
            });
            let Some((way, done)) = crew.recv_timeout(left) else {
                break;
            };
            land(net, way, done, on_failure, tally, &mut ending.first_failure);
            if ending.first_failure.is_some() || clock.stopped() {
                break;
            }
        }
    }
}

// ----- one pass ------------------------------------------------------------

/// One pass of a concurrent loop over the net and every net inside it, at
/// any depth (since 0.4.0; until then a pass was the outer net's launch
/// sweep, and an inner net fired one transition at a time, inline, between
/// sweeps).
///
/// Each net takes its round as its places' [`Stepping`](crate::Stepping)
/// rules order it ([`Pass::round`]), and its own turn in it is one launch
/// sweep of its own transitions ([`Pass::own`]): every launch its own
/// choice, every detachable body handed to the one crew, every other body
/// run on the spot and landed there. Each detachable firing goes with the
/// way down to the net that launched it, by which it lands
/// ([`land`]). The limits, the clock's stop flag, a failure under
/// [`OnFailure::Stop`] and a choice that cannot be made end the whole
/// pass, at whatever depth they are met, which `ending` records.
struct Pass<'p, 'scope, 'env> {
    crew: &'p mut Crew<'scope, 'env>,
    limits: Limits,
    clock: Option<Clock<'p>>,
    on_failure: OnFailure,
    tally: &'p mut Tally,
    ending: &'p mut Ending,
}

/// What a net's part of a pass did, the nets inside it included.
#[derive(Debug, Default, Clone, Copy)]
struct Swept {
    /// Launches, at every level, the inline ones included.
    launched: u64,
    /// Whether one of them was not detachable, and so landed on the spot.
    inline: bool,
    /// Whether the nets of a place were held still ([`Holds`]) and took no
    /// round: once the hold lapses they may have something to launch.
    held: bool,
}

impl Swept {
    fn add(&mut self, other: Swept) {
        self.launched += other.launched;
        self.inline |= other.inline;
        self.held |= other.held;
    }
}

impl Pass<'_, '_, '_> {
    /// One net's round in this pass, `way` the way down to it: the three
    /// bands of a deep step ([`Stepping`](crate::Stepping)), where each net
    /// a band lets go launches what its own rules let go, all of them
    /// together, and a band goes only once every net of the bands before it
    /// is at rest, having launched nothing and having nothing the driver
    /// launched still in flight.
    ///
    /// 1. Every net in an inner-first place.
    /// 2. The ring, from where its cursor says: this net's own sweep and
    ///    every net in a take-turns place, each in turn and every one in
    ///    every pass, so that none waits on another. The cursor moves to
    ///    each member that launched anything, this net included.
    /// 3. Every net in an outer-first place, once this net has nothing of
    ///    its own in flight either.
    ///
    /// The nets of a place held still for a take-all or a drain that waits
    /// for it ([`Holds`]) take no round, in whichever band.
    fn round(&mut self, net: &mut Net, way: &mut Way) -> Swept {
        let mut swept = Swept::default();
        let round = net.round();
        let mut at_rest = true;
        for member in &round.inner_first {
            if !self.ending.is_clear() {
                return swept;
            }
            at_rest &= self.member(net, member, way, &mut swept);
        }
        if !at_rest {
            return swept;
        }
        for turn in &round.ring {
            if !self.ending.is_clear() {
                return swept;
            }
            match turn {
                Turn::Own => {
                    let own = self.own(net, way);
                    swept.add(own);
                    if own.launched > 0 {
                        net.took_own_turn();
                    }
                    at_rest &= own.launched == 0 && !net.has_in_flight();
                }
                Turn::Member(member) => {
                    let before = swept.launched;
                    at_rest &= self.member(net, member, way, &mut swept);
                    if swept.launched > before {
                        net.ring_cursor = Some(member.id);
                    }
                }
            }
        }
        if !at_rest {
            return swept;
        }
        // Asked afresh: this net's own sweep may have taken a token that
        // carries one, or put one in.
        for member in &net.outer_first() {
            if !self.ending.is_clear() {
                break;
            }
            self.member(net, member, way, &mut swept);
        }
        swept
    }

    /// A member's round, inside it, its launches added to `swept`, unless
    /// it is a subnet held still by a firing of `net` in flight, its place
    /// is held still for a take-all or a drain that waits for it
    /// ([`Holds`]), or its token is no longer where it was. True when it is
    /// at rest: it launched nothing in this pass, and nothing the driver
    /// launched inside it is in flight.
    fn member(&mut self, net: &mut Net, member: &Member, way: &mut Way, swept: &mut Swept) -> bool {
        if self.crew.holds.holds(net.id, member.place) {
            // What is in flight in there lands; nothing new launches until
            // the transition that waits for it has had its turn.
            swept.held = true;
        } else if !net.held(member) {
            let inside = net.within_member(member, |inner, level| {
                way.push(level);
                let inside = self.round(inner, way);
                way.pop();
                inside
            });
            if let Some(inside) = inside {
                swept.add(inside);
                if inside.launched > 0 {
                    return false;
                }
            }
        }
        self.crew.inside(member.id) == 0
    }

    /// A net's own turn: one launch sweep of its transitions, which returns
    /// what it launched.
    ///
    /// Each launch is the net's own choice, made as [`Net::step`] makes it
    /// (since 2026-11-28; until then the sweep launched everything fireable
    /// in declaration order): the installed selector's, or the default
    /// rule's, highest priority first and ties by the tie-break, the round
    /// robin's cursor moving to each launch it chose. The choice is among
    /// the transitions fireable now that this sweep has not launched yet, in
    /// declaration order, their take and mutating edges passing over the
    /// busy tokens ([`Busy`]) and their take-all and drain edges waiting
    /// for them, and it is made afresh after each launch, the
    /// guards and priorities asked again, until nothing is offered, the
    /// selector declines, or the pacing limit, the fuse, the clock's stop
    /// flag, a failure under [`OnFailure::Stop`] or a choice that cannot be
    /// made ends the sweep, which `ending` then records. The fuse blames the
    /// transition that was chosen, before it launches, and the cursor stays
    /// where it was.
    ///
    /// A detachable firing goes to the crew with `way`; one that is not
    /// runs inside [`launch`](Net::launch), lands at once, and is not
    /// offered again in this sweep, as the ones in flight are not.
    ///
    /// Once the sweep is over the net sets what it holds still
    /// ([`Holds`]): the places of nets that each of its transitions waiting
    /// for busy tokens takes every token of. A transition that was offered
    /// in this sweep, chosen or not, waits for nothing now and holds
    /// nothing, and neither does one that is no longer fireable otherwise.
    fn own(&mut self, net: &mut Net, way: &Way) -> Swept {
        let mut swept = Swept::default();
        // The transitions that have had their turn in this sweep, by
        // declaration index: launched, or found no longer fireable when
        // launched.
        let mut had_turn = vec![false; net.transitions.len()];
        loop {
            if self.clock.is_some_and(Clock::stopped) {
                break;
            }
            if self.tally.capped(self.limits) {
                self.ending.capped = true;
                break;
            }
            let offered: Vec<TransitionId> = net
                .fireable_with(Some(Busy::new(&self.crew.inside)))
                .into_iter()
                .filter(|transition| !had_turn[transition.0 as usize])
                .collect();
            if offered.is_empty() {
                break;
            }
            let chosen = match net.select_among(&offered) {
                Ok(Some(chosen)) => chosen,
                // The selector declined: this sweep launches no more.
                Ok(None) => break,
                // Nothing fired, and the run stops whatever `on_failure`
                // says.
                Err(error) => {
                    self.ending
                        .fail(StepError::SelectionFailed(error.along(way)));
                    break;
                }
            };
            if let Some(limit) = self.limits.error_after_n_firings
                && self.tally.launched >= limit
            {
                self.ending.fuse = Some((limit, chosen, net.transition_name(chosen)));
                break;
            }
            had_turn[chosen.0 as usize] = true;
            let firing = match net.launch_for_driver(chosen, Some(Busy::new(&self.crew.inside))) {
                Ok(firing) => firing,
                // A guard that answered otherwise when it was asked again.
                Err(NetError::NotFireable { .. }) => continue,
                Err(misuse) => unreachable!("{}: {misuse}", LAUNCHABLE),
            };
            net.count_choice(chosen);
            self.tally.launched += 1;
            swept.launched += 1;
            if firing.is_pending() {
                self.crew.launch(way.clone(), firing);
            } else {
                // Not detachable: `launch` already ran the body.
                swept.inline = true;
                settle(
                    net,
                    way,
                    firing.run(),
                    self.on_failure,
                    self.tally,
                    &mut self.ending.first_failure,
                );
                if self.ending.first_failure.is_some() {
                    break;
                }
            }
        }
        if self.ending.is_clear() {
            let held = if self.crew.inside.is_empty() {
                Vec::new()
            } else {
                net.holding(Busy::new(&self.crew.inside))
            };
            self.crew.holds.set(net.id, held);
        }
        swept
    }
}

/// What has told a concurrent loop to stop launching, as far as it has got:
/// the pacing limit, the fuse, and the first failure under
/// [`OnFailure::Stop`] or the first choice that could not be made.
#[derive(Debug, Default)]
struct Ending {
    capped: bool,
    fuse: Option<(u64, TransitionId, String)>,
    first_failure: Option<StepError>,
}

impl Ending {
    /// Nothing has: the loop goes on launching.
    fn is_clear(&self) -> bool {
        !self.capped && self.fuse.is_none() && self.first_failure.is_none()
    }

    /// Keeps `error` if it is the first.
    fn fail(&mut self, error: StepError) {
        if self.first_failure.is_none() {
            self.first_failure = Some(error);
        }
    }
}

// ----- the crew ------------------------------------------------------------

/// A panic caught on the driver's thread, to go on once the firings
/// airborne have landed.
type Panic = Box<dyn Any + Send + 'static>;

/// The name each worker thread is given, which a panic message and a
/// debugger show.
const WORKER: &str = "petritype worker";

/// Why a completion is always received: the crew keeps a sender of the
/// channel for its whole life, so the channel is never hung up while it
/// lives.
const SENDER_ALIVE: &str = "the crew keeps a sender of its landing channel for its whole life";

/// A detachable firing handed to the workers, with the way down to the net
/// that launched it, empty for the net the driver runs: the way it comes
/// back with, to land by.
struct Job {
    way: Way,
    firing: Firing,
}

/// The worker threads of one concurrent call, and the channels between
/// them and the driver (since 2026-11-28; until then each launch had a
/// thread of its own).
///
/// Bounded and lazy. A worker is spawned only when a firing is handed over,
/// no worker is free to take it, and fewer than the cap are running: at
/// most [`available_parallelism`](thread::available_parallelism), asked
/// the first time a worker is wanted, or the `workers` of
/// [`Mode::ConcurrentWith`]. A firing handed over while every worker is
/// busy waits in the queue for the first one free, so with a cap of one the
/// bodies run one at a time, in the order they were launched. The cap
/// holds for every firing of the call, whichever net launched it (since
/// 0.4.0, when the nets inside began to launch beside the net itself).
///
/// The workers are threads of the call's [`thread::scope`], and each goes
/// home once the crew is dropped and the queue is empty, so every one is
/// joined before the driver returns, on an error or a panic too. A thread
/// that cannot be had is not an error: the firing waits for a worker there
/// is, and with none at all the body runs on the driver's own thread and
/// lands as if a worker had run it.
struct Crew<'scope, 'env> {
    scope: &'scope Scope<'scope, 'env>,
    /// The workers' end of the queue, which they take firings from in turn.
    queue: &'scope Mutex<Receiver<Job>>,
    /// Firings handed over and not yet run: queued, or running on a worker.
    /// A worker counts one off as its body returns, before it sends the
    /// completion, so `unfinished` above the number of workers is exactly
    /// a firing with no worker free to take it.
    unfinished: &'scope AtomicUsize,
    /// The driver's end of the queue.
    jobs: Sender<Job>,
    /// Where the completions land, each with its way; each worker has a
    /// clone.
    landing: Sender<(Way, Completed)>,
    landed: Receiver<(Way, Completed)>,
    /// The cap, `None` for the default until it is first asked.
    cap: Option<NonZeroUsize>,
    /// Workers spawned so far.
    spawned: usize,
    /// Firings handed over whose completions the driver has not taken yet.
    airborne: usize,
    /// For each net inside the one the driver runs, by id, how many of the
    /// firings airborne were launched in it or in a net inside it: what
    /// makes the token that carries it busy ([`Busy`]), and what keeps it
    /// from being at rest in a pass.
    inside: HashMap<NetId, usize>,
    /// The places held still for the take-alls and drains that wait for
    /// their busy tokens, kept from pass to pass.
    holds: Holds,
}

impl<'scope, 'env> Crew<'scope, 'env> {
    fn new(
        scope: &'scope Scope<'scope, 'env>,
        queue: &'scope Mutex<Receiver<Job>>,
        unfinished: &'scope AtomicUsize,
        jobs: Sender<Job>,
        cap: Option<NonZeroUsize>,
    ) -> Crew<'scope, 'env> {
        let (landing, landed) = mpsc::channel();
        Crew {
            scope,
            queue,
            unfinished,
            jobs,
            landing,
            landed,
            cap,
            spawned: 0,
            airborne: 0,
            inside: HashMap::new(),
            holds: Holds::default(),
        }
    }

    /// Hands a detachable firing over to be run, with the way down to the
    /// net that launched it, spawning a worker for it when none is free and
    /// the cap allows one.
    fn launch(&mut self, way: Way, firing: Firing) {
        for level in &way {
            *self.inside.entry(level.id).or_default() += 1;
        }
        let unfinished = self.unfinished.fetch_add(1, Ordering::AcqRel) + 1;
        if unfinished > self.spawned && self.spawned < self.cap().get() && self.hire() {
            self.spawned += 1;
        }
        let job = Job { way, firing };
        let unsent = if self.spawned == 0 {
            Some(job)
        } else {
            // The queue's receiver lives outside the scope, so this fails
            // only if it is gone, and then the firing comes back.
            self.jobs.send(job).err().map(|SendError(job)| job)
        };
        if let Some(Job { way, firing }) = unsent {
            // No worker to be had: the body runs here.
            self.unfinished.fetch_sub(1, Ordering::AcqRel);
            self.landing.send((way, firing.run())).expect(SENDER_ALIVE);
        }
        self.airborne += 1;
    }

    /// The cap on the workers: as given, or the machine's parallelism.
    fn cap(&mut self) -> NonZeroUsize {
        *self
            .cap
            .get_or_insert_with(|| thread::available_parallelism().unwrap_or(NonZeroUsize::MIN))
    }

    /// Spawns one more worker, and says whether there is one.
    fn hire(&self) -> bool {
        let (queue, unfinished, landing) = (self.queue, self.unfinished, self.landing.clone());
        thread::Builder::new()
            .name(String::from(WORKER))
            .spawn_scoped(self.scope, move || work(queue, unfinished, &landing))
            .is_ok()
    }

    /// Firings handed over whose completions have not been taken.
    fn airborne(&self) -> usize {
        self.airborne
    }

    /// How many of the firings airborne were launched in the net `id`, or
    /// in a net inside it.
    fn inside(&self, id: NetId) -> usize {
        self.inside.get(&id).copied().unwrap_or(0)
    }

    /// A completion taken: one firing fewer airborne, and fewer inside each
    /// net on its way.
    fn taken(&mut self, way: &Way) {
        self.airborne -= 1;
        for level in way {
            if let Entry::Occupied(mut count) = self.inside.entry(level.id) {
                *count.get_mut() -= 1;
                if *count.get() == 0 {
                    count.remove();
                }
            }
        }
    }

    /// The next completion, waiting as long as it takes. Called only with
    /// something airborne.
    fn recv(&mut self) -> (Way, Completed) {
        let landed = self.landed.recv().expect(SENDER_ALIVE);
        self.taken(&landed.0);
        landed
    }

    /// A completion that has already landed, if there is one.
    fn try_recv(&mut self) -> Option<(Way, Completed)> {
        let landed = self.landed.try_recv().ok()?;
        self.taken(&landed.0);
        Some(landed)
    }

    /// The next completion within `wait`, or `None` once it has passed.
    fn recv_timeout(&mut self, wait: Duration) -> Option<(Way, Completed)> {
        match self.landed.recv_timeout(wait) {
            Ok(landed) => {
                self.taken(&landed.0);
                Some(landed)
            }
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => unreachable!("{SENDER_ALIVE}"),
        }
    }

    /// Waits for every firing still airborne and lands each as it comes
    /// back, wherever it was launched: the drain a concurrent call ends
    /// with, however its loop ended. A panic of the net's own code while
    /// one lands (a key extractor of an output place) is kept, and the
    /// landing goes on with the rest; the first one kept is returned, to go
    /// on after them.
    fn land_all(
        &mut self,
        net: &mut Net,
        on_failure: OnFailure,
        tally: &mut Tally,
        first_failure: &mut Option<StepError>,
    ) -> Option<Panic> {
        let mut panicked = None;
        while self.airborne > 0 {
            let (way, done) = self.recv();
            let landing = catch_unwind(AssertUnwindSafe(|| {
                land(net, way, done, on_failure, tally, first_failure);
            }));
            if let Err(panic) = landing {
                panicked.get_or_insert(panic);
            }
        }
        panicked
    }
}

/// A worker's life: take a firing from the queue, run it, send the
/// completion back with its way, until the driver hangs up its end of the
/// queue and the queue is empty.
///
/// A worker holds the queue's lock only while it waits for a firing, never
/// while a body runs. A body's panic is caught by [`Firing::run`], so no
/// worker dies of one.
fn work(
    queue: &Mutex<Receiver<Job>>,
    unfinished: &AtomicUsize,
    landing: &Sender<(Way, Completed)>,
) {
    loop {
        let next = queue.lock().unwrap_or_else(PoisonError::into_inner).recv();
        let Ok(Job { way, firing }) = next else {
            return;
        };
        let done = firing.run();
        unfinished.fetch_sub(1, Ordering::AcqRel);
        // The driver takes every completion it is owed before the crew,
        // and its receiver, is dropped, so this send does not fail.
        if landing.send((way, done)).is_err() {
            return;
        }
    }
}

/// The frame of a concurrent call, written once for [`run_concurrent`] and
/// [`tick_concurrent`]: a [`Crew`] inside a [`thread::scope`], the loop
/// `turns` run with it, and then every firing still airborne, in the net
/// and in the nets inside it, landed however the loop ended. A panic of the
/// loop, or of the net's own code while a firing lands, goes on once they
/// have all landed. What the loop returned comes back with what told it to
/// stop launching.
fn with_crew<T>(
    net: &mut Net,
    on_failure: OnFailure,
    tally: &mut Tally,
    workers: Option<NonZeroUsize>,
    turns: impl FnOnce(&mut Net, &mut Crew<'_, '_>, &mut Tally, &mut Ending) -> T,
) -> (T, Ending) {
    let (jobs, queue) = mpsc::channel::<Job>();
    let queue = Mutex::new(queue);
    let unfinished = AtomicUsize::new(0);
    thread::scope(|scope| {
        let mut crew = Crew::new(scope, &queue, &unfinished, jobs, workers);
        let mut ending = Ending::default();
        let ended = catch_unwind(AssertUnwindSafe(|| {
            turns(net, &mut crew, tally, &mut ending)
        }));
        // Nothing goes home in flight, whichever way the loop ended.
        let landed = crew.land_all(net, on_failure, tally, &mut ending.first_failure);
        match (ended, landed) {
            (Ok(turned), None) => (turned, ending),
            // The crew goes with the unwind, and so the workers go home.
            (Err(panic), _) | (Ok(_), Some(panic)) => resume_unwind(panic),
        }
    })
}

// ----- the pieces the loops share ------------------------------------------

/// The check before a run: [`DriverError::HeldByBody`] for
/// [`OnFailure::Disable`] on a net a body holds, the refusal recorded on the
/// body's firing as any refused knob is. The loops reach the net's stages
/// by the paths a step takes, which a body may take on a net it holds.
fn admit(net: &Net, on_failure: OnFailure) -> Result<(), DriverError> {
    if on_failure != OnFailure::Disable {
        return Ok(());
    }
    net.admit_knob(DISABLE_ON_FAILURE)
        .map_err(|_| DriverError::HeldByBody {
            net: net.display_name(),
        })
}

/// Why a `launch` of a transition the driver just chose out of
/// [`fireable`](Net::fireable) can only ever fail as `NotFireable`.
const LAUNCHABLE: &str =
    "a driver launches only what this net's own `fireable` just named, so it exists";

/// Stage three for a firing of `net`, the net `way` leads down to from the
/// one the driver runs, with the failure policy applied: the one place a
/// concurrent driver hands a [`Completed`] back to a net.
///
/// A firing that ran on a worker and one that [`launch`](Net::launch) ran
/// on the spot, in the net itself or in a net inside it, are treated
/// exactly alike. A failure says where by `way`, in its path and its nested
/// ids. Only [`OnFailure::Stop`] keeps the error, and only the first one:
/// the later failures of a drain are counted and logged like any other.
/// [`OnFailure::Disable`] switches the transition off in `net`, where it
/// is, and its [`Toggled`](crate::Event::Toggled) is lifted with whatever
/// else `net` logged.
fn settle(
    net: &mut Net,
    way: &[Level],
    completed: Completed,
    on_failure: OnFailure,
    tally: &mut Tally,
    first_failure: &mut Option<StepError>,
) {
    match net
        .distribute_for_driver(completed)
        .map_err(DistributeError::into_parts)
    {
        // A firing inside is not in the outer net's `fired_count`.
        Ok(()) if !way.is_empty() => tally.inner_fired += 1,
        Ok(()) => {}
        Err((NetError::FiringFailed(error), _)) => {
            tally.failed += 1;
            match on_failure {
                OnFailure::Stop => {
                    if first_failure.is_none() {
                        *first_failure = Some(StepError::FiringFailed(error.along(way)));
                    }
                }
                OnFailure::Disable => net
                    .disable(error.transition)
                    .expect("the transition that just failed is a transition of this net"),
                OnFailure::Continue => {}
            }
        }
        // A driver distributes only what it launched into this very net, so
        // the stamp matches and the transition is in flight.
        Err((misuse, _)) => unreachable!(
            "a driver distributes only the firings it launched into this net: {misuse}"
        ),
    }
}

/// A completion the crew handed back, landed in the net that launched it:
/// the net the driver runs, or the net `way` leads down to inside it, each
/// level found by its place, its token's id and its net's id (since 0.4.0).
/// What each net on the way logged is lifted into the net around it on the
/// way back up, as a deep step lifts it, and each token on the way indexed
/// again.
///
/// The way holds while the firing is in flight: no edge takes or changes a
/// token whose nets have one ([`Busy`]). Only `each_tick` of
/// [`run_indefinitely`] could move such a token, and then the net is looked
/// for by its id wherever it is in the tree and the firing lands there. A
/// net that is in no place of the tree any more has gone with its token,
/// and so does the firing.
fn land(
    net: &mut Net,
    way: Way,
    completed: Completed,
    on_failure: OnFailure,
    tally: &mut Tally,
    first_failure: &mut Option<StepError>,
) {
    let Some(target) = way.last().map(|level| level.id) else {
        settle(net, &way, completed, on_failure, tally, first_failure);
        return;
    };
    let here = |inner: &mut Net, way: &[Level]| {
        settle(inner, way, completed, on_failure, tally, first_failure);
    };
    let Err(here) = net.within_way(&way, here) else {
        return;
    };
    if let Some(found) = net.find_way(target) {
        // The net, found where it is now; it lands by that way, which
        // leads to it, so this is never refused.
        let _ = net.within_way(&found, here);
    }
}

/// The error a concurrent loop returns for what stopped it: a failed
/// firing under [`OnFailure::Stop`], or a choice that could not be made.
fn stopped_by(error: StepError, summary: RunSummary) -> DriverError {
    match error {
        StepError::FiringFailed(error) => DriverError::FiringFailed { summary, error },
        StepError::SelectionFailed(error) => DriverError::SelectionFailed { summary, error },
        refused @ StepError::HeldByBody { .. } => unreachable!("{NOT_REFUSED}: {refused}"),
    }
}

/// Why a driver never meets [`StepError::HeldByBody`]: it steps with
/// [`Net::step_deep`] and the turns of a deep step, which a body may ask of
/// a net it holds, never with the staged steps or
/// [`step_inside`](Net::step_inside), which refuse one.
const NOT_REFUSED: &str = "a driver steps a net deep, which no body is refused";

/// A [`step_deep`](Net::step_deep) of a sequential loop that failed, with
/// the failure policy applied, written once for [`run_sequential`] and
/// [`tick_sequential`]. A failed firing counts as a launch and a failure,
/// and ends the run under [`OnFailure::Stop`]; a choice that cannot be made
/// ends it whatever `on_failure` says. `Ok` when the loop goes on.
fn step_failed(
    net: &mut Net,
    error: StepError,
    on_failure: OnFailure,
    tally: &mut Tally,
) -> Result<(), DriverError> {
    match error {
        StepError::SelectionFailed(error) => Err(DriverError::SelectionFailed {
            summary: tally.summary(net, false),
            error,
        }),
        StepError::FiringFailed(error) => {
            tally.launched += 1;
            tally.failed += 1;
            match on_failure {
                OnFailure::Stop => Err(DriverError::FiringFailed {
                    summary: tally.summary(net, false),
                    error,
                }),
                OnFailure::Disable => {
                    disable(net, &error);
                    Ok(())
                }
                OnFailure::Continue => Ok(()),
            }
        }
        refused @ StepError::HeldByBody { .. } => unreachable!("{NOT_REFUSED}: {refused}"),
    }
}

/// [`OnFailure::Disable`] for the sequential loops, written once: switch
/// off the transition that just failed, which appends a
/// [`Toggled`](crate::Event::Toggled) event.
///
/// A transition of an inner net is switched off where it is, found by the
/// failure's [`path`](FiringFailed::path), each token's net by its
/// [nested id](FiringFailed::nested_ids) (since 0.4.0; until then by its
/// name, so that of two nets of one name in one token the first was
/// switched off whichever had failed); its `Toggled` comes out here as an
/// [`Event::Nested`](crate::Event::Nested) or an
/// [`Event::Subnet`](crate::Event::Subnet), like anything else an inner net
/// logs. The concurrent loops switch it off as it lands ([`settle`]).
fn disable(net: &mut Net, failure: &FiringFailed) {
    disable_at(net, &failure.path, &failure.nested_ids, failure.transition);
}

/// [`disable`] one level at a time, outer to inner.
fn disable_at(net: &mut Net, path: &[PathStep], ids: &[NetId], transition: TransitionId) {
    let Some((step, rest)) = path.split_first() else {
        net.disable(transition)
            .expect("the transition that just failed is a transition of this net");
        return;
    };
    let (id, ids) = match ids.split_first() {
        Some((id, ids)) => (Some(*id), ids),
        None => (None, ids),
    };
    match step {
        // The token has not moved: an inner failure changes nothing out
        // here.
        PathStep::Token {
            place,
            token,
            net: name,
        } => {
            net.nested_scope(*place, *token, |nets| {
                let found = match id {
                    Some(id) => nets.iter_mut().find(|entry| entry.1.id() == id),
                    None => nets.iter_mut().find(|entry| entry.0 == name.as_str()),
                };
                if let Some(entry) = found {
                    disable_at(&mut *entry.1, rest, ids, transition);
                }
            });
        }
        PathStep::Subnet { place } => {
            net.subnet_scope(*place, |subnet| disable_at(subnet, rest, ids, transition))
                .expect("a subnet that has just taken its turn is not held still");
        }
    }
}

/// The per-call bookkeeping every loop keeps: where [`Net::fired_count`]
/// started, how many launches this call has attempted, and how many of them
/// failed.
///
/// Successes are read back off the net rather than counted here, so the
/// summary cannot drift from what the net thinks happened; failures are the
/// driver's own tally, because the net counts only completed firings.
///
/// An inner net's successes are not in the outer net's
/// [`fired_count`](Net::fired_count), so they are counted here as they land.
#[derive(Debug)]
struct Tally {
    fired_at_entry: u64,
    launched: u64,
    failed: u64,
    inner_fired: u64,
}

impl Tally {
    fn new(net: &Net) -> Tally {
        Tally {
            fired_at_entry: net.fired_count(),
            launched: 0,
            failed: 0,
            inner_fired: 0,
        }
    }

    /// One successful [`step_deep`](Net::step_deep) or inner step: a launch,
    /// and an inner success if it happened inside, in a token's net or in a
    /// subnet.
    fn landed(&mut self, step: &DeepStep) {
        self.launched += 1;
        if !matches!(step, DeepStep::Here { .. }) {
            self.inner_fired += 1;
        }
    }

    /// Whether [`stop_after_n_firings`](Limits::stop_after_n_firings) has
    /// been reached, which is checked before a launch and never after one.
    fn capped(&self, limits: Limits) -> bool {
        limits
            .stop_after_n_firings
            .is_some_and(|limit| self.launched >= limit)
    }

    /// What the run did. The net's own count is read as a difference from
    /// where it stood at the start, which saturates at zero: `each_tick`
    /// holds `&mut Net`, and a net it put in place of the one it was handed
    /// may have fired less.
    fn summary(&self, net: &Net, quiesced: bool) -> RunSummary {
        RunSummary {
            quiesced,
            fired_count: net
                .fired_count()
                .saturating_sub(self.fired_at_entry)
                .saturating_add(self.inner_fired),
            failed_count: self.failed,
        }
    }
}
