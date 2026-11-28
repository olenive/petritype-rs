//! Inner nets (PROTOCOL.md §12.1): where each one is in the marking, how a
//! request names one, and the records the owner keeps of the live ones.

use std::collections::{HashMap, HashSet};

use petritype::{Event, Net, NetId, PlaceId, TokenId};
use serde_json::Value;

use crate::protocol::{self, ErrorCode, PathStep};
use crate::translate::{self, Names};

use super::args::{Args, optional};
use super::lifted::Lifted;
use super::{Around, Inner, InnerAddress, Loaded, RpcError};

/// One step down from a net to one of its inner nets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Level {
    /// A net a token carries: the nested place, the token in it, and which
    /// of the token's nets, by its position in `Nested::nets` (names need
    /// not be unique; positions are).
    Token {
        place: PlaceId,
        token: TokenId,
        index: usize,
    },
    /// The subnet of a subnet place.
    Subnet { place: PlaceId },
}

/// An inner net found in the live marking: how to get there, its path as the
/// marking stands, its nested id and its name within its token.
#[derive(Debug, Clone)]
pub(super) struct Found {
    pub(super) levels: Vec<Level>,
    pub(super) path: Vec<PathStep>,
    pub(super) nested_id: NetId,
    pub(super) name: String,
}

/// The net a request names: the outer one, or an inner one.
pub(super) enum Target {
    Outer,
    Inner(Found),
}

/// What [`walk`] calls with each inner net: the levels down to it, the way
/// down to it, its name within its token, and the net. `Some` stops the
/// walk.
type Visit<'v, 'a, R> = dyn FnMut(&[Level], &[Way<'a>], &'a str, &'a Net) -> Option<R> + 'v;

/// `[place, token]` pairs of one net, as its events carry them.
pub(super) type Pairs = Vec<(PlaceId, TokenId)>;

/// One step of the way down to a net on a [`walk`], borrowed from the nets
/// it passes: what a [`PathStep`] is made of. A walk visits every inner net
/// and most visits keep no path, so a path is written out only for a visit
/// that keeps one ([`path_of`]).
#[derive(Debug, Clone, Copy)]
pub(super) enum Way<'a> {
    /// Into a net a token carries: the nested place's name, the token, and
    /// the net's name in it.
    Token {
        place: &'a str,
        token: TokenId,
        net: &'a str,
    },
    /// Into a subnet: its subnet place's name.
    Subnet { place: &'a str },
}

/// The path a way down spells (PROTOCOL.md §12.1).
pub(super) fn path_of(way: &[Way<'_>]) -> Vec<PathStep> {
    way.iter()
        .map(|step| match *step {
            Way::Token { place, token, net } => translate::step(place, token, net),
            Way::Subnet { place } => translate::subnet_step(place),
        })
        .collect()
}

/// Calls `visit` with every inner net below `net`, at every depth and in
/// passive places too, in walk order (places in declaration order, a
/// subnet place's subnet at its place's position, a nested place's tokens
/// oldest first, a token's nets in order, a net's own inner nets straight
/// after it), until `visit` answers `Some`. `levels` and `way` are the way
/// down to `net`, empty for the loaded net, and are left as they were.
///
/// A place a net has for a port of one of its subnet places is not walked:
/// the nets of a port's tokens are reached through the subnet (PROTOCOL.md
/// §12.1), whose own place the port is. A subnet's name, as `visit` is
/// given it, is its subnet place's.
fn walk<'a, R>(
    net: &'a Net,
    levels: &mut Vec<Level>,
    way: &mut Vec<Way<'a>>,
    visit: &mut Visit<'_, 'a, R>,
) -> Option<R> {
    for (place, info) in net.places() {
        if info.port_of.is_some() {
            continue;
        }
        if let Some(subnet) = net.subnet(place).filter(|_| info.subnet) {
            levels.push(Level::Subnet { place });
            way.push(Way::Subnet { place: info.name });
            let found =
                visit(levels, way, info.name, subnet).or_else(|| walk(subnet, levels, way, visit));
            way.pop();
            levels.pop();
            if found.is_some() {
                return found;
            }
            continue;
        }
        if !info.nested {
            continue;
        }
        for (token, _) in net.tokens(place) {
            let Some(nets) = net.nested(place, token) else {
                continue;
            };
            for (index, (name, inner)) in nets.into_iter().enumerate() {
                levels.push(Level::Token {
                    place,
                    token,
                    index,
                });
                way.push(Way::Token {
                    place: info.name,
                    token,
                    net: name,
                });
                let found =
                    visit(levels, way, name, inner).or_else(|| walk(inner, levels, way, visit));
                way.pop();
                levels.pop();
                if found.is_some() {
                    return found;
                }
            }
        }
    }
    None
}

/// Calls `visit` with every inner net of the loaded net `outer`, as
/// [`walk`] does.
pub(super) fn walk_all<'a, R>(outer: &'a Net, visit: &mut Visit<'_, 'a, R>) -> Option<R> {
    walk(outer, &mut Vec::new(), &mut Vec::new(), visit)
}

/// The first inner net below `outer`, at any depth, that `wanted` picks.
pub(super) fn find(outer: &Net, wanted: impl Fn(&Net) -> bool) -> Option<(Found, &Net)> {
    walk_all(outer, &mut |levels, way, name, net| {
        wanted(net).then(|| {
            let found = Found {
                levels: levels.to_vec(),
                path: path_of(way),
                nested_id: net.id(),
                name: name.to_owned(),
            };
            (found, net)
        })
    })
}

/// The live inner net `id` below `outer`, as [`find`] finds it: by
/// `last_path`, the path its record last found it at, while that still
/// leads to it, and otherwise by a walk. Both give the same net, levels and
/// path: a net is in one place at a time, and a path names the places of a
/// net's own, never a place for a port ([`Net::place_named`]), as a walk
/// goes. The first costs a step per level, where a walk passes every net
/// before it.
pub(super) fn locate<'a>(
    outer: &'a Net,
    id: NetId,
    last_path: Option<&[PathStep]>,
) -> Option<(Found, &'a Net)> {
    if let Some(path) = last_path
        && let Ok(Some((found, net))) = resolve_path(outer, path)
        && net.id() == id
    {
        return Some((found, net));
    }
    find(outer, |net| net.id() == id)
}

/// The net around the net `levels` lead to from `outer`, when the last
/// level is into a subnet; `None` for the loaded net and for a net a token
/// carries.
fn around_of(outer: &Net, levels: &[Level]) -> Option<Around> {
    let (last, above) = levels.split_last()?;
    if !matches!(last, Level::Subnet { .. }) {
        return None;
    }
    Some(if above.is_empty() {
        Around::Loaded
    } else {
        Around::Inner(descend(outer, above)?.id())
    })
}

/// The address of the net an inner body is about to run in, as the core's
/// deep step tells of it: the path and the nested id its firing's `step`
/// result and log entry will carry, the places named as
/// [`lifted_path`](Loaded::lifted_path) names them, from `names`, the loaded
/// net's, and the records `inner` keeps of the nets on the way. It reads no
/// net, since the deep step has the loaded net while the body runs. `None`
/// for a level of a kind this runner does not know.
pub(super) fn inner_address(
    names: &Names,
    inner: &HashMap<NetId, Inner>,
    body: &petritype::InnerBody<'_>,
) -> Option<InnerAddress> {
    let mut path = Vec::with_capacity(body.path.len());
    for (depth, step) in body.path.iter().enumerate() {
        let place_name = |place: PlaceId| match depth {
            0 => names.place(place),
            _ => inner
                .get(&body.nested_ids[depth - 1])
                .map_or_else(|| place.to_string(), |above| above.names.place(place)),
        };
        path.push(match step {
            petritype::PathStep::Token { place, token, net } => {
                translate::step(&place_name(*place), *token, net)
            }
            petritype::PathStep::Subnet { place } => translate::subnet_step(&place_name(*place)),
            _ => return None,
        });
    }
    Some(InnerAddress {
        path,
        nested_id: body.nested_id().to_string(),
    })
}

/// The inner net a request's path names (PROTOCOL.md §12.1), resolved one
/// step at a time from the loaded net down, or `None` for the empty path,
/// which names the loaded net. The error names the step that failed by its
/// index from 0.
fn resolve_path<'a>(
    outer: &'a Net,
    steps: &[PathStep],
) -> Result<Option<(Found, &'a Net)>, String> {
    if steps.is_empty() {
        return Ok(None);
    }
    let mut net = outer;
    let mut levels = Vec::with_capacity(steps.len());
    let mut name = "";
    for (index, step) in steps.iter().enumerate() {
        let (level, below, below_name) =
            descend_step(net, step).map_err(|why| format!("step {index} of the path: {why}"))?;
        levels.push(level);
        net = below;
        name = below_name;
    }
    let found = Found {
        levels,
        path: steps.to_vec(),
        nested_id: net.id(),
        name: name.to_owned(),
    };
    Ok(Some((found, net)))
}

/// One step of a path down from `net` (PROTOCOL.md §12.1): the level it
/// takes, the net it reaches and that net's name (in its token, or for a
/// subnet its subnet place's), or why it names no live net. Names are
/// compared exactly as given, and a token id only in its canonical form.
fn descend_step<'a, 's>(
    net: &'a Net,
    step: &'s PathStep,
) -> Result<(Level, &'a Net, &'s str), String> {
    match step {
        PathStep::Token {
            place: place_name,
            token,
            net: wanted,
        } => {
            let (place, info) = net
                .place_named(place_name)
                .and_then(|place| net.places().find(|&(id, _)| id == place))
                .ok_or_else(|| format!("no place named {place_name:?}"))?;
            if info.subnet {
                return Err(format!(
                    "place {place_name:?} is a subnet place, whose step names the place alone"
                ));
            }
            if !info.nested {
                return Err(format!("place {place_name:?} holds no nets"));
            }
            let nets = translate::token_id(token)
                .and_then(|id| Some((id, net.nested(place, id)?)))
                .ok_or_else(|| format!("no token {token:?} in {place_name:?}"));
            let (token_id, nets) = nets?;
            let index = nets
                .iter()
                .position(|(name, _)| name == wanted)
                .ok_or_else(|| {
                    format!("token {token:?} in {place_name:?} holds no net named {wanted:?}")
                })?;
            let level = Level::Token {
                place,
                token: token_id,
                index,
            };
            Ok((level, nets[index].1, wanted))
        }
        PathStep::Subnet { place: place_name } => {
            let place = net
                .place_named(place_name)
                .ok_or_else(|| format!("no place named {place_name:?}"))?;
            let subnet = net
                .subnet(place)
                .ok_or_else(|| format!("place {place_name:?} is not a subnet place"))?;
            Ok((Level::Subnet { place }, subnet, place_name))
        }
    }
}

/// The net `levels` lead to from `net`, as the marking stands.
pub(super) fn descend<'a>(net: &'a Net, levels: &[Level]) -> Option<&'a Net> {
    levels.iter().try_fold(net, |here, level| match *level {
        Level::Token {
            place,
            token,
            index,
        } => here
            .nested(place, token)?
            .into_iter()
            .nth(index)
            .map(|(_, inner)| inner),
        Level::Subnet { place } => here.subnet(place),
    })
}

/// Calls `f` with the net `levels` lead to, mutably, through
/// `Net::nested_scope` or `Net::subnet_scope` at every level, so that what
/// `f` did there is lifted level by level into the outer log as each scope
/// closes. `None` if the levels lead nowhere, or into a subnet held still
/// while a firing on one of its ports is in flight.
pub(super) fn descend_mut<R>(
    net: &mut Net,
    levels: &[Level],
    f: impl FnOnce(&mut Net) -> R,
) -> Option<R> {
    let Some((level, rest)) = levels.split_first() else {
        return Some(f(net));
    };
    match *level {
        Level::Token {
            place,
            token,
            index,
        } => net
            .nested_scope(place, token, |nets| {
                let (_, inner) = nets.get_mut(index)?;
                descend_mut(inner, rest, f)
            })
            .flatten(),
        Level::Subnet { place } => net
            .subnet_scope(place, |subnet| descend_mut(subnet, rest, f))
            .ok()
            .flatten(),
    }
}

impl Loaded {
    /// A record for every inner net the marking holds now, its fireable set
    /// taken silently: what `load` and `reset` start from.
    pub(super) fn know_all(&mut self) {
        let mut seen = Vec::new();
        let outer = &self.net;
        walk_all(outer, &mut |levels, way, _, net| {
            let around = around_of(outer, levels);
            seen.push((net.id(), Inner::new(Some(net), path_of(way), true, around)));
            None::<()>
        });
        self.inner.extend(seen);
    }

    /// A record for every inner net the marking holds that has none, its
    /// fireable set taken silently: what an `inject` adds, the nets inside
    /// the token it brought, which the owner sees from then on as it saw
    /// those the marking held at `load`. Records already kept stay as they
    /// are.
    ///
    /// Returns the nested ids of every inner net the marking holds, for
    /// [`prune`](Loaded::prune) once the change has been told: one walk
    /// serves both, since telling a change reads the marking and changes
    /// none of it.
    pub(super) fn know_new(&mut self) -> HashSet<NetId> {
        let mut seen = Vec::new();
        let mut live = HashSet::new();
        // A flat net holds no inner net, now or ever.
        if self.holding.is_empty() {
            return live;
        }
        let outer = &self.net;
        let records = &self.inner;
        walk_all(outer, &mut |levels, way, _, net| {
            live.insert(net.id());
            if !records.contains_key(&net.id()) {
                let around = around_of(outer, levels);
                seen.push((net.id(), Inner::new(Some(net), path_of(way), true, around)));
            }
            None::<()>
        });
        self.inner.extend(seen);
        live
    }

    /// The net a request's `path` or `nested_id` names (PROTOCOL.md §12.1):
    /// both is `invalid_params`, and neither names the outer net, as the
    /// empty path does. A path of the wrong shape is `invalid_params`, read before
    /// anything is resolved ([`protocol::parse_path`]); a path or an id that
    /// names no live inner net is `unknown_net`, a path's message naming the
    /// step that failed.
    pub(super) fn resolve(&self, args: &Args) -> Result<Target, RpcError> {
        let path = optional(args, "path");
        let nested_id = optional(args, "nested_id");
        match (path, nested_id) {
            (None, None) => Ok(Target::Outer),
            (Some(_), Some(_)) => Err(RpcError::new(
                ErrorCode::InvalidParams,
                "give a path or a nested_id, not both",
            )),
            (Some(path), None) => {
                let steps = protocol::parse_path(path)
                    .map_err(|message| RpcError::new(ErrorCode::InvalidParams, message))?;
                resolve_path(&self.net, &steps)
                    .map(|found| found.map_or(Target::Outer, |(found, _)| Target::Inner(found)))
                    .map_err(|message| RpcError::new(ErrorCode::UnknownNet, message))
            }
            (None, Some(Value::String(wanted))) => {
                // Read once as the `u64` a `NetId` is, then compared as a
                // number on the walk. Only the spelling `Display` gives
                // names a net, so `"07"` and `"+7"` name none, as when the
                // ids were compared as text (PROTOCOL.md §12.1).
                let number = wanted
                    .parse::<u64>()
                    .ok()
                    .filter(|number| number.to_string() == *wanted);
                number
                    .and_then(|number| find(&self.net, |net| net.id().get() == number))
                    .map(|(found, _)| Target::Inner(found))
                    .ok_or_else(|| {
                        RpcError::new(
                            ErrorCode::UnknownNet,
                            format!("no live inner net has the nested id {wanted:?}"),
                        )
                    })
            }
            (None, Some(_)) => Err(RpcError::new(
                ErrorCode::InvalidParams,
                "\"nested_id\" must be a string",
            )),
        }
    }

    /// Makes sure the net `found` names has a record, its fireable set
    /// taken silently: whoever named it reads it in the same answer.
    pub(super) fn know(&mut self, found: &Found) {
        let net = descend(&self.net, &found.levels);
        match self.inner.get_mut(&found.nested_id) {
            Some(inner) => {
                if let Some(net) = net {
                    inner.see(net);
                }
                inner.last_path.clone_from(&found.path);
            }
            None => {
                let around = around_of(&self.net, &found.levels);
                self.inner.insert(
                    found.nested_id,
                    Inner::new(net, found.path.clone(), true, around),
                );
            }
        }
    }

    /// [`know_new`](Loaded::know_new), when `events`, a batch drained from
    /// the net's log, may have brought an inner net into the marking or
    /// taken one out of it ([`moves_nets`](Loaded::moves_nets)); `None`
    /// when they cannot have, and the records stand as they are.
    ///
    /// For a change the owner made itself, a step, a landing or an inject,
    /// whose every change of a token is in `events`. A change made by code
    /// the owner does not see into, a control's handler or the host
    /// program's, can put a net in place of another inside a token through
    /// `nested_scope` and log nothing of it, so it is followed by a whole
    /// walk.
    pub(super) fn know_new_after(&mut self, events: &[Event]) -> Option<HashSet<NetId>> {
        self.moves_nets(events).then(|| self.know_new())
    }

    /// Whether `events` may have brought an inner net into the marking or
    /// taken one out of it, at any depth: one of them took, put, added,
    /// removed or changed a token of a place that holds nets
    /// ([`holding`](super::holding)), in the net it happened in; or it is an
    /// import, or an event this runner does not know; or it happened in an
    /// inner net with no record yet. A firing inside an inner net that moves
    /// plain tokens, the commonest change of a nested net, moves no net.
    fn moves_nets(&self, events: &[Event]) -> bool {
        events.iter().any(|event| {
            let (holding, event) = match Lifted::of(event) {
                None => (&self.holding, event),
                Some(lifted) => match self.inner.get(&lifted.nested_id()) {
                    Some(record) if record.topology.is_some() => (&record.holding, lifted.event),
                    _ => return true,
                },
            };
            let holds = |pairs: &[(PlaceId, TokenId)]| {
                pairs.iter().any(|(place, _)| holding.contains(place))
            };
            match event {
                Event::Added { place, .. }
                | Event::Removed { place, .. }
                | Event::Mutated { place, .. } => holding.contains(place),
                Event::Fired {
                    consumed, produced, ..
                } => holds(consumed) || holds(produced),
                Event::Launched { consumed, .. } | Event::Failed { consumed, .. } => {
                    holds(consumed)
                }
                Event::Distributed { produced, .. } => holds(produced),
                Event::Toggled { .. } | Event::ParamChanged { .. } => false,
                _ => true,
            }
        })
    }

    /// Drops the records of inner nets whose nested ids no longer resolve:
    /// a firing consumed their token and did not put it back. `live` is
    /// what [`know_new`](Loaded::know_new) found, with the marking as it is
    /// now. Called when a firing lands and after an inner firing, never
    /// while a body holds a token, so that a vehicle in transit keeps its
    /// record and version.
    pub(super) fn prune(&mut self, live: &HashSet<NetId>) {
        self.inner.retain(|id, _| live.contains(id));
        self.touched.retain(|id| live.contains(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use petritype::{NetBuilder, TakeEdge};

    /// A `Yard` of bare nets, each with a `Todo -Work-> Done`, and a second
    /// level: a `Crate` place of bare nets inside each, whose `Straw` is
    /// its port.
    /// A yard of two workers, each with a crate of straw, the yard and the
    /// workers declared open: a closed net shields the nets inside it, and
    /// these tests reach the crates from outside.
    fn yard() -> (Net, Vec<NetId>) {
        fn worker(todo: u32) -> Net {
            let mut b = NetBuilder::new();
            let pending = b.place::<u32>("Todo");
            let done = b.place::<u32>("Done");
            b.seed(pending, 0..todo);
            b.transition("Work", (TakeEdge(pending),), (done,), |n: u32| (n,));
            let inside = b.place_nested::<Net>("Crate");
            let mut c = NetBuilder::new();
            let straw = c.place::<u32>("Straw");
            c.port(straw);
            b.seed(inside, [c.build().unwrap()]);
            b.open();
            b.build().unwrap()
        }
        let mut b = NetBuilder::new();
        let yard = b.place_nested::<Net>("Yard");
        b.seed(yard, [worker(1), worker(2)]);
        b.open();
        let net = b.build().unwrap();
        let mut ids = Vec::new();
        walk_all(&net, &mut |_, _, _, n| {
            ids.push(n.id());
            None::<()>
        });
        (net, ids)
    }

    /// The steps of a path, each `(place, token, net)`.
    fn steps(levels: &[(&str, &str, &str)]) -> Vec<PathStep> {
        levels
            .iter()
            .map(|&(place, token, net)| PathStep::Token {
                place: place.into(),
                token: token.into(),
                net: net.into(),
            })
            .collect()
    }

    #[test]
    fn the_walk_goes_down_before_it_goes_on() {
        let (net, ids) = yard();
        assert_eq!(ids.len(), 4, "two workers, a crate in each");
        let mut paths = Vec::new();
        walk_all(&net, &mut |_, way, _, _| {
            paths.push(path_of(way));
            None::<()>
        });
        assert_eq!(
            paths,
            [
                steps(&[("Yard", "0", "")]),
                // The crate is seeded after the work, so it has the next id.
                steps(&[("Yard", "0", ""), ("Crate", "1", "")]),
                steps(&[("Yard", "1", "")]),
                steps(&[("Yard", "1", ""), ("Crate", "2", "")]),
            ]
        );
    }

    /// A net is found by the path it was last seen at as a walk finds it:
    /// the same levels, path and name, by a path that still leads to it
    /// (the cheap way), and by a walk when the path leads to another net or
    /// to none, or names the place for a port, as an event lifted through
    /// it does.
    #[test]
    fn a_net_is_located_by_its_last_path_as_a_walk_finds_it() {
        let (mut net, ids) = yard();
        let walked = |net: &Net, id: NetId| {
            let (found, inner) = find(net, |inner| inner.id() == id).unwrap();
            (found.levels, found.path, found.name, inner.id())
        };
        let located = |net: &Net, id: NetId, path: &[PathStep]| {
            let (found, inner) = locate(net, id, Some(path)).unwrap();
            (found.levels, found.path, found.name, inner.id())
        };
        let crate_2 = steps(&[("Yard", "1", ""), ("Crate", "2", "")]);
        assert_eq!(located(&net, ids[3], &crate_2), walked(&net, ids[3]));
        assert_eq!(
            located(&net, ids[3], &steps(&[("Yard", "0", "")])),
            walked(&net, ids[3]),
            "a path to another net"
        );
        assert_eq!(
            located(&net, ids[3], &steps(&[("Yard", "7", "")])),
            walked(&net, ids[3]),
            "a path to no net"
        );
        // The first worker gone, the path to its crate leads nowhere, and
        // neither the path nor a walk finds the crate.
        let yard = net.place_named("Yard").unwrap();
        let (first, _) = net.tokens(yard).next().unwrap();
        net.remove(yard, first).unwrap();
        let crate_1 = steps(&[("Yard", "0", ""), ("Crate", "1", "")]);
        assert!(locate(&net, ids[1], Some(&crate_1)).is_none(), "gone");
        assert_eq!(located(&net, ids[3], &crate_1), walked(&net, ids[3]));

        // A bin in a port, which a walk reaches through its subnet and a
        // path through the place for the port names too.
        let mut b = NetBuilder::new();
        let berth = b.place_nested::<Net>("Berth");
        b.port(berth);
        b.seed(berth, [NetBuilder::new().build().unwrap()]);
        let dock = b.build().unwrap();
        let mut b = NetBuilder::new();
        let at = b.subnet_place("Dock", dock);
        b.port_of::<Net>(at, "Berth");
        let harbour = b.build().unwrap();
        let mut bins = Vec::new();
        walk_all(&harbour, &mut |_, _, _, inner| {
            bins.push(inner.id());
            None::<()>
        });
        let bin = bins[1];
        // An event lifted through the place for the port names the bin by
        // that place, which a path does not name.
        let through_the_window = steps(&[("Berth", "0", "")]);
        assert!(resolve_path(&harbour, &through_the_window).is_err());
        let (levels, path, ..) = located(&harbour, bin, &through_the_window);
        assert_eq!(
            path,
            [
                PathStep::Subnet {
                    place: "Dock".into()
                },
                steps(&[("Berth", "0", "")]).remove(0),
            ],
            "as the walk goes, through the subnet"
        );
        assert_eq!(levels, walked(&harbour, bin).0);
    }

    #[test]
    fn a_path_resolves_step_by_step_and_names_the_step_that_failed() {
        let (net, ids) = yard();
        let (found, inner) = resolve_path(&net, &steps(&[("Yard", "1", ""), ("Crate", "2", "")]))
            .unwrap()
            .unwrap();
        assert_eq!(found.nested_id, ids[3]);
        assert_eq!(inner.id(), ids[3]);
        assert_eq!(found.levels.len(), 2);
        assert_eq!(found.name, "", "a bare net is unnamed");
        assert!(
            resolve_path(&net, &[]).unwrap().is_none(),
            "the empty path is the loaded net"
        );
        for (path, says) in [
            (
                steps(&[("Lot", "0", "")]),
                "step 0 of the path: no place named \"Lot\"",
            ),
            (
                steps(&[("Yard", "9", "")]),
                "step 0 of the path: no token \"9\" in \"Yard\"",
            ),
            (steps(&[("Yard", "00", "")]), "no token \"00\""),
            (
                steps(&[("Yard", "0", "cabin")]),
                "holds no net named \"cabin\"",
            ),
            (
                steps(&[("Yard", "0", ""), ("Crate", "7", "")]),
                "step 1 of the path: no token \"7\" in \"Crate\"",
            ),
            (
                steps(&[("Yard", "0", ""), ("Todo", "0", "")]),
                "step 1 of the path: place \"Todo\" holds no nets",
            ),
            (
                vec![PathStep::Subnet {
                    place: "Yard".into(),
                }],
                "step 0 of the path: place \"Yard\" is not a subnet place",
            ),
        ] {
            let error = resolve_path(&net, &path).err().unwrap();
            assert!(error.contains(says), "{path:?}: {error}");
        }
    }

    #[test]
    fn descending_mutably_lifts_at_every_level() {
        let (mut net, ids) = yard();
        let (found, _) = find(&net, |n| n.id() == ids[1]).unwrap();
        let added = descend_mut(&mut net, &found.levels, |crate_net| {
            let straw = crate_net.place_ref::<u32>("Straw").unwrap();
            crate_net.put(straw, 7)
        })
        .unwrap();
        let events: Vec<Event> = net.drain_events().collect();
        assert_eq!(events.len(), 1);
        let lifted = Lifted::of(&events[0]).unwrap();
        assert_eq!(lifted.chain(), [ids[0], ids[1]]);
        assert!(matches!(lifted.event, Event::Added { token, .. } if *token == added));
        assert!(lifted.live(&net).is_some_and(|n| n.id() == ids[1]));
    }
}
