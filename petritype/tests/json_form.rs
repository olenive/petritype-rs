//! A token that holds a `NaN` or an infinity has no JSON form (since
//! 2026-11-28), as in the Python library: JSON has no number for either.
//! [`Net::json`], [`Snapshot::json`](petritype::Snapshot::json) and
//! [`Match::json`](petritype::Match::json) say so with [`NoJsonForm`], as
//! they say it of a token whose `Serialize` refuses, where they wrote `null`
//! for the number and the refusal's text for the token; a save and
//! [`Command::to_json`] refuse it, where they wrote `null`.
#![cfg(feature = "serde")]

use petritype::serde_json::{self, json};
use petritype::{Command, CommandError, Filter, Net, NetBuilder, NetError, PlaceId, TokenId};
use serde::{Serialize, Serializer};

/// A reading with a number JSON may not be able to write.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
struct Reading {
    sensor: String,
    value: f64,
}

/// A token whose `Serialize` always refuses.
#[derive(Debug, Clone)]
struct Secret;

impl Serialize for Secret {
    fn serialize<S: Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("a secret is not written down"))
    }
}

impl<'de> serde::Deserialize<'de> for Secret {
    fn deserialize<D: serde::Deserializer<'de>>(_: D) -> Result<Secret, D::Error> {
        Ok(Secret)
    }
}

/// `Readings` holding a finite reading, a NaN, an infinity and minus
/// infinity, and `Secrets` holding one secret, both serializable ports.
fn readings() -> (Net, PlaceId, Vec<TokenId>, PlaceId) {
    let mut b = NetBuilder::new();
    let readings = b.place::<Reading>("Readings");
    let secrets = b.place::<Secret>("Secrets");
    b.serializable(readings);
    b.serializable(secrets);
    b.port(readings);
    b.port(secrets);
    b.keys(readings, ["sensor"], |reading: &Reading| {
        [reading.sensor.as_str().into()]
    });
    let seed = |sensor: &str, value: f64| Reading {
        sensor: sensor.to_owned(),
        value,
    };
    b.seed(
        readings,
        [
            seed("a", 2.5),
            seed("b", f64::NAN),
            seed("c", f64::INFINITY),
            seed("d", f64::NEG_INFINITY),
        ],
    );
    b.seed(secrets, [Secret]);
    let net = b.build().expect("the net builds");
    let tokens = net.tokens(readings.id()).map(|(token, _)| token).collect();
    (net, readings.id(), tokens, secrets.id())
}

#[test]
fn a_token_that_holds_a_nan_or_an_infinity_has_no_json_form() {
    let (net, place, tokens, secrets) = readings();
    assert_eq!(
        net.json(place, tokens[0]),
        Some(Ok(json!({"sensor": "a", "value": 2.5})))
    );
    for (token, number) in tokens[1..].iter().zip(["NaN", "inf", "-inf"]) {
        let none = net
            .json(place, *token)
            .expect("a serializable place")
            .expect_err("no JSON form");
        assert_eq!(
            none.message,
            format!("the value holds {number} at value, a number JSON has no way to write")
        );
        assert_eq!(
            none.to_string(),
            format!("the token has no JSON form: {}", none.message)
        );
        assert!(
            net.debug(place, *token).unwrap().contains(number),
            "the label is as before"
        );
    }
    let (secret, _) = net.tokens(secrets).next().unwrap();
    let refused = net.json(secrets, secret).unwrap().unwrap_err();
    assert_eq!(refused.message, "a secret is not written down");

    // A snapshot and a query say the same.
    let taken = net.snapshot().expect("every place clones");
    assert_eq!(taken.json(place, tokens[1]), net.json(place, tokens[1]));
    let matches = net.query(&Filter::new()).expect("a query");
    let json: Vec<bool> = matches
        .iter()
        .filter(|found| found.place == place)
        .map(|found| found.json.as_ref().expect("serializable").is_ok())
        .collect();
    assert_eq!(json, [true, false, false, false]);
    let secret = matches.iter().find(|found| found.place == secrets).unwrap();
    assert_eq!(
        secret.json,
        Some(Err(refused)),
        "the refusal, where its text stood in for the token"
    );
}

#[test]
fn a_save_refuses_a_token_that_holds_a_nan() {
    let (mut net, place, _, secrets) = readings();
    // The secret refuses on its own account; take it out first.
    let (secret, _) = net.tokens(secrets).next().unwrap();
    net.remove(secrets, secret)
        .expect("a write from the program");
    let error = net.export_marking().expect_err("no JSON form for NaN");
    let NetError::CannotSerialize {
        place: refused,
        name,
        source,
        ..
    } = &error
    else {
        panic!("CannotSerialize, not {error:?}");
    };
    assert_eq!((*refused, name.as_str()), (place, "Readings"));
    assert_eq!(
        source.to_string(),
        "the value holds NaN at value, a number JSON has no way to write"
    );
    assert!(serde_json::to_string(&net).is_err(), "nor is the net saved");

    let command = Command::put(
        net.place_ref::<Reading>("Readings").unwrap(),
        Reading {
            sensor: "e".into(),
            value: f64::NAN,
        },
    );
    let refused = command.to_json(&net).expect_err("no JSON form for NaN");
    assert!(
        matches!(&refused, CommandError::CannotEncodeToken { name, .. } if name == "Readings"),
        "{refused:?}"
    );
}
