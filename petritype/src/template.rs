//! Templates (nets built by name) and the JSON form of a net they make
//! possible: [`Templates`], `Serialize` and `Deserialize` for [`Net`], and
//! [`Net::fork`] (DESIGN §6.6, `serde` feature).
//!
//! A `Net` cannot be written out whole: its bodies, guards and priorities
//! are closures. What can be written is which function built it and what
//! has happened to it since (the marking, the parameters, the switches),
//! and that is enough to get it back, by calling the function again and
//! putting the rest in. A *template* is such a function registered under a
//! name, and a net built through one is *stamped* with it, so the net knows
//! how to be rebuilt without a registry in hand.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::sync::Arc;

use serde::de::Error as _;
use serde::ser::Error as _;

use crate::command::JsonCommand;
use crate::error::{NetError, PathStep, TemplateError};
use crate::ids::PlaceId;
use crate::marking::Marking;
use crate::net::Net;
use crate::param::param_slot_index;

/// What a template's factory is given: parameters by name, as JSON.
///
/// The same type, under the same name, as `petritype_runner::Params`, the
/// load parameters of a net process, so a runner's factory and a template
/// line up.
pub type Params = serde_json::Map<String, serde_json::Value>;

/// A template's factory, stored: the shape of a runner's net factory, with
/// the error boxed.
pub(crate) type Factory = Arc<dyn Fn(&Params) -> Result<Net, BoxedError> + Send + Sync>;

/// An error of any kind that can cross a thread.
type BoxedError = Box<dyn Error + Send + Sync>;

/// A registry of templates: functions that build a [`Net`], each under a
/// name.
///
/// A template is how a net that lives inside a token is saved and restored,
/// and how it is copied: a [`Net`] cannot be cloned or written out whole,
/// because its bodies are closures, but the function that built it can be
/// called again. A net built with [`build`](Templates::build) is *stamped*:
/// it remembers the template's name, the [`Params`] it was given and the
/// factory itself, so that [`Net::fork`] and `Serialize` need no registry.
/// Restoring does, since a saved net is only text, and finds one through
/// [`scope`](Templates::scope).
///
/// ```
/// use petritype::{NetBuilder, Params, Templates};
///
/// let mut templates = Templates::new();
/// templates.register("Counter", |params: &Params| {
///     let start = params.get("start").and_then(|start| start.as_u64()).unwrap_or(0);
///     let mut b = NetBuilder::new();
///     let count = b.place::<u64>("Count");
///     b.serializable(count);
///     b.seed(count, [start]);
///     b.build()
/// });
///
/// let mut params = Params::new();
/// params.insert("start".into(), 7.into());
/// let net = templates.build("Counter", &params)?;
/// assert_eq!(net.template(), Some("Counter"));
///
/// // Saved as text and back, through the same registry.
/// let saved = net.to_json()?;
/// assert_eq!(saved["template"], "Counter");
/// let restored = templates.restore(saved)?;
/// assert_eq!(restored.export_marking()?, net.export_marking()?);
/// assert_ne!(restored.id(), net.id(), "a restored net is a new net");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// Cloning is cheap (the factories are shared), and a registry is `Send +
/// Sync`, so one can be handed to every net that needs it:
/// [`NetBuilder::templates`](crate::NetBuilder::templates) gives a net the
/// registry its nested tokens are restored from.
#[derive(Clone, Default)]
pub struct Templates {
    /// By name, so that [`names`](Templates::names) lists them in a stable
    /// order. Shared: a registry is cloned every time a scope opens.
    factories: Arc<BTreeMap<String, Factory>>,
}

impl Templates {
    /// A registry with no templates in it.
    pub fn new() -> Self {
        Templates::default()
    }

    /// Registers `factory` under `name`, replacing any template of that
    /// name. Chainable.
    ///
    /// A name has at least one character: [`build`](Templates::build)
    /// refuses the empty string, and so does
    /// [`NetBuilder::build`](crate::NetBuilder::build) for a net given a
    /// registry that holds it ([`BuildError::EmptyName`](crate::BuildError::EmptyName)).
    ///
    /// The factory is the shape of a runner's net factory: it takes
    /// [`Params`] and returns a built net or an error, which may be any
    /// error that converts into a boxed one: a
    /// [`BuildError`](crate::BuildError) from
    /// [`NetBuilder::build`](crate::NetBuilder::build), a `String`, a type
    /// of the author's own.
    ///
    /// When a saved net is restored, its factory is given the net's saved
    /// parameters (those registered with
    /// [`serializable_param`](crate::NetBuilder::serializable_param), by
    /// wire name), and each is then set again over what the factory built,
    /// so a value changed at run time wins. A factory therefore reads what
    /// it needs from its net's own wire parameters, with a default for each.
    pub fn register<F, E>(&mut self, name: impl Into<String>, factory: F) -> &mut Self
    where
        F: Fn(&Params) -> Result<Net, E> + Send + Sync + 'static,
        E: Into<BoxedError>,
    {
        let factory: Factory = Arc::new(move |params: &Params| factory(params).map_err(Into::into));
        Arc::make_mut(&mut self.factories).insert(name.into(), factory);
        self
    }

    /// The names of the registered templates, in name order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.factories.keys().map(String::as_str)
    }

    /// Builds a net from the template named `name`, handing its factory
    /// `params`, and stamps it with the template (see
    /// [`Net::template`]).
    ///
    /// # Errors
    ///
    /// [`TemplateError::EmptyName`] for the empty string, which names no
    /// template a net can be saved by, even one registered under it (since
    /// 2026-11-28); [`TemplateError::NoSuchTemplate`] for a name this
    /// registry does not have; and [`TemplateError::Factory`] carrying
    /// whatever the factory returned.
    pub fn build(&self, name: &str, params: &Params) -> Result<Net, TemplateError> {
        if name.is_empty() {
            return Err(TemplateError::EmptyName);
        }
        let factory = self
            .factories
            .get(name)
            .ok_or_else(|| TemplateError::NoSuchTemplate {
                name: name.to_owned(),
            })?;
        build_stamped(name, factory, params).map_err(|source| TemplateError::Factory {
            name: name.to_owned(),
            source,
        })
    }

    /// Runs `f` with this registry in scope: a [`Net`] deserialised inside
    /// `f`, at any depth, is rebuilt from it.
    ///
    /// `Deserialize` for a type with a `Net` field cannot be handed a
    /// registry, so `Net`'s finds one here, in a stack kept per thread.
    /// Scopes nest, and a template is looked for innermost first, then
    /// outwards: a net restoring its own nested tokens opens a scope of its
    /// own registry inside whatever scope restored it. A net opens its scope
    /// itself wherever it decodes tokens
    /// ([`import_marking`](Net::import_marking),
    /// [`Command::from_json`](crate::Command::from_json),
    /// [`apply_json`](Net::apply_json)), so a caller that goes through the
    /// net never needs this; [`restore`](Templates::restore) is the one-call
    /// form for a net saved on its own.
    ///
    /// Deserialising a `Net` with no scope open is a serde error saying so.
    /// The scope closes when `f` returns or panics.
    pub fn scope<R>(&self, f: impl FnOnce() -> R) -> R {
        /// Closes the scope on the way out, by return or by unwinding.
        struct Close;
        impl Drop for Close {
            fn drop(&mut self) {
                SCOPE.with(|scope| {
                    scope.borrow_mut().pop();
                });
            }
        }
        SCOPE.with(|scope| scope.borrow_mut().push(self.clone()));
        let _close = Close;
        f()
    }

    /// A net from its saved JSON (`Net`'s `Serialize` form,
    /// `{"template", "marking", "params", "enabled"}`), rebuilt from this
    /// registry: [`scope`](Templates::scope) around `serde_json::from_value`.
    ///
    /// The net is rebuilt by its template's factory, given the saved
    /// parameters; then its marking is imported, every saved parameter set
    /// and every saved switch thrown. A subnet the factory builds is filled
    /// from its entry in the marking, checked against the template that
    /// built it ([`Net::import_marking`]). Its log is empty and its counters
    /// are at zero, as a freshly built net's are, and so are every
    /// subnet's: restoring is setup. Its tokens have fresh ids, and it is a
    /// new net, with a new [`NetId`](crate::NetId).
    ///
    /// # Errors
    ///
    /// A serde error for a value of the wrong shape, an unknown template, a
    /// factory that failed, a place or a transition or a parameter the
    /// rebuilt net does not have, or a token or a parameter value that does
    /// not decode, each saying which.
    pub fn restore(&self, value: serde_json::Value) -> Result<Net, serde_json::Error> {
        self.scope(|| serde_json::from_value(value))
    }

    /// Adds every template of `other` to this registry, `other`'s winning a
    /// name both have.
    pub(crate) fn merge(&mut self, other: &Templates) {
        if self.factories.is_empty() {
            self.factories = Arc::clone(&other.factories);
            return;
        }
        let factories = Arc::make_mut(&mut self.factories);
        for (name, factory) in other.factories.iter() {
            factories.insert(name.clone(), Arc::clone(factory));
        }
    }
}

/// The names, in order: a factory is a closure and has nothing to print.
impl fmt::Debug for Templates {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Templates")
            .field("names", &self.names().collect::<Vec<_>>())
            .finish()
    }
}

/// What a net built from a template remembers of it: enough to build the
/// net again with no registry in hand.
#[derive(Clone)]
pub(crate) struct Stamp {
    /// The template's name, which a save writes as `"template"`.
    pub(crate) name: String,
    /// What the factory was given.
    pub(crate) params: Params,
    /// The factory itself, shared with the registry it came from.
    pub(crate) factory: Factory,
}

impl fmt::Debug for Stamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stamp")
            .field("name", &self.name)
            .field("params", &self.params)
            .finish_non_exhaustive()
    }
}

/// Calls a factory and stamps what it built.
fn build_stamped(name: &str, factory: &Factory, params: &Params) -> Result<Net, BoxedError> {
    // A saved net names its template, and the empty string names none: a
    // net saved under it is not rebuilt, as `Templates::build` makes none.
    if name.is_empty() {
        return Err(Box::new(TemplateError::EmptyName));
    }
    let mut net = factory(params)?;
    net.stamp = Some(Stamp {
        name: name.to_owned(),
        params: params.clone(),
        factory: Arc::clone(factory),
    });
    Ok(net)
}

thread_local! {
    /// The registries in scope on this thread, outermost first: see
    /// [`Templates::scope`].
    static SCOPE: RefCell<Vec<Templates>> = const { RefCell::new(Vec::new()) };
}

/// The factory of the template named `name`, innermost scope first, or the
/// message of a serde error saying why there is none.
///
/// The borrow of the stack ends before the factory is returned: calling it
/// may open scopes of its own.
fn in_scope(name: &str) -> Result<Factory, String> {
    SCOPE.with(|scope| {
        let scope = scope.borrow();
        if scope.is_empty() {
            return Err(String::from(
                "no template registry in scope: deserialise inside Templates::scope",
            ));
        }
        scope
            .iter()
            .rev()
            .find_map(|templates| templates.factories.get(name).cloned())
            .ok_or_else(|| {
                let mut names: Vec<&str> = scope.iter().flat_map(Templates::names).collect();
                names.sort_unstable();
                names.dedup();
                format!("no template named {name:?} in scope (registered: {names:?})")
            })
    })
}

/// A net as it is written: the template that builds it and what has
/// happened to it since (DESIGN §6.6).
#[derive(serde::Serialize)]
struct Saved<'a> {
    template: &'a str,
    marking: Marking,
    params: Params,
    enabled: BTreeMap<&'a str, bool>,
}

/// A net as it is read back, before anything is built: a saved net, or a
/// subnet's entry in a saved marking, which goes into the subnet already
/// there ([`Net::import_marking`]).
#[derive(serde::Deserialize)]
pub(crate) struct Loaded {
    pub(crate) template: String,
    pub(crate) marking: Marking,
    #[serde(default)]
    pub(crate) params: Params,
    #[serde(default)]
    pub(crate) enabled: BTreeMap<String, bool>,
}

impl Net {
    /// The name of the template this net was built from, or `None` for a
    /// net built some other way.
    ///
    /// Set by [`Templates::build`] and by restoring a saved net. A stamped
    /// net can be saved (`Serialize`) and [forked](Net::fork); an unstamped
    /// one can do neither, because nothing knows how to build it again. A
    /// [subnet](crate::NetBuilder::subnet_place) is saved and forked with
    /// the net around it, so that net can be saved and forked only if every
    /// subnet in it is stamped too: build a subnet through a template, as
    /// the outer net's own factory does when it is a template too.
    pub fn template(&self) -> Option<&str> {
        self.stamp.as_ref().map(|stamp| stamp.name.as_str())
    }

    /// The registry this net decodes its nested tokens with, as given to
    /// [`NetBuilder::template`](crate::NetBuilder::template) and
    /// [`NetBuilder::templates`](crate::NetBuilder::templates); empty if
    /// none was.
    pub fn templates(&self) -> &Templates {
        &self.templates
    }

    /// This net in its saved form, as a JSON value: `Serialize`, for a
    /// caller that wants the value rather than text.
    ///
    /// # Errors
    ///
    /// Whatever `Serialize` refuses: see the impl.
    pub fn to_json(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::to_value(self)
    }

    /// A copy of this net: built again by the template that built it, then
    /// given this net's marking, parameters and switches.
    ///
    /// A `Net` cannot be cloned (its bodies are closures, and an airborne
    /// body is not even in it), but the factory that built it can be called
    /// again, and the rest is data. The factory is given the [`Params`] it
    /// was given first, with every wire parameter's current value over
    /// them. Then the marking goes across as
    /// [`export_marking`](Net::export_marking) writes it, every parameter
    /// value is cloned across (all of them, not only the ones with wire
    /// names: a fork never leaves the process), and every switch, the
    /// [label limit](Net::label_limit), the
    /// [tie-break](Net::set_tie_break) and every nested place's
    /// [stepping rule](Net::set_stepping) are copied. A custom
    /// [selector](Net::set_selector) is a closure and is not, and neither
    /// cursor is (the round robin's and the ring's): a copy starts its
    /// rounds afresh.
    ///
    /// Every place must be serializable, since the marking crosses as JSON;
    /// so the tokens of this net's own nested places are rebuilt from their
    /// saved form, their inner nets by the templates of the copy's
    /// [`templates`](Net::templates) or of any registry in
    /// [scope](Templates::scope), as an import would.
    ///
    /// A [subnet place](crate::NetBuilder::subnet_place)'s subnet is part
    /// of the net, and is copied with it, at every depth: the factory builds
    /// the copy's subnets, the marking fills them, and each is then given
    /// what this one's subnet has beyond its saved form, as this net is
    /// (every parameter value, every switch, the label limit, the tie-break
    /// and the stepping rules). So every subnet must have been built through
    /// a template, as a saved one must.
    ///
    /// The copy is a new net: a new [`NetId`](crate::NetId), fresh token
    /// ids, an empty log and its counters at zero, at every level. Changing
    /// one changes nothing in the other. It is what
    /// [`Nested::fork`](crate::Nested::fork) is for a `Net`, and what a
    /// token type's own `fork` calls for each net it owns.
    ///
    /// ```
    /// use petritype::{NetBuilder, Params, Templates};
    ///
    /// let mut templates = Templates::new();
    /// templates.register("Queue", |_: &Params| {
    ///     let mut b = NetBuilder::new();
    ///     let queue = b.place::<String>("Queue");
    ///     b.serializable(queue);
    ///     b.port(queue);
    ///     b.build()
    /// });
    /// let mut net = templates.build("Queue", &Params::new())?;
    /// let queue = net.place_ref::<String>("Queue").expect("a place of strings");
    /// net.put(queue, "Ada".to_owned());
    ///
    /// let fork = net.fork()?;
    /// assert_eq!(fork.export_marking()?, net.export_marking()?);
    /// assert_eq!(fork.state_counter(), 0);
    /// net.put(queue, "Bram".to_owned());
    /// assert_eq!(fork.count(fork.place_named("Queue").unwrap()), 1, "independent");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// [`NetError::NoTemplate`] for a net no template built;
    /// [`NetError::InFlight`] while a transition is in flight, since the
    /// tokens it took are in no place; whatever
    /// [`export_marking`](Net::export_marking) refuses; and
    /// [`NetError::RebuildFailed`] if the factory fails this time or its net
    /// does not take the marking back.
    pub fn fork(&self) -> Result<Net, NetError> {
        let stamp = self.stamp.as_ref().ok_or(NetError::NoTemplate)?;
        self.check_landed()?;
        let marking = self.export_marking()?;
        let rebuild = |source: BoxedError| NetError::RebuildFailed {
            template: stamp.name.clone(),
            source,
        };

        let mut given = stamp.params.clone();
        given.extend(self.wire_params().map_err(|error| rebuild(error.into()))?);
        let mut fork = build_stamped(&stamp.name, &stamp.factory, &given).map_err(rebuild)?;
        fork.import_marking(&marking)
            .map_err(|error| rebuild(error.into()))?;
        fork.copy_settings(self);
        fork.forget_setup();
        Ok(fork)
    }

    // ----- internals ------------------------------------------------------

    /// What a fork takes from the net it copies beyond the marking, here
    /// and in every subnet: every parameter value, every switch, every
    /// stepping rule, the label limit and the tie-break.
    ///
    /// Both nets came from one factory, so they have the same parameter
    /// types, transitions and places; a fork of a factory that built
    /// something else this time keeps what still fits.
    fn copy_settings(&mut self, original: &Net) {
        for slot in &original.params {
            if let Some(index) = param_slot_index(&self.params, slot.type_id) {
                self.params[index].set(slot.clone_value());
            }
        }
        for slot in &original.transitions {
            if let Some(transition) = self.transition_named(&slot.name) {
                self.transitions[transition.0 as usize].enabled = slot.enabled;
            }
        }
        for slot in original.places.iter().filter(|slot| slot.meta.holds_nets()) {
            if let Some(place) = self.place_named(&slot.meta.name) {
                let meta = &mut self.places[place.0 as usize].meta;
                if meta.holds_nets() {
                    meta.stepping = slot.meta.stepping;
                }
            }
        }
        self.label_limit = original.label_limit;
        self.tie_break = original.tie_break;
        // A subnet is part of its net, and is copied as the net is.
        for slot in &original.places {
            let Some(subnet) = slot.subnet.as_deref() else {
                continue;
            };
            let Some(place) = self.place_named(&slot.meta.name) else {
                continue;
            };
            if let Some(copy) = self.places[place.0 as usize].subnet.as_deref_mut() {
                copy.net.copy_settings(&subnet.net);
            }
        }
    }

    /// [`NetError::InFlight`] for the first transition in flight, if any: a
    /// copy made now would lose the tokens that firing took.
    fn check_landed(&self) -> Result<(), NetError> {
        match self.in_flight().first() {
            Some(&transition) => Err(NetError::InFlight {
                transition,
                name: self.transition_name(transition),
                path: Vec::new(),
            }),
            None => Ok(()),
        }
    }

    /// [`check_landed`](Net::check_landed) here and in every subnet, at any
    /// depth: this net's own transitions first, then each subnet place's
    /// subnet in place order, the error's `path` naming the subnet places
    /// down to the one in flight. What an import asks, since it fills each
    /// subnet where it stands and a firing of one would land into the
    /// marking imported. A net a token carries is replaced by an import,
    /// not filled, so a firing of it cannot land there, and it is not asked.
    pub(crate) fn check_landed_deep(&self) -> Result<(), NetError> {
        self.check_landed()?;
        for (index, slot) in self.places.iter().enumerate() {
            let Some(subnet) = slot.subnet.as_deref() else {
                continue;
            };
            match subnet.net.check_landed_deep() {
                Ok(()) => {}
                Err(NetError::InFlight {
                    transition,
                    name,
                    mut path,
                }) => {
                    path.insert(
                        0,
                        PathStep::Subnet {
                            place: PlaceId(index as u32),
                        },
                    );
                    return Err(NetError::InFlight {
                        transition,
                        name,
                        path,
                    });
                }
                Err(other) => return Err(other),
            }
        }
        Ok(())
    }

    /// The current value of every parameter with a wire name, by that name.
    fn wire_params(&self) -> Result<Params, serde_json::Error> {
        let mut params = Params::new();
        for slot in &self.params {
            if let (Some(name), Some(to_json)) = (&slot.name, slot.to_json) {
                params.insert(name.clone(), to_json(&*slot.value)?);
            }
        }
        Ok(params)
    }

    /// Empties the log and zeroes the counters of a net that was just made
    /// (rebuilt, restored or forked), and of every subnet in it, so that it
    /// starts, as a freshly built net does, with nothing to report.
    fn forget_setup(&mut self) {
        self.events.clear();
        self.state_counter = 0;
        self.fired_count = 0;
        for slot in &mut self.places {
            if let Some(subnet) = slot.subnet.as_deref_mut() {
                subnet.net.forget_setup();
            }
        }
    }

    /// This net in its saved form, as a JSON value, or why it has none: a
    /// subnet's entry in the marking of the net around it
    /// ([`export_marking`](Net::export_marking)).
    pub(crate) fn saved_value(&self) -> Result<serde_json::Value, BoxedError> {
        Ok(serde_json::to_value(self.saved()?)?)
    }

    /// What `Serialize` writes, or why it cannot.
    fn saved(&self) -> Result<Saved<'_>, BoxedError> {
        let stamp = self.stamp.as_ref().ok_or(NetError::NoTemplate)?;
        self.check_landed()?;
        Ok(Saved {
            template: &stamp.name,
            marking: self.export_marking()?,
            params: self.wire_params()?,
            enabled: self
                .transitions
                .iter()
                .map(|slot| (slot.name.as_str(), slot.enabled))
                .collect(),
        })
    }
}

/// A net in its saved form:
/// `{"template": .., "marking": {..}, "params": {..}, "enabled": {..}}`
/// (PROTOCOL.md §12.5 on the wire).
///
/// `template` is the name of the template that built it; `marking` is
/// [`Net::export_marking`]; `params` the current value of every parameter
/// registered with
/// [`serializable_param`](crate::NetBuilder::serializable_param), by wire
/// name (a parameter without a wire name is not saved), and `enabled`
/// every transition's switch, by name. Places, parameters and transitions
/// print in name order.
///
/// It is an error, not a quiet loss, to serialise a net that could not be
/// restored: one that no template built ([`NetError::NoTemplate`]), one with
/// a place that is not serializable, one with a transition in flight, or
/// one with a [subnet place](crate::NetBuilder::subnet_place) whose subnet
/// could not be restored ([`NetError::CannotSaveSubnet`]). A subnet is
/// written inside `marking`, under its place, in this same form.
/// A token type that owns a net derives `Serialize` as usual and gets the
/// net in this form under the field's name.
impl serde::Serialize for Net {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.saved()
            .map_err(S::Error::custom)?
            .serialize(serializer)
    }
}

/// A net from its saved form, rebuilt by its template from the registry in
/// [scope](Templates::scope); see [`Templates::restore`] for what it does
/// and in what order.
///
/// The saved `params` are handed to the factory and then set on what it
/// built through the JSON parameter route, so an unknown name or a value of
/// the wrong shape is an error, as it is for a `set_param` command. An
/// `enabled` entry that names no transition of the rebuilt net is an error
/// too. `template` and `marking` are required; `params` and `enabled` may be
/// left out, as empty. A key other than the four of the saved form is
/// ignored.
impl<'de> serde::Deserialize<'de> for Net {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Net, D::Error> {
        let loaded = Loaded::deserialize(deserializer)?;
        let factory = in_scope(&loaded.template).map_err(D::Error::custom)?;
        let template = loaded.template.clone();
        loaded
            .restore(&factory)
            .map_err(|error| D::Error::custom(format!("template {template:?}: {error}")))
    }
}

impl Loaded {
    /// Builds the net and puts back what was saved: the marking, then the
    /// parameters, then the switches.
    fn restore(self, factory: &Factory) -> Result<Net, BoxedError> {
        let mut net = build_stamped(&self.template, factory, &self.params)?;
        net.import_marking(&self.marking)?;
        for (param, value) in self.params {
            net.apply_json(JsonCommand::SetParam { param, value })?;
        }
        for (name, enabled) in self.enabled {
            let transition = net
                .transition_named(&name)
                .ok_or_else(|| format!("no transition named {name:?}"))?;
            net.transitions[transition.0 as usize].enabled = enabled;
        }
        net.forget_setup();
        Ok(net)
    }
}
