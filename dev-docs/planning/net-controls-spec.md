# Spec: net-declared controls and token styles

Draft for review, 2026-10-16. Nothing here is built.

## 0. The principle

A net declares, as data, the buttons it wants and how its tokens look. A viewer renders
that data with widgets it already has and never names a net. Pressing a button sends
one request, `press`, and the runtime that owns the net executes it with code written
beside the net. Bespoke code lives with the net, in the net's language. The libraries
(protocol, both runners, the Bevy viewer) gain one vocabulary and no net-specific code.

Decided in discussion: runtime-executed controls with a declared kind and node; a value
typed in the viewer travels with the press; a foldable legend inside the net view holds
the controls, the bar keeps its usual buttons; mirrored buttons at the affected nodes are a
later slice; token style is a hook whose output is data on the wire, colour first, glyph
and image later; a place may be tinted by its token's colour by a rule the viewer
evaluates; in Rust the style comes from a trait on the token type, opted in at declaration.

What already exists and is reused:

- Python has `ControlSpec(node, kind, label, to_command, config)` in `petritype/runtime.py`
  with kinds `button | toggle | slider | select`, a control map `{name: ControlSpec}` kept
  off the net, `apply_control`, and a marimo renderer (`petritype/marimo_controls.py`),
  used by `examples/interactive/parcel_sorter.py`. The wire form below is this object.
- The wire already carries `color?` on token entries, `added` and `produced`; Python fills
  it from a token's `.color` attribute; the Bevy viewer draws it (DESIGN §3.7). The Rust
  runner writes `null` in its place and an in-process net gives none.
- Rust core has `Net::put`, `Net::fire(transition)` (refuses with `NotFireable`, net
  untouched), `Command::{put, extend, enable, disable, set_param}` and `Net::apply`, and a
  per-place vtable `PlaceMeta` whose `keys` entry is an `Arc<dyn Fn(&dyn Any) -> ...>`,
  the pattern a style function follows.

## 1. The wire (PROTOCOL.md)

Optional fields and one new method and error code; the protocol version stays 1
(§10: readers ignore what they do not know).

### 1.1 Controls in the topology (§5.1)

`topology.controls?: [control]`, in declaration order, absent when the net declares none.

```
control = {name, label, kind, node?, action?, config?}
kind    = "button" | "toggle" | "slider" | "select" | "text"
action  = "inject" | "fire" | "enable"
config  = {value?, options?, start?, stop?, step?, placeholder?}   (by kind, below)
```

- `name` is unique within the net and is what `press` names. `label` is shown.
- `kind` is the widget and fixes the value a press carries: `button` none; `toggle` a
  boolean; `slider` a number within `config.start..config.stop` (step `config.step`);
  `select` one of `config.options` (strings); `text` a string (`config.placeholder`
  is the hint shown while empty). `config.value` is the initial value where the kind has
  one. Other `config` keys are a runtime's own (marimo's button `kind`) and a viewer
  ignores them.
- `node?` names a place or transition the control acts on. It is for the viewer: the
  legend lights the node on hover and, in the mirrors slice, puts the button there.
- `action?` says what kind of thing the press does, for the viewer's lighting rule only:
  `fire` is lit while `node` is fireable; `enable` is a toggle whose state is `node`'s
  enabled flag; `inject` and absent are always lit. A viewer never executes an action.

### 1.2 Pressing (§6)

```
| `press` | `{control, value?, path?}` | `{status: "pressed" | "nothing", reason?, marking_version}` | runs the control; emits the events of whatever it did |
```

- `value` must match the kind (absent for `button`); otherwise `invalid_params`. An
  option not in `options`, or a number outside `start..stop`, is `invalid_params`.
- `status: "pressed"` when the control applied something; `"nothing"` with a `reason`
  when it chose not to (its `to_command` returned nothing, or a `fire` found the transition
  not fireable: `"Change is not fireable"`). A handler that raises is `internal`.
- New error code `unknown_control`.
- Controls are outer-level in this round: `press` joins `OUTER_ONLY`, so a `path` is
  `invalid_params`.
- Effects arrive as the usual notifications (`added`, `fired`, `toggled`, ...) and
  `fireable` when the set changed. The answer carries the `marking_version` after them.

### 1.3 Token style

`color?` as today. The Rust runner now sends it when the place has a style (§3), in token
entries, `added`, and as the fourth element of `produced`. The sentence in §11 that a
shared transcript "uses no `color`" becomes: a shared transcript carries `color` only
where both runtimes give the token the same colour, which the conformance nets do by
giving hex values, never names.

Reserved for round two, defined now so the hook has a shape: `glyph?` (one grapheme the
viewer draws on the token) and `image?` (a name into `topology.assets?`, an asset table a
net declares once). Both ride as fields on token entries and `added`; `produced`'s
positional form needs a sixth element or an object form, decided when built.

### 1.4 Place style

`topology.places[].style?: {fill: "token"}`, absent when the place has none. The rule the
viewer evaluates: while the place holds exactly one token and that token has a colour,
the place is filled with that colour; a firing's input or output highlight wins while it
lasts; otherwise the theme's fill. No traffic on marking changes; the viewer already has
the token's colour.

### 1.5 Conformance

`traffic-light` (§11) declares two controls and styles its light (§5). The transcript
`08_traffic_light.json` is re-recorded in both runtimes together, as always: `get_state`
now shows `controls` and the place style, the light's tokens carry `color`, and three
exchanges are added at the end: `press change` (fires `Change`), `press car` with
`value: "Taxi"` (an `added` in `Waiting`), and `press car` with `value: 7`
(`invalid_params`). The other transcripts are untouched: their nets declare nothing.

## 2. Python

- `ControlSpec` gains `kind: "text"` and an optional `action: Optional[str] = None`.
  Constructors on the class, each setting `node`, `action` and `to_command`:
  - `ControlSpec.inject(place, make, label, kind="button", config=None)`: `make(value)`
    returns the token to add; `to_command` returns `Extend(place, [make(value)])`.
  - `ControlSpec.fire(transition, label)`: returns a new runtime command `Fire(transition)`.
  - `ControlSpec.enable(transition, label, value=True)`: a toggle returning `Enable` or
    `Disable`.
  Hand-written specs keep working; without `action` they are always lit.
- `Fire(transition)` is a new `Command` in `petritype/runtime.py`. `Runner.drain_inbox`
  fires that transition if fireable through `fire_one_transition` with a pinned selector
  (as the Owner's `_fire` does today), else does nothing. The Owner routes a pressed `Fire`
  through its own `_fire` path so events, progress and `fireable` follow as for `step`.
- `@petri_net(..., controls=None, place_styles=None)`: `controls` is the control map (or
  a zero-argument callable returning one); `place_styles` is `{place: PlaceStyle}` with
  `PlaceStyle(fill="token")`. `NetEntry` carries both; the Owner serves them in
  `topology`. Kept off the net, as `ControlSpec` always was.
- `_press(request_id, params, now, out)` in the Owner: look the control up
  (`unknown_control`), check the value by kind (`invalid_params`), call `to_command`,
  apply the command through `_apply` (or the fire path), answer `pressed` or `nothing`.
- The wire control is the `ControlSpec` minus `to_command`: a `Control` pydantic `Wire`
  model in `protocol.py`; `Place` gains `style?`.
- `marimo_controls.py` renders the new `text` kind (`mo.ui.text` plus a run button) so the
  notebooks and the viewer agree on the vocabulary. Nothing else there changes.
- Token colour needs no new mechanism. The traffic light's colours become a `Colour(str)`
  subclass whose `color` property gives the lamp's hex (`red #ff6347`, `green #32cd32`,
  `amber #ffa500`, the notebook's `LAMPS`), so the JSON form stays `"red"`. To check first:
  that `JsonForm.decode(Colour, "red")` builds a `Colour` on `inject`; if it cannot, the
  fallback is a one-field model `Lamp(colour=...)`, and the Rust side mirrors it.
- The example `examples/edges_on_empty_places/traffic_light.py` declares:
  - `car`: `ControlSpec.inject("Waiting", make, "+ Car", kind="text",
    config={"placeholder": "name"})`, where an empty value names the car `Car N` with `N`
    one more than the cars in `Waiting` and `Passed` together (stateless, so both runtimes
    agree), and a non-empty value is the name.
  - `change`: `ControlSpec.fire("Change", "Change")`.
  - `place_styles={"Light": PlaceStyle(fill="token")}`.
  The junction notebook keeps its `lit()` tint (it draws through graphviz, not the wire).

## 3. Rust

### 3.1 Core (`petritype`)

New module `style`:

- `pub struct Rgb(pub u8, pub u8, pub u8)` with `Rgb::hex("#ff6347") -> Option<Rgb>` and
  `Display` as `#rrggbb`. `pub struct Style { pub colour: Option<Rgb> }`, `#[non_exhaustive]`,
  `Default`; glyph and image join it in round two.
- `pub trait TokenStyle { fn style(&self) -> Style; }`.
- Builder: `style<T: 'static>(&mut self, place: PlaceRef<T>, f: impl Fn(&T) -> Style + Send
  + Sync + 'static)` stores an `Arc<dyn Fn(&dyn Any) -> Style + Send + Sync>` in
  `PlaceMeta.style: Option<StyleFn>` (the `keys` pattern). `styled<T: TokenStyle +
  'static>(&mut self, place: PlaceRef<T>)` is `self.style(place, T::style)`: the one line
  that opts a typed place in, since places have no trait bound and Rust has no
  specialization to default one.
- `Net::style(place: PlaceId, token: TokenId) -> Option<Style>`: `None` when the place has
  no style function or the token is not there.
- `pub struct PlaceStyle { pub fill: Fill }` with `enum Fill { Token }`, `#[non_exhaustive]`;
  builder `place_style(place, PlaceStyle)`; `PlaceInfo.style: Option<PlaceStyle>` and
  `TopologyPlace.style` (serde: `{"fill": "token"}`).

New module `control`:

```rust
pub enum ControlKind { Button, Toggle, Slider { start: f64, stop: f64, step: f64 },
                       Select { options: Vec<String> }, Text { placeholder: Option<String> } }
pub enum Action { Inject, Fire, Enable }
pub enum Value { None, Bool(bool), Number(f64), Text(String) }   // Select carries Text
pub enum Pressed { Applied, Nothing(String) }
pub struct Control { name, label, kind, node: Option<Node>, action: Option<Action>,
                     initial: Option<Value>, handler: Arc<dyn Fn(&mut Net, Value)
                         -> Result<Pressed, String> + Send + Sync> }
```

- Constructors: `Control::button(name, label, f)`, `::text(name, label, placeholder, f)`,
  `::toggle(name, label, initial, f)`, `::slider(name, label, start, stop, step, initial,
  f)`, `::select(name, label, options, initial, f)`; and the actions `Control::inject(name,
  label, place: PlaceRef<T>, make: impl Fn(Value) -> T)` (kind by a builder method, button
  by default), `Control::fire(name, label, transition)` (a button; `Nothing` with
  `"<name> is not fireable"` when `Net::fire` refuses), `Control::enable(name, label,
  transition)` (a toggle).
- `Node` is `Place(PlaceId) | Transition(TransitionId)`.
- Builder `control(&mut self, Control)`; a duplicate name is a `BuildError`. The net
  carries them: `Net::controls() -> &[Control]` and `Net::press(name, Value) ->
  Result<Pressed, PressError>` (`UnknownControl`, `WrongKind`, `Refused(String)`). `press`
  clones the handler's `Arc` before calling it with `&mut self`. Core stores and runs
  controls; it does not interpret `kind` or `action`, and an in-process viewer or runner
  reads them as data. This is the one point where Rust differs from Python's "off the net":
  the `Net` is the object both the runner's factory and the viewer's `NetRes` hand over,
  so it is the carrier.
- The handler mutates the net through the public API (`put`, `fire`, `enable`), so every
  effect is an ordinary `Event`.

### 3.2 Runner (`petritype-runner`)

- `topology.controls` and `places[].style` from `net.controls()` and `PlaceInfo.style`.
- `press` in `METHODS` and `OUTER_ONLY`: decode `value` by the control's kind
  (`invalid_params`), `net.press`, then `flush_events` as `inject` does; `Nothing(reason)`
  answers `status: "nothing"`. Errors map: `UnknownControl` to `unknown_control`.
- Wire `Token` gains `color?`; `Produced` writes the colour string in fourth place when
  `net.style` gives one (and `null` only with keys, as today); `added` carries it.
  `Mutated` has none (the protocol gives it none).
- The transcript harness needs no change; the re-recorded transcript matches both.

### 3.3 Example (`petritype/examples/traffic_light/net.rs`)

The light's token becomes a type the example owns, `Colour` (an enum `Red | Green | Amber`
serialising as the lowercase string, so the wire stays `"red"`), implementing `TokenStyle`
with the three hexes above; `Light` is `.styled(light)` and `.place_style(light,
PlaceStyle::fill_token())`. `next_colour` and `colour(net, light)` follow the type. Two
controls as in §2 (`car` text, `change` fire), same names, labels and behaviour, so the
shared transcript pins them.

## 4. The Bevy viewer

### 4.1 Reading

- `NetView::colour` for a local source reads `net.style(place, token).colour` (today
  `None`). The mirror's `TokenColour` path is unchanged, so in-process tokens colour as
  remote ones do.
- `NetView::controls() -> Vec<ControlInfo>` for both sources (`name, label, kind, node,
  action, initial`); the remote `NetDescription`/`State` gain `controls` and `Place.style`.
- `NetView::place_style(PlaceId) -> Option<PlaceStyle>`.
- Pressing: `NetEvents::press(name, Value)`: local, `net.press` and the result; remote,
  `request("press", ...)` with a new `Pending::Press` whose answer becomes
  `Outcome::Press { control, status, reason }` for the drive to show. Both answer within
  the frame or on the next pump, like `step`.

### 4.2 Drawing

- Place tint: `restyle_places` gives a place with `style.fill == Token` its own material
  (one per colour, as `token_fills` are) while it holds one coloured token; input/output
  highlight materials win while they apply; the theme's idle fill otherwise. The outline
  gizmo is unchanged.
- Nothing new is drawn on tokens in this round (colour exists). Glyph and image follow
  the wire fields when round two comes: a `Text2d` at the centre, a sprite from an asset.

### 4.3 The legend

- A `LegendPlugin` (`src/legend.rs`) and resource `Legend { open: bool }`. It exists only
  while the net declares at least one control; a net without controls shows nothing.
- Position: a floating panel at the canvas's top-left, under the bar (`top: BAR_HEIGHT,
  left: 0`, as the filter bar is), absolute like `SpeedPanelRoot`, `FocusPolicy::Block`,
  above the canvas and below tooltips and the speed panel. The bar's buttons are above it
  and reading order puts the net's own controls next. While the filter bar is open it
  shifts right by `FILTER_BAR_WIDTH`, so both stay visible; the inspector keeps the right.
  Folded, it is a one-line header, `Controls (2)`, with a fold triangle; `L` (free in
  `controls::KEYS`) and a click toggle it. Open by default on load.
- One row per control, in declaration order: the key (digits `1`..`9` for the first nine;
  none after), the label, the widget:
  - `button`: a bar-style button; press sends `press {control}`.
  - `toggle`: a lit/dim button; press sends the flipped value. With `action: enable` its
    lit state is the transition's enabled flag from the view, not the last value sent.
  - `slider`: the speed panel's track and number box, bounds from `config`; releasing the
    track or `Enter` in the box sends the value.
  - `select`: the filter bar's tab buttons, one lit; a click sends that option.
  - `text`: a draft box like the speed box but accepting printable characters, with a
    small send button at the row's end; `Enter` or the button sends the text, which stays in
    the box; `Escape` restores the last sent value.
- Keys: a digit presses the row's button (for `toggle` flips it, for `select` cycles, for
  `text` and `slider` focuses the box). While a draft is editing, keys go to the draft (the
  speed panel's rule). The bar's keys are untouched.
- Lighting: `action: fire` rows are lit while the node is fireable and dim otherwise, and a
  press while dim is still sent (the runtime says `nothing`); `enable` rows show the
  enabled flag; all others are lit. Read-only mode (the HTTP viewer without a token) dims
  every row and sends nothing, as the bar does for Step.
- Hover: resting on a row sets `Hovered` to its node, so the canvas lights the place or
  transition as the inspector does; resting on a node with controls lights its rows.
- Answers: `nothing`'s reason and any error message go to a timed note on the status line
  (`OuterNotes`, 3 s), with the control's label: `Change: Change is not fireable`. This is
  the first general status note from a request; the drive's step and reset refusals, logged
  only today, may adopt it later and are not in this round.
- Inner nets: the legend shows the outer net's controls only and stays while descended,
  like the bar (DESIGN §10).

### 4.4 Example and docs

- `examples/traffic_light.rs`: the in-process traffic light, `Direction::LeftRight`,
  resettable through `NetFactory`, cars arriving by the `+ Car` control and the light
  changed by `Change` or by Step. This closes the earlier open question: no arrival timer,
  the controls are the input.
- README's controls table gains the legend and `L`; "Not here yet" drops the net-specific
  controls line. DESIGN gains slice 14 (styles and the legend) with the decisions: the
  viewer never executes a control; the place tint is a viewer rule; the legend is the single
  source and mirrors are shortcuts.

## 5. Mirrors (the slice after)

Sketch only, to be specified when the legend is in: a control whose `node` is a place or
transition gets a small badge at that node (the nested-net badge pattern, a disc at the
node's top-right with the count), folded by default; a click unfolds the node's rows beside
it as UI positioned from the node's world position each frame (the crate has
`viewport_to_world_2d` only; the inverse is needed). Hover links as in §4.3.

## 6. Tests

- Python: unit tests for `ControlSpec`'s constructors, `Fire`, `_press` (each kind's
  value check, `unknown_control`, `nothing` on a held fire), the wire `Control` model,
  `place_styles` in `topology`, the `Colour` type's colour and JSON round trip; the
  example's tests cover the two controls; the transcript re-recorded.
- Rust: core tests for `style`/`styled`, `Net::style`, `place_style`, `Control` constructors
  and `Net::press` (kinds, unknown, refused, handler sees the net); runner tests for
  `press` dispatch, `color` in `produced`, `added` and token entries; the transcript replays
  in both runtimes (`sha256` identical copies as before).
- Bevy: under `MinimalPlugins` with `ScriptedTransport`: controls parsed from `get_state`,
  rows built in order, each kind's press sends the right `press` line, a `nothing` answer
  lands in the note, `fire` rows light from `fireable`, the tint material follows the light;
  in process: a net with controls pressed through the legend, the view updating, colour on
  local tokens; both runners over the wire (`remote_process.rs`): the traffic light's
  `+ Car` and `Change` pressed and the marking checked, the light's colour read.
- petrivelte: not in this round. Its nets page ignores `controls` (unknown fields are
  ignored) and already draws `color`.

## 7. Order of work

1. Protocol, Python, Rust, transcript: three briefs as for the traffic light rounds
   (protocol text with the Python track; the Rust track waits for the transcript to name
   `press`). Branch `net-controls` in each main checkout, unmerged.
2. Bevy: styles, the legend, both press paths, the in-process example, docs. Depends on 1
   for the wire tests only; the in-process parts can start on the Rust branch at once.
3. Mirrors, specified then.
4. Later, separate: petritype-server gets a `press` route and relays the answer;
   petrivelte renders the legend; the game's nets declare their controls.

## 8. Open points for Oleg

1. One value per control. A control with several parameters (a parcel with weight and
   destination) is a `form` kind, value an object, rows with several widgets: defined
   later, not in this round. Enough for now?
2. Top-left agreed (2026-10-16), yielding to the filter bar. `L` as its key.
3. Rust core carrying `Control` and `Style` on the `Net` (my recommendation, §3.1) rather
   than a side table the runner and the viewer would both need.
4. Python `Colour(str)` for the light, with the `Lamp` model as the fallback if `inject`
   cannot decode a `str` subclass. The Rust enum follows whichever is chosen.
5. Round two's `glyph` and `image` on `produced`: sixth element or object form.
