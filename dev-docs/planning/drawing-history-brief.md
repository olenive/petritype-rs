# Brief: the main drawing follows the history

Designed 2026-10-21. Since the record (2026-10-20) and the event log
(2026-10-20), the thumbnail can show a past bin or a past event while the
net runs on underneath, and since the controls round (2026-10-21) the panel
above the thumbnail drives that. The main drawing has stayed live
throughout, as each brief said it would. Oleg wants it to follow: while the
history is on, the canvas shows the net as it stood at the history's
position, so an observer reviews what happened on the drawing itself, with
names and arcs, in the order it happened. The thumbnail keeps doing what it
does; this round gives the drawing the same source.

One repository, one branch: `~/work/petritype-bevy`, branch
`drawing-history` from `main` (`d51901a` or later), built against
`~/work/petritype-rs` on `main` as it is. The activity crate is not touched
unless §5 finds it has to be; say so in the hand-back if it is.

## 1. What the drawing shows in the history

The history's fold is an `Activity` (`History::fold`): per place a count,
a flow, a pulse and an ember; per transition an in-flight flag and a failure
level. That is all the drawing has to draw from, and it is enough for the
review: who fired, what moved where, how many stood.

While the history is on and the setting of §4 is on, the canvas net's
drawing takes everything from the fold, in both modes, by bin and by
event, whatever the run mode:

- **Colours.** Places and transitions take the decay colours the drawing
  already uses under Activate (`glow_fill`, `decaying_colour` in
  `render.rs`), computed from the fold instead of the live model, with the
  drawing's palette. A transition in flight in the fold takes the running
  colour; one whose failure is fresh takes the failed colour; every other
  transition takes its glow colour, which rests at the idle grey. The fold
  does not know which transitions are fireable, so none draws fireable. The
  live place highlights, the step machine's chosen transition and the
  failure flashes are live things and do not draw while the history shows.
- **Tokens.** The live token discs hide (`Visibility::Hidden`; their
  entities, tweens and `Departing` state live on, so the step machine is
  not held up). In their place the drawing shows history discs: per place,
  as many discs as the fold's count, in the theme's plain token colour,
  arranged as the live discs are at rest (the same arrangement function, no
  tween) and capped as `token_visibility` caps overflow. The count text
  reads the fold's count plainly, `3 tokens`, since the filter does not
  know these tokens. A history disc carries no label and no badge: the log
  holds ids and counts, never values, and the drawing says nothing it does
  not know.
- **Panes.** The inner views in panes stay live; the history follows the
  canvas's net only. So that the two times on screen are told apart, panes
  draw at half opacity while the canvas shows the history, and come back at
  full when it stops.
- **Edges and names** are drawn as now; nothing moves.

When the history turns off (the newest button, `H`, or the setting) the
live discs show again where they are, the history discs go, the colours
return to the live model's or the mode's, and the panes come back.

## 2. The cue on the canvas

The thumbnail panel is outlined while the history is on. The canvas gets a
cue of its own while the drawing follows:

- A frame along the window's edge, a UI node over the whole window that
  takes no pointer, with a 2 px inset border in the theme's `button_active`
  colour, `Display::None` otherwise.
- Centred under the bar, in the bar's text style (amended 2026-10-22: the
  legend has the top-left and the inspector the right, so the caption is
  centred):
  the history's time text (`bin 1234, 0:12.3` or `event 5678, 0:12.3`, the
  same `History::time_text` the panel's line uses), and, by event, a second
  line saying what the entry at the position is, with the layout's names:
  `fired dispatch: inbox -2, queue +1` (the places taken from with their
  counts negative, then the places put to, in the entry's order), `launched
  dispatch: inbox -1`, `distributed dispatch: queue +1`, `failed dispatch,
  restored` or `failed dispatch, lost: inbox -1`, `added inbox +1`,
  `removed inbox -1`, `mutated inbox`, `count inbox = 3`, `in flight
  dispatch on` or `off`; truncated as tooltips are. By bin there is no
  second line.

## 3. What stays live

- The run controls, the speed, the filter and the record keep working on
  the live net; the recording and the log go on underneath, as now.
- The inspector keeps showing the live net, since it is the one place on
  screen that has token values; while the drawing follows the history its
  first row says `live` in the frame's colour, so the mix is explicit.
- Hovering a place while the drawing follows shows the fold's count in the
  tooltip; a history disc has no tooltip; transitions' tooltips show their
  name and nothing of the live state.
- A click on a transition selects it and does not fire it while the drawing
  follows the history: the firing would be live and invisible. Selection
  works as now.

## 4. The setting

`RecordSettings` gains `drawing: bool`, on by default: whether the main
drawing follows the history. A flip row *Drawing follows history* in the
settings window under the history's rows. Off, the drawing stays live while
the thumbnail shows the history, as until now, and no frame is drawn. This
is for comparing the two with eyes, like `decay_in_drawing`.

## 5. How, in outline

- One decision in one place. Today `Activities::drawing(mode)` hands the
  colour systems a `&NetActivity`, and `NetActivity::place_colour` and
  `transition_colour` read its private model. Make those two take the
  `&Activity` to draw from (free functions or methods on a small wrapper),
  and add one accessor that answers for the whole frame: the fold while the
  history is on and the setting is on, the live model under Activate with
  `decay_in_drawing`, nothing otherwise. The colour systems, the history
  discs, the frame, the pane opacity and the tooltip all ask it, so there
  is no second place that decides. `History::fold` takes `&mut self`
  because the players cache; the systems that need the fold take
  `ResMut<History>`, or one system early in the frame copies what the
  drawing needs into a resource. Choose whichever reads cleaner, and keep
  the fold computed once per frame.
- The history discs are entities with a marker, spawned and despawned to
  match the fold's counts each frame the fold changes, with the live token
  arrangement reused as a pure function. A new module, `history_view.rs`
  or a name you prefer, holds them, the frame, the caption and the
  pane opacity; `render.rs` keeps the colour systems.
- Ordering: after `ThumbnailSystems`, where the history moves, and before
  the render systems that colour.
- Nothing in the core crate; nothing new in the activity crate unless the
  drawing needs a reader the fold lacks. If it does, add it on a branch
  `activity-history-view` in petritype-rs and say so.

## 6. Docs

- `thumbnail.rs`'s `History` doc ends with the drawing following when the
  setting is on, in place of "The main drawing stays live".
- README: the history paragraph says the drawing follows and what it shows
  and does not (counts, no values; panes live), the settings window
  paragraph gains the row, the key table's `H` row says the drawing too.
  `lib.rs`'s overview likewise.
- `DESIGN.md`: a dated entry in the shape of the controls' entry: what is
  drawn from the fold, what is live, the cue, the setting, and what the
  tests cover.
- Spec `activity-thumbnail-spec.md` in petritype-rs's `dev-docs/planning/`
  (untracked, edit in place): a paragraph at the end of §5.6 and a line in
  §10.

## 7. Tests

Headless, beside the thumbnail's (`tests/thumbnail.rs` or a sibling using
`tests/common`):

- With a recorded run and the history on at an entry, each place's history
  discs count the fold's count, the live discs are hidden, and the count
  text reads the fold's count; off, the live discs are visible, no history
  disc remains, and `assert_mirror_exact` still holds.
- At the newest entry the drawing's place and transition colours equal the
  live decayed drawing's within one dwell on the wall layers, as the
  thumbnail's test does.
- Stepping forward over a `Fired` entry, the transition draws the pulse at
  its brightest, the place taken from loses that many discs and the place
  put to gains them; the caption's second line reads the entry as §2 says.
- The frame and the caption appear with the history on and the setting
  on, and not with either off; by bin there is one line, by event two.
- A live step under Repeat Step completes while the drawing follows (the
  step machine is not held by hidden discs), and the drawing does not
  change until the history moves.
- Panes draw at half opacity while the drawing follows and at full after.
- A click on a transition while the drawing follows selects it and fires
  nothing; the same click with the history off fires as now.
- The setting off: the thumbnail shows the history and the drawing stays
  live, no frame.

## Settled 2026-10-21

Oleg took the brief as written. So: no token movement along the edges in
this round (the pulse on the transition and the discs changing count show
the order; movement is decided after seeing it on the canvas); panes at
half opacity while the canvas shows the history; the inspector live with a
`live` first row.

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
- At the end leave `drawing-history` checked out.

## Hand-over

Report: the branch and head, each section in one line with the files
touched, what the tests cover, the gate's last lines, how the fold reaches
the drawing (the accessor's name and where it is called), and anything seen
that this brief did not ask about, left alone. Under 50 lines.
