# Brief: the controls legend and the place tint on petrivelte's nets page

For one Opus agent. Written 2026-10-19 from the spec at
`/private/tmp/claude-501/-Users-admin-work-petritype-rs/1c5160ee-1703-4436-bf19-73b935fde528/scratchpad/press-route-spec.md`,
section 3, with the decisions of 18 October applied. Read the spec first.
Where the spec and the code disagree, say so in your report rather than
guessing.

## What this round is

The server will relay `press` (another agent is adding the route on branch
`press-route`); the wire already carries a net's controls and its places'
styles in `get_state`. The nets page drops both today. You make the page
keep them, draw the legend, tint a styled place with its one token's
colour, and press a control through the server's route.

## Decisions since the spec was drafted

- The legend is an overlay at the top-left of the graph panel, matching
  the Bevy viewer, open by default with a fold whose state lasts the
  session. Not a row above the graph.
- A `fire` row whose transition is not fireable is dimmed and still
  sends; the runtime answers `nothing` and the notice line shows it. Not a
  disabled button.
- Branch base is `main`; the traffic-light test commits are merged.
- No commit carries attribution trailers of any kind: no co-author line,
  no session line. Short plain subjects.
- The gate is `just check`: `npm run check`, `npm test`, and the browser
  suite under the pinned Playwright, headless, against the fake control
  plane. `just` lists the recipes.

## The work

Branch `net-controls` from `main`. Spec §3.1 (types and the mirror), §3.2
(the press call and the page state), §3.3 (the legend, with the two
decisions above), §3.4 (tests: the unit tests, the fake control plane's
`press`, the browser tests), §3.5 (docs). Commit in the order that reads
best: the reading, the driving, the legend, the docs, or fewer if they fall
together.

The server's route is not on `main` yet while you work. The page talks to
the fake control plane in tests, so nothing here waits on it; the request
shape is spec §1.1: `POST /api/nets/{id}/press` with `{control, value?}`,
answer `{status, reason?, marking_version}`, `unknown_control` as 404.

## Rules

- Branch in the checkout, no worktrees. Never merge, push, tag or publish.
- Stage files by name; never `git add -A` or `git add .`.
- The browser suite runs headless with the pinned Playwright that `just
  ui` uses; install nothing.
- Plain English in docs and messages: no em-dashes, say what is true.
- The end-to-end look against a real server is the planning thread's job
  with Oleg, after your hand-over. Do not attempt it.

## Hand-over

Report: the commits with their subjects; `just check` output tail; the
browser suite's count before and after; every place where the spec and the
code disagreed and what you did; the a11y warning count before and after.
Under 60 lines.
