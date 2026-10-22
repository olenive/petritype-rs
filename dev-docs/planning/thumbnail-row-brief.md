# Brief: a thumbnail for every net on screen

Proposed 2026-10-22. The thumbnail panel shows the canvas's net. Since the
inner views (2026-10) a window can show several nets at once, the canvas
and one or more panes, and since the activity crate the viewer keeps an
activity model for each of them: `Activities.canvas` for the canvas and
`Activities.views` for the panes, fed every frame (`feed_views`), so far
drawn only into the panes' own colours. The controls round (2026-10-21)
centred the controls on the panel so that the panel could grow. This brief
puts the panes' thumbnails beside the canvas's, in one row. Nothing is
built until Oleg agrees.

One repository, one branch: `~/work/petritype-bevy`, branch
`thumbnail-row` from `main`, built against `~/work/petritype-rs` on `main`.
The activity crate's thumbnail already paints any `Activity` on its grid;
no crate change is expected.

## 1. The row

The thumbnail panel holds a row of pictures, left to right: the canvas's
thumbnail first, then one per pane in the panes' own order. Each picture
is painted by the same painter at the same cell size, from its net's
activity and that net's layout grid, so a small net is a small picture
and a large net a large one; the pictures align at the top, the row's
height is the tallest picture's, and `THUMBNAIL_GAP` (6 px) separates
them. Under each picture a label in the bar's text style, 9 px: the net's
name, truncated as tooltips are, centred under its picture; one line.
The panel's padding stays as it is; the panel is the row and its labels
and keeps its floors (`MIN_WIDTH`, `MIN_HEIGHT`). The controls' box stays
centred on the panel, which now may be wider, as the controls round made
possible.

A pane opening adds its picture that frame; a pane closing removes it.
The panel's cell size setting applies to every picture.

## 2. What the history does

The history is the canvas's. The thumbnail outline while the history is
on goes round the canvas's picture only, not the row, so the eye is told
which picture is in the past. The panes' pictures are live always, as the
panes themselves are live under the history (half opacity in the drawing,
full in the row). The controls act on the canvas's history, as now; no
per-picture controls in this round.

## 3. Hover and click

Hovering a picture shows a tooltip with the net's name and, for a pane's
picture, `pane n` by its order. Hovering any picture shows the controls,
as hovering the panel does today. A click on a picture does nothing new in
this round; what a click should mean, bringing the pane forward or
selecting its net, is left for a later round to decide.

## 4. The setting

`ActivitySettings` is per net and the activity crate's; this is a viewer
setting, beside `decay_in_drawing`: `thumbnails_for_panes: bool`, on by
default, a flip row *Thumbnails for panes* in the settings window. Off,
the panel shows the canvas's picture alone, as today.

## 5. How, in outline

- The panel module (`thumbnail_panel.rs`) spawns one picture node per
  shown net under a row node, keyed by the net's identity as
  `Activities.views` keys it; a system reconciles the row with
  `Activities` each frame: spawn for a new key, despawn for a gone one,
  repaint for the rest. The painter call that paints the canvas's picture
  today becomes a function of (activity, grid, image) called once per
  picture.
- The grids: the canvas's grid is where it is today; each pane's net
  needs its layout grid for the painter, which the pane already has for
  its own drawing. Read it from there; do not compute a second.
- The outline moves from the panel to the canvas's picture node. The
  layout test of the controls round, which measures the panel, is
  extended rather than replaced.
- `Activities.views` is the source of truth for which panes exist; the
  row never asks the pane entities directly.

## 6. Docs

README's thumbnail section describes the row, the labels and the setting;
the key table's hover row gains the picture tooltips; `lib.rs`'s overview
likewise; `DESIGN.md` gets a dated entry; the spec
`activity-thumbnail-spec.md` in petritype-rs's `dev-docs/planning/` gets a
paragraph in §5.6 and a line in §10.

## 7. Tests

Headless, beside `tests/thumbnail.rs`:

- With the canvas and two panes shown, the row holds three pictures in
  order, each the size its grid and the cell size give, separated by the
  gap, aligned at the top, each labelled with its net's name; the panel
  is the row's width plus padding, and the controls' box is centred on it
  to the pixel.
- Closing a pane removes its picture that frame; opening one adds it.
- With the history on, the outline is round the canvas's picture and no
  other; the panes' pictures keep painting the live activity while the
  canvas's shows the position.
- The setting off: one picture, the panel as before the round, the
  existing thumbnail tests unchanged.
- Hovering a pane's picture shows the controls and the tooltip names the
  net and the pane.

## Rules

- Branch in the checkout, no worktrees. Never merge, push, tag or publish.
- Commit messages: short plain subjects, no attribution trailers of any
  kind (the user's convention in every repository).
- Never read, edit, stage or commit `TODO_TEMP.md`; stage files by name,
  never `git add -A` or `git add .`.
- Plain English in comments and docs, no em-dashes. Say what is true.
- No new dependencies. The bundled font covers printable ASCII only.
- Gates before hand-over: `just check`, green; the examples' known
  `ld: __eh_frame` linker warning is not a failure.
- If `grep` hangs on a pipe, use `/usr/bin/grep` or write to a file first.
- At the end leave `thumbnail-row` checked out.

## Hand-over

Report: the branch and head, each section in one line with the files
touched, what the tests cover, the gate's last lines, the row's size with
the canvas and two panes at the default cell size, and anything seen that
this brief did not ask about, left alone. Under 50 lines.
