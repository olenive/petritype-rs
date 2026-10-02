# petritype-rs — Design

Typed, executable, observable Petri nets in Rust. A port of the Python
[petritype](../petritype) library, redesigned around ownership instead of
runtime reflection.

Status: design settled 2026-09-19, no code yet. This document is the reference
for the first implementation. Vocabulary is fixed in
`dev-docs/NAMING_CONVENTIONS.md`. Open questions are listed at the end.

---

## 1. Goals

- **Easy to adopt.** Turning existing functions and types into an executable
  net should need derives, not rewrites. Few decisions per transition; the
  compiler catches wiring mistakes.
- **Typed.** Every place holds tokens of one Rust type. Wiring mistakes are
  compile errors, not runtime errors.
- **Executable.** Transitions are ordinary Rust closures. The net *is* the
  program.
- **Observable.** Every token has a stable identity. Every change is an
  event. Any visualiser (Bevy, egui, Graphviz, a web page) can mirror the
  net's state cheaply and inspect individual tokens.
- **Runtime-agnostic core.** No async runtime, no game engine, no I/O. Firing
  is exposed as three separable stages so any driver (a `for` loop, a thread
  pool, a Bevy system, tokio) can run it.
- **Public crate.** Stable API on its own semver; Bevy churn is isolated in an
  adapter crate.
- **Lightweight** in API surface and dependencies (core: `thiserror` only).

Non-goals for v1: persistence of markings, distributed nets, edge weights
beyond one/all, timed Petri net semantics, topology editing at runtime
(designed for, not built).

## 2. Relationship to Python petritype

Kept, with the same meaning:

| Python | Rust |
|---|---|
| FIFO token extraction | `VecDeque::pop_front` |
| `list[T]` argument = take all tokens | `TakeAllEdge` |
| `SnapshotEdge` / `MutateEdge` | same |
| presence required on every input edge | same |
| type-based output routing for distinct types | routed output form, resolved at build time |
| `return_index` on tuple returns | positional output form |
| `output_distribution_function` | `Distribution` output form |
| three-stage fire | `launch` / `run` / `distribute` |
| concurrent mode: consume synchronously, distribute later | same |
| one flight per transition at a time | same |
| guard, priority, selector | same |
| Enable / Disable | same, stored in the net |
| consumed tokens lost on failure by default | same, `Failed` event records ids |
| `restore_tokens_on_failure` | same, per transition |
| `assert_acyclic`, `assert_no_source_transitions` | build-time opt-in checks |
| fuse (`error_after_n_firings`), `RunSummary.quiesced` | driver features |
| `step`, `run`, `run_indefinitely` | same |
| tokens in places are the single source of truth; UI never mutates directly | enforced by `&mut Net` |
| `kwargs`, `SetParam` | `Param::<T>` declared on the transition, `b.param`, `net.set_param` (§10) |

Dropped, and why:

| Python | Reason |
|---|---|
| `isinstance` routing, subclass matching | no subtyping; enums replace unions |
| list-at-scalar-place "batch" heuristic, empty-list-matches-everything | `Vec<T>` and `T` are distinct types; `Vec::new()` knows its `T` |
| `allow_token_copying`, deepcopy, `id(token)` bookkeeping | ownership: no aliasing unless the author writes `Arc` |
| `Optional[...]` place types to make `None` routable | slot type `Option<T>`; place type stays `T` |
| `execution="thread"`, async/sync body distinction | core is sync; drivers decide where bodies run |
| observer registry, `RunContext` | the driver holds `&mut Net` and is the observer |
| `transition_history`, place histories, `token_history` | event log |
| `last_fired`, `fired_counts`, `fired_since` | derivable from the event log |
| AST extraction, relationship graph, `@petri_net` registry | out of scope |
| pydantic validation | the compiler |
| `strict` flag | `Result` |


## 3. Vocabulary

- **Place**: a typed FIFO queue of tokens.
- **Token**: one owned value of the place's type, with a `TokenId`.
- **Transition**: a body (closure) plus input edges, output edges, optional
  guard and priority, and an enabled flag.
- **Edge**: a directed connection. Input edges have a *kind*; output edges are
  positional.
- **Marking**: the set of all tokens in all places.
- **Enabled**: the author's on/off switch on a transition (`enable`,
  `disable`, `Toggled`).
- **Fireable**: a transition that is enabled, not in flight, whose every
  input place is non-empty and whose guard passes.
- **Firing**: consume inputs, run body, distribute outputs.
- **Quiescent**: nothing fireable and nothing in flight.

## 4. Representation

### 4.1 Identifiers and counters

```rust
pub struct PlaceId(u32);        // erased; used in events and the read API
pub struct TransitionId(u32);
pub struct TokenId(u64);        // monotonic per Net

pub struct NetId(u32);          // process-unique, minted per NetBuilder

pub struct PlaceRef<T> { place: PlaceId, net: NetId, _t: PhantomData<fn() -> T> }  // typed handle, 8 bytes
```

- Ids are **never reused**. Consumed tokens retire their id. Removed places or
  transitions (future) leave tombstone slots so other indices don't shift.
- `PlaceRef<T>` is `Copy`, `Send`, `Sync` regardless of `T` (the `fn() -> T`
  phantom is deliberate). `.id()` gives the erased `PlaceId`.
- `Net` carries two change counters, plain `u64`s that increment by one:
  - `topology_counter`: connectivity changes only (place, transition, edge
    added or removed). v1 never bumps it.
  - `state_counter`: everything else — token added, removed, mutated,
    transition toggled. A change tick for observers, not a firing count:
    a freshly built net is at 0 however it was seeded.
  - `fired_count`: number of completed firings. A statistic, not a counter of
    changes.
  - `emit` bumps `state_counter` once per event: a firing bumps it by one
    (`Fired`) plus one per `MutateEdge` (`Mutated`); a failed firing likewise
    (`Mutated`s then `Failed`); a toggle by one.
- `PlaceRef` is stamped with its net's `NetId` (an `AtomicU32` counter, one
  per process; not stable across runs, never serialised). Every method that
  takes a `PlaceRef` compares the stamp first, so a handle from another net
  cannot index this one: infallible methods (`put`, `seed`, `transition`, ..)
  panic with both net ids in the message; fallible ones return
  `NetError::NoSuchPlace`. Needed because composition (§6.6) puts two nets'
  handles in one scope. `PlaceId` and `TransitionId` stay unstamped.

### 4.2 Places and tokens

Typed at the API boundary, erased inside:

```rust
struct PlaceSlot {
    meta: PlaceMeta,
    tokens: VecDeque<(TokenId, Box<dyn Any + Send + Sync>)>,
}

struct PlaceMeta {
    name: String,
    type_name: &'static str,
    type_id: TypeId,
    debug: fn(&dyn Any) -> String,
    clone: Option<fn(&dyn Any) -> Box<dyn Any + Send + Sync>>,   // None only for no_clone places
    #[cfg(feature = "serde")]
    to_json: Option<fn(&dyn Any) -> serde_json::Value>,          // Some iff marked serializable
    #[cfg(feature = "serde")]
    from_json: Option<fn(serde_json::Value) -> Result<Box<dyn Any + Send + Sync>, _>>,
}
```

`PlaceMeta` is a hand-rolled vtable: monomorphised generic functions
(`debug_of::<T>`, `clone_of::<T>`) coerced to plain `fn` pointers when the
builder knows `T`. Zero per-token overhead, no trait bound on the token beyond
what the builder method requires.

### 4.3 Token bounds

Universal bound: `T: Debug + Clone + Send + Sync + 'static`.

- `Debug`: labels for visualisers.
- `Clone`: snapshot edges, `Net::snapshot()`, `restore_tokens_on_failure`.
  Never called unless one of those is used.
- `Send + Sync`: bodies may run on other threads; the net lives inside a
  Bevy `Resource`, which requires both. Rejects `Rc`, `RefCell`, `Cell`.
- `'static`: tokens are owned data, not borrows.

Escape hatch: `place_no_clone::<T>()` drops the `Clone` bound (for `File`,
sockets, handles). Such a place cannot be the source of a `SnapshotEdge` or
a consuming input to a `restore_tokens_on_failure` transition (build errors;
a `MutateEdge` from it is fine), and makes `Net::snapshot()` return `Err`.

Big data idiom: put heavy fields in `Arc`. Moves are free (a `Vec` is 24
bytes to move); `Arc::clone` is a counter bump; `Arc::make_mut` is
copy-on-write. Shared *mutation* requires `Arc<Mutex<_>>`, which is visible
in the place's type.

### 4.4 Transitions and edges

```rust
struct TransitionSlot {
    name: String,
    inputs: Vec<(PlaceId, EdgeKind)>,      // EdgeKind::{Take, TakeAll, Snapshot, Mutate}
    outputs: Vec<PlaceId>,                 // positional; also the target set for routed / Distribution
    // no routing field: a routed body's TypeId -> output index table is resolved from types when the
    // body is wrapped (transition()) and captured by the erased closure; problems surface at build()
    body: Option<Box<dyn FnMut(ErasedInputs) -> ErasedOutcome + Send + Sync>>,  // None while in flight
    guard: Option<Box<dyn Fn(&Net) -> bool + Send + Sync>>,
    priority: Option<Box<dyn Fn(&Net) -> f64 + Send + Sync>>,
    enabled: bool,
    restore_tokens_on_failure: bool,
    detachable: bool,                      // false iff any input is a MutateEdge
}
```

Input edge kinds and the body parameter each produces:

| kind | consumes? | parameter type | presence required |
|---|---|---|---|
| `TakeEdge(p)` | one token, FIFO | `T` | yes |
| `TakeAllEdge(p)` | every token | `Vec<T>` | yes |
| `SnapshotEdge(p)` | no | `T` (clone of the front token) | yes |
| `MutateEdge(p)` | no | `&mut T` (front token, in place) | yes |

A transition reads each place through at most one input edge
(`DuplicateInputPlace` at build), so fireability is exactly "every input
place non-empty" and `MutateEdge` borrows are disjoint by construction.

Bodies are `FnMut`: they may hold private mutable state (counters, RNGs).
Anything captured must be `Send + Sync`; use `move`, `Arc`, `Mutex`, atomics.

### 4.5 Outputs

Output places are declared as a tuple in the builder (topology needs the
list). The body's **return type** selects one of three forms:

**Positional** — the return type is a tuple. Slot *i* goes to output place
*i*. Each slot is `T`, `Option<T>` or `Vec<T>` for that place's `T` (trait
`IntoTokens<T>`). Checked at compile time. A wrong order between
*different* types fails to compile; between *same* types order is the only
distinguishing information and the author must care anyway.

```rust
(heavy, light, count)  ⇐  |ps: Vec<Parcel>| -> (Vec<Parcel>, Vec<Parcel>, Option<u32>)
```

**Routed** — the return type is not a tuple. Each emitted payload goes to
the unique output place of its type. Order of output places is irrelevant.
Trait `RoutedOutput` (`petritype::routing`) lists the payload types
(`describe`) and emits them (`emit`); implemented by `#[derive(Routed)]` on
an **enum** (each single-field tuple variant `V(P)` sends `P` to the place
of type `P`; a unit variant emits nothing) or on a **struct** (a leaf: the
whole value goes to the place of its own type), by `Either<A, B>` (our own
enum), and by `Option<T>` / `Vec<T>` whose `T: RoutedOutput` (`None` emits
nothing; a `Vec` is always a batch). Payloads are leaves: `Batch(Vec<Job>)`
is one token at a `Vec<Job>` place. There is deliberately **no impl for a
bare `T`**: it would overlap `Option<T>` / `Vec<T>` under coherence and make
`Option<u32>` ambiguous at the call site (E0283). A bare std value uses the
positional 1-tuple `(value,)`; a bare user type derives `Routed`.

```rust
(successes, uncertain)  ⇐  |d: ScrapedData| -> Outcome   // enum Outcome { Ok(SuccessfulScrape), Unsure(UncertainScrapeResult) }
```

Build-time checks for routed bodies (`BuildError` naming transition and type):
- every payload type has exactly one output place of that type
  (`UnroutableOutput` / `AmbiguousOutput`: use positional or a newtype);
- every output place is reachable by some payload type (the diagram must not
  lie).
- The `TypeId -> output index` table is resolved from types alone when the
  body is wrapped, so `transition()` stays infallible; the first problem per
  transition is kept by the builder and reported by `build()`.

**Distribution** — the body, or an *output distribution function* composed
after it, returns a `Distribution`: typed `(place, token)` pairs. The escape
hatch for data-dependent routing among same-typed places, dynamic fan-out, or
anything else.

The output distribution function exists so that **domain functions stay
unchanged**: `make_a_ball` knows nothing about places; the routing decision
lives next to the net, where the place handles are in scope, and can change
with the net's connectivity without touching domain code. It takes the body's
result only (not the inputs), runs inside stage 2 with the body (so off-thread
in concurrent mode), and may return any output form, not only a
`Distribution`. When present, *its* return type decides the form and the
body's return type is merely its input. It is composed onto the **body**,
`body.output_distribution(f)` (extension trait `OutputDistribution`), before
the pair is handed to `transition()`: by the time `transition()` returns the
body's return type has been erased, so there is nothing left to compose. If
the body returns `Result<R, E>`, `f` takes `R` and an `Err` short-circuits to
a failure.

```rust
b.transition("Make a Ball", (TakeEdge(rubber), TakeEdge(paint)), (red, blue, other),
    make_a_ball.output_distribution(move |ball: Ball| {
        let target = match ball.color.as_str() { "red" => red, "blue" => blue, _ => other };
        Distribution::new().to(target, ball)        // to(PlaceRef<Ball>, Ball): type-checked; chainable
    }));
```

Type mismatches are still compile errors. That the place is a declared output
of *this* transition (and of this net) is checked at `distribute` time, before
anything is written; a bad pair fails the whole firing (`DistributionError`
inside `FiringFailed`), the net stays consistent, tokens are lost as in any
failure. There is no build-time reachability check for this form: a
`Distribution` may never name some declared output place, and the net still
builds.

Vocabulary for one transition: **body** (domain function), **output
distribution function** (author-written, optional), **`Distribution`** (the
value it returns), **`distribute`** (engine stage 3, internal; applies whatever
was returned, in every form).

Rules common to all forms:
- `Result<Outputs, E>` always means failure (§7), never routing. Two-way
  routing uses `Either<A, B>` or a derived enum.
- Zero output places ⇒ the body must return `()`. A non-unit return with no
  outputs is a build error, not a silent discard.
- Every produced token gets a fresh `TokenId`. Consumed ids die. Lineage is
  implicit in the `Fired` event.
- Fan-out of one value to two places is an explicit `.clone()`. A
  `Broadcast(value)` helper (clone into every output place of that type) is
  deferred until asked for.

## 5. Builder API (the author's view)

```rust
use petritype::{NetBuilder, TakeEdge, TakeAllEdge, SnapshotEdge, MutateEdge};

#[derive(Debug, Clone)]
struct Parcel { id: u32, weight: f32 }

#[derive(Debug, Clone)]
struct Config { max_weight: f32 }

let mut b = NetBuilder::new();

let inbox  = b.place::<Parcel>("Inbox");            // PlaceRef<Parcel>
let heavy  = b.place::<Parcel>("Heavy");
let light  = b.place::<Parcel>("Light");
let count  = b.place::<u32>("Count");
let config = b.place::<Config>("Config");
b.seed(inbox,  [Parcel { id: 1, weight: 3.0 }, Parcel { id: 2, weight: 30.0 }]);
b.seed(config, [Config { max_weight: 10.0 }]);

let weigh = b.transition(
    "Weigh",
    (TakeAllEdge(inbox), SnapshotEdge(config)),
    (heavy, light, count),
    |parcels: Vec<Parcel>, cfg: Config| {
        let (h, l): (Vec<_>, Vec<_>) = parcels.into_iter().partition(|p| p.weight > cfg.max_weight);
        let n = (h.len() + l.len()) as u32;
        (h, l, Some(n))                                  // positional: same-typed outputs
    },
)
.guard(move |net| net.count(inbox.id()) >= 2)       // 'static: `move` the Copy handle in
.priority(|_| 1.0)
.restore_tokens_on_failure()
.id();                                                   // TransitionId, only if needed later

let net = b.build()?;   // Result<Net, BuildError>
```

Builder ergonomics:
- **Transitions chain.** `transition()` inserts the transition and returns a
  `TransitionBuilder<'_>` (holds `&mut NetBuilder` + the id); `.guard`,
  `.priority`, `.restore_tokens_on_failure` mutate in place and return the
  handle (`.output_distribution` is on the body, see §4.5); `.id()` yields the `TransitionId` when needed.
  Id-taking methods on `NetBuilder` exist underneath but are not the
  documented way.
- **Places don't chain.** `b.place::<T>(name)` returns `PlaceRef<T>` directly;
  most places have no options. The few options are `b.seed(p, ..)`,
  `b.serializable(p)`, and the `place_no_clone::<T>(name)` variant.
- Net-level options: `b.label_limit(n)` (default 100 chars, as Python).
- While a `TransitionBuilder` is alive, `b` is borrowed: declare places first.

Conventions:
- Annotate closure parameter types. Inference through the arity trait is not
  reliable, and the annotation doubles as documentation.
- Bodies, guards and priorities are `'static`: capture place handles with
  `move` (`PlaceRef` is `Copy`, so it costs nothing).
- Input arity is capped at 12. Output arity likewise. Inputs and outputs are
  always tuples: a single one needs the trailing comma, `(TakeEdge(p),)` and
  `(count,)`, and the empty tuple `()` means none.
- Bodies may return `Outputs` or `Result<Outputs, E>` where
  `E: Error + Send + Sync + 'static` (trait `IntoOutcome`).
- Mismatch errors use `#[diagnostic::on_unimplemented]` so the compiler says
  "transition body parameters do not match its input edges", not a trait soup.
- Prefer routed outputs when types are distinct: no order to keep track of.

## 6. Firing semantics

### 6.1 Fireability and selection

A transition is **fireable** iff its `enabled` switch is on, its body is in
its slot (not in flight), every input place is non-empty (all four edge
kinds need a token present) and its guard, if any, returns true. Guards run
during the scan, after the cheaper checks. `Net::fireable()` lists the
fireable transitions in declaration order; `Net::is_fireable(t)` asks about
one. The scan is linear each step; incremental tracking is a later
optimisation. ("Enabled" alone means the switch: `enable`, `disable`,
`is_enabled`.)

Selection among fireable transitions: the net's selector, default = highest
priority (unset or NaN scores 0.0; `Net::priority_of(t)` exposes the score),
ties broken by declaration order. Custom selector via `net.set_selector(f)`,
`f: Fn(&Net, &[TransitionId]) -> Option<TransitionId>`, offered the fireable
list in declaration order; returning `None` fires nothing this step.

**Added 2026-10-02.** The tie-break is a setting, `TieBreak::FirstDeclared`
(default, the Python rule) or `TieBreak::LastDeclared`, set with
`NetBuilder::tie_break` or `Net::set_tie_break`. With FIFO places the first
drains a pipeline in batches and the second lets each token flow through
before the next enters; a custom selector ignores it. `Net::select()` reports
the choice without firing, and `NetBuilder::resettable()` /
`Net::is_resettable()` record the author's promise that rebuilding the net is
a true reset.

### 6.2 Three stages

```rust
let firing: Firing = net.launch(t)?;      // stage 1: consume per edge kind; body taken out of the net; Launched
let done: Completed = firing.run();       // stage 2: call the body (no-op if launch ran it inline); catches panics
net.distribute(done)?;                    // stage 3: put body back, check, push to places, Distributed or Failed

firing.transition(); firing.is_pending();   // false when launch ran the body inline (MutateEdge)
done.transition(); done.succeeded();
net.in_flight() -> Vec<TransitionId>; net.is_in_flight(t)   // slots whose body is out
```

- `Firing` owns the consumed tokens and the body. It does not borrow `Net`.
  `Firing` and `Completed` are always `Send + Sync`: airborne inputs are
  owned, and the body is `Send + Sync` by construction.
- Both carry the net's `NetId`; `distribute` checks it (`ForeignFiring`),
  then that the transition is in flight (`NotInFlight`), before touching
  anything. Dropping a `Firing` or `Completed` instead of distributing it
  strands the transition in flight for good (the body never comes home);
  both types are `#[must_use]`. See §17.
- If the transition has a `MutateEdge` input, `launch` runs the body inline
  and returns an already-`Completed` firing. Derived from edge kinds, no user
  flag.
- `distribute` is atomic: every target place is checked before any token is
  written.
- `distribute` on a failed `Completed` restores inputs if configured, emits
  `Failed`, returns `Err(FiringFailed)`.

### 6.3 Sequential

```rust
net.step() -> Result<Option<TransitionId>, FiringFailed>
```

Select, launch, run, distribute, inline. `Ok(None)` = nothing fireable, or
the selector declined. Deterministic given the selector: the basis for tests
and replay. `net.fire(t) -> Result<(), NetError>` fires a named transition
regardless of the selector (`NotFireable` if it cannot fire right now).

### 6.4 Concurrent

```
loop:
    for t in enabled, not in flight:
        firing = net.launch(t)          // tokens leave the place now; peers cannot take them
        hand firing to a worker         // or run inline if not detachable
    for done in finished:
        net.distribute(done)
    stop when nothing enabled and nothing in flight
```

- Consuming synchronously in the launch sweep is the conflict resolver. No
  locks.
- One flight per transition at a time (`body` slot is `None` while airborne).
- `Net::in_flight()` lists airborne transitions for visualisers.
- Guards observe the marking *without* in-flight tokens.
- Completion order is wall-clock dependent; no deterministic replay in this
  mode.

### 6.5 Time

The core has no clock. Time-based guards call `Instant::now()` themselves.

### 6.6 Composition: a net as a transition

A `Net` can be the body of a transition in a larger net, as in Python where a
net is used as a function. No engine support is needed: a body is a stateful
closure (`FnMut`, `Send + Sync`) and `Net` is `Send + Sync`, so the closure
owns the inner net, puts its inputs into the inner net's input places, runs it
to completion with `drivers::run`, drains the inner output places and returns
the results. Planned sugar: `inner.as_transition(inputs, outputs)` that
writes that closure. Observability stops at the boundary: the outer net's
events do not describe the inner firing; a visualiser that wants to look
inside must be handed the inner net (for example via `Arc<Mutex<Net>>` shared
with the closure). See §17.

## 7. Failure semantics

- **Expected outcomes are data.** Route a token to an error place via a
  routed enum or a positional `Option` slot.
- **Unexpected failures return `Err`** from the body, erased to
  `Box<dyn Error + Send + Sync>`.
- **Panics are caught** (`catch_unwind` with `AssertUnwindSafe`), in
  `Firing::run` and around the inline call in `launch`, and converted to a
  failure whose error is `BodyPanicked { message }` (`Display`
  `"panicked: {message}"`; message = the `&str`/`String` payload, else
  `"panic payload of unknown type"`). The body still comes home, carrying
  whatever state the panic left it in; a `MutateEdge` mutation made before
  the panic stands and is reported as `Mutated`. Under `panic = "abort"` the
  process dies, as the user chose.
- **Consumed tokens are lost by default.** `Failed { transition, consumed, restored, error }`
  records their ids and places; with `restored == true` they are back.
- **`restore_tokens_on_failure(t)`** clones the consumed tokens (`TakeEdge`,
  `TakeAllEdge`) at launch; the clones travel with the firing, and on any
  failure in stage three (body `Err`, panic, bad `Distribution`) go back to
  the front of their places in original order **with their original
  `TokenId`s**, before the `Failed` event and without events of their own.
  Snapshot and mutate edges consume nothing, so nothing is restored for them.
  Cost: one clone per consumed token per fire of that transition. Build error
  (`RestoreOfNoClonePlace`) if a consuming input place is `no_clone`.
- **The net stays consistent** after any failure: body returned to its slot,
  transition not in flight, nothing distributed.
- Drivers apply a policy on `FiringFailed`: `Stop` (default), `Disable` the
  transition and continue, or `Continue`.

Error types (`thiserror`, all `#[non_exhaustive]`):
- `BuildError` — from `NetBuilder::build`.
- `NetError` — API misuse: `NoSuchPlace`, `NoSuchToken`, `TypeMismatch`,
  `NoSuchTransition`, `NotFireable`, `ForeignFiring`, `NotInFlight`,
  `NoSuchParam` (a `set_param` of a type no transition declared),
  `NoClonePlace` (a snapshot of a net with a `no_clone` place). Wraps
  `FiringFailed` transparently so that `net.fire(t)` and
  `net.distribute(done)` return one error type.
- `FiringFailed` — a body returned `Err` or panicked; carries the transition,
  the boxed error, and whether inputs were restored.
- `CommandError` — external input rejected. Wraps `NetError` transparently
  (unknown place, transition or token; type mismatch); with `serde`, also
  `NotSerializable`, `InvalidToken`, `NoSuchPlaceName`,
  `NoSuchTransitionName`, `NoSuchParamName`, `InvalidParam` and
  `ParamNotSerializable` from the JSON route. A rejected command changes
  nothing and emits nothing.
- `TooManyFirings` — the driver fuse, carrying a `RunSummary`.

**Names in errors (decided 2026-09-20).** An error escapes the scope where
the net lives: a `?` in `main` prints the message and exits, and by then an
id is useless to the person reading the log. So a runtime error about a
thing that *exists* carries both its id and its `name`; the name goes in
the `Display` message (`place "Uploads" (PlaceId(3)) cannot clone its
tokens`), the id stays as the field a program matches on. A not-found
error (`NoSuchPlace`, `NoSuchTransition`, `ForeignFiring`, the by-name
`CommandError`s) carries the id alone, since there is no name; an error
naming two things (`DistributionError`: the transition and the place) carries
both names, and a place from another net is named `"<another net>"`.
`BuildError` carries names only, as before. Events carry ids only: they are for
programs, and the net is alive when they are read.

## 8. Observation

Two channels with different jobs.

### 8.1 Event log

Appended by the net, drained by the driver (`net.drain_events()`). Updates
for a visualiser are O(changes), never O(state).

```rust
#[non_exhaustive]
pub enum Event {
    Fired       { transition: TransitionId, consumed: Vec<(PlaceId, TokenId)>, produced: Vec<(PlaceId, TokenId)> },
    Launched    { transition: TransitionId, consumed: Vec<(PlaceId, TokenId)> },   // staged path: net.launch
    Distributed { transition: TransitionId, produced: Vec<(PlaceId, TokenId)> },   // staged path: net.distribute
    Failed      { transition: TransitionId, consumed: Vec<(PlaceId, TokenId)>, restored: bool, error: String },
    Added       { place: PlaceId, token: TokenId },      // external extend / put (not seeds: builder seeding is setup, emits nothing)
    Removed     { place: PlaceId, token: TokenId },      // external remove / set_tokens
    Mutated     { place: PlaceId, token: TokenId },      // MutateEdge, or replace()
    Toggled     { transition: TransitionId, enabled: bool },
    ParamChanged { type_name: &'static str },
}
```

`Fired` is emitted by the inline path (`step`, `fire`); the staged path
(`launch` … `distribute`, which is what a concurrent driver uses) emits
`Launched` then `Distributed` because time passes between them. Per firing
the order is `Launched`, `Mutated`*, then `Distributed` or `Failed`; the
`state_counter` therefore moves once more on the staged path than on the
inline one.

### 8.2 Read API

```rust
net.places()      -> impl Iterator<Item = (PlaceId, PlaceInfo<'_>)>    // by value: name, type_name, count (count is live, never stored)
net.transitions() -> impl Iterator<Item = (TransitionId, TransitionInfo<'_>)>  // by value: name, inputs, outputs, enabled, in_flight, detachable
net.edges()       -> impl Iterator<Item = EdgeInfo>   // Input { place, transition, kind } | Output { transition, place, slot }; transition order, inputs then outputs
net.place_named(&str) -> Option<PlaceId>;  net.transition_named(&str) -> Option<TransitionId>
net.fireable() -> Vec<TransitionId>;  net.is_fireable(t) -> bool;  net.is_enabled(t) -> bool;  net.priority_of(t) -> f64
net.count(place)  -> usize
net.tokens(place) -> impl Iterator<Item = (TokenId, &dyn Any)>
net.label(place, token) -> Option<String>            // via PlaceMeta.debug; truncated to label_limit (default 100) + "..."
net.token::<T>(place, token) -> Option<&T>           // typed downcast for custom widgets
#[cfg(feature = "serde")]
net.json(place, token) -> Option<serde_json::Value>  // structured view, serializable places only
net.snapshot() -> Result<Snapshot, NetError>         // owned copy of the *state*, not the net (see below); Err if any no_clone place
net.topology_counter(), net.state_counter(), net.fired_count()
```

`Snapshot` (slice 11) is the marking, the switches, the in-flight set, the
parameters and the counters, cloned through the `PlaceMeta` vtables into an
owned `Send + Sync + Clone` value with the same read API names as `Net`
(`places`, `transitions`, `count`, `tokens`, `token`, `label`, `json`,
`is_enabled`, `in_flight`, `param`, the counters). It is *not* a `Net`, and
cannot be: bodies are `Box<dyn FnMut>` and cannot be cloned, and an in-flight
transition has no body in the net at all; a what-if copy is a rebuild from
the builder. What it is for: a renderer or inspector on another thread owns
a consistent copy while the driver keeps `&mut Net`, and a test keeps the
state at one moment to compare with another. The fireable set is not stored
(guards are closures on the live net). Snapshotting emits nothing and moves
no counter.

There is no observer callback. The driver holds `&mut Net` between steps and
is the observer.

## 9. External input

In-process input is plain methods; `&mut Net` guarantees a single writer:

```rust
net.extend(place: PlaceRef<T>, tokens: impl IntoIterator<Item = T>) -> Vec<TokenId>
net.put(place: PlaceRef<T>, token: T) -> TokenId                       // sugar: extend of one
net.set_tokens(place: PlaceRef<T>, tokens: impl IntoIterator<Item = T>) // replace all; empty = clear
net.remove(place: PlaceId, token: TokenId) -> Result<(), NetError>
net.replace(place: PlaceRef<T>, token: TokenId, value: T) -> Result<(), NetError>   // emits Mutated
net.set_enabled(t: TransitionId, enabled: bool);  net.enable(t);  net.disable(t);  net.is_enabled(t)
```

Parameters: `net.set_param(value)`, see §10.

For input crossing a thread or coming from an untyped UI:

```rust
pub enum Command {
    Extend    { place: PlaceId, tokens: Vec<Box<dyn Any + Send + Sync>> },
    SetTokens { place: PlaceId, tokens: Vec<Box<dyn Any + Send + Sync>> },
    Remove    { place: PlaceId, token: TokenId },
    Replace   { place: PlaceId, token: TokenId, value: Box<dyn Any + Send + Sync> },
    Enable    { transition: TransitionId },
    Disable   { transition: TransitionId },
    SetParam  { value: Box<dyn Any + Send + Sync> },   // slice 11; JSON form names the parameter (§10)
}
Command::{extend, put, set_tokens, replace}(place: PlaceRef<T>, ..)   // typed constructors, erased result
Command::{remove, enable, disable}(..)
net.apply(cmd) -> Result<(), CommandError>   // the one runtime TypeId check in the engine
```

**Done, slice 10.**

- A `Command` is built typed and travels erased. It carries a bare `PlaceId`,
  no `NetId` stamp: it is meant to cross a boundary where the stamp means
  nothing, and the `TypeId` check in `apply` is the guard. A command built
  for one net and applied to another therefore succeeds if the place at that
  index has the same type, and is `TypeMismatch` otherwise.
- `apply` is **atomic**: every token of an `Extend` or `SetTokens` is checked
  against the place's type before anything is written or cleared (Python's
  `_apply_command` checks all tokens first too), so a rejected command
  changes nothing and emits nothing. `TypeMismatch.found` is `"<erased>"`: a
  `dyn Any` knows its `TypeId` but not its name.
- The typed methods and `apply` share one erased path (`extend_erased`,
  `set_erased`, `replace_erased` over `push_erased`), so the two cannot
  drift; as a consequence the typed `extend`/`set_tokens` are atomic too.
- Channels between a UI thread and the driver are the caller's concern; a
  driver drains one in `run_indefinitely`'s `each_tick` hook (§13).
- With the `serde` feature, `JsonCommand` is the wire form: internally
  tagged (`{"command": "extend", "place": "Inbox", "tokens": [1, 2]}`),
  places and transitions **by name** (what a UI author or a saved file knows,
  stable across declaration-order changes), tokens as `serde_json::Value`
  and `TokenId`s as bare numbers. `Command::from_json(&net, json)` resolves
  names and decodes tokens with the place's `from_json` (places marked
  `serializable` only; `NotSerializable` otherwise), `Command::to_json(&net)`
  is the inverse, `net.apply_json(json)` is decode-then-apply. `PlaceId`,
  `TransitionId` and `TokenId` derive `Serialize`/`Deserialize` transparently;
  `NetId` does not.

Input may arrive whenever the caller holds `&mut Net`, including while
firings are in flight.

## 10. Parameters

Python's `kwargs` injected run-varying values (secrets, API tokens, tuning
constants) into a transition at construction, and `SetParam` changed them at
run time, without adding places to the diagram. Rust equivalent: `Param::<T>`,
a non-edge input declared alongside the input edges. Transitions state what
they need; `build()` refuses to build until every declared parameter has a
value.

```rust
b.transition("Fetch", (TakeEdge(urls), Param::<ApiToken>::new()), (pages,), |url: Url, token: ApiToken| ..);
b.param(ApiToken("..."));         // build error if any declared Param type has no value
net.set_param(ApiToken("..."))?;  // between steps; emits ParamChanged { type_name }; NoSuchParam if undeclared
net.param::<ApiToken>()           // Option<&T>, for guards and inspectors
```

**Done, slice 11.** `Param::<T>::new()` rather than a bare `Param::<T>`,
which is not an expression for a struct with a phantom field. `Param<T>`
implements `InputEdge`, the one implementor that reads no place; the sealed
description of an input tuple yields `Edge { place, kind, net }` or
`Param { type_id, type_name }` per position, and the transition keeps the
body's argument order as a list of sources over its (edges-only) input list,
so fireability, `edges()`, `DuplicateInputPlace`, the cycle and source walks
and stage one see exactly the place edges they always did. Build checks:
`MissingParam { transition, type_name }` (first offender), and with `serde`
`DuplicateParamName { name }`. A registered value no transition declares is
allowed.

- **Storage**: one value per type on the `Net`, keyed by `TypeId`. Two values
  of the same underlying type need newtypes (`GithubToken(String)`,
  `OpenAiToken(String)`).
- **What the body receives**: a clone. The body may run off-thread and cannot
  borrow the net. Heavy parameters put their big fields in `Arc`, as tokens
  do.
- **Bounds**: `Clone + Send + Sync + 'static`. Deliberately no `Debug`:
  `label()` and inspectors can never print a secret; a visualiser shows the
  type name only.
- **Enabledness**: unaffected. Parameters always exist after `build()`;
  `set_param` replaces atomically between steps.
- **Diagram**: not a node. `TransitionInfo` lists each transition's declared
  parameter types so a renderer can badge it.
- **Snapshot**: parameters are cloned into the `Snapshot`, readable there
  through `param::<T>()`.
- **Command**: `SetParam { value: Box<dyn Any + Send + Sync> }` with the
  typed constructor `Command::set_param(v)`, matched by `TypeId` in `apply`
  (`NoSuchParam` with `type_name = "<erased>"` otherwise). On the wire,
  `b.serializable_param::<T>("api_token")` gives the type a human-chosen
  name — `type_name::<T>()` is a compiler detail, not a stable key — and
  `JsonCommand::SetParam { param: "api_token", value }` addresses it;
  `NoSuchParamName`, `InvalidParam`, `ParamNotSerializable` are the JSON
  route's failures.
- **Timing**: the clone is taken at launch, so a body already in flight keeps
  the value it was given and the next launch sees the new one.
- **Not an edge**: excluded from `edges()`, `assert_acyclic`, and
  `assert_no_source_transitions`.

Closures may also simply capture values (`move |url| fetch(&token, url)`).
That is plain Rust, not a feature: the net knows nothing about captured
values, they cannot be changed or observed, and the dependency is invisible.
Fine for a fixed database handle; prefer `Param` for anything that varies per
run or that a reader of the net should know about.

## 11. Runtime changes

- **Toggle** is in the net (`set_enabled`), not the driver: it affects
  enabledness, visualisers read it from the net, every driver honours it.
- **Topology editing** is not built in v1 but is not precluded: ids are never
  reused, `topology_counter` exists, `Firing` does not borrow the net,
  `distribute` validates its targets. Adding `add_place::<T>()` later returns
  a fresh `PlaceRef<T>` and keeps compile-time edge checking.

## 12. Build-time checks

`NetBuilder::build() -> Result<Net, BuildError>`.

Always:
- Duplicate place or transition names.
- One input edge per place per transition (`DuplicateInputPlace`).
- `SnapshotEdge` from a `no_clone` place.
- `restore_tokens_on_failure` on a transition with a consuming (`TakeEdge`,
  `TakeAllEdge`) input from a `no_clone` place.
- A declared `Param::<T>` with no `param(value)` of that type
  (`MissingParam`); with `serde`, two parameter types sharing a wire name
  (`DuplicateParamName`). Both before routing.
- Routed-output resolution (§4.5): `UnroutableOutput`, `AmbiguousOutput`,
  `UnreachableOutput`, each naming the transition and the type or place.
- (Output slot count vs output place count is carried by the types; a
  `debug_assert!` in `distribute` guards the erased index.)

Opt-in:
- `b.assert_acyclic()`: iterative three-colour DFS over consuming input edges
  and output edges; read edges excluded (they move no tokens). Roots are
  transitions in declaration order; the first cycle found is reported as
  `BuildError::Cycle { path }`, node names from the repeated node round to
  itself (first == last, so it normally starts at a transition), joined with
  `" -> "` in the message.
- `b.assert_no_source_transitions()`: every transition has at least one
  `TakeEdge` or `TakeAllEdge` input; a transition with only read edges fails.
  First offender in declaration order → `BuildError::SourceTransition`.
- Both run after the always-on checks, source transitions before cycles.
- The pair is a static termination proof; each alone proves less, and the
  docs say so.

Compile-time (no check needed): edge kinds vs parameter types, positional
slot types and count, non-unit return with zero output places, seeded token
types, `Distribution::to` argument types.

## 13. Drivers

Thin functions over the stages, in `petritype::drivers`:

```rust
pub enum Mode { Sequential, Concurrent }                       // Default: Sequential
pub struct Limits { pub stop_after_n_firings: Option<u64>, pub error_after_n_firings: Option<u64> }
pub enum OnFailure { Stop, Disable, Continue }                 // Default: Stop
pub struct RunSummary { pub quiesced: bool, pub fired_count: u64, pub failed_count: u64 }
pub enum DriverError {
    TooManyFirings { limit: u64, transition: TransitionId, summary: RunSummary },
    FiringFailed   { summary: RunSummary, error: FiringFailed },
}

drivers::run(&mut net, mode, limits, on_failure) -> Result<RunSummary, DriverError>
drivers::run_indefinitely(&mut net, mode, tick: Duration, stop: &AtomicBool, on_failure,
                          each_tick: impl FnMut(&mut Net)) -> Result<RunSummary, DriverError>
```

**Done, slice 9.**

- `run`: until quiescent or `stop_after_n_firings`. `quiesced == false` means
  a limit stopped the run and work may remain; call again to continue. The
  fuse (`error_after_n_firings`) raises `TooManyFirings` before the offending
  firing consumes anything. Subsumes Python's `execute_graph`, `Runner.run`
  and `run_to_completion`, which differed only in limits and return type.
- Both limits are **per call** and count **launches** (attempts, failures
  included); `fired_count` counts successes and `failed_count` failures, so
  a caller running under `Disable` or `Continue` need not scan the event log
  to learn that something failed. Python's fuse lived on a `RunContext`
  shared between calls; `Limits` is a plain `Copy` argument, so there is no
  hidden state to carry a half-spent fuse from one call to the next.
- `OnFailure::Stop` returns `DriverError::FiringFailed` carrying the *first*
  failure; `Disable` calls `net.disable(t)` (a `Toggled` event) and goes on;
  `Continue` only counts. `Continue` with `restore_tokens_on_failure` retries
  for ever, which is what the fuse is for.
- `run_indefinitely`: the real-time regime. Never stops because nothing is
  fireable; ends when `stop` is set, checked before every launch. `each_tick`
  runs at the start of every tick with `&mut Net`, and is where the caller
  drains its own input channel or applies `Command`s: the driver stays
  channel-agnostic (§9), which is why there is no `inbox` parameter. In
  sequential mode a tick is one `step` then a sleep (one firing per frame, as
  Python's `_tick_sequential`); in concurrent mode it is one launch sweep,
  and the wait on the completion channel *is* the sleep, so a completion is
  distributed the moment it lands. A Bevy system per frame is this loop with
  the frame as the tick. No `Limits`: an indefinite run has no firing budget.
- `Mode::Concurrent` uses `std::thread::scope`, one thread per launch, and an
  `mpsc` channel of `Completed`. It launches *everything* fireable in
  declaration order, so the selector and `priority` shape sequential runs
  only. Non-detachable firings run inline and trigger another sweep.
- A driver never returns with anything in flight, on the error path
  included: it stops launching, then distributes every completion it is
  still owed. So `in_flight()` is empty whenever a driver returns, and a
  driver never strands a transition (§17).
- `step` (one firing) is on `Net` itself; `clear_selector` undoes
  `set_selector`.

## 14. Crate layout

Cargo workspace, edition 2024, MSRV = current stable minus two (must be
≥ 1.78 for `#[diagnostic::on_unimplemented]`).

| crate | contents | deps |
|---|---|---|
| `petritype` | builder, `Net`, events, checks, `drivers` | `thiserror`; feature `serde` adds `serde`, `serde_json` |
| `petritype-derive` | `#[derive(Routed)]` on enums (variants) and structs (leaf) | `syn` 2, `quote`, `proc-macro2`; re-exported by `petritype` unconditionally as `petritype::Routed` |
| `petritype-dot` | Graphviz DOT text of a net and its marking (**done, slice 12**) | `petritype` only |
| `petritype-bevy` | `NetRes`, mirror systems, token entities, in-flight highlighting, task-pool driver | pins one Bevy version |
| `petritype-tokio` | only if a need appears | |

Examples (each also an integration test), mirroring the Python acceptance set:
quickstart, parcel distribution, caching (union return → routed enum),
match-up tokens (`TakeAllEdge` + guard + positional), coloured balls
(`Distribution`), web scrape (routed enums), execution modes (sequential vs
concurrent), read-edge scaler (`SnapshotEdge` + `MutateEdge`).

**Done, slice 12.**

- `petritype-dot` renders through a `DotSource` trait implemented for `Net`
  and `Snapshot` (a render thread can own a snapshot), takes `&` only, and
  is a pure function of source and `Style { rankdir, show_tokens,
  max_tokens, highlight, graph_name }`. Node ids are `p<i>` / `t<i>`; names
  live in labels, escaped. The styling mirrors the Python renderer (ovals
  deepskyblue, boxes lightgreen, gold for in flight, magenta outline for the
  last firing via `Highlight::from_event(&Event)`, dashed for `SnapshotEdge`,
  dotted-both-ends for `MutateEdge`) with four additions: disabled
  transitions grey and dashed, `*` on a `TakeAllEdge`, slot numbers on a
  transition with more than one output, red for a `Failed` event. A
  highlighted read edge keeps its shape and only changes colour.
- The examples live in `petritype/examples/` as cargo examples with
  `test = true`, so `cargo test` runs their assertions and `cargo run
  --example` shows them; they draw their nets with `petritype-dot`, which
  makes it a path-only dev-dependency of `petritype` — a cycle cargo
  permits and strips at publish. Where the Python was perpetual or random
  (parcel distribution) the port is finite and deterministic; where it
  relied on `allow_token_copying` for two output places of one type
  (caching) the pair became newtypes; the subgraph-as-a-transition outer net
  (§6.6) is not ported. Order assertions rely on the default selector's
  declaration-order tie-break.

Names: the crate is `petritype`, the repository `petritype-rs` (decided
2026-09-20). Same-name packages across PyPI and crates.io are the norm, an
`-rs` suffix is discouraged on crates.io and would land in every import path,
and the repository name already tells the two projects apart. `petritype`,
`petritype-derive` and `petritype-dot` were all free on crates.io on
2026-09-20.

## 15. Bevy adapter sketch

**Decided 2026-09-20.** `petritype-bevy` is a separate repository
(`../petritype-bevy`, github.com/olenive/petritype-bevy), not a workspace
member: Bevy compiles in minutes and releases every few months, and the
core must not pay for either. It pins the released Bevy (0.19.1; 0.20 when
final) and depends on `petritype` by path until `petritype` is published,
then by version. First cut is layers (a) and (b) below — the resource, a
per-frame stepping system, and the event mirror — with a plain grid
layout, split as a headless `MirrorPlugin` (testable under
`MinimalPlugins`) and a `RenderPlugin`. Layout, hover and the inspector
follow once the demo moves. **2026-10-02:** the demo moved: the layered
layout, hover tooltips, the Step / Repeat Step / Activate run modes, the
theme and Reset landed in the same repository.

- `NetRes(Net)` newtype `Resource` (orphan rule; the newtype lives in the
  adapter).
- Startup: `net.places()`, `net.transitions()`, `net.edges()` → spawn view
  entities, lay out. Re-run when `topology_counter` changes.
- Each frame: step with a budget (or poll `Task<Completed>` from
  `AsyncComputeTaskPool` and distribute), then `drain_events()`:
  - `Fired`/`Distributed` → spawn token entities for `produced`, despawn or
    animate `consumed`; `HashMap<TokenId, Entity>`.
  - `Mutated` → refresh the cached label via `net.label()`.
  - `Toggled`, `Launched` → restyle the transition entity.
- Hover/click → `net.token::<T>()` or `net.json()` for a detail panel.
- Edits from an inspector → `net.apply(Command)` → `Mutated` → view refresh.
  The view never writes to the net except through commands.

## 16. Rust features used, and why

| problem | feature |
|---|---|
| heterogeneous typed places in one net | `Box<dyn Any + Send + Sync>` storage + `TypeId`; typed handle `PlaceRef<T>` with `PhantomData<fn() -> T>` |
| per-type helpers without trait objects on tokens | monomorphised generic fns coerced to `fn` pointers, stored in `PlaceMeta` |
| bodies of any arity in one list | trait over tuples, `macro_rules!` impls for arity 0..=12 (as Bevy `IntoSystem`, axum handlers) |
| `&mut T` parameters from `MutateEdge` | generic associated type `type Arg<'a>` on the edge-kind trait |
| readable mismatch errors | `#[diagnostic::on_unimplemented]` |
| body may return plain or `Result` | trait `IntoOutcome<Outputs, Marker>` over the return type; marker tags (`Positional`, `Failing<M>`, later `Routed`) keep the impls from overlapping, as Bevy's `IntoSystem<Marker>` |
| positional slot may hold one / maybe one / many | trait `IntoTokens<T>` for `T`, `Option<T>`, `Vec<T>` |
| routing by payload type without runtime reflection | trait `RoutedOutput` (`describe` lists `TypeId`s, `emit` pushes into a `RouteSink`); `#[derive(Routed)]` proc macro names `petritype::routing`, so that module is public; table resolved from types at `transition()`, problems reported at `build()`. Cost of marker-based form selection: a return type that fits neither form is blamed on `IntoOutcome`, whose notes therefore describe both forms |
| body in flight without borrowing the net | `Option<Box<dyn FnMut>>` slot, `Option::take` on launch |
| bodies on other threads | `Send + 'static` bounds; `Firing` and `Completed` always `Send` (owned inputs; `MutateEdge` runs inline at `launch`); `std::thread::scope` |
| no aliasing between tokens | ownership and moves; `Clone` explicit; `Arc` for sharing; `Arc::make_mut` for copy-on-write |
| forbid unsafe sharing | `Send + Sync` bounds reject `Rc`, `RefCell`, `Cell` |
| single writer | `&mut Net` |
| forward-compatible enums | `#[non_exhaustive]` on `Event` and errors |
| errors | `thiserror` |
| surviving user panics | `std::panic::catch_unwind` + `AssertUnwindSafe` |
| FIFO places | `VecDeque` |

## 17. Open questions

- `MutateEdge` implementation: **resolved, slice 2.** The GAT route works
  under the arity macro and the `for<'a> FnMut` bound; no fallback.
- Identity preservation for pass-through bodies (`Keep(token)`): deferred
  until an animation needs it.
- `Broadcast(value)` sugar: deferred.
- Stamped net id in `PlaceRef`: **done, slice 2** (§4.1).
- Observability of an inner net used as a transition (§6.6): how a visualiser
  reaches inside. Deferred until the Bevy adapter needs it.
- Stranded in-flight transitions: a `Firing`/`Completed` dropped without
  being distributed, or a rejected `distribute` (`ForeignFiring`,
  `NotInFlight`, which consume their argument), loses the body for good.
  Panics no longer strand (caught since slice 7), and the drivers (slice 9)
  never drop one: they drain everything airborne before returning, on the
  error path too. The misuse rejections could hand the value back.
  **Decided 2026-09-20: deferred, and when it comes it is additive** — a
  second method `try_distribute(done) -> Result<(), (NetError, Completed)>`
  beside the plain `distribute`, so net authors and ordinary drivers keep
  the simple shape and a hand-written driver that juggles several nets opts
  in. Add it when the Bevy adapter or a user asks.

## 18. Implementation plan

Slices, each with tests, in order:

1. Ids, `PlaceMeta`, `PlaceSlot`, `NetBuilder` with `place` / `seed` /
   `build`, read API for places and tokens. `extend`, `put`, `set_tokens`,
   `remove`, `replace`; `Added` / `Removed` / `Mutated` events.
2. Edge kinds, `IntoTokens`, `IntoOutcome`, the arity macro for bodies,
   `transition()` with positional outputs. Compile-fail tests (`trybuild`)
   for mismatched bodies.
3. Enabledness, selector, `step()`, `Fired` event, guards, priority,
   enable/disable.
4. Routed outputs: `RoutedOutput`, build-time resolution, `Either`,
   `petritype-derive` with `#[derive(Routed)]`.
5. `Distribution` output form.
6. `launch` / `run` / `distribute`, `Firing`, `Completed`, in-flight set,
   `Launched` / `Distributed`; detachable vs inline; `MutateEdge`.
7. Failure: `IntoOutcome` for `Result`, panic catching,
   `restore_tokens_on_failure`, `Failed` event.
8. Build-time checks: names, no-clone constraints, `assert_acyclic`,
   `assert_no_source_transitions`.
9. Drivers: `run` with `Mode`, limits, fuse, `OnFailure`; `run_indefinitely`.
10. `Command` / `apply`; `serde` feature: `json()`, serialisable commands.
11. `snapshot()`; `Param::<T>`, `b.param`, `set_param`, `ParamChanged`.
12. `petritype-dot`; port the Python examples as integration tests.
13. `petritype-bevy` — first cut done 2026-09-20 in its own repository
    (§15): `MirrorPlugin` (headless) + `RenderPlugin` + the parcel demo;
    run modes, animation, layered layout, hover, theme and reset added
    2026-10-02 (26 headless tests).
