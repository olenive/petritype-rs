//! Typed, executable, observable Petri nets.
//!
//! A **place** is a FIFO queue of tokens of one Rust type. A **token** is one
//! owned value with a stable [`TokenId`]. A **transition** consumes tokens,
//! runs an ordinary Rust closure, and distributes the results. Every change to
//! the net is an [`Event`], so a visualiser can mirror the net in O(changes)
//! rather than O(state).
//!
//! This version covers *declaring* a net — places, tokens, transitions and
//! the edges between them — and reading it back. The wiring below is checked
//! when it compiles; firing it comes next.
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
//! # Ok::<(), petritype::BuildError>(())
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
//! - [`Net::places`], [`Net::transitions`], [`Net::tokens`], [`Net::label`]
//!   and [`Net::token`] read the net; [`Net::drain_events`] follows it.
//! - [`Net::extend`], [`Net::put`], [`Net::set_tokens`], [`Net::remove`] and
//!   [`Net::replace`] change the marking from outside.
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
mod ids;
mod net;
mod place;
mod sealed;
mod transition;

pub use body::{IntoOutcome, IntoTokens, OutputPlaces, PositionalOutputs, TransitionBody, marker};
pub use builder::NetBuilder;
pub use edge::{EdgeKind, InputEdge, InputEdges, MutateEdge, SnapshotEdge, TakeAllEdge, TakeEdge};
pub use error::{BuildError, NetError};
pub use event::Event;
pub use ids::{NetId, PlaceId, PlaceRef, TokenId, TransitionId};
pub use net::{Net, PlaceInfo};
pub use transition::{TransitionBuilder, TransitionInfo};

/// The `serde_json` this crate was built against, re-exported so that callers
/// of [`Net::json`] need not match its version by hand.
#[cfg(feature = "serde")]
pub use serde_json;
