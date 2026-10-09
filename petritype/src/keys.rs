//! Token keys, filters and queries (DESIGN §8.3).
//!
//! A place may declare **keys**: named attributes of its tokens, derived from
//! each token's value by an extractor the net's author writes
//! ([`NetBuilder::keys`](crate::NetBuilder::keys)). A habitat holding
//! species declares `species`, `class` and `diet`, and then "every bird" is
//! a question about keys the tokens carry, which the net can answer:
//!
//! - [`Net::keys`] reads one token's keys, as its place derives them now;
//! - [`Net::query`] finds every token a [`Filter`] lets through, each as a
//!   [`Match`] with its place, id, keys, label and (for a serializable
//!   place) JSON, in place order and then token order;
//! - [`Net::distinct_values`] lists the values one key takes, in order of
//!   first appearance — what a viewer builds its tabs from.
//!
//! Keys are derived, never stored on the token: a [`MutateEdge`] body or a
//! [`replace`](Net::replace) that changes the value changes its keys. The
//! net keeps an **index** from each key's values to the tokens holding them,
//! updated wherever a token enters a place, leaves one or changes in it, so
//! that a query costs about as much as what it finds rather than a scan of
//! the marking. A [`Snapshot`](crate::Snapshot) answers the same three
//! questions by scanning, having no index of its own.
//!
//! [`MutateEdge`]: crate::MutateEdge

use std::any::Any;
use std::borrow::Borrow;
use std::fmt;
use std::sync::Arc;

use crate::error::NetError;
use crate::ids::{PlaceId, TokenId};
use crate::net::Net;
use crate::param::ParamSlot;
use crate::place::PlaceSlot;

// ----- values and keys --------------------------------------------------------

/// The value of one key: a string, an integer or a boolean, and nothing else.
///
/// Values compare by variant and then by value, so `Int(1)`, `Str("1")` and
/// `Bool(true)` are three different values, as they are three different
/// JSON values on the wire (PROTOCOL.md §13.1). With the `serde` feature a
/// value is written and read *untagged*, as the bare JSON string, integer or
/// boolean it is.
///
/// ```
/// use petritype::KeyValue;
///
/// assert_eq!(KeyValue::from("Bird"), KeyValue::Str("Bird".to_owned()));
/// assert_ne!(KeyValue::from(1), KeyValue::from("1"));
/// assert_ne!(KeyValue::from(1), KeyValue::from(true));
/// assert_eq!(KeyValue::from(true).to_string(), "true");
/// ```
///
/// The order (`Ord`) is total but arbitrary across variants — every string
/// before every integer before every boolean — and exists so that values can
/// be sorted and kept in ordered sets. [`Net::distinct_values`] lists values
/// in order of first appearance, not in this order.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(untagged))]
pub enum KeyValue {
    /// A string, such as a name or an enum variant written as its name.
    Str(String),
    /// A whole number.
    Int(i64),
    /// A flag.
    Bool(bool),
}

impl From<&str> for KeyValue {
    fn from(value: &str) -> Self {
        KeyValue::Str(value.to_owned())
    }
}

impl From<String> for KeyValue {
    fn from(value: String) -> Self {
        KeyValue::Str(value)
    }
}

impl From<i64> for KeyValue {
    fn from(value: i64) -> Self {
        KeyValue::Int(value)
    }
}

impl From<bool> for KeyValue {
    fn from(value: bool) -> Self {
        KeyValue::Bool(value)
    }
}

/// The value alone, as a tab or a label shows it: a string without quotes, a
/// number, `true` or `false`. `Str("1")` and `Int(1)` therefore print alike;
/// `Debug` tells them apart.
impl fmt::Display for KeyValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyValue::Str(value) => f.write_str(value),
            KeyValue::Int(value) => fmt::Display::fmt(value, f),
            KeyValue::Bool(value) => fmt::Display::fmt(value, f),
        }
    }
}

impl KeyValue {
    /// A JSON value as a key value: a string, an integer that fits an `i64`,
    /// or a boolean; `None` for anything else (a float, a null, an array, an
    /// object). What a [`Clause::IsParam`] reads a net parameter's value as.
    #[cfg(feature = "serde")]
    pub(crate) fn from_json(value: &serde_json::Value) -> Option<KeyValue> {
        match value {
            serde_json::Value::String(value) => Some(KeyValue::Str(value.clone())),
            serde_json::Value::Number(number) => number.as_i64().map(KeyValue::Int),
            serde_json::Value::Bool(value) => Some(KeyValue::Bool(*value)),
            _ => None,
        }
    }
}

/// One token's keys: each key its place declares, by name, with the value
/// the place's extractor gave, in the order the names were declared.
///
/// Returned by value from [`Net::keys`] and carried by every [`Match`]: keys
/// are derived from the token whenever they are asked for, never stored on
/// it, so there is nothing to borrow. A token of a place that declares no
/// keys has none, and its `Keys` is empty.
///
/// ```
/// use petritype::{KeyValue, Keys};
///
/// let keys: Keys = [("species", KeyValue::from("Robin")), ("class", "Bird".into())]
///     .into_iter()
///     .collect();
/// assert_eq!(keys.get("class"), Some(&KeyValue::from("Bird")));
/// assert_eq!(keys.get("diet"), None);
/// assert_eq!(keys.names().collect::<Vec<_>>(), ["species", "class"]);
/// ```
///
/// With the `serde` feature it is `Serialize` as a JSON object, `{"species":
/// "Robin", "class": "Bird"}`, written in declaration order: the `keys` of a
/// token entry on the wire (PROTOCOL.md §13.1).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Keys {
    entries: Vec<(&'static str, KeyValue)>,
}

impl Keys {
    /// The value of the key with this name, or `None` if the token's place
    /// declares no such key.
    pub fn get(&self, name: &str) -> Option<&KeyValue> {
        self.entries
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value)
    }

    /// Every key with its value, in declaration order.
    pub fn iter(&self) -> impl Iterator<Item = (&'static str, &KeyValue)> {
        self.entries.iter().map(|(name, value)| (*name, value))
    }

    /// The key names, in declaration order: the place's
    /// [`key_names`](crate::PlaceInfo::key_names).
    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.entries.iter().map(|(name, _)| *name)
    }

    /// How many keys there are.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are none: the token's place declares no keys.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Collects `(name, value)` pairs in order, for a test or a reader that has
/// keys of its own to check a [`Filter`] against.
impl FromIterator<(&'static str, KeyValue)> for Keys {
    fn from_iter<I: IntoIterator<Item = (&'static str, KeyValue)>>(pairs: I) -> Self {
        Keys {
            entries: pairs.into_iter().collect(),
        }
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for Keys {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.entries.len()))?;
        for (name, value) in &self.entries {
            map.serialize_entry(name, value)?;
        }
        map.end()
    }
}

/// Derives one token's key values, in the order of the declared names. See
/// [`KeysVtable`].
pub(crate) type ExtractFn = Arc<dyn Fn(&dyn Any) -> Vec<KeyValue> + Send + Sync>;

/// The keys half of a place's vtable (DESIGN §4.2): the names declared with
/// [`NetBuilder::keys`](crate::NetBuilder::keys) and the extractor that gives
/// their values.
///
/// The extractor is a closure the author wrote, so unlike `debug` and
/// `clone` it cannot be a plain `fn` pointer: it is boxed, in an `Arc` so
/// that [`PlaceMeta`](crate::place::PlaceMeta) stays `Clone` and a
/// [`Snapshot`](crate::Snapshot) shares it. The wrapper around it is
/// monomorphised for `T` where the builder knows `T`, as the other entries
/// are.
#[derive(Clone)]
pub(crate) struct KeysVtable {
    /// The declared names, in order. Static, so that they are known — and on
    /// the wire as `key_names` — before any token exists.
    pub(crate) names: Arc<[&'static str]>,
    /// The values for those names, in the same order.
    pub(crate) extract: ExtractFn,
}

impl KeysVtable {
    /// The vtable for a place of `T`: the names, and the extractor wrapped
    /// so that it takes an erased token.
    ///
    /// The extractor returns an array of exactly as many values as there are
    /// names, so the two cannot drift apart.
    pub(crate) fn of<T, const N: usize>(
        names: [&'static str; N],
        extractor: impl Fn(&T) -> [KeyValue; N] + Send + Sync + 'static,
    ) -> KeysVtable
    where
        T: 'static,
    {
        KeysVtable {
            names: Arc::from(names.as_slice()),
            extract: Arc::new(move |value: &dyn Any| {
                let value = value
                    .downcast_ref::<T>()
                    .expect("token in a place is always of the place's type");
                Vec::from(extractor(value))
            }),
        }
    }

    /// One token's values, in declaration order.
    pub(crate) fn values(&self, value: &dyn Any) -> Vec<KeyValue> {
        (self.extract)(value)
    }

    /// One token's keys: the names zipped with its values.
    pub(crate) fn keys(&self, value: &dyn Any) -> Keys {
        Keys {
            entries: self.names.iter().copied().zip(self.values(value)).collect(),
        }
    }
}

impl fmt::Debug for KeysVtable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeysVtable")
            .field("names", &self.names)
            .finish_non_exhaustive()
    }
}

// ----- filters ----------------------------------------------------------------

/// Which tokens to keep: those that pass every [`Clause`], in the places
/// [`places`](Filter::places) names (if it names any) whose name begins with
/// [`prefix`](Filter::prefix) (if there is one).
///
/// AND across clauses, OR within one: `class in {Bird, Mammal} and diet in
/// {Insectivore}` is two clauses, the first with two values. A token in a
/// place that does not declare a clause's key passes no such clause, so a
/// filter with a clause never keeps a token of a place without keys; a
/// filter with no clauses keeps every token of the places it covers, and the
/// default filter, with no clauses and no restriction, keeps everything.
///
/// ```
/// use petritype::{Filter, KeyValue, Keys};
///
/// let birds = Filter::new().key_in("class", ["Bird"]);
/// let robin: Keys = [("species", KeyValue::from("Robin")), ("class", "Bird".into())]
///     .into_iter()
///     .collect();
/// assert!(birds.matches(&robin));
/// assert!(!birds.clone().key_in("diet", ["Insectivore"]).matches(&robin), "robin has no diet key");
/// assert!(Filter::new().matches(&Keys::default()), "no clauses, so everything");
/// ```
///
/// The restriction is by place: [`places`](Filter::places) by id and
/// [`prefix`](Filter::prefix) by the start of a place's name, the way a
/// viewer names a subsystem (PROTOCOL.md §13.2). It never mentions
/// transitions, which carry no tokens.
///
/// Plain data, built with the chainable methods below or as a struct. A
/// [`Clause::IsParam`] compares a key with the current value of a net
/// parameter, which a filter alone does not know: [`Net::query`] and
/// [`Net::matches`] read it from the net, and [`matches`](Filter::matches)
/// on its own passes no such clause.
///
/// A filter is also what a keyed input edge carries
/// ([`TakeWhereEdge`](crate::TakeWhereEdge),
/// [`MutateWhereEdge`](crate::MutateWhereEdge)): there it is asked of the
/// tokens of the edge's one place, and its `places` and `prefix` are not
/// consulted (DESIGN §8.3).
///
/// With the `serde` feature it is `Serialize` and `Deserialize` in the
/// protocol's shape (PROTOCOL.md §13.2) — `{"clauses": [{"key", "values"} |
/// {"key", "param"}], "places"?, "prefix"?}`, `clauses` defaulting to none
/// and the two restrictions written only when set — except that `places`
/// holds the core's [`PlaceId`]s, as everything the core serialises does
/// (DESIGN §19.2). The protocol names places by name; a runner maps one to
/// the other, as it does for every edge and event.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Filter {
    /// Every one of these must pass.
    #[cfg_attr(feature = "serde", serde(default))]
    pub clauses: Vec<Clause>,
    /// The places to look in, by id, or `None` for every place.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub places: Option<Vec<PlaceId>>,
    /// The start of the name of every place to look in, or `None` for any
    /// name.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub prefix: Option<String>,
}

/// One condition on one key of a token.
///
/// `#[non_exhaustive]`: match with a `_` arm.
///
/// With the `serde` feature a clause is written untagged, as the protocol
/// writes it: `{"key": "class", "values": ["Bird"]}` or `{"key": "class",
/// "param": "season"}`. An object with neither `values` nor `param` reads as
/// neither and is an error.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(untagged))]
#[non_exhaustive]
pub enum Clause {
    /// The token's value for `key` is one of `values`, compared by variant
    /// and value. Empty `values` accept nothing.
    In {
        /// The key's name.
        key: String,
        /// The accepted values.
        values: Vec<KeyValue>,
    },
    /// The token's value for `key` equals the current value of the net
    /// parameter whose wire name is `param` — the name given to
    /// `NetBuilder::serializable_param` — read as a [`KeyValue`] from its
    /// JSON form.
    ///
    /// A parameter whose JSON is not a string, an integer or a boolean, or a
    /// name the net has no parameter under, accepts nothing. Wire names are a
    /// `serde` facility, so without that feature this clause accepts nothing
    /// at all.
    ///
    /// `IsParam` rather than `Param`: a variant of that name beside
    /// [`Param`](crate::Param) makes the compiler spell out
    /// `petritype::Param<T>` in every error that mentions a parameter input.
    IsParam {
        /// The key's name.
        key: String,
        /// The wire name of the net parameter whose value is accepted.
        param: String,
    },
}

impl Filter {
    /// The filter that keeps everything: no clauses, every place.
    pub fn new() -> Filter {
        Filter::default()
    }

    /// Adds a clause: the token's `key` has one of `values`.
    pub fn key_in<V: Into<KeyValue>>(
        mut self,
        key: impl Into<String>,
        values: impl IntoIterator<Item = V>,
    ) -> Filter {
        self.clauses.push(Clause::In {
            key: key.into(),
            values: values.into_iter().map(Into::into).collect(),
        });
        self
    }

    /// Adds a clause: the token's `key` equals the net parameter registered
    /// under the wire name `param` (see [`Clause::IsParam`]).
    pub fn key_is_param(mut self, key: impl Into<String>, param: impl Into<String>) -> Filter {
        self.clauses.push(Clause::IsParam {
            key: key.into(),
            param: param.into(),
        });
        self
    }

    /// Restricts the filter to these places. A place that is not in the net
    /// makes [`Net::query`] fail with [`NetError::NoSuchPlace`].
    pub fn in_places(mut self, places: impl IntoIterator<Item = PlaceId>) -> Filter {
        self.places = Some(places.into_iter().collect());
        self
    }

    /// Restricts the filter to the places whose name begins with `prefix`.
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Filter {
        self.prefix = Some(prefix.into());
        self
    }

    /// Whether a token with these keys passes every clause, the token being
    /// in a place the filter covers. A [`Clause::IsParam`] passes nothing here,
    /// since a filter alone knows no parameters; [`Net::matches`] and
    /// [`Net::query`] read them from the net.
    pub fn matches(&self, keys: &Keys) -> bool {
        self.matches_with(keys, |_| None)
    }

    /// Whether a token with these keys passes every clause, a
    /// [`Clause::IsParam`] reading the parameter's value from `param` (by wire
    /// name; `None` accepts nothing).
    pub fn matches_with(&self, keys: &Keys, param: impl Fn(&str) -> Option<KeyValue>) -> bool {
        self.resolve(param).accepts(keys)
    }

    /// Whether the filter looks in this place at all, by its id and name.
    pub(crate) fn covers(&self, place: PlaceId, name: &str) -> bool {
        self.places
            .as_ref()
            .is_none_or(|places| places.contains(&place))
            && self
                .prefix
                .as_deref()
                .is_none_or(|prefix| name.starts_with(prefix))
    }

    /// The clauses with every [`Clause::IsParam`] replaced by the value it
    /// reads now: the form a filter is evaluated in, token after token.
    ///
    /// This is the primitive a query and a keyed edge share — a filter,
    /// resolved against the net's parameters once, then asked of one
    /// token's keys at a time.
    pub(crate) fn resolve(&self, param: impl Fn(&str) -> Option<KeyValue>) -> Resolved<'_> {
        Resolved {
            clauses: self
                .clauses
                .iter()
                .map(|clause| match clause {
                    Clause::In { key, values } => ResolvedClause {
                        key,
                        accepts: Accepts::Values(values),
                    },
                    Clause::IsParam { key, param: name } => ResolvedClause {
                        key,
                        accepts: Accepts::One(param(name)),
                    },
                })
                .collect(),
        }
    }

    /// [`resolve`](Filter::resolve) against a net's (or a snapshot's)
    /// parameters, by wire name.
    pub(crate) fn resolve_in(&self, params: &[ParamSlot]) -> Resolved<'_> {
        self.resolve(|name| param_key(params, name))
    }
}

/// A [`Filter`]'s clauses, every parameter read: see [`Filter::resolve`].
#[derive(Debug)]
pub(crate) struct Resolved<'f> {
    pub(crate) clauses: Vec<ResolvedClause<'f>>,
}

/// One clause, resolved: a key and the values it accepts.
#[derive(Debug)]
pub(crate) struct ResolvedClause<'f> {
    pub(crate) key: &'f str,
    accepts: Accepts<'f>,
}

/// What a resolved clause accepts: the values written in it, or the one
/// value a parameter had (or nothing, for a parameter that could not be
/// read as a key value).
#[derive(Debug)]
enum Accepts<'f> {
    Values(&'f [KeyValue]),
    One(Option<KeyValue>),
}

impl ResolvedClause<'_> {
    /// The accepted values, as a slice: what the index is asked about.
    pub(crate) fn values(&self) -> &[KeyValue] {
        match &self.accepts {
            Accepts::Values(values) => values,
            Accepts::One(value) => value.as_slice(),
        }
    }

    /// Whether a token with these keys passes.
    fn accepts(&self, keys: &Keys) -> bool {
        keys.get(self.key)
            .is_some_and(|value| self.values().contains(value))
    }
}

impl Resolved<'_> {
    /// Whether a token with these keys passes every clause.
    pub(crate) fn accepts(&self, keys: &Keys) -> bool {
        self.clauses.iter().all(|clause| clause.accepts(keys))
    }
}

/// The current value of the parameter registered under this wire name, as a
/// key value; `None` for no such name, or a value whose JSON is not a
/// string, an integer or a boolean. Without the `serde` feature there are no
/// wire names, and the answer is always `None`.
#[cfg(feature = "serde")]
pub(crate) fn param_key(params: &[ParamSlot], name: &str) -> Option<KeyValue> {
    let slot = params
        .iter()
        .find(|slot| slot.name.as_deref() == Some(name))?;
    let json = (slot.to_json?)(&*slot.value).ok()?;
    KeyValue::from_json(&json)
}

/// Without `serde`, no parameter has a wire name: see [`Clause::IsParam`].
#[cfg(not(feature = "serde"))]
pub(crate) fn param_key(_params: &[ParamSlot], _name: &str) -> Option<KeyValue> {
    None
}

// ----- matches ----------------------------------------------------------------

/// One token a [`Net::query`] found: where it is, its id, its keys, its label
/// and, for a place marked serializable, its JSON.
///
/// `#[non_exhaustive]`: a field may be added, and `json` is there only with
/// the `serde` feature.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Match {
    /// The place the token is in.
    pub place: PlaceId,
    /// The token.
    pub token: TokenId,
    /// Its keys, as its place derives them now; empty for a place that
    /// declares none.
    pub keys: Keys,
    /// Its [`label`](Net::label), cut at the net's label limit.
    pub label: String,
    /// Its JSON value ([`Net::json`]), or `None` for a place not marked
    /// serializable.
    #[cfg(feature = "serde")]
    pub json: Option<serde_json::Value>,
}

// ----- the read API -------------------------------------------------------------

impl Net {
    /// One token's keys, as its place derives them from the token now.
    ///
    /// `None` if the place declares no keys
    /// ([`NetBuilder::keys`](crate::NetBuilder::keys)), or does not exist,
    /// or holds no such token. By value: keys are computed from the token
    /// each time, never stored on it, which is what lets a mutation change
    /// them.
    ///
    /// ```
    /// use petritype::{KeyValue, NetBuilder};
    ///
    /// let mut b = NetBuilder::new();
    /// let names = b.place::<String>("Names");
    /// b.keys(names, ["initial", "long"], |name: &String| {
    ///     [name[..1].into(), (name.len() > 4).into()]
    /// });
    /// let mut net = b.build()?;
    ///
    /// let ada = net.put(names, "Ada".to_owned());
    /// let keys = net.keys(names.id(), ada).expect("a place with keys");
    /// assert_eq!(keys.get("initial"), Some(&KeyValue::from("A")));
    /// assert_eq!(keys.get("long"), Some(&KeyValue::Bool(false)));
    /// # Ok::<(), petritype::BuildError>(())
    /// ```
    pub fn keys(&self, place: PlaceId, token: TokenId) -> Option<Keys> {
        keys_in(&self.places, place, token)
    }

    /// Every token `filter` lets through, in place order and then token
    /// order (oldest first, the order [`tokens`](Net::tokens) gives), each as
    /// a [`Match`].
    ///
    /// ```
    /// use petritype::{Filter, NetBuilder};
    ///
    /// let mut b = NetBuilder::new();
    /// let forest = b.place::<(String, String)>("Forest");
    /// let wetland = b.place::<(String, String)>("Wetland");
    /// for place in [forest, wetland] {
    ///     b.keys(place, ["species", "class"], |(species, class): &(String, String)| {
    ///         [species.as_str().into(), class.as_str().into()]
    ///     });
    /// }
    /// b.seed(forest, [("Robin".into(), "Bird".into()), ("Fox".into(), "Mammal".into())]);
    /// b.seed(wetland, [("Heron".into(), "Bird".into())]);
    /// let net = b.build()?;
    ///
    /// let birds = net.query(&Filter::new().key_in("class", ["Bird"]))?;
    /// let found: Vec<_> = birds
    ///     .iter()
    ///     .map(|m| (m.place, m.keys.get("species").unwrap().to_string()))
    ///     .collect();
    /// assert_eq!(found, [(forest.id(), "Robin".to_owned()), (wetland.id(), "Heron".to_owned())]);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// A filter with clauses is answered from the net's index: the clause
    /// whose values the fewest tokens hold gives the candidates, and only
    /// they are read, so the cost follows what is found rather than the size
    /// of the marking. A filter without clauses keeps every token of the
    /// places it covers, which are read in full. A key no place declares
    /// finds nothing and is not an error. A [`Clause::IsParam`] reads the
    /// parameter's current value (with the `serde` feature; see there).
    ///
    /// # Errors
    ///
    /// [`NetError::NoSuchPlace`] if [`Filter::places`] names a place this net
    /// does not have.
    pub fn query(&self, filter: &Filter) -> Result<Vec<Match>, NetError> {
        check_places(&self.places, filter)?;
        Ok(self.query_indexed(filter))
    }

    /// The tokens `filter` lets through, as [`query`](Net::query) finds them
    /// and in the same order, as `(place, token)` pairs alone: nothing is
    /// rendered, so no label and no JSON is made for a token the caller
    /// will not show.
    ///
    /// For a reader that counts matches, pages them or keeps a set of them —
    /// a protocol runner's projection of the net through a client's filter
    /// (PROTOCOL.md §13.4) — and renders only the tokens it writes. Answered
    /// from the index, as `query` is.
    ///
    /// ```
    /// use petritype::{Filter, NetBuilder};
    ///
    /// let mut b = NetBuilder::new();
    /// let names = b.place::<String>("Names");
    /// b.keys(names, ["initial"], |name: &String| [name[..1].into()]);
    /// let mut net = b.build()?;
    /// net.put(names, "Ada".to_owned());
    /// let bram = net.put(names, "Bram".to_owned());
    ///
    /// let b_names = Filter::new().key_in("initial", ["B"]);
    /// assert_eq!(net.query_ids(&b_names)?, [(names.id(), bram)]);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    ///
    /// [`NetError::NoSuchPlace`] if [`Filter::places`] names a place this net
    /// does not have.
    pub fn query_ids(&self, filter: &Filter) -> Result<Vec<(PlaceId, TokenId)>, NetError> {
        check_places(&self.places, filter)?;
        Ok(self.query_ids_indexed(filter))
    }

    /// The distinct values of `key` over the tokens of the net, or of
    /// `places`, in order of first appearance: place order, then token
    /// order. Empty for a key no place declares.
    ///
    /// What a viewer builds one tab per value from: for a habitat net,
    /// `distinct_values("class", None)` is `Bird, Mammal, Reptile, Fish` if
    /// the first bird lives in the first habitat. Read from the index; a
    /// place in `places` that this net does not have contributes nothing.
    pub fn distinct_values(&self, key: &str, places: Option<&[PlaceId]>) -> Vec<KeyValue> {
        self.distinct_indexed(key, places)
    }

    /// Whether one token passes `filter`: it is in a place the filter
    /// covers, and its keys pass every clause, a [`Clause::IsParam`] reading
    /// the net's parameter now. False for a place or a token that is not
    /// there.
    ///
    /// The check a reader makes of one token at a time — whether a token
    /// that has just been added, or has just changed, is one a filtered view
    /// should show — where [`query`](Net::query) finds them all.
    pub fn matches(&self, filter: &Filter, place: PlaceId, token: TokenId) -> bool {
        matches_in(&self.places, &self.params, filter, place, token)
    }

    /// The token a keyed edge with this filter would choose from `place`
    /// now: the first, in queue order, whose keys pass every clause — the
    /// oldest, unless a failed firing has put a token back at the front.
    /// `None` when no token of the place passes, or there is no such place.
    ///
    /// What a [`TakeWhereEdge`](crate::TakeWhereEdge) takes and a
    /// [`MutateWhereEdge`](crate::MutateWhereEdge) hands over, so a guard on
    /// such a transition reads it where a guard on a plain edge reads the
    /// front token. Asked as the edge asks it: of this one place, so the
    /// filter's [`places`](Filter::places) and [`prefix`](Filter::prefix)
    /// are not consulted, a [`Clause::IsParam`] reading the parameter now,
    /// and answered from the index.
    ///
    /// ```
    /// use petritype::{Filter, NetBuilder};
    ///
    /// let mut b = NetBuilder::new();
    /// let names = b.place::<String>("Names");
    /// b.keys(names, ["initial"], |name: &String| [name[..1].into()]);
    /// let mut net = b.build()?;
    /// net.put(names, "Ada".to_owned());
    /// let bram = net.put(names, "Bram".to_owned());
    ///
    /// let b_names = Filter::new().key_in("initial", ["B"]);
    /// assert_eq!(net.first_match(names.id(), &b_names), Some(bram));
    /// assert_eq!(net.first_match(names.id(), &Filter::new().key_in("initial", ["C"])), None);
    /// # Ok::<(), petritype::BuildError>(())
    /// ```
    pub fn first_match(&self, place: PlaceId, filter: &Filter) -> Option<TokenId> {
        crate::index::first_match(&self.places, &self.params, &self.index, place, filter)
            .map(|(_, token)| token)
    }
}

// ----- the same questions, over places alone ------------------------------------
//
// A snapshot has no index, and asks these questions by scanning. They are
// also what the index is checked against.

/// One token's keys, read through its place's vtable.
pub(crate) fn keys_in(
    places: &[impl Borrow<PlaceSlot>],
    place: PlaceId,
    token: TokenId,
) -> Option<Keys> {
    let slot = places.get(place.0 as usize)?.borrow();
    let keys = slot.meta.keys.as_ref()?;
    let index = slot.position(token)?;
    Some(keys.keys(&*slot.tokens[index].1))
}

/// A token's keys, or none for a place that declares none.
pub(crate) fn keys_or_none(slot: &PlaceSlot, value: &dyn Any) -> Keys {
    slot.meta
        .keys
        .as_ref()
        .map_or_else(Keys::default, |keys| keys.keys(value))
}

/// [`NetError::NoSuchPlace`] for the first place a filter names that is not
/// there.
pub(crate) fn check_places(
    places: &[impl Borrow<PlaceSlot>],
    filter: &Filter,
) -> Result<(), NetError> {
    if let Some(named) = &filter.places
        && let Some(&place) = named.iter().find(|place| place.0 as usize >= places.len())
    {
        return Err(NetError::NoSuchPlace { place });
    }
    Ok(())
}

/// One token as a [`Match`].
pub(crate) fn match_of(
    slot: &PlaceSlot,
    place: PlaceId,
    token: TokenId,
    value: &dyn Any,
    keys: Keys,
    label_limit: usize,
) -> Match {
    Match {
        place,
        token,
        keys,
        label: crate::net::truncate_label((slot.meta.debug)(value), label_limit),
        #[cfg(feature = "serde")]
        json: slot
            .meta
            .to_json
            .map(|to_json| crate::place::json_or_message(to_json, value)),
    }
}

/// Every token a filter lets through, by scanning every place it covers.
pub(crate) fn query_scan(
    places: &[impl Borrow<PlaceSlot>],
    params: &[ParamSlot],
    label_limit: usize,
    filter: &Filter,
) -> Result<Vec<Match>, NetError> {
    let mut found = Vec::new();
    scan(places, params, filter, |slot, place, token, value, keys| {
        found.push(match_of(slot, place, token, value, keys, label_limit));
    })?;
    Ok(found)
}

/// The tokens a filter lets through, as ids, by scanning.
pub(crate) fn query_ids_scan(
    places: &[impl Borrow<PlaceSlot>],
    params: &[ParamSlot],
    filter: &Filter,
) -> Result<Vec<(PlaceId, TokenId)>, NetError> {
    let mut found = Vec::new();
    scan(places, params, filter, |_, place, token, _, _| {
        found.push((place, token));
    })?;
    Ok(found)
}

/// Calls `found` with each token a filter lets through, in place order then
/// token order, by reading every place it covers.
fn scan(
    places: &[impl Borrow<PlaceSlot>],
    params: &[ParamSlot],
    filter: &Filter,
    mut found: impl FnMut(&PlaceSlot, PlaceId, TokenId, &dyn Any, Keys),
) -> Result<(), NetError> {
    check_places(places, filter)?;
    let resolved = filter.resolve_in(params);
    for (index, slot) in places.iter().enumerate() {
        let slot = slot.borrow();
        let place = PlaceId(index as u32);
        if !filter.covers(place, &slot.meta.name) {
            continue;
        }
        // A clause asks for a key, which a place without keys never has.
        if !filter.clauses.is_empty() && slot.meta.keys.is_none() {
            continue;
        }
        for (token, value) in &slot.tokens {
            let keys = keys_or_none(slot, &**value);
            if resolved.accepts(&keys) {
                found(slot, place, *token, &**value, keys);
            }
        }
    }
    Ok(())
}

/// The distinct values of a key, in order of first appearance, by scanning.
pub(crate) fn distinct_scan(
    places: &[impl Borrow<PlaceSlot>],
    key: &str,
    only: Option<&[PlaceId]>,
) -> Vec<KeyValue> {
    let mut seen: Vec<KeyValue> = Vec::new();
    for (index, slot) in places.iter().enumerate() {
        let slot = slot.borrow();
        if only.is_some_and(|only| !only.contains(&PlaceId(index as u32))) {
            continue;
        }
        let Some(keys) = &slot.meta.keys else {
            continue;
        };
        let Some(position) = keys.names.iter().position(|name| *name == key) else {
            continue;
        };
        for (_, value) in &slot.tokens {
            let value = keys.values(&**value).swap_remove(position);
            if !seen.contains(&value) {
                seen.push(value);
            }
        }
    }
    seen
}

/// Whether one token passes a filter, the parameters read from `params`.
pub(crate) fn matches_in(
    places: &[impl Borrow<PlaceSlot>],
    params: &[ParamSlot],
    filter: &Filter,
    place: PlaceId,
    token: TokenId,
) -> bool {
    let Some(slot) = places.get(place.0 as usize).map(Borrow::borrow) else {
        return false;
    };
    let Some(index) = slot.position(token) else {
        return false;
    };
    if !filter.covers(place, &slot.meta.name) {
        return false;
    }
    if filter.clauses.is_empty() {
        return true;
    }
    let keys = keys_or_none(slot, &*slot.tokens[index].1);
    filter.resolve_in(params).accepts(&keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(pairs: &[(&'static str, KeyValue)]) -> Keys {
        pairs.iter().cloned().collect()
    }

    #[test]
    fn a_value_compares_by_variant_and_value() {
        assert_ne!(KeyValue::Int(1), KeyValue::Str("1".into()));
        assert_ne!(KeyValue::Int(1), KeyValue::Bool(true));
        assert_ne!(KeyValue::Str("true".into()), KeyValue::Bool(true));
        assert_eq!(KeyValue::from(7), KeyValue::Int(7));
    }

    #[test]
    fn within_a_clause_any_value_and_across_clauses_every_clause() {
        let robin = keys(&[("class", "Bird".into()), ("diet", "Insectivore".into())]);
        let heron = keys(&[("class", "Bird".into()), ("diet", "Carnivore".into())]);
        let filter = Filter::new()
            .key_in("class", ["Bird", "Mammal"])
            .key_in("diet", ["Insectivore"]);
        assert!(filter.matches(&robin));
        assert!(!filter.matches(&heron));
        assert!(
            !Filter::new()
                .key_in("class", Vec::<KeyValue>::new())
                .matches(&robin),
            "no values accept nothing"
        );
    }

    #[test]
    fn a_param_clause_reads_its_value_from_outside() {
        let robin = keys(&[("class", "Bird".into())]);
        let filter = Filter::new().key_is_param("class", "season");
        assert!(!filter.matches(&robin), "no parameters, no match");
        assert!(filter.matches_with(&robin, |name| (name == "season").then(|| "Bird".into())));
        assert!(!filter.matches_with(&robin, |_| Some(KeyValue::Int(1))));
    }

    #[test]
    fn covers_is_by_place_and_by_name_prefix() {
        let filter = Filter::new()
            .in_places([PlaceId(1), PlaceId(2)])
            .with_prefix("Plat");
        assert!(filter.covers(PlaceId(1), "Platform@Depot"));
        assert!(!filter.covers(PlaceId(0), "Platform@Depot"));
        assert!(!filter.covers(PlaceId(2), "Depot"));
        assert!(Filter::new().covers(PlaceId(9), "anything"));
    }
}
