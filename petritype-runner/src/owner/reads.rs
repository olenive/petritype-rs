//! The reads: one net as a read sees it, and the answers to `get_state`,
//! `get_token` and `query`.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};

use petritype::{Filter, Net, NetError, NetId, PlaceId, TokenId, Topology};

use crate::errors::CoreError;
use crate::projection::Projection;
use crate::protocol::{
    self, ErrorCode, PathStep, PlaceEntry, PlaceMarking, QueryMatch, QueryResult, State,
    SubnetEntry, SubnetMarking, TokenBody, TokenValue,
};
use crate::time::iso8601;
use crate::translate::{self, Names};

use super::address::{Target, descend};
use super::args::{Args, filter_error, optional, page_limit};
use super::events::is_fireable;
use super::{Inner, Outgoing, Owner, ProgressKept, RpcError, Seat, path_text, respond};

/// One net as a read sees it: the outer net, or an inner net with its
/// record.
pub(super) struct View<'a> {
    pub(super) net: &'a Net,
    topology: &'a Topology,
    names: &'a Names,
    progress: &'a ProgressKept,
    pub(super) marking_version: u64,
    /// The net's wire path, empty for the outer one, which the paths of its
    /// tokens' `nets` extend.
    pub(super) path: Vec<PathStep>,
    /// What a result about an inner net carries besides (§12.3).
    inner: Option<InnerView>,
    /// The connection's projection of this net, if it set one (§13.4).
    pub(super) projection: Option<&'a Projection>,
    /// Its subnet places.
    seats: &'a [Seat],
    /// Every inner net's record: its subnets' among them, whose projections
    /// project their ports (§12.7).
    records: &'a HashMap<NetId, Inner>,
}

struct InnerView {
    nested_id: String,
    /// Its name within the token, or a subnet's place: what its
    /// `get_state` calls it.
    name: String,
    template: Option<String>,
}

/// A port place of one of a net's subnet places, as a read of that net
/// reads it: in the subnet, whose place it is (PROTOCOL.md §12.7).
pub(super) struct PortRead<'a> {
    /// The subnet.
    net: &'a Net,
    /// The port's id in the subnet.
    place: PlaceId,
    /// The port's name.
    name: String,
    /// The subnet's path, which the paths of its tokens' nets extend.
    path: Vec<PathStep>,
    /// The subnet's projection, which projects the port in both views.
    projection: Option<&'a Projection>,
}

impl<'a> PortRead<'a> {
    /// The port's `count`, its tokens from `offset` up to `limit`, and
    /// under the subnet's filter `matching`.
    pub(super) fn page(
        &self,
        offset: usize,
        limit: usize,
    ) -> (usize, Option<usize>, Vec<protocol::Token>) {
        page(
            self.net,
            &self.path,
            self.place,
            &self.name,
            self.projection,
            offset,
            limit,
        )
    }
}

/// One place's `count`, its tokens from `offset` up to `limit`, and, under
/// the connection's filter, `matching`, the page then of the tokens the
/// filter keeps (§13.4).
pub(super) fn page(
    net: &Net,
    path: &[PathStep],
    place: PlaceId,
    name: &str,
    projection: Option<&Projection>,
    offset: usize,
    limit: usize,
) -> (usize, Option<usize>, Vec<protocol::Token>) {
    let count = net.count(place);
    match projection {
        None => (
            count,
            None,
            translate::tokens(net, path, place, name, offset, limit),
        ),
        Some(projection) => {
            let kept = projection.kept_in(net, place);
            let tokens = kept
                .iter()
                .skip(offset)
                .take(limit)
                .map(|&token| translate::entry(net, path, place, name, token))
                .collect();
            (count, Some(kept.len()), tokens)
        }
    }
}

impl<'a> View<'a> {
    /// The `path` and `nested_id` a result about an inner net carries; both
    /// `None` for the outer net.
    pub(super) fn address(&self) -> (Option<Vec<PathStep>>, Option<String>) {
        match &self.inner {
            Some(inner) => (Some(self.path.clone()), Some(inner.nested_id.clone())),
            None => (None, None),
        }
    }

    /// The subnet place a window of this net is for, and the port's id in
    /// the subnet; `None` for a place of the net's own.
    fn seat(&self, window: PlaceId) -> Option<(&'a Seat, PlaceId)> {
        self.seats.iter().find_map(|seat| {
            seat.windows
                .iter()
                .find(|(w, _)| *w == window)
                .map(|&(_, port)| (seat, port))
        })
    }

    /// A window of this net read as its port, in its subnet; `None` for a
    /// place of the net's own.
    pub(super) fn port(&self, window: PlaceId) -> Option<PortRead<'a>> {
        let (seat, place) = self.seat(window)?;
        let net = self.net.subnet(seat.place)?;
        let mut path = self.path.clone();
        path.push(translate::subnet_step(&self.names.place(seat.place)));
        Some(PortRead {
            net,
            place,
            name: self.names.place(window),
            path,
            projection: self
                .records
                .get(&seat.id)
                .and_then(|inner| inner.projection.as_ref()),
        })
    }

    /// One token entry of a place of this net, a window's read in its
    /// subnet, so that the paths of the token's nets go through the subnet
    /// (§12.1).
    fn entry(&self, place: PlaceId, token: TokenId) -> protocol::Token {
        match self.port(place) {
            Some(port) => translate::entry(port.net, &port.path, port.place, &port.name, token),
            None => translate::entry(self.net, &self.path, place, &self.names.place(place), token),
        }
    }

    /// The place a request names by `place` and `port` (PROTOCOL.md
    /// §12.7): a place of this net by its name, or a port of one of its
    /// subnet places by the pair, as the place this net has for it. A
    /// subnet place named without a `port` holds no tokens of its own and
    /// is `invalid_params`; a place this net does not have, a `port` its
    /// subnet place does not have, and a `port` beside a place that is no
    /// subnet place are `unknown_place`.
    pub(super) fn place(&self, place: &str, port: Option<&str>) -> Result<PlaceId, RpcError> {
        resolve_place(self.net, place, port)
    }
}

/// [`View::place`], for any net.
pub(super) fn resolve_place(
    net: &Net,
    place: &str,
    port: Option<&str>,
) -> Result<PlaceId, RpcError> {
    let id = net.place_named(place).ok_or_else(|| {
        RpcError::new(ErrorCode::UnknownPlace, format!("no place named {place:?}"))
    })?;
    let subnet = net.subnet(id).is_some();
    match port {
        None if subnet => Err(RpcError::new(
            ErrorCode::InvalidParams,
            format!(
                "{place:?} is a subnet place, which holds no tokens of its own: \
                 name one of its ports as \"port\""
            ),
        )),
        None => Ok(id),
        Some(port) if !subnet => Err(RpcError::new(
            ErrorCode::UnknownPlace,
            format!("{place:?} is not a subnet place, so it has no port {port:?}"),
        )),
        Some(port) => translate::window_of(net, id, port).ok_or_else(|| {
            RpcError::new(
                ErrorCode::UnknownPlace,
                format!("subnet place {place:?} has no port {port:?}"),
            )
        }),
    }
}

impl Owner {
    /// `query {filter?, token_limit?, path?, nested_id?}` (PROTOCOL.md
    /// §13.3): the tokens `filter` keeps (or, without one, the connection's
    /// filter, or else every token) in place order then token order, from
    /// the key index; `count` all of them, and the first `token_limit` in
    /// full, each with its JSON where its place is serializable.
    ///
    /// The ports of the net's subnet places are covered at their subnet
    /// place's position, each match there naming the pair (§12.7). Without
    /// a `filter` a port is projected by its subnet's connection filter and
    /// the net's own places by the net's; a `filter` of the request's own
    /// applies to every place, ports included.
    pub(super) fn query(
        &mut self,
        id: &str,
        args: &Args,
        out: &mut Vec<Outgoing>,
    ) -> Result<(), RpcError> {
        let limit = page_limit(args, "token_limit")?;
        let target = self.target(args)?;
        let view = self.view(&target)?;
        let net = view.net;
        let failed = |error: NetError| RpcError::core(CoreError::Net(&error));
        let found: Vec<(PlaceId, TokenId)> = match optional(args, "filter") {
            Some(value) => {
                let given = translate::filter(value, net).map_err(filter_error)?;
                net.query_ids(&given).map_err(failed)?
            }
            None => {
                let mut found: Vec<(PlaceId, TokenId)> = match view.projection {
                    Some(projection) => projection.kept(net),
                    None => net
                        .query_ids(&Filter::new())
                        .map_err(failed)?
                        .into_iter()
                        .filter(|&(place, _)| view.names.window(place).is_none())
                        .collect(),
                };
                for seat in view.seats {
                    for &(window, _) in &seat.windows {
                        let Some(port) = view.port(window) else {
                            continue;
                        };
                        let kept = match port.projection {
                            Some(projection) => projection.kept_in(port.net, port.place),
                            None => port.net.tokens(port.place).map(|(t, _)| t).collect(),
                        };
                        found.extend(kept.into_iter().map(|token| (window, token)));
                    }
                }
                // Place order: a window's id follows its subnet place's.
                found.sort_by_key(|&(place, _)| place);
                found
            }
        };
        let matches = found
            .iter()
            .take(limit)
            .map(|&(place, token)| {
                let (name, port) = view.names.split(place);
                QueryMatch {
                    token: view.entry(place, token),
                    // Left out for a token that has no JSON form, one that
                    // holds a NaN or an infinity among them (§13.3), as the
                    // Python runner leaves it out.
                    json: net.json(place, token).and_then(Result::ok),
                    place: name,
                    port,
                }
            })
            .collect();
        respond(
            out,
            id,
            QueryResult {
                marking_version: view.marking_version,
                count: found.len(),
                matches,
            },
        );
        Ok(())
    }

    /// The net `target` names, as a read sees it.
    pub(super) fn view(&self, target: &Target) -> Result<View<'_>, RpcError> {
        let loaded = self.loaded_ref();
        let Target::Inner(found) = target else {
            return Ok(View {
                net: &loaded.net,
                topology: &loaded.topology,
                names: &loaded.names,
                progress: &loaded.progress,
                marking_version: self.marking_version,
                path: Vec::new(),
                inner: None,
                projection: loaded.projection.as_ref(),
                seats: &loaded.seats,
                records: &loaded.inner,
            });
        };
        let gone = || {
            RpcError::new(
                ErrorCode::UnknownNet,
                format!("no live inner net at {}", path_text(&found.path)),
            )
        };
        let net = descend(&loaded.net, &found.levels).ok_or_else(gone)?;
        let inner = loaded.inner.get(&found.nested_id).ok_or_else(gone)?;
        let topology = inner.topology.as_ref().ok_or_else(gone)?;
        Ok(View {
            net,
            topology,
            names: &inner.names,
            progress: &inner.progress,
            marking_version: inner.marking_version,
            path: found.path.clone(),
            inner: Some(InnerView {
                nested_id: found.nested_id.to_string(),
                name: found.name.clone(),
                template: inner.template.clone(),
            }),
            projection: inner.projection.as_ref(),
            seats: &inner.seats,
            records: &loaded.inner,
        })
    }

    pub(super) fn state(&self, target: &Target, token_limit: usize) -> Result<State, RpcError> {
        let view = self.view(target)?;
        let net = view.net;
        let transitions = view
            .topology
            .transitions
            .iter()
            .map(|t| {
                // Left out where it cannot be told (§5.2): a closure that
                // panics, a NaN, which is an error, and an infinity, which
                // JSON has no number for, as the Python runner does.
                let priority = catch_unwind(AssertUnwindSafe(|| net.priority_of(t.id)))
                    .ok()
                    .and_then(Result::ok)
                    .filter(|priority| priority.is_finite());
                (
                    t.name.clone(),
                    protocol::TransitionState {
                        enabled: net.is_enabled(t.id),
                        fireable: is_fireable(net, t.id),
                        in_flight: net.is_in_flight(t.id),
                        priority,
                    },
                )
            })
            .collect();
        // Under the connection's filter, each place lists the tokens it
        // keeps and says how many there are (§13.4), from the key index.
        let kept: Option<HashMap<PlaceId, Vec<TokenId>>> = view.projection.map(|projection| {
            let mut kept: HashMap<PlaceId, Vec<TokenId>> = HashMap::new();
            for (place, token) in projection.kept(net) {
                kept.entry(place).or_default().push(token);
            }
            kept
        });
        // A subnet place is `{subnet, ports}` (§12.7), each port read in the
        // subnet under the subnet's filter; the places the net has for those
        // ports have no entry of their own.
        let places = view
            .topology
            .places
            .iter()
            .filter(|p| p.port_of.is_none())
            .map(|p| {
                if let Some(seat) = view.seats.iter().find(|seat| seat.place == p.id)
                    && let Some(subnet) = net.subnet(p.id)
                {
                    let mut path = view.path.clone();
                    path.push(translate::subnet_step(&p.name));
                    let ports = seat
                        .windows
                        .iter()
                        .filter_map(|&(window, _)| view.port(window))
                        .map(|port| {
                            let (count, matching, tokens) = port.page(0, token_limit);
                            let marking = PlaceMarking {
                                count,
                                matching,
                                tokens,
                            };
                            (port.name, marking)
                        })
                        .collect();
                    let entry = SubnetMarking {
                        subnet: SubnetEntry {
                            nested_id: subnet.id().to_string(),
                            path,
                            template: subnet.template().map(str::to_owned),
                            count: translate::own_count(subnet),
                        },
                        ports,
                    };
                    return (p.name.clone(), PlaceEntry::Subnet(entry));
                }
                let (matching, tokens) = match &kept {
                    None => (
                        None,
                        translate::tokens(net, &view.path, p.id, &p.name, 0, token_limit),
                    ),
                    Some(kept) => {
                        let ids = kept.get(&p.id).map_or(&[][..], Vec::as_slice);
                        let tokens = ids
                            .iter()
                            .take(token_limit)
                            .map(|&token| translate::entry(net, &view.path, p.id, &p.name, token))
                            .collect();
                        (Some(ids.len()), tokens)
                    }
                };
                let marking = PlaceMarking {
                    count: net.count(p.id),
                    matching,
                    tokens,
                };
                (p.name.clone(), PlaceEntry::Tokens(marking))
            })
            .collect();
        let (path, nested_id) = view.address();
        let loaded = self.loaded_ref();
        Ok(State {
            protocol_version: protocol::PROTOCOL_VERSION,
            runtime: protocol::RUNTIME.to_owned(),
            net: view
                .inner
                .as_ref()
                .map_or_else(|| loaded.name.to_owned(), |inner| inner.name.clone()),
            // An inner net is in its first generation for as long as it
            // exists (§12.3).
            generation: if view.inner.is_some() {
                1
            } else {
                self.generation
            },
            marking_version: view.marking_version,
            running: loaded.running,
            topology: protocol::Topology {
                // The outer net's alone: a control is pressed on the outer
                // net (`press` is outer-level, §12.3).
                controls: if view.inner.is_none() {
                    translate::controls(net, view.names)
                } else {
                    Vec::new()
                },
                ..translate::topology(net, view.topology, view.names)
            },
            transitions,
            marking: protocol::Marking { places },
            progress: protocol::Progress {
                // Per net (§5.4): this net's own firings, none of those in
                // the nets inside it.
                step_count: net.fired_count(),
                last_fired: view.progress.last_fired.clone(),
                fired_counts: view.progress.fired_counts.clone(),
                last_progress_at: view.progress.last_progress_at.map(iso8601),
            },
            path,
            nested_id,
            template: view.inner.as_ref().and_then(|inner| inner.template.clone()),
        })
    }

    /// `get_token {id}` (PROTOCOL.md §6.5): the live token with that id, in
    /// its one canonical form (`"01"` names no token), among the net's own
    /// places. A port's tokens carry their subnet's ids, which may equal the
    /// net's own, so a port's token is read through its subnet (§12.7).
    pub(super) fn token(&self, target: &Target, wanted: &str) -> Result<TokenValue, RpcError> {
        let view = self.view(target)?;
        let unknown =
            || RpcError::new(ErrorCode::UnknownToken, format!("no live token {wanted:?}"));
        let token = translate::token_id(wanted).ok_or_else(unknown)?;
        let net = view.net;
        let place = view
            .topology
            .places
            .iter()
            .filter(|p| p.port_of.is_none())
            .find(|p| net.tokens(p.id).any(|(t, _)| t == token))
            .ok_or_else(unknown)?;
        // A token the connection's filter hides is not there for it
        // (§13.4), and is answered as one that is not there at all.
        if view
            .projection
            .is_some_and(|projection| !projection.keeps(Some(net), place.id, token))
        {
            return Err(unknown());
        }
        // A token that has no JSON form now, one that holds a NaN or an
        // infinity among them, is shown as text (§6.5), as the Python
        // runner shows it.
        let value = match net.json(place.id, token) {
            Some(Ok(value)) => TokenBody::Json { value },
            Some(Err(_)) | None => TokenBody::Text {
                text: net.debug(place.id, token).unwrap_or_default(),
            },
        };
        let (path, nested_id) = view.address();
        Ok(TokenValue {
            id: wanted.to_owned(),
            place: place.name.clone(),
            type_name: place.type_name.to_owned(),
            value,
            path,
            nested_id,
            nets: translate::nets(net, &view.path, place.id, &place.name, token),
        })
    }
}
