//! Typed, executable, observable Petri nets.
//!
//! A **place** is a FIFO queue of tokens of one Rust type. A **token** is one
//! owned value with a stable [`TokenId`]. A **transition** consumes tokens,
//! runs an ordinary Rust closure, and distributes the results. Every change to
//! the net is an [`Event`], so a visualiser can mirror the net in O(changes)
//! rather than O(state).
//!
//! This version covers *declaring* a net — places, tokens, transitions and
//! the edges between them — reading it back, and firing it, one transition at
//! a time or several at once. What a firing does when it goes wrong is
//! settled too: an expected outcome is *data*, routed to an error place like
//! any other token, while a body that returns `Err` or panics has **failed**
//! — the panic is caught, the body goes back to its slot, nothing is
//! distributed, and the tokens the firing consumed are lost, unless the
//! transition was declared with
//! [`restore_tokens_on_failure`](TransitionBuilder::restore_tokens_on_failure),
//! which puts them back where they were. The wiring below is checked when it compiles. A
//! body may return its outputs as a tuple, slot by slot, as one value to be
//! [routed to the output place of its type](routing), or as a
//! [`Distribution`] of `(place, token)` pairs it names itself — which
//! [`output_distribution`](OutputDistribution::output_distribution) lets an
//! author write beside the net, leaving the domain function unchanged. What
//! the compiler cannot check is checked when the net is built, including two
//! opt-in assertions — [`NetBuilder::assert_acyclic`] and
//! [`NetBuilder::assert_no_source_transitions`] — which together prove that a
//! net left to itself stops.
//!
//! ```
//! use petritype::{EdgeKind, Event, NetBuilder, SnapshotEdge, TakeEdge};
//!
//! #[derive(Debug, Clone, PartialEq)]
//! struct Parcel {
//!     id: u32,
//!     weight: f32,
//! }
//!
//! #[derive(Debug, Clone)]
//! struct Config {
//!     max_weight: f32,
//! }
//!
//! let mut b = NetBuilder::new();
//! let inbox = b.place::<Parcel>("Inbox");                 // PlaceRef<Parcel>
//! let heavy = b.place::<Parcel>("Heavy");
//! let config = b.place::<Config>("Config");
//! b.seed(inbox, [Parcel { id: 1, weight: 3.0 }]);
//! b.seed(config, [Config { max_weight: 10.0 }]);
//!
//! // A transition is a closure plus the wiring around it: one parameter per
//! // input edge, one returned slot per output place. `TakeEdge` consumes the
//! // parcel, `SnapshotEdge` only reads the config, and the `Option` slot says
//! // that a light parcel produces no token at all.
//! let weigh = b
//!     .transition(
//!         "Weigh",
//!         (TakeEdge(inbox), SnapshotEdge(config)),
//!         (heavy,),
//!         |parcel: Parcel, cfg: Config| (Some(parcel).filter(|p| p.weight > cfg.max_weight),),
//!     )
//!     .id();
//!
//! let mut net = b.build()?;
//!
//! // The declared net is where a view starts: no events, counters at zero.
//! // It reads the topology with `places()` and `transitions()`, and the
//! // marking with `tokens()`.
//! let (id, info) = net.transitions().next().unwrap();
//! assert_eq!((id, info.name), (weigh, "Weigh"));
//! assert_eq!(info.inputs[1], (config.id(), EdgeKind::Snapshot));
//! assert_eq!(net.count(inbox.id()), 1);
//! assert_eq!(net.state_counter(), 0);
//! assert_eq!(net.drain_events().count(), 0);
//!
//! // Input from outside the net: `&mut Net` is the single-writer guarantee.
//! let token = net.put(inbox, Parcel { id: 2, weight: 30.0 });
//! assert_eq!(net.count(inbox.id()), 2);
//! assert_eq!(net.token::<Parcel>(inbox.id(), token).unwrap().weight, 30.0);
//! assert_eq!(net.label(inbox.id(), token).unwrap(), "Parcel { id: 2, weight: 30.0 }");
//!
//! // Changes after that are events, which is what the view has to redraw.
//! assert_eq!(net.state_counter(), 1);
//! assert_eq!(
//!     net.drain_events().collect::<Vec<_>>(),
//!     [Event::Added { place: inbox.id(), token }],
//! );
//!
//! // Firing is the net running. `step` scans for the transitions that could
//! // fire, asks the selector for one, and fires it inline. The first parcel
//! // is light, so the `Option` slot produces nothing; the second is heavy.
//! assert_eq!(net.fireable(), [weigh]);
//! assert_eq!(net.step()?, Some(weigh));
//! assert_eq!(net.count(heavy.id()), 0);
//! assert_eq!(net.step()?, Some(weigh));
//! assert_eq!(net.count(heavy.id()), 1);
//!
//! // Inbox is empty now, so nothing is fireable: the net is quiescent.
//! assert!(net.fireable().is_empty());
//! assert_eq!(net.step()?, None);
//! assert_eq!(net.fired_count(), 2);
//!
//! // One `Fired` per firing, naming exactly which tokens it consumed and
//! // which it produced — the second firing took the parcel put in above.
//! let events: Vec<Event> = net.drain_events().collect();
//! let landed = net.tokens(heavy.id()).next().unwrap().0;
//! assert_eq!(events.len(), 2);
//! assert_eq!(
//!     events[1],
//!     Event::Fired {
//!         transition: weigh,
//!         consumed: vec![(inbox.id(), token)],
//!         produced: vec![(heavy.id(), landed)],
//!     },
//! );
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Firing in three stages
//!
//! The [`step`](Net::step) above is three stages in one call. They are public
//! one at a time as well, because the middle one need not happen where the
//! net is:
//!
//! ```
//! use petritype::{NetBuilder, TakeEdge};
//!
//! let mut b = NetBuilder::new();
//! let inbox = b.place::<u32>("Inbox");
//! let out = b.place::<u32>("Out");
//! b.seed(inbox, [21]);
//! let double = b
//!     .transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,))
//!     .id();
//! let mut net = b.build()?;
//!
//! // Stage 1: the token leaves Inbox now, and the body leaves its slot.
//! let firing = net.launch(double)?;
//! assert_eq!(net.count(inbox.id()), 0);
//! assert_eq!(net.in_flight(), [double]);
//!
//! // Stage 2: wherever you like. A `Firing` owns its tokens and its body.
//! let completed = std::thread::spawn(move || firing.run())
//!     .join()
//!     .expect("the body did not panic");
//!
//! // Stage 3: the net writes, as it is the only thing that ever does.
//! net.distribute(completed)?;
//! assert!(net.in_flight().is_empty());
//! let token = net.tokens(out.id()).next().expect("one token landed").0;
//! assert_eq!(net.token::<u32>(out.id(), token), Some(&42));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! The split exists because a body may be slow, and may want a thread, a task
//! pool or a later frame: [`Firing`] and [`Completed`] are `Send` and own
//! everything they need, so stage two can run anywhere while the net remains
//! the single writer — nothing but [`launch`](Net::launch) and
//! [`distribute`](Net::distribute) touches a place. Taking the tokens at
//! launch is also what resolves conflicts: a peer transition that wanted one
//! of them is simply not fireable any more, with no locks anywhere. A
//! transition whose body borrows a token in its place ([`MutateEdge`]) cannot
//! leave the net, so `launch` runs it inline and hands back a `Firing` that
//! is already done — a driver can then treat every transition alike.
//!
//! # Where things are
//!
//! - [`NetBuilder`] declares places and transitions and produces a [`Net`].
//! - [`PlaceRef<T>`] is the typed handle a builder hands back, stamped with
//!   the [`NetId`] of its net; [`PlaceId`], [`TransitionId`] and [`TokenId`]
//!   are the erased ids that events and the read API speak in.
//! - [`TakeEdge`], [`TakeAllEdge`], [`SnapshotEdge`] and [`MutateEdge`] say
//!   how a transition reads a place, and so what its body is handed;
//!   [`TakeWhereEdge`] and [`MutateWhereEdge`] do what `TakeEdge` and
//!   `MutateEdge` do to the oldest token a [`Filter`] accepts rather than to
//!   the front one, leaving every other token where it is (*keyed* edges,
//!   DESIGN §8.3); [`DrainEdge`] and [`SurveyEdge`] take or read every
//!   token there is, zero or more, and never hold their transition back
//!   (DESIGN §4.4); [`EdgeKind`] is the same thing with the type forgotten.
//! - [`#[derive(Inputs)]`](Inputs) binds a body's inputs **by name**
//!   rather than by position: on a struct with one field per input, it
//!   generates a struct of edges with the same field names, each field
//!   wired to any input that [`Reads`] its type. The body takes the struct,
//!   which names no place and no kind of edge, so one library function
//!   serves nets that read its inputs differently; the struct of edges is
//!   one position of the input tuple, its edges in field order.
//!   [`#[derive(Outputs)]`](Outputs) does the same for what a body
//!   returns: a struct of outputs, each field wired to a place that
//!   [`Accepts`] its type, which is the whole of the transition's outputs.
//!   With [`#[derive(Routed)]`](Routed) these are the three ways to bind a
//!   body: by position (tuples), by name (these), by type (routing).
//! - [`IntoTokens`] decides how many tokens one returned slot is worth, and
//!   [`IntoOutcome`] tells outputs from failure.
//! - [`RoutedOutput`] is the second output form: a body that returns a value
//!   rather than a tuple has each payload sent to the output place of its
//!   type, which is what [`#[derive(Routed)]`](Routed) and [`Either`] are
//!   for — see the [`routing`] module.
//! - [`Distribution`] is the third: `(place, token)` pairs the body names
//!   itself, for choosing between output places of the same type or for
//!   fanning out. [`OutputDistribution::output_distribution`] composes the
//!   function that builds one *after* a body, so the domain function keeps
//!   knowing nothing about places, and a pair the transition may not write to
//!   is a [`DistributionError`] that fails the firing without writing
//!   anything.
//! - [`Net::places`], [`Net::transitions`], [`Net::tokens`], [`Net::label`]
//!   and [`Net::token`] read the net; [`Net::drain_events`] follows it.
//! - [`Net::edges`] is that same wiring one edge at a time, as [`EdgeInfo`]:
//!   place → transition with the [`EdgeKind`] saying how it is read, or
//!   transition → place with the slot it is written from. It is what a
//!   diagram draws.
//! - [`Net::extend`], [`Net::put`], [`Net::set_tokens`], [`Net::remove`] and
//!   [`Net::replace`] change the marking from outside.
//! - [`Command`] is that same input with the types forgotten, for a sender
//!   that holds no `&mut Net` — across a channel from a UI thread, or from a
//!   source that never knew the types. [`Net::apply`] carries one out, and is
//!   the one place in the engine where a type is checked at run time; it is
//!   all-or-nothing, so a [`CommandError`] means the net neither changed nor
//!   said anything. With the `serde` feature, `JsonCommand` is the same
//!   instruction addressed by name and carrying `serde_json` values, which
//!   `Command::from_json`, `Command::to_json` and `Net::apply_json`
//!   translate.
//! - [`Param<T>`](Param) is an input that reads no place: a run-varying value
//!   — a secret, a tuning constant, this run's configuration — declared
//!   beside the edges as `Param::<T>::new()` and handed to the body as a
//!   clone. [`NetBuilder::param`] gives the net its value, a declared
//!   parameter without one is a [`BuildError::MissingParam`],
//!   [`Net::set_param`] replaces it between steps (an
//!   [`Event::ParamChanged`], and never a surprise for a
//!   firing already in flight) and [`Net::param`] reads it, which is what
//!   lets a guard depend on one. Parameters are not nodes: they are in no
//!   [`EdgeInfo`], change no fireability, and appear in a diagram only as the
//!   type names on [`TransitionInfo::params`].
//! - [`Net::snapshot`] is the net's state, owned and independent: the
//!   marking, the switches, the in-flight set, the parameters and the
//!   counters, for a renderer on another thread or a test comparing two
//!   moments. It is a [`Snapshot`] and not a `Net` because bodies cannot be
//!   cloned — a what-if copy is a second `build()` — and it is a `Result`
//!   because a [`place_no_clone`](NetBuilder::place_no_clone) place cannot
//!   copy its tokens ([`NetError::NoClonePlace`]).
//! - [`NetBuilder::keys`] declares the *keys* of a place's tokens: named
//!   attributes — a string, an integer or a boolean each, a [`KeyValue`] —
//!   that an extractor the author writes derives from every token, so the
//!   token type needs nothing for them. [`Net::keys`] reads one token's
//!   [`Keys`]; [`Net::query`] finds every token a [`Filter`] of [`Clause`]s
//!   lets through, as [`Match`]es in place order then token order, from an
//!   index the net keeps up to date as tokens come, go and change; and
//!   [`Net::distinct_values`] lists the values a key takes, in order of
//!   first appearance, which is what a viewer makes its tabs of (DESIGN
//!   §8.3). A [`Snapshot`] answers the same questions by scanning. The same
//!   index lets a keyed edge find its token, and [`Net::first_match`] asks
//!   which token that is, as a guard may want to know.
//! - [`Net::topology`] (and [`Snapshot::topology`]) is the net's *shape*,
//!   owned: places, transitions and edges with no tokens and no switches, as
//!   a [`Topology`] that a render thread or a runner can hold without a
//!   borrow. [`Snapshot::edges`] is [`Net::edges`] on a snapshot.
//! - **fireable** and **enabled** are two different questions:
//!   [`Net::enable`] / [`Net::disable`] work the author's on/off switch,
//!   while *fireable* means "could fire right now" — switched on, input
//!   places non-empty (holding a token the filter accepts, for a keyed
//!   edge; anything, nothing included, for a drain or a survey edge), guard
//!   saying yes.
//! - [`Net::fireable`] and [`Net::is_fireable`] say what could fire now;
//!   [`Net::step`] fires one transition, [`Net::fire`] fires a named one,
//!   and a body that returns `Err` or panics gives [`FiringFailed`].
//! - [`TransitionBuilder::restore_tokens_on_failure`] is the one dial on
//!   failure: with it, a failed firing puts the tokens it consumed back where
//!   it found them; without it they are gone. [`BodyPanicked`] is what a
//!   caught panic looks like once it is a failure like any other.
//! - [`Net::launch`], [`Firing::run`] and [`Net::distribute`] are those same
//!   three stages taken one at a time, with [`Net::in_flight`] and
//!   [`Net::is_in_flight`] naming the transitions whose bodies are out.
//!   Dropping a [`Firing`] or a [`Completed`] instead of distributing it
//!   strands its transition in flight for good.
//! - [`Nested`] is a token type that carries Petri nets of its own, and
//!   [`NetBuilder::place_nested`] the place that holds such tokens (DESIGN
//!   §6.6). [`Net::nested`] reads a token's inner nets, [`Net::step_deep`]
//!   fires the net and then, when it has nothing to fire, its inner nets,
//!   with a [`DeepStep`] saying where (or, under the [`DeepPolicy`]
//!   `Interleaved`, the net and its inner nets in turn),
//!   [`Net::step_deep_staged`] is the same step for a driver that fires the
//!   net's own transitions in stages ([`StagedStep`]), and
//!   [`Net::is_quiescent_deep`] asks whether anything can happen at any
//!   depth.
//!   [`NetBuilder::passive`] leaves a place's inner nets to the outer bodies
//!   that reach into them, and [`Net::nested_scope`] is that reach for a
//!   caller that is not a body. Whatever an inner net logs comes out in the
//!   outer log as an [`Event::Nested`], with the place and token it
//!   happened in and the inner net's own [`NetId`], which does not change as
//!   the token moves; [`Net::place_ref`] finds an inner place's typed handle
//!   by name. With the `serde` feature such tokens can be saved, restored
//!   and copied (below).
//! - [`drivers::run`] drives a net until it has nothing left to fire, and
//!   [`drivers::run_indefinitely`] drives it on a clock until a flag says
//!   stop. [`Mode`](drivers::Mode) picks the loop — sequential, where the
//!   selector chooses one transition at a time, or concurrent, where
//!   everything fireable is launched at once on threads of its own;
//!   [`Limits`](drivers::Limits) paces a run or blows a fuse on one that will
//!   not stop; [`OnFailure`](drivers::OnFailure) says whether a failed firing
//!   ends the run, switches its transition off, or is merely counted; and
//!   [`RunSummary`](drivers::RunSummary) reports what the call did.
//! - [`TransitionBuilder::guard`] and [`TransitionBuilder::priority`] decide
//!   what is fireable and which of the fireable ones the default
//!   [`Selector`] picks, with ties going by the net's [`TieBreak`] (first
//!   declared, last declared for flow order, or round robin for turns);
//!   [`Net::set_selector`] replaces that choice,
//!   [`Net::clear_selector`] puts it back, [`Net::select`] reports it
//!   without firing, [`Net::choose`] takes it for a firing made in stages,
//!   and [`Net::enable`] /
//!   [`Net::disable`] switch a transition off and on.
//! - [`NetBuilder::assert_acyclic`] and
//!   [`NetBuilder::assert_no_source_transitions`] are the two opt-in checks
//!   [`NetBuilder::build`] makes: no loop in the token flow, and every
//!   transition consuming at least one token. The pair is a static proof
//!   that the net quiesces ([`BuildError::Cycle`],
//!   [`BuildError::SourceTransition`]); each alone proves less, and their
//!   docs say how much.
//!
//! # Features
//!
//! - `serde`: adds `NetBuilder::serializable`, `Net::json` and
//!   `Snapshot::json`, for inspectors
//!   that want structure rather than a `Debug` string, and `JsonCommand`
//!   with `Command::from_json`, `Command::to_json` and `Net::apply_json`, for
//!   input that arrives as JSON rather than as Rust values —
//!   `NetBuilder::serializable_param` is the parameter half of that, giving a
//!   parameter type the wire name a `set_param` command addresses it by. [`PlaceId`],
//!   [`TransitionId`] and [`TokenId`] become `Serialize`/`Deserialize` as
//!   bare numbers; [`NetId`] becomes `Serialize` only, as the opaque,
//!   process-local id an [`Event::Nested`] names its inner net by, never
//!   something to read back. [`Event`], [`EdgeInfo`], [`EdgeKind`] and [`Topology`]
//!   become `Serialize` (never `Deserialize`) in that id form, for logs and
//!   tests; [`PlaceInfo::serializable`] says which places were marked.
//!   `Net::export_marking` writes every place's tokens as a `Marking` (JSON
//!   by place name, oldest first) and `Net::import_marking` replaces the
//!   whole marking with one, emitting a single [`Event::Resynced`]. A
//!   `Templates` registry builds nets by name and stamps them, so that a
//!   `Net` is itself `Serialize` — `{"template", "marking", "params",
//!   "enabled"}` — and `Deserialize`, rebuilt from the registry in
//!   `Templates::scope`, and `Net::fork` copies one: which is how a token
//!   that carries a net is saved, loaded, and snapshotted
//!   ([`Nested`] gains `fork`). [`KeyValue`] becomes `Serialize` and
//!   `Deserialize` as the bare JSON string, integer or boolean it is,
//!   [`Keys`] `Serialize` as an object, and [`Filter`] and [`Clause`]
//!   `Serialize` and `Deserialize` in the protocol's shape (places by id); a
//!   keyed edge's filter is then on its [`EdgeInfo`]. A [`Clause::IsParam`]
//!   compares a key with a parameter by the wire name `serializable_param`
//!   gave it, and so accepts nothing without the feature. (All of these are
//!   hidden from this page unless the feature is on.)

#![warn(missing_docs, missing_debug_implementations, unreachable_pub)]
#![forbid(unsafe_code)]

mod arity;
#[doc(hidden)]
pub mod binding;
mod body;
mod builder;
mod command;
mod distribution;
pub mod drivers;
mod edge;
mod erased;
mod error;
mod event;
mod firing;
mod ids;
mod index;
mod keys;
#[cfg(feature = "serde")]
mod marking;
mod nested;
mod net;
mod param;
mod place;
pub mod routing;
mod sealed;
mod snapshot;
#[cfg(feature = "serde")]
mod template;
mod topology;
mod transition;

pub use body::{
    Accepts, IntoOutcome, IntoTokens, OutputPlaces, PositionalOutputs, TransitionBody, marker,
};
pub use builder::NetBuilder;
pub use command::Command;
#[cfg(feature = "serde")]
pub use command::JsonCommand;
pub use distribution::{
    Distribution, DistributionError, OutputDistribution, WithOutputDistribution,
};
pub use edge::{
    DrainEdge, EdgeInfo, EdgeKind, InputEdge, InputEdges, MutateEdge, MutateWhereEdge, Reads,
    SnapshotEdge, SurveyEdge, TakeAllEdge, TakeEdge, TakeWhereEdge,
};
#[cfg(feature = "serde")]
pub use error::TemplateError;
pub use error::{BodyPanicked, BuildError, CommandError, FiringFailed, NetError};
pub use event::Event;
pub use firing::{Completed, Firing, Selector, TieBreak};
pub use ids::{NetId, PlaceId, PlaceRef, TokenId, TransitionId};
pub use keys::{Clause, Filter, KeyValue, Keys, Match};
#[cfg(feature = "serde")]
pub use marking::Marking;
pub use nested::{DeepPolicy, DeepStep, Nested, StagedStep};
pub use net::{Net, PlaceInfo};
pub use param::Param;
pub use routing::{Either, RoutedOutput};
pub use snapshot::Snapshot;
#[cfg(feature = "serde")]
pub use template::{Params, Templates};
pub use topology::{Topology, TopologyPlace, TopologyTransition};
pub use transition::{TransitionBuilder, TransitionInfo};

/// Routes a returned value to the output place of its type; see
/// [`RoutedOutput`] and the [`routing`] module.
pub use petritype_derive::Routed;

/// Binds a body's inputs to a net by field name: generates, for a struct
/// with one field per input, the struct of edges that wires it (DESIGN
/// §4.4); see [`Reads`].
pub use petritype_derive::Inputs;

/// Binds a body's outputs to a net by field name: generates, for a struct
/// the body returns, the struct of outputs that wires each field to a place
/// (DESIGN §4.5); see [`Accepts`].
pub use petritype_derive::Outputs;

/// The `serde_json` this crate was built against, re-exported so that callers
/// of [`Net::json`] need not match its version by hand.
#[cfg(feature = "serde")]
pub use serde_json;
