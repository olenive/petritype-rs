//! Declaring a net. The builder is the only place where the concrete token
//! type of each place is still known, so it is where the per-place vtable is
//! filled in.

use std::any::{TypeId, type_name};
use std::collections::{HashSet, VecDeque};
use std::fmt;

use crate::body::{OutputPlaces, TransitionBody};
use crate::edge::{EdgeKind, InputEdges};
use crate::erased::RoutingProblem;
use crate::error::BuildError;
use crate::ids::{NetId, PlaceId, PlaceRef, TransitionId, foreign_place};
use crate::net::Net;
use crate::place::{CloneFn, DebugFn, ErasedToken, PlaceMeta, PlaceSlot, clone_of, debug_of};
use crate::sealed::{BodySealed, EdgeTupleSealed, PlaceTupleSealed};
use crate::transition::{TransitionBuilder, TransitionSlot};

/// How many characters a label keeps by default, as in Python petritype.
const DEFAULT_LABEL_LIMIT: usize = 100;

/// Declares the places of a net and builds it.
///
/// ```
/// use petritype::NetBuilder;
///
/// #[derive(Debug, Clone, PartialEq)]
/// struct Parcel { id: u32 }
///
/// let mut b = NetBuilder::new();
/// let inbox = b.place::<Parcel>("Inbox");     // PlaceRef<Parcel>
/// b.seed(inbox, [Parcel { id: 1 }]);
/// let net = b.build()?;
///
/// assert_eq!(net.count(inbox.id()), 1);
/// # Ok::<(), petritype::BuildError>(())
/// ```
///
/// Places do not chain: `place` returns the typed handle you need for
/// everything else, and the few per-place options take that handle
/// ([`seed`](NetBuilder::seed), and `serializable` with the `serde`
/// feature).
pub struct NetBuilder {
    /// Stamped into every handle this builder hands out, and into the net it
    /// builds. Minted here, once, so that a handle can say which net it
    /// belongs to.
    net: NetId,
    /// Places in declaration order; the index is the [`PlaceId`].
    places: Vec<PlaceSlot>,
    /// Transitions in declaration order; the index is the
    /// [`TransitionId`].
    transitions: Vec<TransitionSlot>,
    /// Initial tokens per place, parallel to `places`. They are erased values
    /// without ids: ids are minted by [`build`](NetBuilder::build) so that a
    /// half-built net never hands out ids the net will not have.
    seeds: Vec<Vec<ErasedToken>>,
    /// What resolving each transition's routed outputs found, parallel to
    /// `transitions`, and `None` for the transitions that had nothing to
    /// resolve.
    ///
    /// Routing itself lives in the erased body, which captured the table it
    /// resolved; only the complaint has to wait, because
    /// [`transition`](NetBuilder::transition) is infallible and
    /// [`build`](NetBuilder::build) is where a wiring mistake is reported.
    /// It stays on the builder rather than on the transition so that the net
    /// carries nothing that was only ever a question at build time.
    routing: Vec<Option<RoutingProblem>>,
    label_limit: usize,
}

impl NetBuilder {
    /// A builder with no places, no transitions, a fresh [`NetId`] and the
    /// default label limit of 100 characters.
    pub fn new() -> Self {
        NetBuilder {
            net: NetId::mint(),
            places: Vec::new(),
            transitions: Vec::new(),
            seeds: Vec::new(),
            routing: Vec::new(),
            label_limit: DEFAULT_LABEL_LIMIT,
        }
    }

    /// The id of the net being built, as [`Net::id`] will report it. Every
    /// handle this builder hands out carries it.
    pub fn id(&self) -> NetId {
        self.net
    }

    /// Declares a place holding tokens of type `T`.
    ///
    /// The bound is the universal token bound: `Debug` for labels, `Clone` for
    /// snapshots (never called unless something asks for one), `Send + Sync`
    /// because bodies may run on another thread and the net may live in a
    /// `Resource`, and `'static` because tokens are owned values, not borrows.
    /// A type that cannot be cloned goes in a
    /// [`place_no_clone`](NetBuilder::place_no_clone).
    ///
    /// The returned [`PlaceRef<T>`] is `Copy`: hold it in a local and use it
    /// as often as you like.
    pub fn place<T>(&mut self, name: impl Into<String>) -> PlaceRef<T>
    where
        T: fmt::Debug + Clone + Send + Sync + 'static,
    {
        self.insert::<T>(name.into(), Some(clone_of::<T> as CloneFn))
    }

    /// Declares a place holding tokens that cannot be cloned — a `File`, a
    /// socket, a connection handle.
    ///
    /// Everything else works as in [`place`](NetBuilder::place). What such a
    /// place gives up is every feature built on cloning a token: snapshot
    /// edges, restoring tokens on failure, and whole-net snapshots.
    pub fn place_no_clone<T>(&mut self, name: impl Into<String>) -> PlaceRef<T>
    where
        T: fmt::Debug + Send + Sync + 'static,
    {
        self.insert::<T>(name.into(), None)
    }

    /// Gives a place its initial tokens. Repeated calls append.
    ///
    /// Seeded tokens get their ids at [`build`](NetBuilder::build) time, in
    /// place order, so they are the lowest ids in the net and every later
    /// token sorts after them.
    ///
    /// Seeding is setup, not change: it emits no events and leaves
    /// [`Net::state_counter`] at zero. A view attaching to a fresh net reads
    /// the initial marking with [`Net::places`] and [`Net::tokens`], and only
    /// then follows [`Net::drain_events`] for what happens next.
    ///
    /// # Panics
    ///
    /// If `place` came from a different builder.
    pub fn seed<T, I>(&mut self, place: PlaceRef<T>, tokens: I)
    where
        T: Send + Sync + 'static,
        I: IntoIterator<Item = T>,
    {
        let index = self.index_of(place, "seed");
        self.seeds[index].extend(
            tokens
                .into_iter()
                .map(|token| Box::new(token) as ErasedToken),
        );
    }

    /// Records how to convert this place's tokens to and from JSON, enabling
    /// [`Net::json`] for them.
    ///
    /// Serialisation is opt-in per place: most tokens are never inspected
    /// structurally, and requiring `Serialize` on every token type would tax
    /// every author for a feature few need.
    ///
    /// # Panics
    ///
    /// If `place` came from a different builder.
    #[cfg(feature = "serde")]
    pub fn serializable<T>(&mut self, place: PlaceRef<T>)
    where
        T: serde::Serialize + serde::de::DeserializeOwned + Send + Sync + 'static,
    {
        let index = self.index_of(place, "serializable");
        let meta = &mut self.places[index].meta;
        meta.to_json = Some(crate::place::to_json_of::<T> as crate::place::ToJsonFn);
        meta.from_json = Some(crate::place::from_json_of::<T> as crate::place::FromJsonFn);
    }

    /// Declares a transition: what it reads, what it writes, and the
    /// ordinary Rust closure that turns the first into the second.
    ///
    /// ```
    /// # use petritype::{NetBuilder, SnapshotEdge, TakeAllEdge};
    /// # #[derive(Debug, Clone)] struct Parcel { weight: f32 }
    /// # #[derive(Debug, Clone)] struct Config { max_weight: f32 }
    /// let mut b = NetBuilder::new();
    /// let inbox = b.place::<Parcel>("Inbox");
    /// let config = b.place::<Config>("Config");
    /// let heavy = b.place::<Parcel>("Heavy");
    /// let light = b.place::<Parcel>("Light");
    /// let count = b.place::<u32>("Count");
    ///
    /// let weigh = b.transition(
    ///     "Weigh",
    ///     (TakeAllEdge(inbox), SnapshotEdge(config)),
    ///     (heavy, light, count),
    ///     |parcels: Vec<Parcel>, cfg: Config| {
    ///         let (h, l): (Vec<_>, Vec<_>) =
    ///             parcels.into_iter().partition(|p| p.weight > cfg.max_weight);
    ///         let n = (h.len() + l.len()) as u32;
    ///         (h, l, Some(n))
    ///     },
    /// )
    /// .id();
    /// ```
    ///
    /// The three type-checked relationships are:
    ///
    /// - one body parameter per input edge, of the type that edge kind
    ///   produces ([`InputEdge`](crate::InputEdge));
    /// - one returned slot per output place, holding one token, zero or one,
    ///   or many ([`IntoTokens`](crate::IntoTokens)) — unless the body
    ///   returns a value that is not a tuple, which is
    ///   [routed](crate::RoutedOutput) to the output place of its type
    ///   instead, in which case the order of the outputs does not matter;
    /// - the return value as a whole is the outputs, or a `Result` of them
    ///   ([`IntoOutcome`](crate::IntoOutcome)).
    ///
    /// Annotate the closure's parameter types. Inference through the arity
    /// traits is not reliable, and the annotation doubles as documentation of
    /// what flows along each edge.
    ///
    /// Declaring a transition is setup, not change: it emits no events and
    /// leaves every counter at zero.
    ///
    /// # Panics
    ///
    /// If any edge or output place came from another net.
    pub fn transition<Edges, Outputs, Body, Marker>(
        &mut self,
        name: impl Into<String>,
        inputs: Edges,
        outputs: Outputs,
        body: Body,
    ) -> TransitionBuilder<'_>
    where
        Edges: InputEdges,
        Outputs: OutputPlaces,
        Body: TransitionBody<Edges, Outputs, Marker>,
    {
        let mut described = Vec::new();
        EdgeTupleSealed::describe(&inputs, &mut described);
        let mut edges = Vec::with_capacity(described.len());
        // Detachable until proven otherwise: one mutating edge is enough to
        // tie the firing to the net.
        let mut detachable = true;
        for (place, kind, net) in described {
            self.index_of_erased(place, net, "transition");
            detachable &= kind != EdgeKind::Mutate;
            edges.push((place, kind));
        }

        let mut described = Vec::new();
        PlaceTupleSealed::describe(&outputs, &mut described);
        let mut targets = Vec::with_capacity(described.len());
        for (place, net) in described {
            self.index_of_erased(place, net, "transition");
            targets.push(place);
        }

        // Wrapping the body also resolves its routing, if it has any; the
        // problem it may find is kept until `build`.
        let (body, problem) = BodySealed::into_erased(body);
        let id = TransitionId(self.transitions.len() as u32);
        self.routing.push(problem);
        self.transitions.push(TransitionSlot {
            name: name.into(),
            inputs: edges,
            outputs: targets,
            // The one place where both the typed body and the erased engine
            // are in scope, so the one place the wrapper can be built.
            body: Some(body),
            guard: None,
            priority: None,
            enabled: true,
            detachable,
        });
        TransitionBuilder { builder: self, id }
    }

    /// Sets the maximum number of characters [`Net::label`] returns before
    /// truncating. The default is 100.
    pub fn label_limit(&mut self, limit: usize) {
        self.label_limit = limit;
    }

    /// Checks the declarations and produces the net.
    ///
    /// The fresh net holds the seeded marking, an empty event log, and every
    /// counter at zero: nothing has happened to it yet.
    ///
    /// # Errors
    ///
    /// [`BuildError::DuplicatePlaceName`] or
    /// [`BuildError::DuplicateTransitionName`] if two of either share a name;
    /// [`BuildError::SnapshotOfNoClonePlace`] if a snapshot edge reads a
    /// place that cannot clone its tokens;
    /// [`BuildError::DuplicateInputPlace`] if one transition reads one place
    /// through two input edges; and, for a transition whose body
    /// [routes](crate::RoutedOutput) its outputs by type,
    /// [`BuildError::UnroutableOutput`], [`BuildError::AmbiguousOutput`] or
    /// [`BuildError::UnreachableOutput`].
    pub fn build(self) -> Result<Net, BuildError> {
        let mut seen = HashSet::with_capacity(self.places.len());
        for slot in &self.places {
            if !seen.insert(slot.meta.name.as_str()) {
                return Err(BuildError::DuplicatePlaceName {
                    name: slot.meta.name.clone(),
                });
            }
        }

        let mut seen = HashSet::with_capacity(self.transitions.len());
        for slot in &self.transitions {
            if !seen.insert(slot.name.as_str()) {
                return Err(BuildError::DuplicateTransitionName {
                    name: slot.name.clone(),
                });
            }
            // One input edge per place per transition. Both halves of
            // firing rely on it: "fireable iff every input place is
            // non-empty" is only exact while one edge wants one place, and
            // the `&mut` borrows a mutating edge hands the body are disjoint
            // by construction rather than by a run-time check.
            let mut read = HashSet::with_capacity(slot.inputs.len());
            for (place, kind) in &slot.inputs {
                if !read.insert(*place) {
                    return Err(BuildError::DuplicateInputPlace {
                        transition: slot.name.clone(),
                        place: self.places[place.0 as usize].meta.name.clone(),
                    });
                }
                // A snapshot needs the place's cloner, which
                // `place_no_clone` did not record — whether or not the type
                // itself is `Clone`.
                if *kind == EdgeKind::Snapshot && self.places[place.0 as usize].meta.clone.is_none()
                {
                    return Err(BuildError::SnapshotOfNoClonePlace {
                        transition: slot.name.clone(),
                        place: self.places[place.0 as usize].meta.name.clone(),
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

        let mut net = Net {
            id: self.net,
            places: self.places,
            transitions: self.transitions,
            next_token_id: 0,
            topology_counter: 0,
            state_counter: 0,
            fired_count: 0,
            selector: None,
            label_limit: self.label_limit,
            events: Vec::new(),
        };
        for (index, tokens) in self.seeds.into_iter().enumerate() {
            let place = PlaceId(index as u32);
            for token in tokens {
                // `insert_erased`, not `push_erased`: the initial marking is
                // where the net starts, not a change to report.
                net.insert_erased(place, token, "<seed>")
                    .expect("a seed is checked against its place's type by the builder");
            }
        }
        Ok(net)
    }

    // ----- internals ------------------------------------------------------

    /// The slot a [`TransitionBuilder`] writes its options into.
    ///
    /// The handle holds the builder rather than the slot so that the borrow
    /// is one `&mut NetBuilder`, which is what lets the chain in DESIGN §5
    /// read as it does; this is how it reaches back in.
    pub(crate) fn transition_slot(&mut self, id: TransitionId) -> &mut TransitionSlot {
        &mut self.transitions[id.0 as usize]
    }

    /// Appends a place with its vtable filled in for `T`.
    fn insert<T>(&mut self, name: String, clone: Option<CloneFn>) -> PlaceRef<T>
    where
        T: fmt::Debug + Send + Sync + 'static,
    {
        let id = PlaceId(self.places.len() as u32);
        self.places.push(PlaceSlot {
            meta: PlaceMeta {
                name,
                type_name: type_name::<T>(),
                type_id: TypeId::of::<T>(),
                // The generic function is instantiated for `T` here, where `T`
                // is known, and kept as an ordinary `fn` pointer afterwards.
                debug: debug_of::<T> as DebugFn,
                clone,
                #[cfg(feature = "serde")]
                to_json: None,
                #[cfg(feature = "serde")]
                from_json: None,
            },
            tokens: VecDeque::new(),
        });
        self.seeds.push(Vec::new());
        PlaceRef::new(self.net, id)
    }

    /// The index of a handle in this builder, rejecting a handle from
    /// another net.
    ///
    /// The stamp is the whole check: within one net an index and a type
    /// always agree, because the handle came from the call that created the
    /// place. The type assertion is kept as a debug assertion to catch a bug
    /// in that reasoning rather than a mistake by the author.
    fn index_of<T: 'static>(&self, place: PlaceRef<T>, method: &str) -> usize {
        assert!(
            place.net() == self.net,
            "{}",
            foreign_place(method, place.id(), place.net(), self.net)
        );
        let index = place.id().0 as usize;
        debug_assert!(
            self.places
                .get(index)
                .is_some_and(|slot| slot.meta.type_id == TypeId::of::<T>()),
            "{method}: place #{index} of this net does not hold {}",
            type_name::<T>()
        );
        index
    }

    /// The index of an erased output place or edge target, rejecting a
    /// handle from another net. The typed check above cannot be used here:
    /// the tuple traits have already forgotten `T`.
    fn index_of_erased(&self, place: PlaceId, net: NetId, method: &str) -> usize {
        assert!(
            net == self.net,
            "{}",
            foreign_place(method, place, net, self.net)
        );
        let index = place.0 as usize;
        debug_assert!(index < self.places.len(), "{method}: no place #{index}");
        index
    }
}

impl Default for NetBuilder {
    fn default() -> Self {
        NetBuilder::new()
    }
}

impl fmt::Debug for NetBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NetBuilder")
            .field("id", &self.net)
            .field("places", &self.places)
            .field("transitions", &self.transitions)
            .field("seeded", &self.seeds.iter().map(Vec::len).sum::<usize>())
            .field(
                "routing problems",
                &self.routing.iter().filter(|p| p.is_some()).count(),
            )
            .field("label_limit", &self.label_limit)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_names_are_rejected() {
        let mut b = NetBuilder::new();
        let _ = b.place::<u32>("Same");
        let _ = b.place::<String>("Same");
        let error = b.build().unwrap_err();
        assert!(matches!(error, BuildError::DuplicatePlaceName { ref name } if name == "Same"));
    }

    #[test]
    #[should_panic(expected = "but this is net")]
    fn seeding_with_a_foreign_handle_panics() {
        let mut other = NetBuilder::new();
        let foreign = other.place::<u32>("Elsewhere");

        let mut b = NetBuilder::new();
        let _ = b.place::<u32>("Here");
        b.seed(foreign, [1_u32]);
    }

    #[test]
    fn every_builder_has_its_own_id_and_passes_it_to_its_net() {
        let a = NetBuilder::new();
        let b = NetBuilder::new();
        assert_ne!(a.id(), b.id());

        let id = a.id();
        assert_eq!(a.build().unwrap().id(), id);
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
