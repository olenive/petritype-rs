//! Typed, executable, observable Petri nets.
//!
//! A **place** is a FIFO queue of tokens of one Rust type. A **token** is one
//! owned value with a stable [`TokenId`]. A **transition** consumes tokens,
//! runs an ordinary Rust closure, and distributes the results. Every change to
//! the net is an [`Event`], so a visualiser can mirror the net in O(changes)
//! rather than O(state).
//!
//! This version covers *declaring* a net — places, tokens, transitions and
//! the edges between them — reading it back, and firing it one transition at
//! a time. The wiring below is checked when it compiles. A body may return
//! its outputs as a tuple, slot by slot, or as one value to be [routed to the
//! output place of its type](routing).
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
//! - [`RoutedOutput`] is the other output form: a body that returns a value
//!   rather than a tuple has each payload sent to the output place of its
//!   type, which is what [`#[derive(Routed)]`](Routed) and [`Either`] are
//!   for — see the [`routing`] module.
//! - [`Net::places`], [`Net::transitions`], [`Net::tokens`], [`Net::label`]
//!   and [`Net::token`] read the net; [`Net::drain_events`] follows it.
//! - [`Net::extend`], [`Net::put`], [`Net::set_tokens`], [`Net::remove`] and
//!   [`Net::replace`] change the marking from outside.
//! - **fireable** and **enabled** are two different questions:
//!   [`Net::enable`] / [`Net::disable`] work the author's on/off switch,
//!   while *fireable* means "could fire right now" — switched on, input
//!   places non-empty, guard saying yes.
//! - [`Net::fireable`] and [`Net::is_fireable`] say what could fire now;
//!   [`Net::step`] fires one transition, [`Net::fire`] fires a named one,
//!   and a body that returns `Err` gives [`FiringFailed`].
//! - [`TransitionBuilder::guard`] and [`TransitionBuilder::priority`] decide
//!   what is fireable and which of the fireable ones the default
//!   [`Selector`] picks; [`Net::set_selector`] replaces that choice, and
//!   [`Net::enable`] / [`Net::disable`] switch a transition off and on.
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
pub use edge::{EdgeKind, InputEdge, InputEdges, MutateEdge, SnapshotEdge, TakeAllEdge, TakeEdge};
pub use error::{BuildError, FiringFailed, NetError};
pub use event::Event;
pub use firing::Selector;
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
