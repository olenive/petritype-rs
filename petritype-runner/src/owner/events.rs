//! The events a net logs, told as the notifications of the wire, in its
//! order, through the connection's projections, with the versions they
//! raise.

use std::collections::{BTreeMap, HashMap};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::{Duration, SystemTime};

use petritype::{Event, Net, NetId, PlaceId, TokenId, TransitionId};

use crate::projection::{Lens, Seen, SubnetLens};
use crate::protocol::{self, HiddenKind, Notification};
use crate::translate::{self, Names};

use super::address::{Found, Pairs, find, locate};
use super::firing::Timed;
use super::hosted::reconcile;
use super::lifted::{InnerFiring, Lifted};
use super::{Around, InFlight, Inner, Loaded, Outgoing, Owner, millis, push};

/// The places a state notification made of `event` changed, as its own net
/// names them (PROTOCOL.md §12.7): what a firing took or put, what a failed
/// one put back when it restored, and the place of a token that was added,
/// removed or mutated. What happened counts, not what a connection's filter
/// lets the notification list, and a non-consuming edge, or a drain of an
/// empty place, takes nothing.
pub(super) fn changed_places(event: &Event) -> Vec<PlaceId> {
    let places = |pairs: &[(PlaceId, TokenId)]| pairs.iter().map(|&(place, _)| place).collect();
    match event {
        Event::Fired {
            consumed, produced, ..
        } => [places(consumed), places(produced)].concat(),
        Event::Launched { consumed, .. } => places(consumed),
        Event::Distributed { produced, .. } => places(produced),
        Event::Failed {
            consumed,
            restored: true,
            ..
        } => places(consumed),
        Event::Added { place, .. }
        | Event::Removed { place, .. }
        | Event::Mutated { place, .. } => {
            vec![*place]
        }
        _ => Vec::new(),
    }
}

/// What one flush wrote: the events, for the caller to read ids off, and
/// the inner firings among them.
pub(super) struct Flushed {
    pub(super) events: Vec<Event>,
    pub(super) inner_firings: Vec<InnerFiring>,
}

/// The order the wire tells a batch of events in (PROTOCOL.md §12.4): what a
/// firing's body did inside the tokens it holds goes out after its
/// `launched` and before its own `mutated`s and its `distributed` (or
/// `fired`, or `failed`).
///
/// The core lifts a mutated token's inner events just before that token's
/// `Mutated` (DESIGN §6.6, rule 2) and a produced token's just after the
/// firing's `Distributed` or `Fired`, under its new place and id (rule 4).
/// Here every `Nested` of the landing comes first (those lifted for mutated
/// tokens, then those for produced ones, each in the order logged), then the
/// `Mutated`s, then the event that ends the firing. The core's own log
/// keeps its rules; only the wire is reordered.
pub(super) fn wire_order(events: &[Event]) -> Vec<&Event> {
    let mut ordered: Vec<&Event> = Vec::with_capacity(events.len());
    let mut index = 0;
    while index < events.len() {
        let event = &events[index];
        index += 1;
        let produced: &[(PlaceId, TokenId)] = match event {
            Event::Fired { produced, .. } | Event::Distributed { produced, .. } => produced,
            Event::Failed { .. } => &[],
            _ => {
                ordered.push(event);
                continue;
            }
        };
        // The landing so far: the run of `Nested` and `Mutated` written
        // since the firing's `Launched` (or since the batch began).
        let start = ordered
            .iter()
            .rposition(|e| !matches!(e, Event::Nested { .. } | Event::Mutated { .. }))
            .map_or(0, |at| at + 1);
        let landing: Vec<&Event> = ordered.drain(start..).collect();
        let mut arrivals = Vec::new();
        while let Some(next @ Event::Nested { place, token, .. }) = events.get(index)
            && produced.contains(&(*place, *token))
        {
            arrivals.push(next);
            index += 1;
        }
        ordered.extend(
            landing
                .iter()
                .copied()
                .filter(|e| matches!(e, Event::Nested { .. })),
        );
        ordered.extend(arrivals);
        ordered.extend(
            landing
                .iter()
                .copied()
                .filter(|e| matches!(e, Event::Mutated { .. })),
        );
        ordered.push(event);
    }
    ordered
}

/// Whether a net's own event is told as a state notification, one that
/// raises that net's marking version: a firing's, a token's, a `toggled` or a
/// `param_changed`. A resync, and an event this runner does not know, are
/// told as a `reloaded`, which raises nothing. A `Nested` event raises the
/// version of the net it happened in, which this does not answer for.
pub(super) fn is_state_event(event: &Event) -> bool {
    matches!(
        event,
        Event::Fired { .. }
            | Event::Launched { .. }
            | Event::Distributed { .. }
            | Event::Failed { .. }
            | Event::Added { .. }
            | Event::Removed { .. }
            | Event::Mutated { .. }
            | Event::Toggled { .. }
            | Event::ParamChanged { .. }
    )
}

/// A net's fireable set by name, in declaration order; see [`fireable`].
pub(super) fn fireable_names(net: &Net, names: &Names) -> Vec<String> {
    fireable(net)
        .into_iter()
        .map(|t| names.transition(t))
        .collect()
}

/// A net's fireable transitions, in declaration order. A guard that
/// panics leaves its transition out, as a guard that says no would, and
/// the runner goes on (since 2026-11-28; until then the panic ended the
/// process), so that a guard which panics is met as a refused step, never
/// as a lost runner.
pub(super) fn fireable(net: &Net) -> Vec<TransitionId> {
    catch_unwind(AssertUnwindSafe(|| net.fireable())).unwrap_or_else(|_| {
        net.transitions()
            .map(|(transition, _)| transition)
            .filter(|&transition| is_fireable(net, transition))
            .collect()
    })
}

/// Whether one transition is fireable, a guard that panics saying no; see
/// [`fireable`].
pub(super) fn is_fireable(net: &Net, transition: TransitionId) -> bool {
    catch_unwind(AssertUnwindSafe(|| net.is_fireable(transition))).unwrap_or(false)
}

/// `produced` entries, labelled from `net` when it can be read, with their
/// keys where their place declares some, and with `""` and no keys when it
/// cannot (an inner net whose token a body holds).
fn produced(
    names: &Names,
    net: Option<&Net>,
    tokens: &[(PlaceId, TokenId)],
) -> Vec<protocol::Produced> {
    match net {
        Some(net) => names.produced(net, tokens),
        None => tokens
            .iter()
            .map(|&(place, token)| protocol::Produced {
                place: names.place_name(place),
                id: token.to_string(),
                label: String::new(),
                color: None,
                keys: None,
            })
            .collect(),
    }
}

/// A token event (`Added`, `Removed` or `Mutated`) as the notification the
/// connection is told (PROTOCOL.md §7.1, §13.4), through the projections
/// `lens` holds: a token the filter hides is told of as `hidden`, and a
/// mutation that carries a token across the filter's boundary as an `added`
/// or a `removed`, in place of its `mutated`. A port of one of the net's
/// subnet places is named by the pair and projected by its subnet's filter
/// (§12.7). `net` is the net it happened in, `None` when it cannot be read.
/// `None` for any other event.
fn token_line(
    names: &Names,
    net: Option<&Net>,
    lens: &mut Lens,
    event: &Event,
    marking_version: u64,
    running: bool,
) -> Option<Notification> {
    let keys = |place, token| net.and_then(|net| translate::keys(net, place, token));
    let added = |place, token| added_line(names, net, place, token, marking_version, running);
    let removed = |place, token| removed_line(names, place, token, marking_version, running);
    let hidden = |kind, place| {
        let (place, port) = names.split(place);
        Notification::Hidden {
            kind,
            place,
            port,
            subnet_marking_versions: None,
            outer_marking_version: None,
            marking_version,
            running,
        }
    };
    Some(match *event {
        Event::Added { place, token } => {
            if lens.arrived(net, place, token) {
                added(place, token)
            } else {
                hidden(HiddenKind::Added, place)
            }
        }
        Event::Removed { place, token } => {
            if lens.left(place, token) {
                removed(place, token)
            } else {
                hidden(HiddenKind::Removed, place)
            }
        }
        Event::Mutated { place, token } => match lens.changed(net, place, token) {
            Seen::Mutated => {
                let (where_, port) = names.split(place);
                Notification::Mutated {
                    place: where_,
                    port,
                    id: token.to_string(),
                    label: net
                        .and_then(|net| net.debug(place, token))
                        .map(translate::cut),
                    keys: keys(place, token),
                    subnet_marking_versions: None,
                    outer_marking_version: None,
                    marking_version,
                    running,
                }
            }
            Seen::Added => added(place, token),
            Seen::Removed => removed(place, token),
            Seen::Hidden => hidden(HiddenKind::Mutated, place),
        },
        _ => return None,
    })
}

/// The lines that tell a connection which tokens went out of its view and
/// which came into it although none of them moved (PROTOCOL.md §13.4): a
/// `removed` for each of `gone`, then an `added` for each of `came`, each a
/// state notification stamped from `version` upward, beside the place it
/// changed.
fn view_lines(
    names: &Names,
    net: &Net,
    gone: &[(PlaceId, TokenId)],
    came: &[(PlaceId, TokenId)],
    version: &mut u64,
    running: bool,
) -> Vec<(PlaceId, Notification)> {
    let mut lines = Vec::with_capacity(gone.len() + came.len());
    for &(place, token) in gone {
        *version += 1;
        lines.push((place, removed_line(names, place, token, *version, running)));
    }
    for &(place, token) in came {
        *version += 1;
        let line = added_line(names, Some(net), place, token, *version, running);
        lines.push((place, line));
    }
    lines
}

/// An `added` of `token` in `place`, labelled, coloured and keyed from
/// `net` when it can be read, and with `""` and no colour or keys when it
/// cannot. A port of one of the net's subnet places is named by the pair
/// (§12.7).
fn added_line(
    names: &Names,
    net: Option<&Net>,
    place: PlaceId,
    token: TokenId,
    marking_version: u64,
    running: bool,
) -> Notification {
    let (where_, port) = names.split(place);
    Notification::Added {
        place: where_,
        port,
        id: token.to_string(),
        label: net.map_or_else(String::new, |net| translate::label(net, place, token)),
        color: net.and_then(|net| translate::color(net, place, token)),
        keys: net.and_then(|net| translate::keys(net, place, token)),
        subnet_marking_versions: None,
        outer_marking_version: None,
        marking_version,
        running,
    }
}

/// A `removed` of `token` from `place`, a port named by the pair (§12.7).
fn removed_line(
    names: &Names,
    place: PlaceId,
    token: TokenId,
    marking_version: u64,
    running: bool,
) -> Notification {
    let (place, port) = names.split(place);
    Notification::Removed {
        place,
        port,
        id: token.to_string(),
        subnet_marking_versions: None,
        outer_marking_version: None,
        marking_version,
        running,
    }
}

/// The tokens one firing event names, as the connection is told of them:
/// what `consumed` it took (moved out of view, kept for a restore) and what
/// `produced` it placed, through the projections `lens` holds, every one of
/// them where none covers its place. `landed` says the firing ended with
/// this event.
fn firing_tokens(
    lens: &mut Lens,
    net: Option<&Net>,
    consumed: &[(PlaceId, TokenId)],
    produced: &[(PlaceId, TokenId)],
    landed: bool,
) -> (Pairs, Pairs) {
    let consumed = lens.took(consumed);
    if landed {
        lens.landed();
    }
    (consumed, lens.placed(net, produced))
}

/// How one net's own events are told: what [`Telling::line`] needs besides
/// the event, the same for every event of a batch in that net.
pub(super) struct Telling<'a> {
    /// The net's names.
    pub(super) names: &'a Names,
    /// The net the events happened in; `None` when it cannot be read, an
    /// inner net whose token a body holds.
    pub(super) net: Option<&'a Net>,
    /// Its generation, which a `reloaded` carries: 1 for an inner net,
    /// which is in its first generation for as long as it exists.
    pub(super) generation: u64,
    /// Whether the loaded net runs.
    pub(super) running: bool,
    /// The `duration_ms` of a firing that the `took` of [`line`](Self::line)
    /// does not time: the landing flight's in the loaded net, or 0 when no
    /// firing lands there, and 0 in an inner net.
    pub(super) flight_ms: f64,
    /// The name the `set_param` being served used, if one is.
    pub(super) param: Option<&'a str>,
}

impl Telling<'_> {
    /// The notification an event of the net's own is told as (PROTOCOL.md
    /// §7): a state notification carrying `at + 1`, `at` being the net's
    /// version before it, or a `reloaded` carrying `at`. The tokens a firing
    /// or a token event names go through the projections `lens` holds.
    /// `took` is how long the firing this event ends took, when the owner
    /// timed it. An event lifted out of an inner net is that net's own, and
    /// is told inside a `nested` line (§12.4).
    pub(super) fn line(
        &self,
        lens: &mut Lens,
        event: &Event,
        at: u64,
        took: Option<Duration>,
    ) -> Notification {
        let (names, net, running) = (self.names, self.net, self.running);
        let version = at + 1;
        match event {
            // A firing in one piece. An inner firing is inline in the core
            // (`step_deep`), so it is one piece on the wire too.
            Event::Fired {
                transition,
                consumed,
                produced: made,
            } => {
                let (consumed, made) = firing_tokens(lens, net, consumed, made, true);
                Notification::Fired {
                    transition: names.transition(*transition),
                    consumed: names.pairs(&consumed),
                    produced: produced(names, net, &made),
                    duration_ms: took.map_or(self.flight_ms, millis),
                    subnet_marking_versions: None,
                    outer_marking_version: None,
                    marking_version: version,
                    running,
                }
            }
            Event::Launched {
                transition,
                consumed,
            } => {
                let (consumed, _) = firing_tokens(lens, net, consumed, &[], false);
                Notification::Launched {
                    transition: names.transition(*transition),
                    consumed: names.pairs(&consumed),
                    subnet_marking_versions: None,
                    outer_marking_version: None,
                    marking_version: version,
                    running,
                }
            }
            // The landing of a staged firing. One inside an inner net was
            // made in stages by a body or the host program: the owner ran
            // neither stage, and timed nothing.
            Event::Distributed {
                transition,
                produced: made,
            } => {
                let (_, made) = firing_tokens(lens, net, &[], made, true);
                Notification::Distributed {
                    transition: names.transition(*transition),
                    produced: produced(names, net, &made),
                    duration_ms: self.flight_ms,
                    subnet_marking_versions: None,
                    outer_marking_version: None,
                    marking_version: version,
                    running,
                }
            }
            Event::Failed {
                transition,
                consumed,
                restored,
                error,
            } => {
                let told = lens.failed(consumed, *restored);
                Notification::Failed {
                    transition: names.transition(*transition),
                    error: error.clone(),
                    restored: *restored,
                    consumed: names.pairs(&told),
                    subnet_marking_versions: None,
                    outer_marking_version: None,
                    marking_version: version,
                    running,
                }
            }
            // Under rule 3 (lifted at launch, the token in the body's hands)
            // an inner net has no net to label from, nor to read keys from.
            event @ (Event::Added { .. } | Event::Removed { .. } | Event::Mutated { .. }) => {
                token_line(names, net, lens, event, version, running).expect("a token event")
            }
            Event::Toggled {
                transition,
                enabled,
            } => Notification::Toggled {
                transition: names.transition(*transition),
                enabled: *enabled,
                marking_version: version,
                running,
            },
            // The name the `set_param` being served used; for a change
            // nobody asked for over the wire, the wire name the net
            // registered, or the type name if it registered none. A body
            // that set an inner parameter is told of by the name from the
            // inner net, which only the outer net's own `set_param` names.
            Event::ParamChanged { type_name } => Notification::ParamChanged {
                param: self
                    .param
                    .or_else(|| net.and_then(|net| net.param_name(type_name)))
                    .unwrap_or(type_name)
                    .to_owned(),
                marking_version: version,
                running,
            },
            // An `import_marking`: the marking was replaced wholesale and is
            // not told as events (DESIGN §8.1), so a reader refetches. Inside
            // an inner net, a body loading a saved cabin, say.
            Event::Resynced => Notification::Reloaded {
                generation: self.generation,
                reason: "resync".into(),
                marking_version: at,
                running,
            },
            // An event this runner does not know: say the marking moved in a
            // way it cannot describe.
            _ => Notification::Reloaded {
                generation: self.generation,
                reason: "resync".into(),
                marking_version: at,
                running,
            },
        }
    }
}

impl Loaded {
    /// The projections one event of the net `of` (the loaded net for
    /// `None`) goes through, taken out of their records: the net's own, and
    /// each of its subnets', for the places it has for their ports
    /// (PROTOCOL.md §12.7). [`restore`](Loaded::restore) puts them back.
    pub(super) fn lens(&mut self, of: Option<NetId>) -> Lens {
        let (own, seats) = match of {
            None => (self.projection.take(), self.seats.clone()),
            Some(id) => self.inner.get_mut(&id).map_or((None, Vec::new()), |inner| {
                (inner.projection.take(), inner.seats.clone())
            }),
        };
        let subnets = seats
            .into_iter()
            .map(|seat| SubnetLens {
                place: seat.place,
                id: seat.id,
                projection: self
                    .inner
                    .get_mut(&seat.id)
                    .and_then(|inner| inner.projection.take()),
                windows: seat.windows,
            })
            .collect();
        Lens { own, subnets }
    }

    /// Puts back the projections [`lens`](Loaded::lens) took.
    pub(super) fn restore(&mut self, of: Option<NetId>, lens: Lens) {
        match of {
            None => self.projection = lens.own,
            Some(id) => {
                if let Some(inner) = self.inner.get_mut(&id) {
                    inner.projection = lens.own;
                }
            }
        }
        for subnet in lens.subnets {
            if let Some(inner) = self.inner.get_mut(&subnet.id) {
                inner.projection = subnet.projection;
            }
        }
    }

    /// Raises the versions on the far side of the port places a state
    /// notification of the net `of` (the loaded net for `None`) changed,
    /// `changed` being the places it changed as that net names them, and
    /// says what they became (PROTOCOL.md §12.7): for each subnet place one
    /// of whose windows changed, its subnet's new version, by the subnet
    /// place's name, the notification's `subnet_marking_versions`; and when
    /// `of` is a subnet and one of its own ports changed, the new version of
    /// the net around it, its `outer_marking_version`, which for the loaded
    /// net is `loaded_version`. Each net moves once, however many of its
    /// ports changed, and is touched, so that its fireable set is compared.
    /// The caller has raised `of`'s own version.
    pub(super) fn cross(
        &mut self,
        of: Option<NetId>,
        changed: &[PlaceId],
        loaded_version: &mut u64,
    ) -> (Option<BTreeMap<String, u64>>, Option<u64>) {
        if changed.is_empty() {
            return (None, None);
        }
        let (moved, around): (Vec<(String, NetId)>, Option<Around>) = {
            let (seats, names, around, topology) = match of {
                None => (&self.seats, &self.names, None, None),
                Some(id) => match self.inner.get(&id) {
                    Some(inner) => (
                        &inner.seats,
                        &inner.names,
                        inner.around,
                        inner.topology.as_ref(),
                    ),
                    None => return (None, None),
                },
            };
            let moved = seats
                .iter()
                .filter(|seat| seat.windows.iter().any(|(w, _)| changed.contains(w)))
                .map(|seat| (names.place(seat.place), seat.id))
                .collect();
            let own_port = topology.is_some_and(|topology| {
                topology
                    .places
                    .iter()
                    .any(|place| place.port && changed.contains(&place.id))
            });
            (moved, around.filter(|_| own_port))
        };
        let mut subnets = BTreeMap::new();
        for (name, id) in moved {
            let here = of.map_or(Around::Loaded, Around::Inner);
            let inner = self
                .inner
                .entry(id)
                .or_insert_with(|| Inner::new(None, Vec::new(), true, Some(here)));
            inner.marking_version += 1;
            subnets.insert(name, inner.marking_version);
            self.touch(id);
        }
        let outer = match around {
            None => None,
            Some(Around::Loaded) => {
                *loaded_version += 1;
                Some(*loaded_version)
            }
            Some(Around::Inner(id)) => {
                let version = self.inner.get_mut(&id).map(|inner| {
                    inner.marking_version += 1;
                    inner.marking_version
                });
                if version.is_some() {
                    self.touch(id);
                }
                version
            }
        };
        ((!subnets.is_empty()).then_some(subnets), outer)
    }

    /// Notes that the inner net `id` had an event, so that its fireable set
    /// is compared at the next comparison.
    pub(super) fn touch(&mut self, id: NetId) {
        if !self.touched.contains(&id) {
            self.touched.push(id);
        }
    }

    /// The `added`s and `removed`s of the tokens that came into a
    /// projection's view, or went out of it, with no token event of their
    /// own (PROTOCOL.md §13.4), after a batch's events and before its
    /// `fireable`s, as the Python runner tells them: after a change of a net
    /// parameter a `{key, param}` clause names, every token is compared;
    /// after inner events, the tokens they were lifted through, whose keys
    /// may read the nets they hold. The outer net's first, then each inner
    /// net's, `nested`; each line a state notification of its own net.
    fn reproject(
        &mut self,
        outer_version: &mut u64,
        params_changed: &[Option<NetId>],
        lifted_through: &[(Option<NetId>, PlaceId, TokenId)],
        out: &mut Vec<Outgoing>,
    ) {
        if params_changed.is_empty() && lifted_through.is_empty() {
            return;
        }
        let running = self.running;
        let mut nets: Vec<Option<NetId>> = vec![None];
        for which in params_changed
            .iter()
            .copied()
            .chain(lifted_through.iter().map(|&(which, ..)| which))
        {
            if !nets.contains(&which) {
                nets.push(which);
            }
        }
        for which in nets {
            let only: Pairs = lifted_through
                .iter()
                .filter(|&&(net, ..)| net == which)
                .map(|&(_, place, token)| (place, token))
                .collect();
            let asks_all = params_changed.contains(&which);
            match which {
                None => {
                    let Some(projection) = self.projection.as_mut() else {
                        continue;
                    };
                    let full = asks_all && projection.reads_params();
                    if !full && only.is_empty() {
                        continue;
                    }
                    let (gone, came) =
                        projection.reconcile(&self.net, (!full).then_some(only.as_slice()));
                    let lines =
                        view_lines(&self.names, &self.net, &gone, &came, outer_version, running);
                    for (place, mut line) in lines {
                        let (subnets, _) = self.cross(None, &[place], outer_version);
                        line.across(subnets, None);
                        push(out, line);
                    }
                }
                Some(id) => {
                    let Some(inner) = self.inner.get_mut(&id) else {
                        continue;
                    };
                    let Some(projection) = inner.projection.as_mut() else {
                        continue;
                    };
                    let full = asks_all && projection.reads_params();
                    if !full && only.is_empty() {
                        continue;
                    }
                    let Some((found, net)) = find(&self.net, |net| net.id() == id) else {
                        continue;
                    };
                    let (gone, came) =
                        projection.reconcile(net, (!full).then_some(only.as_slice()));
                    let lines = view_lines(
                        &inner.names,
                        net,
                        &gone,
                        &came,
                        &mut inner.marking_version,
                        running,
                    );
                    // A token of a subnet's port that comes into view or
                    // goes out of it is a change of the port (§12.7).
                    for (place, mut line) in lines {
                        let (subnets, outer) = self.cross(Some(id), &[place], outer_version);
                        line.across(subnets, outer);
                        push(
                            out,
                            Notification::Nested {
                                path: found.path.clone(),
                                nested_id: id.to_string(),
                                event: Box::new(line),
                                marking_version: *outer_version,
                                running,
                            },
                        );
                    }
                }
            }
        }
    }

    /// The `fireable` of each inner net that had events since it was last
    /// compared, or whose ports a change told by another net touched
    /// (§12.7), `nested`, when it differs from the last one sent: after the
    /// outer net's, and a net's before those of the nets inside it, so
    /// shallower nets first (PROTOCOL.md §12.4). Not a state notification,
    /// so no version moves. A net whose token is gone, or in a body's
    /// hands, is skipped.
    fn compare_inner_fireable(&mut self, outer_version: u64, out: &mut Vec<Outgoing>) {
        let running = self.running;
        let ids = std::mem::take(&mut self.touched);
        let (outer, records) = (&self.net, &self.inner);
        let mut touched: Vec<(NetId, Found, &Net)> = ids
            .into_iter()
            .filter_map(|id| {
                let last_path = records.get(&id).map(|record| &record.last_path[..]);
                let (found, net) = locate(outer, id, last_path)?;
                Some((id, found, net))
            })
            .collect();
        touched.sort_by_key(|(_, found, _)| found.path.len());
        for (id, found, net) in touched {
            let Some(inner) = self.inner.get_mut(&id) else {
                continue;
            };
            inner.see(net);
            inner.last_path.clone_from(&found.path);
            let fireable = fireable_names(net, &inner.names);
            if fireable == inner.last_fireable {
                continue;
            }
            inner.last_fireable.clone_from(&fireable);
            push(
                out,
                Notification::Nested {
                    path: found.path,
                    nested_id: id.to_string(),
                    event: Box::new(Notification::Fireable {
                        transitions: fireable,
                        marking_version: inner.marking_version,
                        running,
                    }),
                    marking_version: outer_version,
                    running,
                },
            );
        }
    }
}

impl Owner {
    /// [`flush_events`](Owner::flush_events) after an inject, which first
    /// takes the fireable sets of the nets inside the token it brought,
    /// silently, as the owner sees from then on those the marking held at
    /// `load` (§7.2).
    pub(super) fn flush_injected(&mut self, now: SystemTime, out: &mut Vec<Outgoing>) -> Flushed {
        let loaded = self.loaded_mut();
        let events: Vec<Event> = loaded.net.drain_events().collect();
        loaded.know_new_after(&events);
        self.tell_events(events, now, None, None, true, out)
    }

    /// Drains the net's log and writes one notification per event, in the
    /// wire's order ([`wire_order`]): each state notification of the outer
    /// net raising its `marking_version` by one, each inner event `nested`
    /// and raising its own net's. Then, if asked and anything happened, the
    /// outer `fireable` set if it changed, and each touched inner net's
    /// after it. Returns the events for the caller to read ids off, and the
    /// inner firings it reported.
    pub(super) fn flush_events(
        &mut self,
        now: SystemTime,
        flight: Option<&InFlight>,
        compare_fireable: bool,
        out: &mut Vec<Outgoing>,
    ) -> Flushed {
        let loaded = self.loaded.as_mut().expect("events need a net");
        let events: Vec<Event> = loaded.net.drain_events().collect();
        self.tell_events(events, now, flight, None, compare_fireable, out)
    }

    /// Writes one notification per event of `events`, a batch drained from
    /// the net's log, as [`flush_events`](Owner::flush_events) says.
    ///
    /// A firing's `duration_ms` is the landing `flight`'s, or `timed`'s for
    /// the one firing it names, a deep step's or a press's; any other
    /// firing in the batch was made by a body or by the host program, and
    /// carries 0, since the owner timed nothing of it.
    pub(super) fn tell_events(
        &mut self,
        events: Vec<Event>,
        now: SystemTime,
        flight: Option<&InFlight>,
        timed: Option<&Timed>,
        compare_fireable: bool,
        out: &mut Vec<Outgoing>,
    ) -> Flushed {
        let generation = self.generation;
        let hosted = self.hosted;
        let host_flights = &mut self.host_flights;
        let marking_version = &mut self.marking_version;
        let loaded = self.loaded.as_mut().expect("events need a net");
        let running = loaded.running;
        let inline = flight.is_some_and(|f| f.inline.is_some());
        let duration = flight.map_or(0.0, |f| f.duration_ms(now));
        let mut inline_consumed: Vec<(PlaceId, TokenId)> = Vec::new();
        // What an inline firing's `launched` took, as it happened, for the
        // port places its one `fired` changed (§12.7).
        let mut inline_taken: Vec<PlaceId> = Vec::new();
        let mut inner_launched = HashMap::new();
        let mut inner_firings = Vec::new();
        // What may bring tokens into a projection's view, or take them out
        // of it, with no token event (PROTOCOL.md §13.4): a parameter that
        // changed, in the outer net (`None`) or an inner one, and the tokens
        // inner events were lifted through, by the net that holds them.
        let mut params_changed: Vec<Option<NetId>> = Vec::new();
        let mut lifted_through: Vec<(Option<NetId>, PlaceId, TokenId)> = Vec::new();
        for event in wire_order(&events) {
            let took = timed.and_then(|timed| timed.of(event));
            if let Some(lifted) = Lifted::of(event) {
                let mut above = None;
                for level in &lifted.levels {
                    // A subnet place holds no token for a key to read.
                    if let Some(token) = level.token {
                        lifted_through.push((above, level.place, token));
                    }
                    above = Some(level.id);
                }
                if matches!(lifted.event, Event::ParamChanged { .. }) {
                    params_changed.push(Some(lifted.nested_id()));
                }
                let line = loaded.lift(
                    event,
                    marking_version,
                    now,
                    took,
                    &mut inner_launched,
                    &mut inner_firings,
                );
                push(out, line);
                continue;
            }
            // The connection's projection of the outer net, if it set one,
            // and of its subnets, for its windows: every token a line names
            // goes through the one that covers its place (§12.7, §13.4).
            let mut lens = loaded.lens(None);
            let names = &loaded.names;
            let net = &loaded.net;
            let version = *marking_version + 1;
            let mut changed = changed_places(event);
            if matches!(event, Event::ParamChanged { .. }) {
                params_changed.push(None);
            }
            let notification = match event {
                Event::Launched { consumed, .. } if inline => {
                    // Reported with the landing, as one `fired`.
                    inline_consumed = firing_tokens(&mut lens, Some(net), consumed, &[], false).0;
                    inline_taken = changed_places(event);
                    None
                }
                Event::Distributed {
                    transition,
                    produced,
                } if inline => {
                    let (_, produced) = firing_tokens(&mut lens, Some(net), &[], produced, true);
                    changed.append(&mut inline_taken);
                    Some(Notification::Fired {
                        transition: names.transition(*transition),
                        consumed: names.pairs(&inline_consumed),
                        produced: names.produced(net, &produced),
                        duration_ms: duration,
                        subnet_marking_versions: None,
                        outer_marking_version: None,
                        marking_version: version,
                        running,
                    })
                }
                event => {
                    let telling = Telling {
                        names,
                        net: Some(net),
                        generation,
                        running,
                        flight_ms: duration,
                        param: loaded.pending_param.as_deref(),
                    };
                    Some(telling.line(&mut lens, event, *marking_version, took))
                }
            };
            loaded.restore(None, lens);
            let Some(mut notification) = notification else {
                continue;
            };
            // The quiet path (`note_events`) raises the version by this
            // rule without building the notification: the two must agree.
            debug_assert_eq!(notification.is_state(), is_state_event(event));
            if notification.is_state() {
                *marking_version = version;
                // A change of a window is a change of its subnet's port too
                // (§12.7): the subnet's version rides along.
                let (subnets, _) = loaded.cross(None, &changed, marking_version);
                notification.across(subnets, None);
            }
            push(out, notification);
            if hosted {
                // Nobody launched this over the wire, so no landing will
                // log it: the firings of a hosted net are logged here, in
                // order with the inner ones.
                loaded.record_host_firing(event, now, host_flights);
            }
        }
        loaded.reproject(marking_version, &params_changed, &lifted_through, out);
        if hosted {
            reconcile(host_flights, &loaded.net);
        }
        // A staged firing the host has in flight is between its `launched`
        // and its `distributed`: the sets are compared when it lands.
        let compare_fireable = compare_fireable && host_flights.is_empty();
        if compare_fireable && !events.is_empty() {
            let fireable = fireable_names(&loaded.net, &loaded.names);
            if fireable != loaded.last_fireable {
                loaded.last_fireable.clone_from(&fireable);
                push(
                    out,
                    Notification::Fireable {
                        transitions: fireable,
                        marking_version: *marking_version,
                        running,
                    },
                );
            }
            loaded.compare_inner_fireable(*marking_version, out);
        }
        Flushed {
            events,
            inner_firings,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use petritype::{MutateEdge, NetBuilder, TakeEdge};

    #[test]
    fn the_wire_tells_a_landing_s_inner_effects_before_the_landing() {
        // A mutate edge and a produced nested token in one firing: the
        // core's rules 2 and 4 put the inner events on both sides of the
        // `Mutated` and after the `Distributed`.
        let mut inner = NetBuilder::new();
        let seen = inner.place::<u32>("Seen");
        inner.port(seen);
        let inner = inner.build().unwrap();
        let mut b = NetBuilder::new();
        let here = b.place_nested::<Net>("Here");
        let there = b.place_nested::<Net>("There");
        let ticks = b.place::<u32>("Ticks");
        b.seed(here, [inner]);
        b.seed(ticks, [1]);
        b.transition(
            "Touch",
            (TakeEdge(ticks), MutateEdge(here).reaching(["Seen"])),
            (there,),
            move |n: u32, held: &mut Net| {
                held.put(seen, n);
                let mut copy = NetBuilder::new();
                let marks = copy.place::<u32>("Marks");
                copy.port(marks);
                let mut copy = copy.build().unwrap();
                copy.put(marks, n);
                (copy,)
            },
        );
        let mut net = b.build().unwrap();
        net.step().unwrap();
        let events: Vec<Event> = net.drain_events().collect();
        let kinds = |events: &[&Event]| -> Vec<&'static str> {
            events
                .iter()
                .map(|e| match e {
                    Event::Nested { .. } => "nested",
                    Event::Mutated { .. } => "mutated",
                    Event::Fired { .. } => "fired",
                    _ => "other",
                })
                .collect()
        };
        let core: Vec<&Event> = events.iter().collect();
        assert_eq!(kinds(&core), ["nested", "mutated", "fired", "nested"]);
        assert_eq!(
            kinds(&wire_order(&events)),
            ["nested", "nested", "mutated", "fired"]
        );
    }
}
