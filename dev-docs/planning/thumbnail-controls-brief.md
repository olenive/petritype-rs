# Brief: the thumbnail's controls in one panel above it

Decided 2026-10-21. Today the thumbnail panel has a `settings` button
over its top-right corner shown on hover, a history bar under the
thumbnail with a play button and a track while the history is on, and a
line under that saying what is shown. Oleg wants one control panel
instead: it appears above the thumbnail while the pointer is over the
thumbnail panel or the controls, covers no cell, and holds everything
that acts on the thumbnail, neatly in rows. The thumbnail panel itself
goes back to being the thumbnail and its padding, with one cue, a tinted
outline, while it shows the history rather than the live net.

One repository, one branch: `~/work/petritype-bevy`, branch
`thumbnail-controls` from `thumbnail-panel-polish-2` (which holds the
glyph painter this reuses), built with `activity-events` checked out in
`~/work/petritype-rs` as it is now. The record and event log branches
are unmerged; this stacks on them.

## 1. The control panel

A node out of the layout, as the settings button is now, anchored to the
thumbnail panel's top edge and centred on it: its root spans the panel's
width (`left: 0`, `right: 0`) with its bottom on the panel's top
(`bottom: 100%`), transparent, with a bottom padding of `CONTROLS_GAP`
(2 px) so that the pointer crossing the gap stays over UI; inside it,
centred (`justify_content: Center`), the visible box with the bar's
background, which may be wider than the panel and then overhangs both
sides equally. Three rows inside the box, `PADDING` (6) round them and
4 px between:

1. **The slider**: the history's track as now, 14 px high, the box's
   width, with its rail and thumb; a press or a drag moves the position,
   as the settings window's sliders.
2. **The buttons**, 20 px wide by 18 px high, 4 px apart, left to right:
   jump to the oldest kept (`«`), one step back (`‹`), play or pause
   (the solid triangle, two bars while playing), one step forward (`›`),
   jump to the newest (`»`), then the mode button (an hourglass by time,
   a ladder by event), then the cog (the settings window). Each button
   is lit while its state is on: play while playing, the cog while the
   window is open, the mode button never.
3. **The line**: what is shown, `bin 1234, 0:12.3` or `event 5678,
   0:12.3` as now, and `live` while the history is off, 9 px text in a
   row 11 px high, left aligned.

Every glyph is a texture drawn by the glyph painter (`glyph_pixels`,
`glyph_image`), 14 px square, in the theme's button text colour, painted
again when the theme changes; one `Glyphs` resource holds every handle
(replacing `PlayGlyphs`). Shapes, in the painter's centred coordinates
with `h` half the size:

- chevron `›`: two strokes 2 px thick from (`-0.3h`, `-0.7h`) to
  (`0.3h`, `0`) and on to (`-0.3h`, `0.7h`); `‹` mirrored; `»` two
  chevrons at `x` offsets `-0.4h` and `+0.4h`, `«` mirrored. A point is
  inside a stroke when its distance to the segment is at most half the
  thickness.
- hourglass: two triangles tip to tip at the centre, bases `|y| = 0.8h`
  spanning `|x| <= 0.6h`, with bars 2 px thick along both bases.
- ladder: two uprights 2 px thick at `x = ±0.45h` for `|y| <= 0.85h`,
  three rungs 2 px thick at `y = -0.5h, 0, 0.5h`.
- triangle, two bars and cog as built.

The old history bar, its play button and the line under the thumbnail
go, and so does the `settings` label: the thumbnail panel's children are
the picture and the controls' root. `MIN_WIDTH` and `MIN_HEIGHT` both
become 40 px, a floor to point at; the controls do not count toward the
panel's size. `STACK_GAP` becomes the controls' height plus the gap and
a little air (about 70 px), so the settings window stands above the
controls. The settings window's `spawn_window` row for the cog's state
stays as it is; the cog here is a second `ControlButton::ActivityWindow`.

## 2. Showing and hiding

The controls' root is displayed while the pointer is over the thumbnail
panel, the picture, the root or anything in it (`Interaction` other than
`None` on any), or while the slider is being dragged, even with the
pointer elsewhere; otherwise `Display::None`. `G` opens the window
whether the controls show or not, as now.

While the history is on, the thumbnail panel carries an `Outline` 1 px
wide in the theme's `button_active` colour (an outline is drawn outside
the node and moves nothing); off, no outline. This is the only cue on
the panel itself.

## 3. What the buttons do

- A press on the slider, the play button, a step or the jump to the
  oldest turns the history on if it is off, at the newest, before it
  acts. Jump to the newest turns the history off: the thumbnail is live
  again, playing stops. So `H` is a shortcut, not a gate, and the way
  out of the history is on screen.
- Steps are one bin or one entry by the mode, as `,` and `.`. Jumps go
  to the oldest kept and to the newest. Play as now: bins by the replay
  speed, entries by the dwell, stopping at the newest.
- The mode button flips `HistoryMode` as `E` does, on or off; its glyph
  shows the mode either way.
- Tooltips, in the bar's style: `oldest`, `back one (,)`, `play`,
  `forward one (.)`, `newest, live`, `by time (E)` or `by event (E)`, and
  `settings (G)`. `History` gets `jump_oldest`, `jump_newest`, and
  whatever `turn_on_at_newest` it needs; `ControlButton` gets variants
  for the four new buttons and the mode, key-less but for the mode's `E`
  and the steps' `,` and `.`, which already exist as `HistoryBack`,
  `HistoryForward` and `HistoryMode`.

## 4. The settings window scrolls

The window climbs by the new gap, and on a 720 px window it would cover
the control bar whenever the thumbnail is tall. So the window's height is
capped to the room between the bar's bottom and the controls' top, less
a margin each side, and when its rows do not fit it scrolls: the rows in
a child with `Overflow::scroll_y()` and `ScrollPosition`, moved by the
mouse wheel while the pointer is over the window, the same way the
inspector scrolls (see `selection.rs` or wherever the inspector's wheel
handler lives; reuse it rather than writing a second). The cap is set
from the primary window's height in the system that places the stack.

## 5. Docs

The module doc of `thumbnail_panel.rs` describes the panel as above. The
README's key table: the settings row becomes the cog in the controls;
the play row becomes one row for the controls panel naming the seven
buttons; the hover row's tooltip list gains the new names; the thumbnail
section and the settings window paragraph follow. `lib.rs`'s key table
likewise. Spec `activity-thumbnail-spec.md` in petritype-rs's
`dev-docs/planning/` (untracked, edit in place): the viewer paragraph at
the end of §5.6 and §10 get a line each.

## 6. Tests

- Layout (the `legend.rs` placement test, extended): with the controls
  shown, their box sits whole above the thumbnail panel with its bottom
  `CONTROLS_GAP` above the panel's top, centred on it to the pixel, clear
  of the thumbnail; the settings window, open, sits whole above the
  controls; both clear the bar and the legend; nothing hovering, the
  controls take no room and the panel is the thumbnail and its padding
  at the new floors.
- At a 720 px window with a 100 px thumbnail, the settings window's
  height is capped so that it clears the bar, and a wheel event over it
  changes its `ScrollPosition`.
- Hover: over the panel the controls show; over the controls alone they
  stay; in the gap they stay; away they hide; dragging the slider with
  the pointer away keeps them.
- Buttons: each of the seven does what §3 says to `History` and the
  window, starting from the history off and from on; jump to the newest
  turns it off and the line reads `live`; the outline appears and
  disappears with the history.
- Glyphs: each shape's alpha at a few points inside and outside, as the
  cog's and the triangle's tests do; the theme repaint covers every
  handle in `Glyphs`.
- The existing history tests move from the old bar to the new controls
  with their assertions kept.

## Rules

- Branch in the checkout, no worktrees. Never merge, push, tag or publish.
- Commit messages: short plain subjects, no attribution trailers of any
  kind (the user's convention in every repository).
- Never read, edit, stage or commit `TODO_TEMP.md`; stage files by name,
  never `git add -A` or `git add .`.
- Plain English in comments and docs, no em-dashes. Say what is true.
- No new dependencies. The bundled font covers printable ASCII only, so
  every symbol is a drawn texture.
- Gates before hand-over: `just check`, green; the examples' known
  `ld: __eh_frame` linker warning is not a failure.
- If `grep` hangs on a pipe, use `/usr/bin/grep` or write to a file first.
- At the end leave `thumbnail-controls` checked out.

## Hand-over

Report: the branch and head, each section in one line with the files
touched, what the tests cover, the gate's last lines, the controls' box
size and the settings window's capped height at 720 px, and anything
seen that this brief did not ask about, left alone. Under 50 lines.
