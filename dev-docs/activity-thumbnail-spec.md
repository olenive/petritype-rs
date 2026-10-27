# Spec: the activity thumbnail, and pulse-and-decay in the viewer

Written 2026-10-18 from the discussion of 17 and 18 October. Two new crates
in the petritype-rs workspace, one of them the viewer's layout moved out,
and a round in petritype-bevy. The core crate is untouched. Nothing here
touches the Python runtime or the wire protocol.

## 0. What is already true

- The viewer's layout (`petritype-bevy/src/layout.rs`) gives every place
  and transition a rank, an order within the rank and a position. Clusters
  by name prefix are each laid out alone and packed in rows; hub places can
  be ghosted; a filtered view is laid out as a subgraph. The algorithm
  depends on the core crate and on Bevy's `Vec2` and `Rect` only, and its
  module doc already promises that the mirror asks it for nothing but a
  position.
- The core's event log says what a firing touched: `Fired` carries the
  tokens consumed and produced with their places; `Launched` the consumed,
  `Distributed` the produced; `Failed` the consumed and whether they went
  back; `Added`, `Removed` and `Mutated` name one place. A staged firing
  is in flight between its `Launched` and its `Distributed` or `Failed`.
- The wire's `fired`, `launched`, `distributed` and `failed` notifications
  carry the same consumed and produced lists, so a remote net can feed the
  same model as a net in process.
- The viewer highlights a place for a fixed 400 ms after a firing takes
  from it or puts into it, flashes a transition the same way, and colours
  a transition in flight with the theme's running colour. Under Repeat
  Step a highlight lasts until the step's animation ends. Run modes are
  Idle, Repeat Step and Activate; Pace times the token moves.
- The style module reserves *glyph* for a token's own symbol, promised for
  round two of the token vocabulary.
- `petritype-dot` is the precedent for a renderer that works over the read
  API with no viewer dependency and is a pure function of its inputs.

## 1. Vocabulary

- **Activity thumbnail**: the small live picture of a net, one cell per
  node, lit by activity. **Thumbnail** alone is the same picture with no
  activity, a net's icon.
- **Cell**: one node's square in the thumbnail.
- **Grid**: where the cells go, derived from the layout's ranks and
  orders, never from its coordinates.
- **Level**: a node's activity, 0 to 1. The **pulse** is the fast level,
  the **ember** the slow one. A **hit** raises a level.
- **Glow**: the brightness drawn for a node, made from its levels. The
  user-facing word for what the levels produce.
- **Flow**: a place's recent balance of tokens added against taken, from
  -1 (draining) to 1 (filling).
- **Record**: a net's activity over time as counts per cell per **bin**,
  a fixed width of time, in a few **channels**.

## 2. The activity model: crate `petritype-activity`

### 2.1 Levels

Every place and transition has a pulse and an ember, each a level in
0 to 1. A hit adds the layer's jump and clamps at 1. Every tick each
layer decays by its half-life: `level *= 2^(-dt / half_life)`. The glow a
renderer draws is made from the two in one of two ways, a setting with a
toggle in the window (decided 2026-10-18, both wanted because they say
different things):

- **Peak**: the larger of the two. Every pulse keeps its shape against
  the ember, and no node is ever brighter than one hit.
- **Sum**: the two added and clamped at 1. A node that is both recently
  and persistently active reads brighter than one that is merely recent,
  at the cost of pulses blurring together on a busy node.

The toggle's label is *Glow adds up*: off is Peak, on is Sum. The ember
is what lets a routine net that fires every few minutes still show a
trace, and what lets a dark region read as dark rather than empty.

Hits by event:

- A transition takes one hit when it fires (`Fired`) or launches
  (`Launched`), and a second, smaller hit when it distributes, half the
  jump, so that the end of a long body reads too (decided 2026-10-19).
- A transition in flight holds its pulse at 1 and does not decay until it
  distributes or fails. A stuck transition therefore reads as steady full
  brightness, which looks different from one firing fast.
- A transition that fails sets a failure level to 1 that decays with the
  pulse's half-life; the renderer colours it with the theme's failed
  colour while it lasts.
- A place's pulse takes one hit per token taken from it and one per
  token added to it, from firings and from outside changes alike, clamped
  as above; its ember takes one hit per event that touches it however many
  tokens move, so the ember counts events and the pulse counts tokens
  (2026-10-20, after a quarter per token saturated the ember on a busy
  run). The moment is the event's: a token leaves at launch or fire and lands
  at distribute or fire, so a place pulses when its token goes and again
  when one arrives.
- A token changed in place by a mutate edge or a replace gives its place
  one hit and leaves the flow alone, since nothing entered or left
  (added 2026-10-19: a firing whose only input is a mutate edge consumes
  and produces nothing, so without this rule its place never lights).
- Nested events are ignored at the outer level. An inner net shown in a
  pane has a thumbnail of its own, fed from its own events, the same
  rule the in-flight notification follows.

Two further quantities per place:

- **Base glow** from the standing token count, updated the moment the
  count changes, by a curve that is a setting (decided 2026-10-19):
  - *Halving*, the default: `base = 1 - 2^(-count / half_count)`. With a
    half-count of 1, one token gives half brightness, two give three
    quarters, three give seven eighths, which is Oleg's intuition for an
    occupied place. The curve saturates quickly, so a queue of five and
    one of fifty look alike; a larger half-count stretches it, at the
    cost of one token showing dimmer.
  - *Log to a cap*: `base = log2(1 + count) / log2(1 + count_cap)`,
    clamped to 1. One token is dim and growth stays visible up to the
    cap, which is the curve for watching a backlog build.
  The drawn glow is the larger of the base and the glow from the levels.
  A backlog is a place that grows bright and stays bright while nothing
  fires; under the halving curve it shows as a place that never dims.
- **Flow**, moving on the ember's timescale: each token added raises it
  by the flow gain, each token taken lowers it by the same, clamped to
  -1 to 1, and it decays toward 0 with the ember's half-life. The hue
  mode of §4.1 reads it. Because it moves slowly, a place being filled
  drifts warm and one being drained drifts cold, with no flicker. A launch
  and its distribute are one exchange for the flow, counted when the
  distribute lands, so a place whose token is out while a body runs reads
  neutral rather than draining; a failure after a launch gives restored
  tokens back with the flow unmoved, and counts lost ones as taken then
  (2026-10-20). Hits and counts still move at the launch.

Built 2026-10-19 on the `activity-crate` branch. Two things the build
settled: a transition in flight holds its pulse at 1, so the distribute
hit shows in the ember and as the start of the pulse's decay rather than
as a second peak, which still reads; and on the sequential path a failed
step emits only a failure with no launch before it, which the model
counts as one firing, one tick of the firings clock and the failure.
The grid centres a short rank within its group in whole cells, as the
drawing centres it (done 2026-10-20 after a look at the edge-aligned
version).

### 2.2 The feed

`Activity` is fed two ways, both pure and both tested without a window:

- `apply(&Event)` for a net in process, mapping the core's events to the
  hits above. The driver or the viewer's mirror calls it with what it
  drains.
- A neutral hit interface for anything that is not a core event: the
  viewer's remote mirror, which reads wire notifications, and tests.
  Roughly: fired, launched, distributed, failed, taken with a count,
  added with a count, and count with the standing total of a place.

`tick(dt)` advances the wall-time decays; a fire or launch advances the
firings clock by one for every node. A remote mirror has no core
topology, so the model and the grid also build from ids and names
(added 2026-10-19, step 3), and that is the remote feed's entry point. Levels, flow and base are read per
node by id. The model knows the net's places and transitions by id and name from
the topology it is built from, and nothing else about the net.

### 2.3 Two clocks

A level can decay against either of two clocks, and the clock is a
setting per layer:

- **Wall time**: the half-life is a duration, and the level halves every
  so many seconds whatever the net does. A net that stops goes dark, which
  is how a stall shows.
- **Firings**: the half-life is a count, and the level halves every so
  many firings of the net as a whole, counted at launch or fire. A node
  that fired three firings ago looks the same whether those three took a
  millisecond or a day. The picture is then a function of the event
  sequence alone: a replay looks the same at any speed, a slow net and a
  fast one are comparable, and a frame of a record means the same thing
  whatever the bin width, which is what a classifier wants.

The flow and the failure level follow the clock of the ember and the
pulse respectively. Because the clock is per layer, the pulse can run on
wall time while the ember runs on firings: the pulse then says what is
happening now and the ember what the recent pattern is, whatever the
pace. That is the first combination worth trying; the defaults start with
both on wall time.

A net that stalls under the firings clock keeps its glow for ever, since
nothing advances the clock. So the firings clock carries a **staleness
dim**, on by default with it: the whole thumbnail fades on a wall-time
half-life of its own toward a floor, a quarter of its glow by default and
a setting, so that a stalled net reads as stalled but its pattern is still
there to see; the first firing, launch, distribution or failure restores
it, and a token added from outside does not. (The floor was added
2026-10-19 after the build showed a dim to zero leaves the thumbnail empty
after a few minutes' stall.)

### 2.4 Settings

A plain data struct, `ActivitySettings`, with `Default`, `Clone` and
`PartialEq`, and `serde` behind the crate's `serde` feature, so a file per
net, the command line, the settings window and the tests all set the same
values. Fields: for each of the pulse and the ember its jump, its clock
(`Wall` or `Firings`, §2.3) and its half-life in that clock's unit; the
distribute hit as a fraction of the jump; the staleness dim's half-life
and floor;
the glow mode (`Peak` or `Sum`, §2.1); the count curve (`Halving` with
its half-count, or `Log` with its cap, §2.1); flow gain; hue mode (`Kind`
or `Flow`); and whether the main drawing uses the decay (§4.3).

Starting values, to be tuned in the settings window: pulse jump 1.0 and
half-life 300 ms on wall time, or 3 firings; ember jump 0.05 (0.25 until
the tuning round of 2026-10-20, §10) and half-life 20 s on wall time, or
50 firings; distribute hit 0.5; staleness dim
half-life 30 s; glow mode `Peak`; count curve `Halving` with a
half-count of 1, and a cap of 16 if `Log` is chosen; flow gain 0.25; hue
mode `Kind`; decay in the drawing on.
These are starting points, not decisions; Oleg expects to find the right
values by looking.

## 3. The grid: crate `petritype-layout`

### 3.1 The move

`layout.rs` moves out of petritype-bevy into a workspace crate,
`petritype-layout`, depending on the core crate alone. Bevy's `Vec2` and
`Rect` are replaced by the crate's own `Point` and `Bounds`, two small
`Copy` structs; the viewer converts at its edge. Depending on glam
directly is not done: a glam version other than the one Bevy pins gives
two incompatible vector types. The public names stay as they are; the
viewer's imports change path. The layout tests move with the code and
must pass unchanged in result.

Built 2026-10-19, on the `layout-crate` branches. What the move found:
the old `Layout` derived Bevy's `Resource`, so the viewer now holds it in
a `LayoutRes` wrapper that derefs to it, built like `NetRes`; the orphan
rule rules out `From` impls in the viewer, so it converts through a
public extension trait, `ToBevy`, which its tests and downstream code
need too; the bows became public because the viewer's routes use them;
and the game's viewer code reads the resource the old way, so its pin
move will touch that file.

### 3.2 The grid

`Grid::from_layout(&Layout, &Topology)` puts every laid-out node in a
cell; the topology supplies the nodes and their names, which a layout
does not hold. Within a
cluster, or the whole net when there are no clusters, the cell is the
node's rank along the main axis and its order along the cross axis, in
the layout's direction: ranks downward for `TopDown`, rightward for
`LeftRight`, so the thumbnail keeps the big view's orientation, and a
rank shorter than its group's widest is centred in whole cells along the
cross axis (2026-10-20). Clusters
are packed in rows in declaration order with a one-cell gap, by the same
rule the layout packs them with, in cell units. Ghosts are drawing aids
and get no cell. The grid is deterministic because the layout is.

The grid is always of the whole net (decided 2026-10-19). A `ViewFilter`
narrows the drawing and not the thumbnail, which is the overview. Under a
filter the viewer's `LayoutRes` holds the layout of the filtered part, so
the thumbnail keeps a whole-net `Layout` of its own, computed once per
topology and kept beside the activity model; the grid is built from that,
never from `LayoutRes`. The cluster packing rule the grid reuses is
private in the layout crate today and becomes public in this step. A net
too large for the panel is not handled in this round; if one appears,
the thumbnail becomes a scrolling minimap in the manner of a strategy
game, with the panel showing a window onto the grid.

Adding a node to a net shifts its grid, which is expected; a record
carries its grid with it (§5).

## 4. Drawing

### 4.1 Colour

The activity crate maps a cell to an `Rgb` as a pure function of the
node's kind, its glow, its flow, its failure and the theme's two colours
per kind, dim and bright. Glow interpolates dim to bright. At glow 0 a
cell is the dim colour of its kind, so the static thumbnail still shows
the net's shape. Empty cells take the page
colour.

- `Kind` mode: places one hue, transitions another, taken from the
  viewer's theme so the thumbnail matches the drawing.
- `Flow` mode: a place's hue runs cold through neutral to warm by its
  flow, blue to orange, which colour-blind readers can tell apart; the
  glow is unchanged. Transitions keep a hue outside that
  range, green.
- A failed transition takes the theme's failed colour while its failure
  level lasts. A transition in flight takes the running colour.

`Thumbnail::cells()` yields column, row and colour for any renderer.
`Thumbnail::svg(cell_px)` writes one crisp-edged rectangle per cell: the
vector form, and the static icon with the activity left at zero.

### 4.2 The Bevy adapter

In petritype-bevy, a `ThumbnailPlugin`:

- One `Activity` resource per shown net, fed by the mirror from the
  events it drains in process, and by the remote mirror, which already
  translates the wire's notifications into core events, so both feed
  `apply`. Counts the wire gives without events, the initial in-flight
  set, and every resync (a `Resynced` event leaves the counts unknown)
  are set through the hit interface from the state the mirror holds. The
  inner views own theirs.
- The thumbnail is one texture per net, `cols × rows` pixels, written
  each frame and shown scaled by whole pixels in a UI node with nearest
  filtering. The scale is the largest whole number of pixels per cell,
  from 1 to 16, that fits the grid in a box of 160 by 100 pixels
  (`CellSize::Fit`, the default since 2026-10-20), or a set size
  (`CellSize::Fixed`); a net too large for the box at one pixel a cell
  still shows whole and the panel grows, until the minimap of §3.2. For nets of a few hundred nodes this costs nothing
  measurable.
- Placement: a panel at the canvas's bottom-right corner (decided
  2026-10-18), hidden and shown with a key, like the speed panel. A click on a cell
  pans the camera to its node, which is the bridge from the overview
  back to the drawing.
- A settings window, a panel like the speed panel, with a slider per
  field of `ActivitySettings`, a toggle for hue mode, the *Glow adds up*
  toggle, the clock choice per layer, and the ring's length and bin width
  with a switch to turn it off (§5.3). It edits the
  struct; nothing else reads the window.
- A layout-level headless test places the panel and checks it does not
  collide with the bar or the legend, the lesson of the clipped speed
  panel.

### 4.3 Pulse-and-decay in the main drawing

Under Activate, the drawing's places and transitions take their colour
from the same `Activity` resource: a place brightens with its level and,
in `Flow` mode, tints by its flow; a transition brightens with its pulse;
in flight it keeps the running colour; failed it keeps the failed colour
for the failure level's life. The drawing uses a palette of its own for
this, the thumbnail's with the theme's idle grey as the dim transition
colour, so a transition flashes green when it fires and fades to idle
rather than reading as fireable (2026-10-20); the thumbnail keeps the dim
green, since its resting cells have to tell a place from a transition. The step modes are unchanged: their
highlight lasts until the token animation ends, because that is what a
person stepping a net is watching. A setting turns the decay in the
drawing off, for comparing the two while tuning.

## 5. The record

### 5.1 Shape

A `Record` is the grid it was taken against, with node names per cell, a
bin width in milliseconds, a start time (or none, for an undated record),
the settings in force when it was recorded (for a reader; a replay takes
its own) and a list of bins. A bin holds the model's `Delta` for its width
of time and the range of positions in the net's event log it was built
from: the host's running count of events applied in process, the stream's
sequence numbers over the wire. That range is the link from any cell at any
moment to the firings behind it in the fuller log.

A delta is what the model applied (decided 2026-10-20, after the tuning
round changed the ember to count events and held a launch's flow to its
distribute; it replaces the six counts per cell first written here): the
firings seen, whether the staleness dim was lifted, and

- per place touched: pulse hits (tokens moved, mutations included), ember
  hits (events that touched it), balance (the signed token count the flow
  took, as applied, so a launch's take shows only when its distribute
  lands) and the standing count at the delta's end;
- per transition touched or in flight: fires (fire and launch hits, full),
  distributes (half hits), failures, milliseconds in flight (counted by
  `tick`) and whether it is in flight at the end.

The model keeps this account as it goes (`Activity::take_delta`), so its
rules are written once, in the model, and a recorder bins the account
(`Delta::absorb`: counts, balances and time in flight add, count and
in-flight state take the later value). `Activity::standing` gives the
counts and flights as a delta with no hit, for a recorder started on a
model already running. A ring that drops its oldest bin passes that bin's
counts and flights to the next wherever it names none, so a replay of what
is kept starts from the counts that stood.

Counts, not levels, because counts merge exactly and levels do not, and
because the decay can then be re-tuned after the fact.

### 5.2 Replay

The thumbnail at any moment of a record is a pure function of the record
and the settings: fold the bins through a fresh model built from the
grid's cells, each bin applied (`Activity::apply_delta`: the firings
clock first, then the hits, failures, balances, counts and flights, the
dim lifted) and then ticked by the bin's width. The order inside a bin is
lost by design: a replayed level at a bin boundary is within one bin's
decay of the live one on the wall clock, and equal on the firings clock
when a bin holds at most one firing and each place one event after it.
Under the firings clock the replay is the same picture at any bin width.
`Record::replay(settings)` lends the model after each bin; `Player`
folds to any bin from the nearest checkpoint, every 64 bins by default,
made on the first pass and dropped when the settings change, and follows a
growing ring with its bins' absolute indices. Playback speed is the
player's business. A slow net's days replay in seconds; a fast net's
minute replays at a tenth of the pace.

### 5.3 Coarsening and size

`coarsen(factor)` absorbs consecutive groups of `factor` bins into one,
joining their log ranges, exactly; a short last group is a bin of its own,
and the width multiplies. The policy of what to keep fine and what to
coarsen belongs to the host that records, not to the crate.

The viewer records by default (decided 2026-10-18): a ring of the last
ten minutes at a bin every 100 ms per net shown, in memory, saved to a
file on request. The ring's length and bin width are settings, and it can
be turned off, for a machine short of memory or a net with thousands of
nodes; a new bin or length starts the ring afresh, so a record is always
uniform. Memory is bounded by the length over the bin width times the
cells touched per bin, so a longer ring at a coarser bin costs the same as
a short fine one. An empty bin costs almost nothing.

Budget: a hundred-node net at a bin every 100 ms, with every cell touched
every bin, is about two gigabytes a day. Real nets touch a few cells per
bin and the bins are sparse, but the point stands: 100 ms is a watched
session's resolution, and a day-long record wants a second or ten.

### 5.4 Files

With the `serde` feature a record is JSON on one line, one record per
file, its fields at zero left out, so an empty bin is `{}`. The file is
named `<net>-<YYYYMMDD-HHMMSS>.record.json` by the start in UTC, or
`<net>-undated.record.json`, the date formatted by hand. The grid's
nodes need `petritype-layout`'s own `serde` feature, which the activity
crate's turns on. Compression and a binary form can come when a size calls
for them.

### 5.5 The tensor

`Record::tensor()` lays the record out as time by rows by columns by eight
channels in row-major `f32`, in this order: fires, distributes, failures,
the fraction of the bin in flight, pulse hits, ember hits, balance and the
standing count, carried from the last bin that set it. A transition's cell
fills the first four and a place's the last four; empty cells are zero.
`Tensor::write_npy` writes it in `.npy` version 1.0, a page of code with no
dependency, so Python loads it with no code of ours. The grid's node names
are the key to reading it. No classifier is built here; this is the shape
one would train on.

Built 2026-10-20, on `activity-record` in petritype-rs and
`thumbnail-record` in petritype-bevy. The viewer keeps `RecordSettings`
beside the model's settings (on, bin, length, directory, replay speed), a
recorder per net shown marked with the count of events its model took (over
the wire too: the stream's cursor stays inside the HTTP transport), rows
for them and a *Save record* button in the settings window, and a history
on `H`: the panel's thumbnail folded to a bin, a scrub bar 10 pixels high
under it with a play button and the moment as `m:ss.t`, and `,` and `.` a
bin back and on, while the recording goes on and the main drawing stays
live.

### 5.6 The event log

A record keeps how much happened in each bin and loses the order inside it,
by design. The event log keeps the order (decided 2026-10-20): the events
the model was fed, an entry each, replayed one at a time and each shown for
a fixed dwell, so that an observer reads the order in which things happened
rather than how long they took.

- **The entry.** The core's `Event` is emitted and never read back, so the
  activity crate keeps an `Entry` of its own, ids only, with the token ids so
  that a reader can follow a token: `Fired`, `Launched`, `Distributed`,
  `Failed`, `Added`, `Removed` and `Mutated` from the core's events
  (`Entry::from_event`, `None` for the rest), and `Count` and `InFlight`,
  the corrections a host makes when it sets the model from a mirror's
  state. `Activity::apply_entry` applies an entry by the same code `apply`
  applies the event by, and a correction sets the count or the flight. With
  `serde` an entry's JSON is the core's for the same event.
- **The log.** `EventLog` is a ring of the last so many entries, or all of
  them, each a `Logged` with the host's position `seq`, the number a
  record's bin marks, so that a bin's log range names its entries and
  `position_at` finds a bin's first one, and `at_ms`, its time from the
  log's start on the wall clock, for a reader. An entry's absolute index is
  the entries dropped plus its place in the ring, and holds still while the
  ring drops behind it. An entry takes on the order of 150 bytes, so a
  hundred thousand take some 15 MB.
- **The base.** `Standing` is the counts and in-flight transitions standing
  at a moment: `Activity::standing_state` reads it off a model, sharing the
  walk with the record's `standing`, and `Standing::advance` moves it on by
  an entry by the model's count rule. A log begins on the model's standing
  state, and a ring that drops an entry advances its base by it, so a replay
  of what is kept starts from the counts that stood.
- **The player by dwell.** `EventPlayer` folds the log to any entry from a
  fresh model on the base: before each entry but the log's very first it
  ticks by the dwell, then applies the entry, so the entry shown is at full
  brightness and those before it have decayed by whole dwells, whatever the
  real gaps were. The firings clock moves by the entries' firings alone.
  Checkpoints every 256 entries, made on the first pass and dropped when the
  settings or the dwell change; it follows a growing ring by absolute index,
  as the record's player does, and a checkpoint made before entries dropped
  keeps the levels they left.
- **The files.** With `serde` a log is one JSON value on one line: its
  capacity, its start, the base, the entries dropped and the entries,
  `<net>-<YYYYMMDD-HHMMSS>.events.json` by the record's rule. The viewer
  writes it beside the record under the record's own stem, so the two files
  of one save share it even after the record's start has moved with its
  ring.

Built 2026-10-20, on `activity-events` in petritype-rs and
`thumbnail-events` in petritype-bevy. The viewer keeps a log beside each
recorder under the one *Record* flip, of `events` entries (100 000, the row
*events kept*) and shows each for `dwell` (300 ms, the row *event step*). An
event takes the position the recorder marks for it; a correction from the
view's state takes the next position, and an entry, only when it changes the
model. `E` flips the history between bins and entries, and the line under
the scrub bar reads `bin 1234, 0:12.3` or `event 5678, 0:12.3`, with a comma
for the brief's middle dot, which the bundled font, printable ASCII alone,
does not have. Since 2026-10-21, on `thumbnail-controls`, the scrub bar and
its line are one control panel above the thumbnail, shown under the pointer:
a slider, buttons to the oldest kept, a step back, play, a step forward, the
newest (the live thumbnail, the history off), the mode and the settings cog,
and the line, `live` while the history is off; the slider, play, the steps
and the oldest turn the history on at the newest first, and the mode flips
with the history on or off.

Since 2026-10-21 in petritype-bevy (on main from 2026-10-22), the main drawing
follows the history as the thumbnail does, while `RecordSettings::drawing`
is on (the default; the settings window's *Drawing follows history*): the
canvas shows the net as it stood at the history's position, by bin or by
event, whatever the run mode. Places and transitions take the drawing's
decay colours from the fold, a transition in flight in the fold in the
running colour, a fresh failure in the failed colour, none as fireable; the
live highlights, the chosen transition and the flashes do not draw. The live
tokens hide with their entities kept, so the step machine goes on, and each
place holds a plain disc per token the fold counts, at the slots tokens take
at rest, its count text reading that count; a disc has no label and no
badge, the log holding ids and counts and no values. Nothing moves along
the edges this round. A frame in the theme's active colour runs along the
window's edge, a caption centred under the bar giving the time text and,
by event, what the entry did, `fired dispatch: inbox -2, queue +1`. The
inspector stays live, its first row `live`; panes stay live at half
opacity. One
accessor in the viewer decides what the drawing takes its levels from, and
the fold is computed once a frame. The activity crate is unchanged.

Since 2026-10-23 in petritype-bevy (on main the same day), the panel holds a
thumbnail for every net on screen: a row, the canvas's picture first, then
one for each pane in the order its view was opened, every picture painted
by the one painter at one cell size, the largest at which every grid fits
the box, so a small net is a small picture. The pictures align at the top,
6 px apart, each with its net's name under it at 9 px, cut as a tooltip is.
The history stays the canvas's: the outline goes round the canvas's picture
alone, the controls act on the canvas's history, and the panes' pictures
paint their live models, as the panes stay live. Hovering a picture shows
the controls and a tooltip with its net's name and, for a pane's, `pane 1`
by its place. Since 2026-10-27 a click on a cell of a pane's picture does
for that pane what a click on the canvas's does for the canvas: it pans the
pane's own camera to the cell's node, centred in the pane's room, and the
pane stops framing its net by itself until its next Fit; the side panel and
a window showing the same inner net share one picture, so one click pans
both. The viewer's
*Thumbnails for panes* (on by default, under *Decay in the drawing* in the
settings window) turns the panes' pictures and the labels off, leaving the
canvas's picture alone. The activity crate is unchanged.

Since 2026-10-23 in petritype-bevy (on main the same day), a step of the
history by event, by a step or by play, moves the discs of the entry stepped
over along the arrows, the paths live tokens take, while
`RecordSettings::motion` is on (the default; the settings window's *Replay
moves tokens*, under *Drawing follows history*): a `Fired` entry's leave
their places, pause on the transition for a fifth of the motion and go on
to the places put to, a `Launched` entry's gather on the transition and stay
there while it is in flight until its `Distributed` entry takes them on, a
failure's come back or fade on the transition, and a disc that appears or
disappears otherwise fades in place. The entry's counts on each side decide
how many leave and how many arrive. A step's motion takes 350 ms and one
under play the dwell; a step back plays the entry the other way, a new step
finishes the motion before it at once, and a jump, a step by bin and a
change of mode draw the fold at once, with no disc on a transition. At rest
the discs are the fold's counts, and the count text reads the fold at once.
The thumbnail does not animate. The activity crate is unchanged.

## 6. Crates and dependencies

| crate | contents | deps |
|---|---|---|
| `petritype-layout` | the layout moved out of the viewer, `Point`, `Bounds` | `petritype` |
| `petritype-activity` | `Activity`, `ActivitySettings`, `Grid`, `Thumbnail`, SVG, `Record`, replay, tensor, `EventLog` and its player | `petritype`, `petritype-layout`; feature `serde` adds `serde`, `serde_json` |
| `petritype-bevy` | `ThumbnailPlugin`, the shared `Activity` resource, pulse-and-decay in the drawing, the settings window | adds the two above by path |

The core crate is untouched and keeps knowing nothing of viewers. Both
new crates start at 0.1.0 and publish after the core crate's first
release, in the order `petritype-layout` then `petritype-activity`, each
after the one it depends on. petrivelte is not in this round; the spec is
written so a web page could later draw the same thumbnail from the same
record, through wasm, an SVG the server emits, or a short reimplementation
of §2 and §3.

## 7. Tests

- Layout move: every existing layout test passes with the same results,
  conversions aside.
- Grid: a clustered net gives a deterministic grid; no two nodes share a
  cell; orientation follows the layout's direction; ghosts are absent;
  the grid ignores a filter.
- Activity: a hit raises a level by the jump and clamps; one half-life
  halves it; in flight holds at 1 and releases on distribute and on fail;
  flow has the right sign and decays toward 0; the halving curve gives a
  half, three quarters and seven eighths for one, two and three tokens,
  and the log curve reaches 1 at its cap;
  under the firings clock a level halves after its half-life in firings
  elsewhere in the net and holds while nothing fires, and the staleness
  dim fades the whole and a firing restores it.
- Thumbnail: the colour at glow 0 and 1 per kind and per theme; Peak and
  Sum give the expected glow from a given pulse and ember; a golden SVG
  for a small net.
- Record: recording a run and replaying it reproduces the live levels at
  every bin boundary under the same settings; coarsening sums exactly;
  the tensor has the right shape and the `.npy` header the right bytes.
- Bevy, headless: a step lights the fired transition's cell; the texture
  resizes when the net changes; the panel's layout test of §4.2.

## 8. Order of work

1. The layout move: a branch in petritype-rs adding `petritype-layout`,
   and a branch in petritype-bevy switching to it by path. `just check`
   in both. This finishes before the mirrored-buttons slice starts, or
   the other way round, since both touch the viewer.
2. `petritype-activity`: model with both clocks, settings, grid,
   thumbnail, SVG, tests.
3. The viewer: the shared `Activity` fed from both sources, the decay in
   the drawing under Activate, the thumbnail panel, the settings window.
4. The record: recorder, replay, coarsen, files, tensor, and a player in
   the viewer with a scrub bar. Its own slice.
5. Deferred, by Oleg's decision: a baseline with a deviation mode;
   classifiers; the Epimemer feed and a bar of many nets; whether
   Epimemer's nets move to Rust.

The ported examples and the game's nets are the test beds. The game's
fast-firing nets are the strongest case for §4.3.

## 9. Open points for Oleg

Decided 2026-10-18 and 19: the panel sits at the canvas's bottom-right
corner; the glow is Peak or Sum by a toggle (§2.1); a level decays
against wall time or firings, per layer (§2.3); the viewer keeps a ring
by default, with its length and bin width settable and a switch to turn
it off (§5.3); a distribution gives a second, smaller hit (§2.1); a
place's base glow follows a settable count curve, halving by default
(§2.1); the grid is always the whole net, with a scrolling minimap left
for a net too large for the panel (§3.2).

None open. The spec is agreed as of 2026-10-19.

## 10. Built in the viewer, 2026-10-19, and what to tune with eyes

Step 3 is on the viewer's `thumbnail` branch with a small follow-up on
the activity crate. As built: the thumbnail panel hides and shows on `M`
and the settings window on `G` or the panel's title button, both in the
bar's key table; one texture, the canvas's, with inner views in panes
keeping fed models but drawing none, and a descended inner net taking the
corner; a fresh model when the epoch moves or the layout's direction,
clustering or ghosting changes, and the same model through a filter
change, since the thumbnail ignores the filter; under Activate the decay
replaces the place highlights and the idle and fireable transition
colours, while running, failed, chosen and switched-off colours stay, a
place filled by its token keeps the token's colour, and panes are not
decayed. The ring's settings wait for step 4.

Seen on the traffic light and parcel distribution at the defaults, to
decide by looking, each a one-line change or a settings default:

1. **The ember jump.** At a quarter it saturates after four hits, so a
   busy run flattens to full glow everywhere for twenty seconds and the
   pulse is lost. Try a twentieth, or one ember hit per event rather
   than per token.
2. **Cell size.** Four pixels makes a seven-node net a sliver. Either a
   minimum panel size that picks the cell size in whole pixels, or a
   small net is meant to look small.
3. **Flow on a place a transition empties and refills every frame**
   reads as draining, because each frame ends just after a launch has
   taken the token. Either count a launch and its distribute as one
   exchange at the distribute, or accept that a place short of its
   token while a body runs is draining.
4. **Short ranks** sit at the edge where the drawing centres them;
   centre them in whole cells.
5. **Idle transitions under the decay** draw dim green, the crate's dim
   transition colour being the viewer's fireable green, where the
   drawing's idle grey would read better.

Done 2026-10-20, on `activity-tuning` in petritype-rs and
`thumbnail-tuning` in petritype-bevy:

1. A place takes one ember hit per event however many tokens move, and
   still a pulse hit per token; the ember jump is 0.05, and the ember's
   doc comment gives its steady state to tune by.
2. The panel's cell size fits the grid into a box of 160 by 100 pixels,
   the largest whole number from 1 to 16 (`CellSize::Fit`), with
   `CellSize::Fixed` for a set size; a net too large for the box shows
   whole at a pixel a cell, and the panel grows past it.
3. A launch and its distribute are one exchange for the flow, counted at
   the distribute; a failure after a launch gives restored tokens back
   with the flow unmoved, or counts lost ones as taken then.
4. The grid centres a short rank within its group in whole cells.
5. The drawing decays in a palette of its own whose dim transition colour
   is the theme's idle grey; the thumbnail keeps the crate palette.

Added 2026-10-20, on `activity-events` and `thumbnail-events`: the event
log beside the record, and the history by entry on `E` (§5.6).

Added 2026-10-21, on `thumbnail-controls`: the thumbnail's controls in one
panel above it (§5.6), each symbol a drawn texture, the panel outlined while
the history is on, and the settings window capped to the room above the
controls, its rows scrolled by the wheel.

Added 2026-10-21, on main from 2026-10-22: the main drawing follows the history
(§5.6), with discs for the fold's counts, a frame with the position captioned
under the bar, the inspector live, panes at half opacity, and a setting to keep the
drawing live.

Added 2026-10-22, on main: the frame outlines the canvas, stopping at the
inspector and the side panel while they are open, and appears with the history
before the first fold; one wheel-scroll function serves the inspector, the
settings window and the filter bar.

Added 2026-10-23, on main the same day: a thumbnail for every net on screen, a
labelled picture for each pane in a row beside the canvas's, the outline on
the canvas's picture, and the viewer's *Thumbnails for panes* (§5.6).

Added 2026-10-23, on main the same day: tokens move with the entry during
replay by event, along the arrows through the transition, and the setting
*Replay moves tokens* (§5.6).
