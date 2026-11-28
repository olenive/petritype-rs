//! Events lifted out of inner nets, each told as one `nested` line around
//! the inner net's own notification (PROTOCOL.md §12.4).

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use petritype::{Event, Net, NetId, PlaceId, TokenId};

use crate::protocol::{LogEntry, Notification, PathStep};
use crate::time::iso8601;
use crate::translate;

use super::address::Pairs;
use super::events::{Telling, changed_places, is_state_event};
use super::{Around, Inner, Loaded, millis};

/// Whether an event was lifted out of an inner net: out of a token's net
/// (`Event::Nested`) or out of a subnet (`Event::Subnet`).
pub(super) fn is_lifted(event: &Event) -> bool {
    matches!(event, Event::Nested { .. } | Event::Subnet { .. })
}

/// One level an event was lifted through.
#[derive(Debug, Clone, Copy)]
pub(super) struct LiftLevel<'e> {
    /// The place in the net above: a nested place, or a subnet place.
    pub(super) place: PlaceId,
    /// The token in it as it was when the event was lifted; `None` for a
    /// subnet place, which holds a net and no tokens.
    pub(super) token: Option<TokenId>,
    /// The net's name in the token; `""` for a subnet.
    net: &'e str,
    /// Its nested id.
    pub(super) id: NetId,
}

/// An event lifted out of an inner net, unwrapped down to the net it
/// happened in: one wrapper on the wire whatever the depth (§12.4).
pub(super) struct Lifted<'e> {
    /// Per level, outer to inner.
    pub(super) levels: Vec<LiftLevel<'e>>,
    /// What the innermost net logged.
    pub(super) event: &'e Event,
}

impl<'e> Lifted<'e> {
    /// `None` for an event of the outer net itself.
    pub(super) fn of(mut event: &'e Event) -> Option<Lifted<'e>> {
        let mut levels = Vec::new();
        loop {
            match event {
                Event::Nested {
                    place,
                    token,
                    net,
                    nested_id,
                    event: inner,
                } => {
                    levels.push(LiftLevel {
                        place: *place,
                        token: Some(*token),
                        net: net.as_str(),
                        id: *nested_id,
                    });
                    event = inner;
                }
                Event::Subnet {
                    place,
                    nested_id,
                    event: inner,
                } => {
                    levels.push(LiftLevel {
                        place: *place,
                        token: None,
                        net: "",
                        id: *nested_id,
                    });
                    event = inner;
                }
                _ => break,
            }
        }
        (!levels.is_empty()).then_some(Lifted { levels, event })
    }

    /// The nested id of the net the event happened in.
    pub(super) fn nested_id(&self) -> NetId {
        self.levels.last().expect("a lifted event has a level").id
    }

    pub(super) fn chain(&self) -> Vec<NetId> {
        self.levels.iter().map(|level| level.id).collect()
    }

    /// The net it happened in, if its token is where the event says, at
    /// every level: not while a body holds it, nor once it is gone.
    pub(super) fn live<'a>(&self, outer: &'a Net) -> Option<&'a Net> {
        self.levels.iter().try_fold(outer, |here, level| {
            let found = match level.token {
                Some(token) => here
                    .nested(level.place, token)?
                    .into_iter()
                    .find(|(_, inner)| inner.id() == level.id)
                    .map(|(_, inner)| inner),
                None => here.subnet(level.place),
            };
            found.filter(|inner| inner.id() == level.id)
        })
    }
}

/// A firing inside an inner net, as a flush reported it: what a `step`
/// that made it answers with.
#[derive(Debug, Clone)]
pub(super) struct InnerFiring {
    pub(super) transition: String,
    pub(super) path: Vec<PathStep>,
    pub(super) nested_id: String,
}

impl Loaded {
    /// The wire path of a lifted event: one step per level, the place's name
    /// in the net above, and for a token's net the token and the net's name
    /// in the token. Makes a record for each net on the way that has none.
    fn lifted_path(&mut self, lifted: &Lifted<'_>) -> Vec<PathStep> {
        let mut path = Vec::with_capacity(lifted.levels.len());
        for (depth, level) in lifted.levels.iter().enumerate() {
            let place = level.place;
            let place_name = match depth {
                0 => self.names.place(place),
                _ => {
                    let above = lifted.levels[depth - 1].id;
                    self.inner
                        .get(&above)
                        .map_or_else(|| place.to_string(), |inner| inner.names.place(place))
                }
            };
            path.push(match level.token {
                Some(token) => translate::step(&place_name, token, level.net),
                None => translate::subnet_step(&place_name),
            });
            let live = Lifted {
                levels: lifted.levels[..=depth].to_vec(),
                event: lifted.event,
            }
            .live(&self.net);
            match self.inner.get_mut(&level.id) {
                Some(inner) => {
                    if let Some(net) = live {
                        inner.see(net);
                    }
                    inner.last_path.clone_from(&path);
                }
                None => {
                    let around = level.token.is_none().then(|| match depth {
                        0 => Around::Loaded,
                        _ => Around::Inner(lifted.levels[depth - 1].id),
                    });
                    self.inner
                        .insert(level.id, Inner::new(live, path.clone(), false, around));
                }
            }
        }
        path
    }

    /// One event lifted out of an inner net as the `nested` line the wire
    /// carries (PROTOCOL.md §12.4): the notification the inner net would
    /// send on its own, with its own names and version, inside one wrapper
    /// with the full path and the loaded net's version. An inner firing also
    /// gets its log entry and its net's progress here. `took` is how long
    /// the firing this event ends took, when the owner timed it (a deep
    /// step's own firing, [`Timed`](super::firing::Timed)); a firing it did
    /// not time, one a body made inside the tokens it holds or one of the
    /// host program's, carries 0.
    ///
    /// A change of a port place (§12.7) moves the versions on its far side
    /// too ([`cross`](Loaded::cross)), which the inner notification
    /// carries; when the loaded net is the net around the subnet that told
    /// it, `loaded_version` moves, and the wrapper carries its new value.
    pub(super) fn lift(
        &mut self,
        event: &Event,
        loaded_version: &mut u64,
        now: SystemTime,
        took: Option<Duration>,
        launched: &mut HashMap<NetId, Pairs>,
        firings: &mut Vec<InnerFiring>,
    ) -> Notification {
        let lifted = Lifted::of(event).expect("lift is given a lifted event");
        let path = self.lifted_path(&lifted);
        let nested_id = lifted.nested_id();
        let running = self.running;
        self.touch(nested_id);
        // The inner net's own projection, if `set_filter` named it, and its
        // subnets' for its windows: its events are told through them,
        // whatever the outer net's is.
        let mut lens = self.lens(Some(nested_id));
        let live = lifted.live(&self.net);
        let inner = self
            .inner
            .get(&nested_id)
            .expect("lifted_path made the record");
        let version = inner.marking_version + 1;
        let telling = Telling {
            names: &inner.names,
            net: live,
            generation: 1,
            running,
            flight_ms: 0.0,
            param: None,
        };
        let mut notification = telling.line(&mut lens, lifted.event, inner.marking_version, took);
        self.restore(Some(nested_id), lens);
        // The quiet path (`note_lifted`) raises versions by this rule
        // without building the notification: the two must agree.
        debug_assert_eq!(notification.is_state(), is_state_event(lifted.event));
        if notification.is_state() {
            self.inner
                .get_mut(&nested_id)
                .expect("lifted_path made the record")
                .marking_version = version;
            let (subnets, outer) = self.cross(
                Some(nested_id),
                &changed_places(lifted.event),
                loaded_version,
            );
            notification.across(subnets, outer);
        }
        self.log_lifted(&lifted, &path, now, took, launched, firings);
        Notification::Nested {
            path,
            nested_id: nested_id.to_string(),
            event: Box::new(notification),
            marking_version: *loaded_version,
            running,
        }
    }

    /// What one event lifted out of an inner net changes when nobody is
    /// listening to a hosted net: what [`lift`](Loaded::lift) keeps (the
    /// net's record, its version and the versions across its ports, its log
    /// entry and its progress), with no notification built.
    pub(super) fn note_lifted(
        &mut self,
        event: &Event,
        loaded_version: &mut u64,
        now: SystemTime,
        launched: &mut HashMap<NetId, Pairs>,
        firings: &mut Vec<InnerFiring>,
    ) {
        let lifted = Lifted::of(event).expect("note_lifted is given a lifted event");
        let path = self.lifted_path(&lifted);
        let nested_id = lifted.nested_id();
        self.touch(nested_id);
        if is_state_event(lifted.event) {
            self.inner
                .get_mut(&nested_id)
                .expect("lifted_path made the record")
                .marking_version += 1;
            self.cross(
                Some(nested_id),
                &changed_places(lifted.event),
                loaded_version,
            );
        }
        // The host program fired it, and the owner timed nothing.
        self.log_lifted(&lifted, &path, now, None, launched, firings);
    }

    /// The log entry and the progress of an inner firing, when the lifted
    /// event ends one (`Fired`, `Distributed` or `Failed`): the entry, a
    /// count in its net's progress unless it failed, and a place in
    /// `firings`. A `Launched` is kept in `launched` until the `Distributed`
    /// that ends it, for what it took. Any other event does nothing. The
    /// entry starts at `now` and lasts `took`, or 0 for a firing the owner
    /// did not time.
    fn log_lifted(
        &mut self,
        lifted: &Lifted<'_>,
        path: &[PathStep],
        now: SystemTime,
        took: Option<Duration>,
        launched: &mut HashMap<NetId, Pairs>,
        firings: &mut Vec<InnerFiring>,
    ) {
        let nested_id = lifted.nested_id();
        let (transition, consumed, made, failure) = match lifted.event {
            Event::Fired {
                transition,
                consumed,
                produced,
            } => (*transition, consumed.clone(), produced.clone(), None),
            Event::Launched { consumed, .. } => {
                launched.insert(nested_id, consumed.clone());
                return;
            }
            Event::Distributed {
                transition,
                produced,
            } => (
                *transition,
                launched.remove(&nested_id).unwrap_or_default(),
                produced.clone(),
                None,
            ),
            Event::Failed {
                transition,
                consumed,
                restored,
                error,
            } => (
                *transition,
                consumed.clone(),
                Vec::new(),
                Some((error.clone(), *restored)),
            ),
            _ => return,
        };
        let inner = self
            .inner
            .get_mut(&nested_id)
            .expect("lifted_path made the record");
        let transition = inner.names.transition(transition);
        if failure.is_none() {
            inner.progress.fired(&transition, now);
        }
        self.log_seq += 1;
        let entry = LogEntry {
            seq: self.log_seq,
            transition: transition.clone(),
            started_at: iso8601(now),
            duration_ms: took.map_or(0.0, millis),
            outcome: if failure.is_some() { "failed" } else { "fired" }.to_owned(),
            consumed: inner.names.pairs(&consumed),
            produced: inner.names.pairs(&made),
            error: failure.as_ref().map(|(error, _)| error.clone()),
            restored: failure.as_ref().map(|&(_, restored)| restored),
            path: Some(path.to_vec()),
            nested_id: Some(nested_id.to_string()),
        };
        self.push_log(entry, lifted.chain());
        firings.push(InnerFiring {
            transition,
            path: path.to_vec(),
            nested_id: nested_id.to_string(),
        });
    }
}
