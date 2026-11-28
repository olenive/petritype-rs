//! A key extractor that panics leaves a place and its key index agreeing
//! (since 2026-11-28): each writer reads a token's keys, old and new,
//! before the token moves or changes, so a panic comes before anything does
//! and the token is where it was, indexed as it was. A query asks the
//! index, so it answers with what the place holds.
//!
//! An extractor should not panic (`NetBuilder::keys`): what a firing does
//! when one does is not these tests' subject, only that the marking and the
//! index stay one.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};

use petritype::{Filter, Net, NetBuilder, PlaceRef, TakeEdge, TokenId};

/// A place of names keyed by the name, whose extractor panics on the name
/// in `fragile` while there is one, and a transition that takes a name out
/// of it.
struct Fixture {
    net: Net,
    names: PlaceRef<String>,
    fragile: Arc<Mutex<Option<&'static str>>>,
}

fn fixture() -> Fixture {
    let fragile: Arc<Mutex<Option<&'static str>>> = Arc::default();
    let mut b = NetBuilder::new();
    let names = b.place::<String>("Names");
    let gone = b.place::<String>("Gone");
    b.port(names);
    let breaks = Arc::clone(&fragile);
    b.keys(names, ["name"], move |name: &String| {
        let fragile = *breaks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(
            fragile != Some(name.as_str()),
            "the extractor broke on {name}"
        );
        [name.as_str().into()]
    });
    b.seed(names, ["Ada".to_owned(), "Bram".to_owned()]);
    b.transition("Leave", (TakeEdge(names),), (gone,), |name: String| (name,));
    Fixture {
        net: b.build().expect("a keyed place"),
        names,
        fragile,
    }
}

/// The tokens the place holds, by id, with their names.
fn held(net: &Net, names: PlaceRef<String>) -> Vec<(TokenId, String)> {
    net.tokens(names.id())
        .map(|(id, _)| (id, net.token::<String>(names.id(), id).unwrap().clone()))
        .collect()
}

/// The token the index names for `name`, if any.
fn indexed(net: &Net, name: &str) -> Vec<TokenId> {
    net.query(&Filter::new().key_in("name", [name]))
        .expect("every place exists")
        .into_iter()
        .map(|found| found.token)
        .collect()
}

/// The place and the index say the same: each token held is found by its
/// name, and nothing else is found.
fn agree(f: &Fixture, names: &[&str]) {
    let held = held(&f.net, f.names);
    assert_eq!(
        held.iter()
            .map(|(_, name)| name.as_str())
            .collect::<Vec<_>>(),
        names
    );
    for (token, name) in &held {
        assert_eq!(indexed(&f.net, name), [*token], "{name} indexed");
    }
}

/// Runs `act` while the extractor panics on `name`.
fn broken<R>(f: &mut Fixture, name: &'static str, act: impl FnOnce(&mut Net) -> R) {
    *f.fragile.lock().unwrap() = Some(name);
    let outcome = catch_unwind(AssertUnwindSafe(|| act(&mut f.net)));
    *f.fragile
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    assert!(outcome.is_err(), "the extractor panicked");
}

#[test]
fn a_put_whose_keys_panic_adds_nothing() {
    let mut f = fixture();
    let names = f.names;
    broken(&mut f, "Cato", |net| net.put(names, "Cato".to_owned()));
    agree(&f, &["Ada", "Bram"]);
    assert!(indexed(&f.net, "Cato").is_empty());
}

#[test]
fn a_replace_whose_new_keys_panic_keeps_the_old_token_indexed() {
    let mut f = fixture();
    let (ada, _) = held(&f.net, f.names)[0].clone();
    let names = f.names;
    broken(&mut f, "Cato", |net| {
        net.replace(names, ada, "Cato".to_owned())
    });
    agree(&f, &["Ada", "Bram"]);
}

#[test]
fn a_remove_whose_keys_panic_leaves_the_token_where_it_was() {
    let mut f = fixture();
    let (bram, _) = held(&f.net, f.names)[1].clone();
    let names = f.names.id();
    broken(&mut f, "Bram", |net| net.remove(names, bram));
    agree(&f, &["Ada", "Bram"]);
}

#[test]
fn a_set_tokens_whose_keys_panic_on_the_way_out_clears_nothing() {
    let mut f = fixture();
    let names = f.names;
    broken(&mut f, "Bram", |net| {
        net.set_tokens(names, Vec::<String>::new())
    });
    agree(&f, &["Ada", "Bram"]);
}

#[test]
fn a_take_whose_keys_panic_leaves_the_token_in_its_place() {
    let mut f = fixture();
    broken(&mut f, "Ada", |net| net.step());
    agree(&f, &["Ada", "Bram"]);
}
