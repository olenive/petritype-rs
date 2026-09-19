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
//!   [`EdgeKind`] is the same thing with the type forgotten.
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
//! - **fireable** and **enabled** are two different questions:
//!   [`Net::enable`] / [`Net::disable`] work the author's on/off switch,
//!   while *fireable* means "could fire right now" — switched on, input
//!   places non-empty, guard saying yes.
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
//! - [`TransitionBuilder::guard`] and [`TransitionBuilder::priority`] decide
//!   what is fireable and which of the fireable ones the default
//!   [`Selector`] picks; [`Net::set_selector`] replaces that choice, and
//!   [`Net::enable`] / [`Net::disable`] switch a transition off and on.
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
//! - `serde`: adds `NetBuilder::serializable` and `Net::json`, for inspectors
//!   that want structure rather than a `Debug` string. (Both are hidden from
//!   this page unless the feature is on.)

#![warn(missing_docs, missing_debug_implementations, unreachable_pub)]
#![forbid(unsafe_code)]

mod arity;
mod body;
mod builder;
mod distribution;
mod edge;
mod erased;
mod error;
mod event;
mod firing;
mod ids;
mod net;
mod place;
pub mod routing;
mod sealed;
mod transition;

pub use body::{IntoOutcome, IntoTokens, OutputPlaces, PositionalOutputs, TransitionBody, marker};
pub use builder::NetBuilder;
pub use distribution::{
    Distribution, DistributionError, OutputDistribution, WithOutputDistribution,
};
pub use edge::{
    EdgeInfo, EdgeKind, InputEdge, InputEdges, MutateEdge, SnapshotEdge, TakeAllEdge, TakeEdge,
};
pub use error::{BodyPanicked, BuildError, FiringFailed, NetError};
pub use event::Event;
pub use firing::{Completed, Firing, Selector};
pub use ids::{NetId, PlaceId, PlaceRef, TokenId, TransitionId};
pub use net::{Net, PlaceInfo};
pub use routing::{Either, RoutedOutput};
pub use transition::{TransitionBuilder, TransitionInfo};

/// Routes a returned value to the output place of its type; see
/// [`RoutedOutput`] and the [`routing`] module.
pub use petritype_derive::Routed;

/// The `serde_json` this crate was built against, re-exported so that callers
/// of [`Net::json`] need not match its version by hand.
#[cfg(feature = "serde")]
pub use serde_json;
