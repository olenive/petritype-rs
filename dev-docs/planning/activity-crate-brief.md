# Brief: `petritype-activity`, the model, the grid and the thumbnail

For one Opus agent. Written 2026-10-19. This is step 2 of the activity
thumbnail spec at
`/private/tmp/claude-501/-Users-admin-work-petritype-rs/1c5160ee-1703-4436-bf19-73b935fde528/scratchpad/activity-thumbnail-spec.md`.
Read the whole spec first; sections 1, 2, 3.2, 4.1, 6 and 7 are what you
build, and sections 3.1, 4.2, 4.3 and 5 are context. Step 1, the layout
crate, is on `main` in petritype-rs and the viewer. Where the spec and the
code disagree, say so in your report rather than guessing.

## What this step is

A new workspace crate in petritype-rs, `petritype-activity`, with no
graphics dependency: the activity model with its two layers, two clocks,
glow modes, count curves, flow and failure; the settings struct; the grid
from a whole-net layout; the thumbnail as cells with colours; and an SVG
writer. Nothing here draws on a screen. The Bevy adapter (step 3) and the
record (step 4) are not built now; design the model's public surface so
that both can be fed and read without a rewrite, which the spec's sections
4.2, 4.3 and 5 describe.

## The crate

- `petritype-rs/petritype-activity/`, a workspace member, version 0.1.0,
  edition and rust-version from the workspace, dependencies `petritype`
  and `petritype-layout` by path with version 0.1.0, feature `serde` adding
  `serde` with derive and `serde_json`, for `ActivitySettings` only in
  this step. Manifest in the shape of `petritype-layout`'s. One README row
  and one DESIGN.md §14 row.
- Modules, roughly: `model` (levels, clocks, hits, flow, failure, in
  flight, the count base), `settings`, `grid`, `thumbnail` (cells and
  colour), `svg`. Names are yours; the spec's vocabulary (§1) is the
  vocabulary of the docs and the public API: level, pulse, ember, hit,
  glow, flow, cell, grid.
- The model is built from a `Topology` (or from a `Net` or `Snapshot`
  through the read API, your call, but it must not hold a reference to
  the net) and knows places and transitions by id and name, nothing else.
- The feed is two things (spec §2.2): `apply(&Event)` for the core's
  events, and a neutral hit interface for a mirror that reads wire
  notifications: fired, launched, distributed, failed, taken with a count,
  added with a count, and the standing count of a place. `tick(dt)`
  advances wall-time decay; a fire or launch advances the firings clock
  for every node. Nested events are ignored.
- The two clocks are per layer (§2.3), with the staleness dim on the
  firings clock. The glow is Peak or Sum (§2.1). The count curve is
  Halving with a half-count or Log with a cap (§2.1). Flow moves on the
  ember's timescale and clock. A transition in flight holds its pulse at
  1. The distribute hit is a fraction of the jump. Defaults are the spec's
  starting values (§2.4).
- The grid (§3.2): from a `Layout` the caller computed over the whole
  net. Rank along the main axis and order along the cross axis in the
  layout's direction; clusters packed in rows by the layout's own packing
  rule, in cell units, with a one-cell gap; ghosts get no cell. The
  packing rule is private in `petritype-layout` today: make it public
  there, in a small commit of its own on the same branch, with a doc
  comment, and use it. The grid carries each cell's node id and name.
- The thumbnail (§4.1): a pure function of the model, the grid, the hue
  mode and a palette. The palette is a plain struct of `Rgb` values the
  caller supplies: dim and bright per kind, the cold and warm ends and the
  neutral for the flow mode, the failed colour, the running colour and
  the page colour, with two provided defaults for a light and a dark
  theme chosen to match the viewer's theme (read `petritype-bevy/src/theme.rs`
  for its colours and mirror them). `cells()` yields column, row and
  colour; `svg(cell_px)` writes one crisp-edged rectangle per cell and
  nothing else.

## Tests

Spec §7, the activity and thumbnail lines, as unit tests in the crate:
hit raises by the jump and clamps; one half-life halves; in flight holds
and releases on distribute and on fail; distribute gives the fraction;
flow sign and decay; the halving curve's half, three quarters and seven
eighths; the log curve reaching 1 at its cap; the firings clock halving
after its half-life in firings elsewhere and holding through a stall; the
staleness dim fading and a firing restoring; Peak and Sum from a given
pulse and ember; colours at glow 0 and 1 per kind and per theme; the grid
of a clustered net deterministic, no two nodes in one cell, orientation
following direction, ghosts absent; a golden SVG for a small net. Use the
workspace's example nets where a real net helps (the traffic light and
parcel distribution are small; `examples/*/net.rs` can be included by
path as the viewer's tests do).

## Also in this step

- The three em-dashes that travelled into `petritype-layout`'s docs with
  the move: replace each with a comma or a full stop, in the packing-rule
  commit or one of its own.

## Gates

`just check` in petritype-rs: the workspace suite with serde, the core
crate alone, clippy with no warnings, `cargo fmt --check`, `cargo doc`
with warnings as errors. The viewer is not touched in this step, but run
its `just check` once at the end against your branch to be sure the
packing-rule change broke nothing there (`just` lists its recipes; its
test recipe needs the Rust runner binary, which is built).

## Rules

- Branch `activity-crate` from `main` in petritype-rs. Branch in the
  checkout, no worktrees. Never merge, push, tag or publish.
- No attribution trailers of any kind on commits; short plain subjects.
- Plain English in docs and messages: no em-dashes, say what is true.
- Stage files by name; never `git add -A` or `git add .`.
- Do not start the Bevy adapter or the record. Do not write to the
  Epimemer memory graph. Install nothing.

## Hand-over

Report: the commits with their subjects; the `just check` tails for both
repositories; the public surface of the model, grid and thumbnail in a
dozen lines, so the step 3 brief can be written from it; every place
where the spec and the code disagreed and what you did; anything the spec
should change. Under 80 lines.
