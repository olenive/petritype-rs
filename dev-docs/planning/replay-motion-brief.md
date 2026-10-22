# Brief: tokens move with the entry during replay

Proposed 2026-10-22, the day the drawing began to follow the history (the
`drawing-history` round, on the viewer's main). That round left one thing
open on purpose: by event, stepping over a `Fired` entry shows the
transition's pulse and the places' disc counts changing at once. The order
is there, the motion is not. This brief says what moving the discs would
look like, so Oleg can decide after a look at the canvas as it is. Nothing
here is built until he says so.

One repository, one branch: `~/work/petritype-bevy`, branch
`replay-motion` from `main`, built against `~/work/petritype-rs` on `main`.
No change to the activity crate is expected; the event log already says
which places an entry took from and put to, with counts.

## 1. What moves

Only by event. By bin there is no order inside a bin, so nothing moves.
When the history's position advances by one entry, by a step or by play:

- `Fired`: for each place the entry took from, that many history discs
  leave the place along its input edge to the transition, pause there
  briefly, then travel along the output edges to the places the entry put
  to, where they come to rest as that place's discs. Discs are not
  conserved: the entry's counts on each side decide how many leave and how
  many arrive, as they do now.
- `Launched`: the input half only. The discs gather at the transition and
  stay there while it is in flight (the fold already draws it running).
- `Distributed`: the output half only, from the transition to the places.
- `Failed, restored`: the discs go to the transition and come back.
- `Failed, lost`: the discs go to the transition and fade there.
- `Added`, `Removed`, `Mutated`, `Count`, `In flight`: no travel. A disc
  that appears or disappears fades in or out in place over the same time.

One step back reverses the motion of the entry stepped over. A jump (the
oldest, the newest, a slider drag of more than one entry) moves nothing:
the fold at the new position is drawn, as now.

## 2. Timing

- Under play by event the position advances once per dwell, so the motion
  takes one dwell: the discs arrive as the next entry begins, and the
  canvas reads as a continuous replay. The pause at the transition is a
  fifth of the dwell.
- A single step animates over `STEP_MOTION`, 350 ms, with the same shape.
- A new step or play tick while discs are still in motion finishes the
  motion at once (discs snap to rest) before the next one starts, so the
  canvas never shows two entries at the same time.

## 3. What does not change

- The fold stays the truth: at every position the resting discs per place
  equal the fold's count once motion ends. During motion, a place shows
  its fold count less the discs still arriving, plus the discs still
  leaving; the sum of what is at rest and what is moving is what the fold
  says before and after.
- The live tokens stay hidden, the panes stay live and at half opacity,
  the frame and the caption stay as they are, the inspector stays live.
- The thumbnail does not animate; it shows the bin or entry as now.

## 4. The setting

`RecordSettings.motion: bool`, on by default: a flip row *Replay moves
tokens* under *Drawing follows history*. Off, the discs change count at
once, as today.

## 5. How, in outline

- `history_view.rs` owns it. `follow_tokens` learns the previous position
  and reads the entry at the new one (`EventPlayer::get(seq)`); from the
  entry it plans the motions: per moving disc a `Motion { from, via, to,
  started, duration }` component on a `HistoryDisc`, with `via` the
  transition's position for the two-legged journeys and `None` for a fade.
  One system advances motions each frame and removes the component at the
  end, where the disc takes its resting slot from the same arrangement
  function the resting discs use.
- Positions come from the layout as the live tweens take them: look at
  how a live token travels place to transition to place (`Tween`,
  `Departing` in the render and mirror code) and reuse the path shape
  rather than inventing a second. If the live path is a straight line,
  so is this.
- Reversal is the same plan with `from` and `to` swapped and the entry's
  sides exchanged.
- Nothing in the core or activity crates.

## 6. Docs

README's history paragraph gains a sentence on the motion and the row;
`DESIGN.md` gets a dated entry in the shape of the drawing round's; the
spec `activity-thumbnail-spec.md` in petritype-rs's `dev-docs/planning/`
gets a line in §5.6 and §10.

## 7. Tests

Headless, beside `tests/drawing_history.rs`:

- Step forward over a `Fired` entry: at half of `STEP_MOTION` a disc is
  between its place and the transition; at the end every disc is at rest
  and the counts per place equal the fold's.
- Step back over the same entry reverses it: at half time the disc is on
  the output side of the transition heading back.
- A jump of several entries moves nothing: no `Motion` component exists
  after the jump, and the counts equal the fold's at once.
- Under play by event, a motion's duration equals the dwell.
- A second step during a motion finishes the first at once.
- `Launched` leaves discs at the transition; the following `Distributed`
  takes them on.
- The setting off: no `Motion` is ever added.

## Rules

- Branch in the checkout, no worktrees. Never merge, push, tag or publish.
- Commit messages: short plain subjects, no attribution trailers of any
  kind (the user's convention in every repository).
- Never read, edit, stage or commit `TODO_TEMP.md`; stage files by name,
  never `git add -A` or `git add .`.
- Plain English in comments and docs, no em-dashes. Say what is true.
- No new dependencies.
- Gates before hand-over: `just check`, green; the examples' known
  `ld: __eh_frame` linker warning is not a failure.
- If `grep` hangs on a pipe, use `/usr/bin/grep` or write to a file first.
- At the end leave `replay-motion` checked out.

## Hand-over

Report: the branch and head, each section in one line with the files
touched, what the tests cover, the gate's last lines, how a motion is
planned from an entry, and anything seen that this brief did not ask
about, left alone. Under 50 lines.
