//! Which method a request names, when it is answered, and the handler that
//! answers it.

use std::time::SystemTime;

use petritype::{Completed, JsonCommand, PlaceId, TransitionId};
use serde_json::{Value, json};

use crate::projection::Projection;
use crate::protocol::{DistinctResult, ErrorCode, LogEntry, PlaceName, PlaceTokens, Request};
use crate::translate;

use super::address::{Level, Target};
use super::args::{
    Args, filter_error, not_place_names, optional, page_limit, port_arg, string_arg, u64_arg,
    usize_arg,
};
use super::reads::page;
use super::{LOG_CAPACITY, Loaded, Outgoing, Owner, RpcError, path_text, respond, respond_error};

/// A method of protocol version 2, read off a request by its wire name.
///
/// Each question the owner asks of a method is a `match` over every method,
/// so a new one is placed in each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Method {
    Describe,
    Load,
    GetState,
    GetPlaceTokens,
    GetToken,
    Step,
    Start,
    Stop,
    Reset,
    Inject,
    Enable,
    Disable,
    SetParam,
    GetLog,
    Query,
    Distinct,
    SetFilter,
    Press,
    Shutdown,
}

impl Method {
    /// The method of protocol version 2 named `name`, or `None` for a name
    /// it does not have.
    fn parse(name: &str) -> Option<Method> {
        Some(match name {
            "describe" => Method::Describe,
            "load" => Method::Load,
            "get_state" => Method::GetState,
            "get_place_tokens" => Method::GetPlaceTokens,
            "get_token" => Method::GetToken,
            "step" => Method::Step,
            "start" => Method::Start,
            "stop" => Method::Stop,
            "reset" => Method::Reset,
            "inject" => Method::Inject,
            "enable" => Method::Enable,
            "disable" => Method::Disable,
            "set_param" => Method::SetParam,
            "get_log" => Method::GetLog,
            "query" => Method::Query,
            "distinct" => Method::Distinct,
            "set_filter" => Method::SetFilter,
            "press" => Method::Press,
            "shutdown" => Method::Shutdown,
            _ => return None,
        })
    }

    /// Its name on the wire.
    fn name(self) -> &'static str {
        match self {
            Method::Describe => "describe",
            Method::Load => "load",
            Method::GetState => "get_state",
            Method::GetPlaceTokens => "get_place_tokens",
            Method::GetToken => "get_token",
            Method::Step => "step",
            Method::Start => "start",
            Method::Stop => "stop",
            Method::Reset => "reset",
            Method::Inject => "inject",
            Method::Enable => "enable",
            Method::Disable => "disable",
            Method::SetParam => "set_param",
            Method::GetLog => "get_log",
            Method::Query => "query",
            Method::Distinct => "distinct",
            Method::SetFilter => "set_filter",
            Method::Press => "press",
            Method::Shutdown => "shutdown",
        }
    }

    /// Whether a request for it is answered at once while a body is
    /// airborne; every other one waits in arrival order until it lands
    /// (DESIGN §19.3.5). `shutdown` is answered because it abandons the body
    /// (§19.3.6), and `describe` because it reads nothing of the net.
    /// `query` and `distinct` are reads (PROTOCOL.md §13.3); `set_filter` is
    /// not, and waits, so that a firing is projected through one filter from
    /// its `launched` to its landing.
    fn answered_while_airborne(self) -> bool {
        match self {
            Method::GetState
            | Method::GetPlaceTokens
            | Method::GetToken
            | Method::GetLog
            | Method::Query
            | Method::Distinct
            | Method::Stop
            | Method::Shutdown
            | Method::Describe => true,
            Method::Load
            | Method::Step
            | Method::Start
            | Method::Reset
            | Method::Inject
            | Method::Enable
            | Method::Disable
            | Method::SetParam
            | Method::SetFilter
            | Method::Press => false,
        }
    }

    /// Whether it is a read that a `path` or a `nested_id` may point at an
    /// inner net (PROTOCOL.md §12.3). `inject` and `set_filter` take one
    /// too, but are not reads: one changes the net it names, the other the
    /// connection's view of it, and both wait for an airborne body to land.
    fn is_addressed_read(self) -> bool {
        match self {
            Method::GetState
            | Method::GetPlaceTokens
            | Method::GetToken
            | Method::GetLog
            | Method::Query
            | Method::Distinct => true,
            Method::Describe
            | Method::Load
            | Method::Step
            | Method::Start
            | Method::Stop
            | Method::Reset
            | Method::Inject
            | Method::Enable
            | Method::Disable
            | Method::SetParam
            | Method::SetFilter
            | Method::Press
            | Method::Shutdown => false,
        }
    }

    /// Whether it acts on the outer net alone (PROTOCOL.md §12.3): a `path`
    /// or a `nested_id` on it is `invalid_params`. An inner net's switches
    /// and parameters are its template's, or an outer body's to set.
    fn is_outer_only(self) -> bool {
        match self {
            Method::Step
            | Method::Start
            | Method::Stop
            | Method::Reset
            | Method::Enable
            | Method::Disable
            | Method::SetParam
            | Method::Press => true,
            Method::Describe
            | Method::Load
            | Method::GetState
            | Method::GetPlaceTokens
            | Method::GetToken
            | Method::Inject
            | Method::GetLog
            | Method::Query
            | Method::Distinct
            | Method::SetFilter
            | Method::Shutdown => false,
        }
    }

    /// Whether a hosted owner answers it ([`Owner::hosted`]): `describe` and
    /// the reads do. The net is the host program's to load, fire and change,
    /// so every other method is `unsupported` there. `set_filter` is among
    /// the refused: the projection it sets is one connection's, and a hosted
    /// net may be read over several at once.
    fn is_served_when_hosted(self) -> bool {
        match self {
            Method::Describe
            | Method::GetState
            | Method::GetPlaceTokens
            | Method::GetToken
            | Method::GetLog
            | Method::Query
            | Method::Distinct => true,
            Method::Load
            | Method::Step
            | Method::Start
            | Method::Stop
            | Method::Reset
            | Method::Inject
            | Method::Enable
            | Method::Disable
            | Method::SetParam
            | Method::SetFilter
            | Method::Press
            | Method::Shutdown => false,
        }
    }
}

impl Owner {
    pub(super) fn request(&mut self, request: Request, now: SystemTime, out: &mut Vec<Outgoing>) {
        if let Some(loaded) = &mut self.loaded
            && loaded.in_flight.is_some()
            && (!Method::parse(&request.method).is_some_and(Method::answered_while_airborne)
                || held_by_the_body(loaded, &request))
        {
            loaded.deferred.push_back(request);
            return;
        }
        if self.serve(request, now, out) {
            return;
        }
        self.drive(now, out);
    }

    pub(super) fn completed(
        &mut self,
        completed: Completed,
        now: SystemTime,
        out: &mut Vec<Outgoing>,
    ) {
        let Some(loaded) = &mut self.loaded else {
            // Nothing launched it: a load replaces the net only when nothing
            // is airborne. Dropping it strands nothing that still exists.
            return;
        };
        if loaded.in_flight.is_none() {
            return;
        }
        self.land(completed, now, out);
        if self.serve_deferred(now, out) {
            return;
        }
        self.drive(now, out);
    }

    /// Serves the requests that waited for a landing, in arrival order, until
    /// one of them launches a body again. True if one of them was `shutdown`.
    fn serve_deferred(&mut self, now: SystemTime, out: &mut Vec<Outgoing>) -> bool {
        let Some(loaded) = &mut self.loaded else {
            return false;
        };
        let mut queue = std::mem::take(&mut loaded.deferred);
        while let Some(request) = queue.pop_front() {
            if self.serve(request, now, out) {
                return true;
            }
            if let Some(loaded) = &mut self.loaded
                && loaded.in_flight.is_some()
            {
                // A deferred `step` launched again: the rest wait for it.
                queue.append(&mut loaded.deferred);
                loaded.deferred = queue;
                return false;
            }
        }
        false
    }

    /// Answers one request. True if the caller must stop: `shutdown`.
    fn serve(&mut self, request: Request, now: SystemTime, out: &mut Vec<Outgoing>) -> bool {
        let Request { id, method, params } = request;
        let Some(known) = Method::parse(&method) else {
            respond_error(
                out,
                &id,
                RpcError::new(
                    ErrorCode::UnknownMethod,
                    format!("no such method: {method:?}"),
                ),
            );
            return false;
        };
        // `null` params are `{}`, as an absent `params` is (PROTOCOL.md §2).
        let args = match params {
            Value::Object(args) => args,
            Value::Null => Args::new(),
            _ => {
                respond_error(
                    out,
                    &id,
                    RpcError::new(ErrorCode::InvalidParams, "params must be an object"),
                );
                return false;
            }
        };
        match known {
            _ if self.hosted && !known.is_served_when_hosted() => respond_error(
                out,
                &id,
                RpcError::new(
                    ErrorCode::Unsupported,
                    format!(
                        "the host program owns this net: it is read over the wire, and {method} is not served"
                    ),
                ),
            ),
            Method::Describe => respond(out, &id, self.describe()),
            Method::Shutdown => {
                respond(out, &id, json!({"status": "ok"}));
                out.push(Outgoing::Exit);
                return true;
            }
            Method::Load => self.load(&id, &args, out),
            _ if self.loaded.is_none() => respond_error(
                out,
                &id,
                RpcError::new(ErrorCode::NotLoaded, format!("{method} needs a loaded net")),
            ),
            _ if known.is_outer_only()
                && (optional(&args, "path").is_some()
                    || optional(&args, "nested_id").is_some()) =>
            {
                respond_error(
                    out,
                    &id,
                    RpcError::new(
                        ErrorCode::InvalidParams,
                        format!(
                            "{method} acts on the outer net only (PROTOCOL.md §12.3): \
                         it takes no path or nested_id"
                        ),
                    ),
                );
            }
            Method::Reset => self.reset(&id, out),
            _ => {
                if let Err(error) = self.serve_loaded(&id, known, &args, now, out) {
                    respond_error(out, &id, error);
                }
            }
        }
        false
    }

    fn serve_loaded(
        &mut self,
        id: &str,
        method: Method,
        args: &Args,
        now: SystemTime,
        out: &mut Vec<Outgoing>,
    ) -> Result<(), RpcError> {
        match method {
            Method::GetState => {
                let limit = page_limit(args, "token_limit")?;
                let target = self.target(args)?;
                let state = self.state(&target, limit)?;
                respond(out, id, state);
            }
            Method::GetPlaceTokens => {
                let place = string_arg(args, "place")?;
                let port = port_arg(args)?;
                let offset = usize_arg(args, "offset")?.unwrap_or(0);
                let limit = page_limit(args, "limit")?;
                let target = self.target(args)?;
                let view = self.view(&target)?;
                let place_id = view.place(place, port)?;
                // Under the connection's filter the page is of the tokens it
                // keeps, and `has_more` is relative to them (§13.4). A port
                // is read in its subnet, under the subnet's filter (§12.7),
                // and answered with the version of the net addressed.
                let (count, matching, tokens) = match view.port(place_id) {
                    Some(port) => port.page(offset, limit),
                    None => page(
                        view.net,
                        &view.path,
                        place_id,
                        place,
                        view.projection,
                        offset,
                        limit,
                    ),
                };
                let (path, nested_id) = view.address();
                let page = PlaceTokens {
                    place: place.to_owned(),
                    port: port.map(str::to_owned),
                    marking_version: view.marking_version,
                    count,
                    matching,
                    offset,
                    limit,
                    has_more: offset.saturating_add(tokens.len()) < matching.unwrap_or(count),
                    tokens,
                    path,
                    nested_id,
                };
                respond(out, id, page);
            }
            Method::Query => self.query(id, args, out)?,
            Method::Distinct => {
                let key = string_arg(args, "key")?.to_owned();
                let target = self.target(args)?;
                let view = self.view(&target)?;
                // A place by its name, a port by {"place", "port"} (§12.7).
                let places = match optional(args, "places") {
                    None => None,
                    Some(Value::Array(names)) => {
                        let names: Vec<PlaceName> = names
                            .iter()
                            .map(PlaceName::parse)
                            .collect::<Result<_, _>>()
                            .map_err(|why| {
                                RpcError::new(
                                    ErrorCode::InvalidParams,
                                    format!("in \"places\", {why}"),
                                )
                            })?;
                        Some(translate::place_ids(&names, view.net).map_err(filter_error)?)
                    }
                    Some(_) => return Err(not_place_names()),
                };
                let values = view.net.distinct_values(&key, places.as_deref());
                respond(
                    out,
                    id,
                    DistinctResult {
                        marking_version: view.marking_version,
                        key,
                        values,
                    },
                );
            }
            Method::SetFilter => {
                let target = self.target(args)?;
                let projection = match optional(args, "filter") {
                    None => None,
                    Some(value) => {
                        let view = self.view(&target)?;
                        let filter = translate::filter(value, view.net).map_err(filter_error)?;
                        Some(Projection::new(filter, view.net))
                    }
                };
                let version = match &target {
                    Target::Outer => {
                        self.loaded_mut().projection = projection;
                        self.marking_version
                    }
                    Target::Inner(found) => {
                        let inner = self
                            .loaded_mut()
                            .inner
                            .get_mut(&found.nested_id)
                            .ok_or_else(|| {
                                RpcError::new(
                                    ErrorCode::UnknownNet,
                                    format!("no live inner net at {}", path_text(&found.path)),
                                )
                            })?;
                        inner.projection = projection;
                        inner.marking_version
                    }
                };
                respond(out, id, json!({ "marking_version": version }));
            }
            Method::GetToken => {
                let token = string_arg(args, "id")?;
                let target = self.target(args)?;
                let value = self.token(&target, token)?;
                respond(out, id, value);
            }
            Method::GetLog => {
                let after = u64_arg(args, "after")?.unwrap_or(0);
                let limit = u64_arg(args, "limit")?
                    .unwrap_or(LOG_CAPACITY as u64)
                    .clamp(1, LOG_CAPACITY as u64) as usize;
                // One ring for the process (PROTOCOL.md §8): an inner net's
                // entries, and its own inner nets', are those whose chain
                // holds it.
                let within = match self.target(args)? {
                    Target::Outer => None,
                    Target::Inner(found) => Some(found.nested_id),
                };
                let entries: Vec<&LogEntry> = self
                    .loaded_ref()
                    .log
                    .iter()
                    .filter(|logged| logged.entry.seq > after)
                    .filter(|logged| within.is_none_or(|id| logged.chain.contains(&id)))
                    .take(limit)
                    .map(|logged| &logged.entry)
                    .collect();
                respond(out, id, json!({ "entries": entries }));
            }
            Method::Step => self.step(id, now, out)?,
            Method::Start => {
                let loaded = self.loaded_mut();
                loaded.running = true;
                loaded.idle = false;
                respond(
                    out,
                    id,
                    json!({"status": "started", "marking_version": self.marking_version, "running": true}),
                );
            }
            Method::Stop => {
                self.loaded_mut().running = false;
                respond(
                    out,
                    id,
                    json!({"status": "stopped", "marking_version": self.marking_version, "running": false}),
                );
            }
            Method::Inject => self.inject(id, args, now, out)?,
            Method::Press => self.press(id, args, now, out)?,
            Method::Enable | Method::Disable => {
                let transition = string_arg(args, "transition")?.to_owned();
                self.apply(if method == Method::Enable {
                    JsonCommand::Enable { transition }
                } else {
                    JsonCommand::Disable { transition }
                })?;
                self.flush_events(now, None, true, out);
                respond(out, id, json!({"marking_version": self.marking_version}));
            }
            Method::SetParam => {
                let param = string_arg(args, "param")?.to_owned();
                let value = args.get("value").cloned().ok_or_else(|| {
                    RpcError::new(ErrorCode::InvalidParams, "set_param needs a \"value\"")
                })?;
                self.loaded_mut().pending_param = Some(param.clone());
                let applied = self.apply(JsonCommand::SetParam { param, value });
                if applied.is_ok() {
                    self.flush_events(now, None, true, out);
                }
                self.loaded_mut().pending_param = None;
                applied?;
                respond(out, id, json!({"marking_version": self.marking_version}));
            }
            // Answered in `serve`, before a handler is looked for.
            Method::Describe | Method::Load | Method::Reset | Method::Shutdown => {
                return Err(RpcError::new(
                    ErrorCode::Internal,
                    format!("{} has no handler in this runner", method.name()),
                ));
            }
        }
        Ok(())
    }

    /// The net a request names, with a record made for an inner one.
    pub(super) fn target(&mut self, args: &Args) -> Result<Target, RpcError> {
        let loaded = self.loaded_mut();
        let target = loaded.resolve(args)?;
        if let Target::Inner(found) = &target {
            loaded.know(found);
        }
        Ok(target)
    }
}

/// Whether a read must wait for the airborne body to land (PROTOCOL.md
/// §12.3, §12.7): it names an inner net that does not resolve now, which
/// may be inside the token the body holds, or a subnet one of whose ports
/// the airborne firing has an edge on. After the landing it resolves, or is
/// `unknown_net` then.
fn held_by_the_body(loaded: &Loaded, request: &Request) -> bool {
    if !Method::parse(&request.method).is_some_and(Method::is_addressed_read) {
        return false;
    }
    let Value::Object(args) = &request.params else {
        return false;
    };
    match loaded.resolve(args) {
        Err(RpcError {
            code: ErrorCode::UnknownNet,
            ..
        }) => true,
        Ok(Target::Inner(found)) => match (found.levels.as_slice(), &loaded.in_flight) {
            ([Level::Subnet { place }], Some(flight)) => loaded.reaches(flight.transition, *place),
            _ => false,
        },
        _ => false,
    }
}

impl Loaded {
    /// Whether the loaded net's `transition` has an edge, of any kind, on a
    /// port of its subnet place `subnet` (PROTOCOL.md §12.7).
    fn reaches(&self, transition: TransitionId, subnet: PlaceId) -> bool {
        let windows: Vec<PlaceId> = self
            .seats
            .iter()
            .filter(|seat| seat.place == subnet)
            .flat_map(|seat| seat.windows.iter().map(|&(window, _)| window))
            .collect();
        self.topology.edges.iter().any(|edge| match *edge {
            petritype::EdgeInfo::Input {
                place,
                transition: t,
                ..
            }
            | petritype::EdgeInfo::Output {
                place,
                transition: t,
                ..
            } => t == transition && windows.contains(&place),
            _ => false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every method of protocol version 2.
    const NAMES: [&str; 19] = [
        "describe",
        "load",
        "get_state",
        "get_place_tokens",
        "get_token",
        "step",
        "start",
        "stop",
        "reset",
        "inject",
        "enable",
        "disable",
        "set_param",
        "get_log",
        "query",
        "distinct",
        "set_filter",
        "press",
        "shutdown",
    ];

    #[test]
    fn a_method_is_known_by_its_wire_name_alone() {
        for name in NAMES {
            let method = Method::parse(name).expect(name);
            assert_eq!(method.name(), name);
        }
        for name in ["", "Describe", "get-state", "getState", " step", "nonsense"] {
            assert_eq!(Method::parse(name), None, "{name:?}");
        }
    }

    /// The methods each question picks, in the order of [`NAMES`].
    #[test]
    fn each_question_about_a_method_picks_its_own_methods() {
        let picked = |question: fn(Method) -> bool| -> Vec<&str> {
            NAMES
                .into_iter()
                .filter(|name| question(Method::parse(name).expect(name)))
                .collect()
        };
        assert_eq!(
            picked(Method::answered_while_airborne),
            [
                "describe",
                "get_state",
                "get_place_tokens",
                "get_token",
                "stop",
                "get_log",
                "query",
                "distinct",
                "shutdown",
            ]
        );
        assert_eq!(
            picked(Method::is_addressed_read),
            [
                "get_state",
                "get_place_tokens",
                "get_token",
                "get_log",
                "query",
                "distinct",
            ]
        );
        assert_eq!(
            picked(Method::is_outer_only),
            [
                "step",
                "start",
                "stop",
                "reset",
                "enable",
                "disable",
                "set_param",
                "press",
            ]
        );
        assert_eq!(
            picked(Method::is_served_when_hosted),
            [
                "describe",
                "get_state",
                "get_place_tokens",
                "get_token",
                "get_log",
                "query",
                "distinct",
            ]
        );
    }
}
