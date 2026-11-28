//! Whether a value has a JSON form (since 2026-11-28).
//!
//! `serde_json` writes a `NaN` or an infinity as `null`, which reads back as
//! a value that is missing, or fails to read back at all. JSON has no
//! number for either, so a value that holds one has no JSON form, as in the
//! Python library, whose encoder refuses them: a token, a parameter value
//! and so a save, a [`Command::to_json`](crate::Command::to_json) and
//! [`Net::json`](crate::Net::json) say so as an error, where they wrote
//! `null`.
//!
//! The value is serialised once, as it always was. Only when that wrote a
//! `null` somewhere, which is the one place a non-finite number can hide,
//! is it walked again by [`Check`], a serializer that writes nothing and
//! stops at the first `NaN` or infinity, naming where it was.

use std::fmt;

use serde::Serialize;
use serde::ser::{self, Error as _};
use serde_json::Value;

/// The value as JSON, or why it has none: its own `Serialize` refused, or
/// it holds a `NaN` or an infinity, which the error names with where it is
/// (`weight`, `items[2]`, `a.b`).
pub(crate) fn to_json<T: Serialize + ?Sized>(value: &T) -> Result<Value, serde_json::Error> {
    let json = serde_json::to_value(value)?;
    if holds_null(&json)
        && let Err(Found::NotFinite { number, path }) = value.serialize(Check)
    {
        return Err(serde_json::Error::custom(unwritable(number, &path)));
    }
    Ok(json)
}

/// Whether a `null` is anywhere in `value`.
fn holds_null(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Array(items) => items.iter().any(holds_null),
        Value::Object(entries) => entries.values().any(holds_null),
        Value::Bool(_) | Value::Number(_) | Value::String(_) => false,
    }
}

/// What a value is told, when it holds a number JSON has no way to write.
fn unwritable(number: f64, path: &[Segment]) -> String {
    if path.is_empty() {
        return format!("the value is {number}, a number JSON has no way to write");
    }
    let mut at = String::new();
    // The segments came up from the inside, so the outermost is last.
    for segment in path.iter().rev() {
        match segment {
            Segment::Field(name) if at.is_empty() => at.push_str(name),
            Segment::Field(name) => {
                at.push('.');
                at.push_str(name);
            }
            Segment::Index(index) => at.push_str(&format!("[{index}]")),
        }
    }
    format!("the value holds {number} at {at}, a number JSON has no way to write")
}

/// One step into a value: a field or a map's key, or a position.
#[derive(Debug)]
enum Segment {
    Field(String),
    Index(usize),
}

/// What [`Check`] stops at.
#[derive(Debug)]
enum Found {
    /// A `NaN` or an infinity, with the way to it, innermost first.
    NotFinite { number: f64, path: Vec<Segment> },
    /// The value's own `Serialize` refused: the first pass said so already.
    Refused,
}

impl Found {
    /// The same, one step further out.
    fn at(self, segment: Segment) -> Found {
        match self {
            Found::NotFinite { number, mut path } => {
                path.push(segment);
                Found::NotFinite { number, path }
            }
            Found::Refused => Found::Refused,
        }
    }

    /// The same, under a variant's name when there is one, as `serde_json`
    /// writes a variant with data: `{"Variant": ...}`.
    fn in_variant(self, variant: Option<&'static str>) -> Found {
        match variant {
            Some(variant) => self.at(Segment::Field(variant.to_owned())),
            None => self,
        }
    }
}

impl fmt::Display for Found {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Found::NotFinite { number, path } => f.write_str(&unwritable(*number, path)),
            Found::Refused => f.write_str("the value's Serialize refused"),
        }
    }
}

impl std::error::Error for Found {}

impl ser::Error for Found {
    fn custom<T: fmt::Display>(_: T) -> Found {
        Found::Refused
    }
}

/// A number, if JSON can write it.
fn finite(number: f64) -> Result<(), Found> {
    if number.is_finite() {
        Ok(())
    } else {
        Err(Found::NotFinite {
            number,
            path: Vec::new(),
        })
    }
}

/// A serializer that writes nothing and stops at the first number JSON
/// has no way to write.
#[derive(Clone, Copy)]
struct Check;

/// A sequence, a tuple or a tuple variant, by position.
struct Elements {
    at: usize,
    variant: Option<&'static str>,
}

impl Elements {
    fn element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Found> {
        let at = self.at;
        self.at += 1;
        value
            .serialize(Check)
            .map_err(|found| found.at(Segment::Index(at)).in_variant(self.variant))
    }
}

/// A struct or a struct variant, by field.
struct Fields {
    variant: Option<&'static str>,
}

impl Fields {
    fn field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<(), Found> {
        value.serialize(Check).map_err(|found| {
            found
                .at(Segment::Field(key.to_owned()))
                .in_variant(self.variant)
        })
    }
}

/// A map, by key, the key written as JSON would write it.
struct Entries {
    key: String,
}

impl ser::Serializer for Check {
    type Ok = ();
    type Error = Found;
    type SerializeSeq = Elements;
    type SerializeTuple = Elements;
    type SerializeTupleStruct = Elements;
    type SerializeTupleVariant = Elements;
    type SerializeMap = Entries;
    type SerializeStruct = Fields;
    type SerializeStructVariant = Fields;

    fn serialize_bool(self, _: bool) -> Result<(), Found> {
        Ok(())
    }
    fn serialize_i8(self, _: i8) -> Result<(), Found> {
        Ok(())
    }
    fn serialize_i16(self, _: i16) -> Result<(), Found> {
        Ok(())
    }
    fn serialize_i32(self, _: i32) -> Result<(), Found> {
        Ok(())
    }
    fn serialize_i64(self, _: i64) -> Result<(), Found> {
        Ok(())
    }
    fn serialize_i128(self, _: i128) -> Result<(), Found> {
        Ok(())
    }
    fn serialize_u8(self, _: u8) -> Result<(), Found> {
        Ok(())
    }
    fn serialize_u16(self, _: u16) -> Result<(), Found> {
        Ok(())
    }
    fn serialize_u32(self, _: u32) -> Result<(), Found> {
        Ok(())
    }
    fn serialize_u64(self, _: u64) -> Result<(), Found> {
        Ok(())
    }
    fn serialize_u128(self, _: u128) -> Result<(), Found> {
        Ok(())
    }
    fn serialize_f32(self, number: f32) -> Result<(), Found> {
        finite(f64::from(number))
    }
    fn serialize_f64(self, number: f64) -> Result<(), Found> {
        finite(number)
    }
    fn serialize_char(self, _: char) -> Result<(), Found> {
        Ok(())
    }
    fn serialize_str(self, _: &str) -> Result<(), Found> {
        Ok(())
    }
    fn serialize_bytes(self, _: &[u8]) -> Result<(), Found> {
        Ok(())
    }
    fn serialize_none(self) -> Result<(), Found> {
        Ok(())
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<(), Found> {
        value.serialize(Check)
    }
    fn serialize_unit(self) -> Result<(), Found> {
        Ok(())
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<(), Found> {
        Ok(())
    }
    fn serialize_unit_variant(self, _: &'static str, _: u32, _: &'static str) -> Result<(), Found> {
        Ok(())
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        value: &T,
    ) -> Result<(), Found> {
        value.serialize(Check)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<(), Found> {
        value
            .serialize(Check)
            .map_err(|found| found.in_variant(Some(variant)))
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Elements, Found> {
        Ok(Elements {
            at: 0,
            variant: None,
        })
    }
    fn serialize_tuple(self, _: usize) -> Result<Elements, Found> {
        self.serialize_seq(None)
    }
    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Elements, Found> {
        self.serialize_seq(None)
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        _: usize,
    ) -> Result<Elements, Found> {
        Ok(Elements {
            at: 0,
            variant: Some(variant),
        })
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Entries, Found> {
        Ok(Entries { key: String::new() })
    }
    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Fields, Found> {
        Ok(Fields { variant: None })
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        _: usize,
    ) -> Result<Fields, Found> {
        Ok(Fields {
            variant: Some(variant),
        })
    }
}

impl ser::SerializeSeq for Elements {
    type Ok = ();
    type Error = Found;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Found> {
        self.element(value)
    }
    fn end(self) -> Result<(), Found> {
        Ok(())
    }
}

impl ser::SerializeTuple for Elements {
    type Ok = ();
    type Error = Found;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Found> {
        self.element(value)
    }
    fn end(self) -> Result<(), Found> {
        Ok(())
    }
}

impl ser::SerializeTupleStruct for Elements {
    type Ok = ();
    type Error = Found;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Found> {
        self.element(value)
    }
    fn end(self) -> Result<(), Found> {
        Ok(())
    }
}

impl ser::SerializeTupleVariant for Elements {
    type Ok = ();
    type Error = Found;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Found> {
        self.element(value)
    }
    fn end(self) -> Result<(), Found> {
        Ok(())
    }
}

impl ser::SerializeStruct for Fields {
    type Ok = ();
    type Error = Found;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Found> {
        self.field(key, value)
    }
    fn end(self) -> Result<(), Found> {
        Ok(())
    }
}

impl ser::SerializeStructVariant for Fields {
    type Ok = ();
    type Error = Found;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Found> {
        self.field(key, value)
    }
    fn end(self) -> Result<(), Found> {
        Ok(())
    }
}

impl ser::SerializeMap for Entries {
    type Ok = ();
    type Error = Found;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Found> {
        self.key = match serde_json::to_value(key) {
            Ok(Value::String(key)) => key,
            Ok(key) => key.to_string(),
            Err(_) => String::from("?"),
        };
        Ok(())
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Found> {
        value
            .serialize(Check)
            .map_err(|found| found.at(Segment::Field(std::mem::take(&mut self.key))))
    }
    fn end(self) -> Result<(), Found> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde::Serialize;
    use serde_json::json;

    use super::to_json;

    #[derive(Serialize)]
    struct Parcel {
        weight: f64,
        sizes: Vec<f32>,
        note: Option<String>,
    }

    #[derive(Serialize)]
    enum Reading {
        Level(f64),
        Pair(f64, f64),
        Point { x: f64 },
    }

    fn refusal<T: Serialize>(value: &T) -> String {
        to_json(value).expect_err("no JSON form").to_string()
    }

    #[test]
    fn a_finite_value_is_its_json_and_a_null_of_its_own_is_kept() {
        let parcel = Parcel {
            weight: 2.5,
            sizes: vec![1.0],
            note: None,
        };
        assert_eq!(
            to_json(&parcel).unwrap(),
            json!({"weight": 2.5, "sizes": [1.0], "note": null})
        );
        assert_eq!(to_json(&f64::MAX).unwrap(), json!(f64::MAX));
    }

    #[test]
    fn a_nan_or_an_infinity_has_no_json_form_and_says_where() {
        assert_eq!(
            refusal(&f64::NAN),
            "the value is NaN, a number JSON has no way to write"
        );
        assert_eq!(
            refusal(&f32::INFINITY),
            "the value is inf, a number JSON has no way to write"
        );
        let parcel = Parcel {
            weight: 1.0,
            sizes: vec![1.0, f32::NEG_INFINITY],
            note: Some("fragile".into()),
        };
        assert_eq!(
            refusal(&parcel),
            "the value holds -inf at sizes[1], a number JSON has no way to write"
        );
        assert!(refusal(&Some(f64::NAN)).starts_with("the value is NaN"));
        assert!(refusal(&Reading::Level(f64::NAN)).contains(" at Level,"));
        assert!(refusal(&Reading::Pair(0.0, f64::NAN)).contains(" at Pair[1],"));
        assert!(refusal(&Reading::Point { x: f64::NAN }).contains(" at Point.x,"));
        let mut map = BTreeMap::new();
        map.insert("depth", vec![f64::INFINITY]);
        assert!(refusal(&map).contains(" at depth[0],"));
    }
}
