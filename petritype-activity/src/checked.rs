//! The numbers a file read back must hold to mean anything: checked as
//! they are read, so that a value no model could have written is refused
//! with the field it was in, rather than spreading through a replay.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer};

/// An `f32` that is finite and not below 0.
pub(crate) fn finite_non_negative_f32<'de, D: Deserializer<'de>>(d: D) -> Result<f32, D::Error> {
    let value = f32::deserialize(d)?;
    if value.is_finite() && value >= 0.0 {
        Ok(value)
    } else {
        Err(D::Error::custom(format!(
            "{value} is not a finite number of zero or more"
        )))
    }
}

/// An `Option<f32>` whose number, if there is one, is finite and not below
/// 0.
pub(crate) fn finite_non_negative_f32_option<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<Option<f32>, D::Error> {
    match Option::<f32>::deserialize(d)? {
        Some(value) if !(value.is_finite() && value >= 0.0) => Err(D::Error::custom(format!(
            "{value} is not a finite number of zero or more"
        ))),
        value => Ok(value),
    }
}

/// An `f64` that is finite and not below 0.
pub(crate) fn finite_non_negative_f64<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    let value = f64::deserialize(d)?;
    if value.is_finite() && value >= 0.0 {
        Ok(value)
    } else {
        Err(D::Error::custom(format!(
            "{value} is not a finite number of zero or more"
        )))
    }
}

/// A width in milliseconds of at least one.
pub(crate) fn at_least_one_ms<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    match u64::deserialize(d)? {
        0 => Err(D::Error::custom("a bin is at least 1 ms wide, not 0")),
        ms => Ok(ms),
    }
}
