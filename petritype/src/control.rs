//! Controls: the buttons a net declares, as data a viewer renders and code
//! the net runs (DESIGN §18, 2026-10-16, net-declared controls).
//!
//! A [`Control`] is a widget of one [`ControlKind`] (a button, a toggle, a
//! slider, a select or a text box) with a name, a label, and a handler written
//! beside the net that does whatever pressing it means, through the net's
//! public API: [`put`](Net::put), [`fire`](Net::fire),
//! [`enable`](Net::enable). [`NetBuilder::control`](crate::NetBuilder::control)
//! declares one, [`Net::controls`] lists them in declaration order, and
//! [`Net::press`] runs one with the [`Value`] the widget gave, which the
//! net checks against the kind first. Every effect of a press is an
//! ordinary [`Event`](crate::Event), so a viewer or a runner that follows
//! the log needs nothing new to show it.
//!
//! The rest of a control is for the viewer only: the [`Node`] it acts on,
//! which a legend lights on hover, and the [`Action`] that says what kind of
//! thing a press does, which decides when the viewer draws the control lit.
//! The core stores and runs controls and interprets neither: a viewer never
//! executes a control, it asks the net to.
//!
//! ```
//! use petritype::control::{Control, Pressed, Value};
//! use petritype::{NetBuilder, PressError, TakeEdge};
//!
//! let mut b = NetBuilder::new();
//! let inbox = b.place::<u32>("Inbox");
//! let out = b.place::<u32>("Out");
//! let double = b.transition("Double", (TakeEdge(inbox),), (out,), |n: u32| (n * 2,)).id();
//! b.control(Control::inject("add", "+ 21", inbox, |_, _| 21));
//! b.control(Control::fire("double", "Double", double));
//! let mut net = b.build()?;
//!
//! let names: Vec<&str> = net.controls().iter().map(|c| c.name.as_str()).collect();
//! assert_eq!(names, ["add", "double"]);
//!
//! assert_eq!(net.press("double", Value::None)?, Pressed::Nothing("Double is not fireable".into()));
//! assert_eq!(net.press("add", Value::None)?, Pressed::Applied);
//! assert_eq!(net.press("double", Value::None)?, Pressed::Applied);
//! let (token, _) = net.tokens(out.id()).next().unwrap();
//! assert_eq!(net.token::<u32>(out.id(), token), Some(&42));
//!
//! assert!(matches!(net.press("nope", Value::None), Err(PressError::UnknownControl { .. })));
//! assert!(matches!(net.press("add", Value::Bool(true)), Err(PressError::WrongKind { .. })));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::fmt;
use std::sync::Arc;

use crate::error::{NetError, PressError};
use crate::ids::{PlaceId, PlaceRef, TransitionId, foreign_place};
use crate::net::Net;

/// The widget a control is, and so the [`Value`] a press of it carries.
///
/// The protocol's `kind` (PROTOCOL.md §5.1), with each kind's settings,
/// which the protocol carries in `config`.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ControlKind {
    /// A button: a press carries [`Value::None`].
    Button,
    /// An on/off switch: a press carries [`Value::Bool`], the new state.
    Toggle,
    /// A number between `start` and `stop`, both included, in steps of
    /// `step`: a press carries [`Value::Number`]. The step is the widget's;
    /// the net checks only the bounds.
    Slider {
        /// The smallest value.
        start: f64,
        /// The largest value.
        stop: f64,
        /// The widget's increment.
        step: f64,
    },
    /// One of `options`: a press carries [`Value::Text`], one of them.
    Select {
        /// The choices, in the order shown.
        options: Vec<String>,
    },
    /// Free text: a press carries [`Value::Text`], possibly empty.
    Text {
        /// The hint shown while the box is empty.
        placeholder: Option<String>,
    },
}

impl ControlKind {
    /// The kind's name on the wire: `"button"`, `"toggle"`, `"slider"`,
    /// `"select"` or `"text"`.
    pub fn name(&self) -> &'static str {
        match self {
            ControlKind::Button => "button",
            ControlKind::Toggle => "toggle",
            ControlKind::Slider { .. } => "slider",
            ControlKind::Select { .. } => "select",
            ControlKind::Text { .. } => "text",
        }
    }

    /// Whether a press of this kind may carry `value`, and if not, why not.
    pub(crate) fn check(&self, value: &Value) -> Result<(), String> {
        match (self, value) {
            (ControlKind::Button, Value::None) => Ok(()),
            (ControlKind::Button, _) => Err("a button takes no value".to_owned()),
            (ControlKind::Toggle, Value::Bool(_)) => Ok(()),
            (ControlKind::Toggle, _) => Err("a toggle takes a boolean".to_owned()),
            (ControlKind::Slider { start, stop, .. }, Value::Number(n)) => {
                if (*start..=*stop).contains(n) {
                    Ok(())
                } else {
                    Err(format!("{n} is outside {start}..{stop}"))
                }
            }
            (ControlKind::Slider { .. }, _) => Err("a slider takes a number".to_owned()),
            (ControlKind::Select { options }, Value::Text(choice)) => {
                if options.contains(choice) {
                    Ok(())
                } else {
                    Err(format!("{choice:?} is not one of its options"))
                }
            }
            (ControlKind::Select { .. }, _) => Err("a select takes one of its options".to_owned()),
            (ControlKind::Text { .. }, Value::Text(_)) => Ok(()),
            (ControlKind::Text { .. }, _) => Err("a text control takes a string".to_owned()),
        }
    }
}

/// What kind of thing a press does, for the viewer's lighting rule only
/// (PROTOCOL.md §5.1): the net never reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Action {
    /// Puts a token into the control's place; always lit.
    Inject,
    /// Fires the control's transition; lit while it is fireable.
    Fire,
    /// Switches the control's transition on or off; a toggle whose state is
    /// the transition's enabled flag.
    Enable,
}

impl Action {
    /// The action's name on the wire: `"inject"`, `"fire"` or `"enable"`.
    pub fn name(self) -> &'static str {
        match self {
            Action::Inject => "inject",
            Action::Fire => "fire",
            Action::Enable => "enable",
        }
    }
}

/// What a press carries, by the control's [`ControlKind`]: nothing for a
/// button, a boolean for a toggle, a number for a slider, a string for a
/// select (one of its options) or a text box.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Value {
    /// A button's press.
    None,
    /// A toggle's new state.
    Bool(bool),
    /// A slider's value.
    Number(f64),
    /// A select's option, or a text box's text.
    Text(String),
}

/// What a press did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pressed {
    /// The control applied something; its events are in the net's log.
    Applied,
    /// The control chose to do nothing, and says why: a fire control whose
    /// transition is not fireable, `"Change is not fireable"`.
    Nothing(String),
}

/// The place or transition a control acts on, for the viewer: the legend
/// lights it while the control's row is hovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Node {
    /// A place of the net.
    Place(PlaceId),
    /// A transition of the net.
    Transition(TransitionId),
}

/// What a press runs: the net, and the value the core has already checked
/// against the control's kind.
type Handler = Arc<dyn Fn(&mut Net, Value) -> Result<Pressed, String> + Send + Sync>;

/// A control a net declares: a widget, the node it acts on, and the code a
/// press runs. See the [module](self).
///
/// The fields are data, for a viewer or a runner to read from
/// [`Net::controls`]; the handler is the net's, run by [`Net::press`]. The
/// constructors cover the kinds ([`button`](Control::button),
/// [`toggle`](Control::toggle), [`slider`](Control::slider),
/// [`select`](Control::select), [`text`](Control::text)) with a handler
/// of the author's, and the three actions ([`inject`](Control::inject),
/// [`fire`](Control::fire), [`enable`](Control::enable)) with one written
/// for them. A handler returns `Ok(`[`Pressed`]`)`, or `Err` with a message
/// when it cannot do what it was asked, which [`Net::press`] gives back as
/// [`PressError::Refused`].
#[derive(Clone)]
pub struct Control {
    /// Unique within the net: what [`Net::press`] names.
    pub name: String,
    /// What the viewer shows.
    pub label: String,
    /// The widget, and so the value a press carries.
    pub kind: ControlKind,
    /// The place or transition the control acts on, if it acts on one.
    pub node: Option<Node>,
    /// What kind of thing a press does, if the control says.
    pub action: Option<Action>,
    /// The widget's value before anyone touches it, for the kinds that
    /// have one: a toggle's state, a slider's number, a select's option.
    pub initial: Option<Value>,
    handler: Handler,
}

impl Control {
    /// A control of `kind` with the author's handler and nothing else.
    fn new(
        name: impl Into<String>,
        label: impl Into<String>,
        kind: ControlKind,
        initial: Option<Value>,
        f: impl Fn(&mut Net, Value) -> Result<Pressed, String> + Send + Sync + 'static,
    ) -> Control {
        Control {
            name: name.into(),
            label: label.into(),
            kind,
            node: None,
            action: None,
            initial,
            handler: Arc::new(f),
        }
    }

    /// A button: `f` runs with [`Value::None`].
    pub fn button(
        name: impl Into<String>,
        label: impl Into<String>,
        f: impl Fn(&mut Net, Value) -> Result<Pressed, String> + Send + Sync + 'static,
    ) -> Control {
        Control::new(name, label, ControlKind::Button, None, f)
    }

    /// A text box, with `placeholder` as its hint while empty: `f` runs with
    /// [`Value::Text`].
    pub fn text(
        name: impl Into<String>,
        label: impl Into<String>,
        placeholder: Option<&str>,
        f: impl Fn(&mut Net, Value) -> Result<Pressed, String> + Send + Sync + 'static,
    ) -> Control {
        let kind = ControlKind::Text {
            placeholder: placeholder.map(str::to_owned),
        };
        Control::new(name, label, kind, None, f)
    }

    /// A toggle, `initial` on or off: `f` runs with [`Value::Bool`], the new
    /// state.
    pub fn toggle(
        name: impl Into<String>,
        label: impl Into<String>,
        initial: bool,
        f: impl Fn(&mut Net, Value) -> Result<Pressed, String> + Send + Sync + 'static,
    ) -> Control {
        Control::new(
            name,
            label,
            ControlKind::Toggle,
            Some(Value::Bool(initial)),
            f,
        )
    }

    /// A slider from `start` to `stop` in steps of `step`, at `initial`: `f`
    /// runs with [`Value::Number`], within the bounds.
    pub fn slider(
        name: impl Into<String>,
        label: impl Into<String>,
        start: f64,
        stop: f64,
        step: f64,
        initial: f64,
        f: impl Fn(&mut Net, Value) -> Result<Pressed, String> + Send + Sync + 'static,
    ) -> Control {
        let kind = ControlKind::Slider { start, stop, step };
        Control::new(name, label, kind, Some(Value::Number(initial)), f)
    }

    /// A select of `options`, at `initial`: `f` runs with [`Value::Text`],
    /// one of the options.
    pub fn select<S: Into<String>>(
        name: impl Into<String>,
        label: impl Into<String>,
        options: impl IntoIterator<Item = S>,
        initial: impl Into<String>,
        f: impl Fn(&mut Net, Value) -> Result<Pressed, String> + Send + Sync + 'static,
    ) -> Control {
        let kind = ControlKind::Select {
            options: options.into_iter().map(Into::into).collect(),
        };
        Control::new(name, label, kind, Some(Value::Text(initial.into())), f)
    }

    /// Puts `make(value, net)` into `place`: a button by default, another
    /// kind with [`with_kind`](Control::with_kind). Its node is the place and
    /// its action [`Action::Inject`].
    ///
    /// `make` is handed the net as it stands before the token goes in, so
    /// that it can read the marking: to number what it makes from the tokens
    /// there are, say, with no counter of its own (Python's `make(value,
    /// graph)`). One that needs only the value ignores it, `|value, _| ...`.
    ///
    /// ```
    /// use petritype::NetBuilder;
    /// use petritype::control::{Control, ControlKind, Value};
    ///
    /// let mut b = NetBuilder::new();
    /// let queue = b.place::<String>("Queue");
    /// b.control(
    ///     Control::inject("join", "+ Join", queue, move |value, net| match value {
    ///         Value::Text(name) if !name.is_empty() => name,
    ///         _ => format!("Guest {}", net.count(queue.id()) + 1),
    ///     })
    ///     .with_kind(ControlKind::Text { placeholder: Some("name".into()) }),
    /// );
    /// let mut net = b.build()?;
    ///
    /// for typed in ["", "Ada", ""] {
    ///     net.press("join", Value::Text(typed.into()))?;
    /// }
    /// let names: Vec<&String> = net
    ///     .tokens(queue.id())
    ///     .filter_map(|(token, _)| net.token::<String>(queue.id(), token))
    ///     .collect();
    /// assert_eq!(names, ["Guest 1", "Ada", "Guest 3"]);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// A press of it on a net that `place` does not belong to is refused.
    pub fn inject<T: Send + Sync + 'static>(
        name: impl Into<String>,
        label: impl Into<String>,
        place: PlaceRef<T>,
        make: impl Fn(Value, &Net) -> T + Send + Sync + 'static,
    ) -> Control {
        let mut control = Control::button(name, label, move |net, value| {
            if place.net() != net.id() {
                return Err(foreign_place("inject", place.id(), place.net(), net.id()));
            }
            let token = make(value, &*net);
            net.put(place, token);
            Ok(Pressed::Applied)
        });
        control.node = Some(Node::Place(place.id()));
        control.action = Some(Action::Inject);
        control
    }

    /// Fires `transition`: a button, its node the transition and its action
    /// [`Action::Fire`]. A transition that is not fireable is
    /// [`Pressed::Nothing`], `"<transition> is not fireable"`, and the net is
    /// untouched; a body that fails is refused with the failure's message,
    /// its events in the log as for any failed firing.
    pub fn fire(
        name: impl Into<String>,
        label: impl Into<String>,
        transition: TransitionId,
    ) -> Control {
        let mut control = Control::button(name, label, move |net, _| match net.fire(transition) {
            Ok(()) => Ok(Pressed::Applied),
            Err(NetError::NotFireable { name, .. }) => {
                Ok(Pressed::Nothing(format!("{name} is not fireable")))
            }
            Err(error) => Err(error.to_string()),
        });
        control.node = Some(Node::Transition(transition));
        control.action = Some(Action::Fire);
        control
    }

    /// Switches `transition` on or off: a toggle, initially on, its node the
    /// transition and its action [`Action::Enable`].
    pub fn enable(
        name: impl Into<String>,
        label: impl Into<String>,
        transition: TransitionId,
    ) -> Control {
        let mut control = Control::toggle(name, label, true, move |net, value| {
            let switched = match value {
                Value::Bool(true) => net.enable(transition),
                Value::Bool(false) => net.disable(transition),
                _ => return Err("an enable control takes a boolean".to_owned()),
            };
            switched.map_err(|error| error.to_string())?;
            Ok(Pressed::Applied)
        });
        control.node = Some(Node::Transition(transition));
        control.action = Some(Action::Enable);
        control
    }

    /// This control with another kind: how an [`inject`](Control::inject)
    /// becomes a text box or a select. The initial value is left as it was.
    pub fn with_kind(mut self, kind: ControlKind) -> Control {
        self.kind = kind;
        self
    }

    /// This control with an initial value.
    pub fn with_initial(mut self, initial: Value) -> Control {
        self.initial = Some(initial);
        self
    }
}

impl fmt::Debug for Control {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Control")
            .field("name", &self.name)
            .field("label", &self.label)
            .field("kind", &self.kind)
            .field("node", &self.node)
            .field("action", &self.action)
            .field("initial", &self.initial)
            .finish_non_exhaustive()
    }
}

impl Net {
    /// The controls the net declares, in declaration order: what a viewer
    /// builds its legend from and a runner serves as `topology.controls`.
    pub fn controls(&self) -> &[Control] {
        &self.controls
    }

    /// Presses the control called `name` with `value`: checks the value
    /// against the control's [`ControlKind`] and runs its handler.
    ///
    /// Whatever the handler does goes through the net's own API, so its
    /// effects are ordinary [`Event`](crate::Event)s in the log, and
    /// [`Pressed`] says whether it applied anything.
    ///
    /// # Errors
    ///
    /// [`PressError::UnknownControl`] for a name the net does not declare,
    /// [`PressError::WrongKind`] for a value its kind cannot take (an
    /// option it does not list, or a number outside a slider's bounds,
    /// included), the net untouched in both cases, and
    /// [`PressError::Refused`] with the handler's message when it returns
    /// `Err`.
    ///
    /// # Panics
    ///
    /// If the handler panics: the panic is the handler's, not caught here.
    pub fn press(&mut self, name: &str, value: Value) -> Result<Pressed, PressError> {
        let control = self
            .controls
            .iter()
            .find(|control| control.name == name)
            .ok_or_else(|| PressError::UnknownControl {
                name: name.to_owned(),
            })?;
        control
            .kind
            .check(&value)
            .map_err(|reason| PressError::WrongKind {
                control: name.to_owned(),
                reason,
            })?;
        // The handler is the net's own, and is handed the net mutably: the
        // `Arc` is cloned first so that nothing of `self` is borrowed while
        // it runs.
        let handler = Arc::clone(&control.handler);
        handler(self, value).map_err(PressError::Refused)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_checks_its_value() {
        let slider = ControlKind::Slider {
            start: 0.0,
            stop: 10.0,
            step: 1.0,
        };
        let select = ControlKind::Select {
            options: vec!["a".into(), "b".into()],
        };
        let text = ControlKind::Text { placeholder: None };
        let cases = [
            (&ControlKind::Button, Value::None, true),
            (&ControlKind::Button, Value::Text(String::new()), false),
            (&ControlKind::Toggle, Value::Bool(false), true),
            (&ControlKind::Toggle, Value::None, false),
            (&slider, Value::Number(0.0), true),
            (&slider, Value::Number(10.0), true),
            (&slider, Value::Number(10.5), false),
            (&slider, Value::Number(f64::NAN), false),
            (&slider, Value::Text("5".into()), false),
            (&select, Value::Text("b".into()), true),
            (&select, Value::Text("c".into()), false),
            (&select, Value::None, false),
            (&text, Value::Text(String::new()), true),
            (&text, Value::Number(7.0), false),
            (&text, Value::None, false),
        ];
        for (kind, value, ok) in cases {
            assert_eq!(kind.check(&value).is_ok(), ok, "{kind:?} with {value:?}");
        }
    }

    #[test]
    fn the_wire_names_are_the_protocol_s() {
        assert_eq!(ControlKind::Button.name(), "button");
        assert_eq!(ControlKind::Text { placeholder: None }.name(), "text");
        assert_eq!(Action::Inject.name(), "inject");
        assert_eq!(Action::Enable.name(), "enable");
    }
}
