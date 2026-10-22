//! A petritype net's activity, and the small live picture that shows it.
//!
//! The **activity thumbnail** is one square **cell** per place and
//! transition, lit by what the net is doing; with no activity it is the
//! net's icon. Nothing here draws on a screen: the crate computes the
//! numbers and the colours, and a renderer, such as the Bevy viewer, puts
//! them on one.
//!
//! - [`Activity`] is the model. Every node has two **levels**, each 0 to 1:
//!   a fast **pulse** and a slow **ember**. A **hit** raises them, when a
//!   transition fires or a token leaves, lands or changes in a place, and
//!   they decay by a half-life on one of two [clocks](Clock), wall time or
//!   the net's firings, chosen per layer. A place also has its **flow**, its recent
//!   balance of tokens added against taken, and a base glow from its token
//!   count. The **glow** drawn for a node is made from these by a
//!   [`GlowMode`]. It is fed the core's [`Event`](petritype::Event)s with
//!   [`Activity::apply`], or the same facts one by one through its hit
//!   interface, and knows the net only by its places' and transitions' ids
//!   and names, read off a [`Topology`](petritype::Topology) or given one by
//!   one ([`Activity::named`]) by a source that has none, such as a mirror
//!   of a net in another process.
//! - [`ActivitySettings`] holds every value the model and its drawing are
//!   tuned by, as plain data (with the `serde` feature, as JSON too).
//! - [`Grid`] says where each node's cell goes, from the ranks and orders of
//!   a whole-net [`Layout`](petritype_layout::Layout), never from its
//!   coordinates, with the nodes read off a topology or given by id and
//!   name ([`Grid::from_layout_named`]).
//! - [`Thumbnail`] is a colour per cell, a pure function of the activity,
//!   the grid, the [`HueMode`] and a [`Palette`]; [`Thumbnail::cells`] hands
//!   them to a renderer and [`Thumbnail::svg`] writes them as SVG.
//! - The **record** is the activity over time. The model keeps an account of
//!   what it applied, a [`Delta`] ([`Activity::take_delta`]), and a
//!   [`Recorder`] bins those deltas by a fixed width of time, in a ring of
//!   the last so many bins or all of them. A [`Record`] holds the bins with
//!   the grid they were taken against. It replays through a fresh model
//!   under any settings ([`Record::replay`], or a [`Player`] to scrub to any
//!   bin), coarsens exactly ([`Record::coarsen`]), lays itself out as a
//!   [`Tensor`] of time by rows by columns by eight channels, which writes
//!   itself in NumPy's `.npy` form, and with the `serde` feature is written
//!   and read as JSON, a file per record named by the net and the start.
//! - The **event log** keeps the order a record bins away: an [`EventLog`]
//!   holds each [`Entry`] the model took ([`Activity::apply_entry`]), with
//!   the host's position a record's bin names it by, in a ring whose
//!   [`Standing`] base is the counts its oldest kept entry starts from, and
//!   an [`EventPlayer`] replays it one entry per fixed dwell, so that an
//!   observer reads the order in which things happened rather than how long
//!   they took.
//!
//! ```
//! use std::time::Duration;
//!
//! use petritype::{NetBuilder, TakeEdge};
//! use petritype_activity::{Activity, ActivitySettings, Grid, Palette, Thumbnail};
//! use petritype_layout::{Layout, LayoutNode};
//!
//! let mut b = NetBuilder::new();
//! let inbox = b.place::<u32>("Inbox");
//! let out = b.place::<u32>("Out");
//! b.seed(inbox, [21]);
//! let double = b.transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,)).id();
//! let mut net = b.build()?;
//!
//! // Once per topology: a layout of the whole net, its grid, and the model.
//! let mut layout = Layout::default();
//! layout.compute(&net);
//! let grid = Grid::from_layout(&layout, &net.topology());
//! let mut activity = Activity::from_net(&net, ActivitySettings::default());
//!
//! // Every frame: what the net did, and the time that passed.
//! net.step()?;
//! for event in net.drain_events() {
//!     activity.apply(&event);
//! }
//! activity.tick(Duration::from_millis(300));
//!
//! // One pulse half-life on, the transition glows at a half.
//! assert!((activity.glow(LayoutNode::Transition(double)) - 0.5).abs() < 1e-5);
//!
//! let thumbnail = Thumbnail::new(&activity, &grid, activity.settings().hue, &Palette::DARK);
//! assert_eq!((thumbnail.columns(), thumbnail.rows()), (1, 3));
//! assert_eq!(thumbnail.cells().count(), 3);
//! assert!(thumbnail.svg(4).starts_with("<svg"));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! The crate depends on [`petritype`] and [`petritype_layout`] and on no
//! graphics library.

#![warn(missing_docs, missing_debug_implementations, unreachable_pub)]
#![forbid(unsafe_code)]

mod delta;
mod grid;
mod log;
mod model;
mod record;
mod settings;
mod svg;
mod thumbnail;

pub use delta::{Delta, PlaceDelta, TransitionDelta};
pub use grid::{Cell, Grid};
pub use log::{Entry, EventLog, EventPlayer, Logged, Standing};
pub use model::{Activity, PlaceActivity, TransitionActivity};
pub use record::{Bin, Player, Record, Recorder, Replay, Tensor};
pub use settings::{ActivitySettings, Clock, CountCurve, GlowMode, HueMode, LayerSettings};
pub use thumbnail::{Palette, Thumbnail};

/// The traffic light, petritype's example net, for tests that want a real
/// one.
#[cfg(test)]
#[allow(unreachable_pub, dead_code)]
#[path = "../../petritype/examples/traffic_light/net.rs"]
mod traffic_light;
