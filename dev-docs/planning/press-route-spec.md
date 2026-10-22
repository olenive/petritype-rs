# Spec: the HTTP round, `press` on the server and the legend on the nets page

Draft for review, 2026-10-16. Finishes the net-declared controls feature across the HTTP
path: petritype-server relays `press`, the Bevy HTTP transport routes it, petrivelte's nets
page shows the legend and the place tint. The vocabulary is fixed by PROTOCOL.md §5.1, §5.3
and §6.6 as built in slice 1; nothing here changes the wire.

## 0. What is already true

- The server passes `get_state` through verbatim on both the plain and the projected
  paths (`IPCResponse.result` untouched; `project_state` spreads the answer and each place),
  so `topology.controls` and `places[].style` already reach a viewer. Every notification is
  relayed as a `WorkerEvent` keyed by its method, so a press's `added`, `fired` and
  `fireable` arrive as they do for `inject` and `step`.
- Each protocol method has a hand-written route on the control plane (`/api/nets/{id}/...`)
  and on the worker (`/nets/{id}/...`); there is no generic dispatch, no per-method
  allowlist, no read-only mode. A POST is scope `operate` unless listed as a read. A method
  missing from the IPC timeout table gets 30 s and kills the net on timeout.
- The Bevy HTTP transport maps each method to a server route in `route()` and refuses the
  rest with "has no route on petritype-server".
- The nets page rebuilds the topology from four keys, so `controls` is dropped today and a
  place's `style` survives only by reference on the raw array. Tokens' `color` is drawn;
  places are filled from the `--place-fill` CSS variable in four places (circle, count scrim,
  text outline, badge stroke). The page drives the net over the server's routes with
  inline error lines, no toasts, no keyboard shortcuts, and has no read-only state (the
  browser session is always a user).

## 1. petritype-server

### 1.1 The route

```
POST /api/nets/{net_id}/press      body {control, value?}      scope operate
```

- Control plane: `press_control` in `execution/proxy.py` beside `inject_token`: a
  `PressBody {control: str, value: Any = None}`; `value` is forwarded only when given
  (a `null` counts as absent, as the protocol says). Idempotency header forwarded as for
  every POST; not retried. No `path` or `nested_id`: `press` joins the outer-only comment
  lists (proxy.py, server.py, RUNTIMES_AND_PROTOCOL.md) with `step`, `enable` and `disable`.
- Worker: `press_control` in `worker/server.py` beside `inject_token`, under
  `idempotent_response`, sending `press {control, value?}` through `send_request` and
  returning the result verbatim: `{status: "pressed" | "nothing", reason?,
  marking_version}`.
- Errors: the runtime's protocol error becomes the worker's 400 `{detail, reason}` as
  today; the control plane maps `unknown_control` to 404 in `PROTOCOL_REASON_STATUS`
  (as `unknown_transition` is); `invalid_params` is 422 and `internal` 500 already.
- Timeout: `press` in `NET_IPC_INACTIVITY_TIMEOUTS` with `step`'s value, since a press
  that fires runs a body.
- Scope: nothing to write; the POST is `operate` by the verb rule. The row goes into
  `EXPECTED_SCOPES` in `tests/test_api_token_scopes.py`, which fails on any route it does
  not know.

### 1.2 Tests

- Control plane unit (`tests/test_execution_proxy_reasons.py` pattern): the body is
  forwarded as `{control, value}` and as `{control}` alone when `value` is absent or null;
  the answer passes through; `unknown_control` from the worker is 404 with the reason;
  `invalid_params` is 422; a missing `control` is 422 before the worker.
- Worker with the fake net process (`tests/fake_net_process.py`, `test_net_handshake.py`
  pattern): a scripted `press` answered `pressed`; one answered with the `unknown_control`
  error; the recorded request shows no address.
- `tests/test_relay_inner_nets.py`: `press` added to the execution routes that send no
  address.
- Pass-through: a `get_state` answer carrying `topology.controls` and a place `style`
  comes back unchanged on the plain path and on the projected path (`?client=`).
- Real runtimes (`tests/test_net_process_e2e.py`, python and rust, with
  `PETRITYPE_PYTHON_SRC` pointing at the `~/work/petritype` checkout and
  `PETRITYPE_RUNNER_BIN` at the Rust runner): load the traffic light, read `get_state` and
  see its controls, press `change` and see `fired`, press `car` with a name and see
  `added`, press an unknown control and get the error. If the e2e module's net list is
  fixed, extend it; it runs only when the env vars are set, as today.

### 1.3 Docs

README's execution table gains `press` (and the missing `enable` and `disable` rows);
ARCHITECTURE's control-plane and worker tables gain `press`; RUNTIMES_AND_PROTOCOL's route
list and outer-only line gain it, and its stale "Not yet implemented" header line is
corrected since the later sections record the work as landed.

### 1.4 Production

A press works in production only when the user deployment pins a petritype release that
has `press`. PyPI's latest is 0.2.0 and `press` is unreleased, so the HTTP path is testable
locally now and usable in production after the next PyPI release (Oleg's). Nothing in this
round deploys or touches a worker.

## 2. petritype-bevy, the HTTP transport

`route("press", ...)` in `src/remote/http.rs` posts `/press` with the body `{control,
value?}`; `press` joins the file's `OUTER_ONLY` list. The route unit tests gain `press`
(with and without a value, and `value: null` dropped); `tests/remote_http.rs` gains a press
against a live server when `PETRITYPE_SERVER_URL`, `PETRITYPE_NET_ID` and
`PETRITYPE_TOKEN` are set, skipped otherwise as its neighbours are. README's HTTP section
names `press` among the routed methods. One commit.

## 3. petrivelte, the nets page

### 3.1 Reading

- `types.ts`: `Control {name, label, kind, node?, action?, config?}` with the kinds and
  actions as string unions; `Topology.controls?: Control[]`; `TopologyPlace.style?:
  {fill: 'token'}`. `mirrorFromState` keeps `controls` (as `[]` when absent) and each
  place's `style`; `MirrorPlace` and `DrawnPlace` carry `style`.
- `drawNet` gives each `DrawnPlace` a `fill?: string`: the colour of its single token when
  `style.fill === 'token'`, the place holds exactly one token, and that token has a
  `color`; otherwise absent. `PlaceCircle` and the place's `<g>` set `style="--place-fill:
  {fill}"` when present, so the circle, the count scrim, the text outline and the badge
  stroke follow with no further change. During a firing animation the page has no
  input/output highlight on places (unlike Bevy), so nothing wins over the tint.

### 3.2 Driving

- `api.ts`: `executionPress(netId, control, value?)` posting `/api/nets/{id}/press` with
  `{control, value}` (`value` omitted when undefined), an idempotency key as the other
  writes, throwing `NetProxyError` on a non-OK answer, returning `{status, reason?,
  marking_version}`.
- The page: `pressing: string | null` (the control in flight), `pressNotice: string |
  null`. A press's answer: `pressed` clears the notice; `nothing` sets it to `"<label>:
  <reason>"`; an error sets it to `"<label>: <message>"`. The marking follows through the
  event stream as for `inject`; no state re-read.

### 3.3 The legend

- `ControlsLegend.svelte`, rendered as an overlay at the top-left of the graph panel
  (the panel already has an absolute box top-right for zoom), only when
  `mirror.topology.controls.length > 0`. A header `Controls (n)` with a fold chevron;
  folded by default? No: open by default, folded state kept in page state for the session.
  It matches the Bevy viewer's placement; its look follows the page's card and button
  classes.
- One row per control in declaration order: the label and the widget by kind, native
  inputs: `button` a button; `toggle` a checkbox-styled button showing its state;
  `slider` `<input type="range">` with `min`, `max`, `step` from `config` (defaults 0,
  100, a hundredth of the range) and the number beside it, sending on `change`; `select`
  a `<select>` of `options`, sending on change; `text` an `<input type="text">` with the
  placeholder and a send button, Enter sends. Initial values from `config.value`.
- Lighting: a `fire` row is dimmed (opacity) while its node is not in the fireable set
  and still sends (the runtime answers `nothing`); an `enable` row shows the transition's
  `enabled` flag; others are plain. A row is disabled while its press is in flight.
- The notice line sits under the rows in the page's inline-error style
  (`text-destructive` for errors, muted for `nothing`), cleared by the next press.
- No keyboard shortcuts in this round: the page has none and browser keys are taken.
- Hover: resting on a row adds a `data-hover` to its node's `<g>` and the graph's existing
  hover style applies, if the page has one; otherwise nothing.

### 3.4 Tests

- `netView.test.ts`: the traffic light fixture gains `controls` and `Light`'s style and a
  `color` on the light's token; tests that `mirrorFromState` keeps controls and styles,
  that `drawNet` gives `Light` its token's colour and no fill with two tokens or none or
  no colour, and that the fixtures without controls draw as before.
- `tests/ui/fake_cp.py`: `net_state(..., controls=None)` serves `topology.controls`; places
  given with `style` pass through; the write regex gains `press`; the handler finds the
  control, checks the value by kind (`invalid_params`), and acts by `action`: `inject`
  puts a token into `node` (named by the test's `makers` map, a callable per control
  giving `(label, color?)` from the value, so a car can be `"Car N"`), `fire` fires `node`
  through the existing `fire` helper when fireable or answers `nothing` with a reason,
  `enable` toggles; an unknown control is `unknown_control`. `FakeControlPlane` records
  presses in `cp.presses`, reset in the fixture.
- `tests/ui/test_net_protocol.py`: the traffic light with controls and the light's colour:
  the legend lists two rows; pressing `+ Car` with a typed name adds a car (counts and the
  log); pressing `Change` fires it and the light's circle fill changes to the next colour
  (read the `--place-fill` style or the circle's computed fill); a fire control on a held
  transition (add a `go` fire control in this test's net only) shows the `nothing` notice;
  a press answered with a refusal shows the error line; the legend folds and unfolds; a
  net without controls shows no legend.
- `npm run check` clean of new errors; the a11y warnings count unchanged or lower.

### 3.5 Docs

`implementation_docs/PROTOCOL_V1_FRONTEND.md` gains "Controls and token styles in the page
(2026-10-16)" in the established pattern (Status; What the wire gives the page; Decisions;
Tests), and its line "The page has no legend" is corrected. `COMS.md`'s call/answer table
gains `press` and the refusal `unknown_control`.

## 4. Order of work and rules

1. Server plus the Bevy HTTP transport: one agent, branch `press-route` in both repos.
   petritype-server commits carry no attribution lines at all (the user's convention);
   petritype-bevy commits carry the usual two trailers. The server tree holds one
   uncommitted doc edit of the user's: never stage, stash, revert or commit it. Never run
   anything under `scripts/`, never deploy, never touch a worker; `uv run pytest` and the
   e2e module with local checkouts only.
2. petrivelte: one agent in parallel, branch `net-controls`, usual trailers. Browser tests
   with the pinned offline Playwright.
3. Bases: each branch starts from `main` where the earlier branches are merged, else from
   the current tip (`net-controls` for Bevy, `traffic-light` for petrivelte).
4. Verification by the planning thread: every gate re-run, and one end-to-end pass of the
   Bevy HTTP viewer and the nets page against a local server with the traffic light, if a
   local server can be run without touching production (to confirm with Oleg).

## 5. Open points for Oleg

1. The legend on the web as an overlay top-left of the graph (matching Bevy) rather than a
   row above it.
2. Dim-but-still-sends for a held fire control on the web, as in Bevy, rather than a
   disabled button.
3. The production dependency: the HTTP path needs the next petritype PyPI release to carry
   `press`.
4. Whether a local server run for the end-to-end look is something you want this round,
   and how you run it (so the brief never guesses at production).
