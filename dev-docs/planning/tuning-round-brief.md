# Brief: the activity thumbnail's tuning round

Five things seen by eye on the traffic light and the parcel distribution at
the default settings, each a small change. Spec: `activity-thumbnail-spec.md`
in this folder, §10 lists them; this brief says what to do about each.
Oleg's decisions of 2026-10-20 are marked. Where a choice is still open the
recommended option is the default; do that unless told otherwise.

Two repositories, two branches, in this order, because the viewer builds
against the Rust checkout by path (`../petritype-rs/petritype-activity`):

1. `~/work/petritype-rs`, branch `activity-tuning` from `main`: changes 1,
   3 and 4 (crate `petritype-activity`).
2. `~/work/petritype-bevy`, branch `thumbnail-tuning` from `main`: changes
   2 and 5, built with `activity-tuning` checked out in petritype-rs.

## 1. The ember saturates

Where: `petritype-activity/src/model.rs`, `hit_place`, `touch`, `hit`;
`settings.rs` defaults.

Now: a place takes one ember hit per token taken or added, at a jump of
0.25, so four tokens' worth of movement fills the ember and a busy run
flattens to full glow everywhere for twenty seconds. The pulse is lost
under it.

Do:

- The ember takes one hit per place per event, whatever the token count
  (`hits.min(1)` for the ember's raise in `hit_place`). The pulse keeps a
  hit per token: at the default jump of 1 that changes nothing, at a lower
  pulse jump a batch flashes brighter than a single token, which is the
  point of keeping it.
- Default ember jump 0.05.
- Document the steady state in the settings' doc comment, since it is how
  a reader tunes it: on the wall clock the ember settles near
  `jump × half_life × rate × 1.44` (rate in hits per second), on the
  firings clock near `jump × half_life × share × 1.44` (share of the net's
  firings that hit the node). At the defaults, 0.05 and 20 s, a node
  saturates above 0.7 hits a second; at 0.05 and 50 firings, above a 28%
  share, with an evenly busy node of a seven-node net sitting near a half.

Tests: a place that receives ten tokens in one distribute has the same
ember as one that received one; the defaults.

## 2. Cell size

Where: `petritype-bevy/src/thumbnail_panel.rs`, `DEFAULT_CELL_PX`,
`ThumbnailPanel::cell_px`, `MIN_WIDTH`, `MIN_HEIGHT`, the sizing system
near line 386.

Now: four pixels a cell, whatever the net, so a seven-node net is a
sliver and a two-hundred-node one a postcard.

Do: the cell size fits the grid into a target box, in whole pixels.

- `ThumbnailPanel` gets `fit: CellSize` with `CellSize::Fit { width,
  height, max_px }` (default a box of 160 by 100 pixels, cells no larger
  than 16) and `CellSize::Fixed(u32)` (what `cell_px` was). The cell is
  the largest whole number of pixels that fits both `columns × cell ≤
  width` and `rows × cell ≤ height`, clamped to 1 and `max_px`. A net too
  large for the box at one pixel a cell still shows whole, and the panel
  grows: the whole net always, the minimap comes later (decided
  2026-10-19).
- The panel's floor stays, for the settings button's room.
- The settings window does not get rows for these yet; a value on the
  panel resource and the command line are enough for tuning.

Tests: a 2 by 6 grid in the default box gets 16-pixel cells; a 40 by 30
grid gets 3; a 400 by 10 grid gets 1 and a panel wider than the box; the
placement test still clears the bar and the legend.

## 3. Flow on a place a transition empties and refills

Where: `petritype-activity/src/model.rs`, `apply` for `Launched`,
`Distributed`, `Failed`; `touch`; the per-transition record.

Now: the flow is a slow average of added minus taken. A launch takes the
token and a distribute puts one back, so a place whose token is out while
a long body runs spends most of each cycle one token short and drifts
cold, reading as draining when nothing is being drained.

Do: a launch and its distribute are one exchange for the flow, counted
at the distribute. Recommended, open until Oleg says.

- At `Launched`: the place's hits and count change as now; the flow does
  not move. The transition's in-flight record keeps the launch's
  `consumed` list.
- At `Distributed`: the flow of every place touched takes the balance of
  the held `consumed` and the event's `produced` together, through one
  tally as `exchange` does now, so a self-loop nets to zero; hits and
  counts for `produced` as now.
- At `Failed` with the transition in flight: `restored` true means the
  tokens went back, so the places get hits and their counts back and the
  flow stays; `restored` false means they are gone, so the flow takes the
  held `consumed` as taken, now.
- The hit interface (`taken`, `added`) stays immediate: it has no
  transition to wait for, and it is what tests and count resyncs use.
- Both mirrors feed `apply`, so the rule holds on the canvas and the
  remote alike.

The cost: a place short of a token while a body runs shows neutral flow.
Its count-based base glow drops and the transition shows the running
colour, so the state is still visible, just not as a cold tint.

Tests: a self-loop place after launch, then distribute, has flow 0 and a
place that only lost a token has flow `-gain` only once the distribute
lands; a failed launch with `restored` leaves flow 0 and the count
unchanged; without `restored`, flow `-gain`.

## 4. Short ranks sit at the edge

Where: `petritype-activity/src/grid.rs`, `from_layout_named`, the loop
that places each node at `(order, rank)` within its cluster group.

Now: a rank with fewer nodes than the widest one starts at the group's
left (or top) edge, where the drawing centres it.

Do: centre each rank within its group in whole cells: offset by
`(group_extent - rank_length) / 2` along the cross axis (columns for
`TopDown`, rows for `LeftRight`), integer division. `Placement::order`
counts from 0 within a rank, so the rank's length is its largest order
plus one. Keep the grid's property tests: every cell within bounds, no two
cells in one position, `at` finds each cell.

Tests: a group with ranks of 3 and 1 puts the lone node in column 1; of 4
and 2 puts the pair in columns 1 and 2.

## 5. Idle transitions dim green in the drawing

Where: `petritype-bevy/src/thumbnail.rs`, `cell_palette`,
`NetActivity::transition_colour`, `place_colour`; `render.rs`,
`decaying_colour`.

Now: under Activate with the decay on, the drawing's idle transitions
take the crate palette's dim transition colour, which is the viewer's
fireable green, so every transition looks fireable.

Do: a second palette for the drawing, `drawing_palette(theme)`: the cell
palette with `transition_dim` set to the theme's `transition_idle` (226,
232, 240 light; 45, 55, 72 dark) and `transition_bright` left as the
fireable green, so a transition flashes green when it fires and fades to
the idle grey. `place_dim` is already the theme's place fill. The
thumbnail keeps the crate palette: its resting cells must still tell a
place from a transition. `transition_colour` and `place_colour` take the
palette to use (or a flag for the drawing) rather than only the theme.

Tests: under the decay, a transition that never fired is the theme's idle
colour in the drawing and the crate's dim green in the thumbnail.

## Rules

- Branch in the checkout, no worktrees. Never merge, push, tag or publish.
- Commit messages: short plain subjects, no attribution trailers of any
  kind (the user's convention in every repository).
- In petritype-bevy, never read, edit, stage or commit `TODO_TEMP.md`;
  stage files by name, never `git add -A` or `git add .`.
- Plain English in comments and docs, no em-dashes. Say what is true.
- Gates before hand-over: `just check` in each repository, green with no
  warnings. In the viewer that is with `activity-tuning` checked out in
  petritype-rs.
- If `grep` hangs on a pipe, use `/usr/bin/grep` or write to a file first.
- Spec §10 gets a line per change saying what was done, and §2.4's defaults
  change for the ember jump.

## Hand-over

Report: the branch names and heads, each change in one line, what the
tests cover, the gate outputs' last lines, and anything seen by eye or in
code that this brief did not ask about, left alone.
