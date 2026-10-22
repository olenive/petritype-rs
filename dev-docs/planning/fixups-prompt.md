# Prompt: fix-ups found during the controls and layout rounds, 2026-10-19

For one or more Opus agents, one section each or all together. Each
section stands alone. The common rules at the end apply to every section.
None of these is planned work of the planning thread; each was found by a
building agent and left alone because it was outside its brief.

## 1. petrivelte: the theme colour classes resolve to nothing on the nets page

Repository `/Users/admin/work/petrivelte`, frontend under `frontend/`.
Branch `theme-classes` from `main`.

What was found: the Tailwind colour classes `text-destructive`, `bg-card`,
`border-border` and `text-foreground-muted` are defined at `:root`, but
the theme variables they point to live on `body`, so on the nets page
`text-destructive` renders as the inherited text colour and `bg-card` as
transparent. The graph panel's own comment says the same. The legend built
on 19 October uses the theme variables directly to work around it, and the
page's inject error line has the defect. It is probably page-wide.

Do: find where the classes and the variables are defined, make the classes
resolve wherever they are used (moving the variables to `:root`, or the
classes to `body`, whichever the stylesheet's own structure argues for),
and check the fix in a browser probe the way the finding was made: read
the computed colour of an element with `text-destructive` and one with
`bg-card`. Then either leave the legend's direct variables as they are or
switch it to the classes, whichever leaves the page consistent, and say
which. Remove the graph panel comment if it no longer holds. Gate: `just
check` (lint, unit tests, browser suite headless). Report the before and
after computed colours.

## 2. petrivelte: a slider keeps keyboard focus while its press is in flight

Same repository, branch `slider-focus` from `main`, or on the branch of
section 1 after it.

What was found: a `fire`, `toggle`, `select` or `slider` row in the
controls legend is disabled while its press is in flight, and a disabled
input loses keyboard focus, so a keyboard user of a slider loses their
place after every step. The text box already avoids this by going
read-only instead of disabled.

Do: make the slider, and any other row where the same applies, keep focus
while its press is in flight, without sending a second press: a read-only
or aria-busy state that ignores input until the answer lands, matching
what the text box does. Add a browser test that steps a slider twice with
the keyboard and sees both presses sent in order and the focus still on
the slider. Gate: `just check`.

## 3. petritype-bevy: the linker warning on the examples' `__eh_frame`

Repository `/Users/admin/work/petritype-bevy`. Branch `eh-frame` from
`main`. The viewer builds against `/Users/admin/work/petritype-rs` by
path; its `just test` needs the Rust runner binary at
`/Users/admin/work/petritype-rs/target/debug/examples/parcel_runner`,
which is built.

What was found: `cargo test` prints a linker warning about the examples'
`__eh_frame` section, about 36 MB against a 16 MB limit. It is a
warning, the suite passes, and it predates the layout move.

Do: find what makes the section that large in the example binaries
(debug info, monomorphisation of Bevy in every example, or a profile
setting) and whether a profile change in the manifest for examples or
tests, such as a split or packed debuginfo setting, removes it without
slowing the warm build or hiding panics' backtraces. Make the smallest
change that removes the warning, or, if none is reasonable, write what
you found in a short note in DESIGN.md's decisions section and change
nothing. Gate: `just check`, and report the suite's wall time before and
after so a slower build is visible.

## 4. petritype-rs: the three em-dashes in the moved layout docs

Skip this section if the `activity-crate` branch already exists in
`/Users/admin/work/petritype-rs`: its brief includes this fix.

Otherwise, branch `layout-docs` from `main`: in
`petritype-layout/src/lib.rs` replace each em-dash with a comma or a full
stop, keeping the sentence true, one commit. Gate: `just check`.

## 5. Housekeeping across the programme's repositories

Repositories: `/Users/admin/work/petritype-rs`, `petritype-bevy`,
`petrivelte`, `petritype-server`, `petritype`, and the game at
`/Users/admin/work/games/Cyberpunk2020-Simulation`.

- **Merged branches.** List, per repository, every local branch whose
  tip is already contained in `main`, with its last commit date. Do not
  delete anything: put the list in your report for Oleg to approve. A
  branch not contained in `main` is listed apart with how many commits it
  has beyond `main`, and is also left alone.
- **A Justfile for the game.** The five programme repositories each have
  a Justfile where `just` lists the recipes and `just check` runs the
  gates in order. Read two of them, the viewer's and petritype-rs's, and
  write one for the game in the same shape: `check` over `test`, `lint`
  and `fmt` as the game's own CI or README defines them, a doc comment
  above each recipe. Branch `justfile` from the game's `main`. Do not
  touch the game's pinned clones under `../pinned/`. If the game tree has
  uncommitted edits, do not stage them; stage the Justfile by name.
- **The game's stale design lines.** The game's spec documents carry
  lines about an arity cap that the runtime no longer has. Find them
  (search the game's markdown for "arity"), and for each either correct
  the line against the current petritype-rs core (`petritype/src/arity.rs`
  and its docs) or mark it as history with the date, as the document's own
  style does elsewhere. Same branch as the Justfile or its own, your
  call.

- **petritype-rs: the record player's follow after a cleared recorder.**
  `Player::follow` in `petritype-activity/src/record.rs` copies the oldest
  bin only after the recorder has dropped some, so a recorder that is
  cleared while its dropped count is still 0 leaves a following player
  with stale bins. The viewer never clears a recorder (it starts a new
  one), so nothing shows today. Make `clear` count the cleared bins as
  dropped, as `EventLog::clear` does in `log.rs`, so an index is never
  reused, keep the start moving on as it does now, and add the test: a
  player following a recorder that is cleared shows nothing stale.
  Branch from `main` once the record branches are merged.

## Common rules

- Branch in the checkout, no worktrees. Never merge, push, tag, publish
  or deploy. Never delete a branch.
- No attribution trailers of any kind on commits, no co-author or session
  lines; short plain subjects. Plain English in docs: no em-dashes, say
  what is true.
- Stage files by name; never `git add -A` or `git add .`. In the viewer,
  never read, edit, stage or commit `TODO_TEMP.md`. In the server, never
  touch Oleg's uncommitted files, never run anything under `scripts/`,
  never touch a worker, never read or print a token.
- The shell's `grep` wrapper hangs on piped input; use `/usr/bin/grep`.
- `just` is at `/opt/homebrew/bin/just`. Install nothing.
- Do not write to the Epimemer memory graph.
- Report per section: the branch and its commits, the gate's tail, what
  you found, and anything you left undone and why. Under 40 lines per
  section.
