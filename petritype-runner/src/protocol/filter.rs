//! Filters: which tokens a query, a projection or a keyed edge keeps.

use petritype::KeyValue;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::PlaceName;

/// A filter on the wire (PROTOCOL.md §13.2): every clause must pass (AND
/// across clauses), and the token must lie in a place `places` names, if
/// given, whose name begins with `prefix`, if given. Places by name, a port
/// place of one of the net's subnet places by `{"place", "port"}`
/// ([`PlaceName`]); `prefix` is tested against a port's own name.
///
/// Read off a request with [`Filter::parse`], which is as strict as the
/// Python runner; the derived `Deserialize` is the lenient reading of a
/// line this runner wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filter {
    /// Every one must pass.
    #[serde(default)]
    pub clauses: Vec<Clause>,
    /// The places to look in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub places: Option<Vec<PlaceName>>,
    /// The start of the name of every place to look in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
}

/// One clause of a [`Filter`]: `{key, values}`, `{key, param}` or, in a
/// keyed edge's filter in a topology only, `{key, input}`; and, read
/// tolerantly from a topology, any other shape, as [`Clause::Other`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Clause {
    /// The token's `key` is one of `values` (OR within the clause),
    /// compared by JSON type and value.
    In {
        /// The key's name.
        key: String,
        /// The values accepted.
        values: Vec<KeyValue>,
    },
    /// The token's `key` equals the current value of the net parameter
    /// `param` (keyed edges, §13.5). `IsParam`, as the core's, and not
    /// `Param`, which is the core's parameter input.
    IsParam {
        /// The key's name.
        key: String,
        /// The net parameter's wire name.
        param: String,
    },
    /// The token's `key` equals the key `input.key` of the token the same
    /// firing gets from the input place `input` names (keyed edges, §13.5,
    /// since 2026-11-23). It has a meaning only within a firing, so it is
    /// found only in a keyed edge's filter in a topology; in a request's
    /// filter it is `invalid_params` ([`Filter::parse`]).
    IsInput {
        /// The key's name, on the edge's place.
        key: String,
        /// The source place and its key.
        input: InputKey,
    },
    /// A clause of a shape this version does not know, kept as it was
    /// written and written back unchanged (since 2026-11-23), so that a
    /// topology holding one is read whole (PROTOCOL.md §13.2). A request's
    /// filter never holds one: [`Filter::parse`] refuses an unknown shape.
    Other(Value),
}

/// The source of a [`Clause::IsInput`]: `{place, port?, key}`, the place
/// named as an edge names it (a port by its subnet place and `port`), and
/// the key of that place whose value is accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputKey {
    /// The source place, or the subnet place of a port.
    pub place: String,
    /// The port of the subnet place `place`, for a source on a port place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    /// The key of the source place.
    pub key: String,
}

impl Filter {
    /// Reads a request's filter (PROTOCOL.md §13.2), as strictly as the
    /// Python runner reads one: an object with no fields but `clauses`,
    /// `places` and `prefix`; `clauses` a list (or `null`) of objects with a
    /// string `key` and either a non-empty list `values` of strings,
    /// integers and booleans or a string `param`, never both; `places` a
    /// list of place names and `{"place", "port"}` objects
    /// ([`PlaceName::parse`]); `prefix` a string. Whether the places exist
    /// is the net's to say. A clause with `input`, which only a keyed edge's
    /// filter in a topology carries, has no meaning outside a firing and is
    /// refused (§13.2, since 2026-11-23).
    ///
    /// # Errors
    ///
    /// What is wrong with the shape, for an `invalid_params` answer.
    pub fn parse(value: &Value) -> Result<Filter, String> {
        let Value::Object(object) = value else {
            return Err("a filter is an object".into());
        };
        if let Some(unknown) = object
            .keys()
            .find(|key| !["clauses", "places", "prefix"].contains(&key.as_str()))
        {
            return Err(format!("a filter has no {unknown:?}"));
        }
        let clauses = match object.get("clauses") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(clauses)) => clauses
                .iter()
                .map(Clause::parse)
                .collect::<Result<_, _>>()?,
            Some(_) => return Err("a filter's \"clauses\" is a list".into()),
        };
        let places = match object.get("places") {
            None | Some(Value::Null) => None,
            Some(Value::Array(places)) => Some(
                places
                    .iter()
                    .map(PlaceName::parse)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|why| format!("in \"places\", {why}"))?,
            ),
            Some(_) => return Err("\"places\" is a list of places".into()),
        };
        let prefix = match object.get("prefix") {
            None | Some(Value::Null) => None,
            Some(Value::String(prefix)) => Some(prefix.clone()),
            Some(_) => return Err("\"prefix\" is a string".into()),
        };
        Ok(Filter {
            clauses,
            places,
            prefix,
        })
    }
}

impl Clause {
    fn parse(value: &Value) -> Result<Clause, String> {
        const SHAPE: &str = "a clause is {\"key\", \"values\"} or {\"key\", \"param\"}";
        let Some(Value::String(key)) = value.get("key") else {
            return Err(SHAPE.into());
        };
        if value.get("input").is_some() {
            return Err(
                "a clause with \"input\" chooses by another input of a firing, so it \
                        has no meaning in a request"
                    .into(),
            );
        }
        let key = key.clone();
        match (value.get("values"), value.get("param")) {
            (None, Some(Value::String(param))) => Ok(Clause::IsParam {
                key,
                param: param.clone(),
            }),
            (None, None) => Err(SHAPE.into()),
            (Some(_), Some(_)) | (None, Some(_)) => {
                Err("a clause has \"values\" or a string \"param\"".into())
            }
            (Some(values), None) => {
                let values = values
                    .as_array()
                    .filter(|values| !values.is_empty())
                    .and_then(|values| values.iter().map(key_value).collect::<Option<Vec<_>>>())
                    .ok_or(
                        "a clause's \"values\" is a non-empty list of strings, integers or booleans",
                    )?;
                Ok(Clause::In { key, values })
            }
        }
    }
}

/// A JSON value as a key value: a string, an integer that fits an `i64`, or
/// a boolean (PROTOCOL.md §13.1); `None` for anything else.
pub(crate) fn key_value(value: &Value) -> Option<KeyValue> {
    match value {
        Value::String(value) => Some(KeyValue::Str(value.clone())),
        Value::Number(number) => number.as_i64().map(KeyValue::Int),
        Value::Bool(value) => Some(KeyValue::Bool(*value)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    use crate::protocol::{EdgeKind, Topology, to_value};

    #[test]
    fn a_filter_reads_strictly_and_writes_what_it_has() {
        let filter = Filter::parse(&json!({
            "clauses": [{"key": "class", "values": ["Bird", 2, true]},
                        {"key": "class", "param": "season"}],
            "places": ["Forest"], "prefix": "F"}))
        .unwrap();
        assert_eq!(
            filter.clauses,
            [
                Clause::In {
                    key: "class".into(),
                    values: vec!["Bird".into(), KeyValue::Int(2), KeyValue::Bool(true)]
                },
                Clause::IsParam {
                    key: "class".into(),
                    param: "season".into()
                }
            ]
        );
        assert_eq!(
            to_value(&filter),
            json!({"clauses": [{"key": "class", "values": ["Bird", 2, true]},
                               {"key": "class", "param": "season"}],
                   "places": ["Forest"], "prefix": "F"})
        );
        assert_eq!(
            Filter::parse(&json!({})).unwrap(),
            Filter::default(),
            "everything is optional"
        );
        assert_eq!(
            Filter::parse(&json!({"clauses": null, "places": null, "prefix": null})).unwrap(),
            Filter::default()
        );
        assert_eq!(to_value(Filter::default()), json!({"clauses": []}));
        for bad in [
            json!(null),
            json!({"clauses": [{"key": "class"}]}),
            json!({"clauses": [{"key": "class", "values": [null]}]}),
            json!({"clauses": [{"key": "class", "values": "Bird"}]}),
            json!({"clauses": [{"key": "class", "param": 3}]}),
            json!({"clauses": [{"key": 3, "values": [1]}]}),
            json!({"places": [1]}),
            json!({"limit": 3}),
            json!({"clauses": [{"key": "name", "input": {"place": "Orders", "key": "unit"}}]}),
        ] {
            assert!(Filter::parse(&bad).is_err(), "{bad}");
        }
    }

    /// A topology holding an edge kind and a clause shape this version does
    /// not know is read whole: the kind and the clause are kept as written
    /// and written back unchanged, and the rest is read as usual.
    #[test]
    fn an_unknown_kind_and_an_unknown_clause_are_kept() {
        let value = json!({
            "places": [{"name": "Shelf", "type_name": "Book", "serializable": true}],
            "transitions": [{"name": "Glance", "detail": {"params": [], "detachable": false}}],
            "edges": [
                {"place": "Shelf", "transition": "Glance", "direction": "in", "kind": "glance",
                 "filter": {"clauses": [{"key": "title", "near": {"place": "Desk", "by": 2}},
                                        {"key": "shelf", "values": ["A"]}]}},
                {"place": "Shelf", "transition": "Glance", "direction": "in", "kind": "read_all"}
            ]
        });
        let topology: Topology = serde_json::from_value(value.clone()).unwrap();
        let edge = &topology.edges[0];
        assert_eq!(edge.kind, Some(EdgeKind::Other("glance".into())));
        let clauses = &edge.filter.as_ref().unwrap().clauses;
        assert_eq!(
            clauses[0],
            Clause::Other(json!({"key": "title", "near": {"place": "Desk", "by": 2}}))
        );
        assert_eq!(
            clauses[1],
            Clause::In {
                key: "shelf".into(),
                values: vec!["A".into()]
            }
        );
        assert_eq!(topology.edges[1].kind, Some(EdgeKind::ReadAll));
        assert_eq!(
            to_value(&topology.edges[0])["kind"],
            json!("glance"),
            "written back as it was"
        );
        assert_eq!(
            to_value(&topology.edges[0])["filter"]["clauses"][0],
            json!({"key": "title", "near": {"place": "Desk", "by": 2}})
        );
        assert!(
            Filter::parse(&json!({"clauses": [{"key": "title", "near": {}}]})).is_err(),
            "a request's filter is still read strictly"
        );
    }

    /// A clause that chooses by another input is written in a topology,
    /// its source named as an edge names its place, and read back by the
    /// lenient reading; a request's filter may not hold one.
    #[test]
    fn an_input_clause_is_written_and_read_back_but_refused_in_a_request() {
        let filter = Filter {
            clauses: vec![
                Clause::IsInput {
                    key: "name".into(),
                    input: InputKey {
                        place: "Orders".into(),
                        port: None,
                        key: "unit".into(),
                    },
                },
                Clause::IsInput {
                    key: "kind".into(),
                    input: InputKey {
                        place: "Yard".into(),
                        port: Some("Bay".into()),
                        key: "kind".into(),
                    },
                },
            ],
            places: None,
            prefix: None,
        };
        let value = to_value(&filter);
        assert_eq!(
            value,
            json!({"clauses": [
                {"key": "name", "input": {"place": "Orders", "key": "unit"}},
                {"key": "kind", "input": {"place": "Yard", "port": "Bay", "key": "kind"}}]})
        );
        let back: Filter = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(back, filter);
        let refused = Filter::parse(&value).unwrap_err();
        assert!(refused.contains("\"input\""), "{refused}");
    }
}
