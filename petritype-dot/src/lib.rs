//! Graphviz DOT text of a petritype net and its marking.
//!
//! One function, [`dot`], turns a net into the source of a `digraph`. Feed
//! the string to `dot -Tsvg`, or to any Graphviz binding, and you have the
//! picture: places as ovals holding their tokens, transitions as boxes, and
//! the kinds of input edge drawn apart, so that a reader can tell a token
//! being *taken* from one being *read*, and an edge that waits for a token
//! from one that an empty place satisfies.
//!
//! The renderer works over the read API of DESIGN §8.2 and nothing else, so
//! it draws a live [`Net`] or an owned [`Snapshot`] alike ([`DotSource`] is
//! the one trait the two share here). That is the point of the snapshot: a
//! render thread owns a consistent copy of the state while the driver keeps
//! `&mut Net`, and the two renders are the same text:
//! `dot(&net) == dot(&net.snapshot()?)`.
//!
//! Nothing here is `&mut`, nothing drains events, and the output is a pure
//! function of the source and the [`Style`]: the same net and style always
//! give byte-identical text, which is what makes a diagram diffable and this
//! crate testable.
//!
//! # What it draws
//!
//! The shapes and colours mirror the Python petritype renderer
//! (`petritype/plotting/rustworkx_to_graphviz.py`,
//! `activation_coloured_attr_functions`), so a net ported from Python looks
//! like the Python one. Four things are additions, marked *new* below.
//!
//! | element | drawn as |
//! |---|---|
//! | place | `shape=oval, style=filled, fillcolor=deepskyblue, color=black, penwidth=1`; the label is the place name, then one line per token |
//! | transition | `shape=box, style=filled, fillcolor=lightgreen, color=black, penwidth=1`; a second label line `[A, B]` lists its [parameter](petritype::Param) types |
//! | transition, in flight | `fillcolor=gold, color=darkorange, penwidth=2` (this one is *working*) |
//! | transition, disabled | `fillcolor=lightgray, style="filled,dashed"` (*new*: the Python renderer had no disabled state) |
//! | highlighted node | `color=magenta` (`color=red` for a failure), `penwidth=2` on a place and `3` on a transition; an in-flight transition keeps its gold, as in Python |
//! | [`TakeEdge`](petritype::TakeEdge) | `color=black, penwidth=1` |
//! | [`TakeAllEdge`](petritype::TakeAllEdge) | the same, plus `label="*"` (*new*: the arity matters to a reader) |
//! | [`SnapshotEdge`](petritype::SnapshotEdge) | `style=dashed, color=dimgray, arrowhead=normal, penwidth=1` |
//! | [`MutateEdge`](petritype::MutateEdge) | `dir=both, arrowhead=dot, arrowtail=dot, color=dimgray, penwidth=2` |
//! | [`TakeWhereEdge`](petritype::TakeWhereEdge), [`MutateWhereEdge`](petritype::MutateWhereEdge) | as `TakeEdge` and `MutateEdge`, plus the filter as the label, `label="class in {Bird}"` (*new*: a keyed edge chooses its token, and the diagram says by what) |
//! | [`DrainEdge`](petritype::DrainEdge) | as `TakeAllEdge`, with `arrowhead=empty` (*new*: a hollow arrowhead marks an edge an empty place satisfies) |
//! | [`SurveyEdge`](petritype::SurveyEdge) | as `SnapshotEdge`, with `arrowhead=empty` and `label="*"` (*new*: every token, and possibly none) |
//! | an input edge of a kind this version does not know | as a `TakeEdge` |
//! | output edge | `color=black, penwidth=1`, plus `label="<slot>"` when the transition has more than one output (*new*: positional order is what a positional body returns into) |
//! | highlighted edge | both its ends highlighted: the kind's shape is kept and its `color` and `penwidth` become `magenta`/`red` and `2` |
//!
//! A token is drawn as its [`label`](petritype::Net::label), its `Debug`
//! truncated by the net's own limit, and never as its
//! [`TokenId`]: a diagram reads values, not ids. Long
//! places are cut off after [`Style::max_tokens`] lines with one `… and N
//! more`.
//!
//! # Drawing a net
//!
//! ```
//! use petritype::{NetBuilder, TakeEdge};
//!
//! let mut b = NetBuilder::new();
//! let inbox = b.place::<u32>("Inbox");
//! let out = b.place::<u32>("Out");
//! b.seed(inbox, [21]);
//! b.transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,));
//! let net = b.build()?;
//!
//! let text = petritype_dot::dot(&net);
//! assert!(text.starts_with("digraph \"petritype\" {"));
//! assert!(text.contains("shape=oval"));
//! assert!(text.contains(r#"label="Inbox\n21""#), "the place and its one token");
//! assert!(text.contains("p0 -> t0 [color=black, penwidth=1];"));
//!
//! // The same text off an owned copy of the state, for a render thread.
//! assert_eq!(text, petritype_dot::dot(&net.snapshot()?));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Showing what just happened
//!
//! The event log says what changed; [`Highlight::from_event`] turns one
//! event into the outline for the next frame, which is how an animation
//! shows a firing rather than only its result.
//!
//! ```
//! use petritype::{NetBuilder, TakeEdge};
//! use petritype_dot::{Highlight, Style, dot_with};
//!
//! # let mut b = NetBuilder::new();
//! # let inbox = b.place::<u32>("Inbox");
//! # let out = b.place::<u32>("Out");
//! # b.seed(inbox, [21]);
//! # b.transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,));
//! let mut net = b.build()?;
//! net.step()?;
//!
//! let last = net.drain_events().next_back().expect("the firing was logged");
//! let style = Style {
//!     highlight: Highlight::from_event(&last),
//!     ..Style::default()
//! };
//!
//! let text = dot_with(&net, &style);
//! assert!(text.contains("color=magenta, penwidth=3"), "the transition that fired");
//! assert!(text.contains("color=magenta, penwidth=2"), "the places it touched");
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![warn(missing_docs, missing_debug_implementations, unreachable_pub)]
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::fmt::{self, Write as _};

use petritype::{
    Clause, EdgeInfo, EdgeKind, Event, Filter, Net, PlaceId, PlaceInfo, Snapshot, TokenId,
    TransitionId, TransitionInfo,
};

/// Anything the renderer can draw: a live [`Net`] or an owned [`Snapshot`].
///
/// The methods are the read API of DESIGN §8.2, narrowed to what a diagram
/// needs, and they carry the names [`Net`] gives them. One deliberate
/// difference: [`token_ids`](DotSource::token_ids) yields ids alone, because
/// a diagram draws [`label`](DotSource::label)s rather than values, and the
/// erased `&dyn Any` of `Net::tokens` would only be thrown away.
///
/// Every method takes `&self`. A renderer never writes to the net, and never
/// drains its events.
pub trait DotSource {
    /// Every place, in id order, with its name, token type and count:
    /// [`Net::places`].
    fn places(&self) -> impl Iterator<Item = (PlaceId, PlaceInfo<'_>)>;

    /// Every transition, in declaration order, with its inputs, parameter
    /// type names, outputs and switches: [`Net::transitions`].
    fn transitions(&self) -> impl Iterator<Item = (TransitionId, TransitionInfo<'_>)>;

    /// Every edge, one at a time, in [`Net::edges`] order: each transition in
    /// id order, its input edges and then its output edges.
    fn edges(&self) -> impl Iterator<Item = EdgeInfo>;

    /// The ids of a place's tokens, oldest first: [`Net::tokens`] without the
    /// values.
    fn token_ids(&self, place: PlaceId) -> impl Iterator<Item = TokenId>;

    /// One token rendered with its type's `Debug`, already truncated:
    /// [`Net::label`].
    fn label(&self, place: PlaceId, token: TokenId) -> Option<String>;
}

impl DotSource for Net {
    fn places(&self) -> impl Iterator<Item = (PlaceId, PlaceInfo<'_>)> {
        Net::places(self)
    }

    fn transitions(&self) -> impl Iterator<Item = (TransitionId, TransitionInfo<'_>)> {
        Net::transitions(self)
    }

    fn edges(&self) -> impl Iterator<Item = EdgeInfo> {
        Net::edges(self)
    }

    fn token_ids(&self, place: PlaceId) -> impl Iterator<Item = TokenId> {
        Net::tokens(self, place).map(|(id, _)| id)
    }

    fn label(&self, place: PlaceId, token: TokenId) -> Option<String> {
        Net::label(self, place, token)
    }
}

impl DotSource for Snapshot {
    fn places(&self) -> impl Iterator<Item = (PlaceId, PlaceInfo<'_>)> {
        Snapshot::places(self)
    }

    fn transitions(&self) -> impl Iterator<Item = (TransitionId, TransitionInfo<'_>)> {
        Snapshot::transitions(self)
    }

    fn edges(&self) -> impl Iterator<Item = EdgeInfo> {
        Snapshot::edges(self)
    }

    fn token_ids(&self, place: PlaceId) -> impl Iterator<Item = TokenId> {
        Snapshot::tokens(self, place).map(|(id, _)| id)
    }

    fn label(&self, place: PlaceId, token: TokenId) -> Option<String> {
        Snapshot::label(self, place, token)
    }
}

/// Which way Graphviz lays the graph out.
///
/// [`TB`](RankDir::TB) is Graphviz's own default, and what the Python
/// renderer drew with; [`LR`](RankDir::LR) suits a long pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RankDir {
    /// Top to bottom.
    #[default]
    TB,
    /// Left to right.
    LR,
}

impl RankDir {
    /// The `rankdir=` value, as Graphviz spells it.
    fn as_str(self) -> &'static str {
        match self {
            RankDir::TB => "TB",
            RankDir::LR => "LR",
        }
    }
}

/// What to outline as "this just happened", derived from one [`Event`] by
/// [`from_event`](Highlight::from_event).
///
/// One transition and the places that firing touched. A renderer holds the
/// highlight for one frame: the event log is the cheap update channel, and
/// this is what a frame makes of the last entry in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Highlight {
    /// The transition the event was about.
    pub transition: TransitionId,
    /// The places it consumed from or produced into, in that order, each
    /// named once.
    pub places: Vec<PlaceId>,
    /// Whether the event was an [`Event::Failed`], which is drawn red rather
    /// than magenta.
    pub failed: bool,
}

impl Highlight {
    /// The highlight one event asks for, or `None` for an event that is not
    /// about a firing.
    ///
    /// - [`Fired`](Event::Fired): the transition, and the places it consumed
    ///   from and then produced into.
    /// - [`Distributed`](Event::Distributed): the transition and the places
    ///   it produced into. What it consumed was reported by the
    ///   [`Launched`](Event::Launched) that opened the firing, which is not
    ///   this event.
    /// - [`Launched`](Event::Launched): the transition and the places it
    ///   consumed from. The firing is airborne, and the transition will also
    ///   draw itself gold.
    /// - [`Failed`](Event::Failed): the transition and the places it consumed
    ///   from, with [`failed`](Highlight::failed) set.
    /// - Everything else ([`Added`](Event::Added), [`Removed`](Event::Removed),
    ///   [`Mutated`](Event::Mutated), [`Toggled`](Event::Toggled),
    ///   [`ParamChanged`](Event::ParamChanged)) names no firing, so there is
    ///   nothing to outline: `None`.
    ///
    /// ```
    /// # use petritype::{Event, NetBuilder, TakeEdge};
    /// # use petritype_dot::Highlight;
    /// # let mut b = NetBuilder::new();
    /// # let inbox = b.place::<u32>("Inbox");
    /// # let out = b.place::<u32>("Out");
    /// # b.seed(inbox, [1]);
    /// # let double = b
    /// #     .transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,))
    /// #     .id();
    /// # let mut net = b.build()?;
    /// net.step()?;
    /// let fired = net.drain_events().next_back().expect("one firing");
    /// let highlight = Highlight::from_event(&fired).expect("a firing is highlightable");
    /// assert_eq!(highlight.transition, double);
    /// assert_eq!(highlight.places, [inbox.id(), out.id()]);
    /// assert!(!highlight.failed);
    ///
    /// // A parameter change is not a firing.
    /// assert_eq!(Highlight::from_event(&Event::ParamChanged { type_name: "Cfg" }), None);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn from_event(event: &Event) -> Option<Highlight> {
        let (transition, touched, failed): (_, Vec<&[(PlaceId, TokenId)]>, _) = match event {
            Event::Fired {
                transition,
                consumed,
                produced,
            } => (*transition, vec![consumed, produced], false),
            Event::Launched {
                transition,
                consumed,
            } => (*transition, vec![consumed], false),
            Event::Distributed {
                transition,
                produced,
            } => (*transition, vec![produced], false),
            Event::Failed {
                transition,
                consumed,
                ..
            } => (*transition, vec![consumed], true),
            _ => return None,
        };
        let mut places: Vec<PlaceId> = Vec::new();
        for (place, _) in touched.into_iter().flatten() {
            if !places.contains(place) {
                places.push(*place);
            }
        }
        Some(Highlight {
            transition,
            places,
            failed,
        })
    }

    /// The outline colour this highlight draws with.
    fn colour(&self) -> &'static str {
        if self.failed { "red" } else { "magenta" }
    }
}

/// How to draw: layout, how much of the marking to show, and what to outline.
///
/// An ordinary exhaustive struct with a [`Default`], like
/// [`drivers::Limits`](petritype::drivers::Limits), so one field is changed
/// by naming it and taking the rest:
///
/// ```
/// use petritype_dot::{RankDir, Style};
///
/// let wide = Style {
///     rankdir: RankDir::LR,
///     ..Style::default()
/// };
/// assert_eq!(wide.max_tokens, 10);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    /// Which way Graphviz lays the graph out. Default [`RankDir::TB`].
    pub rankdir: RankDir,
    /// Whether a place's label lists its tokens, one per line under the
    /// place's name. Default true; false draws the topology alone.
    pub show_tokens: bool,
    /// How many token lines a place shows before it gives up and writes
    /// `… and N more`. Default 10.
    pub max_tokens: usize,
    /// The firing to outline, usually [`Highlight::from_event`] of the last
    /// event. Default `None`: a resting diagram.
    pub highlight: Option<Highlight>,
    /// The name of the `digraph`. Default `"petritype"`.
    pub graph_name: String,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            rankdir: RankDir::default(),
            show_tokens: true,
            max_tokens: 10,
            highlight: None,
            graph_name: String::from("petritype"),
        }
    }
}

impl Style {
    /// Whether this place is part of the highlighted firing.
    fn highlights_place(&self, place: PlaceId) -> bool {
        self.highlight
            .as_ref()
            .is_some_and(|highlight| highlight.places.contains(&place))
    }

    /// Whether this is the highlighted transition.
    fn highlights_transition(&self, transition: TransitionId) -> bool {
        self.highlight
            .as_ref()
            .is_some_and(|highlight| highlight.transition == transition)
    }

    /// The outline colour, or `"magenta"` when nothing is highlighted and the
    /// answer is never asked for.
    fn colour(&self) -> &'static str {
        self.highlight.as_ref().map_or("magenta", Highlight::colour)
    }
}

/// The DOT source of a net or a snapshot, with [`Style::default`].
///
/// See the [crate docs](self) for what it draws; [`dot_with`] is the same
/// with the style in hand.
pub fn dot(source: &impl DotSource) -> String {
    dot_with(source, &Style::default())
}

/// The DOT source of a net or a snapshot, drawn the given way.
///
/// ```
/// # use petritype::{NetBuilder, TakeEdge};
/// # use petritype_dot::{RankDir, Style, dot_with};
/// # let mut b = NetBuilder::new();
/// # let inbox = b.place::<u32>("Inbox");
/// # let out = b.place::<u32>("Out");
/// # b.seed(inbox, [1, 2, 3]);
/// # b.transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,));
/// # let net = b.build()?;
/// let topology_only = Style {
///     rankdir: RankDir::LR,
///     show_tokens: false,
///     graph_name: String::from("pipeline"),
///     ..Style::default()
/// };
/// let text = dot_with(&net, &topology_only);
/// assert!(text.starts_with("digraph \"pipeline\" {\n  rankdir=LR;"));
/// assert!(text.contains(r#"label="Inbox""#), "the name, and no tokens under it");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn dot_with(source: &impl DotSource, style: &Style) -> String {
    let mut out = String::new();
    render(source, style, &mut out).expect("writing into a String cannot fail");
    out
}

/// The whole graph. Split out so that the one `expect` above covers every
/// `write!` in the crate: a `String` is an infallible `fmt::Write`.
fn render(source: &impl DotSource, style: &Style, out: &mut String) -> fmt::Result {
    let places: HashMap<PlaceId, usize> = source
        .places()
        .enumerate()
        .map(|(index, (id, _))| (id, index))
        .collect();
    let transitions: HashMap<TransitionId, usize> = source
        .transitions()
        .enumerate()
        .map(|(index, (id, _))| (id, index))
        .collect();
    // How many output places each transition declared, which decides whether
    // an output edge needs its slot number spelled out.
    let outputs: HashMap<TransitionId, usize> = source
        .transitions()
        .map(|(id, info)| (id, info.outputs.len()))
        .collect();

    writeln!(out, "digraph \"{}\" {{", escape(&style.graph_name))?;
    writeln!(out, "  rankdir={};", style.rankdir.as_str())?;
    writeln!(out, "  node [fontname=\"Helvetica\"];")?;

    for (index, (id, info)) in source.places().enumerate() {
        let (colour, penwidth) = if style.highlights_place(id) {
            (style.colour(), 2)
        } else {
            ("black", 1)
        };
        writeln!(
            out,
            "  p{index} [shape=oval, style=filled, fillcolor=deepskyblue, \
             color={colour}, penwidth={penwidth}, label=\"{}\"];",
            escape(&place_label(source, style, id, info.name)),
        )?;
    }

    for (index, (id, info)) in source.transitions().enumerate() {
        let mut fillcolor = "lightgreen";
        let mut node_style = "filled";
        let mut colour = "black";
        let mut penwidth = 1;
        if !info.enabled {
            fillcolor = "lightgray";
            node_style = "\"filled,dashed\"";
        }
        if style.highlights_transition(id) {
            colour = style.colour();
            penwidth = 3;
        }
        // In flight wins over the outline, as in the Python renderer: what a
        // transition is doing right now beats what it did a moment ago.
        if info.in_flight {
            fillcolor = "gold";
            colour = "darkorange";
            penwidth = 2;
        }
        writeln!(
            out,
            "  t{index} [shape=box, style={node_style}, fillcolor={fillcolor}, \
             color={colour}, penwidth={penwidth}, label=\"{}\"];",
            escape(&transition_label(info.name, info.params)),
        )?;
    }

    for edge in source.edges() {
        let drawn = match edge {
            EdgeInfo::Input {
                place,
                transition,
                kind,
                filter,
                ..
            } => {
                let lit = style.highlights_place(place) && style.highlights_transition(transition);
                Some((
                    format!("p{}", places[&place]),
                    format!("t{}", transitions[&transition]),
                    input_attrs(kind, filter.as_ref(), lit.then(|| style.colour())),
                ))
            }
            EdgeInfo::Output {
                transition,
                place,
                slot,
            } => {
                let lit = style.highlights_place(place) && style.highlights_transition(transition);
                let slot = (outputs[&transition] > 1).then_some(slot);
                Some((
                    format!("t{}", transitions[&transition]),
                    format!("p{}", places[&place]),
                    output_attrs(slot, lit.then(|| style.colour())),
                ))
            }
            // `EdgeInfo` is `#[non_exhaustive]`: a kind of edge this renderer
            // has never heard of is left undrawn rather than guessed at.
            _ => None,
        };
        let Some((from, to, attrs)) = drawn else {
            continue;
        };
        writeln!(out, "  {from} -> {to} [{attrs}];")?;
    }

    writeln!(out, "}}")
}

/// A place's label: its name, then one line per token while there is room.
fn place_label(source: &impl DotSource, style: &Style, place: PlaceId, name: &str) -> String {
    let mut label = String::from(name);
    if !style.show_tokens {
        return label;
    }
    let tokens: Vec<TokenId> = source.token_ids(place).collect();
    for token in tokens.iter().take(style.max_tokens) {
        label.push('\n');
        label.push_str(&source.label(place, *token).unwrap_or_default());
    }
    if let Some(more) = tokens
        .len()
        .checked_sub(style.max_tokens)
        .filter(|n| *n > 0)
    {
        label.push('\n');
        label.push_str(&format!("… and {more} more"));
    }
    label
}

/// A transition's label: its name, and its parameter types in brackets.
///
/// The types are shortened to their last path segment, because a diagram has
/// no room for `alloc::vec::Vec<my_crate::domain::Cfg>` and a reader loses
/// nothing by seeing `Vec<Cfg>`.
fn transition_label(name: &str, params: &[&'static str]) -> String {
    let mut label = String::from(name);
    if !params.is_empty() {
        let names: Vec<String> = params.iter().map(|param| short_type_name(param)).collect();
        label.push('\n');
        label.push('[');
        label.push_str(&names.join(", "));
        label.push(']');
    }
    label
}

/// The attributes of a place → transition edge, by the kind of read it is,
/// with a keyed edge's filter as its label.
fn input_attrs(kind: EdgeKind, filter: Option<&Filter>, highlight: Option<&str>) -> String {
    let mut attrs: Vec<String> = match kind {
        EdgeKind::Take | EdgeKind::TakeWhere => {
            vec![String::from("color=black"), String::from("penwidth=1")]
        }
        EdgeKind::TakeAll => vec![
            String::from("color=black"),
            String::from("penwidth=1"),
            String::from("label=\"*\""),
        ],
        EdgeKind::Snapshot => vec![
            String::from("style=dashed"),
            String::from("color=dimgray"),
            String::from("arrowhead=normal"),
            String::from("penwidth=1"),
        ],
        EdgeKind::Mutate | EdgeKind::MutateWhere => vec![
            String::from("dir=both"),
            String::from("arrowhead=dot"),
            String::from("arrowtail=dot"),
            String::from("color=dimgray"),
            String::from("penwidth=2"),
        ],
        // The two that read every token there is, nothing included: their
        // all-at-once counterparts with a hollow arrowhead, which is the
        // mark of an edge that an empty place does not hold back.
        EdgeKind::Drain => vec![
            String::from("color=black"),
            String::from("arrowhead=empty"),
            String::from("penwidth=1"),
            String::from("label=\"*\""),
        ],
        EdgeKind::Survey => vec![
            String::from("style=dashed"),
            String::from("color=dimgray"),
            String::from("arrowhead=empty"),
            String::from("penwidth=1"),
            String::from("label=\"*\""),
        ],
        // `EdgeKind` is `#[non_exhaustive]`: a kind this renderer has never
        // heard of is drawn as a plain take, as a protocol reader would.
        _ => vec![String::from("color=black"), String::from("penwidth=1")],
    };
    if let Some(filter) = filter {
        attrs.push(format!("label=\"{}\"", escape(&filter_label(filter))));
    }
    join_attrs(attrs, highlight)
}

/// A keyed edge's filter as a reader writes one: `class in {Bird, Mammal}
/// and diet in {Insectivore}`, a parameter clause as `class = $season`, and
/// `any` for a filter with no clauses. Its places and prefix are left out:
/// an edge reads one place, and does not consult them.
fn filter_label(filter: &Filter) -> String {
    if filter.clauses.is_empty() {
        return String::from("any");
    }
    filter
        .clauses
        .iter()
        .map(|clause| match clause {
            Clause::In { key, values } => {
                let values: Vec<String> = values.iter().map(ToString::to_string).collect();
                format!("{key} in {{{}}}", values.join(", "))
            }
            Clause::IsParam { key, param } => format!("{key} = ${param}"),
            // `Clause` is `#[non_exhaustive]`: a condition this version
            // cannot spell is still a condition.
            _ => String::from("?"),
        })
        .collect::<Vec<_>>()
        .join(" and ")
}

/// The attributes of a transition → place edge, with its slot number when the
/// transition has more than one output to tell apart.
fn output_attrs(slot: Option<usize>, highlight: Option<&str>) -> String {
    let mut attrs = vec![String::from("color=black"), String::from("penwidth=1")];
    if let Some(slot) = slot {
        attrs.push(format!("label=\"{slot}\""));
    }
    join_attrs(attrs, highlight)
}

/// Joins an edge's attributes, recolouring it if it is part of the
/// highlighted firing.
///
/// A highlight replaces `color` and `penwidth` and leaves everything else
/// alone, so a read edge stays dashed and a mutating edge keeps its dots: the
/// outline says *what happened*, the shape says *what kind of edge it is*,
/// and neither should hide the other.
fn join_attrs(attrs: Vec<String>, highlight: Option<&str>) -> String {
    let attrs: Vec<String> = match highlight {
        None => attrs,
        Some(colour) => attrs
            .into_iter()
            .map(|attr| {
                if attr.starts_with("color=") {
                    format!("color={colour}")
                } else if attr.starts_with("penwidth=") {
                    String::from("penwidth=2")
                } else {
                    attr
                }
            })
            .collect(),
    };
    attrs.join(", ")
}

/// A type name with its module path dropped, generics kept and shortened too:
/// `alloc::vec::Vec<my::Cfg>` becomes `Vec<Cfg>`.
///
/// `std::any::type_name` is fully qualified, which is right for an error
/// message and far too long for a box in a diagram.
fn short_type_name(name: &str) -> String {
    const DELIMITERS: [char; 9] = ['<', '>', ',', ' ', '(', ')', '[', ']', '&'];
    let mut out = String::with_capacity(name.len());
    let mut segment = String::new();
    for ch in name.chars() {
        if DELIMITERS.contains(&ch) {
            out.push_str(last_segment(&segment));
            segment.clear();
            out.push(ch);
        } else {
            segment.push(ch);
        }
    }
    out.push_str(last_segment(&segment));
    out
}

/// The part of a path after its last `::`.
fn last_segment(path: &str) -> &str {
    path.rsplit("::").next().unwrap_or(path)
}

/// A string as a DOT double-quoted label.
///
/// A token's label is whatever its `Debug` printed, so it may hold anything:
/// a quote, a backslash, a newline. The first two are escaped and the third
/// becomes a `\n`, which Graphviz draws as a line break. That is exactly
/// what the multi-line labels this crate builds want.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{escape, join_attrs, last_segment, short_type_name, transition_label};

    #[test]
    fn short_type_name_keeps_a_plain_name() {
        assert_eq!(short_type_name("u32"), "u32");
        assert_eq!(short_type_name("Cfg"), "Cfg");
    }

    #[test]
    fn short_type_name_drops_a_path() {
        assert_eq!(short_type_name("my::domain::Cfg"), "Cfg");
        assert_eq!(short_type_name("alloc::string::String"), "String");
    }

    #[test]
    fn short_type_name_keeps_generics_and_shortens_them_too() {
        assert_eq!(short_type_name("alloc::vec::Vec<my::Cfg>"), "Vec<Cfg>");
        assert_eq!(short_type_name("core::option::Option<u32>"), "Option<u32>");
    }

    #[test]
    fn short_type_name_handles_nested_generics_and_references() {
        assert_eq!(
            short_type_name(
                "std::collections::HashMap<alloc::string::String, alloc::vec::Vec<my::Cfg>>"
            ),
            "HashMap<String, Vec<Cfg>>",
        );
        assert_eq!(short_type_name("&core::primitive::str"), "&str");
        assert_eq!(short_type_name("(my::A, my::B)"), "(A, B)");
    }

    #[test]
    fn last_segment_of_a_bare_name_is_itself() {
        assert_eq!(last_segment(""), "");
        assert_eq!(last_segment("A"), "A");
        assert_eq!(last_segment("a::b::C"), "C");
    }

    #[test]
    fn escaping_covers_quotes_backslashes_and_newlines() {
        assert_eq!(escape("plain"), "plain");
        assert_eq!(escape("a\"b"), "a\\\"b");
        assert_eq!(escape("a\\b"), "a\\\\b");
        assert_eq!(escape("a\nb"), "a\\nb");
        assert_eq!(escape("a\r\tb"), "a\\r\\tb");
        // A `Debug`-printed string arrives already escaped once, and is
        // escaped again on the way into DOT: `q"\` debug-prints as the seven
        // characters `"q\"\\"`, and every backslash and quote in them doubles.
        assert_eq!(format!("{:?}", "q\"\\"), r#""q\"\\""#);
        assert_eq!(escape(r#""q\"\\""#), r#"\"q\\\"\\\\\""#);
    }

    #[test]
    fn a_transition_label_brackets_its_parameter_types() {
        assert_eq!(transition_label("Fetch", &[]), "Fetch");
        assert_eq!(
            transition_label("Fetch", &["my::ApiToken", "alloc::vec::Vec<my::Cfg>"]),
            "Fetch\n[ApiToken, Vec<Cfg>]",
        );
    }

    #[test]
    fn a_highlight_recolours_an_edge_without_reshaping_it() {
        let dashed = vec![
            String::from("style=dashed"),
            String::from("color=dimgray"),
            String::from("arrowhead=normal"),
            String::from("penwidth=1"),
        ];
        assert_eq!(
            join_attrs(dashed.clone(), None),
            "style=dashed, color=dimgray, arrowhead=normal, penwidth=1",
        );
        assert_eq!(
            join_attrs(dashed, Some("magenta")),
            "style=dashed, color=magenta, arrowhead=normal, penwidth=2",
        );
    }
}
