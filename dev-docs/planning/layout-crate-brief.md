# Brief: the layout moves out of the Bevy viewer into `petritype-layout`

For one Opus agent. Written 2026-10-19. This is step 1 of the activity
thumbnail spec at
`/private/tmp/claude-501/-Users-admin-work-petritype-rs/1c5160ee-1703-4436-bf19-73b935fde528/scratchpad/activity-thumbnail-spec.md`,
section 3.1. Read that section and section 6 first. The rest of the spec is
context for why; none of it is built in this step.

## What this step is

`petritype-bevy/src/layout.rs` lays a net out in ranks, orders and
clusters. It depends on the core crate and on Bevy's `Vec2` and `Rect` and
nothing else of Bevy. It moves, with its tests, into a new workspace crate
in petritype-rs, `petritype-layout`, which depends on `petritype` alone,
so that a renderer that is not Bevy can use the same layout. The viewer
then depends on the new crate and converts at its edge. Behaviour does not
change: every existing layout test passes with the same results, and the
viewer's own suite passes unchanged.

## The crate

- `petritype-rs/petritype-layout/`, a member of the workspace, edition and
  rust-version from the workspace, version 0.1.0, dependency `petritype =
  { path = "../petritype", version = "0.1.0" }`, no features. Its manifest
  follows `petritype-dot`'s shape. The workspace README mentions it in one
  line where the crates are listed; DESIGN.md §14's crate table gains a
  row.
- Two small `Copy` types replace Bevy's: `Point { x: f32, y: f32 }` with
  the handful of operations the layout uses (new, zero, add, sub, scale,
  `to_array`, `From<[f32; 2]>`), and `Bounds { min: Point, max: Point }`
  with the handful it uses (from points, center, size, union, shift).
  Write only what the layout needs; do not reach for glam. The orphan rule
  stops the viewer writing `From<Point> for Vec2`, so the viewer converts
  with `Vec2::from_array(point.to_array())` through a one-line private
  helper or a local extension trait, whichever reads better.
- The public names stay: `Layout`, `Placement`, `LayoutNode`, `Direction`,
  `Clustering`, `Ghosting`, `GhostSide` and the rest keep their names and
  their doc comments; `Placement::position` and the other positions become
  `Point`. The module doc moves with it and becomes the crate doc, with the
  line about the mirror reworded for a crate that has no mirror.
- Tests move with the code. Where a test built a `Vec2` it builds a
  `Point`. Results are unchanged.

## The viewer

- `petritype-bevy` depends on `petritype-layout` by path, beside
  `petritype`. `src/layout.rs` goes; `pub use` the crate's items from
  `lib.rs` where the viewer's public API re-exported them, so downstream
  code and the docs in `lib.rs` keep working. Every `Vec2` that came from
  the layout is converted at the call site that receives it.
- The `.cargo` path overrides noted in memory for building the viewer
  against a Rust branch apply as before; the new crate is one more path
  under the same root.

## Gates

`just check` in petritype-rs (the workspace suite with serde, the core
crate alone, clippy with no warnings, `cargo fmt --check`, `cargo doc` with
warnings as errors) and `just check` in petritype-bevy (the suite under both
runners, clippy, fmt). Both green before hand-over. The layout tests' count
in the new crate equals the count that left the viewer.

## Rules

- Branches: `layout-crate` in petritype-rs from `main`, `layout-crate` in
  petritype-bevy from `main`. Branch in the checkout, no worktrees. Never
  merge, push, tag or publish.
- No attribution trailers of any kind on commits; short plain subjects.
- In petritype-bevy, never read, edit, stage or commit `TODO_TEMP.md`.
  Stage files by name; never `git add -A` or `git add .`.
- Do not start on the activity crate, the grid or anything else in the
  spec; this step is the move alone. If the move shows the spec something
  it got wrong about the layout, say so in your report.
- Plain English in docs and messages: no em-dashes, say what is true.

## Hand-over

Report: the commits on each branch with their subjects; the two `just
check` output tails; the layout test count before and after; the
conversion helper you chose and why; anything the spec got wrong. Under 60
lines.
