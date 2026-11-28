//! The firing cycle: a step, a launch, a landing, and a running net driven
//! from one firing to the next.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use petritype::{
    BodyPanicked, Completed, DeepStep, Event, FiringFailed, NetError, PlaceId, StagedStep,
    StepError, TokenId, TransitionId,
};
use serde_json::json;

use crate::errors::CoreError;
use crate::protocol::{ErrorCode, LogEntry, Notification};
use crate::registry::Hosting;
use crate::time::iso8601;

use super::address::inner_address;
use super::lifted::Lifted;
use super::{
    InFlight, Outgoing, Owner, RpcError, StatusCell, lock, panic_message, push, respond,
    respond_error,
};

/// A firing whose body ran inline inside one core call, a deep step's or a
/// press's, and how long that call took: the `duration_ms` its `fired`
/// and its log entry carry. It is named as its events name it: the way
/// down to the net it was in, one place per level with the token that
/// holds the net (none for a subnet), empty for the loaded net, and its
/// transition there. A firing that a body made inside the tokens it holds
/// is in a net further down, so it is never this one.
pub(super) struct Timed {
    levels: Vec<(PlaceId, Option<TokenId>)>,
    transition: TransitionId,
    took: Duration,
}

impl Timed {
    /// The inner firing a deep step made, `step`, which took `took`.
    fn inside(mut step: &DeepStep, took: Duration) -> Timed {
        let transition = step.transition();
        let mut levels = Vec::new();
        loop {
            step = match step {
                DeepStep::Inside {
                    place, token, step, ..
                } => {
                    levels.push((*place, Some(*token)));
                    step
                }
                DeepStep::Subnet { place, step, .. } => {
                    levels.push((*place, None));
                    step
                }
                _ => break,
            };
        }
        Timed {
            levels,
            transition,
            took,
        }
    }

    /// The inner firing a deep step made that failed, which took `took`.
    /// `None` for a path with a step this runner does not know.
    fn failed(failed: &FiringFailed, took: Duration) -> Option<Timed> {
        let levels = failed
            .path
            .iter()
            .map(|step| match step {
                petritype::PathStep::Token { place, token, .. } => Some((*place, Some(*token))),
                petritype::PathStep::Subnet { place } => Some((*place, None)),
                _ => None,
            })
            .collect::<Option<_>>()?;
        Some(Timed {
            levels,
            transition: failed.transition,
            took,
        })
    }

    /// The firing of the loaded net among `events`, which a call that took
    /// `took` made, when it made exactly one; `None` when it made none, or
    /// more than one, whose times the one call does not tell apart.
    pub(super) fn only(events: &[Event], took: Duration) -> Option<Timed> {
        let mut firings = events.iter().filter_map(|event| match event {
            Event::Fired { transition, .. } | Event::Failed { transition, .. } => Some(*transition),
            _ => None,
        });
        let transition = firings.next()?;
        firings.next().is_none().then_some(Timed {
            levels: Vec::new(),
            transition,
            took,
        })
    }

    /// How long this firing took, if `event` is the one that ends it, its
    /// `Fired` or its `Failed`, outer or lifted; `None` for any other.
    pub(super) fn of(&self, event: &Event) -> Option<Duration> {
        let (levels, ended): (Vec<_>, _) = match Lifted::of(event) {
            Some(lifted) => (
                lifted.levels.iter().map(|l| (l.place, l.token)).collect(),
                lifted.event,
            ),
            None => (Vec::new(), event),
        };
        let (Event::Fired { transition, .. } | Event::Failed { transition, .. }) = ended else {
            return None;
        };
        (*transition == self.transition && levels == self.levels).then_some(self.took)
    }
}

impl Owner {
    /// One firing (PROTOCOL.md §6.3, §12.3), as the stepping rules of the
    /// loaded net's nested places order it (`Net::step_deep_staged`): the
    /// outer net's own choice, launched in stages, or an inner firing, made
    /// inline. Under the default rule, inner first, an inner net's firing
    /// comes first, and the outer net's choice only when every inner net in
    /// such a place is at rest.
    pub(super) fn step(
        &mut self,
        id: &str,
        now: SystemTime,
        out: &mut Vec<Outgoing>,
    ) -> Result<(), RpcError> {
        let (stepped, took) = match self.step_deep_timed(now) {
            Ok(stepped) => stepped,
            // The net's own code panicked choosing: nothing fired, and the
            // answer says what panicked (`internal`), as for a choice that
            // could not be made. What the unwinding left in the log is told.
            Err(panicked) => {
                self.tell_unwound(now, out);
                return Err(RpcError::new(ErrorCode::Internal, panicked));
            }
        };
        match stepped {
            Ok(Some(StagedStep::Chosen(transition))) => {
                return self
                    .launch(transition, Some(id.to_owned()), now, out)
                    .map_err(LaunchError::into_answer);
            }
            Ok(Some(StagedStep::Inside(step))) => {
                let timed = Timed::inside(&step, took);
                self.report_inside(None, Some(timed), Some(id), now, out);
            }
            Ok(Some(_)) => self.report_inside(None, None, Some(id), now, out),
            Err(StepError::FiringFailed(failed)) => {
                let timed = Timed::failed(&failed, took);
                self.report_inside(Some(Failure::of(&failed)), timed, Some(id), now, out);
            }
            // A choice that could not be made: nothing fired, so nothing is
            // told, and the answer says why, as the Python runner's does
            // (`internal`).
            Err(error) => return Err(RpcError::new(ErrorCode::Internal, error.to_string())),
            Ok(None) => {
                let running = self.loaded_ref().running;
                respond(
                    out,
                    id,
                    json!({"fired": false, "marking_version": self.marking_version, "running": running}),
                );
            }
        }
        Ok(())
    }

    /// One deep step of the loaded net (`Net::step_deep_staged_with`), and
    /// how long it took. When it fired inside, that firing's body ran inline
    /// in this call, so this is how long the body took.
    ///
    /// Just before such a body starts, the status cell is set to name it:
    /// its transition, `now`, and the address of the net it is in. It is
    /// cleared when the step returns, the body having fired or failed, and
    /// when the step unwinds. The `running` gauge is mirrored first, for the
    /// heartbeats during that body, or during the body of the choice it
    /// hands back, which is launched next.
    ///
    /// # Errors
    ///
    /// What the net's own code said when it panicked during the step: a
    /// selector, a priority, a guard, a key, the nets a token lends
    /// (since 2026-11-28; until then the panic ended the process). The
    /// core puts back what the step had taken before it lets the panic go
    /// on, and logs what it lifted, which the caller tells.
    fn step_deep_timed(
        &mut self,
        now: SystemTime,
    ) -> Result<(Result<Option<StagedStep>, StepError>, Duration), String> {
        self.mirror_running();
        // No body is airborne when a deep step is made (a `step` waits for a
        // landing, and a run fires only when nothing is in flight), so the
        // cell names nothing but what this step puts in it.
        let _clear = ClearStatus::new(&self.status);
        let status = &self.status;
        let loaded = self.loaded.as_mut().expect("a deep step needs a net");
        let (names, inner) = (&loaded.names, &loaded.inner);
        let started = Instant::now();
        let stepped = catch_unwind(AssertUnwindSafe(|| {
            loaded.net.step_deep_staged_with(|body| {
                let address = inner_address(names, inner, body);
                *lock(status) = Some((body.name.to_owned(), now, address));
            })
        }));
        let took = started.elapsed();
        stepped.map(|stepped| (stepped, took)).map_err(|payload| {
            format!(
                "the net's own code panicked during a step: {}",
                panic_message(payload.as_ref())
            )
        })
    }

    /// Tells what a step or a launch that panicked left in the net's log,
    /// as any change is told, the fireable sets that changed included.
    pub(super) fn tell_unwound(&mut self, now: SystemTime, out: &mut Vec<Outgoing>) {
        let live = self.loaded_mut().know_new();
        self.flush_events(now, None, true, out);
        self.loaded_mut().prune(&live);
    }

    /// Reports the inner firing a deep step just made inline: its events,
    /// `nested`, the `step` that asked for it if one did, and a stop of a
    /// run on a failure (`failure`). `timed` names the firing and how long
    /// it took, for its `fired` and its log entry. The inner net's progress
    /// counted it when the flush lifted it, and no net around it counts it
    /// (§5.4).
    fn report_inside(
        &mut self,
        failure: Option<Failure>,
        timed: Option<Timed>,
        step_id: Option<&str>,
        now: SystemTime,
        out: &mut Vec<Outgoing>,
    ) {
        let loaded = self.loaded_mut();
        loaded.idle = false;
        // A token the firing put into a nested place may carry nets nobody
        // has seen: their fireable sets are taken now, silently (§7.2).
        let events: Vec<Event> = loaded.net.drain_events().collect();
        let live = loaded.know_new_after(&events);
        let flushed = self.tell_events(events, now, None, timed.as_ref(), true, out);
        let loaded = self.loaded_mut();
        if let Some(live) = &live {
            loaded.prune(live);
        }
        if let Some(failure) = &failure
            && loaded.running
        {
            loaded.running = false;
            push(
                out,
                Notification::ExecutionStopped {
                    reason: "transition_failed".into(),
                    error_type: failure.error_type.map(str::to_owned),
                    message: Some(failure.message.clone()),
                    marking_version: self.marking_version,
                    running: false,
                },
            );
        }
        let Some(step_id) = step_id else {
            return;
        };
        // The firing's own line said where it was: the last inner firing
        // the flush reported.
        let Some(firing) = flushed.inner_firings.last() else {
            respond_error(
                out,
                step_id,
                RpcError::new(ErrorCode::Internal, "an inner firing left no event"),
            );
            return;
        };
        let mut answer = json!({
            "fired": true,
            "transition": firing.transition,
            "path": firing.path,
            "nested_id": firing.nested_id,
            "marking_version": self.marking_version,
            "running": self.loaded_ref().running,
        });
        if failure.is_some() {
            answer["failed"] = json!(true);
        }
        respond(out, step_id, answer);
    }

    /// Launches `transition`. A detachable body goes out as
    /// [`Outgoing::Launch`]; an inline one has already run, timed round
    /// the launch, and lands here.
    ///
    /// The status cell names the transition from just before the launch
    /// until the firing lands; a launch that is refused or panics clears it
    /// at once.
    ///
    /// # Errors
    ///
    /// [`LaunchError::Refused`] when the net refuses the launch, and
    /// [`LaunchError::Panicked`] when the net's own code panics in it, a
    /// guard, a key or the nets a token lends (since 2026-11-28; until then
    /// the panic ended the process); what the unwinding left in the log is
    /// told first.
    fn launch(
        &mut self,
        transition: TransitionId,
        step_id: Option<String>,
        now: SystemTime,
        out: &mut Vec<Outgoing>,
    ) -> Result<(), LaunchError> {
        let loaded = self.loaded_mut();
        let name = loaded.names.transition(transition);
        *lock(&self.status) = Some((name.clone(), now, None));
        let clear = ClearStatus::new(&self.status);
        let loaded = self.loaded_mut();
        let started = Instant::now();
        let launched = catch_unwind(AssertUnwindSafe(|| loaded.net.launch(transition)));
        let took = started.elapsed();
        let firing = match launched {
            Ok(Ok(firing)) => firing,
            Ok(Err(error)) => {
                return Err(LaunchError::Refused(RpcError::core(CoreError::Net(&error))));
            }
            Err(payload) => {
                drop(clear);
                self.tell_unwound(now, out);
                return Err(LaunchError::Panicked(format!(
                    "the net's own code panicked launching {name:?}: {}",
                    panic_message(payload.as_ref())
                )));
            }
        };
        // A detachable body runs from now until it lands, which clears the
        // cell; an inline one has run, and its landing, next, clears it too.
        clear.keep();
        let loaded = self.loaded_mut();
        loaded.idle = false;
        let inline = (!firing.is_pending()).then_some(took);
        loaded.in_flight = Some(InFlight {
            transition,
            name,
            started_at: now,
            step_id,
            consumed: Vec::new(),
            inline,
        });
        if inline.is_some() {
            // `launch` ran the body; `run` only unwraps the outcome.
            self.land(firing.run(), now, out);
        } else {
            // The fireable set is compared when the firing lands, not now:
            // the launch's own change (this transition in flight) is
            // transient, and a `fireable` between `launched` and
            // `distributed` would split the pair a viewer folds into `fired`.
            let flushed = self.flush_events(now, None, false, out);
            let consumed = flushed.events.into_iter().find_map(|event| match event {
                Event::Launched { consumed, .. } => Some(consumed),
                _ => None,
            });
            if let Some(flight) = &mut self.loaded_mut().in_flight {
                flight.consumed = consumed.unwrap_or_default();
            }
            out.push(Outgoing::Launch(firing));
        }
        Ok(())
    }

    /// Distributes a firing that has run, and reports everything its landing
    /// means: the events, the log entry, progress, the fireable set, a stop
    /// on failure, and the `step` that was waiting for it.
    pub(super) fn land(&mut self, completed: Completed, now: SystemTime, out: &mut Vec<Outgoing>) {
        let loaded = self.loaded_mut();
        let flight = loaded
            .in_flight
            .take()
            .expect("land is only called with a firing in flight");
        // The loaded net launched it and no body holds the loaded net, so
        // nothing is handed back: only a failed firing comes back here.
        let landed = catch_unwind(AssertUnwindSafe(|| loaded.net.distribute(completed)));
        let result = match landed {
            Ok(result) => result.map_err(NetError::from),
            Err(payload) => {
                let message = format!(
                    "the net's own code panicked landing {:?}: {}",
                    flight.name,
                    panic_message(payload.as_ref())
                );
                self.landing_panicked(flight, message, now, out);
                return;
            }
        };
        // A token the firing produced may carry nets nobody has seen: their
        // fireable sets are taken at the landing, silently (§7.2), so the
        // first `fireable` for one reports its first change.
        let events: Vec<Event> = loaded.net.drain_events().collect();
        let live = loaded.know_new_after(&events);
        *lock(&self.status) = None;
        let events = self
            .tell_events(events, now, Some(&flight), None, true, out)
            .events;

        let consumed = events.iter().find_map(|event| match event {
            Event::Launched { consumed, .. } | Event::Failed { consumed, .. } => {
                Some(consumed.clone())
            }
            _ => None,
        });
        let produced = events.iter().find_map(|event| match event {
            Event::Distributed { produced, .. } => Some(produced.clone()),
            _ => None,
        });
        let failure = match &result {
            Ok(()) => None,
            Err(NetError::FiringFailed(failed)) => Some(Failure::of(failed)),
            Err(other) => Some(Failure {
                error_type: None,
                message: other.to_string(),
                restored: false,
            }),
        };

        let duration_ms = flight.duration_ms(now);
        let loaded = self.loaded_mut();
        loaded.log_seq += 1;
        let consumed = consumed.unwrap_or(flight.consumed);
        let entry = LogEntry {
            seq: loaded.log_seq,
            transition: flight.name.clone(),
            started_at: iso8601(flight.started_at),
            duration_ms,
            outcome: if failure.is_some() { "failed" } else { "fired" }.to_owned(),
            consumed: loaded.names.pairs(&consumed),
            produced: loaded.names.pairs(&produced.unwrap_or_default()),
            error: failure.as_ref().map(|f| f.message.clone()),
            restored: failure.as_ref().map(|f| f.restored),
            path: None,
            nested_id: None,
        };
        loaded.push_log(entry, Vec::new());
        if failure.is_none() {
            loaded.progress.fired(&flight.name, now);
        }
        // A token this firing consumed and did not put back took its inner
        // nets with it (§12.6).
        if let Some(live) = &live {
            loaded.prune(live);
        }

        if let Some(failure) = &failure
            && loaded.running
        {
            loaded.running = false;
            push(
                out,
                Notification::ExecutionStopped {
                    reason: "transition_failed".into(),
                    error_type: failure.error_type.map(str::to_owned),
                    message: Some(failure.message.clone()),
                    marking_version: self.marking_version,
                    running: false,
                },
            );
        }

        if let Some(step_id) = flight.step_id {
            let failed = matches!(result, Err(NetError::FiringFailed(_)));
            match result {
                Ok(()) | Err(NetError::FiringFailed(_)) => {
                    let running = self.loaded_ref().running;
                    let mut answer = json!({
                        "fired": true,
                        "transition": flight.name,
                        "marking_version": self.marking_version,
                        "running": running,
                    });
                    // PROTOCOL.md §6.3: a step whose body failed says so.
                    if failed {
                        answer["failed"] = json!(true);
                    }
                    respond(out, &step_id, answer);
                }
                Err(other) => respond_error(
                    out,
                    &step_id,
                    RpcError::new(ErrorCode::Internal, other.to_string()),
                ),
            }
        }
    }

    /// What a landing whose distribution panicked comes to (since
    /// 2026-11-28; until then the panic ended the process): the net's own
    /// code panicked placing what the body made, a key of an output, say.
    /// The core put the body back in its slot before it placed anything,
    /// so nothing is left in flight. What the unwinding left in the log is
    /// told, a run stops as on a body that panicked, and the `step` that
    /// launched the firing, if one did, is answered `internal` naming the
    /// panic, as a step whose own choosing panicked is. The firing is not
    /// logged, as a launch that panicked is not.
    fn landing_panicked(
        &mut self,
        flight: InFlight,
        message: String,
        now: SystemTime,
        out: &mut Vec<Outgoing>,
    ) {
        *lock(&self.status) = None;
        self.tell_unwound(now, out);
        if self.loaded_ref().running {
            self.stop_on("panic", message.clone(), out);
        }
        if let Some(step_id) = flight.step_id {
            respond_error(out, &step_id, RpcError::new(ErrorCode::Internal, message));
        }
    }

    /// Keeps a running net going: one firing in the order of the loaded
    /// net's stepping rules, as `step` makes it (the outer net's next,
    /// launched, or an inner one, inline, §12.3), or handles quiescence
    /// (nothing fireable anywhere) with `idle` once per spell for a `"24/7"`
    /// net and `execution_stopped` for any other.
    pub(super) fn drive(&mut self, now: SystemTime, out: &mut Vec<Outgoing>) {
        let Some(loaded) = &self.loaded else {
            return;
        };
        // A hosted net is the host program's to fire, running or not.
        if self.hosted || !loaded.running || loaded.in_flight.is_some() {
            return;
        }
        let continuous = self.nets[loaded.entry].hosting == Hosting::Continuous;
        let (stepped, took) = match self.step_deep_timed(now) {
            Ok(stepped) => stepped,
            // The net's own code panicked choosing: the run stops, as on a
            // body that panicked, and a later `start` asks again.
            Err(panicked) => {
                self.tell_unwound(now, out);
                self.stop_on("panic", panicked, out);
                return;
            }
        };
        match stepped {
            Ok(Some(StagedStep::Chosen(transition))) => {
                match self.launch(transition, None, now, out) {
                    Ok(()) => {}
                    Err(LaunchError::Panicked(message)) => {
                        self.stop_on("panic", message, out);
                        return;
                    }
                    // The selector chose it, so `launch` cannot refuse it;
                    // if it somehow does, stop rather than spin.
                    Err(LaunchError::Refused(refused)) => {
                        let message = format!("{}: {}", refused.code, refused.message);
                        self.stop_on("error", message, out);
                        return;
                    }
                }
            }
            // Inline, like a mutate-edge body: one per `handle`, and a tick
            // asked for so that a `stop` in between is heard.
            Ok(Some(StagedStep::Inside(step))) => {
                let timed = Timed::inside(&step, took);
                self.report_inside(None, Some(timed), None, now, out);
            }
            Ok(Some(_)) => self.report_inside(None, None, None, now, out),
            Err(StepError::FiringFailed(failed)) => {
                let timed = Timed::failed(&failed, took);
                self.report_inside(Some(Failure::of(&failed)), timed, None, now, out);
            }
            // A choice that could not be made: nothing fired, and the run
            // stops, as on a failed firing; a later `start` asks again.
            Err(error) => {
                let error_type = if error.selection().is_some() {
                    "selection"
                } else {
                    "error"
                };
                self.loaded_mut().running = false;
                push(
                    out,
                    Notification::ExecutionStopped {
                        reason: "transition_failed".into(),
                        error_type: Some(error_type.into()),
                        message: Some(error.to_string()),
                        marking_version: self.marking_version,
                        running: false,
                    },
                );
                return;
            }
            Ok(None) => {
                self.quiescent(continuous, now, out);
                return;
            }
        }
        let loaded = self.loaded_ref();
        self.poke = loaded.running && loaded.in_flight.is_none();
    }

    /// Stops a run that could not go on, with `execution_stopped`, reason
    /// `transition_failed`, and `error_type` and `message` saying why.
    fn stop_on(&mut self, error_type: &str, message: String, out: &mut Vec<Outgoing>) {
        self.loaded_mut().running = false;
        push(
            out,
            Notification::ExecutionStopped {
                reason: "transition_failed".into(),
                error_type: Some(error_type.into()),
                message: Some(message),
                marking_version: self.marking_version,
                running: false,
            },
        );
    }

    /// A running net found nothing fireable anywhere: `idle` once per spell
    /// for a `"24/7"` net, which keeps running, and `execution_stopped` for
    /// any other, which stops.
    fn quiescent(&mut self, continuous: bool, now: SystemTime, out: &mut Vec<Outgoing>) {
        if continuous {
            if !self.loaded_ref().idle {
                self.loaded_mut().idle = true;
                push(
                    out,
                    Notification::Idle {
                        idle_since: iso8601(now),
                        marking_version: self.marking_version,
                        running: true,
                    },
                );
            }
            return;
        }
        self.loaded_mut().running = false;
        push(
            out,
            Notification::ExecutionStopped {
                reason: "no_fireable_transitions".into(),
                error_type: None,
                message: None,
                marking_version: self.marking_version,
                running: false,
            },
        );
    }
}

/// Why [`Owner::launch`] launched nothing.
#[derive(Debug)]
enum LaunchError {
    /// The net refused the launch: the answer that says why.
    Refused(RpcError),
    /// The net's own code panicked in the launch: what it said.
    Panicked(String),
}

impl LaunchError {
    /// The error a `step` is answered with: a panic is `internal`.
    fn into_answer(self) -> RpcError {
        match self {
            LaunchError::Refused(refused) => refused,
            LaunchError::Panicked(message) => RpcError::new(ErrorCode::Internal, message),
        }
    }
}

/// Clears the status cell when dropped, unless told to [`keep`] it: the
/// body it names no longer runs once the step or launch that named it has
/// returned or unwound, and a heartbeat must not name it after that.
///
/// [`keep`]: ClearStatus::keep
struct ClearStatus {
    cell: StatusCell,
    armed: bool,
}

impl ClearStatus {
    fn new(cell: &StatusCell) -> ClearStatus {
        ClearStatus {
            cell: Arc::clone(cell),
            armed: true,
        }
    }

    /// Leaves the cell as it is: the body it names runs on.
    fn keep(mut self) {
        self.armed = false;
    }
}

impl Drop for ClearStatus {
    fn drop(&mut self) {
        if self.armed {
            *lock(&self.cell) = None;
        }
    }
}

/// How a landing failed, for `execution_stopped` and the log.
struct Failure {
    error_type: Option<&'static str>,
    message: String,
    restored: bool,
}

impl Failure {
    fn of(failed: &FiringFailed) -> Failure {
        let panicked = failed.error.downcast_ref::<BodyPanicked>().is_some();
        Failure {
            error_type: Some(if panicked { "panic" } else { "error" }),
            message: failed.error.to_string(),
            restored: failed.restored,
        }
    }
}
