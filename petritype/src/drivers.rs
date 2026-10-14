//! Loops that drive a net: run it to quiescence, or run it on a clock.
//!
//! A driver is *thin* (DESIGN §13). It owns no state of its own and adds no
//! semantics: it is a loop over the stages [`Net`] already exposes —
//! [`step_deep`](Net::step_deep) for the sequential one,
//! [`launch`](Net::launch) / [`Firing::run`](crate::Firing::run) /
//! [`distribute`](Net::distribute) for the concurrent one — plus a decision
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
//! complete before the next begins — and, when the net has nothing to fire
//! itself, one firing in the first inner net that has, if its tokens carry
//! nets of their own ([`Nested`](crate::Nested)). It is **deterministic given
//! the selectors**, which is what makes a sequential run the thing to write a
//! test or a replay against — and what makes [`Net::set_selector`] a way to
//! shape a whole run rather than one step.
//!
//! [`Mode::Concurrent`] sweeps [`Net::fireable`] in declaration order and
//! [`launch`](Net::launch)es **every** transition it finds, each detachable
//! body on a thread of its own inside a [`std::thread::scope`], then
//! distributes the completions as they arrive over an
//! [`mpsc`] channel. Consuming the inputs synchronously in
//! the sweep is what resolves conflicts: a peer that wanted one of those
//! tokens is simply not fireable any more, and no lock is involved (DESIGN
//! §6.4). Two consequences are worth saying out loud:
//!
//! - **The selector is ignored.** There is no choice to make when everything
//!   fireable is launched. [`priority`](crate::TransitionBuilder::priority)
//!   and a custom selector shape a sequential run only.
//! - **Order is wall-clock order.** Completions are distributed as they come
//!   back, so the event log and the final token ids depend on how long the
//!   bodies took. One flight per transition at a time, though: a transition
//!   in flight is not fireable, so it is never launched twice over.
//!
//! A transition that is not detachable — one with a
//! [`MutateEdge`](crate::MutateEdge) input, whose body borrows a token where
//! it lies — has already run by the time [`launch`](Net::launch) returns, so
//! the concurrent driver distributes it on the spot and sweeps again rather
//! than calling the net quiescent: the marking moved.
//!
//! Inner nets do not get threads of their own. When a sweep launches nothing
//! and nothing is airborne, the concurrent driver makes **one** inner firing
//! inline, exactly as [`Net::step_deep`] would choose it, and sweeps again,
//! since a guard out here may read the marking in there. The ordering
//! guarantee of the inner nets is therefore the sequential one, but only the
//! sequential driver promises an order for the whole run.
//!
//! Both drivers honour the net's [`DeepPolicy`]. Under
//! [`Interleaved`](DeepPolicy::Interleaved) the concurrent driver takes the
//! cycle as `step_deep` does, a launch sweep being the outer net's turn: one
//! inline inner firing for each inner net's turn, in the order of the round,
//! made whether or not outer bodies are airborne, so an outer net that can
//! always launch does not starve its inner nets here either.
//!
//! # Limits: pacing and the fuse
//!
//! [`Limits`] holds two caps, and both count **launches** — attempts,
//! including ones that go on to fail, inside inner nets as well as in the net
//! itself — rather than successes.
//!
//! - [`stop_after_n_firings`](Limits::stop_after_n_firings) is *pacing*: the
//!   driver stops launching and returns `Ok`, with
//!   [`quiesced`](RunSummary::quiesced) false to say that work may remain.
//!   Call again to continue; nothing resumes by itself, because everything
//!   that survives the call is in the net.
//! - [`error_after_n_firings`](Limits::error_after_n_firings) is a *fuse*:
//!   the driver returns [`DriverError::TooManyFirings`] **before** the
//!   offending launch consumes anything. It is the guard against a net that
//!   will not stop — most sharply against a transition that
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
//! may not write to (DESIGN §7). The net is consistent either way by the time
//! the driver hears about it, so the only open question is whether the loop
//! goes on, and [`OnFailure`] answers it: [`Stop`](OnFailure::Stop) ends the
//! run and reports the *first* failure, [`Disable`](OnFailure::Disable)
//! switches the offending transition off and keeps going, and
//! [`Continue`](OnFailure::Continue) keeps going as if nothing had happened.
//!
//! The driver emits no events of its own. Everything an observer sees comes
//! from the net: one [`Failed`](crate::Event::Failed) per failed firing, and
//! a [`Toggled`](crate::Event::Toggled) for each transition
//! [`Disable`](OnFailure::Disable) switches off.
//!
//! # Nothing is left in flight
//!
//! A driver never returns while a firing is airborne, not even on the way out
//! of an error. A [`Firing`](crate::Firing) or
//! [`Completed`] that is dropped instead of distributed
//! takes its transition's body with it and **strands that transition in
//! flight for good** — see the `#[must_use]` notes on both types. So the
//! concurrent driver, once it has decided to stop, stops *launching* and then
//! waits for every completion it is still owed, distributing each one
//! (failures among them are counted in
//! [`failed_count`](RunSummary::failed_count) and logged, but the error that
//! is returned stays the first one). `net.in_flight()` is empty whenever one
//! of these functions returns.

use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{FiringFailed, NetError};
use crate::firing::Completed;
use crate::ids::{PlaceId, TokenId, TransitionId};
use crate::nested::{DeepPolicy, DeepStep};
use crate::net::Net;

/// Why a blocking `recv` cannot fail here: the driver holds a sender of its
/// own for the whole of the scope, so the channel is never hung up.
const SENDER_ALIVE: &str = "the driver's own sender is alive for the whole scope";

/// How a driver fires: one at a time through the selector, or every fireable
/// transition launched at once.
///
/// The default is [`Sequential`](Mode::Sequential), which is the
/// reproducible one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// [`Net::step_deep`] in a loop: the net's [`Selector`](crate::Selector)
    /// picks one of the fireable transitions, it fires to completion, and the
    /// loop asks again; when the net itself has nothing to fire, the first
    /// inner net that has fires once. Deterministic given the selectors.
    #[default]
    Sequential,
    /// Launch everything fireable, in declaration order, and run each
    /// detachable body on its own thread inside a
    /// [`std::thread::scope`]. The selector is not consulted and completion
    /// order is wall-clock order. Inner nets are stepped inline, one firing
    /// at a time, whenever a sweep finds nothing to launch — or, under
    /// [`DeepPolicy::Interleaved`], in their turns between the sweeps.
    Concurrent,
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
    /// for and distributed first — see
    /// [the module docs](self#nothing-is-left-in-flight) — so later failures
    /// may be in the event log and in
    /// [`failed_count`](RunSummary::failed_count) even though the error
    /// reports the first.
    #[default]
    Stop,
    /// Switch the failing transition off with [`Net::disable`] — which
    /// appends a [`Toggled`](crate::Event::Toggled) event — and keep going.
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
    /// True: there was nothing left to fire when the call returned — the net
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

/// A run that ended badly: the fuse blew, or a firing failed under
/// [`OnFailure::Stop`].
///
/// Both variants carry the [`RunSummary`] of the interrupted call, because
/// what the run managed to do before it stopped is exactly as interesting as
/// why it stopped. Neither leaves anything in flight.
///
/// # Names and ids
///
/// A driver error escapes the scope the net lives in even more readily than
/// the rest: `drivers::run(&mut net, ..)?` in `main` prints the message and
/// exits the process, and by then `TransitionId(2)` says nothing to the
/// person reading the log. So the transition is named as well as numbered —
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
}

/// Drives a net until it has nothing left to fire, or until a limit stops it.
///
/// The workhorse: it subsumes Python petritype's `execute_graph`,
/// `Runner.run` and `run_to_completion`, which differed only in their limits
/// and their return type (DESIGN §13). `mode` picks the loop, `limits` says
/// when to give up, and `on_failure` says what a failed firing means for the
/// rest of the run — see the [module docs](self) for all three.
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
/// // thread of its own. Which token lands first is wall-clock order.
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
/// wants to fire, and [`DriverError::FiringFailed`] when a firing fails under
/// [`OnFailure::Stop`]. Both carry a [`RunSummary`] of what the call managed
/// first, and both leave the net consistent with nothing in flight.
///
/// # Panics
///
/// Not for a panicking body: [`Firing::run`](crate::Firing::run) catches it
/// and it arrives as an ordinary failure, so a worker thread never dies of
/// one.
pub fn run(
    net: &mut Net,
    mode: Mode,
    limits: Limits,
    on_failure: OnFailure,
) -> Result<RunSummary, DriverError> {
    let mut tally = Tally::new(net);
    match mode {
        Mode::Sequential => run_sequential(net, limits, on_failure, &mut tally),
        Mode::Concurrent => run_concurrent(net, limits, on_failure, &mut tally),
    }
}

/// Drives a net on a clock until `stop` is set: the real-time regime.
///
/// The difference from [`run`] is the whole point: this loop **never returns
/// because nothing is fireable**. An idle tick means the net is waiting —
/// for input from outside, or for a time-based guard — not that it is
/// finished (DESIGN §13). It ends when `stop` is set, which is checked at the
/// top of every tick and again before every launch, so a stop is acted on
/// promptly rather than at the end of the sweep.
///
/// A tick is: call `each_tick`, fire, wait out the rest of `tick`. In
/// [`Mode::Sequential`] "fire" is one [`step`](Net::step) — one firing per
/// tick, which is what an animation wants, one firing per frame — followed by
/// a [`sleep`](std::thread::sleep). In [`Mode::Concurrent`] it is one launch
/// sweep, and the wait is a
/// [`recv_timeout`](std::sync::mpsc::Receiver::recv_timeout) on the
/// completion channel until the tick's deadline, so completions are
/// distributed the moment they arrive instead of at the next tick.
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
/// in it, apply commands, poll a UI, top up a source place — whatever the
/// caller's side of the loop is. DESIGN §13 sketched an
/// `inbox: Option<Receiver<Command>>` parameter instead; a closure is
/// strictly more general and keeps the driver channel-agnostic, which is what
/// DESIGN §9 asks for: "channels between a UI thread and the driver are the
/// caller's concern". The net is the single writer either way, since
/// `each_tick` is called by this loop and holds the same `&mut Net`.
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
///             stop.store(true, Ordering::Relaxed);
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
/// [`OnFailure::Stop`]. The fuse has no place here, so
/// [`DriverError::TooManyFirings`] is never returned.
///
/// # Panics
///
/// Not for a panicking body, as for [`run`].
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
    let mut tally = Tally::new(net);
    match mode {
        Mode::Sequential => tick_sequential(net, tick, stop, on_failure, each_tick, &mut tally),
        Mode::Concurrent => tick_concurrent(net, tick, stop, on_failure, each_tick, &mut tally),
    }
}

// ----- the loops -----------------------------------------------------------

/// [`Mode::Sequential`] for [`run`]: stop check, fuse check, one
/// [`step`](Net::step), repeat.
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
            Err(error) => {
                tally.launched += 1;
                tally.failed += 1;
                match on_failure {
                    OnFailure::Stop => {
                        return Err(DriverError::FiringFailed {
                            summary: tally.summary(net, false),
                            error,
                        });
                    }
                    OnFailure::Disable => disable(net, &error),
                    OnFailure::Continue => {}
                }
            }
        }
    }
}

/// [`Mode::Concurrent`] for [`run`]: sweep, wait, distribute, repeat.
///
/// The loop ends in one of three ways, and the first two of them drain before
/// they return, because a completion that is never distributed strands its
/// transition for good:
///
/// - a sweep launched nothing and nothing was airborne — quiescence;
/// - a limit or a failure said to stop launching — drain, then report;
/// - the fuse blew before a launch — drain, then report.
fn run_concurrent(
    net: &mut Net,
    limits: Limits,
    on_failure: OnFailure,
    tally: &mut Tally,
) -> Result<RunSummary, DriverError> {
    thread::scope(|scope| {
        // The driver keeps this sender for the whole scope, so a worker that
        // is still to be spawned never finds the channel hung up, and a
        // blocking `recv` below is only ever reached with something airborne
        // to wait for.
        let (tx, rx) = mpsc::channel::<Completed>();
        let mut airborne: usize = 0;
        let mut capped = false;
        let mut fuse: Option<(u64, TransitionId, String)> = None;
        let mut first_failure: Option<FiringFailed> = None;

        loop {
            let interleaved = net.deep_policy() == DeepPolicy::Interleaved;
            let (ahead, behind) = net.round();
            let mut launched_this_sweep = 0_usize;
            // An inner net took its turn this time round, inline.
            let mut inner_turn = false;
            if tally.capped(limits) {
                capped = true;
            }
            // Under `Interleaved`, the inner nets whose turn comes before the
            // outer net's in this round.
            if !capped && fuse.is_none() && first_failure.is_none() {
                inner_turn = inner_step(
                    net,
                    ahead,
                    limits,
                    on_failure,
                    tally,
                    &mut fuse,
                    &mut first_failure,
                );
            }
            // The outer net's turn: one launch sweep.
            if !inner_turn && !capped && fuse.is_none() && first_failure.is_none() {
                for transition in net.fireable() {
                    if tally.capped(limits) {
                        capped = true;
                        break;
                    }
                    if let Some(limit) = limits.error_after_n_firings
                        && tally.launched >= limit
                    {
                        fuse = Some((limit, transition, net.transition_name(transition)));
                        break;
                    }
                    let firing = match net.launch(transition) {
                        Ok(firing) => firing,
                        // An inline firing earlier in this same sweep may
                        // have emptied a place this one was counting on; the
                        // list was taken before that happened.
                        Err(NetError::NotFireable { .. }) => continue,
                        Err(misuse) => unreachable!("{}: {misuse}", LAUNCHABLE),
                    };
                    tally.launched += 1;
                    launched_this_sweep += 1;
                    if firing.is_pending() {
                        let tx = tx.clone();
                        // `Firing::run` catches a panicking body, so this
                        // worker never dies on the body's account, and the
                        // send only fails if the receiver is gone, which it
                        // is not until the scope ends.
                        scope.spawn(move || {
                            let _ = tx.send(firing.run());
                        });
                        airborne += 1;
                    } else {
                        // Not detachable: `launch` already ran the body.
                        absorb(net, firing.run(), on_failure, tally, &mut first_failure);
                        if first_failure.is_some() {
                            break;
                        }
                    }
                }
                if launched_this_sweep > 0 {
                    net.deep_cursor = Some(net.id());
                }
            }
            // The inner nets whose turn comes after the outer net's: every
            // one of them under `OuterFirst`, and only once nothing out here
            // is airborne, since a guard out here may read the marking in
            // there; under `Interleaved` the rest of the round, airborne
            // bodies or not.
            if !inner_turn
                && launched_this_sweep == 0
                && (interleaved || airborne == 0)
                && !capped
                && fuse.is_none()
                && first_failure.is_none()
            {
                inner_turn = inner_step(
                    net,
                    behind,
                    limits,
                    on_failure,
                    tally,
                    &mut fuse,
                    &mut first_failure,
                );
                if !inner_turn && airborne == 0 && fuse.is_none() && first_failure.is_none() {
                    // Every net had its turn, and none had a firing.
                    return Ok(tally.summary(net, true));
                }
            }

            if capped || fuse.is_some() || first_failure.is_some() {
                while airborne > 0 {
                    let done = rx.recv().expect(SENDER_ALIVE);
                    airborne -= 1;
                    absorb(net, done, on_failure, tally, &mut first_failure);
                }
                break;
            }
            if airborne == 0 {
                // Inline firings only, and they moved the marking: sweep
                // again before calling the net quiescent.
                continue;
            }
            if inner_turn {
                // An inner net fired while bodies are airborne: take what has
                // landed meanwhile, and go round again without waiting.
                while let Ok(done) = rx.try_recv() {
                    airborne -= 1;
                    absorb(net, done, on_failure, tally, &mut first_failure);
                }
                continue;
            }

            let done = rx.recv().expect(SENDER_ALIVE);
            airborne -= 1;
            absorb(net, done, on_failure, tally, &mut first_failure);
            // Whatever else came back while that one was being distributed.
            while let Ok(done) = rx.try_recv() {
                airborne -= 1;
                absorb(net, done, on_failure, tally, &mut first_failure);
            }
        }

        if let Some((limit, transition, name)) = fuse {
            return Err(DriverError::TooManyFirings {
                limit,
                transition,
                name,
                summary: tally.summary(net, false),
            });
        }
        if let Some(error) = first_failure {
            return Err(DriverError::FiringFailed {
                summary: tally.summary(net, false),
                error,
            });
        }
        Ok(tally.summary(net, false))
    })
}

/// [`Mode::Sequential`] for [`run_indefinitely`]: one firing per tick, then
/// sleep out the rest of it.
fn tick_sequential<F>(
    net: &mut Net,
    tick: Duration,
    stop: &AtomicBool,
    on_failure: OnFailure,
    mut each_tick: F,
    tally: &mut Tally,
) -> Result<RunSummary, DriverError>
where
    F: FnMut(&mut Net),
{
    while !stop.load(Ordering::Relaxed) {
        each_tick(net);
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match net.step_deep() {
            Ok(Some(step)) => tally.landed(&step),
            // An idle tick is not a finish line here: the net is waiting.
            Ok(None) => {}
            Err(error) => {
                tally.launched += 1;
                tally.failed += 1;
                match on_failure {
                    OnFailure::Stop => {
                        return Err(DriverError::FiringFailed {
                            summary: tally.summary(net, false),
                            error,
                        });
                    }
                    OnFailure::Disable => disable(net, &error),
                    OnFailure::Continue => {}
                }
            }
        }
        thread::sleep(tick);
    }
    let quiesced = net.is_quiescent_deep();
    Ok(tally.summary(net, quiesced))
}

/// [`Mode::Concurrent`] for [`run_indefinitely`]: one launch sweep per tick,
/// and the wait on the completion channel *is* the rest of the tick.
fn tick_concurrent<F>(
    net: &mut Net,
    tick: Duration,
    stop: &AtomicBool,
    on_failure: OnFailure,
    mut each_tick: F,
    tally: &mut Tally,
) -> Result<RunSummary, DriverError>
where
    F: FnMut(&mut Net),
{
    thread::scope(|scope| {
        let (tx, rx) = mpsc::channel::<Completed>();
        let mut airborne: usize = 0;
        let mut first_failure: Option<FiringFailed> = None;

        while !stop.load(Ordering::Relaxed) && first_failure.is_none() {
            each_tick(net);
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let interleaved = net.deep_policy() == DeepPolicy::Interleaved;
            let (ahead, behind) = net.round();
            // One turn a tick: under `Interleaved`, an inner net's whose turn
            // comes before the outer net's, if one has a firing; else the
            // outer net's sweep; else an inner net's after it.
            let inner_turn = inner_step(
                net,
                ahead,
                Limits::default(),
                on_failure,
                tally,
                &mut None,
                &mut first_failure,
            );
            if first_failure.is_some() {
                break;
            }
            let mut launched_this_sweep = 0_usize;
            let sweep = if inner_turn {
                Vec::new()
            } else {
                net.fireable()
            };
            for transition in sweep {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let firing = match net.launch(transition) {
                    Ok(firing) => firing,
                    Err(NetError::NotFireable { .. }) => continue,
                    Err(misuse) => unreachable!("{}: {misuse}", LAUNCHABLE),
                };
                tally.launched += 1;
                launched_this_sweep += 1;
                if firing.is_pending() {
                    let tx = tx.clone();
                    scope.spawn(move || {
                        let _ = tx.send(firing.run());
                    });
                    airborne += 1;
                } else {
                    absorb(net, firing.run(), on_failure, tally, &mut first_failure);
                    if first_failure.is_some() {
                        break;
                    }
                }
            }
            if first_failure.is_some() {
                break;
            }
            if launched_this_sweep > 0 {
                net.deep_cursor = Some(net.id());
            }
            // Nothing out here to launch and nothing airborne (or, under
            // `Interleaved`, airborne bodies or not): one inner firing this
            // tick, inline, in the round's order.
            if !inner_turn
                && launched_this_sweep == 0
                && (interleaved || airborne == 0)
                && !stop.load(Ordering::Relaxed)
            {
                inner_step(
                    net,
                    behind,
                    Limits::default(),
                    on_failure,
                    tally,
                    &mut None,
                    &mut first_failure,
                );
                if first_failure.is_some() {
                    break;
                }
            }

            // The rest of the tick, spent waiting on the channel rather than
            // sleeping: a completion is distributed the moment it lands.
            let deadline = Instant::now() + tick;
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                match rx.recv_timeout(left) {
                    Ok(done) => {
                        airborne -= 1;
                        absorb(net, done, on_failure, tally, &mut first_failure);
                    }
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => unreachable!("{SENDER_ALIVE}"),
                }
                if first_failure.is_some() || stop.load(Ordering::Relaxed) {
                    break;
                }
            }
        }

        // Nothing goes home in flight, whichever way the loop ended.
        while airborne > 0 {
            let done = rx.recv().expect(SENDER_ALIVE);
            airborne -= 1;
            absorb(net, done, on_failure, tally, &mut first_failure);
        }

        if let Some(error) = first_failure {
            return Err(DriverError::FiringFailed {
                summary: tally.summary(net, false),
                error,
            });
        }
        let quiesced = net.is_quiescent_deep();
        Ok(tally.summary(net, quiesced))
    })
}

// ----- the pieces the loops share ------------------------------------------

/// Why a `launch` of a transition the driver just read out of
/// [`fireable`](Net::fireable) can only ever fail as `NotFireable`.
const LAUNCHABLE: &str =
    "a driver launches only what this net's own `fireable` just named, so it exists";

/// One inner firing, inline, among the inner nets whose place in the round's
/// walk is in `turns` ([`Net::round`]), for a concurrent loop: the fuse
/// first, then the firing, counted, its failure handled as `on_failure`
/// says ([`Stop`](OnFailure::Stop) keeps the first one in `first_failure`).
///
/// True when an inner net took its turn, firing or failing. False when
/// `turns` is empty, when none of those nets had a firing, or when the fuse
/// was spent and something anywhere is fireable, in which case `fuse` now
/// says which, and nothing was attempted.
fn inner_step(
    net: &mut Net,
    turns: Range<usize>,
    limits: Limits,
    on_failure: OnFailure,
    tally: &mut Tally,
    fuse: &mut Option<(u64, TransitionId, String)>,
    first_failure: &mut Option<FiringFailed>,
) -> bool {
    if turns.is_empty() {
        return false;
    }
    if let Some(limit) = limits.error_after_n_firings
        && tally.launched >= limit
        && let Some((transition, name)) = net.first_fireable_deep()
    {
        *fuse = Some((limit, transition, name));
        return false;
    }
    match net.inner_turn(turns) {
        Ok(Some(step)) => {
            tally.landed(&step);
            true
        }
        Ok(None) => false,
        Err(error) => {
            tally.launched += 1;
            tally.failed += 1;
            match on_failure {
                OnFailure::Stop => {
                    if first_failure.is_none() {
                        *first_failure = Some(error);
                    }
                }
                OnFailure::Disable => disable(net, &error),
                OnFailure::Continue => {}
            }
            true
        }
    }
}

/// Stage three, with the failure policy applied: the one place a driver
/// hands a [`Completed`] back to the net.
///
/// Both concurrent loops and the inline path go through here, so a firing
/// that ran on a worker and one that [`launch`](Net::launch) ran on the spot
/// are treated exactly alike. Only [`OnFailure::Stop`] keeps the error, and
/// only the first one: the later failures of a drain are counted and logged
/// like any other.
fn absorb(
    net: &mut Net,
    completed: Completed,
    on_failure: OnFailure,
    tally: &mut Tally,
    first_failure: &mut Option<FiringFailed>,
) {
    match net.distribute(completed) {
        Ok(()) => {}
        Err(NetError::FiringFailed(error)) => {
            tally.failed += 1;
            match on_failure {
                OnFailure::Stop => {
                    if first_failure.is_none() {
                        *first_failure = Some(error);
                    }
                }
                OnFailure::Disable => disable(net, &error),
                OnFailure::Continue => {}
            }
        }
        // A driver distributes only what it launched into this very net, so
        // the stamp matches and the transition is in flight.
        Err(misuse) => unreachable!(
            "a driver distributes only the firings it launched into this net: {misuse}"
        ),
    }
}

/// [`OnFailure::Disable`], written once: switch off the transition that just
/// failed, which appends a [`Toggled`](crate::Event::Toggled) event.
///
/// A transition of an inner net is switched off where it is, found by the
/// failure's [`path`](FiringFailed::path); its `Toggled` comes out here as an
/// [`Event::Nested`](crate::Event::Nested), like anything else an inner net
/// logs.
fn disable(net: &mut Net, failure: &FiringFailed) {
    disable_at(net, &failure.path, failure.transition);
}

/// [`disable`] one level at a time, outer to inner.
fn disable_at(net: &mut Net, path: &[(PlaceId, TokenId, String)], transition: TransitionId) {
    let Some(((place, token, name), rest)) = path.split_first() else {
        net.disable(transition)
            .expect("the transition that just failed is a transition of this net");
        return;
    };
    // The token has not moved: an inner failure changes nothing out here.
    net.nested_scope(*place, *token, |nets| {
        if let Some(entry) = nets.iter_mut().find(|entry| entry.0 == name.as_str()) {
            disable_at(&mut *entry.1, rest, transition);
        }
    });
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
    /// and an inner success if it happened inside.
    fn landed(&mut self, step: &DeepStep) {
        self.launched += 1;
        if matches!(step, DeepStep::Inside { .. }) {
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

    fn summary(&self, net: &Net, quiesced: bool) -> RunSummary {
        RunSummary {
            quiesced,
            fired_count: net.fired_count() - self.fired_at_entry + self.inner_fired,
            failed_count: self.failed,
        }
    }
}
