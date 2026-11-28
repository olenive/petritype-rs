//! What players, recorders, rosters and the model do at their edges (since
//! 2026-11-28): a player resyncs after the recorder or the log it follows
//! is cleared, a frame of any length neither overflows a recorder nor a
//! player following it, a fold keeps no account, a level that is not a
//! number does not light a node, and a roster keeps a look only for a token
//! it names.

use std::time::Duration;

use petritype::{Net, NetBuilder, PlaceId, TakeEdge, TokenId, TransitionId};
use petritype_activity::{
    Activity, ActivitySettings, Delta, Entry, EventLog, EventPlayer, GlowMode, Grid, Logged, Look,
    PlaceDelta, Player, Recorder, Roster, Standing,
};
use petritype_layout::Layout;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// `In -> Go -> Out`, `In` holding `seed`: `In` is place 0, `Out` place
/// 1, `Go` transition 0.
fn net(seed: &[u32]) -> Net {
    let mut b = NetBuilder::new();
    let input = b.place::<u32>("In");
    let out = b.place::<u32>("Out");
    b.seed(input, seed.to_vec());
    b.transition("Go", (TakeEdge(input),), (out,), |n: u32| (n,));
    b.build().expect("the net builds")
}

const IN: PlaceId = PlaceId::new(0);
const GO: TransitionId = TransitionId::new(0);

fn grid_of(net: &Net) -> Grid {
    let mut layout = Layout::default();
    layout.compute(net);
    Grid::from_layout(&layout, &net.topology())
}

/// A delta that sets `In`'s count to `count`.
fn counted(count: usize) -> Delta {
    let mut delta = Delta::default();
    delta.places.insert(
        IN,
        PlaceDelta {
            count,
            ..PlaceDelta::default()
        },
    );
    delta
}

/// The standing state of `In` holding `count` tokens, read off a model.
fn standing(net: &Net, count: usize) -> Standing {
    let mut model = Activity::new(&net.topology(), ActivitySettings::default());
    model.set_count(IN, count);
    model.standing_state()
}

fn added(seq: u64) -> Entry {
    Entry::Added {
        place: IN,
        token: TokenId::new(seq),
    }
}

#[test]
fn a_player_follows_a_recorder_cleared_before_it_dropped_a_bin() {
    let net = net(&[]);
    let grid = grid_of(&net);
    let settings = ActivitySettings::default();
    let mut recorder = Recorder::new(ms(100), None, None);
    for count in 1..=3 {
        recorder.observe(counted(count), ms(100));
    }
    let mut player = Player::following(&recorder, &grid, settings, 2);
    assert_eq!(player.at(2).place(IN).map(|p| p.count), Some(3));
    // Cleared, and one bin since: nothing was ever dropped, so the
    // recorder's first bin is at 0 as the player's is.
    recorder.clear();
    recorder.observe(counted(7), ms(100));
    player.follow(&recorder);
    assert_eq!((player.first(), player.len()), (0, 1));
    assert_eq!(player.record().bins, recorder.record(&grid, None).bins);
    assert_eq!(player.at(0).place(IN).map(|p| p.count), Some(7));
}

#[test]
fn an_event_player_follows_a_log_cleared_with_nothing_kept() {
    let net = net(&[]);
    let grid = grid_of(&net);
    let mut log = EventLog::new(None, None, Standing::default());
    let mut player = EventPlayer::new(&log, &grid, ActivitySettings::default(), ms(100), 4);
    assert_eq!(player.before_first().place(IN).map(|p| p.count), Some(0));
    log.clear(standing(&net, 5));
    player.follow(&log);
    assert_eq!(
        player.before_first().place(IN).map(|p| p.count),
        Some(5),
        "the base the clear set"
    );
}

#[test]
fn an_event_player_starts_again_on_the_base_a_clear_sets() {
    let net = net(&[]);
    let grid = grid_of(&net);
    let mut log = EventLog::new(None, None, Standing::default());
    for seq in 0..3 {
        log.push(added(seq), seq, ms(seq));
    }
    let mut player = EventPlayer::new(&log, &grid, ActivitySettings::default(), ms(100), 4);
    assert_eq!(player.at(2).place(IN).map(|p| p.count), Some(3));
    // The host reads the net again, and nine stand in `In`.
    log.clear(standing(&net, 9));
    log.push(
        Entry::Mutated {
            place: IN,
            token: TokenId::new(0),
        },
        3,
        ms(3),
    );
    player.follow(&log);
    assert_eq!((player.first(), player.len()), (3, 1));
    assert_eq!(
        player.at(3).place(IN).map(|p| p.count),
        Some(9),
        "folded from the new base, not from where the player stood"
    );
    assert_eq!(player.tokens_at(3), log.base_tokens());
}

#[test]
fn a_frame_of_any_length_overflows_neither_a_ring_nor_a_player_following_it() {
    let net = net(&[]);
    let grid = grid_of(&net);
    let mut recorder = Recorder::new(ms(100), Some(2), None);
    recorder.observe(counted(4), ms(50));
    recorder.observe(Delta::default(), Duration::MAX);
    assert_eq!(recorder.len(), 2);
    assert_eq!(
        recorder.dropped(),
        u64::MAX - 1,
        "every bin it could not keep"
    );
    let mut player = Player::following(&recorder, &grid, ActivitySettings::default(), 1);
    assert_eq!((player.first(), player.len()), (u64::MAX - 1, 2));
    for index in [0, 1, 5, usize::MAX] {
        assert_eq!(
            player.at(index).place(IN).map(|p| p.count),
            Some(4),
            "the count carried into the oldest kept bin, at {index}"
        );
    }
    // On past the last index a `u64` holds: the bins dropped stop there.
    recorder.observe(Delta::default(), ms(250));
    assert_eq!(recorder.dropped(), u64::MAX);
    player.follow(&recorder);
    assert_eq!(player.first(), u64::MAX);
    assert_eq!(player.at(3).place(IN).map(|p| p.count), Some(4));
}

#[test]
fn a_recorder_that_keeps_every_bin_closes_at_most_so_many_empty_ones_for_a_frame() {
    let mut recorder = Recorder::new(ms(1), None, None);
    recorder.observe(counted(1), ms(Recorder::MAX_EMPTY_BINS + 10));
    assert_eq!(
        recorder.len() as u64,
        1 + Recorder::MAX_EMPTY_BINS,
        "the open bin and the empty ones"
    );
    assert_eq!(recorder.dropped(), 0);
    // A frame of every duration there is, part of a bin in: the widest
    // bins there are, a thousand of them.
    let mut wide = Recorder::new(Duration::MAX, None, None);
    wide.observe(counted(1), ms(1));
    wide.observe(Delta::default(), Duration::MAX);
    assert_eq!(wide.len(), 1000);
}

#[test]
fn a_fold_keeps_no_account() {
    let mut net = net(&[1]);
    let grid = grid_of(&net);
    let settings = ActivitySettings::default();
    let mut live = Activity::from_net(&net, settings.clone());
    let mut recorder = Recorder::new(ms(100), None, None);
    let mut log = EventLog::new(None, None, live.standing_state());
    net.step().expect("Go fires");
    for (seq, event) in net.drain_events().enumerate() {
        live.apply(&event);
        if let Some(entry) = Entry::from_event(&event) {
            log.push(entry, seq as u64, ms(seq as u64));
        }
    }
    recorder.observe(live.take_delta(), ms(100));
    let record = recorder.record(&grid, None);

    let mut replay = record.replay(settings.clone());
    let replayed = replay.next_bin().expect("a bin").clone();
    assert!(replayed.firings() > 0, "the bin was folded");
    assert!(replayed.clone().take_delta().is_empty());
    let mut player = Player::new(&record, settings.clone(), 1);
    assert!(player.at(0).clone().take_delta().is_empty());
    let mut events = EventPlayer::new(&log, &grid, settings.clone(), ms(10), 1);
    assert!(events.at(0).firings() > 0);
    assert!(events.at(0).clone().take_delta().is_empty());
    assert!(events.before_first().clone().take_delta().is_empty());
    // A fresh model of the record keeps one, for a caller to record.
    let mut fresh = record.model(settings);
    fresh.fired(GO);
    assert!(!fresh.take_delta().is_empty());
}

#[test]
fn a_level_that_is_not_a_number_lights_nothing() {
    for mode in [GlowMode::Peak, GlowMode::Sum] {
        assert_eq!(mode.combine(f32::NAN, f32::NAN), 0.0, "{mode:?}");
        assert_eq!(mode.combine(f32::NAN, 0.25), 0.25, "{mode:?}");
        assert_eq!(mode.combine(0.5, f32::NAN), 0.5, "{mode:?}");
    }
}

fn look(label: &str) -> Look {
    Look::new(label, None, 200)
}

fn logged(entry: Entry) -> Logged {
    Logged {
        seq: 0,
        at_ms: 0,
        entry,
        looks: Vec::new(),
    }
}

fn t(n: u64) -> TokenId {
    TokenId::new(n)
}

/// `In` naming tokens 1 and 2, each with a look.
fn roster() -> Roster {
    let mut roster = Roster::default();
    roster.set_place(IN, [(t(1), Some(look("one"))), (t(2), Some(look("two")))]);
    roster
}

#[test]
fn a_second_launch_with_no_landing_forgets_the_look_of_what_the_first_took() {
    let mut roster = roster();
    let launch = |token| {
        logged(Entry::Launched {
            transition: GO,
            consumed: vec![(IN, token)],
        })
    };
    roster.advance(&launch(t(1)));
    // As a host that missed the landing logs it.
    roster.advance(&launch(t(2)));
    assert_eq!(roster.look(t(1)), None, "named nowhere now");
    assert_eq!(roster.look(t(2)), Some(&look("two")), "in flight");
}

#[test]
fn a_failure_that_names_none_of_what_its_launch_took_forgets_their_looks() {
    let mut roster = roster();
    roster.advance(&logged(Entry::Launched {
        transition: GO,
        consumed: vec![(IN, t(1))],
    }));
    roster.advance(&logged(Entry::Failed {
        transition: GO,
        consumed: Vec::new(),
        restored: false,
        error: "lost".to_owned(),
    }));
    assert_eq!(roster.look(t(1)), None);
    assert_eq!(roster.tokens(IN), [t(2)]);
    assert_eq!(roster.look(t(2)), Some(&look("two")));
}

#[test]
fn a_roster_worked_back_past_a_launch_before_its_entries_keeps_no_look_of_it() {
    let mut roster = roster();
    roster.advance(&logged(Entry::Launched {
        transition: GO,
        consumed: vec![(IN, t(1))],
    }));
    let back = roster.worked_back(&[added(6)]);
    assert_eq!(back.look(t(1)), None, "what the launch took is not known");
    assert_eq!(back.tokens(IN), [t(2)]);
    assert_eq!(back.look(t(2)), Some(&look("two")));
}
