//! A net the host program owns: read over the wire, fired by the program.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

use petritype::{Event, Net, PlaceId, TokenId, TransitionId};

use crate::protocol::LogEntry;
use crate::registry::{LoadError, NetEntry, Params};
use crate::time::iso8601;

use super::address::{path_of, walk_all};
use super::events::{changed_places, fireable_names, is_state_event, wire_order};
use super::lifted::is_lifted;
use super::{Loaded, Outgoing, Owner, StatusCell, duration_ms};

/// A staged firing the host program launched on a hosted net, between its
/// `Launched` and its `Distributed` or `Failed`.
pub(super) struct HostFlight {
    transition: TransitionId,
    consumed: Vec<(PlaceId, TokenId)>,
    started_at: SystemTime,
}

/// What a hosted owner's net is called when something reaches for it.
const HOSTED: &str = "a hosted owner always holds its net";

impl Owner {
    /// An owner of `net`, which the host program built and keeps firing:
    /// what [`Host`](crate::Host) wraps.
    ///
    /// The net is loaded from the start, as generation 1, under `name`. The
    /// wire reads it (`describe` and the reads) and nothing else: every
    /// other method is `unsupported`, and the owner never fires it. It
    /// reports `running: true` until [`set_running`](Owner::set_running)
    /// says otherwise.
    pub(crate) fn hosted(
        name: &'static str,
        net: Net,
        status: StatusCell,
        heartbeat: Duration,
    ) -> Owner {
        let entry = NetEntry::new(name, |_: &Params| {
            Err(LoadError::Other("the host program owns this net".into()))
        })
        .description("Owned by the program that serves it.");
        let mut owner = Owner::new(vec![entry], status, heartbeat);
        owner.hosted = true;
        owner.install(0, Params::new(), net);
        owner.loaded.as_mut().expect(HOSTED).running = true;
        owner.mirror_gauges();
        owner
    }

    /// The hosted net, to read.
    pub(crate) fn hosted_net(&self) -> &Net {
        &self.loaded.as_ref().expect(HOSTED).net
    }

    /// The version the loaded net's marking is at: the version of the
    /// last line [`handle`](Owner::handle) or a hosted change made that
    /// moved it.
    pub(crate) fn marking_version(&self) -> u64 {
        self.marking_version
    }

    /// Whether the hosted net is reported as running.
    pub(crate) fn running(&self) -> bool {
        self.loaded.as_ref().is_some_and(|l| l.running)
    }

    /// What every later line says of the hosted net's `running`: whether
    /// the host program is firing it.
    pub(crate) fn set_running(&mut self, running: bool) {
        self.loaded.as_mut().expect(HOSTED).running = running;
        self.mirror_gauges();
    }

    /// Whether anyone reads the lines that tell of the hosted net's
    /// changes. While nobody does, [`host_changes`](Owner::host_changes)
    /// builds no line. Turned on, it first takes the fireable sets afresh
    /// and tells no one, so that the next `fireable` line goes out exactly
    /// when a set differs from what a reader of the state now holds.
    /// Setting it to what it already is does nothing.
    pub(crate) fn set_listening(&mut self, listening: bool) {
        if listening && !self.listening {
            self.loaded.as_mut().expect(HOSTED).refresh_fireable();
        }
        self.listening = listening;
    }

    /// Hands the hosted net to `change`, then reports whatever it did as
    /// the lines a step or an inject would have caused: one notification
    /// per event, the fireable sets that changed, and a log entry and
    /// progress for every firing.
    ///
    /// The fireable sets are compared once no staged firing the host
    /// launched is still in flight, as a runner compares them at a landing
    /// and never between a `launched` and its `distributed`.
    ///
    /// While nobody is listening ([`set_listening`](Owner::set_listening))
    /// there are no lines: [`note_events`](Owner::note_events) keeps the
    /// rest and compares no fireable set.
    ///
    /// A call that leaves the net's event log empty costs the closure and
    /// nothing else.
    pub(crate) fn host_changes<R>(
        &mut self,
        now: SystemTime,
        change: impl FnOnce(&mut Net) -> R,
    ) -> (R, Vec<Outgoing>) {
        let mut out = Vec::new();
        let loaded = self.loaded.as_mut().expect(HOSTED);
        let result = change(&mut loaded.net);
        let events: Vec<Event> = loaded.net.drain_events().collect();
        if events.is_empty() {
            return (result, out);
        }
        // A token the host put in or a firing produced may carry nets
        // nobody has seen: their fireable sets are taken silently (§7.2).
        let live = loaded.know_new();
        // Each firing is counted in the progress of the net it was in alone
        // (PROTOCOL.md §5.4), as the events are told or noted.
        if self.listening {
            self.tell_events(events, now, None, None, true, &mut out);
        } else {
            self.note_events(&events, now);
        }
        let loaded = self.loaded.as_mut().expect(HOSTED);
        // While a staged firing's body holds tokens, the nets inside them
        // keep their records, as they do under `launch`.
        if self.host_flights.is_empty() {
            loaded.prune(&live);
        }
        self.mirror_gauges();
        (result, out)
    }

    /// Keeps what `events` changed, for a hosted net nobody is listening
    /// to, and builds no notification: the marking version, raised once
    /// for each event a state notification would have told of, and the
    /// versions across the port places it changed (§12.7); the log entry
    /// and progress of every firing, the outer net's and the inner nets';
    /// and the inner nets' records and versions, and which of them had
    /// events. In the wire's order, so that the log is the one a listener's
    /// would be. No fireable set is compared:
    /// [`set_listening`](Owner::set_listening) takes them afresh. A hosted
    /// net has no projection (`set_filter` is not served there), so there
    /// is no view to reconcile either.
    fn note_events(&mut self, events: &[Event], now: SystemTime) {
        let host_flights = &mut self.host_flights;
        let marking_version = &mut self.marking_version;
        let loaded = self.loaded.as_mut().expect(HOSTED);
        let mut inner_launched = HashMap::new();
        let mut inner_firings = Vec::new();
        for event in wire_order(events) {
            if is_lifted(event) {
                loaded.note_lifted(
                    event,
                    marking_version,
                    now,
                    &mut inner_launched,
                    &mut inner_firings,
                );
                continue;
            }
            if is_state_event(event) {
                *marking_version += 1;
                loaded.cross(None, &changed_places(event), marking_version);
            }
            loaded.record_host_firing(event, now, host_flights);
        }
        reconcile(host_flights, &loaded.net);
    }
}

/// Forgets every staged firing among `flights` whose transition `net` no
/// longer has in flight: its landing never reached the owner as events,
/// because the program drained the net's events itself inside a change
/// (since 2026-11-28; until then such a firing was kept, and with it the
/// fireable sets went uncompared and the records of dead inner nets
/// unpruned for as long as the net lived). A firing whose `Firing` or
/// `Completed` the program dropped stays: the net keeps that transition in
/// flight for good.
pub(super) fn reconcile(flights: &mut Vec<HostFlight>, net: &Net) {
    flights.retain(|flight| net.is_in_flight(flight.transition));
}

impl Loaded {
    /// The log entry and the progress of one firing the host program made
    /// on a hosted net, read off the event that told of it: `Fired` for a
    /// firing in one piece, `Launched` and then `Distributed` or `Failed`
    /// for a staged one, whose flight is kept in `flights` between the two.
    /// Any other event is not a firing and does nothing.
    pub(super) fn record_host_firing(
        &mut self,
        event: &Event,
        now: SystemTime,
        flights: &mut Vec<HostFlight>,
    ) {
        let mut landed = |transition: TransitionId| {
            let at = flights.iter().position(|f| f.transition == transition)?;
            Some(flights.remove(at))
        };
        let (transition, consumed, produced, started_at, failure) = match event {
            Event::Fired {
                transition,
                consumed,
                produced,
            } => (*transition, consumed.clone(), produced.clone(), now, None),
            Event::Launched {
                transition,
                consumed,
            } => {
                flights.push(HostFlight {
                    transition: *transition,
                    consumed: consumed.clone(),
                    started_at: now,
                });
                return;
            }
            Event::Distributed {
                transition,
                produced,
            } => {
                let (consumed, started_at) =
                    landed(*transition).map_or((Vec::new(), now), |f| (f.consumed, f.started_at));
                (*transition, consumed, produced.clone(), started_at, None)
            }
            Event::Failed {
                transition,
                consumed,
                restored,
                error,
            } => {
                let started_at = landed(*transition).map_or(now, |f| f.started_at);
                (
                    *transition,
                    consumed.clone(),
                    Vec::new(),
                    started_at,
                    Some((error.clone(), *restored)),
                )
            }
            _ => return,
        };
        let name = self.names.transition(transition);
        self.log_seq += 1;
        let entry = LogEntry {
            seq: self.log_seq,
            transition: name.clone(),
            started_at: iso8601(started_at),
            duration_ms: duration_ms(started_at, now),
            outcome: if failure.is_some() { "failed" } else { "fired" }.to_owned(),
            consumed: self.names.pairs(&consumed),
            produced: self.names.pairs(&produced),
            error: failure.as_ref().map(|(error, _)| error.clone()),
            restored: failure.as_ref().map(|&(_, restored)| restored),
            path: None,
            nested_id: None,
        };
        self.push_log(entry, Vec::new());
        if failure.is_none() {
            self.progress.fired(&name, now);
        }
    }

    /// Takes the fireable sets afresh from the marking as it is now, the
    /// outer net's and that of every inner net with a record, and tells no
    /// one: what a hosted net does when someone starts listening, so that
    /// what it remembers is what that reader's `get_state` reports. An inner
    /// net whose token a body holds cannot be read: one that had events
    /// stays touched, and is compared once its token is back.
    fn refresh_fireable(&mut self) {
        self.last_fireable = fireable_names(&self.net, &self.names);
        let records = &mut self.inner;
        let mut refreshed = HashSet::new();
        walk_all(&self.net, &mut |_, way, _, net| {
            if let Some(record) = records.get_mut(&net.id()) {
                record.see(net);
                record.last_path = path_of(way);
                record.last_fireable = fireable_names(net, &record.names);
                refreshed.insert(net.id());
            }
            None::<()>
        });
        self.touched.retain(|id| !refreshed.contains(id));
    }
}
