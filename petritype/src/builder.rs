//! Declaring a net. The builder is the only place where the concrete token
//! type of each place is still known, so it is where the per-place vtable is
//! filled in.
//!
//! It is also where the net is checked. Most checks always run — duplicate
//! names, two input edges into one place, a snapshot or a restore of a place
//! that cannot clone, an output no payload type can reach — because each one
//! is a wiring mistake with no sensible reading. Two more are opt-in, because
//! what they forbid is legitimate in many nets:
//! [`assert_acyclic`](NetBuilder::assert_acyclic) and
//! [`assert_no_source_transitions`](NetBuilder::assert_no_source_transitions).
//!
//! # The termination proof
//!
//! Together the two are a static proof that the net stops. The marking is
//! finite, every firing consumes at least one token (no source transitions),
//! and tokens only move forward through a directed acyclic graph, so the
//! supply is finite and no firing replenishes anything upstream of itself:
//! the net runs out of things to do.
//!
//! Each alone proves less. An acyclic net with a source transition never
//! quiesces, because the source's firing leaves every place it reads as it
//! found them and it is immediately fireable again. A net where every
//! transition consumes can still run for ever around a cycle, since the
//! tokens a firing consumes come back to where they started.
//!
//! The proof is about the net left to itself. Tokens put in from outside
//! after [`build`](NetBuilder::build) — [`Net::put`](crate::Net::put),
//! [`Net::extend`](crate::Net::extend),
//! [`Net::set_tokens`](crate::Net::set_tokens) — are outside it: an
//! interactive net that is fed while it runs has no finish line, and is
//! bounded while it runs instead. Read edges are outside it too, and are
//! excluded from both checks, because they move no tokens: a configuration
//! place every transition snapshots is not a cycle, and a transition that
//! only reads is a source however many input edges it has.

use std::any::{TypeId, type_name};
use std::collections::{HashSet, VecDeque};
use std::fmt;

use crate::body::{OutputPlaces, TransitionBody};
use crate::edge::{EdgeKind, InputEdges, InputSpec};
use crate::erased::RoutingProblem;
use crate::error::BuildError;
use crate::firing::TieBreak;
use crate::ids::{NetId, PlaceId, PlaceRef, TransitionId, foreign_place};
use crate::nested::{Nested, NestedVtable, forget_inner_events};
use crate::net::Net;
use crate::param::ParamSlot;
use crate::place::{CloneFn, DebugFn, ErasedToken, PlaceMeta, PlaceSlot, clone_of, debug_of};
use crate::sealed::{BodySealed, EdgeTupleSealed, PlaceTupleSealed};
use crate::transition::{ArgSource, TransitionBuilder, TransitionSlot};

/// How many characters a label keeps by default, as in Python petritype.
const DEFAULT_LABEL_LIMIT: usize = 100;

/// One node of the token-flow graph
/// [`assert_acyclic`](NetBuilder::assert_acyclic) walks.
///
/// The two kinds are numbered separately, so a node is a kind and an index:
/// names are unique per kind, and a place and a transition may share one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Node {
    /// The place with this index.
    Place(u32),
    /// The transition with this index.
    Transition(u32),
}

/// How far the walk has got with a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Colour {
    /// Not reached yet.
    Unseen,
    /// On the path being walked right now: reaching it again closes a cycle.
    OnPath,
    /// Walked, along with everything reachable from it, and no cycle found
    /// through it. Reaching it again is a second way to the same place, not
    /// a loop.
    Done,
}

/// What the builder knows about one parameter type before the net is built
/// (DESIGN §10).
///
/// One entry per type, created by whichever of
/// [`param`](NetBuilder::param) and
/// [`serializable_param`](NetBuilder::serializable_param) is called first,
/// because neither implies the other and the order they are written in is the
/// author's business. [`build`](NetBuilder::build) is where the two halves
/// meet: an entry with a value becomes a [`ParamSlot`], and an entry without
/// one is a [`BuildError::MissingParam`] if any transition declared it and
/// nothing at all otherwise.
struct BuilderParam {
    type_id: TypeId,
    type_name: &'static str,
    /// The registered value with its cloner, or `None` for a type that has
    /// only been marked serialisable so far.
    value: Option<(ErasedToken, CloneFn)>,
    /// The wire name and codecs from
    /// [`serializable_param`](NetBuilder::serializable_param).
    #[cfg(feature = "serde")]
    wire: Option<ParamWire>,
}

/// What [`serializable_param`](NetBuilder::serializable_param) records: the
/// name this parameter travels under and the two codecs for its type.
#[cfg(feature = "serde")]
struct ParamWire {
    name: String,
    to_json: crate::place::ToJsonFn,
    from_json: crate::place::FromJsonFn,
}

/// The colour of a node, from the two vectors the walk keeps.
fn colour(node: Node, places: &[Colour], transitions: &[Colour]) -> Colour {
    match node {
        Node::Place(id) => places[id as usize],
        Node::Transition(id) => transitions[id as usize],
    }
}

/// Paints a node in whichever of the two vectors it belongs to.
fn set_colour(node: Node, places: &mut [Colour], transitions: &mut [Colour], colour: Colour) {
    match node {
        Node::Place(id) => places[id as usize] = colour,
        Node::Transition(id) => transitions[id as usize] = colour,
    }
}

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
    /// One entry per parameter type mentioned by
    /// [`param`](NetBuilder::param) or
    /// [`serializable_param`](NetBuilder::serializable_param), in the order
    /// the types were first mentioned. A short linear list keyed by
    /// [`TypeId`]: a net has a handful of parameters at most.
    params: Vec<BuilderParam>,
    label_limit: usize,
    /// Whether [`build`](NetBuilder::build) refuses a net whose token flow
    /// has a cycle, as asked for by
    /// [`assert_acyclic`](NetBuilder::assert_acyclic). Off by default: a
    /// feedback loop is a perfectly good net.
    assert_acyclic: bool,
    /// Whether [`build`](NetBuilder::build) refuses a net with a transition
    /// that consumes nothing, as asked for by
    /// [`assert_no_source_transitions`](NetBuilder::assert_no_source_transitions).
    /// Off by default: a generator is a perfectly good transition.
    assert_no_source_transitions: bool,
    /// How the built net's default selector breaks ties, as set by
    /// [`tie_break`](NetBuilder::tie_break).
    tie_break: TieBreak,
    /// Whether the author declared the net resettable, with
    /// [`resettable`](NetBuilder::resettable).
    resettable: bool,
    /// The first place [`passive`](NetBuilder::passive) was asked of that
    /// was not declared with [`place_nested`](NetBuilder::place_nested),
    /// kept until [`build`](NetBuilder::build) reports it as
    /// [`BuildError::NotNested`]: the per-place options are infallible.
    not_nested: Option<PlaceId>,
    /// The registry the net will restore its nested tokens from, as given
    /// to [`template`](NetBuilder::template) and
    /// [`templates`](NetBuilder::templates).
    #[cfg(feature = "serde")]
    templates: crate::template::Templates,
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
            params: Vec::new(),
            label_limit: DEFAULT_LABEL_LIMIT,
            assert_acyclic: false,
            assert_no_source_transitions: false,
            tie_break: TieBreak::FirstDeclared,
            resettable: false,
            not_nested: None,
            #[cfg(feature = "serde")]
            templates: crate::template::Templates::new(),
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

    /// Declares a place whose tokens carry Petri nets of their own: inner
    /// nets, which the engine runs, reads and reports on through the token
    /// (DESIGN §6.6).
    ///
    /// The token type says which nets it owns by implementing [`Nested`]; a
    /// type that does not is refused when the program compiles. What the
    /// place adds over an ordinary one:
    ///
    /// - [`Net::nested`] reads a token's inner nets, for a guard or a viewer;
    /// - [`Net::step_deep`] and the [`drivers`](crate::drivers) fire them,
    ///   after the outer net has nothing to fire, unless the place is
    ///   [`passive`](NetBuilder::passive);
    /// - everything they log comes out in this net's log as an
    ///   [`Event::Nested`](crate::Event::Nested), with this place and the
    ///   token's id.
    ///
    /// The place is a [no-clone](NetBuilder::place_no_clone) place: no
    /// snapshot edge from it, and no consuming edge from it on a transition
    /// that restores its inputs. Without the `serde` feature there is no
    /// [`Net::snapshot`] of the net either; with it, a snapshot copies the
    /// place's tokens with `Nested::fork`, which rebuilds their inner nets
    /// from templates (DESIGN §6.6). A [`MutateEdge`](crate::MutateEdge) on
    /// it is the way an outer body reaches inside.
    ///
    /// With the `serde` feature, marking the place `serializable` as any
    /// other — the token type deriving `Serialize` and `Deserialize`, its
    /// nets saved in `Net`'s own form — is what lets the outer net's marking
    /// be saved and loaded with its inner nets in it.
    ///
    /// ```
    /// use petritype::{Nested, Net, NetBuilder};
    ///
    /// #[derive(Debug)]
    /// struct Vehicle {
    ///     inner: Net,
    /// }
    ///
    /// impl Nested for Vehicle {
    ///     fn nets(&self) -> Vec<(&str, &Net)> {
    ///         vec![("cabin", &self.inner)]
    ///     }
    ///     fn nets_mut(&mut self) -> Vec<(&str, &mut Net)> {
    ///         vec![("cabin", &mut self.inner)]
    ///     }
    /// }
    ///
    /// let cabin = NetBuilder::new().build()?;
    /// let mut b = NetBuilder::new();
    /// let depot = b.place_nested::<Vehicle>("Depot");
    /// b.seed(depot, [Vehicle { inner: cabin }]);
    /// let net = b.build()?;
    ///
    /// let (token, _) = net.tokens(depot.id()).next().unwrap();
    /// let nets = net.nested(depot.id(), token).expect("a nested place");
    /// assert_eq!(nets[0].0, "cabin");
    /// assert!(net.places().next().unwrap().1.nested);
    /// # Ok::<(), petritype::BuildError>(())
    /// ```
    pub fn place_nested<T>(&mut self, name: impl Into<String>) -> PlaceRef<T>
    where
        T: Nested + fmt::Debug + Send + Sync + 'static,
    {
        let place = self.insert::<T>(name.into(), None);
        self.places[place.id().0 as usize].meta.nested = Some(NestedVtable::of::<T>());
        place
    }

    /// Marks a nested place passive: [`Net::step_deep`], and so the
    /// drivers, never step the inner nets of its tokens.
    ///
    /// For an outer net that wants manual control of what happens inside: a
    /// body with a [`MutateEdge`](crate::MutateEdge) on the place steps the
    /// inner net itself — `vehicle.inner.step()`, or
    /// [`drivers::run`](crate::drivers::run) on it — and what that does is
    /// reported in the outer log as for any body that reaches inside. A
    /// passive place's inner nets also do not count against
    /// [`Net::is_quiescent_deep`].
    ///
    /// # Errors
    ///
    /// [`BuildError::NotNested`] from [`build`](NetBuilder::build) if the
    /// place was not declared with
    /// [`place_nested`](NetBuilder::place_nested): only inner nets can be
    /// left alone. The call itself is infallible, as the other per-place
    /// options are.
    ///
    /// # Panics
    ///
    /// If `place` came from a different builder.
    pub fn passive<T: 'static>(&mut self, place: PlaceRef<T>) {
        let index = self.index_of(place, "passive");
        let meta = &mut self.places[index].meta;
        if meta.nested.is_some() {
            meta.passive = true;
        } else {
            self.not_nested.get_or_insert(place.id());
        }
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
    /// [`Net::json`] for them, the JSON commands, and
    /// [`Net::export_marking`] and [`Net::import_marking`], which want every
    /// place marked.
    ///
    /// Serialisation is opt-in per place: most tokens are never inspected
    /// structurally, and requiring `Serialize` on every token type would tax
    /// every author for a feature few need.
    ///
    /// A [nested](NetBuilder::place_nested) place is marked the same way,
    /// after it is declared: a token type with a [`Net`] field derives both
    /// traits, and its nets travel in `Net`'s own saved form, rebuilt on the
    /// way in from the templates of [`templates`](NetBuilder::templates).
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

    /// Gives the net its value for the parameter type `T` (DESIGN §10).
    ///
    /// A parameter is a run-varying value a transition needs but that is not
    /// a token — an API key, a tuning constant, the configuration of this
    /// run. It is declared as an input with
    /// [`Param::<T>::new()`](crate::Param), stored once per net keyed by
    /// type, and handed to a body as a clone. Registering one is setup, not
    /// change: it emits nothing and moves no counter, and
    /// [`Net::set_param`](crate::Net::set_param) is how it is replaced once
    /// the net runs.
    ///
    /// ```
    /// # use petritype::{NetBuilder, Param, TakeEdge};
    /// #[derive(Clone)]
    /// struct Scale(u32);
    ///
    /// let mut b = NetBuilder::new();
    /// let inbox = b.place::<u32>("Inbox");
    /// let out = b.place::<u32>("Out");
    /// b.seed(inbox, [7]);
    /// b.param(Scale(3));
    /// b.transition(
    ///     "Scale",
    ///     (TakeEdge(inbox), Param::<Scale>::new()),
    ///     (out,),
    ///     |n: u32, scale: Scale| (n * scale.0,),
    /// );
    /// let mut net = b.build()?;
    ///
    /// net.step()?;
    /// let token = net.tokens(out.id()).next().expect("one token").0;
    /// assert_eq!(net.token::<u32>(out.id(), token), Some(&21));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// One value per type: a second call with the same `T` replaces the
    /// first, and two values of one underlying type need newtypes
    /// (`GithubToken(String)`, `OpenAiToken(String)`). The bound is the token
    /// bound minus `Debug` — deliberately, since a parameter may be a secret
    /// and nothing in the engine ever prints one — plus `Clone`, because
    /// every firing gets a copy.
    ///
    /// Registering a parameter no transition declares is allowed: it is a
    /// value the net carries, readable with
    /// [`Net::param`](crate::Net::param) and usable from a
    /// [guard](crate::TransitionBuilder::guard). The other way round is not:
    /// a declared `Param::<T>` with no value is a
    /// [`BuildError::MissingParam`].
    pub fn param<T: Clone + Send + Sync + 'static>(&mut self, value: T) {
        let entry = self.param_entry::<T>();
        entry.value = Some((Box::new(value) as ErasedToken, clone_of::<T> as CloneFn));
    }

    /// Records how to convert this parameter type to and from JSON, and the
    /// name it travels under, enabling `JsonCommand::SetParam` for it.
    ///
    /// The name is a wire key chosen by a human, exactly as a place's name
    /// is: `std::any::type_name::<T>()` is a compiler detail that changes
    /// when a type moves module, so it is no basis for a saved file or a UI.
    ///
    /// ```
    /// # #[cfg(feature = "serde")] {
    /// # use petritype::{JsonCommand, NetBuilder, Param, TakeEdge};
    /// # use petritype::serde_json::json;
    /// #[derive(Clone, serde::Serialize, serde::Deserialize)]
    /// struct ApiToken(String);
    ///
    /// let mut b = NetBuilder::new();
    /// # let inbox = b.place::<u32>("Inbox");
    /// b.param(ApiToken("old".into()));
    /// b.serializable_param::<ApiToken>("api_token");
    /// # b.transition("Use", (TakeEdge(inbox), Param::<ApiToken>::new()), (), |_: u32, _: ApiToken| {});
    /// let mut net = b.build()?;
    ///
    /// net.apply_json(JsonCommand::SetParam {
    ///     param: "api_token".into(),
    ///     value: json!("new"),
    /// })?;
    /// assert_eq!(net.param::<ApiToken>().map(|t| t.0.as_str()), Some("new"));
    /// # }
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// Order-independent: marking a type may come before or after the
    /// [`param`](NetBuilder::param) that gives it a value, and a marking with
    /// no value is reported exactly as a missing value is — as a
    /// [`BuildError::MissingParam`] if a transition declared that type, and
    /// as nothing at all if none did. Two parameter types marked with one
    /// name are a [`BuildError::DuplicateParamName`].
    #[cfg(feature = "serde")]
    pub fn serializable_param<T>(&mut self, name: impl Into<String>)
    where
        T: serde::Serialize + serde::de::DeserializeOwned + Clone + Send + Sync + 'static,
    {
        let name = name.into();
        let entry = self.param_entry::<T>();
        entry.wire = Some(ParamWire {
            name,
            to_json: crate::place::to_json_of::<T> as crate::place::ToJsonFn,
            from_json: crate::place::from_json_of::<T> as crate::place::FromJsonFn,
        });
    }

    /// Registers one template for the net's nested tokens: the function that
    /// builds an inner net, under the name a saved token gives it (DESIGN
    /// §6.6). The net decodes its tokens with
    /// [`Net::templates`] in [scope](crate::Templates::scope), so a vehicle
    /// read back from JSON gets its cabin rebuilt by the `"Cabin"` template
    /// registered here. Chainable; a second template of one name replaces
    /// the first.
    ///
    /// See [`Templates::register`](crate::Templates::register) for the
    /// factory's shape.
    #[cfg(feature = "serde")]
    pub fn template<F, E>(&mut self, name: impl Into<String>, factory: F) -> &mut Self
    where
        F: Fn(&crate::Params) -> Result<Net, E> + Send + Sync + 'static,
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        self.templates.register(name, factory);
        self
    }

    /// Gives the net every template of a registry, as
    /// [`template`](NetBuilder::template) would one by one; a name already
    /// registered is replaced. Chainable.
    ///
    /// The usual order: build the registry, build the tokens' inner nets
    /// from it ([`Templates::build`](crate::Templates::build), which stamps
    /// them so they can be saved), then hand the outer net the same
    /// registry, so it can read them back.
    #[cfg(feature = "serde")]
    pub fn templates(&mut self, templates: &crate::Templates) -> &mut Self {
        self.templates.merge(templates);
        self
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
    /// - one body parameter per input, of the type that input produces
    ///   ([`InputEdge`](crate::InputEdge)) — which for the four edge kinds
    ///   depends on how they read their place, and for a
    ///   [`Param<T>`](crate::Param), which reads no place at all, is `T`;
    /// - one returned slot per output place, holding one token, zero or one,
    ///   or many ([`IntoTokens`](crate::IntoTokens)) — unless the body
    ///   returns a value that is not a tuple, which is
    ///   [routed](crate::RoutedOutput) to the output place of its type
    ///   instead, in which case the order of the outputs does not matter, or
    ///   a [`Distribution`](crate::Distribution), which names its places
    ///   itself and is checked when its tokens land;
    /// - the return value as a whole is the outputs, or a `Result` of them
    ///   ([`IntoOutcome`](crate::IntoOutcome)).
    ///
    /// The body may be a closure, a function by name, or either of those with
    /// an output distribution function composed after it by
    /// [`output_distribution`](crate::OutputDistribution::output_distribution)
    /// — in which case it is *that* function's return type that answers the
    /// second and third questions.
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
        let mut params = Vec::new();
        // The tuple is the body's parameter list, so its order has to
        // survive the split into edges and parameters.
        let mut args = Vec::with_capacity(described.len());
        // Detachable until proven otherwise: one mutating edge is enough to
        // tie the firing to the net. A parameter is a clone taken at launch
        // and borrows nothing, so it never ties one down.
        let mut detachable = true;
        for spec in described {
            match spec {
                InputSpec::Edge { place, kind, net } => {
                    self.index_of_erased(place, net, "transition");
                    detachable &= kind != EdgeKind::Mutate;
                    args.push(ArgSource::Edge(edges.len()));
                    edges.push((place, kind));
                }
                InputSpec::Param { type_id, type_name } => {
                    args.push(ArgSource::Param { type_id, type_name });
                    params.push(type_name);
                }
            }
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
            params,
            args,
            outputs: targets,
            // The one place where both the typed body and the erased engine
            // are in scope, so the one place the wrapper can be built.
            body: Some(body),
            guard: None,
            priority: None,
            enabled: true,
            restore_tokens_on_failure: false,
            detachable,
        });
        TransitionBuilder { builder: self, id }
    }

    /// Sets the maximum number of characters [`Net::label`] returns before
    /// truncating. The default is 100.
    pub fn label_limit(&mut self, limit: usize) {
        self.label_limit = limit;
    }

    /// Sets how the built net's default selector breaks ties between
    /// fireable transitions of equal priority: batch order
    /// ([`TieBreak::FirstDeclared`], the default) or flow order
    /// ([`TieBreak::LastDeclared`]). See [`TieBreak`] for why it is a
    /// choice, and [`Net::set_tie_break`] to change it later.
    pub fn tie_break(&mut self, tie_break: TieBreak) -> &mut Self {
        self.tie_break = tie_break;
        self
    }

    /// Declares that rebuilding this net with the code that built it gives
    /// back an equivalent starting state, so a tool may offer to reset it.
    ///
    /// A [`Net`] cannot be cloned, so a reset can only be a rebuild, and
    /// only the author knows whether a rebuild is a true reset: bodies that
    /// write files, call out over a network, draw random numbers or keep
    /// global state are not undone by building a new net. Tools that reset
    /// — the Bevy visualiser's Reset button — check
    /// [`Net::is_resettable`] and refuse a net that was not declared. Off by
    /// default.
    pub fn resettable(&mut self) -> &mut Self {
        self.resettable = true;
        self
    }

    /// Asks [`build`](NetBuilder::build) to refuse a net whose token flow
    /// has a cycle.
    ///
    /// The graph walked has both kinds of node, places and transitions, and
    /// two kinds of edge: a **consuming** input edge, from a place to a
    /// transition that takes from it ([`TakeEdge`](crate::TakeEdge) and
    /// [`TakeAllEdge`](crate::TakeAllEdge)), and every output edge, from a
    /// transition to a place it writes. Read edges are left out —
    /// [`SnapshotEdge`](crate::SnapshotEdge) and
    /// [`MutateEdge`](crate::MutateEdge) move no tokens, so a loop through
    /// one cannot feed itself, and a configuration place every transition
    /// reads is not a cycle.
    ///
    /// ```
    /// # use petritype::{BuildError, NetBuilder, TakeEdge};
    /// let mut b = NetBuilder::new();
    /// b.assert_acyclic();
    /// let inbox = b.place::<u32>("Inbox");
    /// b.transition("Loop", (TakeEdge(inbox),), (inbox,), |n: u32| (n + 1,));
    ///
    /// let error = b.build().expect_err("Loop feeds its own input place");
    /// assert_eq!(
    ///     error.to_string(),
    ///     "the net's token flow has a cycle: Loop -> Inbox -> Loop",
    /// );
    /// ```
    ///
    /// Passing proves that tokens only move forward, not that the net stops:
    /// a transition with no consuming input fires for ever in a perfectly
    /// acyclic net.
    ///
    /// # The pair
    ///
    /// With
    /// [`assert_no_source_transitions`](NetBuilder::assert_no_source_transitions)
    /// this is a static proof that the net stops. The marking is finite,
    /// every firing consumes at least one token, and tokens only move
    /// forward through a directed acyclic graph, so nothing a firing
    /// consumes can come back to it and the net runs out of things to do.
    /// The proof is about the net left to itself: tokens put in from outside
    /// after `build` — [`Net::put`](crate::Net::put),
    /// [`Net::extend`](crate::Net::extend) — are outside it, and a net that
    /// is fed while it runs is bounded while it runs instead.
    ///
    /// # Errors
    ///
    /// [`BuildError::Cycle`] from [`build`](NetBuilder::build), naming the
    /// nodes of one cycle from the node the walk closed on round to itself,
    /// so that the first and the last name are the same one. Transitions are
    /// walked in declaration order and their output places in declaration
    /// order, and the first cycle closed is the one reported; a net with
    /// several is built once more after each is broken.
    pub fn assert_acyclic(&mut self) -> &mut Self {
        self.assert_acyclic = true;
        self
    }

    /// Asks [`build`](NetBuilder::build) to refuse a net with a transition
    /// that consumes nothing.
    ///
    /// Every transition must have at least one consuming input edge — a
    /// [`TakeEdge`](crate::TakeEdge) or a
    /// [`TakeAllEdge`](crate::TakeAllEdge). A transition without one is a
    /// *source*: it takes no token out of any place, so firing it changes
    /// nothing about whether it can fire again, and it fires for ever. Read
    /// edges do not count, and a transition whose every input is a
    /// [`SnapshotEdge`](crate::SnapshotEdge) or a
    /// [`MutateEdge`](crate::MutateEdge) is a source like one with no inputs
    /// at all: both leave their places as they found them.
    ///
    /// ```
    /// # use petritype::{BuildError, NetBuilder};
    /// let mut b = NetBuilder::new();
    /// b.assert_no_source_transitions();
    /// let out = b.place::<u32>("Out");
    /// b.transition("Spawn", (), (out,), || (1_u32,));
    ///
    /// let error = b.build().expect_err("Spawn consumes nothing");
    /// assert!(matches!(error, BuildError::SourceTransition { ref transition } if transition == "Spawn"));
    /// ```
    ///
    /// Passing proves that every firing shrinks the marking, not that the
    /// net stops: tokens can still go round a cycle for ever, since what one
    /// firing consumes the next one puts back. Only paired with
    /// [`assert_acyclic`](NetBuilder::assert_acyclic) is it a termination
    /// proof, as that method's docs set out. A generator, or a transition fed
    /// from outside the net, is a deliberate source — do not assert this on
    /// such a net.
    ///
    /// # Errors
    ///
    /// [`BuildError::SourceTransition`] from [`build`](NetBuilder::build),
    /// naming the first such transition in declaration order.
    pub fn assert_no_source_transitions(&mut self) -> &mut Self {
        self.assert_no_source_transitions = true;
        self
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
    /// [`BuildError::NotNested`] if a place marked
    /// [`passive`](NetBuilder::passive) was not declared with
    /// [`place_nested`](NetBuilder::place_nested);
    /// [`BuildError::SnapshotOfNoClonePlace`] if a snapshot edge reads a
    /// place that cannot clone its tokens;
    /// [`BuildError::RestoreOfNoClonePlace`] if a transition that
    /// [restores its inputs on failure](crate::TransitionBuilder::restore_tokens_on_failure)
    /// consumes such a place;
    /// [`BuildError::DuplicateInputPlace`] if one transition reads one place
    /// through two input edges; and, for a transition whose body
    /// [routes](crate::RoutedOutput) its outputs by type,
    /// [`BuildError::UnroutableOutput`], [`BuildError::AmbiguousOutput`] or
    /// [`BuildError::UnreachableOutput`].
    ///
    /// Between the two, the parameters: with the `serde` feature,
    /// `BuildError::DuplicateParamName` if two parameter types were marked
    /// serialisable with `NetBuilder::serializable_param` under one wire
    /// name, and [`BuildError::MissingParam`] if a transition declared a
    /// [`Param<T>`](crate::Param) and no [`param`](NetBuilder::param) value
    /// of that type was registered — the first offender in transition order,
    /// then in the order the inputs were written.
    ///
    /// Then, only if they were asked for,
    /// [`BuildError::SourceTransition`] from
    /// [`assert_no_source_transitions`](NetBuilder::assert_no_source_transitions)
    /// and [`BuildError::Cycle`] from
    /// [`assert_acyclic`](NetBuilder::assert_acyclic).
    pub fn build(self) -> Result<Net, BuildError> {
        let mut seen = HashSet::with_capacity(self.places.len());
        for slot in &self.places {
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
                // Restoring is done with clones taken at stage one, so a
                // transition that restores can only consume places that can
                // clone. The read edges are exempt because they consume
                // nothing: a mutating edge has nothing to put back, and a
                // snapshot of such a place was refused just above.
                if slot.restore_tokens_on_failure
                    && matches!(*kind, EdgeKind::Take | EdgeKind::TakeAll)
                    && self.places[place.0 as usize].meta.clone.is_none()
                {
                    return Err(BuildError::RestoreOfNoClonePlace {
                        transition: slot.name.clone(),
                        place: self.places[place.0 as usize].meta.name.clone(),
                    });
                }
            }
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
                if !slot.inputs.iter().any(|(_, kind)| kind.consumes()) {
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
                Some(ParamSlot {
                    type_id: entry.type_id,
                    type_name: entry.type_name,
                    value,
                    clone,
                    #[cfg(feature = "serde")]
                    name: entry.wire.as_ref().map(|wire| wire.name.clone()),
                    #[cfg(feature = "serde")]
                    to_json: entry.wire.as_ref().map(|wire| wire.to_json),
                    #[cfg(feature = "serde")]
                    from_json: entry.wire.as_ref().map(|wire| wire.from_json),
                })
            })
            .collect();

        let mut net = Net {
            id: self.net,
            places: self.places,
            transitions: self.transitions,
            params,
            next_token_id: 0,
            topology_counter: 0,
            state_counter: 0,
            fired_count: 0,
            selector: None,
            tie_break: self.tie_break,
            resettable: self.resettable,
            label_limit: self.label_limit,
            events: Vec::new(),
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

    /// The slot a [`TransitionBuilder`] writes its options into.
    ///
    /// The handle holds the builder rather than the slot so that the borrow
    /// is one `&mut NetBuilder`, which is what lets the chain in DESIGN §5
    /// read as it does; this is how it reaches back in.
    pub(crate) fn transition_slot(&mut self, id: TransitionId) -> &mut TransitionSlot {
        &mut self.transitions[id.0 as usize]
    }

    /// The first cycle in the net's token flow, as the names of its nodes
    /// from the node the walk closed on round to itself, or `None` if there
    /// is none.
    ///
    /// A depth-first walk with three colours — unseen, on the path, done —
    /// where meeting a node that is on the path is a cycle. The stack is
    /// explicit rather than the call stack, so that a long pipeline cannot
    /// overflow it. Every transition is a root, in declaration order, which
    /// is enough: a cycle alternates places and transitions, so it contains
    /// a transition, and a transition already walked was walked with
    /// everything reachable from it.
    fn find_cycle(&self) -> Option<Vec<String>> {
        // The reverse of `inputs`, taken once: which transitions consume a
        // place, in declaration order. Read edges are not token flow.
        let mut consumers: Vec<Vec<u32>> = vec![Vec::new(); self.places.len()];
        for (index, slot) in self.transitions.iter().enumerate() {
            for (place, kind) in &slot.inputs {
                if kind.consumes() {
                    consumers[place.0 as usize].push(index as u32);
                }
            }
        }

        // One colour per node, in two vectors because the two kinds are
        // numbered separately.
        let mut places = vec![Colour::Unseen; self.places.len()];
        let mut transitions = vec![Colour::Unseen; self.transitions.len()];
        for root in 0..self.transitions.len() as u32 {
            if transitions[root as usize] != Colour::Unseen {
                continue;
            }
            transitions[root as usize] = Colour::OnPath;
            // The path being walked, each node with how many of its
            // successors have been followed so far.
            let mut path = vec![(Node::Transition(root), 0_usize)];
            while let Some(&(node, followed)) = path.last() {
                path.last_mut().expect("the path is not empty here").1 += 1;
                let Some(next) = self.successor(&consumers, node, followed) else {
                    // Every way out of this node led nowhere: it is not on
                    // any cycle, and nothing need come back to it.
                    set_colour(node, &mut places, &mut transitions, Colour::Done);
                    path.pop();
                    continue;
                };
                match colour(next, &places, &transitions) {
                    Colour::OnPath => {
                        let from = path
                            .iter()
                            .position(|(node, _)| *node == next)
                            .expect("a node on the path is in the path");
                        return Some(
                            path[from..]
                                .iter()
                                .map(|(node, _)| self.node_name(*node))
                                .chain(std::iter::once(self.node_name(next)))
                                .collect(),
                        );
                    }
                    Colour::Done => {}
                    Colour::Unseen => {
                        set_colour(next, &mut places, &mut transitions, Colour::OnPath);
                        path.push((next, 0));
                    }
                }
            }
        }
        None
    }

    /// Where the `index`-th edge out of `node` leads, or `None` once they are
    /// exhausted: a transition's output places, or the transitions that
    /// consume a place, both in declaration order.
    fn successor(&self, consumers: &[Vec<u32>], node: Node, index: usize) -> Option<Node> {
        match node {
            Node::Transition(id) => self.transitions[id as usize]
                .outputs
                .get(index)
                .map(|place| Node::Place(place.0)),
            Node::Place(id) => consumers[id as usize]
                .get(index)
                .map(|transition| Node::Transition(*transition)),
        }
    }

    /// What to call a node in a reported cycle. Names are unique per kind,
    /// and a place and a transition may share one, so a path can name the
    /// same word twice without meaning the same node.
    fn node_name(&self, node: Node) -> String {
        match node {
            Node::Place(id) => self.places[id as usize].meta.name.clone(),
            Node::Transition(id) => self.transitions[id as usize].name.clone(),
        }
    }

    /// The entry for the parameter type `T`, appended if this is the first
    /// mention of it.
    ///
    /// Both [`param`](NetBuilder::param) and
    /// [`serializable_param`](NetBuilder::serializable_param) fill in one
    /// half of an entry, in either order, which is why the lookup is written
    /// once here rather than in each of them.
    fn param_entry<T: 'static>(&mut self) -> &mut BuilderParam {
        let type_id = TypeId::of::<T>();
        let index = match self
            .params
            .iter()
            .position(|entry| entry.type_id == type_id)
        {
            Some(index) => index,
            None => {
                self.params.push(BuilderParam {
                    type_id,
                    type_name: type_name::<T>(),
                    value: None,
                    #[cfg(feature = "serde")]
                    wire: None,
                });
                self.params.len() - 1
            }
        };
        &mut self.params[index]
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
                nested: None,
                passive: false,
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
        let mut out = f.debug_struct("NetBuilder");
        out.field("id", &self.net)
            .field("places", &self.places)
            .field("transitions", &self.transitions)
            .field("seeded", &self.seeds.iter().map(Vec::len).sum::<usize>())
            // Type names only: a parameter value is never printed, having no
            // `Debug` bound to print it with (DESIGN §10).
            .field(
                "params",
                &self
                    .params
                    .iter()
                    .map(|entry| entry.type_name)
                    .collect::<Vec<_>>(),
            )
            .field(
                "routing problems",
                &self.routing.iter().filter(|p| p.is_some()).count(),
            )
            .field("label_limit", &self.label_limit)
            .field("assert_acyclic", &self.assert_acyclic)
            .field("tie_break", &self.tie_break)
            .field("resettable", &self.resettable)
            .field("not_nested", &self.not_nested);
        #[cfg(feature = "serde")]
        out.field("templates", &self.templates);
        out.field(
            "assert_no_source_transitions",
            &self.assert_no_source_transitions,
        )
        .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Resetting is the author's promise, so nothing makes it but the
    /// builder call.
    #[test]
    fn a_net_is_resettable_only_when_declared() {
        let plain = NetBuilder::new().build().expect("an empty net builds");
        assert!(!plain.is_resettable(), "off by default");

        let mut b = NetBuilder::new();
        b.resettable();
        let declared = b.build().expect("an empty net builds");
        assert!(declared.is_resettable());
    }

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
