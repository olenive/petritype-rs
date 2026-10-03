//! Integration tests for the first slice: places, tokens, the read API and
//! external input.

use std::cell::RefCell;
use std::rc::Rc;

use petritype::{BuildError, Event, Net, NetBuilder, NetError, PlaceId, PlaceRef, TokenId};

/// A token type that is deliberately not `Clone`, as a file handle or a
/// socket would be.
#[derive(Debug)]
struct Connection {
    host: String,
}

/// Reads a `u32` place back as plain values, oldest first.
fn values(net: &Net, place: PlaceId) -> Vec<u32> {
    net.tokens(place)
        .map(|(_, value)| *value.downcast_ref::<u32>().expect("place holds u32"))
        .collect()
}

fn ids(net: &Net, place: PlaceId) -> Vec<TokenId> {
    net.tokens(place).map(|(id, _)| id).collect()
}

// Shorthands so the event assertions below read as one line each.
fn added(place: PlaceId, token: TokenId) -> Event {
    Event::Added { place, token }
}

fn removed(place: PlaceId, token: TokenId) -> Event {
    Event::Removed { place, token }
}

fn mutated(place: PlaceId, token: TokenId) -> Event {
    Event::Mutated { place, token }
}

#[test]
fn places_are_declared_and_read_back() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let outbox = b.place::<String>("Outbox");
    b.seed(inbox, [1, 2, 3]);
    let net = b.build().expect("distinct names");

    let places: Vec<_> = net.places().collect();
    assert_eq!(places.len(), 2);
    assert_eq!(places[0].0, inbox.id());
    assert_eq!(places[0].1.name, "Inbox");
    assert_eq!(places[0].1.type_name, "u32");
    assert_eq!(places[0].1.count, 3);
    assert_eq!(places[1].1.count, 0);

    assert_eq!(net.place_named("Outbox"), Some(outbox.id()));
    assert_eq!(net.place_named("Nowhere"), None);
    assert_eq!(net.count(inbox.id()), 3);
    assert_eq!(net.count(outbox.id()), 0);
    assert_eq!(net.label_limit(), 100);
    assert_eq!(net.topology_counter(), 0);
    assert_eq!(net.fired_count(), 0);
}

#[test]
fn a_place_ref_is_copy_and_usable_many_times() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let also_inbox = inbox; // a move would make the next line fail to compile
    b.seed(inbox, [1]);
    let mut net = b.build().unwrap();
    net.put(inbox, 2);
    net.put(also_inbox, 3);

    assert_eq!(inbox, also_inbox);
    assert_eq!(values(&net, inbox.id()), [1, 2, 3]);
}

/// `PlaceRef<T>` must be a plain handle whatever `T` is: the `fn() -> T`
/// phantom keeps the auto traits and the derives off `T`.
#[test]
fn a_place_ref_is_send_sync_copy_hash_for_an_unsendable_token_type() {
    fn assert_handle<H: Copy + Send + Sync + std::fmt::Debug + Eq + std::hash::Hash>() {}

    // `Rc<RefCell<u8>>` is none of `Send`, `Sync`, `Copy` or `Hash`.
    assert_handle::<PlaceRef<Rc<RefCell<u8>>>>();
}

/// The net must be able to live in a `Resource` and travel between threads
/// with its tokens, which is what the `Send + Sync` bound on every token type
/// buys.
#[test]
fn a_net_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<Net>();
}

#[test]
fn tokens_come_out_oldest_first() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    b.seed(inbox, [1, 2]);
    b.seed(inbox, [3]); // repeated seeds append
    let mut net = b.build().unwrap();
    net.extend(inbox, [4, 5]);

    assert_eq!(values(&net, inbox.id()), [1, 2, 3, 4, 5]);
    let order = ids(&net, inbox.id());
    let mut sorted = order.clone();
    sorted.sort();
    assert_eq!(order, sorted, "FIFO order is id order");
}

/// Seeding is setup, not change: the net a view attaches to has its marking
/// but no history, and the counters start ticking from there.
#[test]
fn seeding_is_setup_and_leaves_the_counters_at_zero() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let spare = b.place::<u32>("Spare");
    b.seed(inbox, [1, 2]);
    let mut net = b.build().unwrap();

    // The marking is there to be read with the read API.
    assert_eq!(values(&net, inbox.id()), [1, 2]);
    assert_eq!(net.count(spare.id()), 0);

    // Nothing has happened to the net yet.
    assert_eq!(net.state_counter(), 0);
    assert_eq!(net.topology_counter(), 0);
    assert_eq!(net.fired_count(), 0);
    assert_eq!(net.drain_events().count(), 0, "seeding logs nothing");

    // Seeded tokens still have real ids, in place order, and every later
    // token sorts after them.
    let seeded = ids(&net, inbox.id());
    let later = net.put(inbox, 3);
    assert!(seeded[0] < seeded[1] && seeded[1] < later);

    // The first change is the first event and the first tick.
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [added(inbox.id(), later)]
    );
    assert_eq!(net.state_counter(), 1);
}

#[test]
fn external_input_emits_events_and_bumps_the_state_counter() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let mut net = b.build().unwrap();
    assert_eq!(net.state_counter(), 0);

    // extend: one Added each, in iteration order.
    let pair = net.extend(inbox, [1, 2]);
    assert_eq!(net.state_counter(), 2);
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [added(inbox.id(), pair[0]), added(inbox.id(), pair[1]),]
    );

    // put: sugar for an extend of one.
    let third = net.put(inbox, 3);
    assert_eq!(net.state_counter(), 3);
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [added(inbox.id(), third)]
    );

    // replace: same id, same position, Mutated.
    net.replace(inbox, pair[0], 10).unwrap();
    assert_eq!(net.state_counter(), 4);
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [mutated(inbox.id(), pair[0])]
    );
    assert_eq!(values(&net, inbox.id()), [10, 2, 3]);
    assert_eq!(ids(&net, inbox.id())[0], pair[0]);

    // remove: Removed, and the queue closes up.
    net.remove(inbox.id(), pair[1]).unwrap();
    assert_eq!(net.state_counter(), 5);
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [removed(inbox.id(), pair[1])]
    );
    assert_eq!(values(&net, inbox.id()), [10, 3]);

    // set_tokens: every old token Removed, then every new one Added.
    let before = ids(&net, inbox.id());
    net.set_tokens(inbox, [7, 8]);
    let after = ids(&net, inbox.id());
    assert_eq!(net.state_counter(), 9, "2 removed + 2 added");
    assert_eq!(
        net.drain_events().collect::<Vec<_>>(),
        [
            removed(inbox.id(), before[0]),
            removed(inbox.id(), before[1]),
            added(inbox.id(), after[0]),
            added(inbox.id(), after[1]),
        ]
    );
    assert_eq!(values(&net, inbox.id()), [7, 8]);

    // set_tokens with nothing clears the place.
    net.set_tokens(inbox, std::iter::empty());
    assert_eq!(net.count(inbox.id()), 0);
    assert_eq!(net.state_counter(), 11);
}

#[test]
fn token_ids_are_monotonic_and_never_reused() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    b.seed(inbox, [1]);
    let mut net = b.build().unwrap();

    let mut seen: Vec<TokenId> = ids(&net, inbox.id());
    seen.extend(net.extend(inbox, [2, 3]));
    net.remove(inbox.id(), seen[1]).unwrap();
    net.set_tokens(inbox, [4]); // clears, then adds
    seen.extend(ids(&net, inbox.id()));
    seen.push(net.put(inbox, 5));

    let mut sorted = seen.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), seen.len(), "no id is ever handed out twice");
    assert_eq!(sorted, seen, "ids increase");

    // A retired id does not come back to life.
    assert!(matches!(
        net.remove(inbox.id(), seen[1]),
        Err(NetError::NoSuchToken { .. })
    ));
}

#[test]
fn labels_are_truncated_on_a_character_boundary() {
    let mut b = NetBuilder::new();
    let notes = b.place::<String>("Notes");
    b.label_limit(5);
    let mut net = b.build().unwrap();
    let token = net.put(notes, String::from("日本語のメモ"));

    // `Debug` of a `String` adds the quotes, so five characters is `"日本語`.
    let label = net.label(notes.id(), token).unwrap();
    assert_eq!(label, "\"日本語の...");
    assert_eq!(label.chars().count(), 5 + 3);
    assert_eq!(net.label_limit(), 5);

    // Nothing is cut when it already fits.
    let short = net.put(notes, String::from("ab"));
    assert_eq!(net.label(notes.id(), short).unwrap(), "\"ab\"");
}

#[test]
fn debug_is_the_text_a_label_is_cut_from() {
    let mut b = NetBuilder::new();
    let notes = b.place::<String>("Notes");
    b.label_limit(5);
    let mut net = b.build().unwrap();
    let token = net.put(notes, String::from("日本語のメモ"));

    assert_eq!(net.debug(notes.id(), token).unwrap(), "\"日本語のメモ\"");
    assert_eq!(net.label(notes.id(), token).unwrap(), "\"日本語の...");
    assert!(net.debug(notes.id(), token).unwrap().chars().count() > net.label_limit());
}

#[test]
fn typed_reads_reject_the_wrong_type() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let mut net = b.build().unwrap();
    let token = net.put(inbox, 7);

    assert_eq!(net.token::<u32>(inbox.id(), token), Some(&7));
    assert_eq!(net.token::<i64>(inbox.id(), token), None);
    assert_eq!(net.token::<u64>(inbox.id(), token), None);
}

#[test]
fn missing_places_and_tokens_are_reported_not_guessed() {
    let mut b = NetBuilder::new();
    let inbox = b.place::<u32>("Inbox");
    let mut net = b.build().unwrap();
    let token = net.put(inbox, 1);

    // Handles from another net: one would index a place of this net, the
    // other indexes nothing at all. The stamp catches both, so neither can
    // address a place here by accident.
    let mut other = NetBuilder::new();
    let same_index = other.place::<String>("Inbox");
    let out_of_range = other.place::<String>("Spare");
    assert_eq!(same_index.id(), inbox.id(), "same index, different net");

    assert!(matches!(
        net.remove(out_of_range.id(), token),
        Err(NetError::NoSuchPlace { .. })
    ));
    assert!(matches!(
        net.replace(out_of_range, token, String::new()),
        Err(NetError::NoSuchPlace { .. })
    ));
    assert!(matches!(
        net.replace(same_index, token, String::new()),
        Err(NetError::NoSuchPlace { .. })
    ));

    // Removing the same token twice names the token, not the place.
    net.remove(inbox.id(), token).unwrap();
    assert!(matches!(
        net.remove(inbox.id(), token),
        Err(NetError::NoSuchToken { .. })
    ));
    assert!(matches!(
        net.replace(inbox, token, 2),
        Err(NetError::NoSuchToken { .. })
    ));

    // Reads of what is not there are empty, not panics.
    assert_eq!(net.label(out_of_range.id(), token), None);
    assert_eq!(net.token::<u32>(out_of_range.id(), token), None);
    assert_eq!(net.count(out_of_range.id()), 0);
    assert_eq!(net.tokens(out_of_range.id()).count(), 0);

    // Only the one real change was logged.
    let events: Vec<_> = net.drain_events().collect();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1], removed(inbox.id(), token));
}

#[test]
fn a_place_can_hold_tokens_that_cannot_be_cloned() {
    let mut b = NetBuilder::new();
    let connections = b.place_no_clone::<Connection>("Connections");
    b.seed(connections, [Connection { host: "a".into() }]);
    let mut net = b.build().unwrap();
    let token = net.put(connections, Connection { host: "b".into() });

    assert_eq!(net.count(connections.id()), 2);
    assert_eq!(
        net.token::<Connection>(connections.id(), token)
            .unwrap()
            .host,
        "b"
    );
    assert_eq!(
        net.label(connections.id(), token).unwrap(),
        "Connection { host: \"b\" }"
    );
}

#[test]
fn duplicate_place_names_are_a_build_error() {
    let mut b = NetBuilder::new();
    let _ = b.place::<u32>("Inbox");
    let _ = b.place_no_clone::<Connection>("Inbox");

    match b.build() {
        Err(BuildError::DuplicatePlaceName { name }) => assert_eq!(name, "Inbox"),
        other => panic!("expected a duplicate name error, got {other:?}"),
    }
}

#[cfg(feature = "serde")]
#[test]
fn json_is_available_only_for_places_marked_serializable() {
    let mut b = NetBuilder::new();
    let counts = b.place::<u32>("Counts");
    let notes = b.place::<String>("Notes");
    b.serializable(counts);
    let mut net = b.build().unwrap();

    let counted = net.put(counts, 7);
    let noted = net.put(notes, String::from("hi"));

    assert_eq!(net.json(counts.id(), counted).unwrap().to_string(), "7");
    assert_eq!(net.json(notes.id(), noted), None, "not marked serializable");
    assert_eq!(net.json(counts.id(), noted), None, "no such token here");
}

#[test]
fn ids_can_be_made_from_their_numbers_and_parsed_from_their_text() {
    use petritype::{PlaceId, TokenId, TransitionId};

    let mut b = NetBuilder::new();
    let notes = b.place::<String>("Notes");
    let mut net = b.build().unwrap();
    let token = net.put(notes, String::from("a"));

    // The first place is index 0; the number round-trips through Display.
    assert_eq!(PlaceId::new(0), notes.id());
    assert_eq!(PlaceId::new(0).index(), 0);
    assert_eq!(
        notes.id().to_string().parse::<PlaceId>().unwrap(),
        notes.id()
    );
    assert_eq!(TransitionId::new(3).index(), 3);
    assert_eq!("3".parse::<TransitionId>().unwrap(), TransitionId::new(3));
    assert_eq!(TokenId::new(token.get()), token);
    assert_eq!(token.to_string().parse::<TokenId>().unwrap(), token);
    assert!("x".parse::<TokenId>().is_err());

    // The topology carries the same numbers.
    assert_eq!(net.topology().places[0].id, PlaceId::new(0));

    // An id the net never issued is answered with nothing, not a panic.
    assert_eq!(net.label(PlaceId::new(99), token), None);
    assert_eq!(net.label(notes.id(), TokenId::new(99)), None);
    assert!(!net.is_enabled(TransitionId::new(99)));
}
