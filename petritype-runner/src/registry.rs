//! The nets a binary carries: a plain table of [`NetEntry`]s, passed to
//! [`main`](crate::main).

use std::fmt;

use petritype::{BuildError, Net};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::protocol::{NetDescription, ParamDescription};

/// A net's load parameters: the JSON object `load {params}` carried, with the
/// declared defaults filled in for keys it left out.
pub type Params = serde_json::Map<String, Value>;

/// What builds a net from its load parameters.
///
/// Boxed rather than a plain `fn` pointer because [`NetEntry::typed`] wraps
/// the author's function in a closure that decodes the parameters first, and
/// a closure that captures a generic function is not a `fn` pointer.
pub type Factory = Box<dyn Fn(&Params) -> Result<Net, LoadError> + Send + Sync>;

/// Why a factory produced no net.
///
/// Reported to the host as `invalid_params` for
/// [`InvalidParams`](LoadError::InvalidParams) and `load_failed` for the
/// other two. A factory that panics is `load_failed` as well, with the panic
/// message.
#[derive(Debug)]
pub enum LoadError {
    /// The parameters were missing, of the wrong type, or out of range.
    InvalidParams(String),
    /// The net did not build.
    Build(BuildError),
    /// Anything else: a file that would not open, a service that did not
    /// answer.
    Other(String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::InvalidParams(message) => write!(f, "invalid params: {message}"),
            LoadError::Build(error) => write!(f, "the net did not build: {error}"),
            LoadError::Other(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LoadError::Build(error) => Some(error),
            _ => None,
        }
    }
}

impl From<BuildError> for LoadError {
    fn from(error: BuildError) -> Self {
        LoadError::Build(error)
    }
}

/// How a host should run a net: `describe`'s `mode` and `schedule`.
///
/// A hint for a host that schedules; the runner itself only looks at
/// [`Continuous`](Hosting::Continuous), which idles at quiescence instead of
/// stopping.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Hosting {
    /// Run when someone asks: `"manual"`.
    #[default]
    Manual,
    /// Keep running, idling when nothing is fireable: `"24/7"`.
    Continuous,
    /// Run to completion and stop: `"batch"`.
    Batch,
    /// Run on a schedule, given as a cron expression: `"cron"`.
    Cron(&'static str),
}

impl Hosting {
    fn mode(self) -> &'static str {
        match self {
            Hosting::Manual => "manual",
            Hosting::Continuous => "24/7",
            Hosting::Batch => "batch",
            Hosting::Cron(_) => "cron",
        }
    }

    fn schedule(self) -> Option<&'static str> {
        match self {
            Hosting::Cron(schedule) => Some(schedule),
            _ => None,
        }
    }
}

/// One load parameter a net declares, for `describe`.
///
/// ```
/// use petritype_runner::ParamSpec;
/// let trucks = ParamSpec::new("trucks", "u32").default(2);
/// let region = ParamSpec::new("region", "String").required();
/// # let _ = (trucks, region);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct ParamSpec {
    name: &'static str,
    type_name: &'static str,
    required: bool,
    default: Option<Value>,
}

impl ParamSpec {
    /// A parameter that is optional and has no default: the factory sees it
    /// only when `load` supplies it.
    pub fn new(name: &'static str, type_name: &'static str) -> Self {
        ParamSpec {
            name,
            type_name,
            required: false,
            default: None,
        }
    }

    /// The value used when `load` leaves this parameter out. Makes the
    /// parameter optional.
    pub fn default(mut self, value: impl Into<Value>) -> Self {
        self.default = Some(value.into());
        self.required = false;
        self
    }

    /// `load` must supply this parameter; a load without it is
    /// `invalid_params` and the factory is not called. Drops any default.
    pub fn required(mut self) -> Self {
        self.required = true;
        self.default = None;
        self
    }

    fn describe(&self) -> ParamDescription {
        ParamDescription {
            name: self.name.to_owned(),
            type_name: self.type_name.to_owned(),
            required: self.required,
            default: self.default.clone(),
        }
    }
}

/// One net a binary carries: its name, its factory, and what `describe` says
/// about it.
///
/// ```
/// use petritype::{Net, NetBuilder};
/// use petritype_runner::{Hosting, LoadError, NetEntry, ParamSpec, Params};
///
/// fn build(_params: &Params) -> Result<Net, LoadError> {
///     let mut b = NetBuilder::new();
///     b.place::<u32>("Inbox");
///     Ok(b.build()?)
/// }
///
/// let entry = NetEntry::new("inbox", build)
///     .description("Holds numbers")
///     .param(ParamSpec::new("size", "u32").default(3))
///     .hosting(Hosting::Continuous);
/// # let _ = entry;
/// ```
pub struct NetEntry {
    pub(crate) name: &'static str,
    pub(crate) factory: Factory,
    description: Option<&'static str>,
    pub(crate) params: Vec<ParamSpec>,
    pub(crate) hosting: Hosting,
}

impl NetEntry {
    /// A net built by `factory` from the JSON load parameters.
    pub fn new<F>(name: &'static str, factory: F) -> Self
    where
        F: Fn(&Params) -> Result<Net, LoadError> + Send + Sync + 'static,
    {
        NetEntry {
            name,
            factory: Box::new(factory),
            description: None,
            params: Vec::new(),
            hosting: Hosting::Manual,
        }
    }

    /// A net built by `factory` from parameters decoded into `P` first.
    ///
    /// A parameter object that does not decode as `P` is reported as
    /// `invalid_params` with serde's message, and `factory` is not called.
    ///
    /// ```
    /// use petritype::{Net, NetBuilder};
    /// use petritype_runner::{LoadError, NetEntry, ParamSpec};
    ///
    /// #[derive(serde::Deserialize)]
    /// struct Size { size: u32 }
    ///
    /// fn build(params: Size) -> Result<Net, LoadError> {
    ///     let mut b = NetBuilder::new();
    ///     let inbox = b.place::<u32>("Inbox");
    ///     b.seed(inbox, 0..params.size);
    ///     Ok(b.build()?)
    /// }
    ///
    /// let entry = NetEntry::typed("inbox", build).param(ParamSpec::new("size", "u32").default(3));
    /// # let _ = entry;
    /// ```
    pub fn typed<P, F>(name: &'static str, factory: F) -> Self
    where
        P: DeserializeOwned,
        F: Fn(P) -> Result<Net, LoadError> + Send + Sync + 'static,
    {
        NetEntry::new(name, move |params: &Params| {
            let decoded = serde_json::from_value::<P>(Value::Object(params.clone()))
                .map_err(|error| LoadError::InvalidParams(error.to_string()))?;
            factory(decoded)
        })
    }

    /// What the net is for, shown by `describe`.
    pub fn description(mut self, description: &'static str) -> Self {
        self.description = Some(description);
        self
    }

    /// Declares one load parameter. Defaults are filled in before the
    /// factory runs, and a missing required parameter is `invalid_params`.
    pub fn param(mut self, param: ParamSpec) -> Self {
        self.params.push(param);
        self
    }

    /// How a host should run it; [`Hosting::Manual`] unless set.
    pub fn hosting(mut self, hosting: Hosting) -> Self {
        self.hosting = hosting;
        self
    }

    /// The registered name.
    pub fn name(&self) -> &'static str {
        self.name
    }

    pub(crate) fn describe(&self) -> NetDescription {
        NetDescription {
            name: self.name.to_owned(),
            description: self.description.map(str::to_owned),
            mode: self.hosting.mode().to_owned(),
            schedule: self.hosting.schedule().map(str::to_owned),
            params: self.params.iter().map(ParamSpec::describe).collect(),
        }
    }

    /// The parameters the factory sees: `given` with every declared default
    /// filled in for a key it lacks.
    ///
    /// # Errors
    ///
    /// The message for `invalid_params` when a required parameter is missing.
    pub(crate) fn complete(&self, mut given: Params) -> Result<Params, String> {
        for spec in &self.params {
            if given.contains_key(spec.name) {
                continue;
            }
            if let Some(default) = &spec.default {
                given.insert(spec.name.to_owned(), default.clone());
            } else if spec.required {
                return Err(format!("missing required parameter {:?}", spec.name));
            }
        }
        Ok(given)
    }
}

impl fmt::Debug for NetEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NetEntry")
            .field("name", &self.name)
            .field("description", &self.description)
            .field("params", &self.params)
            .field("hosting", &self.hosting)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn empty(_: &Params) -> Result<Net, LoadError> {
        Ok(petritype::NetBuilder::new().build()?)
    }

    #[test]
    fn param_spec_required_and_default_exclude_each_other() {
        let spec = ParamSpec::new("n", "u32").required().default(2);
        assert!(!spec.required);
        assert_eq!(spec.default, Some(json!(2)));
        let spec = ParamSpec::new("n", "u32").default(2).required();
        assert!(spec.required);
        assert_eq!(spec.default, None);
    }

    #[test]
    fn defaults_are_filled_and_required_ones_checked() {
        let entry = NetEntry::new("x", empty)
            .param(ParamSpec::new("a", "u32").default(1))
            .param(ParamSpec::new("b", "u32").required())
            .param(ParamSpec::new("c", "u32"));
        let mut given = Params::new();
        assert!(entry.complete(given.clone()).is_err());
        given.insert("b".into(), json!(5));
        let full = entry.complete(given).unwrap();
        assert_eq!(Value::Object(full), json!({"a": 1, "b": 5}));
    }

    #[test]
    fn hosting_becomes_mode_and_schedule() {
        let d = NetEntry::new("x", empty)
            .hosting(Hosting::Cron("0 * * * *"))
            .describe();
        assert_eq!(
            (d.mode.as_str(), d.schedule.as_deref()),
            ("cron", Some("0 * * * *"))
        );
        let d = NetEntry::new("x", empty).describe();
        assert_eq!((d.mode.as_str(), d.schedule), ("manual", None));
        assert_eq!(
            NetEntry::new("x", empty)
                .hosting(Hosting::Continuous)
                .describe()
                .mode,
            "24/7"
        );
        assert_eq!(
            NetEntry::new("x", empty)
                .hosting(Hosting::Batch)
                .describe()
                .mode,
            "batch"
        );
    }

    #[test]
    fn a_typed_factory_reports_a_bad_parameter_as_invalid_params() {
        #[derive(serde::Deserialize)]
        struct P {
            #[allow(dead_code)]
            n: u32,
        }
        let entry = NetEntry::typed("x", |_: P| empty(&Params::new()));
        let mut params = Params::new();
        params.insert("n".into(), json!("three"));
        assert!(matches!(
            (entry.factory)(&params),
            Err(LoadError::InvalidParams(_))
        ));
        params.insert("n".into(), json!(3));
        assert!((entry.factory)(&params).is_ok());
    }
}
