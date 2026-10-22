# Brief: step 4, the activity record

The record is the activity thumbnail's memory: what every cell did, in
time bins, so a run can be replayed, scrubbed, saved, coarsened and laid
out as a tensor. Spec: `activity-thumbnail-spec.md` in this folder, §5
(the record), §7 (tests) and §8 (order of work); §2 for the model it
replays. This brief turns §5 into work against the crate as built after
the 2026-10-20 tuning round, which changed two things §5 did not know:
the ember counts events rather than tokens, and a launch's flow waits
for its distribute. Where this brief and §5 differ, this brief wins and
§5 is updated at the end.

Two repositories, two branches, in this order, because the viewer builds
against the Rust checkout by path:

1. `~/work/petritype-rs`, branch `activity-record` from `main`: the
   crate work of §1 to §5 below, in `petritype-activity`, plus the small
   `serde` feature on `petritype-layout` of §4.
2. `~/work/petritype-bevy`, branch `thumbnail-record` from `main`: the
   viewer work of §6 to §8, built with `activity-record` checked out in
   petritype-rs.

## 1. The delta: what the model did

The record must replay exactly what the model applied, and the model's
rules (one ember hit per event, a pulse hit per token, the launch's flow
held to the distribute, the distribute's half hit, the staleness lift)
must not be written a second time in a recorder. So the model keeps a
running account of what it applied, and the recorder bins that account.

`Delta`, a plain data struct in `model.rs` (or a `delta.rs`), `Default`,
`Clone`, `PartialEq`, with `serde` derives behind the feature:

- `firings: u32`: fires and launches seen, the firings clock's advance.
- `lifted: bool`: whether anything lifted the staleness dim (a fire,
  launch, distribute or failure).
- per place touched, keyed by `PlaceId`: `pulse_hits` (tokens moved,
  mutations included), `ember_hits` (events that touched it), `balance`
  (the signed token count the flow took, as applied, so a launch's take
  shows here only when its distribute lands), `count` (the standing
  count at the end of the delta, also set by `set_count`).
- per transition touched or in flight, keyed by `TransitionId`: `fires`
  (fire and launch hits, full), `distributes` (half hits), `failures`,
  `in_flight_ms` (accumulated by `tick` while in flight), `in_flight`
  (the state at the end).

Use `Vec<(id, entry)>` or `BTreeMap` for the per-node parts; small and
ordered, so JSON is stable.

`Activity::take_delta(&mut self) -> Delta` hands the account over and
starts a fresh one. `Activity::apply_delta(&mut self, &Delta)` replays
one: the firings clock decays by `firings` first (as live, where each
firing decays everyone before its own hit), then the hits by the current
settings' jumps and distribute fraction, the failure levels set to 1,
the balances by the current flow gain, the counts set, the in-flight
states set, the dim lifted when `lifted`. Wall-time decay is not in the
delta; the replayer calls `tick(bin width)` after each bin.

The order inside a bin is lost, by design. Document the consequence: a
replayed level at a bin boundary is within one bin's decay of the live
one on the wall clock, and equal on the firings clock when a bin holds
at most one firing.

## 2. The recorder

`Recorder` in `record.rs`:

- `Recorder::new(bin_width: Duration, capacity: Option<usize>, start:
  Option<SystemTime>)`. `capacity` is the ring: `None` keeps everything,
  `Some(n)` drops the oldest bin past `n`. `start` is for file names and
  the record's `start_unix_ms`; tests pass `None`.
- `observe(&mut self, delta: Delta, elapsed: Duration)`: merges the
  delta into the open bin and advances the clock by `elapsed`; when the
  clock crosses a bin boundary the bin closes and a new one opens, and a
  long frame closes several, the intermediate ones empty. An empty bin
  is a `Delta::default()` and costs almost nothing.
- `mark(&mut self, position: u64)`: the open bin's event-log range takes
  the position (first and last). The host says what a position is: the
  running count of events applied in process, the stream's sequence
  number over the wire.
- `dropped(&self) -> u64`: how many bins the ring has dropped, so a bin's
  absolute index is `dropped + i`.
- `record(&self, grid: &Grid, settings: Option<&ActivitySettings>) ->
  Record`: a snapshot of the ring, the grid cloned in, the start moved
  forward by the dropped bins.
- `clear`, `len`, `is_empty`, `bin_width`, `capacity`. Changing the width
  or the capacity means `clear`, so a record is always uniform; the host
  does that.

Merging a delta into a bin is the same addition `coarsen` uses (§3):
counts add, balances add, `in_flight_ms` adds, `count` and `in_flight`
take the later value, `lifted` ors, `firings` adds. Write it once as
`Delta::absorb(&mut self, &Delta)`.

## 3. The record, replay and coarsening

`Record`, plain data: `grid: Grid`, `bin_width_ms: u64`, `start_unix_ms:
Option<u64>`, `settings: Option<ActivitySettings>` (what was in force
when recorded, for the reader; replay takes its own), `bins: Vec<Bin>`
where `Bin { delta: Delta, log: Option<(u64, u64)> }`.

- `replay(&self, settings: ActivitySettings) -> Replay`: an iterator
  over the bins yielding `&Activity` after each, owning an `Activity`
  built with `Activity::named` from the grid's cells (the grid knows the
  nodes and names; the model needs nothing else).
- `Player::new(record: &Record, settings, checkpoint_every: usize)`: the
  pure scrubber. `at(&mut self, index) -> &Activity` folds from the
  nearest checkpoint at or before `index`, so a scrub bar at any point
  costs at most `checkpoint_every` bins; checkpoints are built lazily on
  the first pass and dropped when the settings change. Default every 64.
  The viewer's player is a thin skin over this.
- `coarsen(&self, factor: usize) -> Record`: consecutive groups of
  `factor` bins absorbed into one, the log ranges unioned, a short last
  group kept as it is; the width multiplies. Exact by construction.
- `len`, `is_empty`, `duration`.

## 4. Files and the tensor

With the `serde` feature: `Record`, `Bin`, `Delta`, `Grid` and `Cell`
derive `Serialize` and `Deserialize`. `Cell` holds a
`petritype_layout::LayoutNode`, so `petritype-layout` gains a `serde`
feature deriving on `LayoutNode` and `Direction`, and
`petritype-activity`'s `serde` feature turns on `petritype/serde` and
`petritype-layout/serde`. `PlaceId` and `TransitionId` already
serialise as bare numbers under the core's feature.

- `Record::write_json(&self, w: impl Write)` and
  `Record::read_json(r: impl Read)` through `serde_json`, pretty
  printing off (a record is large).
- `Record::file_name(&self, net: &str) -> String`:
  `<net>-<start>.record.json`, the start as `YYYYMMDD-HHMMSS` UTC from
  `start_unix_ms`, or `undated`. Format the date by hand from the
  milliseconds; no new dependency for it.

`Record::tensor(&self) -> Tensor { shape: [usize; 4], data: Vec<f32> }`,
time by rows by columns by channels, row-major, the channels in this
order and documented on the type: fires, distributes, failures, the
in-flight fraction of the bin, pulse hits, ember hits, balance, count.
Empty cells are zero. `Tensor::write_npy(&self, w: impl Write)`: the
`.npy` version 1.0 form, the magic `\x93NUMPY`, major 1 minor 0, a
header `{'descr': '<f4', 'fortran_order': False, 'shape': (t, r, c, 8), }`
padded with spaces so that the whole preamble is a multiple of 64 bytes
and ends in a newline, then the data as little-endian `f32`. No
dependency; the format is a page long.

## 5. Crate tests

- A traffic-light run stepped under both clocks, recorded with one
  recorder beside the live model: at every bin boundary the replayed
  levels match the live ones within one bin's wall decay, and exactly
  on the firings clock when each bin holds one firing at most.
- `take_delta` after a launch and its distribute shows the place's
  balance only in the distribute's delta; after a mutate, a pulse hit and
  an ember hit and no balance; a long `tick` while in flight accrues
  `in_flight_ms`.
- The ring drops the oldest bin past its capacity and `dropped` counts
  it; a long frame closes several bins.
- `coarsen(3)` on nine bins gives three whose deltas are the sums, and
  the fold of the coarsened record equals the fold of the original at
  the coarse boundaries on the firings clock.
- JSON round-trip of a record with a clustered grid is equal.
- The tensor's shape is `[bins, rows, columns, 8]`; a cell's channels
  carry its counts; the `.npy` preamble is a multiple of 64 bytes, starts
  with the magic and parses as the documented header; the data length is
  the product of the shape times four.
- `file_name` for a known start and for `None`.

## 6. The viewer: recording by default

`RecordSettings`, a viewer resource (not part of `ActivitySettings`,
which is the model's): `on: bool` (true), `bin: Duration` (100 ms),
`length: Duration` (10 min), `directory: PathBuf` (`records`, relative
to the working directory), `replay_speed: f32` (1.0, bins per bin width
of wall time). Capacity is `length / bin`.

Each `NetActivity` carries `Option<Recorder>`, present while recording
is on. One system after the feed and the tick, in the thumbnail's
system set: `recorder.observe(activity.take_delta(), time.delta())`,
and `mark` with the running count of events the model applied, which
`feed` counts. If the remote mirror's stream cursor is reachable from
there without widening the change, mark with it instead over the wire;
if not, say so in the hand-back and leave the count. A model rebuilt
for a new epoch starts a fresh recorder. Turning recording off drops
the recorders; changing the bin or the length clears them, which the
settings window says in the row's name or a note.

The settings window gains rows for `on` (a flip, "Record"), `bin` (a
ratio slider, 10 ms to 10 s), `length` (a ratio slider, 10 s to 1 h),
`replay_speed` (a ratio slider, 0.1 to 100), and a button row "Save
record", which writes the canvas's record as JSON under `directory`
(creating it) with `file_name` of the net's name, logs the path with
`info!`, and shows it beside the button for a few seconds if that is
cheap in the window's row machinery. The rows table is typed over
`ActivitySettings` today; a second small table over `RecordSettings`,
or a row variant carrying its own getter and setter, is the worker's
call. The window's existing test that every field has a row extends to
the new struct.

## 7. The viewer: the player

A replay mode for the canvas's thumbnail, toggled with `H` (history), in
the bar's key table like `M` and `G`. On:

- The panel's thumbnail shows the player's fold at the scrub position
  instead of the live model. Recording continues underneath, so the
  range keeps growing and the ring keeps dropping; the scrub position is
  an absolute bin index, so it holds still while bins drop behind it,
  and clamps to the oldest kept when it falls off.
- A scrub bar appears under the thumbnail inside the panel, the panel's
  width, 10 px high, draggable like the window's sliders, with the bin's
  time offset from the record's start as text beside it (`m:ss.t`).
- A play/pause button at the bar's left; playing advances
  `replay_speed` bins per bin width of wall time and stops at the
  newest bin. `,` and `.` step one bin back and forward while the mode
  is on.
- The main drawing stays live in this slice; replaying it too is a
  later round, and the panel says nothing about it.
- The panel's floor grows by the bar's height while the mode is on. The
  layout-level headless test checks the panel with the bar shown clears
  the bar and the legend.

Off: the live thumbnail returns, the bar and button hide with
`Display::None`, the panel shrinks back.

## 8. Viewer tests

- Headless: a step under Repeat Step adds a bin whose delta counts the
  fired transition and the places it touched; after `length / bin` plus
  one bins the oldest is gone.
- The flip off drops the recorders; the bin slider clears them.
- Save writes a file whose name follows `file_name` under a temp
  directory set through `RecordSettings::directory`, and the file reads
  back as the same record.
- `H` shows the bar and button, the fold at the last bin equals the live
  thumbnail's colours at that moment within the one-bin tolerance, `,`
  moves the position back by one, `H` again hides everything.
- The panel placement test with the bar shown.

## Rules

- Branch in the checkout, no worktrees. Never merge, push, tag or publish.
- Commit messages: short plain subjects, no attribution trailers of any
  kind (the user's convention in every repository).
- In petritype-bevy, never read, edit, stage or commit `TODO_TEMP.md`;
  stage files by name, never `git add -A` or `git add .`.
- Plain English in comments and docs, no em-dashes. Say what is true.
- No new dependencies beyond what the crates already have; the `.npy`
  writer and the date formatting are written by hand.
- Gates before hand-over: `just check` in each repository, green with no
  warnings; in the viewer with `activity-record` checked out in
  petritype-rs.
- If `grep` hangs on a pipe, use `/usr/bin/grep` or write to a file first.
- At the end, update spec §5 to the delta design and the eight channels,
  the crate's `lib.rs` overview with the record in one paragraph, the
  viewer's README and `lib.rs` key table with `H`, `,` and `.`, and the
  settings window's row list where it is documented.

## Hand-over

Report: the branch names and heads, each part of §1 to §8 in one line
with the files touched, what the tests cover, the gates' last lines, and
anything seen that this brief did not ask about, left alone. Under 50
lines.
