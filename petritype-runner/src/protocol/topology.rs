//! A net's shape keyed by names: its places, transitions, edges, groups
//! and controls.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::Filter;

/// The net's shape keyed by names (PROTOCOL.md §5.1). No `hints`: nothing in
/// the core produces positions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Topology {
    /// Every place, in id order.
    pub places: Vec<Place>,
    /// Every transition, in declaration order.
    pub transitions: Vec<Transition>,
    /// Every edge, transition by transition, inputs then outputs.
    pub edges: Vec<Edge>,
    /// The net's own groups, in declared order (§5.1); written only when
    /// there are some. An inner net's topology carries its own groups and
    /// none of the net around it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,
    /// The controls the net declares, in declaration order (§5.1); written
    /// only when there are some.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controls: Vec<Control>,
    /// Whether the net's definition declares it open (§5.1, since
    /// 2026-11-23): `open: true`, written only then. A net without `open`
    /// is closed: from outside it, tokens are written only at the places it
    /// shows as `port`, and an `inject` anywhere else is `closed_net`, so a
    /// net that shows no port takes nothing from outside, and nor does any
    /// net inside it, at a port or not. (Until 2026-11-28 in this runner, a
    /// net that showed no port was open too, and a closed net shielded
    /// nothing inside it.)
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub open: bool,
}

/// A control in [`Topology`]: a widget a viewer renders, which `press`
/// names (§5.1, §6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Control {
    /// Unique within the net; what `press` names.
    pub name: String,
    /// What the viewer shows.
    pub label: String,
    /// `"button"`, `"toggle"`, `"slider"`, `"select"` or `"text"`: the
    /// widget, and the value a press carries.
    pub kind: String,
    /// The place or transition the control acts on, by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    /// `"inject"`, `"fire"` or `"enable"`: what a press does, for the
    /// viewer's lighting rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// The kind's settings; omitted when it has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<ControlConfig>,
}

/// A [`Control`]'s `config`: what its kind needs, each field written only
/// when it has one.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ControlConfig {
    /// The initial value, for the kinds that have one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    /// A select's choices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
    /// A slider's smallest value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<f64>,
    /// A slider's largest value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<f64>,
    /// A slider's increment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    /// A text box's hint while empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
}

impl ControlConfig {
    /// Whether nothing is set, in which case a control has no `config`.
    pub fn is_empty(&self) -> bool {
        *self == ControlConfig::default()
    }
}

/// A place in [`Topology`].
///
/// A subnet place (PROTOCOL.md §12.7) is `{name, type_name, subnet: true,
/// ports, stepping, group?}`: its `type_name` the template that built its
/// subnet, or `""`, and `ports` its subnet's port places in the subnet's
/// declaration order, each as the subnet's own topology has it but without
/// `port` and `group`. It is not serializable, holding no tokens of its
/// own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    /// Unique within the net.
    pub name: String,
    /// The Rust type name of its tokens, for display; for a subnet place,
    /// its subnet's template, or `""`.
    pub type_name: String,
    /// Whether its tokens have a JSON form. Not written for a subnet place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serializable: Option<bool>,
    /// Whether its tokens may hold nets of their own (§12.2); written only
    /// when true.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub nested: bool,
    /// Whether it is a subnet place (§12.7); written only when true.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub subnet: bool,
    /// A subnet place's ports (§12.7); written for a subnet place alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<Place>>,
    /// Whether the place is one of the net's ports, where it takes tokens
    /// from outside (§5.1, §12.7), in the net's own topology, whichever net
    /// it is: the loaded net, a net a token carries or a subnet (since
    /// 2026-11-23; until then only a subnet's topology showed it). Written
    /// only when true, so never on a port listed in a subnet place's
    /// `ports`, a list that holds ports alone.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub port: bool,
    /// The rule by which deep stepping fires the nets the place holds
    /// (§5.1, §12.3): written exactly for a place that holds nets, the
    /// default rule included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stepping: Option<Stepping>,
    /// The keys the place derives from its tokens, in declaration order
    /// (§13.1); written only when there are some.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub key_names: Vec<String>,
    /// How the place itself is drawn (§5.1); written only when it has a
    /// style.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<PlaceStyle>,
    /// The name of the group the place is in, one of [`Topology::groups`]
    /// (§5.1); written only for a place in a group, and never for a port
    /// listed in its subnet place's `ports`, whose group is the subnet's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

/// A [`Place`]'s `stepping` (PROTOCOL.md §12.3): when the nets in the place
/// fire, measured against the net around them, by the protocol's four
/// names. The core's rule is `petritype::Stepping`, and the runner writes it
/// as this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stepping {
    /// `"inner_first"`, the default: the nets fire until they are at rest
    /// before the net around them fires.
    InnerFirst,
    /// `"take_turns"`: the net around them and each net here fire in turn.
    TakeTurns,
    /// `"outer_first"`: the nets fire only when the net around them and its
    /// turn-takers have nothing to fire.
    OuterFirst,
    /// `"passive"`: deep stepping never enters the place; the program or a
    /// body steps its nets.
    Passive,
}

/// A [`Place`]'s `style`: `{"fill": "token"}`, a rule the viewer
/// evaluates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaceStyle {
    /// How the place is filled.
    pub fill: Fill,
}

/// How a [`PlaceStyle`] fills its place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fill {
    /// With the colour of the one token it holds, while it holds exactly
    /// one that has a colour.
    Token,
}

/// A transition in [`Topology`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transition {
    /// Unique within the net.
    pub name: String,
    /// Rust's introspection: parameter type names and `detachable`.
    pub detail: TransitionDetail,
    /// The name of the group the transition is in, one of
    /// [`Topology::groups`] (§5.1); written only for a transition in a
    /// group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

/// The Rust runtime's `detail` object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitionDetail {
    /// The type names of the transition's `Param<T>` inputs.
    pub params: Vec<String>,
    /// False for a transition with a mutate edge or a read edge, whose body
    /// runs inline.
    pub detachable: bool,
}

/// An edge in [`Topology`]. An edge on a port place of one of the net's
/// subnet places names the subnet place as `place` and the port as `port`
/// (§12.7); a reader keys an edge by its place, its port if any, its
/// transition and its direction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    /// The place end, or the subnet place of a port.
    pub place: String,
    /// The port of the subnet place `place`, for an edge on a port place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    /// The transition end.
    pub transition: String,
    /// `"in"` (place to transition) or `"out"`.
    pub direction: Direction,
    /// How an input edge reads its place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<EdgeKind>,
    /// The filter a keyed edge (`take_where`, `mutate_where`) chooses its
    /// token with, as declared (§13.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<Filter>,
    /// The ports of its tokens' nets an input edge on a place of nets says
    /// its body reaches, as declared (§5.1, since 2026-11-23); written only
    /// when there are some.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reaches: Vec<String>,
    /// The output slot, for a transition with more than one output that
    /// returns them by position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<usize>,
}

/// Which way an [`Edge`] points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Place to transition.
    In,
    /// Transition to place.
    Out,
}

/// How an input [`Edge`] reads its place.
///
/// Read tolerantly (since 2026-11-23): a kind this version does not know is
/// kept as [`Other`](EdgeKind::Other), its name as written, and the rest of
/// the topology is read; a reader draws such an edge as a plain edge
/// (PROTOCOL.md §5.1). Until then an unknown kind failed the whole topology,
/// which is what a reader built on 0.2.x still does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// Consumes the front token.
    Take,
    /// Consumes every token.
    TakeAll,
    /// Reads a clone of the front token.
    Snapshot,
    /// Borrows the front token and may change it.
    Mutate,
    /// Consumes the oldest token its filter accepts (§13.5).
    TakeWhere,
    /// Borrows the oldest token its filter accepts and may change it.
    MutateWhere,
    /// Consumes every token, zero or more: an empty place does not hold the
    /// transition back (§5.1).
    Drain,
    /// Reads a clone of every token, zero or more: an empty place does not
    /// hold the transition back (§5.1).
    Survey,
    /// Lends the front token to look at, taking and copying nothing (§5.1,
    /// since 2026-11-23).
    Read,
    /// Lends the oldest token its filter accepts to look at (§13.5).
    ReadWhere,
    /// Lends every token to look at, zero or more: an empty place does not
    /// hold the transition back.
    ReadAll,
    /// A kind this version does not know, by the name it was written with,
    /// written back unchanged.
    #[serde(untagged)]
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    use crate::protocol::to_value;

    #[test]
    fn a_place_style_and_the_controls_have_the_wire_shape() {
        let place = Place {
            name: "Light".into(),
            type_name: "Colour".into(),
            serializable: Some(true),
            nested: false,
            subnet: false,
            ports: None,
            port: false,
            stepping: None,
            key_names: Vec::new(),
            style: Some(PlaceStyle { fill: Fill::Token }),
            group: None,
        };
        assert_eq!(
            to_value(&place),
            json!({"name": "Light", "type_name": "Colour", "serializable": true,
                   "style": {"fill": "token"}})
        );
        let change = Control {
            name: "change".into(),
            label: "Change".into(),
            kind: "button".into(),
            node: None,
            action: None,
            config: None,
        };
        assert_eq!(
            to_value(&change),
            json!({"name": "change", "label": "Change", "kind": "button"}),
            "node, action and config only when there are some"
        );
        let car = Control {
            node: Some("Waiting".into()),
            action: Some("inject".into()),
            kind: "text".into(),
            config: Some(ControlConfig {
                placeholder: Some("name".into()),
                ..ControlConfig::default()
            }),
            ..change.clone()
        };
        assert_eq!(
            to_value(&car),
            json!({"name": "change", "label": "Change", "kind": "text", "node": "Waiting",
                   "action": "inject", "config": {"placeholder": "name"}})
        );
        let topology = Topology {
            places: vec![place],
            transitions: Vec::new(),
            edges: Vec::new(),
            groups: Vec::new(),
            controls: Vec::new(),
            open: false,
        };
        assert!(
            to_value(&topology).get("controls").is_none(),
            "a net without controls writes none"
        );
        let back: Topology = serde_json::from_value(to_value(Topology {
            controls: vec![car.clone()],
            ..topology
        }))
        .unwrap();
        assert_eq!(back.controls, [car]);
        assert!(ControlConfig::default().is_empty());
    }

    #[test]
    fn a_group_is_written_only_when_set() {
        let sorted = Place {
            name: "Sorted".into(),
            type_name: "String".into(),
            serializable: Some(true),
            nested: false,
            subnet: false,
            ports: None,
            port: false,
            stepping: None,
            key_names: Vec::new(),
            style: None,
            group: Some("Sorting".into()),
        };
        let sort = Transition {
            name: "Sort".into(),
            detail: TransitionDetail {
                params: Vec::new(),
                detachable: true,
            },
            group: Some("Sorting".into()),
        };
        let topology = Topology {
            places: vec![sorted.clone()],
            transitions: vec![sort.clone()],
            edges: Vec::new(),
            groups: vec!["Sorting".into(), "Packing".into()],
            controls: Vec::new(),
            open: false,
        };
        let value = to_value(&topology);
        assert_eq!(
            value,
            json!({"places": [{"name": "Sorted", "type_name": "String", "serializable": true,
                               "group": "Sorting"}],
                   "transitions": [{"name": "Sort", "detail": {"params": [], "detachable": true},
                                    "group": "Sorting"}],
                   "edges": [], "groups": ["Sorting", "Packing"]})
        );
        let back: Topology = serde_json::from_value(value).unwrap();
        assert_eq!(back, topology);

        let plain = Topology {
            places: vec![Place {
                group: None,
                ..sorted
            }],
            transitions: vec![Transition {
                group: None,
                ..sort
            }],
            groups: Vec::new(),
            ..topology
        };
        let value = to_value(&plain);
        assert!(value.get("groups").is_none(), "a net with no groups");
        assert!(value["places"][0].get("group").is_none());
        assert!(value["transitions"][0].get("group").is_none());
        let back: Topology = serde_json::from_value(value).unwrap();
        assert_eq!(back, plain, "a reader takes a topology without them");
    }

    /// An edge's `reaches` is read and written back, and a field this
    /// version does not know is passed over while the rest is read.
    #[test]
    fn an_edge_reads_its_reaches_and_passes_over_a_field_it_does_not_know() {
        let value = json!({"place": "Field", "transition": "Hand Over", "direction": "in",
                           "kind": "mutate", "reaches": ["Orders In"], "aim": {"at": 3}});
        let edge: Edge = serde_json::from_value(value).unwrap();
        assert_eq!(edge.reaches, ["Orders In"]);
        assert_eq!(edge.kind, Some(EdgeKind::Mutate));
        assert_eq!(
            to_value(&edge),
            json!({"place": "Field", "transition": "Hand Over", "direction": "in",
                   "kind": "mutate", "reaches": ["Orders In"]})
        );
        let plain: Edge = serde_json::from_value(
            json!({"place": "Field", "transition": "Hand Over", "direction": "in"}),
        )
        .unwrap();
        assert!(plain.reaches.is_empty());
        assert!(
            to_value(&plain).get("reaches").is_none(),
            "written only when there are some"
        );
    }
}
