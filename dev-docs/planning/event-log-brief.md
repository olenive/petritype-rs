# Brief: the event log, for review in true order

The record (`record-brief.md`, built 2026-10-20 on `activity-record` and
`thumbnail-record`) bins what the model did per 100 ms. Inside a bin the
order of events is gone, by design. Oleg wants the opposite view too: the
events themselves, replayed one at a time, each shown for a fixed interval,
so an observer can read the order in which things happened rather than how
long they took (decided 2026-10-20). This brief adds that beside the record.
Spec: `activity-thumbnail-spec.md` §5; this brief adds a §5.6 at the end.

Two repositories, two branches, in this order, because the viewer builds
against the Rust checkout by path. Both start from the record branches,
which are not yet merged; they will be merged first.

1. `~/work/petritype-rs`, branch `activity-events` from `activity-record`:
   §1 to §4 below, in `petritype-activity`.
2. `~/work/petritype-bevy`, branch `thumbnail-events` from
   `thumbnail-record`: §5 to §7, built with `activity-events` checked out
   in petritype-rs.

## 1. The entry: what the model is fed

The core's `petritype::Event` cannot be stored and read back: it is
`Serialize` only, one variant holds a `&'static str`, and the core's stance
is that events are emitted, never read (`edge.rs` says so of its types).
So the activity crate gets its own entry, in a new `log.rs`:

- `Entry`, an enum of what the model acts on, ids only, `Clone`,
  `PartialEq`, `Debug`, with `serde` derives behind the feature tagged
  `event` in `snake_case` like the core's own JSON: `Fired { transition,
  consumed: Vec<(PlaceId, TokenId)>, produced }`, `Launched { transition,
  consumed }`, `Distributed { transition, produced }`, `Failed {
  transition, consumed, restored: bool, error: String }`, `Added { place,
  token }`, `Removed { place, token }`, `Mutated { place, token }`, and two
  the host adds when it corrects the model from a mirror's state: `Count {
  place, count: usize }` and `InFlight { transition, on: bool }`. Token ids
  are kept: they are what lets a reader follow a token through the log,
  and they are two words each.
- `Entry::from_event(&Event) -> Option<Entry>`: `None` for a variant the
  model ignores (`Toggled`, `ParamChanged`, `Resynced`, anything the
  `non_exhaustive` enum grows).
- `Activity::apply_entry(&mut self, &Entry)`. The rules exist once: either
  `apply` converts and calls `apply_entry`, or both go through the same
  private functions from one match. No second copy of the launch, flow or
  ember rules. `Count` calls `set_count`, `InFlight` calls `set_in_flight`.
- `Logged { seq: u64, at_ms: u64, entry: Entry }`: `seq` is the host's
  position, the same number the record's `mark` takes, so a bin's log range
  names entries; `at_ms` is the offset from the log's start on the wall
  clock, for the reader and the time text, never for the replay's pace.

## 2. The log

`EventLog` in `log.rs`, a ring:

- `EventLog::new(capacity: Option<usize>, start: Option<SystemTime>,
  base: Standing)`: `capacity` as the recorder's; `base` is the counts and
  in-flight transitions standing when the log began (§3).
- `push(&mut self, entry: Entry, seq: u64, at: Duration)`; past the
  capacity the oldest entry is dropped and the base advanced by it (§3).
- `dropped() -> u64`, `first() -> u64` (the absolute index of the oldest
  kept, which is `dropped`), `len`, `is_empty`, `capacity`, `get(index:
  u64) -> Option<&Logged>` by absolute index, `iter()`, `clear` (keeps the
  capacity; the base becomes the standing state the host passes), `base()`.
- `position_at(seq) -> Option<u64>`: the absolute index of the first kept
  entry with `seq >= position`, for the jump from a bin to its events.
- `start_unix_ms`, and `file_name(net) -> String`:
  `<net>-<YYYYMMDD-HHMMSS>.events.json`, the same stamp rule as the
  record's, so the two files of one save share a stem.
- With `serde`: `write_json(w)` and `read_json(r)`, one JSON value with the
  start, the base, `dropped` and the entries, pretty printing off.

Memory: an entry with two short token lists is on the order of 150 bytes,
so a hundred thousand is some 15 MB. Say that in the setting's doc.

## 3. The base and the player

A ring that has dropped entries must still replay from the counts that
stood at its oldest kept entry.

- `Standing { counts: Vec<(PlaceId, usize)>, in_flight: Vec<TransitionId>
  }`, `Default`, `Clone`, `PartialEq`, serde behind the feature.
  `Activity::standing_state() -> Standing` reads it off a model (the
  record's `standing()` already builds a `Delta` with the same facts;
  share the walk).
- `Standing::advance(&mut self, &Entry)`: the count rule only. `Fired`
  takes `consumed` and adds `produced`; `Launched` takes and marks the
  transition in flight; `Distributed` adds and clears it; `Failed` with the
  transition in flight adds `consumed` back when `restored` and nothing
  otherwise, and clears it; `Failed` with it not in flight takes
  `consumed` unless `restored`; `Added` adds one, `Removed` takes one,
  `Mutated` nothing; `Count` and `InFlight` set. This is counts, not glow,
  and the test in §4 pins it to the model.
- `EventPlayer::new(log: &EventLog, grid: &Grid, settings:
  ActivitySettings, dwell: Duration, checkpoint_every: usize)`: a model
  from the grid's cells (`Activity::named`) with the base's counts and
  flights set, then `at(&mut self, index: u64) -> &Activity` folds from the
  nearest checkpoint at or before `index`: for each entry from the start of
  the fold, `tick(dwell)` first unless it is the very first entry of the
  log, then `apply_entry`. So the entry shown is at full brightness and the
  ones before it have decayed by whole dwells, whatever the real gaps were.
  The firings clock is untouched by the dwell. Checkpoints every 256
  entries by default, built lazily, dropped when the settings or the dwell
  change (`set_settings`, `set_dwell`). `follow(&EventLog)` as the record's
  player follows a growing ring, with absolute indices.
- `len`, `first`, `settings`, `dwell`.

## 4. Crate tests

- `from_event` covers every variant the model acts on and returns `None`
  for the rest; a traffic-light run and a parcel-distribution run (with
  launches, distributes and a failure) applied through `apply` and through
  `from_event` plus `apply_entry` give equal models.
- `Standing::advance` over the same runs equals `standing_state()` of the
  live model after every entry, with and without a ring that drops.
- The ring drops past its capacity, `dropped` and `first` count it, and a
  player on the dropped ring at its first index shows the base's counts.
- `position_at` finds the first entry at or after a position, and `None`
  past the end.
- The player at any index equals a straight fold with the same dwell;
  decay between entries is exactly `dwell` on the wall layers and nothing
  on the firings clock.
- JSON round trip is equal; `file_name` for a known start and for `None`
  matches the record's stem.

## 5. The viewer: logging beside the recording

`RecordSettings` gains `events: usize` (100 000, how many entries the ring
keeps) and `dwell: Duration` (300 ms, how long the history shows each
entry). The one `on` flip governs the recorder and the log together.

`NetActivity` gains `log: Option<(u64, EventLog)>` beside the recorder,
started with the model's `standing_state()` as the base and the wall clock
at that moment as the start. In `feed`, each event the model applies is
pushed as an entry with `seq` equal to the position the recorder marks for
it (the count before the increment) and `at` from the frame's time, so a
bin's log range names its entries exactly. In `read_state`, a `Count` is
pushed only when `set_count` changes the model's count, and an `InFlight`
only when the state changes (the comparison is already there); a resync
that changes nothing logs nothing. A model rebuilt for a new epoch starts a
fresh log. Off drops it; a change to `events` clears it.

Saving: the Save record button writes the log beside the record, same
directory and stem, `.events.json`, when the log is on; the note shows the
record's path as now and the log line names both. The settings window
gains two rows under the record's: *Events kept* (a ratio slider, 1 000 to
1 000 000) and *Step* (a ratio slider, 50 ms to 5 s), with the memory
estimate in the doc comment. The window is near 25 rows already; add the
two and say in the hand-back how tall it is at 720 px.

## 6. The viewer: the history by event

`History` gains a mode, `HistoryMode::{Bins, Events}`, `Bins` by default,
flipped with `E` while the history is on (`ControlButton::HistoryMode`, a
key only, in the table beside `H`, `,` and `.`). The mode shows in the time
text: `bin 1234 · 0:12.3` and `event 5678 · 0:12.3`, the event's time its
`at_ms` from the log's start.

In `Events`:

- The position is an absolute entry index; the thumbnail shows the event
  player's fold at it; the scrub bar's track spans the kept entries;
  `,` and `.` step one entry; play advances one entry per `dwell` of wall
  time and stops at the newest. `replay_speed` does not apply in this mode;
  the dwell is the pace, and the README says so.
- Flipping `Bins` to `Events` maps the position through the bin's log
  range to its first entry (`position_at`), and `Events` to `Bins` to the
  bin whose range holds the entry's `seq`; when neither finds anything, the
  newest.
- The ring keeps dropping underneath; the position holds still and moves
  to the oldest kept when it falls off, as in `Bins`.
- The main drawing stays live, as decided for the record. Replaying the
  drawing by event is the next step and the panel says nothing about it.

## 7. Viewer tests

- Headless: a Repeat Step logs one entry per event the model took, its
  `seq` inside the bin's log range of the same frame; a count correction
  from the view logs a `Count` only when it changes the model.
- `E` while the history is on flips the mode, and the position maps from a
  bin to its first entry and back; off, `E` does nothing.
- In `Events`, `,` and `.` step one entry; play advances one entry per
  dwell and stops at the newest; the fold at the newest entry equals the
  live thumbnail's colours within one dwell on the wall layers.
- The ring drops past `events` and the position clamps to the oldest kept.
- Save writes both files under a temp directory, and the log reads back
  equal.
- The panel placement test still clears the bar and the legend with the
  bar shown.

## Rules

- Branch in the checkout, no worktrees. Never merge, push, tag or publish.
- Commit messages: short plain subjects, no attribution trailers of any
  kind (the user's convention in every repository).
- In petritype-bevy, never read, edit, stage or commit `TODO_TEMP.md`;
  stage files by name, never `git add -A` or `git add .`.
- Plain English in comments and docs, no em-dashes. Say what is true.
- No new dependencies. The core crate is not touched.
- Gates before hand-over: `just check` in each repository, green with no
  warnings; in the viewer with `activity-events` checked out in
  petritype-rs. The viewer's examples print a known `ld: __eh_frame`
  linker warning that is not a failure.
- If `grep` hangs on a pipe, use `/usr/bin/grep` or write to a file first.
- At the end: spec §5 gains §5.6 (the entry, the log, the base, the player
  by dwell, the files) and §10 a line; the crate's `lib.rs` overview
  gains the log in a sentence beside the record; the viewer's README key
  table and history paragraph and its `lib.rs` key table gain `E` and the
  two rows.

## Hand-over

Report: the branch names and heads, each part of §1 to §7 in one line with
the files touched, what the tests cover, the gates' last lines, the
settings window's height at 720 px, and anything seen that this brief did
not ask about, left alone. Under 50 lines.
