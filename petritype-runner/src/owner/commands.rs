//! The requests that change the net: `load`, `reset`, `inject`, `press`,
//! and the switches and parameters.

use std::collections::{HashMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::{Instant, SystemTime};

use petritype::control::Pressed;
use petritype::{Command, Event, JsonCommand, Net};
use serde_json::{Value, json};

use crate::errors::CoreError;
use crate::protocol::{ErrorCode, LogEntry, Notification};
use crate::registry::Params;
use crate::time::iso8601;
use crate::translate::{self, Names};

use super::address::{Target, descend_mut};
use super::args::{Args, optional, port_arg, string_arg};
use super::events::fireable_names;
use super::firing::Timed;
use super::lifted::Lifted;
use super::reads::resolve_place;
use super::{
    Loaded, Outgoing, Owner, ProgressKept, RpcError, holding, lock, millis, panic_message,
    path_text, push, respond, respond_error, seats_of,
};

impl Owner {
    /// `inject {place, port?, token, path?, nested_id?}`: the token decoded
    /// into the named net's place and appended (PROTOCOL.md §6.4, §12.3), or
    /// into the port `port` of its subnet place `place` (§12.7), which the
    /// net that holds the subnet place tells of. Into an inner net it goes
    /// through `nested_scope` or `subnet_scope` at every level, so its
    /// `added` comes out `nested`, and the answer carries that net's version
    /// and its address, `path` and `nested_id`. The scopes shield a net
    /// inside a closed net, so such an inject answers `closed_net` (since
    /// 2026-11-28), after `unknown_place` and `invalid_token`.
    pub(super) fn inject(
        &mut self,
        id: &str,
        args: &Args,
        now: SystemTime,
        out: &mut Vec<Outgoing>,
    ) -> Result<(), RpcError> {
        let place = string_arg(args, "place")?.to_owned();
        let port = port_arg(args)?.map(str::to_owned);
        let token = args
            .get("token")
            .cloned()
            .ok_or_else(|| RpcError::new(ErrorCode::InvalidParams, "inject needs a \"token\""))?;
        let no_event =
            || RpcError::new(ErrorCode::Internal, "the token was added without an event");
        match self.target(args)? {
            Target::Outer => {
                let net = &mut self.loaded_mut().net;
                let put = catch_unwind(AssertUnwindSafe(|| {
                    put_json(net, &place, port.as_deref(), token)
                }));
                self.injected(put, now, out)??;
                let flushed = self.flush_injected(now, out);
                let added = flushed
                    .events
                    .iter()
                    .find_map(|event| match event {
                        Event::Added { token, .. } => Some(token.to_string()),
                        _ => None,
                    })
                    .ok_or_else(no_event)?;
                respond(
                    out,
                    id,
                    json!({"status": "injected", "id": added, "marking_version": self.marking_version}),
                );
            }
            Target::Inner(found) => {
                let net = &mut self.loaded_mut().net;
                let put = catch_unwind(AssertUnwindSafe(|| {
                    descend_mut(net, &found.levels, |net| {
                        put_json(net, &place, port.as_deref(), token)
                    })
                }));
                self.injected(put, now, out)?.ok_or_else(|| {
                    RpcError::new(
                        ErrorCode::UnknownNet,
                        format!("no live inner net at {}", path_text(&found.path)),
                    )
                })??;
                let flushed = self.flush_injected(now, out);
                let added = flushed
                    .events
                    .iter()
                    .find_map(|event| {
                        let lifted = Lifted::of(event)?;
                        match lifted.event {
                            Event::Added { token, .. } if lifted.nested_id() == found.nested_id => {
                                Some(token.to_string())
                            }
                            _ => None,
                        }
                    })
                    .ok_or_else(no_event)?;
                let version = self
                    .loaded_ref()
                    .inner
                    .get(&found.nested_id)
                    .map_or(0, |inner| inner.marking_version);
                // A result about an inner net carries its address
                // (PROTOCOL.md §12.3, §12.5).
                respond(
                    out,
                    id,
                    json!({"status": "injected", "id": added, "marking_version": version,
                           "path": found.path, "nested_id": found.nested_id.to_string()}),
                );
            }
        }
        Ok(())
    }

    /// What an inject's put came to, a panic of the net's own code, a key
    /// of the token brought, say, answered `internal` naming it (since
    /// 2026-11-28; until then the panic ended the process), after what the
    /// unwinding left in the log is told. A put whose keys panic puts
    /// nothing, and a scope that panics has indexed and lifted what it did.
    fn injected<R>(
        &mut self,
        put: std::thread::Result<R>,
        now: SystemTime,
        out: &mut Vec<Outgoing>,
    ) -> Result<R, RpcError> {
        put.map_err(|payload| {
            self.tell_unwound(now, out);
            RpcError::new(
                ErrorCode::Internal,
                format!(
                    "the net's own code panicked during an inject: {}",
                    panic_message(payload.as_ref())
                ),
            )
        })
    }

    /// `press {control, value?}` (PROTOCOL.md §6): the value read as the
    /// core's ([`translate::press_value`]), the control run by
    /// [`Net::press`], which checks the value against its kind, and its
    /// events flushed as `inject`'s are. Each firing it made is logged and
    /// counted as a step's is. A handler runs on the owner's thread, so a
    /// press that fired one transition of the loaded net gives that firing
    /// the time the press took ([`Timed::only`]). The answer is `pressed`,
    /// or `nothing` with the control's reason, and the version after the
    /// events; an unknown control is `unknown_control`, a value its kind
    /// cannot take `invalid_params`, and a handler that refuses or panics
    /// `internal`. Outer-level only, as `Method::is_outer_only` has it.
    pub(super) fn press(
        &mut self,
        id: &str,
        args: &Args,
        now: SystemTime,
        out: &mut Vec<Outgoing>,
    ) -> Result<(), RpcError> {
        let control = string_arg(args, "control")?.to_owned();
        let value = translate::press_value(optional(args, "value"))
            .map_err(|message| RpcError::new(ErrorCode::InvalidParams, message))?;
        let loaded = self.loaded_mut();
        let started = Instant::now();
        let pressed = catch_unwind(AssertUnwindSafe(|| loaded.net.press(&control, value)));
        let took = started.elapsed();
        // A token the control put into a nested place may carry nets nobody
        // has seen: their fireable sets are taken now, silently (§7.2).
        let live = loaded.know_new();
        let events: Vec<Event> = loaded.net.drain_events().collect();
        let timed = Timed::only(&events, took);
        let flushed = self.tell_events(events, now, None, timed.as_ref(), true, out);
        self.log_pressed(&flushed.events, timed.as_ref(), now);
        // A token a firing consumed took its inner nets with it (§12.6).
        self.loaded_mut().prune(&live);
        let pressed = pressed
            .map_err(|panic| {
                RpcError::new(
                    ErrorCode::Internal,
                    format!(
                        "control {control:?} panicked: {}",
                        panic_message(panic.as_ref())
                    ),
                )
            })?
            .map_err(|error| RpcError::core(CoreError::Press(&error)))?;
        let answer = match pressed {
            Pressed::Applied => {
                json!({"status": "pressed", "marking_version": self.marking_version})
            }
            Pressed::Nothing(reason) => json!({
                "status": "nothing",
                "reason": reason,
                "marking_version": self.marking_version,
            }),
        };
        respond(out, id, answer);
        Ok(())
    }

    /// Logs and counts the firings a press made, as a step's landing does:
    /// each `fired` and `failed` of the outer net, at `now`, lasting the
    /// time `timed` gives the one it names, and 0 for any other.
    fn log_pressed(&mut self, events: &[Event], timed: Option<&Timed>, now: SystemTime) {
        let loaded = self.loaded_mut();
        for event in events {
            let took = timed.and_then(|timed| timed.of(event));
            let (transition, consumed, produced, error, restored) = match event {
                Event::Fired {
                    transition,
                    consumed,
                    produced,
                } => (transition, consumed, &produced[..], None, None),
                Event::Failed {
                    transition,
                    consumed,
                    restored,
                    error,
                } => (
                    transition,
                    consumed,
                    &[][..],
                    Some(error.clone()),
                    Some(*restored),
                ),
                _ => continue,
            };
            let name = loaded.names.transition(*transition);
            let failed = error.is_some();
            loaded.log_seq += 1;
            let entry = LogEntry {
                seq: loaded.log_seq,
                transition: name.clone(),
                started_at: iso8601(now),
                duration_ms: took.map_or(0.0, millis),
                outcome: if failed { "failed" } else { "fired" }.to_owned(),
                consumed: loaded.names.pairs(consumed),
                produced: loaded.names.pairs(produced),
                error,
                restored,
                path: None,
                nested_id: None,
            };
            loaded.push_log(entry, Vec::new());
            if !failed {
                loaded.progress.fired(&name, now);
            }
        }
    }

    pub(super) fn apply(&mut self, command: JsonCommand) -> Result<(), RpcError> {
        self.loaded_mut()
            .net
            .apply_json(command)
            .map_err(|error| RpcError::core(CoreError::Command(&error)))
    }

    pub(super) fn load(&mut self, id: &str, args: &Args, out: &mut Vec<Outgoing>) {
        if optional(args, "net").is_none()
            && ["module", "function", "factory_params"]
                .iter()
                .any(|key| optional(args, key).is_some())
        {
            respond_error(
                out,
                id,
                RpcError::new(
                    ErrorCode::InvalidParams,
                    "the {module, function} form of load is Python's; a Rust runner loads {net, params}",
                ),
            );
            return;
        }
        let name = match args.get("net") {
            Some(Value::String(name)) => name.clone(),
            _ => {
                respond_error(
                    out,
                    id,
                    RpcError::new(ErrorCode::InvalidParams, "load needs a string \"net\""),
                );
                return;
            }
        };
        let Some(entry) = self.nets.iter().position(|e| e.name == name) else {
            respond_error(
                out,
                id,
                RpcError::new(ErrorCode::UnknownNet, format!("no net named {name:?}")),
            );
            return;
        };
        let given = match optional(args, "params") {
            None => Params::new(),
            Some(Value::Object(params)) => params.clone(),
            Some(_) => {
                respond_error(
                    out,
                    id,
                    RpcError::new(ErrorCode::InvalidParams, "\"params\" must be an object"),
                );
                return;
            }
        };
        let params = match self.nets[entry].complete(given) {
            Ok(params) => params,
            Err(message) => {
                respond_error(out, id, RpcError::new(ErrorCode::InvalidParams, message));
                return;
            }
        };
        let running = self.loaded.as_ref().is_some_and(|l| l.running);
        push(
            out,
            Notification::LoadProgress {
                step: "build".into(),
                message: format!("building {name}"),
                marking_version: self.marking_version,
                running,
            },
        );
        if self.build(id, entry, params, "load", out) {
            respond(
                out,
                id,
                json!({"status": "loaded", "generation": self.generation, "marking_version": self.marking_version}),
            );
        }
    }

    pub(super) fn reset(&mut self, id: &str, out: &mut Vec<Outgoing>) {
        let loaded = self.loaded_ref();
        let (entry, params) = (loaded.entry, loaded.params.clone());
        if self.build(id, entry, params, "reset", out) {
            respond(
                out,
                id,
                json!({"status": "reset", "generation": self.generation, "marking_version": self.marking_version}),
            );
        }
    }

    /// Runs the factory and, if it gives a net, installs it as the next
    /// generation and writes `reloaded`. On failure writes the error
    /// response, leaves the previous net as it was, and returns false.
    ///
    /// Every inner net goes with the old marking (PROTOCOL.md §12.6): the
    /// new generation's are walked and recorded afresh, at version 0, its
    /// subnets among them (§12.7).
    fn build(
        &mut self,
        id: &str,
        entry: usize,
        params: Params,
        reason: &str,
        out: &mut Vec<Outgoing>,
    ) -> bool {
        let factory = &self.nets[entry].factory;
        let built = catch_unwind(AssertUnwindSafe(|| factory(&params)));
        let net = match built {
            Ok(Ok(net)) => net,
            Ok(Err(error)) => {
                respond_error(out, id, RpcError::core(CoreError::Load(&error)));
                return false;
            }
            Err(payload) => {
                respond_error(
                    out,
                    id,
                    RpcError::new(
                        ErrorCode::LoadFailed,
                        format!("the factory panicked: {}", panic_message(payload.as_ref())),
                    ),
                );
                return false;
            }
        };
        self.install(entry, params, net);
        push(
            out,
            Notification::Reloaded {
                generation: self.generation,
                reason: reason.to_owned(),
                marking_version: self.marking_version,
                running: false,
            },
        );
        true
    }

    /// Installs `net` as the next generation of the entry at `entry`, with
    /// nothing running and every inner net recorded afresh.
    pub(super) fn install(&mut self, entry: usize, params: Params, mut net: Net) {
        // A new generation starts from its built marking; whatever the
        // factory did to it on the way is part of that.
        net.drain_events().for_each(drop);
        let topology = net.topology();
        let names = Names::new(&topology);
        let last_fireable = fireable_names(&net, &names);
        let seats = seats_of(&net);
        self.generation += 1;
        let mut loaded = Loaded {
            holding: holding(&topology),
            seats,
            entry,
            name: self.nets[entry].name,
            params,
            net,
            topology,
            names,
            running: false,
            in_flight: None,
            deferred: VecDeque::new(),
            last_fireable,
            log: VecDeque::new(),
            log_seq: 0,
            progress: ProgressKept::default(),
            idle: false,
            pending_param: None,
            inner: HashMap::new(),
            touched: Vec::new(),
            projection: None,
        };
        loaded.know_all();
        self.loaded = Some(loaded);
        *lock(&self.status) = None;
    }
}

/// Appends one token, as JSON, to `place` of `net`, or to the port `port`
/// of its subnet place `place` (PROTOCOL.md §6.4, §12.7). A port's token is
/// decoded as the subnet's port decodes it and put into the place `net`
/// has for the port, so that `net` tells of it, under the subnet's id.
fn put_json(net: &mut Net, place: &str, port: Option<&str>, token: Value) -> Result<(), RpcError> {
    let failed = |error: petritype::CommandError| RpcError::core(CoreError::Command(&error));
    let id = resolve_place(net, place, port)?;
    let Some(port) = port else {
        return net
            .apply_json(JsonCommand::Extend {
                place: place.to_owned(),
                tokens: vec![token],
            })
            .map_err(failed);
    };
    let subnet = net
        .place_named(place)
        .and_then(|subnet| net.subnet(subnet))
        .expect("resolve_place found the subnet place");
    let decoded = Command::from_json(
        subnet,
        JsonCommand::Extend {
            place: port.to_owned(),
            tokens: vec![token],
        },
    )
    .map_err(failed)?;
    let Command::Extend { tokens, .. } = decoded else {
        unreachable!("an extend decodes to an extend")
    };
    net.apply(Command::Extend { place: id, tokens })
        .map_err(failed)
}
