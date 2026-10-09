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

Non-goals for v1: persistence of markings (beyond a marking's JSON form,
export and import, which nets as tokens needed: §9, slice 16), distributed
nets, edge weights beyond one/all, timed Petri net semantics, topology
editing at runtime (designed for, not built).

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
| `ListPlaceNode(keys={..})`, `Filter`, `Query.matches`, `Query.distinct` | `b.keys(place, names, extractor)`, `Filter`, `net.query`, `net.distinct_values` (§8.3; with an index, which Python does without) |
| `TakeWhereEdge`, `MutateWhereEdge` (subclasses of the plain edges) | same names, their own structs `TakeWhereEdge(place, filter)`, `MutateWhereEdge(place, filter)` (§8.3, slice 19) |

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
  input place is non-empty (holds a token the filter accepts, for a keyed
  edge) and whose guard passes.
- **Firing**: consume inputs, run body, distribute outputs.
- **Quiescent**: nothing fireable and nothing in flight.
- **Inner net**: a net carried by a token, in a *nested* place; the net
  holding the token is the **outer net** (§6.6, slice 15). *Quiescent deep*:
  quiescent here and in every inner net that is not passive.
- **Key**: a named attribute of a token — a string, an integer or a
  boolean — that its place derives from the token's value with an extractor
  the author declared (§8.3, slice 18). A **filter** keeps the tokens whose
  keys pass its clauses; a **query** asks a net for them; a **keyed edge**
  (`TakeWhereEdge`, `MutateWhereEdge`, slice 19) takes or mutates the oldest
  token its filter accepts instead of the front one.
- **Template**: a function registered by name that builds a net; a net
  built through one is **stamped** with it, and can then be saved, restored
  and **forked** (copied by building it again), §6.6, slice 16.

## 4. Representation

### 4.1 Identifiers and counters

```rust
pub struct PlaceId(u32);        // erased; used in events and the read API
pub struct TransitionId(u32);
pub struct TokenId(u64);        // monotonic per Net

pub struct NetId(u32);          // process-unique, minted per NetBuilder; also names an inner net (§6.6)

pub struct PlaceRef<T> { place: PlaceId, net: NetId, _t: PhantomData<fn() -> T> }  // typed handle, 8 bytes
```

- Ids are **never reused**. Consumed tokens retire their id. Removed places or
  transitions (future) leave tombstone slots so other indices don't shift.
- `PlaceRef<T>` is `Copy`, `Send`, `Sync` regardless of `T` (the `fn() -> T`
  phantom is deliberate). `.id()` gives the erased `PlaceId`.
- The three erased ids have explicit constructors (`PlaceId::new(index)`,
  `TransitionId::new(index)`, `TokenId::new(number)`), accessors
  (`index()`, `get()`) and `FromStr` as the inverse of `Display`, added
  2026-10-03 for a mirror of a remote net (§19) that numbers places in
  topology order and reads token ids off the wire. A hand-made id the net
  never issued is answered with `None` or `false`; it cannot become a
  `PlaceRef`.
- `Net` carries two change counters, plain `u64`s that increment by one:
  - `topology_counter`: connectivity changes only (place, transition, edge
    added or removed). v1 never bumps it.
  - `state_counter`: everything else — token added, removed, mutated,
    transition toggled. A change tick for observers, not a firing count:
    a freshly built net is at 0 however it was seeded.
  - `fired_count`: number of completed firings. A statistic, not a counter of
    changes.
  - `emit` bumps `state_counter` once per event: a firing bumps it by one
    (`Fired`) plus one per mutating edge, `MutateEdge` or `MutateWhereEdge`
    (`Mutated`); a failed firing likewise
    (`Mutated`s then `Failed`); a toggle by one.
- `PlaceRef` is stamped with its net's `NetId` (an `AtomicU32` counter, one
  per process; not stable across runs). Every method that
  takes a `PlaceRef` compares the stamp first, so a handle from another net
  cannot index this one: infallible methods (`put`, `seed`, `transition`, ..)
  panic with both net ids in the message; fallible ones return
  `NetError::NoSuchPlace`. Needed because composition (§6.6) puts two nets'
  handles in one scope. `PlaceId` and `TransitionId` stay unstamped.
- **Changed 2026-10-09 (slice 15).** `NetId` was never serialised; it now
  is, one way. It is the stable id of an inner net in `Event::Nested`
  (§6.6), which a consumer follows a token by while its `TokenId` changes,
  so it has `get()`, prints as its bare number and, with `serde`, is
  `Serialize` (not `Deserialize`) as that number: an opaque, process-local
  id, not something to store and read back.

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
    to_json: Option<fn(&dyn Any) -> Result<serde_json::Value, _>>, // Some iff marked serializable
    #[cfg(feature = "serde")]
    from_json: Option<fn(serde_json::Value) -> Result<Box<dyn Any + Send + Sync>, _>>,
    nested: Option<NestedVtable>,   // Some iff place_nested (§6.6)
    passive: bool,                  // step_deep leaves its inner nets alone; never true without nested
    keys: Option<KeysVtable>,       // Some iff b.keys (§8.3, slice 18)
}

struct KeysVtable {                 // slice 18: the declared names and the author's extractor
    names: Arc<[&'static str]>,
    extract: Arc<dyn Fn(&dyn Any) -> Vec<KeyValue> + Send + Sync>,   // a closure, so not a fn pointer
}

struct NestedVtable {               // slice 15: T::nets and T::nets_mut of the token's Nested impl
    nets: fn(&dyn Any) -> Vec<(&str, &Net)>,
    nets_mut: fn(&mut dyn Any) -> Vec<(&str, &mut Net)>,
    #[cfg(feature = "serde")]
    fork: fn(&dyn Any) -> Result<Box<dyn Any + Send + Sync>, NetError>,  // slice 16: T::fork
}
```

`to_json` became fallible in slice 16: a `Serialize` impl may refuse (a token
holding a net no template built), and a save must hear it. `Net::json` and
`Command::to_json` keep their old behaviour and show the refusal as a string;
`export_marking` returns it as an error. `fork` is the nested place's cloner,
fallible because it rebuilds nets: `Net::snapshot` calls it where an
ordinary place's `clone` would be (§4.3, §6.6).

`PlaceMeta` is a hand-rolled vtable: monomorphised generic functions
(`debug_of::<T>`, `clone_of::<T>`) coerced to plain `fn` pointers when the
builder knows `T`. Zero per-token overhead, no trait bound on the token beyond
what the builder method requires. The one entry that is not a `fn` pointer
is the key extractor (slice 18): it is the author's closure, wrapped where
`T` is known to take an erased token and kept in an `Arc`, so that
`PlaceMeta` stays `Clone` and a snapshot shares it.

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

`place_nested::<T>()` (slice 15, §6.6) is the second: `T: Nested + Debug +
Send + Sync + 'static`, no `Clone`, because a `Net` cannot be cloned. It is a
no-clone place with all of the above, except, since slice 16 and with the
`serde` feature, `Net::snapshot()`: a snapshot copies each of its tokens with
`Nested::fork`, which rebuilds the token's inner nets from their templates
(§6.6). The snapshot edge and the restoring consuming edge stay build
errors: both clone on the launch path, where nothing may fail, and a fork
can.

Big data idiom: put heavy fields in `Arc`. Moves are free (a `Vec` is 24
bytes to move); `Arc::clone` is a counter bump; `Arc::make_mut` is
copy-on-write. Shared *mutation* requires `Arc<Mutex<_>>`, which is visible
in the place's type.

### 4.4 Transitions and edges

```rust
struct TransitionSlot {
    name: String,
    inputs: Vec<(PlaceId, EdgeKind)>,      // EdgeKind::{Take, TakeAll, Snapshot, Mutate, TakeWhere, MutateWhere}
    filters: Vec<Option<Filter>>,          // beside inputs: Some exactly for the two keyed kinds (slice 19)
    outputs: Vec<PlaceId>,                 // positional; also the target set for routed / Distribution
    // no routing field: a routed body's TypeId -> output index table is resolved from types when the
    // body is wrapped (transition()) and captured by the erased closure; problems surface at build()
    body: Option<Box<dyn FnMut(ErasedInputs) -> ErasedOutcome + Send + Sync>>,  // None while in flight
    guard: Option<Box<dyn Fn(&Net) -> bool + Send + Sync>>,
    priority: Option<Box<dyn Fn(&Net) -> f64 + Send + Sync>>,
    enabled: bool,
    restore_tokens_on_failure: bool,
    detachable: bool,                      // false iff any input is a MutateEdge or a MutateWhereEdge
}
```

Input edge kinds and the body parameter each produces:

| kind | consumes? | parameter type | presence required |
|---|---|---|---|
| `TakeEdge(p)` | one token, FIFO | `T` | yes |
| `TakeAllEdge(p)` | every token | `Vec<T>` | yes |
| `SnapshotEdge(p)` | no | `T` (clone of the front token) | yes |
| `MutateEdge(p)` | no | `&mut T` (front token, in place) | yes |
| `TakeWhereEdge(p, filter)` | one token, the oldest the filter accepts | `T` | a token the filter accepts |
| `MutateWhereEdge(p, filter)` | no | `&mut T` (the oldest token the filter accepts, in place) | a token the filter accepts |

A transition reads each place through at most one input edge
(`DuplicateInputPlace` at build), so fireability is exactly "every input
place non-empty" (for a keyed edge, "holding a token the filter accepts")
and the borrows of the mutating edges are disjoint by construction. The two
keyed edges (slice 19) are §8.3's.

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
- **Which form, read back** (slice 17): `TransitionInfo::positional` and
  `TopologyTransition::positional` say whether the body (or its output
  distribution function) returns by position, `()` included, read off the
  return type when the transition is declared. It is what an output edge's
  `slot` means: a position in what the body returns only for a positional
  body, the declared order and nothing more otherwise, so the protocol
  runner writes `slot` for positional outputs alone (§19.3.11), as Python
  writes its `return_index`. A flag rather than an enum of the three forms:
  a variant called `Positional` beside `marker::Positional` makes rustc
  spell the marker's whole path in every error that names a body.

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
its slot (not in flight), every input place is non-empty (all four plain
edge kinds need a token present, and a keyed edge one its filter accepts,
§8.3) and its guard, if any, returns true. Guards run
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
- If the transition has a `MutateEdge` or `MutateWhereEdge` input, `launch`
  runs the body inline and returns an already-`Completed` firing. Derived
  from edge kinds, no user flag.
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
`net.step_deep()` (slice 15) is `step` and, when that fires nothing, one
firing inside a token that carries nets of its own (§6.6).

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

### 6.6 Composition: nets as tokens

**Built 2026-10-09, slice 15.** A token may carry Petri nets of its own,
*inner nets*; the net holding the token is the *outer net*. The engine runs,
reads and reports on inner nets through the token. Asked for by a consumer
whose entities (a vehicle, a body, a building) move between the places of a
larger process and have life cycles of their own, of differing shapes
(`Cyberpunk2020-Simulation/dev-docs/NETS_AS_TOKENS.md`; this slice is its R1
to R4). The example is `petritype/examples/transit.rs`: a city whose
vehicles carry a cabin, a deck or a pod, and the proposal's acceptance
script as its tests.

```rust
pub trait Nested: Send + Sync + 'static {
    fn nets(&self) -> Vec<(&str, &Net)>;              // each inner net with a name, in a fixed order
    fn nets_mut(&mut self) -> Vec<(&str, &mut Net)>;  // the same, mutably
}
impl Nested for Net { .. }                            // one inner net, itself, named ""

b.place_nested::<T: Nested + Debug + Send + Sync + 'static>(name) -> PlaceRef<T>
b.passive(place)                                      // BuildError::NotNested on any other place
net.nested(place, token) -> Option<Vec<(&str, &Net)>>
net.nested_scope(place, token, f: impl FnOnce(&mut [(&str, &mut Net)]) -> R) -> Option<R>
net.step_deep() -> Result<Option<DeepStep>, FiringFailed>
net.step_inside() -> Result<Option<DeepStep>, FiringFailed>   // slice 17: step_deep without this net's own step
net.is_quiescent_deep() -> bool
net.place_ref::<T>(name) -> Option<PlaceRef<T>>       // a typed handle by name, checked by TypeId

pub enum DeepStep {
    Here   { transition: TransitionId },
    Inside { place: PlaceId, token: TokenId, net: String, nested_id: NetId, step: Box<DeepStep> },
}
```

- **Declaring.** A token type says which nets it owns; one without
  `Nested` is a compile error (`#[diagnostic::on_unimplemented]`).
  `place_nested` records `T::nets` and `T::nets_mut` in the place's vtable
  (§4.2). The place is no-clone (§4.3); phase 2 lets a snapshot fork it.
- **Running.** `step()` keeps its meaning: one firing of this net.
  `step_deep()` tries `step()` and, when it fires nothing, walks the nested
  places in place order, passive ones skipped, their tokens oldest first,
  each token's nets in `nets()` order, calling `step_deep` on each; the
  first firing found is the step. In a loop that is the policy (the
  proposal's default 1): the outer net until it is quiet, then the inner nets
  in place order then token order, repeated until everything is quiet.
  Deterministic given the selectors. `is_quiescent_deep()` is §3's
  quiescence here plus every non-passive inner net quiescent deep;
  `fireable()` stays shallow. The drivers step deep (§13).
  `step_inside()` (public since slice 17, `pub(crate) step_inner` before)
  is the second half alone: the walk, without asking this net's selector.
  It is for a driver that has already asked `select()` because it fires
  this net's own transitions in stages — the concurrent driver, and the
  protocol runner, whose outer bodies run on threads of their own while its
  inner firings are inline (§19.3.11) — and would otherwise ask the
  selector twice and fire inline whatever it chose.
- **Passive.** `step_deep` never descends into a passive place, and its
  inner nets do not count against `is_quiescent_deep`. An outer body steps
  them itself: `vehicle.inner.step()`, or `drivers::run` on it, inside a
  `MutateEdge` body.
- **Reaching inside** adds nothing to the engine beyond reading. A
  `MutateEdge` body is handed `&mut T` and uses the inner net's own API
  (`put`, `remove`, `set_param`, `apply`, `drivers::run`). A guard reads
  `net.nested(place, token)` of the front token (`net.tokens(place).next()`),
  which is the one a `MutateEdge` is handed. A caller that is not a body uses
  `nested_scope`. A body knows the inner places by name, so
  `place_ref::<T>(name)` turns a name into the handle the inner net's typed
  API wants (stamped with that net's id), rather than every token type
  storing handles beside its net.
- **An outer firing is one unit** (the proposal's default 4).
  `restore_tokens_on_failure` restores the outer tokens the firing consumed
  and nothing else: what a failing body already did inside an inner net
  stands. A body that reaches inside should be infallible after its first
  inner write, or write with `Command`s, each of which is atomic.
- **Observability.** An inner net's events come out in the outer log
  wrapped (the proposal's default 3), so consumers that know nothing of
  nesting are untouched: `Event::Nested { place, token, net, nested_id,
  event: Box<Event> }`, a `Nested` inside a `Nested` one level further down.
  The *net* lifts them, not the driver, so every driver and every caller of
  `fire` or `put` gets the same log; `drain_events` does not sweep tokens.
  The rules, each under the place and token id the token has at that moment:
  1. an inner firing made by `step_deep`: straight after it, where the token
     sits;
  2. a `MutateEdge` (or, since slice 19, `MutateWhereEdge`) body on a
     nested token: before the firing's `Mutated` for that token, so
     `Nested`*, `Mutated`, then `Fired`, `Distributed` or `Failed`;
  3. a consuming edge (`TakeEdge`, `TakeAllEdge`, since slice 19
     `TakeWhereEdge`) from a nested place: at launch, before `Launched` or
     `Fired`, under the source place and the old id, so nothing logged
     inside a token that is never put back is lost;
  4. a token produced into a nested place: after `Fired` or `Distributed`,
     under the destination and the *new* id, which the reader has then seen;
  5. external input: `put`, `extend`, `set_tokens`, `replace` into a nested
     place after the `Added` or `Mutated`; `remove`, `set_tokens`, `replace`
     of an outgoing token before the `Removed` or `Mutated`;
  6. `nested_scope`: when the closure returns.

  Every lifted event goes through `emit`, so `state_counter` moves once per
  lifted event. Seeds are setup: `build()` drops what a seeded token's inner
  nets logged while they were being made. As built, rule 2 lifts when the
  firing lands rather than when the body returns: the token never left its
  place, and landing is where the `Mutated` is written.
- **The stable id.** `nested_id` is the inner net's own `NetId`, which does
  not change when the outer token is taken and put back under a new
  `TokenId`; a consumer follows a vehicle by it from station to station.
  Hence `NetId`'s new `Display` and `Serialize` (§4.1).
- **Failures say where.** A failure inside is the inner net's
  `FiringFailed`, its `path` filled in level by level on the way up (§7).
- **Limits** (the proposal's R10). No shared places: the outer net reaches
  in only through the token, and no inner guard or body sees the outer net.
  Depth is unbounded; the tests go two deep (a passenger carrying luggage).
  Adding a vehicle is adding a token, never a place.
- **On the wire** (phase 3, slice 17): the protocol runner addresses inner
  nets by path or nested id, reports their events wrapped in `nested`
  notifications with versions of their own, and steps them with
  `step_inside` (PROTOCOL.md §12; §19.3.11). `petritype-dot` draws the outer
  net only.

**Phase 2, 2026-10-09: saving and forking** (slice 16; the proposal's R5a,
R5b, R5, R6 and step 6 of its script). Everything here is behind the `serde`
feature; without it the crate is as phase 1 left it.

```rust
pub struct Marking;                                    // place name -> tokens as JSON, oldest first; BTreeMap, transparent
net.export_marking() -> Result<Marking, NetError>      // every place serializable, or NotSerializable (first offender)
net.import_marking(&Marking) -> Result<(), CommandError>  // wholesale; one Event::Resynced

pub type Params = serde_json::Map<String, serde_json::Value>;   // the runner's Params, same definition
pub struct Templates;                                  // name -> Arc<dyn Fn(&Params) -> Result<Net, Box<dyn Error + Send + Sync>> + Send + Sync>
Templates::new(); templates.register(name, factory) -> &mut Self; templates.names()
templates.build(name, &Params) -> Result<Net, TemplateError>    // stamps the net
templates.scope(|| ..) -> R;  templates.restore(serde_json::Value) -> Result<Net, serde_json::Error>
b.template(name, factory); b.templates(&Templates);    // the registry the net decodes nested tokens with
net.template() -> Option<&str>;  net.templates() -> &Templates
impl Serialize for Net; impl<'de> Deserialize<'de> for Net;  net.to_json()
net.fork() -> Result<Net, NetError>
trait Nested { .., fn fork(&self) -> Result<Self, NetError> where Self: Sized { Err(NotForkable) } }
snapshot.nested(place, token) -> Option<Vec<(&str, &Net)>>
```

- **The JSON form of a net** (PROTOCOL.md §12.5, which fixes it for both
  runtimes) is exactly `{"template", "marking", "params", "enabled"}`: the
  template that built it; its marking as `export_marking` writes it; the
  current value of every parameter registered with `serializable_param`, by
  wire name (a parameter without a wire name is not saved); and every
  transition's switch, by name. A token type with a `Net` field derives
  `Serialize` and `Deserialize` and gets its net in this form under the
  field's name, so a vehicle is `{"name", "kind", "destination", "cabin"}`.
  Serialising a net that could not be restored is an error, not a quiet
  loss: one with no template stamp, one with a place that is not
  serializable, one with a transition in flight.
- **Templates and the stamp.** A template is a factory of the runner's
  shape (§19.3.1) registered by name. `Templates::build` calls it and
  *stamps* the net with the template's name, the `Params` it was given and
  an `Arc` of the factory, so that `Serialize` and `fork` need no registry.
  Restoring calls the factory with the saved `params` object, imports the
  marking, sets every saved parameter again through the JSON parameter route
  (so a value changed at run time wins over what the factory baked in; an
  unknown name or a bad value is an error naming it), throws every saved
  switch, and empties the log: a restored net starts at zero, as a built one
  does. A factory therefore reads what it needs from its net's own wire
  parameters, with defaults.
- **The scope rule.** `Deserialize` for a type with a `Net` field cannot be
  handed a registry, so `Net`'s reads one from a stack kept per thread
  (`thread_local!` over a `RefCell<Vec<Templates>>`; no `unsafe`).
  `Templates::scope(f)` pushes a registry for the duration of `f`, and pops
  it on return or unwind. A template is looked for innermost scope first,
  then outwards. A net opens a scope of its own registry wherever it decodes
  tokens — `import_marking`, `Command::from_json` and so `apply_json`, and
  around the forks of `snapshot` — so a caller that goes through the net
  never sees the stack. Depth two works either way: a cabin that carries
  the `Luggage` template itself finds it in its own scope, and one that does
  not finds it in the city's, one level out. Deserialising a `Net` with no
  scope open is a serde error saying so.
- **Import is a resync.** `import_marking` checks everything first — every
  name in the marking is a place (`NoSuchPlaceName`), every place of the net
  is serializable (`NotSerializable`, mentioned or not, since emptying a
  place that cannot be filled would lose what it holds), every token decodes
  (`InvalidToken`) — and only then empties every place and fills it, with
  fresh token ids. A place the marking does not mention loads empty. It
  emits one `Event::Resynced` and no `Removed` or `Added`: it is a
  replacement, not a sequence of changes (§8.1). Incoming nested tokens are
  setup, their inner logs forgotten as a seed's are, so no stale `Nested`
  is lifted later. A rejected import changes nothing and emits nothing.
- **Fork.** `Net::fork` builds the net again from its stamp — the factory
  given the stamp's `Params` with every wire parameter's current value over
  them — imports this net's exported marking, clones every parameter value
  across (all of them: a fork never leaves the process, so it keeps what a
  save cannot), copies every switch, the label limit and the tie-break, and
  empties the log. The selector is a closure and is not copied. A net with
  no stamp is `NoTemplate`; one with a transition in flight is `InFlight`,
  since the tokens that firing took are in no place; a factory that fails
  this time, or a marking that does not load back, is `RebuildFailed`.
  `Nested` gains `fork` with a default that refuses (`NotForkable`), so
  phase 1's implementors compile unchanged and a type that wants snapshots
  writes three lines; `Net`'s own is `Net::fork`.
- **What a snapshot does with a nested place.** An ordinary place is cloned
  as before; a nested place has each token forked through the vtable's
  `fork` (§4.2) under the net's templates, keeping its live token id, and
  the first failure is `CannotFork { place, name, token, source }`; a place
  with neither is `NoClonePlace` as before. `Snapshot::nested` reads the
  forks. A clone of a snapshot shares its forked places through an `Arc`
  instead of forking again, since `Clone` cannot fail and nothing writes to
  a snapshot; ordinary places are still cloned token by token.
- **Deviations from the proposal, and why.**
  1. The proposal names four JSON fields and the first brief for this phase
     a fifth, `template_params`, for what the factory was given as distinct
     from the live parameters. PROTOCOL.md §12.5, settled with the Python
     track, has the four: the factory is given the saved `params`. What a
     factory was given beyond its net's wire parameters is kept on the stamp
     and used by `fork`, but not saved.
  2. The proposal wanted the builder to refuse a snapshot of a nested place
     without a template. The builder cannot know what template a future
     token's net will carry, so the refusal is a run-time error of
     `snapshot()` naming the place, the token and the reason.
  3. The proposal wanted a templated nested place to become an ordinary,
     cloneable place, so that snapshot edges work. Snapshot edges and
     restoring consuming edges from a nested place stay build errors: both
     would need a fallible clone on the launch path. This is the one part of
     R6 not taken.
  4. An import emits `Event::Resynced`, Python petritype's event of the same
     name for a change that is not told as events, rather than a `Removed`
     and an `Added` per token; one for the whole import, with no fields,
     where Python records one per place with its name.
  5. The registry for `Deserialize` comes from a thread-local scope, the one
     way a derived `Deserialize` can be given context; the net opens it
     itself wherever it decodes.
- **The example** became the protocol's conformance net `transit`
  (PROTOCOL.md §11): the net lives in `petritype/examples/transit/net.rs`,
  which depends on `petritype`, `serde` and `serde_json` only so that the
  runner's example can include it in phase 3, and `transit.rs` holds the
  script and its tests; the example needs the `serde` feature. Its inner net
  is called `cabin` whatever the template (`Cabin`, `Deck`, `Pod`), its
  parameters are `stopped` (starting true), `seats` and `capacity`, the
  city's is `destination`, and every transition is built switched on (the
  script holds them back). Two guards follow §11 rather than the proposal:
  nobody sits or stands while an announcement stands, and the announcement
  is cleared only once nobody is aboard, so after step 4 it is not cleared,
  as the proposal's §6 has it, but stands until the vehicle is empty. Step 6
  saves the city's marking as JSON text after steps 3 and 5, loads each
  into a freshly built city, carries on, and compares the two cities'
  exported markings, which reach inside the vehicles.

The plan this section held before — a net as the *body* of a transition,
run to completion by a stateful closure, with `inner.as_transition(..)`
sugar — is superseded rather than forbidden: a body may still own a net, but
the engine sees nothing of it. A token is where an inner net lives when the
outer net should see it.

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
  `TakeAllEdge`, `TakeWhereEdge`) at launch; the clones travel with the
  firing, and on any failure in stage three (body `Err`, panic, bad
  `Distribution`) go back to the front of their places in original order
  **with their original `TokenId`s**, before the `Failed` event and without
  events of their own. A `TakeWhereEdge`'s token may have come from the
  middle of its place and goes back to the front all the same: the id is
  kept, the position is not (§8.3). Snapshot and mutate edges consume
  nothing, so nothing is restored for them.
  Cost: one clone per consumed token per fire of that transition. Build error
  (`RestoreOfNoClonePlace`) if a consuming input place is `no_clone`.
- **The net stays consistent** after any failure: body returned to its slot,
  transition not in flight, nothing distributed.
- **Inner changes are not rolled back** (slice 15, §6.6). A body that wrote
  into a token's inner net before it failed has written: restoring puts back
  the outer tokens the firing consumed, not the inner nets' markings.
- Drivers apply a policy on `FiringFailed`: `Stop` (default), `Disable` the
  transition and continue, or `Continue`.

Error types (`thiserror`, all `#[non_exhaustive]`):
- `BuildError` — from `NetBuilder::build`.
- `NetError` — API misuse: `NoSuchPlace`, `NoSuchToken`, `TypeMismatch`,
  `NoSuchTransition`, `NotFireable`, `ForeignFiring`, `NotInFlight`,
  `NoSuchParam` (a `set_param` of a type no transition declared),
  `NoClonePlace` (a snapshot of a net with a `no_clone` place). Wraps
  `FiringFailed` transparently so that `net.fire(t)` and
  `net.distribute(done)` return one error type. With `serde`, since slice
  16 (§6.6): `NotSerializable { place, name }` (an export of a net with a
  place not marked), `CannotSerialize { place, name, token, source }` (a
  token's own `Serialize` refused), `NoTemplate` (a fork of a net no
  template built), `InFlight { transition, name }` (a fork or a save while a
  firing is airborne), `NotForkable { type_name }` (the default
  `Nested::fork`), `CannotFork { place, name, token, source: Box<NetError> }`
  (a snapshot that could not fork a nested token) and `RebuildFailed {
  template, source }` (a fork whose factory failed or whose marking did not
  load back).
- `TemplateError` (`serde`, slice 16) — from `Templates::build`:
  `NoSuchTemplate { name }`, `Factory { name, source }`.
- `FiringFailed` — a body returned `Err` or panicked; carries the transition,
  the boxed error, and whether inputs were restored. Since slice 15 also
  `path: Vec<(PlaceId, TokenId, String)>`: where the transition is when it is
  inside a token's inner net, one `(place, token, net name)` per level, outer
  to inner, empty for the net's own. `step_deep` fills it in on the way up,
  and the message names it (`transition "Sit" (TransitionId(0)) inside
  PlaceId(0) #5 "cabin" failed: ..`).
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
    Mutated     { place: PlaceId, token: TokenId },      // MutateEdge or MutateWhereEdge, or replace()
    Toggled     { transition: TransitionId, enabled: bool },
    ParamChanged { type_name: &'static str },
    Nested      { place: PlaceId, token: TokenId, net: String, nested_id: NetId, event: Box<Event> },  // slice 15
    Resynced,                                            // slice 16: the marking was replaced wholesale (import_marking)
}
```

`Fired` is emitted by the inline path (`step`, `fire`); the staged path
(`launch` … `distribute`, which is what a concurrent driver uses) emits
`Launched` then `Distributed` because time passes between them. Per firing
the order is `Launched`, `Mutated`*, then `Distributed` or `Failed`; the
`state_counter` therefore moves once more on the staged path than on the
inline one.

`Nested` (slice 15) is an inner net's event, lifted into this log with the
place and token it happened in, the inner net's name and its stable id; the
six rules for when are in §6.6. With `serde` the wrapped event is under
`"inner"`, since `"event"` is the tag.

`Resynced` (slice 16) is Python petritype's event of the same name:
something changed that is not told as events. `import_marking` emits it
once, instead of a `Removed` and an `Added` per token, and moves
`state_counter` by one; every token after it has an id the reader has not
seen, so a view re-reads the marking. Python records one per place, with the
place's name; an import here replaces every place at once, so it is one,
with no fields. The runner already reported a variant it does not know as
`reloaded {reason: "resync"}`, which is what this one means, and since
slice 17 maps it so by name, inside an inner net as `nested` around it.

### 8.2 Read API

```rust
net.places()      -> impl Iterator<Item = (PlaceId, PlaceInfo<'_>)>    // by value: name, type_name, serializable, nested, passive, key_names (slice 18), count (count is live, never stored)
net.transitions() -> impl Iterator<Item = (TransitionId, TransitionInfo<'_>)>  // by value: name, inputs, filters (slice 19), outputs, positional (slice 17), enabled, in_flight, detachable
net.edges()       -> impl Iterator<Item = EdgeInfo>   // Input { place, transition, kind, filter (slice 19) } | Output { transition, place, slot }; transition order, inputs then outputs
net.topology()    -> Topology   // places, transitions, edges and topology_counter as owned values; shape only, no counts or switches (slice 14)
net.place_named(&str) -> Option<PlaceId>;  net.transition_named(&str) -> Option<TransitionId>
net.place_ref::<T>(&str) -> Option<PlaceRef<T>>   // slice 15: a typed handle by name, checked by TypeId
net.nested(place, token) -> Option<Vec<(&str, &Net)>>   // slice 15: a nested token's inner nets (§6.6)
net.nested_scope(place, token, f) -> Option<R>    // slice 15: the same, mutably, lifting on return
net.is_quiescent_deep() -> bool                   // slice 15: quiescent here and in every non-passive inner net
net.step_inside() -> Result<Option<DeepStep>, FiringFailed>   // slice 17: step_deep's inner half, for a driver that asked select() itself (§6.6)
#[cfg(feature = "serde")]
net.param_name(type_name) -> Option<&str>         // slice 17: the wire name behind a ParamChanged's type name, from serializable_param
net.fireable() -> Vec<TransitionId>;  net.is_fireable(t) -> bool;  net.is_enabled(t) -> bool;  net.priority_of(t) -> f64
net.count(place)  -> usize
net.tokens(place) -> impl Iterator<Item = (TokenId, &dyn Any)>
net.label(place, token) -> Option<String>            // via PlaceMeta.debug; truncated to label_limit (default 100) + "..."
net.debug(place, token) -> Option<String>            // the same Debug text untruncated: what label is cut from
net.token::<T>(place, token) -> Option<&T>           // typed downcast for custom widgets
#[cfg(feature = "serde")]
net.json(place, token) -> Option<serde_json::Value>  // structured view, serializable places only
#[cfg(feature = "serde")]
net.export_marking() -> Result<Marking, NetError>    // slice 16: every place's tokens as JSON by place name (§9)
#[cfg(feature = "serde")]
net.fork() -> Result<Net, NetError>                  // slice 16: rebuilt from its template, state copied across (§6.6)
#[cfg(feature = "serde")]
net.template() -> Option<&str>;  net.templates() -> &Templates   // slice 16: the stamp's name; the registry nested tokens are rebuilt from
net.snapshot() -> Result<Snapshot, NetError>         // owned copy of the *state*, not the net (see below); Err if any no_clone place
net.keys(place, token) -> Option<Keys>               // slice 18: the token's keys, derived now (§8.3)
net.query(&Filter) -> Result<Vec<Match>, NetError>   // slice 18: every token a filter lets through, from the index (§8.3)
net.distinct_values(key, places) -> Vec<KeyValue>    // slice 18: a key's values in order of first appearance
net.matches(&Filter, place, token) -> bool           // slice 18: one token against a filter, parameters read from the net
net.first_match(place, &Filter) -> Option<TokenId>   // slice 19: the token a keyed edge with this filter would choose now
net.topology_counter(), net.state_counter(), net.fired_count()
```

`Snapshot` (slice 11) is the marking, the switches, the in-flight set, the
parameters and the counters, cloned through the `PlaceMeta` vtables into an
owned `Send + Sync + Clone` value with the same read API names as `Net`
(`places`, `transitions`, `edges`, `topology`, `count`, `tokens`, `token`,
`label`, `debug`, `json`, `is_enabled`, `in_flight`, `param`, `nested` since slice 16, `keys`, `query`,
`distinct_values` and `matches` since slice 18, by scanning, the counters). It is *not* a `Net`, and
cannot be: bodies are `Box<dyn FnMut>` and cannot be cloned, and an in-flight
transition has no body in the net at all; a what-if copy is a rebuild from
the builder. What it is for: a renderer or inspector on another thread owns
a consistent copy while the driver keeps `&mut Net`, and a test keeps the
state at one moment to compare with another. The fireable set is not stored
(guards are closures on the live net). Snapshotting emits nothing and moves
no counter. Without `serde` a net with a nested place cannot be snapshotted;
with it (slice 16) the nested place's tokens are forked from their templates
(§6.6), and `snapshot.nested(place, token)` reads their inner nets, under the
live token ids.

There is no observer callback. The driver holds `&mut Net` between steps and
is the observer.

### 8.3 Keys, queries and the index

**Built 2026-10-09, slice 18.** Phase 1 of
`Cyberpunk2020-Simulation/dev-docs/TOKEN_QUERYING_AND_FILTERING.md` (its
R1 and R2; the Rust track's first item). A token may carry **keys**: named
attributes a place derives from each of its tokens by an extractor the
net's author writes. With keys, "every bird" is a question the net can
answer: a process asks for the tokens a filter lets through and gets each
one's place, id, keys, label and JSON; a viewer filters what it draws by
them, and makes its tabs from the values one key takes. The example is
`petritype/examples/wildlife.rs`, species across habitats, which is also
the protocol's conformance net `wildlife` (PROTOCOL.md §11).

```rust
pub enum KeyValue { Str(String), Int(i64), Bool(bool) }    // Clone, Eq, Hash, Ord; From<&str | String | i64 | bool>; Display
pub struct Keys;                                           // (name, value) pairs in declaration order: get, iter, names, len, is_empty
b.keys(place: PlaceRef<T>, names: [&'static str; N], extractor: impl Fn(&T) -> [KeyValue; N] + Send + Sync + 'static)

pub struct Filter { pub clauses: Vec<Clause>, pub places: Option<Vec<PlaceId>>, pub prefix: Option<String> }
#[non_exhaustive] pub enum Clause { In { key, values: Vec<KeyValue> }, IsParam { key, param } }
Filter::new().key_in(key, values).key_is_param(key, param).in_places(places).with_prefix(prefix)
filter.matches(&keys) -> bool;  filter.matches_with(&keys, |param| ..) -> bool

#[non_exhaustive] pub struct Match { pub place, pub token, pub keys: Keys, pub label: String, #[cfg(serde)] pub json: Option<Value> }
net.keys(place, token) -> Option<Keys>
net.query(&filter) -> Result<Vec<Match>, NetError>             // NoSuchPlace for a place `places` names that is not there
net.query_ids(&filter) -> Result<Vec<(PlaceId, TokenId)>, NetError>   // slice 20: the same tokens, nothing rendered
net.distinct_values(key, places: Option<&[PlaceId]>) -> Vec<KeyValue>
net.matches(&filter, place, token) -> bool
snapshot.{keys, query, query_ids, distinct_values, matches}  // the same answers, by scanning
PlaceInfo::key_names: &[&'static str];  TopologyPlace::key_names: Vec<&'static str>
```

- **Declaring.** `b.keys(place, ["species", "class", "diet"], |s: &Species|
  [..])`. The names are given statically, before any token exists, because
  a reader needs them first — a viewer's filter bar, the protocol's
  `key_names` (PROTOCOL.md §13.1) — and the extractor returns an array of
  exactly as many values, in the same order, so the two cannot drift. The
  token type needs no trait: the closure form is the one the proposal asked
  for at least, and the only one built (Python likewise built only its
  closure form). One declaration per place; a name repeated in one
  declaration is `BuildError::DuplicateKeyName { place, name }` and a second
  declaration for one place `BuildError::KeysAlreadyDeclared { place }`,
  both reported from `build()` like `NotNested`, the call itself staying
  infallible. Any place may declare keys, a nested one included.
- **Values** are strings, integers and booleans, nothing else (the
  proposal's §7), and compare by kind and then by value: `Int(1)`,
  `Str("1")` and `Bool(true)` are three values, as `1`, `"1"` and `true` are
  on the wire. With `serde`, `KeyValue` is untagged, the bare JSON value,
  and `Keys` serialises as an object in declaration order. `Ord` exists for
  sorted containers and is arbitrary across kinds.
- **The vtable** (§4.2) gains `keys: Option<KeysVtable>`: the names, and
  the extractor wrapped for an erased token. The extractor is the author's
  closure, so unlike `debug` and `clone` it cannot be a `fn` pointer; it is
  an `Arc<dyn Fn(&dyn Any) -> Vec<KeyValue> + Send + Sync>`, so that
  `PlaceMeta` stays `Clone` and a `Snapshot` shares it.
- **Keys are derived, never stored.** `Net::keys` computes them from the
  token each time it is asked, which is why it returns `Keys` by value
  where the proposal wrote `Option<&Keys>`. A `MutateEdge` body or a
  `replace` that changes what the extractor reads changes the token's keys;
  a decline that changes only `population` changes none. The extractor
  should be pure: a key that changes behind the net's back (by interior
  mutability, say) is one the index does not see. Nor should it panic: it
  runs inside the net's bookkeeping, where a panic is not caught as a
  body's is (§7), the footing a token's `Debug` and `Clone` are on already.
- **Reading the names.** `PlaceInfo::key_names` is a slice borrowed from
  the net rather than the `Vec` first planned, so that `PlaceInfo` stays
  `Copy` (the Bevy reader takes it by value); `TopologyPlace::key_names` is
  owned, and with `serde` written only when non-empty, as `nested` and
  `passive` are, so a net without keys has the topology it always had.
- **Filters.** A token passes when it passes every clause (AND across) and
  lies in a place the filter covers; a clause accepts one of its values (OR
  within), compared by kind and value. A token of a place that does not
  declare a clause's key passes no such clause, so a filter with a clause
  never keeps a token of a keyless place, while a filter with no clauses
  keeps every token of the places it covers. The restriction is by place
  only — `places` by id, `prefix` by the start of a place's *name*, the
  protocol's §13.2 and the Python as-built — and never by transition, which
  holds no tokens. A place in `places` that the net does not have makes
  `query` fail with `NetError::NoSuchPlace` (the wire's `unknown_place`); a
  key no place declares finds nothing and is not an error. `Filter` and
  `Clause` are plain Rust; since slice 19, with `serde`, they are also
  `Serialize` and `Deserialize` in the protocol's shape with places by id
  (below), and the runner keeps a wire shape of its own, keyed by place
  names.
- **A clause on a parameter.** `Clause::IsParam { key, param }` accepts one
  value: the current value of the net parameter registered under the wire
  name `param` with `serializable_param`, its JSON read as a `KeyValue`. A
  parameter whose JSON is not a string, an integer or a boolean, or a name
  the net has none under, accepts nothing. Wire names are a `serde`
  facility, so without the feature such a clause accepts nothing at all.
  A filter alone knows no parameters: `Filter::matches(&keys)` passes no
  such clause, `matches_with` takes a lookup, and `Net::query` and
  `Net::matches` read the net's own. The variant is `IsParam` and not the
  `Param` of the brief: a variant of that name beside `petritype::Param`
  makes rustc print `petritype::Param<T>` in full in every diagnostic that
  names a parameter input (the `trybuild` expectations caught it), the
  same trimmed-paths effect that kept `positional` a flag in §4.5.
- **Queries.** `Net::query` returns `Match`es in place order and then
  token order, the order `tokens` gives (oldest first); `label` is
  `Net::label`, `json` is `Net::json` for a serializable place.
  `Net::query_ids` (slice 20) finds the same tokens in the same order and
  returns their `(place, token)` alone, rendering no label and no JSON: what
  the runner's projection (§19.3.12) counts, pages and remembers, rendering
  only the tokens it writes. `Net::matches`
  asks the same question of one token, for a reader that has just been told
  of a token by an event. `Net::distinct_values(key, places)` lists the
  values in order of first appearance, place order then token order (the
  proposal's `Bird, Mammal, Reptile, Fish`; `Omnivore, Herbivore` for
  `Tundra`), and is empty for a key no place declares.
- **The index** is what lets a query cost what it finds. Per key name, per
  value, the ordered set of `(PlaceId, TokenId)` holding it
  (`HashMap<&'static str, HashMap<KeyValue, BTreeSet<..>>>`, values no token
  holds removed). A query with clauses resolves its parameters once, asks
  the index how many tokens hold each clause's values, starts from the
  clause with the fewest, and reads only those tokens — found in their place
  by binary search, since a place's queue is in id order — checking the
  other clauses against their keys. A query without clauses reads the
  places it covers in full; it keeps every token in them anyway.
  `distinct_values` reads the index alone. It is kept by the net at every
  point a token enters a place, leaves one or changes in it: the one
  insertion path `insert_erased` (seeds, `put`, `extend`, `set_tokens`,
  `apply`, a firing's outputs, an import), a failed firing restoring what it
  took, the consuming edges at launch, `remove`, the clearing in
  `set_tokens`, `replace`, and a `MutateEdge` body, whose tokens' keys are
  taken before the body runs and again after it (a panicking body included).
  No token's keys are stored: the extractor is pure, so a token's old
  entries are found by extracting from the value about to leave, or from the
  value before the body changed it. The one change the net learns of only
  afterwards is inside a nested token — an inner firing of `step_deep`, a
  `nested_scope` closure — where a key may read the inner nets; there the
  token's entries are found by looking through its keys' values, and it is
  indexed afresh. `import_marking` empties the index and refills it as it
  refills the places; nothing of it is ever saved (the proposal's R7). A
  `Net`'s `Debug` prints how many values each key has, never the values.
- **Token order.** The index's sets are in place order then id order, and
  a place's queue is in id order too, except after two restoring firings of
  one place land out of order behind a token put in meanwhile (§7). Such a
  place is marked unordered until it next empties, and a query or a
  `distinct_values` reads it in queue order instead. Correctness first: the
  crate's own tests compare the kept index with one rebuilt from the
  marking after every kind of change, and `tests/slice18.rs` compares the
  net's answers with a snapshot's and with the read API by hand.
- **A snapshot** has no index of its own: `Snapshot::query`,
  `distinct_values`, `keys` and `matches` scan, and give the net's answers
  for the same moment, parameters included.
- **What Python does differently** (its "As built"). No index: keys are
  derived on demand and a query scans, the index being this side's
  performance measure; the spelling is `ListPlaceNode(keys={..})`,
  `Keys.of`, `Filter.where(class_="Bird")`, `Query.matches` and
  `Query.distinct`; a clause holds `values` or `param`, as here, and a
  filter names places by name, which the runner will translate to ids.

**Keyed input edges, 2026-10-09** (slice 19). Phase 2 of the same proposal:
its R5, the Rust track's fourth item. Two input edges choose their token by
its keys instead of taking the front one, so that "the oldest bird in
`Forest`" is one edge rather than a transition that takes the whole place
and puts most of it back. Python built the same edges (its "As built"),
under the same names.

```rust
pub struct TakeWhereEdge<T>(pub PlaceRef<T>, pub Filter);     // body parameter T
pub struct MutateWhereEdge<T>(pub PlaceRef<T>, pub Filter);   // body parameter &mut T
#[non_exhaustive] pub enum EdgeKind { .., TakeWhere, MutateWhere }   // "take_where", "mutate_where"
kind.is_keyed() -> bool;  EdgeKind::TakeWhere.consumes() == true
TransitionInfo::filters: &[Option<Filter>]     // beside `inputs`, Some exactly for a keyed edge
TopologyTransition::filters: Vec<Option<Filter>>
EdgeInfo::Input { place, transition, kind, filter: Option<Filter> }   // no longer Copy
net.first_match(place, &filter) -> Option<TokenId>
BuildError::KeyedEdgeOnUnkeyedPlace { transition, place }
#[cfg(serde)] Filter, Clause: Serialize + Deserialize   // the protocol's shape, places by id
```

- **Fireable** when, besides the usual conditions, the edge's place holds
  a token whose keys pass every clause of the filter. The filter is asked
  of that one place: its `places` and `prefix` are not consulted, by the
  edge or by `first_match` (a query still consults them). A
  `Clause::IsParam` reads the parameter's value whenever fireability is
  asked, so a `set_param` can make a keyed transition fireable or not
  without a token moving. The guard runs after, as it always has, and so
  only when there is a token to be had; it reads that token with
  `Net::first_match`, as a guard on a plain edge reads the front one. A
  filter without clauses accepts every token, and the edge behaves as its
  plain counterpart; a clause on a key the place does not declare accepts
  none, and the edge never fires.
- **Which token.** The first in queue order whose keys pass: the oldest,
  unless a restoring firing has put a younger one in front (token order,
  above). A `TakeWhereEdge` takes it from wherever it stands and the body
  is handed it as a `TakeEdge`'s would be; a `MutateWhereEdge` lends it to
  the body where it stands, as a `MutateEdge` lends the front one, and like
  that edge makes the transition non-detachable and reports the token in a
  `Mutated`. Every other token stays where it was, under its id: the
  firing's `Fired` (or `Launched`) names the one token consumed, and no
  `Removed` or `Added` is emitted for the rest.
- **Asked of the index**, without extracting a key. Per clause, the
  postings of each value it accepts; a key or a clause no token holds
  answers "none" at once. The clause holding the fewest tokens gives the
  candidates, each of its values' sets walked in id order within the
  place until a token is also in some posting of every other clause; the
  earliest of those is the token, and a binary search finds its position.
  An unordered place is walked in queue order instead. So a keyed edge on
  a long place whose matches are rare costs the matches it tries, not the
  place's length, and fireability is asked at every scan. The crate's
  tests compare it with a front-to-back scan after every kind of change.
- **The index kept right** at the same points as before: a token taken
  from the middle leaves it as one taken from the front does (removal from
  the middle keeps a place in id order), and a mutated token's keys are
  taken before the body and again after, found by its position in the
  place, which the body cannot change. The mutating path no longer assumes
  the front: stage one records, per mutating edge, the argument it fills
  and where its token stands (`Lent`), and the borrow, the reindexing and
  the event are all made from that.
- **Restore.** `restore_tokens_on_failure` puts a `TakeWhereEdge`'s token
  back at the **front** of its place, as every restore does, with its id:
  the id is kept, the position is not. It is then the first token the
  filter accepts, so a retry takes it again; a place it re-entered ahead
  of an older token is marked unordered, as before.
- **Checks.** A keyed edge reads its place like any other
  (`DuplicateInputPlace`). `TakeWhereEdge` consumes (`EdgeKind::consumes`),
  so it counts in `assert_acyclic`, in `assert_no_source_transitions` and
  in `RestoreOfNoClonePlace`; `MutateWhereEdge` consumes nothing and counts
  in none of them. A keyed edge on a place that declares no keys is
  `BuildError::KeyedEdgeOnUnkeyedPlace { transition, place }`: no token
  there could ever pass, and a transition that can never fire is cheaper
  refused at build than found by watching a net that does not move. (There
  is no warning channel to say it more gently.) A wrong body parameter is
  a compile error, as for every edge (`tests/ui/fail_take_where_vec.rs`,
  `fail_mutate_where_by_value.rs`).
- **What it cost the types.** The filter is not `Copy`, so it lives
  *beside* the kind: `EdgeKind` stays `Copy` and `TransitionInfo` keeps its
  `inputs` slice, gaining a parallel `filters` slice (still `Copy`, being
  borrowed). `EdgeInfo::Input` carries the filter and lost `Copy`, which is
  the one change a reader of the old shape must make (a `filter: None` where
  it builds one, `..` where it matches one, a borrow where it copied one).
  The sealed `EdgeSealed` lost its `Copy` bound, and `spec` takes `&self`.
  `EdgeKind` became `#[non_exhaustive]` with the new variants: a matcher had
  to change for them anyway, and will not have to for the next.
- **On the wire** (PROTOCOL.md §5.1, §13.5) a keyed edge is `kind:
  "take_where"` or `"mutate_where"` with `filter` as declared, a `{key,
  param}` clause unevaluated. The core's `Serialize` for `EdgeInfo` and
  `Topology` writes that shape in its id form —
  `{"edge":"input",..,"kind":"take_where","filter":{"clauses":[{"key":"class","values":["Bird"]}]}}`,
  `places` as `PlaceId`s, `filter` and `filters` written only where there
  is one — because a `Filter` alone knows no names: the protocol's shapes
  keyed by name live in the runner (§19.2), which maps the ids; since the
  runner's half of keys (slice 20, §19.3.12) it sends `kind` and `filter`
  with places by name, and sent a keyed edge without `kind` until then.
  `petritype-dot` draws one as its plain counterpart labelled with the
  filter (`class in {Bird}`).
- **The example.** `Migrate Forest Wetland` is one
  `TakeWhereEdge(forest, class in {Bird})` into `Migrating`, PROTOCOL.md
  §11's `take_where`; its guard keeps the test on `season`, which is a
  parameter and not a key, so not a clause. The proposal's done-when is
  its test: on a `Forest` of a robin and a fox, one `Fired`, one token
  consumed and one produced, the fox under its old id, the five questions
  answered by the index after it; and with the fox in front and an owl
  behind it, the owl is taken.

**Pattern notes.** The game's pattern P2
(`Cyberpunk2020-Simulation/docs/world-net-design.md` §8.1, "select by
key": take the whole place, a guard that scans it, a body that splits it,
the rest returned through a `Distribution`) is one keyed edge now. Its cost
was per firing, in the size *n* of the place: *n* tokens consumed and *n*
produced (a remove and an add per token, as the proposal counts them),
*n* − 1 of them tokens nothing happened to, back where they were under
fresh ids, and a viewer re-animating every one; and the choice was made
twice, by a scan in the guard and another in the body.
A keyed edge costs one token out and, if the body says so, one in; the
others are not mentioned and keep their ids; and the choice is one question
to the index, asked at fireability and again at launch. `wildlife`'s
`Migrate` was P2 until this slice. P1 (a guard comparing the *front*
token's key) stays right where only the front may go, since FIFO order is
the point there; P2 is the pattern this replaces.

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
  `NetId` only `Serialize`, since slice 15 (§4.1).

Input may arrive whenever the caller holds `&mut Net`, including while
firings are in flight.

**Done, slice 16** (`serde`): a whole marking, out and back in.

```rust
pub struct Marking;   // place name -> tokens as serde_json::Value, oldest first; BTreeMap, #[serde(transparent)]
Marking::new(); marking.get(place) -> Option<&[Value]>; marking.insert(place, tokens); marking.iter(); len(); is_empty()
net.export_marking() -> Result<Marking, NetError>
net.import_marking(&Marking) -> Result<(), CommandError>
```

- `export_marking` writes every place, empty ones as `[]`, in name order,
  so the same marking always prints the same text:
  `{"Door": ["Ada"], "Seats": ["Bram", "Cato"]}`. Every place must be
  marked `serializable` (`NetError::NotSerializable`, the first offender,
  checked before anything is encoded), nested places included, whose tokens
  then carry their inner nets in `Net`'s saved form (§6.6). Reading is not a
  change; tokens airborne with a firing in flight are in no place and in no
  marking.
- `import_marking` is a wholesale replacement: every check first
  (`NoSuchPlaceName`, then `NotSerializable` for any place of the net, then
  `InvalidToken`), then every place emptied and filled; a place the marking
  does not mention loads empty. **Token ids are not preserved**: each token
  gets a fresh id from this net (an id-preserving variant can follow). The
  event rule: one `Event::Resynced` and nothing per token (§8.1), the
  incoming nested tokens' inner logs forgotten as setup. Switches,
  parameters and the in-flight set are untouched. Atomic, like `apply`.

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
- `passive` on a place not declared with `place_nested` (`NotNested`, the
  first one marked; slice 15). `passive` itself stays infallible, as the
  other per-place options are.
- A `keys` declaration that names a key twice (`DuplicateKeyName { place,
  name }`), or a second `keys` declaration for one place
  (`KeysAlreadyDeclared { place }`); the first offender, after `NotNested`
  (slice 18, §8.3). `keys` itself is infallible.
- One input edge per place per transition (`DuplicateInputPlace`).
- `SnapshotEdge` from a `no_clone` place.
- `restore_tokens_on_failure` on a transition with a consuming (`TakeEdge`,
  `TakeAllEdge`, `TakeWhereEdge`) input from a `no_clone` place.
  (A nested place counts as `no_clone` for both, with `serde` too: its
  tokens are copied by a fork, which can fail, and these two copy on the
  launch path, where nothing may; §4.3, §6.6.)
- A keyed edge (`TakeWhereEdge`, `MutateWhereEdge`) on a place that declares
  no keys (`KeyedEdgeOnUnkeyedPlace { transition, place }`, slice 19,
  §8.3): it could never fire. Checked edge by edge with the three above, the
  first offender in transition order.
- A declared `Param::<T>` with no `param(value)` of that type
  (`MissingParam`); with `serde`, two parameter types sharing a wire name
  (`DuplicateParamName`). Both before routing.
- Routed-output resolution (§4.5): `UnroutableOutput`, `AmbiguousOutput`,
  `UnreachableOutput`, each naming the transition and the type or place.
- (Output slot count vs output place count is carried by the types; a
  `debug_assert!` in `distribute` guards the erased index.)

Opt-in:
- `b.assert_acyclic()`: iterative three-colour DFS over consuming input edges
  (`TakeWhereEdge` among them) and output edges; read edges excluded (they
  move no tokens). Roots are
  transitions in declaration order; the first cycle found is reported as
  `BuildError::Cycle { path }`, node names from the repeated node round to
  itself (first == last, so it normally starts at a transition), joined with
  `" -> "` in the message.
- `b.assert_no_source_transitions()`: every transition has at least one
  `TakeEdge`, `TakeAllEdge` or `TakeWhereEdge` input; a transition with
  only read edges (`MutateWhereEdge` among them) fails.
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

**Done, slice 15** (nets as tokens, §6.6):

- `Mode::Sequential`, in `run` and `run_indefinitely`, is `step_deep` in a
  loop rather than `step`, so running a net runs its inner nets, in §6.6's
  order and deterministically. `quiesced` is deep quiescence; the end of
  `run_indefinitely` asks `is_quiescent_deep`.
- `Mode::Concurrent` keeps its launch sweep for the outer net. When a sweep
  launches nothing and nothing is airborne, it makes one inner firing
  inline with `step_inside`, as `step_deep` would choose it, and sweeps
  again (an outer guard
  may read an inner marking). Inner nets never get threads of their own in
  this version, and the ordering guarantee is the sequential driver's.
- Both limits count every launch, inner ones included, and the fuse blames
  the first fireable transition at any depth. `RunSummary.fired_count`
  counts inner successes as well as the outer net's: an inner firing is not
  in the outer `fired_count`, so the driver counts it as it lands.
- `OnFailure::Disable` switches off an inner transition where it is, found
  by the failure's `path`; its `Toggled` is lifted like any inner event.

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
| `petritype-runner` | turns a user's binary into a net process speaking the shared protocol (**done, slice 14**, §19.3) | `petritype` with `serde`, `serde_json`, `rustix` on Unix |

Examples (each also an integration test), mirroring the Python acceptance set:
quickstart, parcel distribution, caching (union return → routed enum),
match-up tokens (`TakeAllEdge` + guard + positional), coloured balls
(`Distribution`), web scrape (routed enums), execution modes (sequential vs
concurrent), read-edge scaler (`SnapshotEdge` + `MutateEdge`). A ninth,
transit (slice 15), is no port: it is the nets-as-tokens proposal's example
and acceptance script (§6.6). Since slice 16 it is also the protocol's
conformance net `transit`, in two files (`examples/transit.rs` for the
script and its tests, `examples/transit/net.rs` for the net, which a
runner's example can include), and needs the `serde` feature
(`required-features`), since its vehicles save their cabins as JSON.
A tenth, wildlife (slice 18), is the token-keys proposal's example and the
protocol's conformance net `wildlife`, split the same way
(`examples/wildlife.rs` for the proposal's questions and their answers as
tests, `examples/wildlife/net.rs` for the net) and needing `serde` for the
same reason: its species are JSON and its `season` is a wire parameter.
Since slice 19 its `Migrate` is a keyed edge (§8.3).

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
theme and Reset landed in the same repository. **2026-10-02:** mouse pan
and zoom and a read-only inspector panel landed too; a menu of options
registered by the net's author is deferred. **2026-10-03:** the remote
net source (a net process watched over the protocol, §19.4) was designed
in `../petritype-bevy/DESIGN.md` and its first slice landed: every system
reads the net through a `NetReader` over whichever source is present,
`NetRes` being the in-process one. **2026-10-04:** slices 2 to 4 landed:
`RemoteNet` mirrors a net process over the protocol, is driven from the
same controls, and `RemoteNet::spawn` starts either runner; the `remote`
example shows the conformance net under Python or under this
workspace's `parcel_runner`, and its process tests run against both.
The
first run on screen, the same day, found one glitch, fixed as slice 4a: a
token is hidden while its tween waits, so a one-piece firing's output no
longer shows at the transition before its input has arrived.
**2026-10-05:** slice 5 landed, the last transport: `HttpTransport`
behind the `http` feature reaches a net already loaded on a
petritype-server worker, requests as the server's routes and
notifications as server-sent events, with no `describe` or `load`. The
relay does not keep the pipe's order between an answer and the
notifications its request caused, so the transport holds a state-changing
answer until the stream reaches its version, and the mirror ignores a
version it already holds instead of calling it a gap (protocol §9).

**2026-10-09:** nested nets (phase 4 of nets as tokens, R9): a token that holds nets is
badged with the tokens inside (from `nets[].count`, kept from `nested`; locally from
`Net::nested` and `Event::Nested`), and `RemoteNet::descend` shows its first inner net in the
same window, read by `nested_id` so the view stays attached as the token moves, with
`Enter`/`Backspace`, a breadcrumb, and a return up when the inner net is gone; one level,
remote only, not over HTTP (the server relays no `nested_id`); 190 tests with `http`. **2026-10-09:** token keys (phase 4, R4): keys and edge
filters on the wire and in the mirror, a `TokenFilter` that composes with
`ViewFilter` (hidden tokens keep their entities, marked `FilteredOut`;
counts read `matching / count`; a banner clears), a filter bar with
toggles and tabs (`K`), `distinct` asked only when the mirror knows fewer
tokens than the net holds, keyed edges dashed; the `set_filter` projection
deferred (bevy DESIGN §3.6); 213 tests with `http`.

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
| finding the nets inside a token whose type the net has forgotten | trait `Nested`; `T::nets` / `T::nets_mut` monomorphised into `PlaceMeta` as `fn` pointers, as `debug` and `clone` are (slice 15) |
| an event inside an event inside an event | `Box<Event>` in `Event::Nested`: one level per box, depth unbounded |
| key names and values that cannot drift apart | const generics: `names: [&'static str; N]` and an extractor returning `[KeyValue; N]` (slice 18) |
| a per-place closure in a vtable that must stay `Clone` | `Arc<dyn Fn(&dyn Any) -> Vec<KeyValue> + Send + Sync>` beside the `fn` pointers (slice 18) |
| a query that costs what it finds, in place then token order | an index of `HashMap<KeyValue, BTreeSet<(PlaceId, TokenId)>>` per key name, kept by the net (slice 18) |
| an edge that chooses its token by keys, without extracting any | the same index: a `range` over one place of the smallest clause's sets, membership in the others' (slice 19) |
| a filter on an edge without losing `Copy` on the edge kind | the filter beside the kind (`TransitionInfo::filters`, a parallel slice), and on `EdgeInfo`, which gave up `Copy` (slice 19) |
| FIFO places | `VecDeque` |

## 17. Open questions

- `MutateEdge` implementation: **resolved, slice 2.** The GAT route works
  under the arity macro and the `for<'a> FnMut` bound; no fallback.
- Identity preservation for pass-through bodies (`Keep(token)`): deferred
  until an animation needs it.
- `Broadcast(value)` sugar: deferred.
- Stamped net id in `PlaceRef`: **done, slice 2** (§4.1).
- Observability of an inner net used as a transition (§6.6): how a visualiser
  reaches inside. Deferred until the Bevy adapter needs it; **answered
  2026-10-09, slice 15**, by moving the inner net from a body into a token.
  The net lifts every inner event into its own log as `Event::Nested`, with
  the place and token id of the moment and the inner net's stable
  `NetId`; a visualiser reads an inner net with `net.nested(place, token)`
  and follows one by `nested_id` across moves. Paths on the runner protocol
  and a "descend" view in the Bevy adapter are phases 3 and 4 of the
  proposal.
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
- Interop follow-ups (§19), deferred until a user asks, except the first,
  **done 2026-10-03**: explicit `PlaceId`/`TransitionId`/`TokenId`
  constructors for a remote mirror (§4.1), instead of going through their
  `serde` form; parameter lists for
  `describe` derived from a schema crate rather than written by hand;
  running a net process's bodies with `Mode::Concurrent` (`start` launches
  one firing at a time in v1); a non-Unix answer to the stdout guard.

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
    2026-10-02 (26 headless tests). Remote source designed 2026-10-03;
    slices 1 to 4 of 5 landed by 2026-10-04 (the `NetReader`, the
    remote mirror, driving, the child process; 98 tests, five of them
    run against each real runner). Slice 5, HTTP through the server,
    landed 2026-10-05 behind the `http` feature (122 tests with it, 13
    of them against a fake control plane on a local socket).
14. Interop (§19): `PlaceInfo::serializable`, the owned `Topology`,
    `Serialize` on `Event`, `EdgeInfo` and `EdgeKind`; then the
    `petritype-runner` crate (function-table registry, `--describe`, the
    stdout guard, the owner loop), tested against protocol transcripts.
    Core part and runner crate landed 2026-10-03. The Python repository
    adopted the transcript format the same day and holds the shared set
    (`../petritype/tests/protocol/`, four files over the conformance net
    `../petritype/tests/protocol_nets/parcel_distribution.py`, a copy of
    our `parcel_distribution` example). Done 2026-10-03: the runner
    example carries that net under the shared name (the demo net became
    `parcel-fleet`), the harness has the §11 masks, and all four shared
    files hold against this runner (§19.3.10).
15. Nets as tokens (§6.6), 2026-10-09, phase 1 of
    `Cyberpunk2020-Simulation/dev-docs/NETS_AS_TOKENS.md` (its R1 to R4):
    `Nested` and `place_nested`, passive places, `step_deep` and
    `is_quiescent_deep`, `nested` and `nested_scope`, `place_ref`,
    `Event::Nested` and the six lifting rules, `NetId` as the stable id,
    `FiringFailed.path`, `BuildError::NotNested`, the drivers stepping deep;
    the `transit` example running the acceptance script's steps 1 to 5 and
    7, and `tests/slice15.rs`. Phase 2 (serialising nested tokens,
    templates, marking export, a cloneable nested place, step 6) and phase 3
    (the runner protocol) follow.
16. Nets as tokens, phase 2 (§6.6), 2026-10-09, the proposal's R5a, R5b,
    R5 and R6, all behind `serde`: `Marking`, `export_marking` and
    `import_marking` with `Event::Resynced` (§9); `Templates`, `Params`,
    the stamp, `NetBuilder::template` and `templates`, `Net::template` and
    `templates`; `Serialize` and `Deserialize` for `Net` in PROTOCOL.md
    §12.5's form, through the thread-local template scope; `Net::fork`,
    `Nested::fork` with a refusing default, the vtable's `fork`, and
    `Net::snapshot` forking nested places, with `Snapshot::nested`; seven
    `NetError` variants and `TemplateError`. The transit example became the
    protocol's conformance net (PROTOCOL.md §11) in `examples/transit/net.rs`
    and runs the acceptance script's step 6; `tests/slice16.rs`.
17. Nets as tokens, phase 3 (§19.3.11), 2026-10-09, the proposal's R8 on
    PROTOCOL.md §12: the runner addresses inner nets by `path` or
    `nested_id`, keeps a record per inner net (its own `marking_version`,
    names, progress, fireable set), wraps inner events in `nested` in the
    wire's order, steps inner nets inline, lists `nets` on token entries,
    filters `get_log` by the chain of nets, and carries the conformance net
    `transit` (the core example, included by path); the shared transcript
    `05_nested_nets.json` replays unchanged. The core gained
    `Net::step_inside`, `Net::param_name` and `TransitionInfo::positional`;
    `tests/slice17.rs`.
18. Token keys, phase 1 (§8.3), 2026-10-09, R1 and R2 of
    `Cyberpunk2020-Simulation/dev-docs/TOKEN_QUERYING_AND_FILTERING.md`:
    `KeyValue`, `Keys`, `NetBuilder::keys` with `BuildError::DuplicateKeyName`
    and `KeysAlreadyDeclared`, the vtable's `keys`, `PlaceInfo::key_names`
    and `TopologyPlace::key_names`; `Filter`, `Clause`, `Match`,
    `Net::keys`, `Net::query`, `Net::distinct_values` and `Net::matches`,
    the same four on `Snapshot` by scanning; the key index and every point
    that keeps it. The example `wildlife`, the protocol's conformance net of
    that name with `Migrate` in its before-keyed-edges form, holds the
    proposal's §6 answers; `tests/slice18.rs`. Next, as it turned out: keyed
    edges (R5, slice 19), then the runner (PROTOCOL.md §13, the proposal's
    R3), each on a branch of its own.
19. Token keys, phase 2 (§8.3, "Keyed input edges"), 2026-10-09, R5 of the
    same proposal: `TakeWhereEdge` and `MutateWhereEdge`, `EdgeKind::
    {TakeWhere, MutateWhere}` (and `EdgeKind` `#[non_exhaustive]`),
    `EdgeKind::is_keyed`, `TransitionInfo::filters`,
    `TopologyTransition::filters`, the filter on `EdgeInfo::Input` (which
    lost `Copy`), `Net::first_match`, `BuildError::KeyedEdgeOnUnkeyedPlace`,
    and with `serde` `Filter` and `Clause` in the protocol's shape (places by
    id). Fireability and the token chosen are asked of the key index; the
    mutating path finds its token by position rather than at the front.
    `Migrate Forest Wetland` in `wildlife` is one `TakeWhereEdge`, and the
    proposal's done-when is its test; the game's pattern P2 is one edge
    (§8.3, "Pattern notes"). `petritype-dot` labels a keyed edge with its
    filter; the runner sends one without `kind` until its own half of keys.
    `tests/slice19.rs`, two `trybuild` cases.
20. Token keys, phase 3 (§19.3.12), 2026-10-09, R3 of the same proposal on
    PROTOCOL.md §13: the runner puts `key_names` on places, `keys` on token
    entries, `added` and `mutated`, and as the fifth element of a `produced`
    entry; sends keyed edges with `kind` and `filter` by name; answers
    `query`, `distinct` and `set_filter`; and projects each net, the outer
    one or an inner one, through the connection's filter, with `matching`
    in reads and `hidden` in place of what the filter hides. It carries the
    conformance net `wildlife` (the core example, included by path), and
    the shared `06_keys_query_filter.json` replays unchanged, as 01 to 05
    still do. The outer `progress` counts inner firings under their paths,
    as Python's does. The core gained `Net::query_ids` and
    `Snapshot::query_ids`; `tests/slice20.rs`.

## 19. Interop: one protocol, two runtimes, two viewers

**Decided 2026-10-03.** A net can run on the Python library or on this
crate, behind the same server, and be watched in the Svelte frontend
(petrivelte) or in the Bevy visualiser. A net is written in one language, its
bodies are code, so "mix and match" means choosing the runtime per net and
the viewer per session; a net never moves between languages. What makes it
work is one protocol, specified once in the Python repository:
[`../petritype/PROTOCOL.md`](../petritype/PROTOCOL.md)
(<https://github.com/olenive/petritype/blob/main/PROTOCOL.md>). A net process
speaks it as newline-delimited JSON on stdin and stdout; the server relays
the same shapes over HTTP and server-sent events. This section is the Rust
side only and does not restate the shapes. Design proposal, not yet
implemented.

The decisions that shape it, all of 2026-10-03:

- **The protocol lives in the Python library**, as its optional
  `petritype.runner` module, because any viewer can then host a Python net
  by spawning it, with no server. This crate conforms to that spec; the
  shared transcripts (`PROTOCOL.md` §11) are its acceptance tests.
- **Layout belongs to the viewer.** The protocol carries topology and
  optional position hints. No runner computes coordinates, so nothing here
  grows a layout; `petritype-dot` is unaffected and stays a rendering of
  its own.
- **Rust nets reach petritype-server as a built Linux binary**: the server's
  build pipeline gains a cargo step and the worker spawns the artifact. No
  Rust toolchain on the worker. Details in
  `../petritype-server/dev-docs/RUNTIMES_AND_PROTOCOL.md`.
- **Python adopts this crate's model of identity**: monotonic token ids that
  are never reused, and consumed/produced per firing. Nothing in §4.1 or
  §8.1 changes; the protocol's `fired`, `launched`, `distributed`, `failed`,
  `added`, `removed`, `mutated` and `toggled` are `Event`'s variants with
  names in place of ids.

### 19.1 Mapping onto §8 and §9

| protocol | Rust |
|---|---|
| `topology.places` | `net.places()`: `name`, `type_name`, and `serializable` (new field, §19.2) |
| `topology.transitions[].detail` | `TransitionInfo::params` (type names) and `detachable` |
| `topology.edges` | `net.edges()`: `EdgeKind::{Take, TakeAll, Snapshot, Mutate}` as `"take"`, `"take_all"`, `"snapshot"`, `"mutate"`; `Output { slot }` as `slot` where the transition has more than one output and `TransitionInfo::positional` (slice 17; a routed or distributing body has no slots to speak of) |
| keyed edges, `kind: "take_where"` / `"mutate_where"` with `filter` (§5.1, §13.5) | `EdgeKind::{TakeWhere, MutateWhere}` and `EdgeInfo::Input::filter` (slice 19), the filter's `places` mapped from ids to names by the runner (slice 20; sent without `kind` before it) |
| `topology.places[].nested`, `passive` (§12.2) | `TopologyPlace::nested`, `passive`, written only when true |
| `path`, `nested_id` (§12.1) | a path walked through `net.nested(place, token)` level by level; a nested id is the inner net's `NetId` printed with its `Display`, found by walking every nested place at every depth; mutable reach (`inject`) through `nested_scope` at each level |
| a token's `nets` (§12.2) | `net.nested(place, token)`: each net's name, `id()`, its path, `template()`, and the sum of its places' counts |
| `nested` notification (§12.4) | `Event::Nested`, unwrapped to the event the innermost net logged: one wrapper with the full path, the inner notification built with that net's names and version |
| inner `marking_version` (§12.3) | the runner's counter per inner net, one per state notification of that net; the outer one does not move |
| `step` or a running net, inside (§12.3) | `select()`; when it picks nothing, `step_inside()`, inline: `nested` around `fired` |
| `topology.places[].key_names`, a token's `keys` (§13.1) | `TopologyPlace::key_names`, written only when non-empty; `Net::keys`, read when the line is written (slice 20, §19.3.12) |
| a `produced` entry's fifth element (§7.1) | `protocol::Produced`: three elements, or five with `null` for the colour Rust never has when the place declares keys |
| a request's filter (§13.2) | `protocol::Filter::parse`, then place names resolved to the named net's ids: a `Filter` with `Clause::{In, IsParam}` |
| `query`, `distinct` (§13.3) | `Net::query_ids` (slice 20), rendering only the matches written; `Net::distinct_values` |
| `set_filter`, `matching`, `hidden` (§13.4) | a `Projection` per net, the runner's (§19.3.12): `Net::query_ids` for reads, `Net::matches` for each token an event names, a set of what was shown for the tokens that are gone |
| `transitions[name]` | `is_enabled`, `is_in_flight`, membership of `fireable()`, `priority_of` |
| `marking` | `tokens(place)` ids; the wire `label` is `debug(place, token)` cut by the runner at the protocol's 1000 characters, whatever the net's own `label_limit` |
| token id `"17"` | `TokenId` printed with its `Display` (the bare number) |
| `progress.step_count` | `fired_count()`, plus, for the outer net, the firings the runner made inside its tokens (slice 20) |
| `progress.last_fired`, `fired_counts`, `last_progress_at` | kept by the runner from the events: the core dropped them on purpose (§2) and does not get them back; the outer net's count an inner firing under its path without the net's name, then the transition (`Depot/#17/cabin/Sit`) |
| `marking_version` | the runner's own counter, one per state notification. Not `state_counter`, which starts at 0 again when `reset` rebuilds the net |
| `generation` | the runner's counter of loads and resets; a rebuilt `Net` numbers its tokens from the start again, which is why the protocol has generations at all |
| state notifications | `Event` variants, ids resolved to names through the read API, produced tokens labelled with `label()`; `ParamChanged { type_name }` reported under the parameter's wire name from `serializable_param` when it has one |
| `fireable` notification | the runner compares `net.fireable()` after each batch of events |
| `step`, `start` | `select()`, `launch`, the body on a thread of its own, `distribute`; one firing at a time, so `Launched` then `Distributed` |
| `inject {place, token}` | `apply_json` of `JsonCommand::Extend` with one token; the new id is read off the `Added` event. Needs a `serializable` place, else `not_serializable` |
| `enable`, `disable` | `net.enable(t)`, `net.disable(t)` (`Toggled`) |
| `set_param {param, value}` | `JsonCommand::SetParam`; `transition` ignored; `invalid_params` for a name the net has no parameter under (slice 17; `unsupported` before), `unsupported` for a parameter with no wire name |
| `get_token {id}` | `net.json()` gives `kind: "json"`; otherwise `debug()` gives `kind: "text"`, untruncated |
| `reset` | rebuild from the same factory and parameters |

The runner owns the `Net` on one thread and is its only writer, which is
§9's single-writer rule unchanged. A detached body holds no borrow of the
net (§6.2), so `get_state` and `stop` are answered from the live net while a
body runs, without `snapshot()`.

### 19.2 Additive work in the core

All landed 2026-10-03, the core part of slice 14. All behind the existing
`serde` feature except the first two:

- `PlaceInfo::serializable: bool`. `PlaceInfo` is `#[non_exhaustive]`, so
  adding a field breaks nothing. Always present, `false` without `serde`.
- `Net::debug` and `Snapshot::debug`, the `Debug` rendering before
  `label_limit` is applied. Done 2026-10-03. Without it a non-serializable
  token's "full value" on the wire would have been the net's 100-character
  label.
- An owned `Topology` (`net.topology()`, and the same on `Snapshot`): places,
  transitions and edges as owned values, `Serialize` with `serde`. A render
  thread or a runner can then hold the shape of the net without a borrow.
  As built: `Topology { places: Vec<TopologyPlace>, transitions:
  Vec<TopologyTransition>, edges: Vec<EdgeInfo>, topology_counter }`, shape
  only (no counts, no switches), each entry carrying its id although index
  and id agree, and no `NetId` because that is process-local. One
  crate-private constructor builds it from the read API, and `Snapshot`
  gained `edges()` so both sides derive edges through the same helper.
- `Serialize` (not `Deserialize`) on `Event`, `EdgeInfo` and `EdgeKind`, in
  their id form, for logs and tests. Shapes: `EdgeKind` is a snake_case
  string (`"take_all"`); `EdgeInfo` is tagged under `"edge"`, because
  `"kind"` is the input edge's `EdgeKind`; `Event` is tagged under `"event"`
  with variant names in snake_case (`"param_changed"`).
- Slice 19: `Serialize` and `Deserialize` on `Filter` and `Clause`, in the
  protocol's shape (§13.2) but the core's id form: `places` are
  `PlaceId`s, since a filter alone knows no names, and the runner maps them
  as it maps every edge and event. A keyed edge's filter is written on its
  `EdgeInfo` and in `TopologyTransition::filters`, each only where there is
  one.

The protocol's own shapes, keyed by names with string token ids, live in
`petritype-runner`, not in the core: the core's types must not be frozen by
a wire format they do not own. `Snapshot` stays unserialisable; tokens are
`dyn Any` and have JSON only on `serializable` places.

### 19.3 The `petritype-runner` crate

**Designed and built 2026-10-03**; as-built notes in §19.3.10, nested nets in §19.3.11, keys in §19.3.12. A library crate in this
workspace plus an example binary. A net's bodies are the user's code, so the
binary is always the user's own: it lists its nets and hands over to the
crate. Dependencies: `petritype` with `serde`, `serde`, `serde_json`, and
`rustix` (Unix only, for the stdout guard). No async runtime: the protocol
is one line in, some lines out, and a body runs on a thread of its own.

The example binary (`examples/parcel_runner.rs`, its nets in
`parcel_runner/nets.rs`) carries the demo nets, the conformance net
`parcel-distribution` of PROTOCOL.md §11 and, since slice 17, the second
conformance net `transit`, which is not a copy: `nets.rs` includes the
core's own example net, `petritype/examples/transit/net.rs`, with
`#[path]`, and registers `build_city` as the entry's factory. Since slice
20 it carries the third, `wildlife`, the same way
(`petritype/examples/wildlife/net.rs`, `build_wildlife`).

#### 19.3.1 What the user writes

```rust
use petritype_runner::{Hosting, LoadError, NetEntry, ParamSpec, Params};

fn main() -> std::process::ExitCode {
    petritype_runner::main([
        NetEntry::new("parcel-distribution", parcel::build)
            .description("Sort and dispatch parcels")
            .param(ParamSpec::new("trucks", "u32").default(2)),
        NetEntry::new("feed-watcher", feeds::build).hosting(Hosting::Continuous),
    ])
}

// A factory takes the load parameters as JSON and returns a built net.
// `Params` is `serde_json::Map<String, serde_json::Value>`.
fn build(params: &Params) -> Result<petritype::Net, LoadError>;
```

- `main` takes any `IntoIterator<Item = NetEntry>`. Factories are stored
  boxed (`Box<dyn Fn(&Params) -> Result<Net, LoadError> + Send + Sync>`),
  so `NetEntry::new` and `NetEntry::typed` take `impl Fn` and a plain `fn`
  works.
- `LoadError::{InvalidParams(String), Build(BuildError), Other(String)}`
  maps to `invalid_params`, `load_failed`, `load_failed`. A factory that
  panics is `load_failed` with the panic message (`catch_unwind`, as bodies
  already get in slice 7).
- `NetEntry::typed::<P: DeserializeOwned>(name, fn(P) -> Result<Net, LoadError>)`
  decodes the parameters first and reports a bad one as `invalid_params`.
- **For phase 3 of nets as tokens** (2026-10-09): the core's `Templates`
  (§6.6, slice 16) stores factories of this shape, `Fn(&Params) ->
  Result<Net, E>` with the error boxed, and `petritype::Params` is the same
  `serde_json::Map<String, serde_json::Value>` as the runner's, so a
  `NetEntry` factory can be registered as a template as it is (`LoadError`
  being an error that converts into a boxed one) and a template's net loaded
  as an entry.
- **Registry: a plain function table**, passed to `main`. Chosen over a
  `register!` macro collecting entries at link time (`inventory`-style):
  no extra dependency, no linker behaviour to explain, and the list of nets
  is visible where the binary starts.
- `Hosting::{Manual, Continuous, Batch, Cron(&'static str)}` becomes
  `describe`'s `mode` (`"manual"`, `"24/7"`, `"batch"`, `"cron"`) and
  `schedule`, the same hints Python's `@petri_net` declares. `Manual` is the
  default.
- `main` handles `--describe` (print the `describe` answer as one line, exit
  0), no arguments (serve stdin), and anything else (usage on stderr, exit
  2). `runtime` is `"rust"`, `runtime_version` the `petritype` crate
  version.

#### 19.3.2 Three layers, and only the outer one does I/O

1. **`protocol`**: the wire shapes as serde types, keyed by names with
   string token ids, and the closed set of error codes. Nothing here knows
   about a `Net`. This is where PROTOCOL.md's JSON lives in Rust, and it is
   deliberately not in the core (§19.2).
2. **`Owner`**: the state machine. It owns the registry and `Option<Loaded>`
   and exposes one method, `handle(&mut self, Input) -> Vec<Outgoing>`:

   ```rust
   enum Input    { Request(Request), Completed(Completed), Tick(Instant) }
   enum Outgoing { Line(String), Launch(Firing), Exit }
   ```

   `Line` is a notification or a response, already serialised, in the order
   it must be written. `Launch` is a detachable firing the caller must run
   on some thread and feed back as `Input::Completed`. `Owner` creates no
   threads, reads no clock it is not handed, and writes nothing, which is
   what makes every behaviour below testable by feeding inputs and reading
   lines, and what the transcript tests (§19.3.8) drive.
3. **`main`**: the stdout guard, a reader thread that parses stdin into
   `Input::Request` (a malformed line becomes an `invalid_params` error
   response carrying the line's `id` if one could be read, else `""`); a
   writer thread that owns the duplicated stdout and takes whole batches of
   lines, so nothing ever splits a request's notifications from its
   response; a heartbeat thread (§19.3.5); one `mpsc` channel that the
   reader, the body threads and a ticker feed; and the loop: `recv`,
   `handle`, hand the batch to the writer, spawn a thread for each `Launch`
   whose body sends `Completed` back, stop on `Exit` or when the reader
   reports end of file.

#### 19.3.3 What `Loaded` holds besides the net

The core dropped these on purpose (§2); the runner keeps them:

| field | why |
|---|---|
| `topology: Topology` | the name tables, taken once per generation (§19.2); every id in an event is resolved through it |
| `generation`, `marking_version`, `running` | §4 of the protocol; `marking_version` continues across `load` and `reset` and never decreases, which is our reading of a spec that says both "sets a new value" and "does not move it" (flagged upstream) |
| `in_flight: Option<{transition, started_at, step_id: Option<String>}>` | the one firing airborne, for `heartbeat`, `duration_ms`, and the deferred `step` response |
| `deferred: VecDeque<Request>` | requests that must wait for the firing to land (§19.3.5) |
| `last_fireable: Vec<String>` | the last `fireable` notification sent, compared after every batch |
| `log: VecDeque<LogEntry>` (1000) | `get_log`; each entry keeps, off the wire, the chain of nested ids it happened in (slice 17) |
| `inner: HashMap<NetId, Inner>` | slice 17: per inner net, its own `marking_version`, `Topology` and `Names`, progress, last fireable set, template and last path (§19.3.11) |
| `touched: Vec<NetId>` | slice 17: inner nets with events since their fireable set was last compared |
| `projection: Option<Projection>` | slice 20: the connection's filter for the outer net and what it has been shown (§19.3.12); an inner net's is in its `Inner` |
| `progress: {step_count, last_fired, fired_counts, last_progress_at}` | `progress`; the outer net's also counts the inner firings the runner made (slice 20) |
| `idle: bool` | one `idle` per idle spell |
| `factory`, `params` | `reset` rebuilds from them |

#### 19.3.4 Translating events

After every core call that can log events, the owner drains the log and
writes one notification per event, in order, each stamped with the
`marking_version` it raised to, and then compares `net.fireable()` to the
last set sent. Only then does it write the response, which is the protocol's
"notifications before the response" rule.

| `Event` | notification |
|---|---|
| `Fired` | `fired` (inline bodies only, §19.3.5) |
| `Launched` | `launched` |
| `Distributed` | `distributed` with `duration_ms` from `in_flight.started_at` |
| `Failed` | `failed`; the log gets a `"failed"` entry |
| `Added` / `Removed` / `Mutated` | `added` / `removed` / `mutated`, or under the connection's filter `hidden`, or an `added` or a `removed` for a mutation across it (§19.3.12) |
| `Toggled` | `toggled` |
| `ParamChanged` | `param_changed {param}`, the name taken from the `set_param` request being served, because the owner is the only writer and the event carries only a type name; since slice 17, otherwise `Net::param_name` (an outer body setting an inner net's parameter) |
| `Resynced` | `reloaded {reason: "resync"}` (slice 16's `import_marking`), which moves no version |
| `Nested` | `nested {path, nested_id, event}` around the inner net's own notification (slice 17, §19.3.11) |

A token on the wire is `{id, label}`: the id is `TokenId`'s `Display`, the
label `debug()` cut at 1000 characters on a character boundary with `"..."`
(§19.1). Rust places are monomorphic, so `type_name` is never sent, and
`color` is not either. What a token has instead, since slice 18, is keys
(§8.3): named values its place derives from it, which the protocol's §13
puts on a token entry as `keys: {name: value}` (from `Net::keys`) and on a
place as `key_names` (from `TopologyPlace::key_names`), and the runner
writes them since slice 20 (§19.3.12). A transition's `detail` is
`{params, detachable}`; `priority` is `priority_of` behind `catch_unwind`
and omitted if the closure panicked.

#### 19.3.5 The firing cycle

```
step / start tick:   select() -> launch(t)
   detachable:       Launched written; Outgoing::Launch(firing); in_flight set
   inline (MutateEdge): launch already ran the body; distribute at once; Fired written
Input::Completed:    distribute(done) -> Distributed or Failed written;
                     log entry; progress; fireable compared;
                     the pending step answered; deferred requests served in order;
                     if running: select the next
```

- `step` answers `{fired: true, transition}` after the firing has landed,
  `{fired: false}` when `select()` returns nothing. It is the one request
  answered out of order.
- **While a firing is airborne**, `get_state`, `get_place_tokens`,
  `get_token`, `get_log` and `stop` are answered at once from the live net:
  a detached body holds no borrow (§6.2), so no snapshot is needed. Every
  other request is queued in `deferred` and served, in arrival order, after
  the landing. A second `step` therefore waits for the first.
- **An inline body blocks the owner.** A transition with a `MutateEdge`
  runs its body inside `launch` (§6.2), so for that long nothing is
  answered, `get_state` included. This is a documented deviation from the
  protocol's "never waits for a running body", confined to mutate-edge
  transitions, whose bodies are expected to be short; the alternative, a
  snapshot before every inline firing, is O(state) per firing and refused
  for `no_clone` places. The whole net is unavailable for that time, not
  just the mutated place: the body holds `&mut` into a token inside the
  net, and the net is one value under one borrow (§6.2, §9). The heartbeat
  still goes out, see below.
- `start` sets `running`, answers, and the owner launches whenever it is
  free: after each landing and on each `Tick`. Quiescence (`select()` is
  `None` and nothing is airborne): a `Continuous` net emits `idle` once and
  keeps checking on every tick and after every request; any other net
  clears `running` and emits `execution_stopped {reason:
  "no_fireable_transitions"}`.
- A `Failed` landing while `running` clears it and emits
  `execution_stopped {reason: "transition_failed", error_type, message}`,
  as the Python runner does. One firing at a time, so a run never has two
  bodies airborne.
- `stop` clears `running` and answers at once; the airborne body lands
  normally and is reported normally.
- `heartbeat` (default every 30 s) is written by a thread of its own, not
  by `Owner`, so it keeps going while an inline body has the owner blocked,
  which is exactly when a host most wants it. The owner records the
  airborne or inline transition and its start time in a shared status cell
  before any launch and clears it on landing; the heartbeat thread reads
  the cell and sends `{step_started_at, executing_transition}` or two
  nulls. `Tick` is the ticker's input to `Owner` (same period) and only
  drives the run loop: a `Continuous` net re-checks `select()` on it.

#### 19.3.6 Load, reset, shutdown

- `load {net, params}`: unknown name is `unknown_net`; the factory runs on
  the owner thread under `catch_unwind`; on success the previous net, if
  any, is dropped (a run in progress is stopped first, with no
  `execution_stopped`: the response says `running: false`), `generation`
  rises, `topology` is retaken, `reloaded {reason: "load"}` precedes the
  response. `load_progress` is emitted once, before the factory runs.
- `reset`: the same factory and parameters; `reloaded {reason: "reset"}`.
  Both wait in `deferred` if a firing is airborne, like any mutating request.
- `inject {place, token}`: `apply_json(JsonCommand::Extend)` with one
  token; the id is read off the `Added` event. A place without `from_json`
  is `not_serializable`; a value that does not decode is `invalid_token`.
- `set_param {param, value}`: `JsonCommand::SetParam`; `transition` is
  ignored; a name the net did not register is `unsupported` (since slice 17
  `invalid_params`, §19.3.11).
- `enable` / `disable`: `net.enable` / `net.disable`; `toggled` only if the
  switch moved.
- `shutdown`: answer `{status: "ok"}`, then `Exit`. An airborne body is
  abandoned with the process; the host asked for that.
- End of file on stdin is `Exit` without a response.

#### 19.3.7 Error mapping

`CommandError` and `NetError` variants map onto the protocol's closed set:
unknown place or transition names and ids to `unknown_place`,
`unknown_transition`, `unknown_token`; a place without a JSON form to
`not_serializable`; a decode failure to `invalid_token`, a nested token
whose saved net the core refuses included (§12.5); a parameter name the net
does not know to `invalid_params` (`unsupported` until slice 17) and a
parameter type with no wire name to `unsupported`; an address that names no
live inner net to `unknown_net`; a request before `load` to `not_loaded`;
malformed params to `invalid_params`; anything else to `internal` with the
error's `Display` as the message. The mapping is one function with a test
per arm.

#### 19.3.8 Tests

- **Unit**: `Owner` driven directly. Each method has a test feeding a
  request and asserting the exact lines out, including the order of
  notifications before responses, the deferred `step`, the out-of-order
  reads, quiescence and `idle`, failure stopping a run, generation and
  version arithmetic, and the error mapping.
- **Transcripts**: `tests/protocol/*.json`, the format this crate defines
  and the Python repository will adopt (§11 of the protocol): `{net, params,
  exchanges: [{send: request, expect: [line, ...]}]}` where an expected line
  may use the masks `"<label>"`, `"<time>"`, `"<ms>"` and `"<id>"` in any
  string position. A harness runs the example binary's nets through `Owner`
  (no process), feeding each `Launch` back synchronously, and compares. The
  Rust-only shape `launched` + `distributed` is normalised to `fired` by the
  harness before comparing, so one transcript serves both runtimes; that
  rule goes upstream with the format. **2026-10-04:** no longer Rust-only.
  The first slow Python body watched from Bevy showed nothing until it
  landed, while the Python owner already knew the transition was in flight
  and had taken its inputs. Decided: the Python runner reports two pieces
  too, always; `fired` stays legal in version 1 for a runtime that cannot
  observe a body part-way; mid-body reads show the taken inputs gone. The
  fold stays, now for such a runtime.
- **Process**: one test spawns the example binary, sends `describe`, `load`,
  `step`, `shutdown` over real pipes, and checks that a `println!` in a body
  arrived on stderr, not stdout.

#### 19.3.9 Not in this slice

HTTP and server-sent events (the host relays); protocol version 0; token
colours and position hints (nothing in the core produces them); `get_state
{token_limit}` beyond the default 200 and `get_place_tokens` paging are in,
but no complete-snapshot cache, because reads are answered from the live net.

#### 19.3.10 As built, 2026-10-03

Where the implementation settled something the design left open, or chose
differently, with the reason:

- **Inline firings** still log `Launched` then `Distributed` in the core;
  the runner folds the pair into one `fired`, `duration_ms: 0`, since the
  owner has one `now` for the whole `handle`.
- **`fireable` is not compared after a detached launch**, only after the
  landing, so no `fireable` line falls between `launched` and
  `distributed`. At `load` and `reset` the set is recorded silently; the
  `reloaded` already means "refetch".
- **Inline runs stay interruptible.** A running net whose next firing is
  inline fires one per `handle` and `Owner::wants_tick()` asks the loop to
  queue a `Tick`, so a `stop` between firings is heard.
- **Answered during a body**: the five reads and `stop`, plus `describe`
  and `shutdown`.
- **Heartbeat** carries `marking_version` and `running` like every
  notification, read from atomics the owner mirrors; sent only while a net
  is loaded. Period from `PETRITYPE_RUNNER_HEARTBEAT_MS` when set, for
  tests.
- **Load parameters**: `ParamSpec` defaults are filled in before the
  factory runs; a missing required parameter is `invalid_params`.
- **`set_param`**: a value that does not decode is `invalid_params`; a
  parameter the net knows but did not register for the wire is
  `unsupported`.
- **`describe`** omits `schedule` and `description` when absent; the
  protocol's example was corrected to match the same day.
  `runtime_version` is the runner crate's own version; the core exports no
  version constant.
- **A failed `step`** answers `{fired: true, transition, failed: true}`
  after its `failed`: a transition was chosen and its firing landed.
  `progress` counts successful firings only. Now specified in PROTOCOL.md
  §6.3, amended the same day.
- **Future `Event` variants** the runner does not know become `reloaded
  {reason: "resync"}`. `load_progress` is sent for `load` only.
- **Token ids start at 0** in Rust and at 1 in Python, and after the
  `launched`+`distributed` fold a Rust `fired` has still moved the version
  by two. PROTOCOL.md §2 and §4 now say both are the runtime's business and
  §11 has shared transcripts mask `<id>` and `<version>`. The wire label cap
  of 1000 characters includes the `...` marker (§5.3), which the runner
  honours.
- **Four transcripts** in `petritype-runner/tests/protocol/`: describe and
  load and state; steps with inject and `get_token`; toggles, a failing
  body, `set_param` and reset; a continuous net going idle and waking.
- **Shared transcripts** (2026-10-03): the harness gained `<any>`,
  `{"<unordered>": [...]}` (with backtracking) and `<id:NAME>`, reads the
  shared set from `../petritype/tests/protocol/` or
  `PETRITYPE_PROTOCOL_TRANSCRIPTS` and skips with a message when neither is
  there, and replays it against the conformance net alone, the §11 rule
  settled when the first meeting of the two runtimes showed `describe` as
  the only difference. Everything else matched first time: `fireable`
  timing, `load_progress`, progress, log entries, `execution_stopped`,
  slots, `reloaded`, `get_token`.
- The crate is `#![forbid(unsafe_code)]`; the fd swap is `rustix`'s
  `fcntl_dupfd_cloexec` and `dup2_stdout`. The non-Unix path compiles and
  is untested.

#### 19.3.11 Nested nets on the wire, 2026-10-09

Phase 3 of nets as tokens, slice 17: the proposal's R8, on PROTOCOL.md §12,
which the Python track wrote first and this runner conforms to. The shared
transcript `05_nested_nets.json`, over the conformance net `transit`,
replays here unchanged, both from the Python repository and from a verbatim
copy in `tests/protocol/`.

- **Addressing** (§12.1). Every read and `inject` takes `path` or
  `nested_id`; both is `invalid_params`, neither the outer net. A path is
  walked level by level through `Net::nested`: the first segment must be
  the entry's name, then per level the place, `#` and the token id, and the
  net's name in the token, so that an unnamed inner net (a bare `Net`
  token) ends its level in `/`; the entry's name alone is the outer net. A
  nested id is the inner net's `NetId` as its `Display` prints it, found by
  walking every nested place, passive ones included, at every depth. No
  match is `unknown_net`, and the message names the segment that failed
  (the place, the token, the net, a level cut short) or the id. `step`,
  `start`, `stop`, `reset`, `enable`, `disable` and `set_param` act on the
  outer net alone, and a `path` or `nested_id` on one is `invalid_params`
  (`step` too, as in the Python runner). An `inject` into an inner net goes
  through `nested_scope` at every level, so its `added` is lifted level by
  level as each scope closes.
- **A record per inner net** (`Loaded::inner`, by `NetId`; §12.3): its own
  `marking_version` (0 when first seen, one more per state notification of
  that net), its `Topology` and `Names` (taken once: an inner net's
  topology never changes), its progress, its last fireable set, its
  template and the path it was last seen at. Made at `load` and `reset` by
  walking the marking, or when a request names the net (in both cases the
  fireable set is taken silently, as `load` takes the outer one), or when
  one of its events arrives first (its fireable set empty, so a non-empty
  one is announced). Dropped eagerly, at the first landing or inner firing
  after which its id no longer resolves: one walk of the nested places per
  landing, never while a body holds a token, so a vehicle in transit keeps
  its record and its version. `load` and `reset` drop them all.
- **Versions are per net.** A `nested` line carries the outer
  `marking_version`, unchanged; the inner net's is inside its `event`.
  `nets[].count` is a hint read with the outer state.
- **The `nested` wrapper** (§12.4). `flush_events` unwraps an
  `Event::Nested` to the event the innermost net logged and writes one
  `nested {path, nested_id, event}` whatever the depth: the inner
  notification built as the outer net's would be, with that net's names,
  version and labels. The path is the places and tokens the event was
  lifted under, place names taken from the records of the nets above.
  Labels come from the inner net when it can be reached. Under rule 3 of
  §6.6 (lifted at launch, the token in the body's hands) it cannot, and an
  inner `added`'s label is `""` (a `mutated`'s is left out). An inner
  `ParamChanged` is named by `Net::param_name` on the inner net, else by
  its type name. `Resynced` inside is `nested` around `reloaded
  {generation: 1, reason: "resync"}`; outside it is `reloaded {reason:
  "resync"}`, mapped explicitly now, as an unknown variant still is.
- **The wire tells a firing's inner effects before the firing's landing,
  as it tells its `mutated`s.** The core lifts a produced token's inner
  events after the firing's `Distributed` or `Fired`, under the token's new
  place and id (rule 4), and a mutated token's just before its `Mutated`
  (rule 2). `wire_order` reorders one batch: of a landing, every `Nested`
  (those for mutated tokens, then those for produced ones, each in the
  order logged), then the `Mutated`s, then the event that ends the firing.
  So `Depart` reads `launched`, `nested` `param_changed`, `distributed`,
  and `Board` `nested` `added`, `mutated`, `fired`. The path on those lines
  is the token's place once the firing has placed it (`In Transit` for a
  `Depart`), which is what the core stamps. The core's own log keeps its
  rules; only the wire is reordered.
- **Inner firings are inline.** `step` asks the outer selector
  (`select()`) and, when it picks nothing, makes the first inner firing in
  `step_deep`'s order with `Net::step_inside()` (§6.6), inline: the inner
  net logs `Fired`, and the wire says `nested` around `fired`, one of the
  two shapes §12.4 allows; a harness folds the other into it. The `step`
  result is `{fired: true, transition, path, nested_id, marking_version:
  <outer>, running}`, with `failed: true` after a `nested` `failed`;
  `fired: false` means nothing fireable anywhere outside passive places. A
  running net does the same, one inner firing per `handle` with
  `wants_tick` asking for the next, so `execution_stopped {reason:
  "no_fireable_transitions"}` and `idle` mean deep quiescence, and an inner
  failure stops a run as an outer one does. An inner net's `progress`
  counts its own firings (its `step_count` is its `fired_count`). The outer
  net's counted its own only until slice 20; it now counts each inner
  firing the runner makes too, as the Python runner does: `step_count` one
  more, `fired_counts` and `last_fired` under the path without the net's
  name and then the transition (`Depot/#17/cabin/Sit`, the path where the
  inner net fired), and `last_progress_at`. A failed inner firing counts in
  neither; an inner firing an outer body makes counts in its own net's
  alone. No shared transcript pins it; the Python track asked for parity.
- **Inner `fireable`** (§7.2, §12.4). After a batch, after the outer
  `fireable`, each inner net that had events (in that batch, or since the
  last compare, across a launch) is compared with the last set sent for it,
  and `nested` around `fireable` goes out when it differs. It moves no
  version. As `05_nested_nets.json` has it: the cabin's `["Sit"]` after the
  first board, nothing at load.
- **Reads** (§12.2, §12.3). `topology.places[]` carry `nested` and
  `passive` when true. Every token entry (`get_state`, `get_place_tokens`,
  `get_token`) carries `nets: [{name, nested_id, path, template?, count}]`
  when its token holds nets; `produced`, `added` and `mutated` carry none.
  An inner `get_state` is that net's state: `net` its name in its token,
  `generation` 1, its own version, topology, transitions, marking and
  progress, plus `path`, `nested_id` and `template`; `get_place_tokens` and
  `get_token` echo `path` and `nested_id`. `inject` into an inner net
  answers with that net's version and no address, as the shared transcript
  has it. While a body is airborne, a read whose address does not resolve
  waits for the landing (the net may be inside the token the body took) and
  is answered then, or `unknown_net`; every other inner net is read at
  once. Inner bodies run inline on the owner thread, so nothing is read
  during one.
- **The log** (§8) is one ring. An inner firing's entry, written when its
  `Fired`, `Distributed` or `Failed` is flushed (`started_at` the handle's
  `now`, `duration_ms` 0, since it ran inline), carries `path` and
  `nested_id` and the inner net's place names and ids; `seq` is shared.
  Each entry keeps, off the wire, the chain of nested ids from the outer
  net down, and `get_log` with an address keeps the entries whose chain
  holds that net: its own and those of the nets inside it.
- **Errors.** `set_param` naming a parameter the net does not have is
  `invalid_params`, as `05_nested_nets.json` pins (it was `unsupported`):
  the runtime does set parameters, the request named one that is not there.
  `unsupported` stays for a parameter type with no wire name. A nested
  token the core refuses on `inject` (an unknown template, a parameter the
  template does not declare, an unknown inner place or transition, an inner
  token that does not decode) comes back from the core as `InvalidToken`,
  so `invalid_token` (§12.5) with no new arm.
- **What the core gained**: `Net::step_inside` (§6.6); `Net::param_name`
  (§8.2); `TransitionInfo::positional` and `TopologyTransition::positional`
  (§4.5), so that `slot` is written for positional outputs alone —
  `transit`'s `Arrive` distributes to one of four stations, and the shared
  transcript has no slots on its edges.
- **The harness** (`tests/transcripts.rs`) replays a transcript over a
  conformance net against that net alone, wherever the file is. It gained
  the `<path>` mask, captures substituted inside a longer string when
  sending (`"transit/Depot/#<id:bus>/cabin"`), and two folds: a `nested`
  `launched` followed by its `nested` `distributed` into a `nested`
  `fired`, and a `launched` … `distributed` pair with only the firing's own
  `nested` and `mutated` lines between into those lines and a `fired`. The
  folds apply to the expected lines as well as the actual ones; the second
  is what this runner's inline firings need, which it reports as one
  `fired` after their `mutated`s where the Python runner reports `launched`
  first. Our transcript 01's `describe` lists `transit` too, since the
  example binary carries it, and 03 has `invalid_params` for the unknown
  parameter.
- **Deferred.** Inner firings on threads of their own; an id-preserving
  import. (Keys, queries and filters, deferred here, landed as slice 20,
  §19.3.12.)

#### 19.3.12 Keys, queries and the projection on the wire, 2026-10-09

Phase 3 of token keys, slice 20: the proposal's R3, on PROTOCOL.md §13,
which the Python track wrote first and this runner conforms to. The shared
transcript `06_keys_query_filter.json`, over the conformance net `wildlife`
(the core's example, included by path as `transit` is), replays here
unchanged, both from the Python repository and from a verbatim copy in
`tests/protocol/`; 01 to 05 still do.

- **Keys on the wire** (§13.1). `topology.places[].key_names` comes from
  `TopologyPlace::key_names`, written only when there are some. A token
  entry — in `get_state`, `get_place_tokens` and `query` — carries `keys`
  (`protocol::Keys`, an object in declaration order) from `Net::keys` when
  its place declares keys, and so do `added` and `mutated`, read when the
  line is written, as labels are, so a `mutated` has the keys after the
  change. `get_token`'s answer is not a token entry and has none, as the
  transcript has it. A `produced` entry is `protocol::Produced`: three
  elements, or five when its place declares keys, `null` in the colour's
  place, since a Rust token has no colour (and so never four).
- **Keyed edges** (§13.5) go out with `kind: "take_where"` or
  `"mutate_where"` and their `filter` as declared, places mapped from ids to
  names, a `{key, param}` clause unevaluated.
- **A filter off the wire** (§13.2) is read by `protocol::Filter::parse`,
  as strictly as Python's `Filters.from_wire`: no field but `clauses`,
  `places` and `prefix`; a clause a string `key` and either a non-empty
  `values` of strings, integers and booleans or a string `param`, never
  both. `translate::filter` then resolves the place names against the net
  the request names. A bad shape is `invalid_params`, an unknown place
  `unknown_place` (`CoreError::Filter`, a test per arm). `{key, param}` is
  `Clause::IsParam`, which reads the parameter by its wire name.
- **Served from the index.** Every question of many tokens asks
  `Net::query_ids`, the core's one addition: `query`'s tokens as ids alone,
  from the key index, so a reader renders — label, keys, nets, JSON — only
  the tokens it writes. `query` counts all its matches and renders the first
  `token_limit` (200 by default, 1 to 1000); `get_state` under a filter
  groups the kept ids by place, for `matching` and the first `token_limit`
  of each; `get_place_tokens` pages its place's (asked of that place
  alone). `distinct` is `Net::distinct_values`, the index alone. Not from
  the index: one token's question as its event is read, which
  `Net::matches` answers by extracting that token's keys, and `get_token`'s.
- **`query` and `distinct` are reads.** Answered at once while a body is
  airborne, from the live net (a taken token is absent, as from
  `get_state`), and held for the landing only when they name an inner net a
  body may hold, like the other reads. `query` without `filter` uses the
  connection's filter for that net, or else keeps every token; with one,
  that one alone. `distinct` is over the net's tokens, not the projection:
  §13.3 says so, and Python does the same.
- **One projection per net.** `set_filter {filter?, path?, nested_id?}`
  gives the net it names a `Projection` (`projection.rs`): the core's
  `Filter`, places by id, and the set of `(place, token)` the connection has
  been shown. The outer net's is in `Loaded`, an inner net's in its `Inner`
  record, so it goes with the record, and `load` and `reset` clear the
  outer one (in Python too: a new generation is a new net record). An inner
  net's events go through its own projection, never the outer net's.
  `set_filter` answers with the named net's version and announces nothing:
  the client changed its view and reads it afresh. It is not a read, so it
  waits for an airborne body to land: a firing is told through one filter
  from its `launched` to its landing.
- **The `hidden` rule.** As the owner flushes the log, each token event
  goes through the net's projection (`token_line`, shared by the outer flush
  and the inner lift). An `added` is told when the filter keeps the token
  where it stands; a `removed` when the connection had been shown the
  token; a `mutated` by where it crosses — kept before and after,
  `mutated`; after only, `added`; before only, `removed`; neither,
  `hidden`. In place of every line it hides the connection gets `hidden
  {kind, place}`, a state notification, so the version moves exactly as it
  would unfiltered and a filtered client never sees a gap. A firing's lists
  name only what the connection is told: `launched` (and an inline
  `fired`) the consumed tokens it had been shown, which the projection keeps
  aside as taken; `failed` exactly those, shown again when `restored`;
  `distributed` and `fired` the produced tokens the filter keeps. The line
  goes out with empty lists too, since it moves the version. The set of
  what was shown is the one thing a filter cannot answer by itself: a
  consumed or removed token is gone by the time its event is read.
- **Into or out of view without a token event.** After a batch's events and
  before its `fireable`s: when a parameter of a net changed and that net's
  filter has a `{key, param}` clause, every token the filter keeps now
  (from the index) is compared with what was shown, and the difference is
  told as `removed`s and then `added`s, each in id order, the Python
  runner's `_sync_visible`; after inner events, only the tokens they were
  lifted through are compared, since a key may read the nets a token holds
  (§8.3). Python compares after every batch; here only where something can
  have moved without an event, so a filtered firing costs the tokens it
  names. These lines are state notifications of their net and move its
  version, as in Python: the one place a filtered connection's version runs
  ahead of the version an unfiltered one would have read.
- **The outer progress** counts the runner's inner firings (§19.3.11), for
  parity with the Python runner.
- **`set_param` keeps the parameter's JSON type**, as it always has: the
  value goes through the parameter's strict decoder (§6.4), so a string for
  a number, a float for an integer or a number for a string is
  `invalid_params` and moves nothing. Now tested as such.
- **What the harness needed.** The `wildlife` registry, and one fold
  widened: a `launched` and its `distributed` with only the firing's own
  lines between become those lines and a `fired`, and the firing's own
  lines now include `hidden`, `added` and `removed`, the forms its mutations
  take under a filter. `06_keys_query_filter.json` has Python's `Spread
  Savanna Reef` as `launched`, `hidden {kind: "mutated"}`, `distributed`;
  this runner's inline firing tells `hidden`, `fired`.
- **Read in PROTOCOL.md, where it is silent or loose** (for its next
  revision): `failed.consumed` is projected like `launched.consumed` here,
  so a hidden token's id is never told, where §13.4 names only `launched`,
  `distributed` and `fired` and the Python runner lists every consumed
  token; the `added`s and `removed`s a parameter change brings move the
  version, which §13.4's "the versions a filtered connection sees are the
  net's" does not quite say; a clause's empty `values` is `invalid_params`,
  as in Python, where §13.2 says nothing; `query {filter: null}` reads as no
  filter here (Python answers `invalid_params`); and a `mutated` inside an
  inner net whose token a body holds cannot be checked against the filter,
  so it keeps the token's view as it was.

### 19.4 Who spawns it

- **petritype-server**: the user's repository is a cargo package whose
  binary calls `petritype_runner::main`; the build stage compiles it to a
  static Linux binary, runs `--describe`, and stores both; the worker
  downloads the binary and spawns it like `python -m petritype.runner`.
- **petritype-bevy**: spawns the binary (or `python -m petritype.runner`)
  itself and mirrors it as a remote source, beside today's in-process
  `NetRes` (`../petritype-bevy/DESIGN.md`, building on §15).

### 19.5 Out of scope

Moving a net between languages; composing across languages (a transition
body in one language running a net in the other) beyond the fact that such
a body could speak the same protocol; WASM.
