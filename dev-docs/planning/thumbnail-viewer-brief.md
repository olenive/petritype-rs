# Brief: the activity thumbnail in the Bevy viewer

For one Opus agent. Written 2026-10-19. This is step 3 of the activity
thumbnail spec at
`/private/tmp/claude-501/-Users-admin-work-petritype-rs/1c5160ee-1703-4436-bf19-73b935fde528/scratchpad/activity-thumbnail-spec.md`.
Read the whole spec first; sections 4.2 and 4.3 are what you build, with
2, 3.2 and 4.1 as the crate you build on and 5 as what comes after. Steps
1 and 2 are on `main` in petritype-rs: `petritype-layout` and
`petritype-activity`. Read `petritype-activity`'s crate docs and its
public surface before the viewer's code. Where the spec and the code
disagree, say so in your report rather than guessing.

## What this step is

The viewer gets one `Activity` per shown net, fed from both of its
sources; the drawing's places and transitions take their colour from it
under Activate; a thumbnail panel sits at the canvas's bottom-right; and
a settings window edits the activity settings. No record, no replay, no
file: that is step 4.

## The viewer

Repository `/Users/admin/work/petritype-bevy`, branch `thumbnail` from
`main`. It depends on `petritype-activity` by path beside the other two.

1. **The activity resource.** One per shown net, the outer net's and
   each inner view's, built from the topology when a view is built or
   rebuilt and fed as the spec's §4.2 says: the in-process mirror and the
   remote mirror both hand `apply` the core events they already produce
   (the remote mirror translates wire notifications into them); counts,
   the initial in-flight set and every resync are set through the hit
   interface from the state the mirror holds. `tick` runs once a frame
   with the frame's delta. A rebuild of the view, a reload or a reset
   makes a fresh resource. A `ThumbnailPlugin` owns this, added by
   `PetritypePlugin`; the resource and the tick must work under
   `MinimalPlugins` so the mirror tests can read levels without a window.
2. **The thumbnail panel** (§4.2). The grid comes from a whole-net
   `Layout` the plugin computes once per topology with the viewer's
   current layout settings, never from `LayoutRes`, which may hold a
   filtered layout. One texture per shown net, `columns × rows` pixels,
   rewritten each frame from `Thumbnail::cells()`, shown in a UI node at
   the canvas's bottom-right scaled by whole pixels with nearest
   filtering, cell size a setting with a sensible default (three or four
   pixels). Hidden and shown with a key, like the speed panel; the key
   joins the legend's key list if there is one. A click on a cell pans
   the camera to the node (`Grid::at`). The outer net's thumbnail is the
   one in the corner; an inner view shown on the canvas shows its own.
3. **Pulse-and-decay in the drawing** (§4.3). Under Activate, when the
   `decay_in_drawing` setting is on, a place's fill brightness and (in
   flow mode) its tint, and a transition's brightness, come from the
   activity resource through `Palette::place` and `Palette::transition`,
   with the viewer's theme colours (the crate's `LIGHT` and `DARK` mirror
   them). In flight keeps the running colour and failed the failed
   colour, as now. Idle and Repeat Step are unchanged: their highlights
   and flashes stay exactly as they are. With the setting off, Activate
   draws as it does today.
4. **The settings window.** A panel like the speed panel, with a control
   per field of `ActivitySettings`: sliders for the numbers, toggles for
   the clocks per layer, *Glow adds up* for the glow mode, the count
   curve, the hue mode, the staleness dim with its half-life and floor,
   and `decay_in_drawing`. It edits the resource's settings and nothing
   else reads the window. Its key joins the same list.
5. **Theme.** The palette follows the viewer's theme toggle, light or
   dark.

## Tests

Spec §7's Bevy line, headless under `MinimalPlugins` where the mirror
tests run: a step lights the fired transition's cell and the places it
touched; a launch holds the transition until its distribute; the texture
resizes when the net changes; a reset gives a fresh resource; a remote
net fed through the scripted transport lights the same cells as the net
in process for the same events. A layout-level test, the kind the speed
panel got after the legend bug, places the thumbnail panel and the
settings window and checks neither collides with the bar, the legend or
each other. The drawing's Activate colours get one test that a fired
transition is brighter than an idle one under the setting and unchanged
without it.

## Gates

`just check` in petritype-bevy: the suite under both runners, clippy with
no warnings, `cargo fmt --check`. Run `just check` in petritype-rs once at
the end if you changed anything there, which you should not need to; if
the activity crate's surface needs a change for the viewer, make it on a
branch `activity-crate-2` from `main` there, say so in your report, and
note that the viewer branch builds only with it.

## Rules

- Branch in the checkout, no worktrees. Never merge, push, tag or publish.
- No attribution trailers of any kind on commits; short plain subjects.
  Several commits are fine: the resource, the panel, the drawing, the
  settings window, the docs.
- Never read, edit, stage or commit `TODO_TEMP.md`. Stage files by name.
- Plain English in docs and messages: no em-dashes, say what is true. The
  viewer's README and `lib.rs` index gain the thumbnail and the settings
  window; DESIGN.md's decisions section gains a note for this round.
- Do not start the record. Do not write to the Epimemer memory graph.
  Install nothing.
- The shell's `grep` wrapper hangs on piped input; use `/usr/bin/grep`.
  `just` is at `/opt/homebrew/bin/just`; the test recipe needs the Rust
  runner binary at
  `/Users/admin/work/petritype-rs/target/debug/examples/parcel_runner`,
  which is built, and uv.

## Hand-over

Report: the commits with their subjects; the `just check` tail; the keys
you chose; every place where the spec and the code disagreed and what you
did; what the thumbnail looks like on the traffic light and parcel
distribution at the default settings, in words; anything the spec should
change. Under 80 lines.
