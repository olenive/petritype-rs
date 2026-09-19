//! Typed, executable, observable Petri nets.
//!
//! A **place** is a FIFO queue of tokens of one Rust type. A **token** is one
//! owned value with a stable [`TokenId`]. A **transition** (not yet in this
//! version) consumes tokens, runs an ordinary Rust closure, and distributes
//! the results. Every change to the net is an [`Event`], so a visualiser can
//! mirror the net in O(changes) rather than O(state).
//!
//! This version covers places and tokens: declaring places, seeding them,
//! reading them, and changing them from outside the net. Transitions and
//! firing follow.
//!
//! ```
//! use petritype::{Event, NetBuilder};
//!
//! #[derive(Debug, Clone, PartialEq)]
//! struct Parcel {
//!     id: u32,
//!     weight: f32,
//! }
//!
//! let mut b = NetBuilder::new();
//! let inbox = b.place::<Parcel>("Inbox");                 // PlaceRef<Parcel>
//! b.seed(inbox, [Parcel { id: 1, weight: 3.0 }]);
//! let mut net = b.build()?;
//!
//! // The seeded marking is where the net starts: no events, counter at zero.
//! // A view reads it with `places()` / `tokens()` when it attaches.
//! assert_eq!(net.count(inbox.id()), 1);
//! assert_eq!(net.state_counter(), 0);
//! assert_eq!(net.drain_events().count(), 0);
//!
//! // Input from outside the net: `&mut Net` is the single-writer guarantee.
//! let heavy = net.put(inbox, Parcel { id: 2, weight: 30.0 });
//! assert_eq!(net.count(inbox.id()), 2);
//! assert_eq!(net.token::<Parcel>(inbox.id(), heavy).unwrap().weight, 30.0);
//! assert_eq!(net.label(inbox.id(), heavy).unwrap(), "Parcel { id: 2, weight: 30.0 }");
//!
//! // Changes after that are events, which is what the view has to redraw.
//! assert_eq!(net.state_counter(), 1);
//! assert_eq!(
//!     net.drain_events().collect::<Vec<_>>(),
//!     [Event::Added { place: inbox.id(), token: heavy }],
//! );
//! # Ok::<(), petritype::BuildError>(())
//! ```
//!
//! # Where things are
//!
//! - [`NetBuilder`] declares places and produces a [`Net`].
//! - [`PlaceRef<T>`] is the typed handle a builder hands back; [`PlaceId`],
//!   [`TransitionId`] and [`TokenId`] are the erased ids that events and the
//!   read API speak in.
//! - [`Net::places`], [`Net::tokens`], [`Net::label`] and [`Net::token`] read
//!   the marking; [`Net::drain_events`] follows it.
//! - [`Net::extend`], [`Net::put`], [`Net::set_tokens`], [`Net::remove`] and
//!   [`Net::replace`] change it from outside.
//!
//! # Features
//!
//! - `serde`: adds `NetBuilder::serializable` and `Net::json`, for inspectors
//!   that want structure rather than a `Debug` string. (Both are hidden from
//!   this page unless the feature is on.)

#![warn(missing_docs, missing_debug_implementations, unreachable_pub)]
#![forbid(unsafe_code)]

mod builder;
mod error;
mod event;
mod ids;
mod net;
mod place;

pub use builder::NetBuilder;
pub use error::{BuildError, NetError};
pub use event::Event;
pub use ids::{PlaceId, PlaceRef, TokenId, TransitionId};
pub use net::{Net, PlaceInfo};

/// The `serde_json` this crate was built against, re-exported so that callers
/// of [`Net::json`] need not match its version by hand.
#[cfg(feature = "serde")]
pub use serde_json;
