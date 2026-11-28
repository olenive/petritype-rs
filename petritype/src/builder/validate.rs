//! What [`build`](NetBuilder::build) checks a net's declarations for,
//! in the order its docs give, and the net it makes of them.

use std::collections::HashSet;

use crate::edge::EdgeKind;
use crate::erased::RoutingProblem;
use crate::error::{BuildError, Named};
use crate::ids::{PlaceId, TransitionId};
use crate::index::KeyIndex;
use crate::nested::forget_inner_events;
use crate::net::Net;
use crate::param::ParamSlot;
use crate::place::Window;
use crate::transition::ArgSource;

use super::cycles::Node;
use super::{GroupProblem, KeysProblem, NetBuilder};

impl NetBuilder {
    /// Checks the declarations and produces the net.
    ///
    /// The fresh net holds the seeded marking, an empty event log, and every
    /// counter at zero: nothing has happened to it yet.
    ///
    /// # Errors
    ///
    /// The first problem found, the checks made in this order.
    ///
    /// First, the first thing wrong with a port or an option given to one:
    /// [`BuildError::NoSuchPort`] or [`BuildError::PortTypeMismatch`] for a
    /// [`port_of`](NetBuilder::port_of) that named no port or the wrong
    /// type, [`BuildError::PortIsSubnetPlace`] for a
    /// [`port`](NetBuilder::port) given a port of a subnet place, and
    /// [`BuildError::PortOption`] for a [`keys`](NetBuilder::keys), a
    /// [`style`](NetBuilder::style) or another option that says what a
    /// place is, given to such a port (since 0.4.0). Then the names:
    /// [`BuildError::EmptyName`] for the first name
    /// given as the empty string, of a place, a subnet place, a key, a
    /// transition, a group, a control, a parameter's wire name or a
    /// template, and [`BuildError::NoKeys`] for a [`keys`](NetBuilder::keys)
    /// declaration with no key (both since 2026-11-28); then
    /// [`BuildError::DuplicatePlaceName`] if two places share a name (a
    /// port's place is named by its subnet place and its port, and does not
    /// count). Then the places' options: [`BuildError::NotNested`] if a
    /// place given a [`stepping`](NetBuilder::stepping) rule holds no nets,
    /// and [`BuildError::DuplicateKeyName`] or
    /// [`BuildError::KeysAlreadyDeclared`] for the first
    /// [`keys`](NetBuilder::keys) declaration that repeats a name or a
    /// place.
    ///
    /// Then each transition in declaration order, and for each:
    /// [`BuildError::DuplicateTransitionName`] if an earlier one has its
    /// name; [`BuildError::EdgeOnSubnetPlace`] if it has an edge on a subnet
    /// place, which the typed API already refuses when it compiles;
    /// [`BuildError::EdgeCannotSharePlace`] if it reads one place through
    /// two or more input edges and one of them is neither a
    /// [`TakeEdge`](crate::TakeEdge) nor a
    /// [`TakeWhereEdge`](crate::TakeWhereEdge) (since 0.4.0, when two takes
    /// came to share a place; until then any two edges on one place were
    /// refused); then, edge by edge,
    /// [`BuildError::SnapshotOfNoClonePlace`] if a snapshot or a survey edge
    /// reads a place that cannot clone its tokens,
    /// [`BuildError::RestoreOfNoClonePlace`] if the transition
    /// [restores its inputs on failure](crate::TransitionBuilder::restore_tokens_on_failure)
    /// and the edge consumes such a place, and
    /// [`BuildError::KeyedEdgeOnUnkeyedPlace`] if a keyed edge
    /// ([`TakeWhereEdge`](crate::TakeWhereEdge),
    /// [`MutateWhereEdge`](crate::MutateWhereEdge),
    /// [`ReadWhereEdge`](crate::ReadWhereEdge)) reads a place that declares
    /// no [keys](NetBuilder::keys); then, edge by edge,
    /// [`BuildError::ReachIntoNoNets`] for an edge that names ports it
    /// [reaches](crate::Reaching) on a place whose tokens carry no nets, and
    /// [`BuildError::EmptyReachName`] for a port named by the empty string;
    /// then, for a keyed edge that chooses its token by another input's
    /// ([`Clause::IsInput`](crate::Clause::IsInput)),
    /// [`BuildError::BoundToTwoSources`],
    /// [`BuildError::BoundSourceNotAnInput`],
    /// [`BuildError::BoundPlaceShared`],
    /// [`BuildError::BoundSourceGivesMany`],
    /// [`BuildError::BoundSourceIsBound`] or
    /// [`BuildError::BoundKeyNotDeclared`]; and last
    /// [`BuildError::DuplicateOutputPlace`] if two of its output edges go to
    /// one place (since 0.4.0).
    ///
    /// Then [`BuildError::DuplicateControlName`] if two
    /// [controls](NetBuilder::control) share a name, and then the
    /// [groups](NetBuilder::group): [`BuildError::DuplicateGroupName`] if
    /// two share a name, and then, for the first node put in a group
    /// wrongly, [`BuildError::PlaceInTwoGroups`] or
    /// [`BuildError::TransitionInTwoGroups`] for one put in a second group
    /// and [`BuildError::PortInGroup`] for a port of a subnet place.
    ///
    /// Then the parameters: with the `serde` feature,
    /// `BuildError::DuplicateParamName` if two parameter types were marked
    /// serialisable with `NetBuilder::serializable_param` under one wire
    /// name, and [`BuildError::MissingParam`] if a transition declared a
    /// [`Param<T>`](crate::Param) and no [`param`](NetBuilder::param) value
    /// of that type was registered: the first offender in transition order,
    /// then in the order the inputs were written.
    ///
    /// Then, for the first transition whose body
    /// [routes](crate::RoutedOutput) its outputs by type and cannot,
    /// [`BuildError::UnroutableOutput`], [`BuildError::AmbiguousOutput`] or
    /// [`BuildError::UnreachableOutput`].
    ///
    /// Last, only if they were asked for,
    /// [`BuildError::SourceTransition`] from
    /// [`assert_no_source_transitions`](NetBuilder::assert_no_source_transitions)
    /// and [`BuildError::Cycle`] from
    /// [`assert_acyclic`](NetBuilder::assert_acyclic).
    pub fn build(mut self) -> Result<Net, BuildError> {
        // A port asked for wrongly first: the handle it gave out names a
        // stand-in, which every later check would only stumble on.
        if let Some(problem) = self.port_problem.take() {
            return Err(problem);
        }
        // No name is the empty string (since 2026-11-28), and no keys
        // declaration names no key: before the duplicates, since a name
        // given twice as "" is a duplicate as well.
        if let Some(what) = self.empty_name() {
            return Err(BuildError::EmptyName { what });
        }
        if let Some(place) = self.no_keys {
            return Err(BuildError::NoKeys {
                place: self.places[place.0 as usize].meta.name.clone(),
            });
        }
        let mut seen = HashSet::with_capacity(self.places.len());
        // A window is named by its subnet place and its port, so its name
        // is unique among its subnet's ports, not among this net's places.
        for slot in self.places.iter().filter(|slot| slot.meta.window.is_none()) {
            if !seen.insert(slot.meta.name.as_str()) {
                return Err(BuildError::DuplicatePlaceName {
                    name: slot.meta.name.clone(),
                });
            }
        }
        if let Some(place) = self.not_nested {
            return Err(BuildError::NotNested {
                place: self.places[place.0 as usize].meta.name.clone(),
            });
        }
        match self.keys_problem {
            Some(KeysProblem::Duplicate { place, name }) => {
                return Err(BuildError::DuplicateKeyName {
                    place: self.places[place.0 as usize].meta.name.clone(),
                    name: name.to_owned(),
                });
            }
            Some(KeysProblem::Again { place }) => {
                return Err(BuildError::KeysAlreadyDeclared {
                    place: self.places[place.0 as usize].meta.name.clone(),
                });
            }
            None => {}
        }

        let mut seen = HashSet::with_capacity(self.transitions.len());
        // Each transition's bound edges by their sources, found below with
        // its other checks and kept once every transition has passed.
        let mut bindings = Vec::with_capacity(self.transitions.len());
        for slot in &self.transitions {
            if !seen.insert(slot.name.as_str()) {
                return Err(BuildError::DuplicateTransitionName {
                    name: slot.name.clone(),
                });
            }
            // A subnet place holds no tokens, so an edge on it would read
            // or write nothing. The typed API cannot name one (a
            // `SubnetRef` is no `PlaceRef`), so this is the defence behind
            // that.
            if let Some(place) = slot
                .inputs
                .iter()
                .map(|(place, _)| place)
                .chain(&slot.outputs)
                .find(|place| self.places[place.0 as usize].meta.subnet)
            {
                return Err(BuildError::EdgeOnSubnetPlace {
                    transition: slot.name.clone(),
                    place: self.places[place.0 as usize].meta.name.clone(),
                });
            }
            // Several edges on one place only where each takes a token of
            // its own. Both halves of firing rely on it: fireability counts
            // what the takes on a place need together, and the borrows a
            // mutating or a read edge hands the body are disjoint by
            // construction, each such edge alone on its place, rather than
            // by a run-time check.
            crate::sharing::check(slot, &self.places)?;
            for (place, kind) in &slot.inputs {
                // A snapshot needs the place's cloner, which
                // `place_no_clone` did not record (whether or not the type
                // itself is `Clone`), and a survey is a snapshot of every
                // token. A nested place is such a place, so this is also
                // what refuses a snapshot or a survey of one.
                if matches!(kind, EdgeKind::Snapshot | EdgeKind::Survey)
                    && self.places[place.0 as usize].meta.clone.is_none()
                {
                    return Err(BuildError::SnapshotOfNoClonePlace {
                        transition: slot.name.clone(),
                        place: self.places[place.0 as usize].meta.name.clone(),
                    });
                }
                // Restoring is done with clones taken at stage one, so a
                // transition that restores can only consume places that can
                // clone, a drain's included, empty or not: the check is about
                // the edge, not the marking. The non-consuming edges are exempt
                // because they consume nothing: a mutating or a read edge has
                // nothing to put back, and a snapshot or a survey of such a place
                // was refused just above.
                if slot.restore_tokens_on_failure
                    && kind.consumes()
                    && self.places[place.0 as usize].meta.clone.is_none()
                {
                    return Err(BuildError::RestoreOfNoClonePlace {
                        transition: slot.name.clone(),
                        place: self.places[place.0 as usize].meta.name.clone(),
                    });
                }
                // A keyed edge chooses by keys, and a place that declares
                // none gives no token a key, so no clause could ever pass:
                // the transition would never fire, which is cheaper to say
                // now than to find out by watching it.
                if kind.is_keyed() && self.places[place.0 as usize].meta.keys.is_none() {
                    return Err(BuildError::KeyedEdgeOnUnkeyedPlace {
                        transition: slot.name.clone(),
                        place: self.places[place.0 as usize].meta.name.clone(),
                    });
                }
            }
            // A reach is declared into the nets a place's tokens carry, so a
            // place that holds none has nothing to reach, and a name is not
            // empty. Nothing else about a reach is checked (DESIGN §8.5).
            for ((place, _), reach) in slot.inputs.iter().zip(slot.reaches.iter()) {
                if reach.is_empty() {
                    continue;
                }
                let meta = &self.places[place.0 as usize].meta;
                if meta.nested.is_none() {
                    return Err(BuildError::ReachIntoNoNets {
                        transition: slot.name.clone(),
                        place: meta.name.clone(),
                    });
                }
                if reach.iter().any(String::is_empty) {
                    return Err(BuildError::EmptyReachName {
                        transition: slot.name.clone(),
                        place: meta.name.clone(),
                    });
                }
            }
            // A keyed edge that chooses by another input's token, and that
            // input: one pair for each firing to find, or a refusal.
            bindings.push(crate::pairing::bindings(slot, &self.places, self.net)?);
            // One output edge per place: an output edge is one slot of what
            // the body returns, and one slot carries any number of tokens.
            let mut written = HashSet::with_capacity(slot.outputs.len());
            if let Some(place) = slot.outputs.iter().find(|place| !written.insert(**place)) {
                return Err(BuildError::DuplicateOutputPlace {
                    transition: slot.name.clone(),
                    place: self.places[place.0 as usize].meta.name.clone(),
                });
            }
        }
        for (slot, bindings) in self.transitions.iter_mut().zip(bindings) {
            slot.bindings = bindings;
        }

        let mut seen = HashSet::with_capacity(self.controls.len());
        for control in &self.controls {
            if !seen.insert(control.name()) {
                return Err(BuildError::DuplicateControlName {
                    name: control.name().to_owned(),
                });
            }
        }

        // The groups, a naming matter like the controls: a name two groups
        // share, then the first node put in a group wrongly.
        let mut seen = HashSet::with_capacity(self.groups.len());
        for name in &self.groups {
            if !seen.insert(name.as_str()) {
                return Err(BuildError::DuplicateGroupName { name: name.clone() });
            }
        }
        if let Some(problem) = self.group_problem {
            return Err(self.group_error(problem));
        }

        // The parameters, in the same spirit: a wire name two types share is
        // a naming mistake like a duplicate place name, and a declared
        // parameter with no value is this slice's version of a dangling
        // edge. Both come before routing, which is the subtlest check of
        // the three.
        #[cfg(feature = "serde")]
        {
            let mut seen = HashSet::with_capacity(self.params.len());
            for entry in &self.params {
                let Some(wire) = &entry.wire else {
                    continue;
                };
                if !seen.insert(wire.name.as_str()) {
                    return Err(BuildError::DuplicateParamName {
                        name: wire.name.clone(),
                    });
                }
            }
        }
        for slot in &self.transitions {
            for arg in &slot.args {
                let ArgSource::Param { type_id, type_name } = *arg else {
                    continue;
                };
                // A marked-but-valueless parameter is not a value, so it is
                // reported here like one that was never mentioned at all.
                if !self
                    .params
                    .iter()
                    .any(|entry| entry.type_id == type_id && entry.value.is_some())
                {
                    return Err(BuildError::MissingParam {
                        transition: slot.name.clone(),
                        type_name,
                    });
                }
            }
        }

        // Routing last: a net whose transitions are not yet distinct or
        // whose edges are wrong has a more basic problem to report first.
        // Each transition resolved its own table when it was declared; this
        // is where the index an unreachable output was known by becomes the
        // place's name.
        for (index, problem) in self.routing.iter().enumerate() {
            let Some(problem) = problem else {
                continue;
            };
            let slot = &self.transitions[index];
            let transition = slot.name.clone();
            return Err(match *problem {
                RoutingProblem::Unroutable { type_name } => BuildError::UnroutableOutput {
                    transition,
                    type_name,
                },
                RoutingProblem::Ambiguous { type_name } => BuildError::AmbiguousOutput {
                    transition,
                    type_name,
                },
                RoutingProblem::Unreachable { output } => BuildError::UnreachableOutput {
                    transition,
                    place: self.places[slot.outputs[output].0 as usize]
                        .meta
                        .name
                        .clone(),
                },
            });
        }

        // The opt-in checks last: a net that is wired wrongly has a more
        // basic problem to report than one that is wired for ever. The
        // per-transition complaint comes before the whole-net one, being the
        // easier of the two to act on.
        if self.assert_no_source_transitions {
            for slot in &self.transitions {
                // A drain consumes, but may take nothing: only an edge that
                // always takes a token makes a firing pay for itself.
                if !slot.inputs.iter().any(|(_, kind)| kind.always_consumes()) {
                    return Err(BuildError::SourceTransition {
                        transition: slot.name.clone(),
                    });
                }
            }
        }
        if self.assert_acyclic
            && let Some(path) = self.find_cycle()
        {
            return Err(BuildError::Cycle { path });
        }

        // The two halves of each parameter meet here. An entry with no value
        // has already been answered for: no transition declared it, or the
        // check above would have refused the net, so it is dropped with the
        // builder.
        let params: Vec<ParamSlot> = self
            .params
            .into_iter()
            .filter_map(|entry| {
                let (value, clone) = entry.value?;
                #[cfg_attr(not(feature = "serde"), allow(unused_mut))]
                let mut slot = ParamSlot::new(entry.type_id, entry.type_name, value, clone);
                #[cfg(feature = "serde")]
                if let Some(wire) = &entry.wire {
                    slot.name = Some(wire.name.clone());
                    slot.to_json = Some(wire.to_json);
                    slot.from_json = Some(wire.from_json);
                }
                Some(slot)
            })
            .collect();

        // Which transitions reach each subnet through its windows: while one
        // of them is in flight, the subnet takes no turn.
        for (index, transition) in self.transitions.iter().enumerate() {
            let reached: Vec<Window> = transition
                .inputs
                .iter()
                .map(|(place, _)| place)
                .chain(&transition.outputs)
                .filter_map(|place| self.places[place.0 as usize].meta.window)
                .collect();
            for window in reached {
                let subnet = self.places[window.subnet.0 as usize]
                    .subnet
                    .as_mut()
                    .expect("a window's place holds a subnet");
                let id = TransitionId(index as u32);
                if !subnet.through.contains(&id) {
                    subnet.through.push(id);
                }
            }
        }

        let holds_nets = self.places.iter().any(|slot| {
            slot.meta.window.is_none() && (slot.meta.nested.is_some() || slot.meta.subnet)
        });
        let mut net = Net {
            holds_nets,
            id: self.net,
            open: self.open,
            held: None,
            shield: None,
            inside: false,
            places: self.places,
            transitions: self.transitions,
            params,
            next_token_id: 0,
            topology_counter: 0,
            state_counter: 0,
            fired_count: 0,
            selector: None,
            tie_break: self.tie_break,
            last_pick: None,
            ring_cursor: None,
            resettable: self.resettable,
            label_limit: self.label_limit,
            events: Vec::new(),
            controls: self.controls,
            groups: self.groups,
            // Filled by the seeds below, which go in through the one
            // insertion path like every other token.
            index: KeyIndex::new(),
            #[cfg(feature = "serde")]
            stamp: None,
            #[cfg(feature = "serde")]
            templates: self.templates,
        };
        for (index, tokens) in self.seeds.into_iter().enumerate() {
            let place = PlaceId(index as u32);
            let nested = net.places[index].meta.nested;
            for mut token in tokens {
                // Whatever a seeded token's inner nets logged while they were
                // being set up is setup too (DESIGN §6.6).
                if let Some(nested) = nested {
                    forget_inner_events(nested, &mut token);
                }
                // `insert_erased`, not `push_erased`: the initial marking is
                // where the net starts, not a change to report.
                net.insert_erased(place, token, "<seed>")
                    .expect("a seed is checked against its place's type by the builder");
            }
        }
        Ok(net)
    }

    // ----- internals ------------------------------------------------------

    /// The build error a recorded group problem is, with every name filled
    /// in.
    fn group_error(&self, problem: GroupProblem) -> BuildError {
        let group = |index: u32| self.groups[index as usize].clone();
        match problem {
            GroupProblem::Twice {
                node: Node::Place(place),
                first,
                second,
            } => BuildError::PlaceInTwoGroups {
                place: self.places[place as usize].meta.name.clone(),
                first: group(first),
                second: group(second),
            },
            GroupProblem::Twice {
                node: Node::Transition(transition),
                first,
                second,
            } => BuildError::TransitionInTwoGroups {
                transition: self.transitions[transition as usize].name.clone(),
                first: group(first),
                second: group(second),
            },
            GroupProblem::Port { place, group: at } => {
                let meta = &self.places[place.0 as usize].meta;
                let subnet = meta
                    .window
                    .map(|window| self.places[window.subnet.0 as usize].meta.name.clone())
                    .unwrap_or_default();
                BuildError::PortInGroup {
                    place: meta.name.clone(),
                    subnet,
                    group: group(at),
                }
            }
        }
    }

    /// What the first name given as the empty string names, in the order
    /// [`BuildError::EmptyName`] gives: places and subnet places, keys,
    /// transitions, groups, controls, parameters' wire names, templates. A
    /// port's place is named by its subnet, whose own build refused an empty
    /// name, and is passed over.
    fn empty_name(&self) -> Option<Named> {
        let own = || self.places.iter().filter(|slot| slot.meta.window.is_none());
        if let Some(slot) = own().find(|slot| slot.meta.name.is_empty()) {
            return Some(if slot.meta.subnet {
                Named::SubnetPlace
            } else {
                Named::Place
            });
        }
        if let Some(place) = self.empty_key {
            return Some(Named::Key {
                place: self.places[place.0 as usize].meta.name.clone(),
            });
        }
        if self.transitions.iter().any(|slot| slot.name.is_empty()) {
            return Some(Named::Transition);
        }
        if self.groups.iter().any(String::is_empty) {
            return Some(Named::Group);
        }
        if self
            .controls
            .iter()
            .any(|control| control.name().is_empty())
        {
            return Some(Named::Control);
        }
        #[cfg(feature = "serde")]
        {
            if let Some(entry) = self
                .params
                .iter()
                .find(|entry| entry.wire.as_ref().is_some_and(|wire| wire.name.is_empty()))
            {
                return Some(Named::Parameter {
                    type_name: entry.type_name,
                });
            }
            if self.templates.names().any(str::is_empty) {
                return Some(Named::Template);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::PlaceRef;

    #[test]
    fn duplicate_names_are_rejected() {
        let mut b = NetBuilder::new();
        let _ = b.place::<u32>("Same");
        let _ = b.place::<String>("Same");
        let error = b.build().unwrap_err();
        assert!(matches!(error, BuildError::DuplicatePlaceName { ref name } if name == "Same"));
    }

    /// An edge on a subnet place cannot be written with the typed API, since
    /// a `SubnetRef` is no `PlaceRef`, so the check behind that is asked
    /// with a handle made in here.
    #[test]
    fn an_edge_on_a_subnet_place_is_refused() {
        use crate::TakeEdge;
        for output in [false, true] {
            let mut b = NetBuilder::new();
            let inbox = b.place::<u32>("Inbox");
            let level = b.subnet_place("Level 3", NetBuilder::new().build().unwrap());
            let forged = PlaceRef::<u32>::new(b.id(), level.id());
            if output {
                b.transition("Stuff", (TakeEdge(inbox),), (forged,), |n: u32| (n,));
            } else {
                b.transition("Rob", (TakeEdge(forged),), (inbox,), |n: u32| (n,));
            }
            let error = b.build().expect_err("a subnet place holds no tokens");
            assert!(
                matches!(&error, BuildError::EdgeOnSubnetPlace { place, .. } if place == "Level 3"),
                "{error}"
            );
        }
    }

    #[test]
    fn seeds_are_numbered_in_place_order() {
        let mut b = NetBuilder::new();
        let first = b.place::<u32>("First");
        let second = b.place::<u32>("Second");
        b.seed(second, [10, 11]);
        b.seed(first, [1]);
        let net = b.build().unwrap();

        let ids: Vec<_> = net
            .tokens(first.id())
            .chain(net.tokens(second.id()))
            .map(|(id, _)| id)
            .collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
        assert_eq!(net.count(first.id()), 1);
        assert_eq!(net.count(second.id()), 2);
    }
}
