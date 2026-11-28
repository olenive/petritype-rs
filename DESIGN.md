# petritype-rs: Design

Typed, executable, observable Petri nets in Rust. A port of the Python
[petritype](https://github.com/olenive/petritype) library, redesigned around ownership instead of
runtime reflection.

Status: design settled 2026-09-19 and built since, with 0.1.0 the first release and 0.2.0 the first to speak version 2 of the net protocol. This document is the reference
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
| `DrainEdge` / `SurveyEdge`, which require none (2026-10-14) | same (slice 22, §4.4) |
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
| `Optional[...]` place types to make `None` routable | a return position of type `Option<T>`; place type stays `T` |
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
  edge; anything, nothing included, for a drain or a survey edge) and whose
  guard passes.
- **Firing**: consume inputs, run body, distribute outputs.
- **Quiescent**: nothing fireable and nothing in flight.
- **Inner net**: a net carried by a token, in a *nested* place; the net
  holding the token is the **outer net** (§6.6, slice 15). *Quiescent deep*:
  quiescent here and in every inner net that is not in a passive place.
- **Subnet place**: a place that holds one net of its own, its **subnet**,
  for the outer net's whole life, and no tokens; a **port place**, or
  *port*, is a place of the subnet that its definition marks, which the net
  around it reaches by the pair of the subnet place and the port's name
  (§6.7, since 2026-11-21). A subnet is an inner net too.
- **Stepping rule**: how the nets in one place that holds nets, a nested
  place or a subnet place, take their turns beside the outer net, one of
  inner first (the default), take turns, outer first or passive (§6.6, since
  2026-11-21).
- **Group**: a named set of places and transitions of one net, stated in
  its definition, that only drawings read; a node is in one group or in
  none, and a net runs the same with or without them (§6.8, since
  2026-11-21). A group is a label on parts of one net, where a subnet place
  holds a separate net.
- **Key**: a named attribute of a token (a string, an integer or a
  boolean) that its place derives from the token's value with an extractor
  the author declared (§8.3, slice 18). A **filter** keeps the tokens whose
  keys pass its clauses; a **query** asks a net for them; a **keyed edge**
  (`TakeWhereEdge`, `MutateWhereEdge`, slice 19) takes or mutates the oldest
  token its filter accepts instead of the front one.
- **Presence**: what an input edge asks of its place before its transition
  may fire. Six kinds require it; a **drain** edge (`DrainEdge`) and a
  **survey** edge (`SurveyEdge`), slice 22, do not: they take or read every
  token there is, zero or more (§4.4).
- **Template**: a function registered by name that builds a net; a net
  built through one is **stamped** with it, and can then be saved, restored
  and **forked** (copied by building it again), §6.6, slice 16.

In the literature's terms this is a *coloured Petri net*: a token's value is
its colour and a place's type its colour set, with a body where the textbook
has arc expressions. A place here is a FIFO queue where the textbook has a
multiset. An inner net is the literature's
*nets within nets* (object nets). A subnet place is nearest to the
*substitution place* of the hierarchical coloured nets, joined to the net
around it through port places as there, with the difference that the inside
is a net of its own with its own stepping (§6.7). This document uses
"colour" only for a token's display colour (`style`).

## 4. Representation

### 4.1 Identifiers and counters

```rust
pub struct PlaceId(u32);        // erased; used in events and the read API
pub struct TransitionId(u32);
pub struct TokenId(u64);        // monotonic per Net

pub struct NetId(u64);          // process-unique, minted per NetBuilder; also names an inner net (§6.6); u32 until 0.4.0

pub struct PlaceRef<T> { place: PlaceId, net: NetId, _t: PhantomData<fn() -> T> }  // typed handle, 16 bytes
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
  - `state_counter`: everything else (token added, removed, mutated,
    transition toggled). A change tick for observers, not a firing count:
    a freshly built net is at 0 however it was seeded.
  - `fired_count`: number of completed firings. A statistic, not a counter of
    changes.
  - `emit` bumps `state_counter` once per event: a firing bumps it by one
    (`Fired`) plus one per mutating edge, `MutateEdge` or `MutateWhereEdge`
    (`Mutated`); a failed firing likewise
    (`Mutated`s then `Failed`); a toggle by one.
- `PlaceRef` is stamped with its net's `NetId` (an `AtomicU64` counter, one
  per process, an `AtomicU32` until 0.4.0, so that a process that builds
  nets for years cannot wrap it; not stable across runs). Every method that
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
  id, not something to store and read back. A JSON reader that keeps
  numbers as doubles, JavaScript's, reads it exactly below 2^53; the runner
  spells it as a decimal string (`nested_id`), exact at any size, and reads
  one back as a `u64` in that spelling alone.

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
    stepping: Stepping,             // how its inner nets take their turns (§6.6); read only where nested
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
a `SurveyEdge` (slice 22), or a consuming input to a
`restore_tokens_on_failure` transition (build errors; a `MutateEdge` from it
is fine, and so is a `DrainEdge` on a transition that does not restore), and
makes `Net::snapshot()` return `Err`.

`place_nested::<T>()` (slice 15, §6.6) is the second: `T: Nested + Debug +
Send + Sync + 'static`, no `Clone`, because a `Net` cannot be cloned. It is a
no-clone place with all of the above, except, since slice 16 and with the
`serde` feature, `Net::snapshot()`: a snapshot copies each of its tokens with
`Nested::fork`, which rebuilds the token's inner nets from their templates
(§6.6). The snapshot edge, the survey edge and the restoring consuming
edge stay build errors: all three clone on the launch path, where nothing may
fail, and a fork can.

Big data idiom: put heavy fields in `Arc`. Moves are free (a `Vec` is 24
bytes to move); `Arc::clone` is a counter bump; `Arc::make_mut` is
copy-on-write. Shared *mutation* requires `Arc<Mutex<_>>`, which is visible
in the place's type.

### 4.4 Transitions and edges

```rust
struct TransitionSlot {
    name: String,
    inputs: Vec<(PlaceId, EdgeKind)>,      // EdgeKind::{Take, TakeAll, Snapshot, Mutate, TakeWhere, MutateWhere, Drain, Survey}
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
| `DrainEdge(p)` | every token, zero or more | `Vec<T>`, possibly empty | no |
| `SurveyEdge(p)` | no | `Vec<T>` (clones of every token, zero or more) | no |

Fireability is "every input place non-empty" (for a keyed edge, "holding
a token the filter accepts"; for a drain or a survey edge, nothing at all),
with one widening since 0.4.0: take and take-where edges may share a place,
below, and then the place must hold a token for each. Every other kind is
alone on its place among its transition's edges, so the borrows of the
mutating and the read edges are disjoint by construction. The two keyed
edges (slice 19) are §8.3's; the two that require no presence (slice 22)
are below.

**Edges that share a place, 0.4.0** (the owner's decision D6; Python
petritype has allowed it all along, and is the reference). Two or more
input edges of one transition on one place are allowed when each takes a
token of its own: `TakeEdge` and `TakeWhereEdge`, in any mix. Two oxygen
radicals taken from one place by two take edges make one molecule, and
with one radical the transition waits; from a ballroom one keyed edge takes
a lady and another a gentleman, and the transition is fireable only while
the ballroom holds one of each. There is no count on a single edge: an
edge is what gives each token its argument.

- **Which tokens.** The edges on a place choose in declaration order, each
  the first token in queue order that no edge before it chose: a take the
  front one left, a take-where the first left that its filter accepts.
  This is Python's `plan_stage_1` (`_first_unconsumed_index`,
  `_first_kept_unconsumed_index`). It is greedy as Python's is: when the
  first of two filters takes the only token the second also accepts, the
  transition is not fireable, though the other way round it would have
  been, and a take declared before a take-where takes the front token
  whatever the filter wanted. No search for another assignment: the rule
  stays one pass and one answer, the same in both runtimes, and an author
  who needs the other assignment declares the narrower filter first.
- **Fireability and stage one** ask one function (`sharing::share`), which
  asks the key index for a take-where's first match and reads on through
  the matches only when an earlier edge chose that one. Stage one's three
  passes stand: `plan` chooses every token before anything moves, the
  positions as the marking stands, so a key extractor or a `Clone` that
  panics leaves every token where it was; `take` counts past the tokens
  taken ahead of a position from the same place. Each edge's token is in
  `consumed` in edge order, each edge's `reaches` holds its own token's
  nets, and the index forgets each token as it leaves.
- **Restore.** A failed firing that restores puts the tokens back at the
  front of the place they came from, a port's included, with their ids,
  in edge order, the first edge's at the front, whatever order they stood
  in, as Python's `restore_argument_tokens_to_places` does. A place left
  out of id order is marked unordered, as before.
- **Refused at build.** Every other kind beside another edge on its place
  (`EdgeCannotSharePlace { transition, place, kind }`, `kind` the first
  edge there that cannot share): a mutate or a read edge lends a token
  another edge may take, a snapshot or a survey copies it, a take-all or a
  drain takes every token. Python allows a take beside a snapshot, a drain
  or a list-mode take; the Rust core refuses those, as the owner decided.
  A bound edge or a source (§8.3) on a shared place is
  `BoundPlaceShared { transition, place }`, as in Python: a source is one
  edge, and a second edge could take the token a pair was found with.
- **Outputs.** Two output edges of one transition to one place are
  `DuplicateOutputPlace { transition, place }`, as in Python since
  2026-11-28. Until then a positional body (`(out, out)`), a named one
  (two fields on one place) and a distributing one built, and each slot
  landed in the place; a routed one was `AmbiguousOutput`, and is this
  error now, checked first. An output edge is one slot, and one slot of
  `Vec<T>` already carries several tokens.
- **Drawn.** `Net::edges`, the topology and the wire list each edge, so a
  shared place has two edges to one transition; `petritype-dot` writes
  two `p -> t` lines, which Graphviz draws apart, and `petritype-layout`
  counts them (`Layout::edges_between`) and gives each a control point to
  one side of the chord (`Layout::fan`). PROTOCOL.md §5.1 still says a
  reader keys an edge by its endpoints, which two such edges share.
- **Tests:** `tests/edges_sharing_a_place.rs`, the cases of Python's
  `tests/test_edges_sharing_a_place.py` that both runtimes allow, and the
  refusals.

**Edges on empty places, 2026-10-14** (slice 22). The proposal
`dev-docs/EDGES_ON_EMPTY_PLACES.md`, its R1 to R7 for this runtime (the
viewer's R7 is petritype-bevy's), shared with the Python track (R8, D2).
Every input edge before this one held its transition back until its place
had a token. That is right for a token the transition works *on*, and wrong
for a place it only needs to *look at* when something is there: "every
promotion running", "every subscriber to notify", where none is a perfectly
good answer. The workaround was an *empty twin*, a second transition with
the same inputs less that edge, guarded on the place being empty: one
optional place doubles a transition, and *k* of them read by one transition
need 2^*k*. (The other workaround, a single token holding a list, stays the
right one for an aggregate; these edges are for members that must be
individual tokens.)

```rust
pub struct DrainEdge<T>(pub PlaceRef<T>);     // body parameter Vec<T>, every token, consumed
pub struct SurveyEdge<T>(pub PlaceRef<T>);    // body parameter Vec<T>, clones of every token; T: Clone
#[non_exhaustive] pub enum EdgeKind { .., Drain, Survey }   // "drain", "survey"
kind.requires_presence() -> bool;   // false exactly for Drain and Survey
EdgeKind::Drain.consumes() == true;  EdgeKind::Survey.consumes() == false
```

- **Fireable** whatever the place holds, nothing included: the scan (§6.1)
  skips the presence check for an edge whose kind does not
  `requires_presence()`. The transition's other edges and its guard still
  apply, so a guard can hold it back. In Petri-net terms a drain is a
  *reset arc*, an extension that leaves the enabling rule alone, and a
  survey a pure data dependency with no effect on enabling at all.
- **Firing.** Launch takes a drain's tokens as a `TakeAllEdge`'s, through
  the same code: every token, oldest first, into a `Vec` that is empty for
  an empty place; `Fired.consumed` (or `Launched.consumed`) names each of
  them, which may be none from that place; the index forgets them and a
  nested token's inner log is lifted (§6.6, rule 3) as for any consuming
  edge. A survey clones every token at launch with the place's cloner, as
  a snapshot clones the front one, and moves nothing: nothing in
  `consumed`, no event of its own. Its clones are owned, so a transition
  whose reads are snapshots and surveys stays detachable and may run on a
  worker; the body sees the place as it was at launch. Nothing new in
  `Marking`, `Snapshot` or `export_marking`.
- **Restore.** `restore_tokens_on_failure` puts a drain's tokens back at
  the front of the place, oldest first, with their ids, as every restore
  does (§7), ahead of anything put in while the firing was in flight. A
  survey has nothing to put back.
- **Checks.** Neither shares its place with another edge of its
  transition (`EdgeCannotSharePlace` since 0.4.0, `DuplicateInputPlace`
  until then). A survey of a place that cannot
  clone, a nested place among them, is `SnapshotOfNoClonePlace` (its
  message says the transition "snapshots" the place, which a survey of
  every token does; no sibling error was worth adding), and `SurveyEdge<T>`
  asks `T: Clone` at compile time as `SnapshotEdge<T>` does. A drain of
  such a place is fine; a restoring transition's drain of one is
  `RestoreOfNoClonePlace`, empty place or not. `assert_no_source_transitions`
  and `assert_acyclic` count only the edges that *always* take a token
  (`EdgeKind::always_consumes`, crate-private: consuming and requiring
  presence), so a transition whose only input is a `DrainEdge` is a source,
  and fires for ever on an empty place, and a loop closed only by a drain
  is not a cycle of the proof's. The termination proof holds over the
  edges it counts (§12): each transition still has one, fed from upstream
  of it, however many tokens a drain carries round meanwhile.
- **Read API and wire.** `TransitionInfo::inputs`, `EdgeInfo::Input` and
  the topology carry the kinds as they carry the others, with
  `filters`/`filter` `None`. With `serde`, `EdgeKind` writes `"drain"` and
  `"survey"`, the names PROTOCOL.md §5.1 gained; the runner sends them
  (§19.3.14), and a reader that does not know them draws a plain edge, as
  §5.1 already said. `petritype-dot` draws them as `TakeAllEdge` and
  `SnapshotEdge` are drawn, with a hollow arrowhead (`arrowhead=empty`, the
  proposal's D5) and `*` on both.
- **Not here** (the proposal's §7): no keyed zero-or-more edge (D4) and no
  mutating one (D3); `requires_presence` is what either would reuse.
  Returning a token to the front of a place from a body (D6) is a separate
  feature.
- **The examples** are two since 2026-10-16, each a conformance net of
  the protocol (PROTOCOL.md §11) with its net in a file of its own that a
  runner can include. The shop, `examples/shop.rs` with the net in
  `examples/shop/net.rs`, surveys: `Price Order` takes an order and reads
  every promotion running, and leaves them running. The traffic light,
  `examples/traffic_light.rs` with the net in
  `examples/traffic_light/net.rs`, drains: the light is one place,
  `Light`, whose one token is its colour, and it has two transitions.
  `Go`, guarded on green and someone waiting, takes every car waiting in
  one firing and reads the light through a plain snapshot, leaving its
  token alone; `Change`, unguarded, takes the light and writes the next
  colour back, so the light cycles for ever. A drain accepts an empty
  place, so without "someone waiting" `Go`, declared first, would fire at
  every green taking nothing and the light would stick there; the example
  never drains an empty queue (the shop's survey shows the zero case), and
  `Go`, whose only consuming edge is the drain, is what
  `assert_no_source_transitions` flags, its guard being what makes it
  safe, so the example does not ask for that check. The
  shared transcripts `07_empty_place_edges.json` and
  `08_traffic_light.json` drive them over the wire, and `tests/slice22.rs`
  holds both. (Until then the shop alone was the proposal's example, and
  drained its promotions at the close of the day.)

**Binding by name, 2026-10-15** (slice 23). The proposal
`dev-docs/BINDING_BY_NAME.md`, its R2 to R9; R1, the tuple cap (§5), is
its own branch. A function is bound to a net in one of three ways, on
either side: **by position** (a tuple of inputs is the parameter list, a
returned tuple fills the outputs in order), **by name** (a struct's fields,
below) and **by type** (routed outputs, §4.5). Inputs and outputs are both
*edges with a kind*: the input side has the eight kinds above, with
`Param<T>` beside them; the output side has one kind today, *put*, a place
handle that appends what a field is worth (§4.5), and a new one (replace,
put at the front, upsert by key) would be added as a struct with an impl of
its own, as drain and survey were added on the input side.

Binding by position goes wrong at the sizes a game reaches
(`dev-docs/TRANSITION_ARITY.md` §1): an insertion shifts every later
parameter, two parameters of one type swap silently, and a struct of
*edges* in the function's signature would tie a library function to one way
of reading its inputs. Binding by name keeps the function's struct free of
places and edge kinds, so one function serves nets that read it
differently:

```rust
#[derive(Inputs)]
pub struct Pricing { pub order: Order, pub promos: Vec<Promo>, pub prices: Prices }

// A shop: the front order, the promotions running, the prices a parameter.
PricingEdges { order: TakeEdge(orders), promos: SurveyEdge(promos), prices: Param::new() }

// A wholesaler, the same function: trade orders only, the prices on a place.
PricingEdges {
    order: TakeWhereEdge(orders, Filter::new().key_in("tier", ["trade"])),
    promos: SurveyEdge(promos),
    prices: SnapshotEdge(price_list),
}
```

- **The derive.** `#[derive(Inputs)]` on a struct with named fields
  generates `<Name>Edges<F0, ..>`: the same field names, one type parameter
  per field bounded by `Reads<field type>`, as visible as the struct,
  `Clone`/`Copy`/`Debug` where its inputs are (`#[binding(edges = "..")]`,
  or `inputs = ".."`, renames it). It refuses enums, unions, tuple and
  empty structs, type and const parameters, a second lifetime and a `where`
  clause, each with a message saying what to write instead.
- **Hygiene (since 0.4.0).** The three derives write every path in full,
  `::core::clone::Clone` and the like for the wiring's derives and
  `::petritype` for the rest, and mark their impls
  `#[automatically_derived]`, so they compile in a `#[no_implicit_prelude]`
  module and beside a caller's own `Clone` or `Debug`.
  `#[binding(crate = "path")]` and `#[routed(crate = "path")]` name the
  crate by another path, a renamed dependency or a re-export. The type
  parameters avoid every name the expansion writes beside them, the
  struct's and the wiring's included, so a struct called `F0` keeps its
  name, and the lifetime avoids every lifetime in the field types, so a
  `for<'a>` inside one is not shadowed. A key given twice in
  `#[binding(..)]` or `#[routed(..)]` is a compile error. A field that
  `#[cfg]` leaves out needs nothing of the derive: the compiler removes it
  before the derive reads the struct.
- **`Reads<V>`**, implemented in the core one line per kind: `TakeEdge`,
  `SnapshotEdge`, `TakeWhereEdge` and `Param` read `T`; `TakeAllEdge`,
  `DrainEdge` and `SurveyEdge` read `Vec<T>`; `MutateEdge` and
  `MutateWhereEdge` read `&'a mut T` for any `'a`. A struct with a
  `&'a mut T` field has that one lifetime and the body takes `Pricing<'_>`;
  the token is borrowed where it lies, and the transition is not
  detachable, as for the edges themselves. The derive asks for the hidden
  engine half, `Extract<'a, V>`, `for<'a>`, which ties the field's lifetime
  to the cursor's (§16).
- **One position, several edges.** The struct of edges is one input
  position: it stands in the input tuple like an edge or a parameter,
  `(PricingEdges { .. }, TakeEdge(stamps))`, and the body takes the struct
  there. Underneath, `EdgeSealed` describes into a vec and extracts from a
  cursor over the erased inputs, one per edge the position described; the
  builder, launch, restore, the index, the shared-place check,
  `TransitionInfo::inputs`, the topology and the wire see the flat list, the
  fields' edges in field order, whatever order a literal writes them in,
  and nothing of the struct. `assert_no_source_transitions` counts the
  edges inside a struct that always take a token, as anywhere else.
- **Errors.** A field wired to the wrong kind is an error at that field of
  the literal, "`TakeEdge<Promo>` cannot fill a field of type
  `Vec<Promo>`", with the struct's own field as the bound it broke; a
  missing field is rustc's own "missing field `promos`".
- **Not here.** A struct of edges is not accepted without the tuple around
  it (the proposal's R6): the blanket impl that would allow it puts two
  impls in reach of every input tuple, and the compiler then reports a place
  handle in a tuple as "not a tuple" and every mismatch of the outputs as a
  mismatch of the inputs; the tuple it is, and the message for a lone
  struct says so. Nested tuples of edges, `Group`, and arrays of edges as a
  position are not public API.
- **The example** is the proposal's pricing library and its nets,
  `examples/pricing.rs` with the library in `examples/pricing/library.rs`
  and the nets in `examples/pricing/nets.rs`; `tests/binding.rs` holds the
  rest, and the till of `TRANSITION_ARITY.md` §2 written by name, fourteen
  inputs and thirteen outputs through one struct each.

Bodies are `FnMut`: they may hold private mutable state (counters, RNGs).
Anything captured must be `Send + Sync`; use `move`, `Arc`, `Mutex`, atomics.

### 4.5 Outputs

Output places are declared in the builder (topology needs the list), as a
tuple or as the struct of outputs `#[derive(Outputs)]` generates. Outputs
are edges too, of one kind today, *put*: a place handle, which appends the
tokens a value is worth to its place. The body's **return type**, and the
outputs it meets, select one of four forms:

**Positional**: the return type is a tuple. Return position *i* goes to
output place *i*, and is `T`, `Option<T>` or `Vec<T>` for that place's `T`
(trait `IntoTokens<T>`). Checked at compile time. A wrong order between
*different* types fails to compile; between *same* types order is the only
distinguishing information and the author must care anyway.

```rust
(heavy, light, count)  ⇐  |ps: Vec<Parcel>| -> (Vec<Parcel>, Vec<Parcel>, Option<u32>)
```

**Named** (slice 23, §4.4): the return type is a struct that derives
`Outputs`, and the outputs are the struct of outputs generated for it,
`<Name>Edges`, whole: each field goes to the place wired to its name,
whatever order the literal writes them in, and is `T`, `Option<T>` or
`Vec<T>` for that place's `T` (trait `Accepts<V>`, implemented for
`PlaceRef<T>` through `IntoTokens`, where a future output kind adds its
own impl). The struct names no place, so one library function's result
goes to different places in different nets, and the order a tuple would
make the author keep is replaced by the field names.

```rust
#[derive(Outputs)]
pub struct Priced { pub order: Order, pub note: Option<Note>, pub restock: Vec<Item> }

PricedEdges { order: priced, note: log, restock }  ⇐  |p: Pricing| -> Priced
```

The struct of outputs is never a position inside a tuple of outputs. The
form reads back as positional (`TransitionInfo::positional`, D4 of the
proposal): its order is real, field *i* to output *i*, and the wire has no
third value, so the runner writes `slot` by field order. A struct that
derives both `Routed` and `Outputs` is unambiguous: the routed form asks for
a tuple of outputs, so the struct is routed whole where the outputs are a
tuple and bound by name where they are its own struct of outputs.

**Routed**: the return type is not a tuple, and the outputs are a tuple.
Each emitted payload goes to the unique output place of its type. Order of
output places is irrelevant.
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

**Distribution**: the body, or an *output distribution function* composed
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
  distribution function) returns by position, `()` and a named struct
  included (slice 23), read off the return type when the transition is
  declared. It is what an output edge's `slot` means: a position in what
  the body returns (a field's, for a named struct) only for a positional
  body, the declared order and nothing more otherwise, so the protocol
  runner writes `slot` for positional outputs alone (§19.3.11), as Python
  writes its `return_index`. A flag rather than an enum of the forms:
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

The same transition may bind its body by name instead (§4.4, §4.5): a
library function takes and returns structs that name no place and no edge
kind, and the net wires their fields through the structs
`#[derive(Inputs)]` and `#[derive(Outputs)]` generate. The struct of inputs
is one input position, so it goes in the input tuple beside any other
inputs; the struct of outputs is the whole of the outputs.

```rust
#[derive(Inputs)]  struct Batch   { parcels: Vec<Parcel>, cfg: Config }
#[derive(Outputs)] struct Weighed { heavy: Vec<Parcel>, light: Vec<Parcel>, count: Option<u32> }

fn weigh(batch: Batch) -> Weighed { /* .. */ }      // knows nothing about the net

b.transition(
    "Weigh",
    (BatchEdges { parcels: TakeAllEdge(inbox), cfg: SnapshotEdge(config) },),
    WeighedEdges { heavy, light, count },
    weigh,
);
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
- Input arity is capped at 16, where Bevy's `IntoSystem` and axum's handlers
  stop (12 until 2026-10-15, `dev-docs/TRANSITION_ARITY.md`). Output arity
  likewise. Inputs and outputs are always tuples: a single one needs the
  trailing comma, `(TakeEdge(p),)` and `(count,)`, and the empty tuple `()`
  means none.
- Bodies may return `Outputs` or `Result<Outputs, E>` where
  `E: Error + Send + Sync + 'static` (trait `IntoOutcome`).
- Beside the tuples, inputs and outputs may be bound by name: a struct of
  edges is one position of the input tuple (alone, `(BatchEdges { .. },)`),
  and a struct of outputs is all of the outputs, never inside a tuple. A
  struct has no cap of its own, so a body with more inputs or outputs than
  the tuple cap allows takes them as one struct each; the net still sees
  every edge, in field order. Prefer it for a library function that more
  than one net wires, or wherever two parameters of one type could swap.
- Mismatch errors use `#[diagnostic::on_unimplemented]` so the compiler says
  "transition body parameters do not match its input edges", not a trait soup.
- Prefer routed outputs when types are distinct: no order to keep track of.

## 6. Firing semantics

### 6.1 Fireability and selection

A transition is **fireable** iff its `enabled` switch is on, its body is in
its slot (not in flight), every input place is non-empty (all four plain
edge kinds need a token present, and a keyed edge one its filter accepts,
§8.3; take edges that share a place a token each, all different, since
0.4.0, §4.4) and its guard, if any, returns true. Guards run
during the scan, after the cheaper checks. `Net::fireable()` lists the
fireable transitions in declaration order; `Net::is_fireable(t)` asks about
one. The scan is linear each step; incremental tracking is a later
optimisation. ("Enabled" alone means the switch: `enable`, `disable`,
`is_enabled`.)

**Added 2026-10-14 (slice 22).** The presence condition is asked only of an
edge whose kind `requires_presence()`: a `DrainEdge` or a `SurveyEdge` reads
every token its place holds, zero or more, and is satisfied by an empty
place, so it never holds its transition back (§4.4). Such a transition is
fireable when its other edges and its guard say so; one whose only inputs
are drains and surveys is fireable whenever it is switched on and home,
and is a source (§12).

Selection among fireable transitions: the net's selector, default = highest
priority (unset scores 0.0, an infinity ranks as itself; `Net::priority_of(t)`
exposes the score), ties broken by declaration order. Custom selector via
`net.set_selector(f)`, `f: Fn(&Net, &[TransitionId]) -> Option<TransitionId>`,
offered the fireable list in declaration order; returning `None` fires
nothing this step. A choice that cannot be made is an error, `SelectionFailed`
(since 2026-11-28, as Python's `ValueError`): a priority that is NaN, which
ranks neither above nor below any other, or a selector's answer it was not
offered. Nothing fires and the cursor stays.

**Added 2026-10-02.** The tie-break is a setting, `TieBreak::FirstDeclared`
(default, the Python rule) or `TieBreak::LastDeclared`, set with
`NetBuilder::tie_break` or `Net::set_tie_break`. With FIFO places the first
drains a pipeline in batches and the second lets each token flow through
before the next enters; a custom selector ignores it. `Net::select()` reports
the choice without firing, and `NetBuilder::resettable()` /
`Net::is_resettable()` record the author's promise that rebuilding the net is
a true reset.

**Added 2026-10-12 (slice 21).** A third tie-break, `TieBreak::RoundRobin`
(Python petritype's `tie_break="round_robin"`; decision B1 of the phase).
Among the fireable transitions of top priority, in declaration order, the
default selector picks the first one declared *after* the transition it
chose last, the *cursor*, wrapping to the first when none follows; with no
cursor yet it picks the first declared. The cursor is the declaration index
of the default selector's last choice taken to be fired, one per net, inner
nets included; it moves under any tie-break and only round robin reads it.
`step` takes the choice, and so do `step_deep`, the drivers and the runner;
`select()` reports it and moves nothing, so a driver that fires in stages
takes it with `Net::choose()`, which is `select()` with the choice counted.
A custom selector neither reads nor moves the cursor, nor does `fire` by
name; a switched-off transition is not fireable, so never a candidate. Each
launch of a concurrent sweep is a choice taken to be fired, and moves it
(since 2026-11-28, §6.4; until then a sweep neither read nor moved it). The cursor lives in memory only: a fresh net, a net restored
from JSON or forked, and a reset (a rebuild) start their round afresh.
A FIFO pipeline alternates its stages under it, and transitions competing
for one place take turns. No wire change: PROTOCOL.md §5 carries no
tie-break.

### 6.2 Three stages

```rust
let firing: Firing = net.launch(t)?;      // stage 1: consume per edge kind; body taken out of the net; Launched
let done: Completed = firing.run();       // stage 2: call the body (no-op if launch ran it inline); catches panics
net.distribute(done)?;                    // stage 3: put body back, check, push to places, Distributed or Failed
                                          //   -> Result<(), DistributeError> (since 0.4.0; NetError until then)

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
- `distribute`'s error is a `DistributeError` (since 0.4.0): the
  `NetError` (`error()`) and, for the refusals that leave the firing as it
  was (`HeldByBody`, `ForeignFiring`, `NotInFlight`), the `Completed`
  itself (`completed()`, `into_completed()`, `into_parts()`), so that it
  can still land in the net that launched it; `None` for `FiringFailed`,
  whose body is home. Its message and `source()` are the `NetError`'s, it
  is `Send + Sync`, and `?` turns it into a `NetError` (dropping the
  `Completed`, as `distribute` did until then) or into a boxed error
  (keeping it).
- If the transition has a `MutateEdge` or `MutateWhereEdge` input, `launch`
  runs the body inline and returns an already-`Completed` firing. Derived
  from edge kinds, no user flag.
- `distribute` is atomic: every target place is checked before any token is
  written.
- `distribute` on a failed `Completed` restores inputs if configured, emits
  `Failed`, returns `Err(FiringFailed)`.

### 6.3 Sequential

```rust
net.step() -> Result<Option<TransitionId>, StepError>   // FiringFailed or SelectionFailed
```

Select, launch, run, distribute, inline. `Ok(None)` = nothing fireable, or
the selector declined. Deterministic given the selector: the basis for tests
and replay. `net.fire(t) -> Result<(), NetError>` fires a named transition
regardless of the selector (`NotFireable` if it cannot fire right now).
`net.step_deep()` (slice 15) is one firing, of `step`'s or inside a token
that carries nets of its own, in the order the stepping rules of the nested
places give (§6.6, since 2026-11-21).

### 6.4 Concurrent

```
loop:
    while t = the net's choice among fireable, not launched this sweep:
        firing = net.launch(t)          // tokens leave the place now; peers cannot take them
        hand firing to a worker         // or run inline if not detachable
    for done in finished:
        net.distribute(done)
    stop when nothing fireable and nothing in flight
```

- Consuming synchronously in the launch sweep is the conflict resolver. No
  locks.
- **Added 2026-11-28, by the owner's decision.** Each launch of a sweep is the
  net's own choice, made as a sequential step makes it (§6.1): the custom
  selector, else priority and the tie-break, the round robin's cursor
  moving with each launch. It is offered the transitions fireable now that
  the sweep has not launched yet, in declaration order, and after each
  launch the choice is made afresh, guards and priorities asked again,
  until nothing is offered or the selector declines. A net with no
  selector, no priority and the default tie-break launches in declaration
  order, as every sweep did before. An inline firing has one turn a sweep.
  A choice that cannot be made stops the run after the firings in flight
  have landed. Python's `_launch_sweep` asks its selector the same way.
- One flight per transition at a time (`body` slot is `None` while airborne).
- **Since 0.4.0** the nets inside the net, at any depth, launch beside it in
  the same way, each by its own choice, onto the same bounded workers, as
  the stepping rules let them go, and a token whose nets have a firing in
  flight is passed over by every edge that takes or changes a token until
  it lands (§13).
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
vehicles carry a cabin, a deck or a pod, and whose journey runs under plain
`step_deep` (until 2026-10-10 it ran the proposal's acceptance script
instead; §18 says why that changed).

```rust
pub trait Nested: Send + Sync + 'static {
    fn nets(&self) -> Vec<(&str, &Net)>;              // each inner net with a name, in a fixed order
    fn nets_mut(&mut self) -> Vec<(&str, &mut Net)>;  // the same, mutably
}
impl Nested for Net { .. }                            // one inner net, itself, named ""

b.place_nested::<T: Nested + Debug + Send + Sync + 'static>(name) -> PlaceRef<T>
b.stepping(place, Stepping)                           // 2026-11-21; BuildError::NotNested on a place that holds no nets
net.nested(place, token) -> Option<Vec<(&str, &Net)>>
net.nested_scope(place, token, f: impl FnOnce(&mut [(&str, &mut Net)]) -> R) -> Option<R>
net.step_deep() -> Result<Option<DeepStep>, StepError>
net.step_inside() -> Result<Option<DeepStep>, StepError>   // slice 17: step_deep without this net's own turn
net.step_deep_staged() -> Result<Option<StagedStep>, StepError>   // slice 21: step_deep, this net's own choice handed back
net.set_stepping(place, Stepping) -> Result<(), NetError>;  net.stepping(place) -> Option<Stepping>   // 2026-11-21
net.is_quiescent_deep() -> bool
net.place_ref::<T>(name) -> Option<PlaceRef<T>>       // a typed handle by name, checked by TypeId

#[non_exhaustive] pub enum Stepping { #[default] InnerFirst, TakeTurns, OuterFirst, Passive }   // 2026-11-21
#[non_exhaustive] pub enum StagedStep { Chosen(TransitionId), Inside(DeepStep) }  // slice 21

#[non_exhaustive] pub enum DeepStep {
    Here   { transition: TransitionId },
    Inside { place: PlaceId, token: TokenId, net: String, nested_id: NetId, step: Box<DeepStep> },
    Subnet { place: PlaceId, nested_id: NetId, step: Box<DeepStep> },   // §6.7
}
```

The other way to nest, a place that holds one net for the outer net's whole
life and shares its port places with it, is the subnet place, §6.7. What
this section says of stepping, lifting and saving holds for it as for a
token's nets, except where §6.7 says otherwise.

- **Declaring.** A token type says which nets it owns; one without
  `Nested` is a compile error (`#[diagnostic::on_unimplemented]`).
  `place_nested` records `T::nets` and `T::nets_mut` in the place's vtable
  (§4.2). The place is no-clone (§4.3); phase 2 lets a snapshot fork it.
- **Running.** `step()` keeps its meaning: one firing of this net.
  `step_deep()` fires at most one transition, in this net or anywhere below
  it, in the order the stepping rules of this net's nested places give
  (below); called in a loop it runs the net and everything in it until all
  of it is at rest. Deterministic given the selectors.
  `is_quiescent_deep()` is §3's quiescence here plus every inner net in a
  place that is not passive quiescent deep; `fireable()` stays shallow. The
  drivers step deep (§13). `step_inside()` (public since slice 17,
  `pub(crate) step_inner` before) is a deep step without this net's own
  turn: the turns ahead of it, then the turns behind it. It is for a
  driver that has already asked `select()` because it fires this net's own
  transitions in stages, and would otherwise ask the selector twice and fire
  inline whatever it chose. The protocol runner (§19.3.11, §19.3.13) and
  the concurrent driver (§13) take the three parts in order instead.
- **The stepping rule** (2026-11-21; `dev-docs/subnet-place-spec.md` §1.3,
  the one definition for both runtimes). It replaces slice 21's deep policy
  (`DeepPolicy::{OuterFirst, Interleaved}`, set per net) and slice 15's
  passive places (`b.passive(place)`), both removed. Each nested place has
  one rule, `Stepping`, set with `NetBuilder::stepping` or
  `Net::set_stepping` and read with `Net::stepping` and
  `PlaceInfo::stepping`; the default is `InnerFirst`. One deep step of a net
  `N` tries, in this order, and stops at the first firing or failure:
  1. every net in a place of `N` whose rule is inner first, in walk order:
     a deep step of that net;
  2. the *ring* of `N`: `N` itself and every net in a place of `N` whose
     rule is take turns, in walk order, starting after the member that
     fired last and going round once. `N`'s turn is one firing chosen by
     its own selector; another member's turn is a deep step of that net;
  3. every net in a place of `N` whose rule is outer first, in walk order:
     a deep step of that net, while nothing of `N` is in flight.

  A passive place is in no band. Walk order is places in declaration
  order, a place's tokens oldest first, a token's nets in `nets()` order. A
  member's turn is its own deep step, so each level follows the rules of
  its own places, and one deep step fires at most one transition in the
  whole tree. Inner first means that whenever the outer net fires, the
  inside is at rest; a net there that never comes to rest stops the net
  around it, and its place takes turns or is passive instead (a net driven
  by time is given time as tokens, and rests between them). Take turns
  means that no live member waits longer than one round, however busy the
  others are. Outer first is the order every deep step had before slice
  21, and the default until 2026-11-21: an outer net that can always fire
  starves its inner nets, so a net meant to run this way gives its outer
  transitions guards that wait on what is inside, as the transit example's
  city waits on its vehicles' doors, and that city states outer first on
  every place that holds vehicles. Its condition on what is in flight is
  what "nothing to fire" means for a driver with bodies airborne, whose
  landing may give the outer net more to fire; the sequential driver steps
  between firings, and never meets it. The concurrent driver keeps each
  rule's meaning and lets the nets a band lets go launch together (§13,
  since 0.4.0).

  The ring's *cursor* is the member that took the last turn in it, `N`'s
  own `NetId` or a take-turns net's nested id, kept per net in memory only:
  never saved, never forked, and a member whose token was consumed or moved
  to a place with another rule is gone, so the round starts at `N` again.
  A turn in the ring moves it, a failed firing as well as one that landed;
  a firing in the other two bands leaves it alone, and so does
  `set_stepping`. `fork` copies the rules, as it does the tie-break; a
  saved net does not carry them, since they are part of the definition.
  Quiescence does not depend on the rules, passive aside.

  What changed: the rule is per place and applies at every level, where
  the policy of the outermost net governed the whole tree and every level
  below fired its own transitions before the nets inside it; inner first is
  new, and the default; the ring holds one level, where the interleaved
  cycle held every net of the tree. Two pieces of evidence tie the change
  to what was there. With outer first on every place, the transit city
  fires exactly what the old default walk fired, in the same order (a test
  beside the example compares the two over 200 firings under each
  tie-break, and `05_nested_nets.json` replays unchanged). One level of
  take turns steps as the interleaved policy did (slice 21's tests, kept
  and moved to the rule).

  As built, `step_deep` is `step_deep_staged` with the chosen transition
  fired inline, and `step_deep_staged` is the three parts: the turns ahead
  (`take_turn(Part::Ahead)`, inside the crate), this net's choice
  (`choose`, which makes `N` the cursor), and the turns behind
  (`take_turn(Part::Behind)`). It hands back `StagedStep::Chosen(t)` when
  the turn falls to this net and makes an inner firing inline otherwise
  (`StagedStep::Inside`). Both parts are worked out from the cursor and
  one walk, taken once per step (`members`, each net with its place,
  token, position and rule; since 2026-11-28, where each part walked
  afresh): a turn that fires nothing changes nothing, and nor does an own
  turn that chose nothing, so the part behind sees what the part ahead
  saw. A net none of whose own places holds nets, which is fixed at build,
  walks nothing. On the wire the rule is
  `stepping`, which the runner writes on every place that holds nets since
  protocol version 2 (§19.3.16); version 1 had `passive`, which the runner
  wrote for a place whose rule is `Passive`.
- **Watching a deep step** (2026-11-22). An inner firing runs its body
  inside the deep step, before the step returns, so a driver that reports
  what is running (the protocol runner's heartbeat, §19.3.5) cannot name
  such a body from outside. `step_deep_staged_with(before_body)` is
  `step_deep_staged` with an observer, a closure passed as a value, which
  the core calls once just before it runs an inner body inline, with an
  `InnerBody`: the transition's id and name, and the way down from the net
  that was stepped, one `PathStep` per level (the path a failure of that
  body carries) beside the nested id of the net each step leads to. It is
  called for no firing of the net that was stepped (that is the caller's
  `Chosen`), for no firing a body makes inside the nets of its own tokens
  (the body's own doing), and for nothing a control's press fires, since
  `press` takes no observer. The core reads no clock for it; the observer
  does, if it wants one. Inside the crate the watch goes down beside the
  turn: `member_turn` and `subnet_turn` add a level around the inner deep
  step and take it off after, and a step without a watch passes `None`, so
  `step_deep`, `step_deep_staged`, `step_inside` and the drivers step as
  before, at the cost of one branch per turn.
- **Passive places.** `step_deep` never enters a place whose rule is
  `Passive`, and its inner nets do not count against `is_quiescent_deep`.
  An outer body steps them itself: `vehicle.inner.step()`, or
  `drivers::run` on it, inside a `MutateEdge` body; a program that is not
  a body uses `nested_scope`.
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
     `TakeWhereEdge`, since slice 22 `DrainEdge`) from a nested place: at
     launch, before `Launched` or
     `Fired`, under the source place and the old id, so nothing logged
     inside a token that is never put back is lost;
  4. a token produced into a nested place: after `Fired` or `Distributed`,
     under the destination and the *new* id, which the reader has then seen;
  5. external input: `put`, `extend`, `set_tokens`, `replace` into a nested
     place after the `Added` or `Mutated`; `remove`, `set_tokens`, `replace`
     of an outgoing token before the `Removed` or `Mutated`;
  6. `nested_scope`: when the closure returns.
  7. a firing the concurrent driver launches inside (since 0.4.0, §13): its
     `Launched`, or an inline firing's whole story, once that net's part of
     the pass is over; its `Distributed` or `Failed` as it lands, through
     every level on the way down to it.

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
  `FiringFailed`, its `path` filled in level by level on the way up (§7),
  and since 0.4.0 its `nested_ids` beside it, the id of the net each step
  leads to, which tells apart two nets of one name in one token.
- **Limits** (the proposal's R10). No shared places: the outer net reaches
  in only through the token, and no inner guard or body sees the outer net.
  (A subnet place's ports are the one kind of shared place, §6.7.)
  Depth is unbounded; the tests go two deep (a passenger carrying luggage).
  Adding a vehicle is adding a token, never a place.
- **On the wire** (phase 3, slice 17): the protocol runner addresses inner
  nets by path or nested id, reports their events wrapped in `nested`
  notifications with versions of their own, and steps them with
  `step_inside` (PROTOCOL.md §12; §19.3.11), since slice 21 with
  `step_deep_staged` (§19.3.13), in the order of the stepping rules since
  2026-11-21.
  `petritype-dot` draws the outer net only.

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
  tokens (`import_marking`, `Command::from_json` and so `apply_json`, and
  around the forks of `snapshot`), so a caller that goes through the net
  never sees the stack. Depth two works either way: a cabin that carries
  the `Luggage` template itself finds it in its own scope, and one that does
  not finds it in the city's, one level out. Deserialising a `Net` with no
  scope open is a serde error saying so.
- **Import is a resync.** `import_marking` checks everything first: every
  name in the marking is a place (`NoSuchPlaceName`), every place of the net
  is serializable (`NotSerializable`, mentioned or not, since emptying a
  place that cannot be filled would lose what it holds), every token decodes
  (`InvalidToken`). Only then does it empty every place and fill it, with
  fresh token ids. A place the marking does not mention loads empty. It
  emits one `Event::Resynced` and no `Removed` or `Added`: it is a
  replacement, not a sequence of changes (§8.1). Incoming nested tokens are
  setup, their inner logs forgotten as a seed's are, so no stale `Nested`
  is lifted later. A rejected import changes nothing and emits nothing.
- **Fork.** `Net::fork` builds the net again from its stamp (the factory
  given the stamp's `Params` with every wire parameter's current value over
  them), imports this net's exported marking, clones every parameter value
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
  runner's example can include it in phase 3, and `transit.rs` steps it and
  holds its tests; the example needs the `serde` feature. Its inner net is
  called `cabin` whatever the template (`Cabin`, `Deck`, `Pod`), its
  parameters are `stopped` (starting true), `seats` and `capacity`, the
  city's is `destination`, and every transition is built switched on.
  **Since 2026-10-10 it runs on its own** (§18 has why): every template
  has two control places, `Doors Open` and `Doors Closed`, holding one
  token per door, the door's name (`Front` in a `Cabin` or a `Pod`, `Bow`
  and `Stern` in a `Deck`), every door open as built. With the
  announcement they are the vehicle's phase: a door open and no
  announcement standing is boarding, a door open and an announcement
  standing alighting, every door closed under way. The city's guards count
  them (`Board` wants a door open and no announcement, `Alight` an
  announcement, `Depart` no door open); inside, `Close Doors` moves one
  door from open to closed once nobody is at the `Door` and no
  announcement stands, so a ferry fires it twice, and `Clear Announcement`
  takes the announcement down once nobody is aboard; outside, `Arrive`
  opens every door, in order, as it puts the announcement up. Alighted
  passengers go to `Arrived`; vehicles follow a fixed loop, `destination`
  (`null` as built) an override. `main` calls `step_deep` until all seven
  passengers have arrived (45 firings from the seed) and prints each firing
  under its path; nothing is switched off and nothing fired by name. The
  two guards §11 had changed from the proposal are said with the doors and
  the announcement now: nobody sits or stands unless the vehicle is
  boarding, and the announcement comes down only once nobody is aboard. Of
  the script's steps, step 6 survives as a save the first time a vehicle
  is between stations, loaded into a freshly built city and stepped to the
  end, the two cities' exported markings, which reach inside the vehicles,
  compared; the others are what the journey does, and its tests check it.

The plan this section held before (a net as the *body* of a transition,
run to completion by a stateful closure, with `inner.as_transition(..)`
sugar) is superseded rather than forbidden: a body may still own a net, but
the engine sees nothing of it. A token is where an inner net lives when the
outer net should see it.

### 6.7 Subnet places and port places

**Built 2026-11-21** (`dev-docs/subnet-place-spec.md`, the one definition
for both runtimes; `dev-docs/subnet-place-concept.md` has the reasons). A
*subnet place* holds one net, its *subnet*, for the outer net's whole life,
and holds no tokens. The subnet is separate and isolated: the outer net
reaches it only through its *port places*, ordinary places of the subnet
that its definition marks as ports, and an edge of the outer net on a port
is an ordinary edge. A game's levels are the example:
`petritype/examples/levels.rs` is a world whose `Level 1` and `Level 2` are
built from one template, heroes entering each by its `Lobby` and leaving by
its `Exit Hall`.

```rust
// The subnet's definition.
let lobby = b.place::<Hero>("Lobby");
b.port(lobby);                                          // BuildError::PortIsSubnetPlace for a port of a subnet place
// keys, style, styled, place_style and serializable on a port of a subnet place: BuildError::PortOption (since 0.4.0; a panic until then)
// The outer net.
let level = b.subnet_place("Level 1", net) -> SubnetRef // not a PlaceRef: an edge on it does not compile (EdgeOnSubnetPlace behind that)
let lobby = b.port_of::<Hero>(level, "Lobby") -> PlaceRef<Hero>   // NoSuchPort, PortTypeMismatch
b.stepping(level, Stepping)                             // a SubnetRef or a PlaceRef: trait PlaceHandle
net.subnet(place) -> Option<&Net>;  net.subnet_scope(place, f) -> Result<R, NetError>
net.port_ref::<T>(subnet: PlaceId, name) -> Option<PlaceRef<T>>
PlaceInfo { .., subnet: bool, port: bool, port_of: Option<PlaceId> }   // and TopologyPlace
Event::Subnet { place, nested_id, event: Box<Event> }
DeepStep::Subnet { place, nested_id, step: Box<DeepStep> }
pub enum PathStep { Token { place, token, net }, Subnet { place } }    // FiringFailed::path
snapshot.subnet(place) -> Option<&Snapshot>
```

- **How a port is resolved.** The outer net has a real place for each port
  of each of its subnet places, a *window*: its own `PlaceId`, the port's
  name and a copy of the port's metadata, pushed right after the subnet
  place in the subnet's declaration order, so that edges, guards, the read
  API and external input name it as any place. Its own queue stays empty.
  Every function that touches a place's tokens finds a window's port in the
  subnet and works on the port's queue, the subnet's key index and the
  subnet's token counter: `slot_of` for readers, `holder_mut` for writers,
  and `resolve_mut` and the lending in `firing/launch.rs` for a firing. The queue
  never moves. A window is kept out of the duplicate-name check and out of
  `place_named`: it is named by its subnet place and the port's name, and
  `port_ref` finds it so. What a port is beyond its tokens (its keys, style,
  serialiser and stepping rule) is the subnet's to declare, so the per-place
  options of the builder refuse a window: `keys`, `style`, `styled`,
  `place_style` and `serializable` with `BuildError::PortOption` from
  `build` (since 0.4.0; they panicked until then), `port` with
  `PortIsSubnetPlace`, `in_group` with `PortInGroup` and `stepping` with
  `NotNested`, all recorded and returned by `build`.
- **Ids and keys.** A port has one queue and one set of token ids, minted by
  the subnet, whichever net puts a token there, so every id minted into a
  port is larger than those already in the subnet. A port's tokens are
  indexed by the subnet alone; a query of the outer net asks the subnet's
  index about the port and answers under the window's id. A token crosses
  the border only by a firing of an outer transition, so it gets a new id,
  as in any firing.
- **Events.** A change is told once, by the net whose transition made it.
  An outer firing on a port reports it in its own `Fired` or `Launched` and
  `Distributed`, under the window's id; a firing of the subnet is in the
  subnet's log, lifted into the outer one as `Event::Subnet` right after it
  fires in a deep step, or when `subnet_scope`'s closure returns.
- **Stepping.** In a deep step a subnet is one net at its place's position
  in walk order, under its place's rule (§6.6), and `DeepStep::Subnet` says
  a firing was in there. A port's tokens that carry nets are the subnet's to
  step, under the rule the subnet gives the port, so a window has no rule.
- **The in-flight rule.** While a transition of the outer net with an edge
  on one of a subnet's ports is in flight, launched and not yet landed, that
  subnet takes no turn (it keeps its place in the ring), and `subnet_scope`
  refuses it with `NetError::InFlight`. The builder records, per subnet,
  which transitions reach it. One firing happens at a time across the tree
  in a deep step, except that an outer body may still run on its own
  thread. The concurrent driver runs the subnet's own firings beside the
  outer net's since 0.4.0 (§13) and keeps this rule: a firing of the outer
  net on a port holds the subnet still until it lands, and a firing of the
  subnet's own lands whatever is in flight out here.
- **Failure.** A failed outer firing restores what it took from a port as
  from any place, into the port. A failure inside a subnet comes up with
  `PathStep::Subnet { place }` at that level of `FiringFailed::path`, so a
  path names every level, a subnet's or a token's. `OnFailure::Disable`
  follows it through `subnet_scope`. A run's `RunSummary::fired_count`
  counts a subnet's firings, as it does a token's net's.
- **Reading.** `PlaceInfo` (and so `TopologyPlace`, with `serde` written
  only when set) says `subnet` for a subnet place, `port_of` for a window,
  naming its subnet place, and `port` for a place its own net marked a port,
  which is how the subnet's own topology shows it. A subnet place's
  `type_name` is `"<subnet>"`; its template, with `serde`, is
  `net.subnet(place)?.template()`. That is every fact the protocol's
  topology needs (PROTOCOL.md §12.7): a subnet place with its ports in
  declaration order, `port: true` inside, an edge naming a port by its
  window.
- **Saving.** A subnet built through a template is stamped, and is saved,
  restored and forked with the outer net. In a marking, a subnet place's
  entry is a list of one, the subnet in its saved form (`template`,
  `marking`, `params`, `enabled`), and a port's tokens are written once, in
  the subnet's marking, so the outer net writes nothing for a window. An
  import puts an entry into the subnet the net already holds, which keeps
  its id, after checking that the entry was saved from the template that
  built it; everything at every level is decoded before anything changes,
  and each subnet lifts a `Resynced` of its own before the outer net's. A
  restore rebuilds the subnets as the outer template's factory builds them,
  from the saved parameters, and fills them so. A fork copies each subnet as
  it copies the net, everything a save cannot carry included, and a
  snapshot holds a snapshot of each subnet, with the port's tokens copied
  into the outer snapshot's window too, under the same live ids. A subnet
  built without a template runs, and the outer net then has no saved form:
  `NetError::CannotSaveSubnet` and `CommandError::CannotLoadSubnet` name the
  place and carry why, `NoTemplate` for that case, the same error again for
  a deeper level. The stepping rules and the ring's cursor are not saved. A
  net without subnet places saves to the bytes it always did
  (`tests/subnet_saving.rs` checks one against what the commit before wrote).
- **Drawing.** `petritype-layout` is told each place's `PlaceKind`
  (`Ordinary`, `Subnet`, or `Port` of a subnet place), read off the net by
  `Layout::compute` and given with each `LayoutPlace` by a renderer that
  lays a net out by name. A subnet place is one node of the layout, so an
  edge on a port ranks and orders as an edge on the box; a port is no node
  of its own and has no `placement`. The box is sized for its port circles
  (of `Layout::port_radius`, a ghost's 14 by default, each with its count
  beside it), its title and the count inside, and its neighbours keep the
  room a place gives them, across its rank and to the ranks beside it. Each
  port's circle goes on the side of the box that faces most of the
  transitions it joins, the side with more ports when that is a tie, and
  on each side the circles follow where their transitions ended up.
  `Layout::subnet_box` answers with the box, the room for its title and
  count, and each circle; `position_of_place` answers for a port with its
  circle's centre, and `radius_of_place` with its radius. A net with no
  subnet places lays out to the same numbers as before (the crate's tests
  pin seven layouts to what the commit before computed). `petritype-dot`
  draws a subnet place as the Python renderer does: one `Mrecord` box,
  lightskyblue, titled with the place's name, with a cell per port in the
  subnet's order showing the port's name and count. The cells are named
  `port0`, `port1`, ... by position, so no character of a name reaches the
  syntax, and an edge on a port ends at its cell (`tailport` from it,
  `headport` into it). The windows are not drawn as nodes, so the other
  places keep their `p<i>` ids, and the box is outlined when a firing
  touched one of its ports. In a subnet's own drawing a port is one dotted
  ring (`style="filled,dotted"`; a second ring, `peripheries=2`, until
  0.4.0, §6.9 says why). The title is the place's name alone, as a
  place's label here is its name and tokens, so a net and its snapshot
  draw the same text; the Python renderer adds the template under it. A
  net with no subnet places draws to the same text as before.
- **Considered and set aside: moving the queue.** The spec first had a
  firing borrow a port's queue into the window for the length of the firing.
  A spike showed it does not hold: the two nets' counts differ while a
  firing is in flight, ids collide, and two firings in flight on one port
  lose a token. The window that resolves to the port was built instead.

### 6.8 Groups

**Built 2026-11-21** (`dev-docs/group-spec.md`, the one definition for both
runtimes). A *group* is a named set of places and transitions of one net,
stated in the net's definition, that only drawings read. The engine never
does: firing, stepping, ids, saving and the events are those of the same net
with no groups. A net whose rules all read and change one shared place
cannot be split into subnets without moving those rules to the outer net,
which leaves the first view no simpler, so such a net stays flat and is read
by gathering its parts under names. `petritype/examples/hike.rs` is one:
walking and eating both change the walker's `Energy`, each concern is a
group, and the energy is in none.

```rust
let walk = b.group("Walk") -> GroupRef                  // DuplicateGroupName
b.in_group(walk, paid);                                 // a PlaceRef or a SubnetRef (PlaceHandle); PlaceInTwoGroups, PortInGroup
b.transition("Fund Walk", ..).group(walk);              // TransitionInTwoGroups
net.groups() -> impl Iterator<Item = &str>              // declared order; snapshot.groups() too
PlaceInfo { .., group: Option<&str> };  TransitionInfo { .., group: Option<&str> }
Topology { .., groups: Vec<String> };  TopologyPlace, TopologyTransition { .., group: Option<String> }
```

| | Group | Subnet place |
|---|---|---|
| What it is | a label on parts of one net | a separate net held by a place |
| Token logic | the flat net's, unchanged | its own net, its own ids and turns |
| Shared places | yes, freely | no, only ports |
| What it is for | reading a large flat net | a part that is really separate |

- **The rules.** A place or a transition is in one group or in none, and
  groups are flat. A group has a name, unique among its net's groups, and
  nothing else; names may hold any characters, and none of them means
  anything. Groups are in declared order, which drawings follow, and
  declaring one or putting a node in one changes no node's id and not the
  order the nodes are declared in. A subnet place may be in a group. A
  subnet, or a net carried by a token, has groups of its own, from its own
  definition, and the place a net has for a port is in none: a port is
  drawn in its subnet place's box and goes where the box goes.
- **Storage.** The net keeps the names in declared order, and a place's
  metadata and a transition's slot keep the position of its group, which
  the read API turns into the name. A window is in none, whatever group its
  port is in inside the subnet. A snapshot carries the names, so it reads
  and draws as its net does.
- **Misuse.** Two groups of one name, a node put in a second group and a
  port's handle put in a group are build errors (`DuplicateGroupName`,
  `PlaceInTwoGroups`, `TransitionInTwoGroups`, `PortInGroup`), reported
  after the controls' names, because `in_group` and `TransitionBuilder::group`
  are infallible, as the other per-node options are. The same group again
  changes nothing. A group handle from another builder panics, naming the
  node, as a place handle from another net does.
- **Saving.** Groups are part of the definition. A marking and a saved net
  never mention them, and a template's net has them each time it is built,
  so a restore and a fork have them too. A net saves to the same bytes with
  and without groups, and a net with no groups writes its topology to the
  bytes it always did: `tests/groups.rs` runs one net with and without
  groups under each tie-break and compares the deep steps, the events, the
  markings and the saved form, and pins the plain net's topology and saved
  form to what the commit before wrote.
- **Drawing.** `petritype-dot` draws a group as a Graphviz cluster,
  `subgraph cluster_<n>` with `n` its position in declared order, the
  group's name as its label and a dotted outline (dashed until 0.4.0,
  when an open net's places took the dashes), holding its places, a
  subnet place with its box, and then its transitions. The nodes in no
  group come first, at the top level, so a net with no groups draws to the
  text it drew before, and every node keeps its `p<i>` or `t<i>` id.
  Closing a group is the viewer's, and is not drawn here. The runner writes
  groups in each net's topology (§19.3.18).
- **Layout.** `petritype-layout` takes the groups in place of the clusters
  by name prefix it had, which are removed (`Clustering`, `prefix_of`,
  `Layout::display_name`): no name is split or shortened, and a node is
  laid out under its whole name. Each `LayoutPlace` and the new
  `LayoutTransition` carry the node's group, `compute` reads them and the
  declared order off the net, and
  `compute_named` and `compute_subgraph` take the order as a list of names,
  a group no node names coming after the listed ones in the order of its
  first node. An open group is laid out on its own, by the passes over its
  own edges, in a frame with room for its title, as a cluster was. The nodes
  in no group are one block with no frame, packed first, before the groups
  in declared order, as Graphviz draws them first. A group named in
  `Layout::closed` is one box among the nodes in no group, sized from its
  name as a subnet place's box is and with no ports, which ranks and orders
  as a node: an edge between a member and a node of the block is an edge of
  the box, an edge inside the group is not drawn, and the edges in one
  direction between the box and one node, or two boxes, are one merged
  arrow; one into an open group crosses its frame and takes no part. Its
  members have no position while it is closed, and `hidden_in` names the
  box that stands for them. A ghosted place gets one ghost beside a closed
  box on each side it needs, whatever the number of members it joins, and
  a ghosted place hidden in a box has no home and no ghosts, its edges out
  being the box's arrows. A part of a net keeps its meaning with group names
  in its focus, a focused group drawn open whatever is closed. A net with no
  groups lays out to the same numbers as before, and open groups to the
  numbers of the clusters they replace (`tests/groups.rs` pins both against
  what the commit before computed).

### 6.9 Closed nets

**Decided and built 2026-11-23, stage one** (step 3c of the edges spec,
with the Python track; the findings are in
`dev-docs/refusing-a-reach-outside-the-ports.md`), **stage two built
2026-11-28**, with the reach enforced and the shield (rules 2, 3 and 8 of
the Python track's note of 2026-11-27). The owner's words: "in
general, we don't want to be injecting tokens arbitrarily into places in
the Petri net... definitely the bodies should not be doing that. I'd say
unless a net has been specifically flagged as one where we expect to
inject tokens, that should also throw an error if your program does it.
But if the user wants to override that by specifying that in the net
definition, I think that's fine. I would rather err on the side of being
too strict here."

Every net is **closed** unless its definition declares it **open**: from
outside it, tokens are put, taken, removed, replaced or set only at its
ports (`NetBuilder::port`), and a net that marks no port takes no token
from outside at all. A net declared open lets anyone outside write to any
of its places. Stage one (2026-11-23) closed only a net that marked a
port, and left a net with none open as every net had been.

**Why every net (stage two).** The owner, 2026-11-27, on learning that an
edge's `reaches` was an unchecked label: it "goes against the most
fundamental principle of this library - that code and visual
representation can not disagree". Measured on the Python suite and
notebooks, about nine in ten writes by a body into a net went into nets
that marked no port, where `reaches` has nothing to name, so he approved
stage two the same day. What a drawing now means: a place marked as a port
is where tokens can enter a net from outside, the `reaches` on an edge is
where its body goes inside a token's net, and a net declared `open` is the
stated exception, which its topology shows and `petritype-dot` draws with a
dashed outline on every place but a port and a subnet place (since 0.4.0,
as the Python renderer). So a body is held to its edge's `reaches`, writes no
closed net inside the one it holds, and a closed net shields the nets
inside it from the program and the runner (the owner, on rule 8:
"unexpected skipping sounds bad"): until then a token put into a subnet's
port by addressing the subnet skipped whatever route the outer net draws
to that port.

```rust
b.port(inbox);                       // where tokens enter from outside
b.open() -> &mut Self                // the override: open, ports or not
net.is_open() -> bool                // the declaration
net.is_closed() -> bool              // the rule's answer: not declared open
net.try_put(place, token) -> Result<TokenId, NetError>      // and try_extend, try_set_tokens
NetError::ClosedNet { net: String, place: Option<String>, act: &'static str, why: ClosedRefusal }
ClosedRefusal::{NotAPort, Undeclared, Deeper, Around { net: String }}
NetError::HeldByBody { net: String, act: &'static str }
```

**How a way in is drawn** (the owner, 2026-11-28, for both renderers). In
the drawing of the net that marks it, a port is one dotted ring,
`style="filled,dotted"`, and a port of an open net is dotted too, never
dashed as well; an open net's other places are dashed, and a group's frame
is dotted. Until then a port had a second ring (`peripheries=2`), which
looked like a place nested inside a place, and only a nested net is to
look nested. The marks stay light so that a firing and tokens moving are
what catch the eye. Making ports stand out more is to be an option of a
visualiser, not built yet; the default drawing keeps the one dotted ring.

**Who is outside.** A body, guard or key function of a transition of the
net around it, by any edge; the program and its drivers; a runner's
requests. **Inside:** its own transitions' bodies, guards and keys; the
handlers of its own controls while `Net::press` runs them; the tokens its
definition starts it with (seeds, and a template's tokens); and the
engine's own moves, a firing's outputs, a failed firing's restore and a
deep step's inner firings.

**Tokens.** The check is on the net itself, in every public method that
writes a token: `put`, `extend`, `set_tokens`, their `try_` forms,
`remove`, `replace`, and `apply` of an erased `Command` (and so
`apply_json`). It holds whoever calls: the program, a closure of
`nested_scope` or `subnet_scope`, and a body that holds the net by a
mutate or mutate-where edge or owns it through a take, take-where,
take-all or drain edge (an owned net is the same net, so the same check).
A read edge lends `&T` and has nothing to write with. The internal paths
go round it: seeds (`insert_erased` in `build`), outputs (`push_produced`),
restores (`restore_consumed`), imports (`load_marking`), forks and
restores from a saved form (both through `import_marking` on a new net),
and deep steps. The check, in `Net::admit`, in this order:

1. A handler of the net's own control, while `press` runs it, writes
   anywhere.
2. A body that holds the net *by an edge* writes a closed net only at the
   ports that edge names (`Undeclared` at another port, `NotAPort`
   elsewhere), and an open net anywhere but at a window onto a closed
   subnet (`Deeper`). Nothing around the net counts: the body is on its
   side of it.
3. A body that holds the net *deeper*, through a scope on a net it holds,
   writes no place of a closed one (`Deeper`), and is judged on an open
   one as in 4.
4. Anyone else writes a closed net at its ports only (`NotAPort`) and an
   open one anywhere, unless the net is *shielded*, when nothing lands
   (`Around`, naming the nearest closed net around).

The net's own rule is judged before the shield, as in Python: a private
place of a net inside a closed net is `NotAPort`, its port `Around`.

**Windows.** A window, the place a net has for a port of one of its subnet
places (`port_of`), is a place of the net that holds it, and can never be
one of that net's own ports (`PortIsSubnetPlace`). A write at it from
outside is judged by the holding net's rule first: refused when the
holding net is closed, landing when it is open, the subnet's port taking
it. A write at the window made inside the holding net (its seeds, its own
bodies' outputs) lands as before. This is the strict, literal reading the
owner asked for; a program that deals tokens onto a map's windows, as
graph-tactics' `GameNet::deal` does, declares the map open.

**Knobs and firing by name.** Setting a parameter, a switch, the tie-break,
a stepping rule or a selector, firing by name (`fire`, `launch`,
`distribute`, `step_inside`), pressing a control and importing a marking
stay with the program, its drivers and the runner, whatever the net. They
are refused to a body of the net around it on every net it holds, closed
or declared open: the override opens the places, not the controls. (Stage
one refused them only on a net that marked a port, so the transit
example's `Depart` and `Arrive` set a cabin's `stopped`.) A setting a body
wants goes to a transition of the net's own, which is inside it, or is
left to the program. Pressing a control is refused on every held net for
the same reason: a control is the program's and the runner's, and its
handler is inside the net. Reading and stepping are
allowed for everyone, a body included: `step` and `step_deep` on a net a
body holds run that net's own transitions, which are inside it.

**What else a body leaves alone** (built 2026-11-28, the Python track's
rows 32, 33 and 42). On a net it holds, by an edge or deeper, a body is
also refused draining its events (`drain_events`: the net around lifts
them as the firing ends), moving its tie-break cursor without a firing
(`choose`; `select` reads only), a staged step (`step_deep_staged`,
`step_deep_staged_with`, which leave a choice for `launch` and
`distribute`), and a run of a driver with `OnFailure::Disable`, a switch,
refused before anything fires with `DriverError::HeldByBody` in both modes
and by `run` and `run_indefinitely` alike. Each is recorded on the hold as
the other knobs are. Driving is stepping: a body may drive a net it holds
with `drivers::run` in either mode, with the same marking. The engine's
own moves go round these checks by internal paths: `take_events` for the
lifting of an inner net's events, `choose_now` for a step's choice, and
`launch_for_driver` and `distribute_for_driver` for the concurrent driver,
which until then met the refusal of `launch` inside a body and panicked.

**The mark.** A body is told apart from the program by a mark on the nets
it holds, `Net::held`, a `closed::Mark`: a `Weak` to a `Hold` the firing
owns (`Firing::hold`, `Completed::hold`) and the reach, the edge's index
or *deeper*. The hold keeps the transition's `reaches` (an `Arc`, shared
with `TransitionSlot::reaches`). `consume` marks the nets of each
token a consuming edge takes from a place of nets, through
`Nested::nets_mut`, with that edge, and they stay marked, wherever the
body puts them, until the firing ends at the end of stage three, when the
`Arc` is dropped and every `Weak` lapses by itself. `mark_lent` marks the
nets of each token a mutating edge lends, around the inline call to the
body, and `unmark_lent` puts back what was there. `nested_scope` and
`subnet_scope` called on a marked net mark the nets they lend *deeper* for
the closure's time, so the mark travels down through the scopes to any
depth. A read edge lends `&T`, so it marks nothing. The program never
holds a marked net: a token's nets are marked only while the body that
has them runs, inline or owned by an airborne `Firing`. A body that puts a
net it built in place of one it holds (`*unit = fresh`) has made a new
net: the swap is let through, and the new net carries no mark, so its
knobs are the body's; its tokens are under its own rule, as the program's
are, and its seeds are inside. The owner kept the swap on 2026-11-28 (a
game that resets a character swaps its whole net), and it is told as a
new net: its own `NetId`, the old id gone from the state, the token's
`Mutated` in the log and every later event of its nets under the new id;
on the wire a new `nested_id`, the old one answering `unknown_net`. No
global state, no unsafe code: the mark
is per firing, and `Hold` is `Send + Sync`, so it travels with a
detachable body to a worker thread.

**The shield.** The program reaches a net inside another only through
`nested_scope` and `subnet_scope` (so do the drivers and the runner's
`descend_mut`), so the chain of nets is in hand at each level and no net
needs to know where it rests, as Python's notes do. A scope opened on a
closed net, or on a shielded one, lends its nets with `Net::shield`, a
`Weak<str>` to the name of the nearest closed net around, which the scope
owns (`closed::Lending`). Opened through a window, the subnet behind it
counts first. The scope restores what each net had after the closure,
under a `catch_unwind` that resumes the panic afterwards, and drops the
name, so a shield lapses when its scope ends however it ends and no fork,
save or copy carries one. A handler of the net's own control is inside
it, so a scope it opens on its own net shields nothing by that net, and
the looking stops at a net a body holds by an edge. Imports
(`admit_import`) are refused into a shielded net too, with no place. The
knobs of a shielded net stay the program's: the drivers' `disable_at`
reaches an inner transition through the same scopes.

**A refusal.** `NetError::ClosedNet` names the net (by its template, or
its id, `NetId(7)`, when no template built it), the place (none for an
import) and the act (`"put a token into"`), and its `why` which refusal
it was; the message says how to put each right: for `NotAPort` mark the
place as a port, start the net with the token in its definition, or
declare the net open; for `Undeclared` name the port in the edge's
`reaching`; for `Deeper` go through a port of the held net; for `Around`
give the closed net a port and a transition that carries the token in, or
declare it open. `NetError::HeldByBody` names the net and the act
(`"set a parameter of"`) and says to do it from the program or give the
net a transition of its own that does it.

- **For the program**, the methods that return a `Result` return it, the
  net unchanged and silent. `put`, `extend` and `set_tokens` keep their
  signatures and panic with the refusal's message, as they already panic
  on another net's handle, so a program in the right changes nothing; the
  `try_` forms return it. `set_tie_break`, `set_selector`,
  `clear_selector` and `drain_events` panic for a body that holds the
  net, which only a body can meet, and since 0.4.0 the first three have
  `try_set_tie_break`, `try_set_selector` and `try_clear_selector`, which
  return `Result<(), NetError>`. `step_inside`, `step_deep_staged`,
  `step_deep_staged_with` and `choose` return
  `StepError::HeldByBody { net, act }` (since 0.4.0; until then they
  panicked), which `?` turns into `NetError::HeldByBody`; `select`, which
  only reads, keeps `SelectionFailed`. `press` answers a body with
  `PressError::Refused` carrying the message, the handler not run.
- **For a body**, an ordinary failed firing with the named error as its
  error, not `BodyPanicked`. A net that refuses a body keeps the first
  refusal on the mark, and `call_body` fails the firing with it after the
  body returns, whether the body passed it on, dropped the `Err`, or caught
  the panic of a method with no `Result`. (The panic's payload is the
  refusal's message, a `String`, so that the default panic hook prints it
  for a program; the typed error comes back through the mark.) What is
  restored is what any failure restores (`restore_tokens_on_failure`), and
  what the body did before the refusal stays done; the refused call
  changed nothing. A refusal in a consuming firing costs what any failure
  costs: the Rust core cannot restore a consumed token that carries a net,
  since a place of nets has no cloner (`BuildError::RestoreOfNoClonePlace`),
  so a firing that takes such a token and is refused loses it.
- **For the runner**, `inject` refused this way answers `closed_net`, at
  the top level and by path or nested id, at a window as at any place
  (§19.3.23), after `unknown_place` and `invalid_token`: `Net::apply`
  checks the place and the tokens' type before the rule.
- **Rust cannot hold everything the same way.** A body that writes a net
  no edge handed it (one it built, or reached through a captured
  `Arc<Mutex<Net>>`) and meets a shield has no hold to record the refusal
  on, there being no global state: it fails its firing only if it passes
  the error on, as the program would.

**A question changes no net** (the Python track's rule 7). A guard, a
priority and a selector are given the net as `&Net`, a key function the
token as `&T`, and `fireable` and `select` take `&self`, so the compiler
refuses them every change through the library (a compile-fail case,
`tests/ui/fail_guard_changes_its_net.rs`). It cannot close interior
mutability inside a token (a `Mutex` or an atomic reached through `&T`),
state a closure captured (an `Arc<Mutex<Net>>` of another net), or a net
the closure builds and writes itself, which Python refuses. An output
distribution function is given the body's result by value and runs inside
the body's call, so it can change a net that result carries, under the
body's mark; Python refuses it that.

**Whole-state acts** stay allowed for the program: importing a marking
(`import_marking`, refused to a body on a net it holds, as a knob is, and
to everyone on a shielded net), restoring from a saved form, forking,
rebuilding as a reset, and saving. They replace the state as a whole, and
are how a save is loaded.

**What Rust enforces.** Every write to a place goes through a `Net`
method, and a body has nothing else to write with, so the rule holds in
full in this runtime: there is no queue a program or a body can reach
round it. (Python can enforce it only through its library's own functions
and the runner, plus a before-and-after check for a body; its documents
say so.)

**What broke in this repository.** Stage one broke one test: the runner's
`a_subnet_place_s_entry_holds_its_subnet_and_its_ports_under_the_token_limit`
injected into a level's `Arena`, which the level does not mark a port; its
fourth hero now walks in from the lobby by a step. The conformance nets
and the eighteen shared recorded fixtures replay unchanged, and the
`post-office` net with the nineteenth, `19_closed_nets.json`, shows the
rule on the wire. Stage two broke 166 tests, doc tests and example tests
in the core crate, nearly all of them the program putting a token into a
net that marked no port. Most now mark that place a port, which is what
their story said it was; a few seed the net, and the tests about writing
anywhere from outside, or at a window, declare their net open. The tests
of stage one that said a net with no port was open, and that a body kept
its knobs on one, now say the opposite. The reach and the shield broke
33 more: bodies whose edges now name the port they write, and tests that
reach into a net inside a closed one from outside, whose outer nets are
now declared open (one seeds the net instead, one uses the open
`levels`). The conformance nets were changed as the Python track's note
lists, and the shared transcripts re-recorded there replay.

#### Closed nets: what a program changes

How to tell that a net is closed: it does not declare itself open. On the
wire its topology shows no `open`; in Rust, `Net::is_closed()`; in Python,
`graph.is_closed`.

A write from outside at a place that is not one of its ports is refused
(Rust `NetError::ClosedNet`, Python `ClosedNetError`, the runner's
`closed_net`). There are three ways to make it land:

1. mark the place as a port (`b.port(place)`);
2. start the net with the token in its definition, as a seed or a
   template's token, which is inside the net;
3. or declare the net open (`b.open()`, Python `open=True`), which lets
   anyone outside write to any of its places.

A body's write at a port its edge does not name is put right by naming
the port in the edge's `reaching`; a body's write into a net inside the
one it holds goes to a port of the held net instead, for a transition of
that net to carry further in. A write or an import from outside into a net
inside a closed net is put right by declaring every net around open, or by
giving the closed net a port and a transition that carries the token in.

Knobs (a parameter, a switch, the tie-break, the stepping rule, a
selector, a control) and firing by name stay with the program and the
runner, on any net, a net inside a closed net included. A body of the net
around is refused them on every net it holds, open or closed: give such a
setting to a transition of the net's own, or leave it to the program. So
are draining a net's events, `choose`, a staged step and a driver's
`OnFailure::Disable`: a body reads the marking instead, steps the net, or
drives it with `OnFailure::Stop` or `OnFailure::Continue`.
Whole-state acts (importing a marking, restoring, forking, resetting,
saving) stay allowed for the program, except an import into a net inside
a closed net.

## 7. Failure semantics

- **Expected outcomes are data.** Route a token to an error place via a
  routed enum or a positional `Option` return.
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
  `TakeAllEdge`, `TakeWhereEdge`, `DrainEdge`) at launch; the clones travel with the
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
- `BuildError`: from `NetBuilder::build`.
- `NetError`: API misuse: `NoSuchPlace`, `NoSuchToken`, `TypeMismatch`,
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
  (a snapshot that could not fork a nested token), `RebuildFailed {
  template, source }` (a fork whose factory failed or whose marking did not
  load back) and, since subnet places (§6.7), `CannotSaveSubnet { place,
  name, source }` (an export, a save or a fork of a net whose subnet has no
  saved form, `source` saying why: `NoTemplate` for one no template built).
  Also `NotSubnet` (a `subnet_scope` of a place that holds no subnet), and
  `InFlight` without the feature too, for a `subnet_scope` while a firing on
  one of the subnet's ports is airborne.
- `TemplateError` (`serde`, slice 16): from `Templates::build`:
  `NoSuchTemplate { name }`, `Factory { name, source }`.
- `FiringFailed`: a body returned `Err` or panicked; carries the transition,
  the boxed error, and whether inputs were restored. Since slice 15 also
  `path`: where the transition is when it is in a net below the one that was
  stepped, one step per level, outer to inner, empty for the net's own. Each
  step is a `PathStep` (§6.7): `Token { place, token, net }` for a token's
  inner net, `Subnet { place }` for a subnet place's subnet, which names no
  token. `step_deep` fills it in on the way up, and the message names it
  (`transition "Sit" (TransitionId(0)) inside PlaceId(1) / PlaceId(0) #5
  "cabin" failed: ..`). It was a `(place, token, net name)` per level until
  subnet places, which a level with no token could not use. Since 0.4.0
  `nested_ids` holds, beside each step, the nested id of the net it leads
  to, which `OnFailure::Disable` follows.
- `CommandError`: external input rejected. Wraps `NetError` transparently
  (unknown place, transition or token; type mismatch); with `serde`, also
  `NotSerializable`, `InvalidToken`, `NoSuchPlaceName`,
  `NoSuchTransitionName`, `NoSuchParamName`, `InvalidParam` and
  `ParamNotSerializable` from the JSON route, and `CannotLoadSubnet { place,
  name, source }` for a subnet place's entry in an imported marking that
  does not load into its subnet (§6.7). A rejected command changes nothing
  and emits nothing.
- `TooManyFirings`: the driver fuse, carrying a `RunSummary`.

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
net.places()      -> impl Iterator<Item = (PlaceId, PlaceInfo<'_>)>    // by value: name, type_name, serializable, nested, stepping (Some iff the place holds nets, 2026-11-21), key_names (slice 18), subnet, port, port_of (§6.7), count (count is live, never stored)
net.transitions() -> impl Iterator<Item = (TransitionId, TransitionInfo<'_>)>  // by value: name, inputs, filters (slice 19), outputs, positional (slice 17), enabled, in_flight, detachable
net.edges()       -> impl Iterator<Item = EdgeInfo>   // Input { place, transition, kind, filter (slice 19) } | Output { transition, place, slot }; transition order, inputs then outputs
net.topology()    -> Topology   // places, transitions, edges and topology_counter as owned values; shape only, no counts or switches (slice 14)
net.groups()      -> impl Iterator<Item = &str>   // §6.8: the groups' names in declared order; PlaceInfo::group and TransitionInfo::group say a node's
net.place_named(&str) -> Option<PlaceId>;  net.transition_named(&str) -> Option<TransitionId>
net.place_ref::<T>(&str) -> Option<PlaceRef<T>>   // slice 15: a typed handle by name, checked by TypeId
net.nested(place, token) -> Option<Vec<(&str, &Net)>>   // slice 15: a nested token's inner nets (§6.6)
net.nested_scope(place, token, f) -> Option<R>    // slice 15: the same, mutably, lifting on return
net.subnet(place) -> Option<&Net>;  net.subnet_scope(place, f) -> Result<R, NetError>   // §6.7: a subnet place's subnet, to read, and to step or change
net.port_ref::<T>(subnet, &str) -> Option<PlaceRef<T>>   // §6.7: this net's handle for a port, by its subnet place and its name
net.is_quiescent_deep() -> bool                   // slice 15: quiescent here and in every inner net not in a passive place
net.step_inside() -> Result<Option<DeepStep>, StepError>   // slice 17: step_deep without this net's own turn, for a driver that asked select() itself (§6.6)
net.step_deep_staged() -> Result<Option<StagedStep>, StepError>   // slice 21: step_deep with this net's own choice handed back, in the stepping rules' order (§6.6)
net.choose() -> Result<Option<TransitionId>, StepError>   // slice 21: select(), and the choice counts for the round robin (§6.1); StepError since 0.4.0
#[cfg(feature = "serde")]
net.param_name(type_name) -> Option<&str>         // slice 17: the wire name behind a ParamChanged's type name, from serializable_param
net.fireable() -> Vec<TransitionId>;  net.is_fireable(t) -> bool;  net.is_enabled(t) -> bool;  net.priority_of(t) -> Result<f64, SelectionFailed>
net.count(place)  -> usize
net.tokens(place) -> impl Iterator<Item = (TokenId, &dyn Any)>
net.label(place, token) -> Option<String>            // via PlaceMeta.debug; truncated to label_limit (default 100) + "..."
net.debug(place, token) -> Option<String>            // the same Debug text untruncated: what label is cut from
net.token::<T>(place, token) -> Option<&T>           // typed downcast for custom widgets
#[cfg(feature = "serde")]
net.json(place, token) -> Option<Result<serde_json::Value, NoJsonForm>>  // structured view, serializable places only; no JSON form for a NaN or an infinity
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
`distinct_values` and `matches` since slice 18, by scanning, `groups` since 2026-11-21, the counters). It is *not* a `Net`, and
cannot be: bodies are `Box<dyn FnMut>` and cannot be cloned, and an in-flight
transition has no body in the net at all; a what-if copy is a rebuild from
the builder. What it is for: a renderer or inspector on another thread owns
a consistent copy while the driver keeps `&mut Net`, and a test keeps the
state at one moment to compare with another. The fireable set is not stored
(guards are closures on the live net). Snapshotting emits nothing and moves
no counter. Without `serde` a net with a nested place cannot be snapshotted;
with it (slice 16) the nested place's tokens are forked from their templates
(§6.6), and `snapshot.nested(place, token)` reads their inner nets, under the
live token ids. A subnet place's subnet is snapshotted as a `Snapshot` of
its own, which `snapshot.subnet(place)` reads (§6.7).

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
  a reader needs them first: a viewer's filter bar, the protocol's
  `key_names` (PROTOCOL.md §13.1). The extractor returns an array of
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
  owned, and with `serde` written only when non-empty, as `nested` is, so
  a net without keys has the topology it always had.
- **Filters.** A token passes when it passes every clause (AND across) and
  lies in a place the filter covers; a clause accepts one of its values (OR
  within), compared by kind and value. A token of a place that does not
  declare a clause's key passes no such clause, so a filter with a clause
  never keeps a token of a keyless place, while a filter with no clauses
  keeps every token of the places it covers. The restriction is by place
  only (`places` by id, `prefix` by the start of a place's *name*, the
  protocol's §13.2 and the Python as-built) and never by transition, which
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
  clause with the fewest, and reads only those tokens (found in their place
  by binary search, since a place's queue is in id order), checking the
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
  value before the body changed it. A `nested_scope` closure, where a key
  may read the inner nets, is told as a body is: the token's keys are read
  before the nets are lent and again after. The one change the net learns
  of only afterwards is an inner firing of `step_deep`; there a key whose
  new value already lists the token is left as it is, and a key the firing
  changed is found by looking through that key's values (since 2026-11-28,
  where every key was). Reading each member's keys before its turn would
  cost more: most turns tried fire nothing. `import_marking` empties the index and refills it as it
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
  without a token moving. The value's key is written from its JSON once
  per value (since 2026-11-28, where every scan wrote it for every keyed
  edge): the parameter's slot keeps it from the first clause that asks
  until the value is set again, by `set_param`, a command, an import or a
  fork. The guard runs after, as it always has, and so
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
- **Checks.** A `TakeWhereEdge` may share its place with other takes
  (since 0.4.0, §4.4); a `MutateWhereEdge` shares it with no edge
  (`EdgeCannotSharePlace`). `TakeWhereEdge` consumes (`EdgeKind::consumes`),
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
  `Topology` writes that shape in its id form
  (`{"edge":"input",..,"kind":"take_where","filter":{"clauses":[{"key":"class","values":["Bird"]}]}}`,
  `places` as `PlaceId`s, `filter` and `filters` written only where there
  is one) because a `Filter` alone knows no names: the protocol's shapes
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
- **The example, second edition** (2026-10-13, decided with the Python
  track, which re-recorded the shared `06_keys_query_filter.json`). The
  first edition's `Spread Savanna Reef` and `Decline Forest` were plain
  `MutateEdge`s with guards on the front token, so on screen the zebra,
  first in `Savanna`, halved itself onto the reef, and the robin, first in
  `Forest`, halved down to a population of 0 that stayed; with `season`
  `"Summer"` by default a plain run was three spreads and seven declines.
  Now every transition but `Settle` is keyed and every guard reads the
  token its edge would take (`first_match`, through the example's
  `oldest`): `Migrate Forest Wetland` and `Migrate Tundra Wetland` take the
  oldest bird (`take_where`, `class in {Bird}`) in autumn, the default;
  `Spread Savanna Wetland` halves the egret, the oldest bird of `Savanna`,
  while it is above `spread_threshold` (200), and sends a copy with the
  rest to `Wetland` (`mutate_where` with an output); `Decline Forest`
  halves the deer (`diet in {Herbivore}`) while above 10 and `Decline
  Tundra` the lemmings (`species in {Lemming}`) while above 100, floors
  that keep every population above 0. The seed is twenty tokens, four to a
  habitat. Left alone, the net fires nineteen times (two migrations from
  `Forest`, one from `Tundra`, three settlings, three spreads, five and
  five declines, in that order under `FirstDeclared`) and stops. No engine
  change: the keyed edges of this slice already did all of it.

**Choosing by another input, 2026-11-23** (step 1 of the edges spec,
agreed with Oleg; built with the Python track). A keyed edge's filter may
compare one of its keys with a key of the token another input of the same
firing gives: "the unit this order names". It is a third kind of clause,
beside constants and a net parameter, so there is no new edge kind.

```rust
Filter::new().key_is_input("name", orders, "unit")     // orders: PlaceRef<Order>
#[non_exhaustive] pub enum Clause { In {..}, IsParam {..}, IsInput { key: String, input: InputKey } }
pub struct InputKey { pub place: PlaceId, pub key: String }
MutateWhereEdge(units, Filter::new().key_is_input("name", orders, "unit"))
BuildError::{BoundSourceNotAnInput, BoundSourceGivesMany, BoundSourceIsBound,
             BoundToTwoSources, BoundKeyNotDeclared}
```

- **Words.** The edge whose filter holds such a clause is *bound*; the
  input it names is its *source*. Both values are keys declared on their
  places, so the clause is plain data: no `serde` is asked of anything, and
  it shows on the wire and in a drawing as declared.
- **Which pair fires** (decided by Oleg): the oldest source token that has
  a match, with the oldest match for it. The source's candidates are its
  place's tokens in queue order, or, for a source that is keyed itself
  (`TakeWhereEdge`, `MutateWhereEdge`), those its own filter accepts. For a
  candidate, each edge bound to the source asks its place for the first
  token its whole filter accepts, each input clause reading the candidate's
  key, a constant or a parameter clause beside it as usual. The first
  candidate with a match in every bound edge is the pair; a candidate
  without one stays where it is and holds back none behind it. This is the
  rule keyed edges already followed, carried over to a pair. Several
  sources in one transition are independent, each reading its own place.
- **Fireable** when every source has a pair, the other edges hold as
  before, and the guard passes, asked last. **Launch** asks the same of the
  same marking once, before any edge moves a token (`consume`), and every
  source and bound edge then takes its chosen position: a take source takes
  its token from wherever it stands, a snapshot source clones it, a mutate
  source lends it, each bound edge takes or lends its match. Positions stay
  good because a bound edge and its source read places no other edge of
  the transition reads (`BoundPlaceShared`, since 0.4.0, when two takes
  came to share a place). Ids, events,
  detachability and the restore are those of the edge kinds: a failed
  firing that restores puts both tokens back at the front of their places,
  with their ids.
- **The cost.** The bound filters are resolved against the parameters once
  per question; each candidate costs one extraction of its keys and one
  lookup in the key index per bound edge (`first_resolved`), never a scan
  of the bound place. A keyed source's candidates come from the index too
  (`all_resolved`, the postings of its rarest clause in id order, or the
  queue of an unordered place), lazily, so the first candidate that pairs
  ends the question. The crate's tests compare the pairing with a scan
  through places put out of id order by restores (`pairing.rs`).
- **Build checks** (`pairing::bindings`, after a transition's other input
  checks): the source must be an input of the transition
  (`BoundSourceNotAnInput`) whose edge gives one token, a take, a snapshot
  or a mutation, keyed or not (`BoundSourceGivesMany` for a take-all, a
  drain or a survey); a source may not be bound itself, which refuses a
  chain, a cycle of two and a clause naming its own edge's place
  (`BoundSourceIsBound`); one filter names one source, though two clauses
  may read two keys of it (`BoundToTwoSources`); and the bound place must
  declare the clause's key and the source place the key it reads
  (`BoundKeyNotDeclared`). Constant and parameter clauses keep their old
  rule, a key no token has accepting nothing. Several edges may be bound to
  one source. A transition keeps what was found as `bindings`, and an edge
  in one is asked with its pair rather than on its own.
- **Outside a firing** there is no source token: a query, `Filter::matches`,
  `Net::matches` and `Net::first_match` accept nothing through the clause.
  A runner refuses one in a client's `query` or `set_filter` with
  `invalid_params` (PROTOCOL.md §13.2).
- **Ports.** A source or a bound edge may be a port of a subnet place: its
  candidates and its match are read from the port's queue and the subnet's
  index, as any keyed edge on a port is, and on the wire the clause names
  the source as an edge names its place, `{"place", "port"?, "key"}`.
- **On the wire** (PROTOCOL.md §5.1, §13.2, §13.5) the clause is
  `{"key": "name", "input": {"place": "Orders", "key": "unit"}}`, in a keyed
  edge's `filter`; the core's `Serialize` writes the place by id. The
  protocol stays version 2: a new optional clause shape, shown as declared.
  `petritype-dot` writes it `name = Orders.unit`, a port's source as
  `Subnet/Port.unit`. The shared conformance net `couriers` and
  `16_keyed_by_input.json` over it show both forms, a source token passed
  over, a failure that restores both tokens and the refusal of a `query`.
  `tests/keyed_by_input.rs`.

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

Reaching one token by a value another input carries ("the unit this order
names") was a drain and a refill until 2026-11-23: drain the units, find the
one the order names in the body, and put every unit back, each under a fresh
id, a viewer re-animating them all, the choice made in the body where no
guard or reader could see it. A bound edge does it in one firing
(`Filter::key_is_input`, above): the unit is chosen by the index, the
others are not mentioned and keep their ids, and an order whose unit is not
there waits without holding back the orders behind it.

### 8.4 Read edges

**2026-11-23** (step 2 of the edges spec, names agreed with Oleg; built with
the Python track). Three input edges lend the body tokens to look at,
without taking or copying them:

```rust
pub struct ReadEdge<T>(pub PlaceRef<T>);                  // body parameter &T, the front token
pub struct ReadWhereEdge<T>(pub PlaceRef<T>, pub Filter); // &T, the oldest token the filter accepts
pub struct ReadAllEdge<T>(pub PlaceRef<T>);               // Vec<&T>, every token, zero or more
EdgeKind::{Read, ReadWhere, ReadAll}                      // "read", "read_where", "read_all"
kind.lends() -> bool                                      // a mutating or a read kind: runs inline
```

- **Words.** These three, with snapshot, survey, mutate and mutate-where,
  are the *non-consuming edges*, the edges that leave their token in its
  place; "read edge" means only these three. The documents said "read
  edges" for the whole group until this step.
- **Nothing moves.** No token leaves its place, no id changes, nothing is
  copied, and no event names a token read: no `Mutated`, and the firing's
  `consumed` does not list it. A firing whose inputs are all non-consuming
  is still a firing, reported as such. `Read` and `ReadWhere` hold their
  transition back while there is no token (one the filter accepts, for a
  read-where); `ReadAll` never does, and lends an empty `Vec` on an empty
  place, as `Survey` copies one.
- **No `Clone`.** The impls ask `T: Send + Sync + 'static` alone, so a read
  edge reads a `place_no_clone` place and a place of net-carrying tokens,
  where a snapshot or a survey is refused (`SnapshotOfNoClonePlace`). A
  token that carries nets is read in place, so its nets keep their ids.
- **How the borrow is lent.** Two erased inputs, `ErasedInput::Lent(&'a
  ErasedToken)` and `ErasedInput::LentAll(Vec<&'a ErasedToken>)`, beside
  `Borrowed`. Stage one records each read edge as it records a mutating
  one (`Lent`: the argument, the token's position, the slot that holds it
  and, for a port, the port), in a list of its own, `looked`, so that the
  mutation events and the reindexing see only the mutating edges. The pass
  that lends the `&mut`s (`borrow_mutated`) lends these too: it walks the
  places with `iter_mut`, a subnet's ports with the subnet's own, and for a
  read edge turns its slot's `&mut` into a shared borrow of the one token or
  of every token. Every borrow it takes may be alive at once because a read
  edge shares its place with no other edge of its transition
  (`EdgeCannotSharePlace`).
  No unsafe code. The body cannot change what it is lent (`tests/ui/
  fail_read_edge_mutates.rs`, `fail_read_edge_writes.rs`), and nothing else
  can while it runs.
- **Inline.** A lent borrow ties the firing to the net, so a transition
  with a read edge is not detachable, as one with a mutate edge is not
  (`EdgeKind::lends`), and `launch` runs its body there and then. The
  runner reports such a firing in one piece.
- **With the other rules.** `ReadWhere` is keyed: it chooses as
  `TakeWhere` does, through the index, a port's through its subnet's; its
  place must declare keys; and it may be bound to a source by a
  `Clause::IsInput` (§8.3). `Read` and `ReadWhere` give one token, so each
  may be a source; `ReadAll` gives every token and is refused as one
  (`BoundSourceGivesMany`). None of the three consumes, so none counts in
  `assert_acyclic`, `assert_no_source_transitions` or
  `RestoreOfNoClonePlace`. Each works on a port of a subnet place.
- **Drawn** by `petritype-dot` dotted and dim grey, where a snapshot is
  dashed; a read-where with its filter as the label, and a read-all with the
  hollow head and the `*` of the edges that take every token, zero or
  more. The shared conformance net `roll-call` and `17_read_edges.json`
  read a place of net-carrying tokens by each form. `tests/read_edges.rs`.

### 8.5 Ports on a token's net, and the reach an edge declares

**2026-11-23** (step 3 of the edges spec, parts 3a and 3b; built with the
Python track). A net a token carries may mark ports as any net may
(`NetBuilder::port`); until now only a subnet's showed them.

```rust
MutateEdge(field).reaching(["Orders In"])      // any edge kind: Reaching<E>
EdgeInfo::Input { .., reaches: Vec<String> }   // empty for an edge that names none
TransitionInfo::reaches: &[Vec<String>]        // TopologyTransition::reaches, one per input edge
BuildError::{ReachIntoNoNets, EmptyReachName}
```

- **Ports shown everywhere (3a).** `PlaceInfo::port` was always true on a
  marked place of any net; the runner now writes `port: true` on the
  topology of every net, the loaded net and a net a token carries as well
  as a subnet (PROTOCOL.md §5.1). `petritype-dot` already marked such a
  place in a net drawn on its own, with a second ring then and one dotted
  ring since 0.4.0 (§6.9).
- **No declaration on the place of nets.** A place whose tokens carry nets
  does not say which ports they offer: the nets arrive with their tokens at
  run time and may differ, each shows its own ports when read by path, and
  the edge's reach is the outer net's statement of what it uses. A
  declaration on the place would be a second statement of the same thing,
  true of the nets the author had in mind and checked against none.
- **The reach (3b).** `.reaching([..])`, an inherent method on every edge
  kind, wraps the edge in `Reaching<E>`, which reads, takes or lends exactly
  as `E` does and describes itself as `E` with the names beside it
  (`InputSpec::Edge::reaches`). The names travel with the transition
  (`TransitionSlot::reaches`, beside `filters`) to the read API, the
  topology, a snapshot, the wire (`reaches`, written only when not empty)
  and a drawing (`petritype-dot` writes `reaches Orders In` as the label's
  last line). A `Reaching` field may stand in a struct of inputs.
- **Enforced since 2026-11-28** (§6.9). A body writes a closed net its
  edge hands it only at the ports the edge names. The names are an upper
  bound: a name the token's net does not have as a port is found out
  nowhere, the nets arriving at run time, and is no error. The build
  refuses only what can never be right: a reach on a place whose tokens
  carry no nets (`ReachIntoNoNets`) and an empty name (`EmptyReachName`).
  `EdgeInfo::Input` gained a field, so code that builds one by struct
  literal adds `reaches: vec![]`.
- **The conformance net** `trays` and `18_ports_on_a_token.json`: a unit, a
  token that is a net with two ports, handed an order through one and its
  shot collected from the other, each outer edge naming the port it
  reaches, and the unit read by path showing `port`. `tests/reaches.rs`.

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
so fireability, `edges()`, the shared-place check, the cycle and source walks
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
  name (`type_name::<T>()` is a compiler detail, not a stable key) and
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
- `stepping` on a place that holds no nets, one not declared with
  `place_nested` (`NotNested`, the first one given a rule; slice 15 for the
  `passive` mark, which the rule replaced on 2026-11-21). `stepping` itself
  stays infallible, as the other per-place options are.
- A `keys` declaration that names a key twice (`DuplicateKeyName { place,
  name }`), or a second `keys` declaration for one place
  (`KeysAlreadyDeclared { place }`); the first offender, after `NotNested`
  (slice 18, §8.3). `keys` itself is infallible.
- Two or more input edges of one transition on one place only where each
  is a take or a take-where (`EdgeCannotSharePlace { transition, place,
  kind }`, since 0.4.0, §4.4; until then any two were `DuplicateInputPlace`),
  checked for each transition before its edges one by one; two output
  edges of one transition to one place (`DuplicateOutputPlace`, since
  0.4.0), after its bound edges.
- `SnapshotEdge` or `SurveyEdge` (slice 22) from a `no_clone` place, both
  `SnapshotOfNoClonePlace`.
- `restore_tokens_on_failure` on a transition with a consuming (`TakeEdge`,
  `TakeAllEdge`, `TakeWhereEdge`, `DrainEdge`) input from a `no_clone` place.
  (A nested place counts as `no_clone` for both, with `serde` too: its
  tokens are copied by a fork, which can fail, and these two copy on the
  launch path, where nothing may; §4.3, §6.6.)
- A keyed edge (`TakeWhereEdge`, `MutateWhereEdge`) on a place that declares
  no keys (`KeyedEdgeOnUnkeyedPlace { transition, place }`, slice 19,
  §8.3): it could never fire. Checked edge by edge with the three above, the
  first offender in transition order.
- A keyed edge that chooses by another input (`Clause::IsInput`, §8.3,
  2026-11-23), after the transition's other input checks: a filter naming
  two sources (`BoundToTwoSources`), a source the transition does not read
  (`BoundSourceNotAnInput`), a bound edge or a source on a place another
  edge of the transition reads (`BoundPlaceShared`, since 0.4.0), one that
  gives every token (`BoundSourceGivesMany`), one that is bound itself or
  the edge's own place
  (`BoundSourceIsBound`), and a key either place does not declare
  (`BoundKeyNotDeclared`); the first offender in transition order, then
  edge order.
- Groups (§6.8): two of one name (`DuplicateGroupName`), then the first
  node put in a second group (`PlaceInTwoGroups`, `TransitionInTwoGroups`)
  or a port's handle put in one (`PortInGroup`). `in_group` and
  `TransitionBuilder::group` stay infallible.
- A declared `Param::<T>` with no `param(value)` of that type
  (`MissingParam`); with `serde`, two parameter types sharing a wire name
  (`DuplicateParamName`). Both before routing.
- Routed-output resolution (§4.5): `UnroutableOutput`, `AmbiguousOutput`,
  `UnreachableOutput`, each naming the transition and the type or place.
- (Return position count vs output place count is carried by the types; a
  `debug_assert!` in `distribute` guards the erased index.)

Opt-in:
- `b.assert_acyclic()`: iterative three-colour DFS over consuming input edges
  (`TakeWhereEdge` among them) and output edges; non-consuming edges
  excluded (they move no tokens), and since slice 22 `DrainEdge` too, which
  consumes but may take nothing, so the proof does not lean on it (§4.4).
  Since 2026-11-28 a subnet place is followed through: a port leads to
  every port of the same subnet place that the subnet's own flow reaches,
  at any depth, itself included when a way leads back to it, so a cycle
  through a subnet's ports is found; one wholly inside a subnet is the
  subnet's own. Roots are transitions in declaration order, then places;
  the first cycle found is reported as
  `BuildError::Cycle { path }`, node names from the repeated node round to
  itself (first == last, so it normally starts at a transition), a port as
  `S/In` with `(inside subnet place "S")` between two ports, joined with
  `" -> "` in the message.
- `b.assert_no_source_transitions()`: every transition has at least one
  `TakeEdge`, `TakeAllEdge` or `TakeWhereEdge` input, an edge that always
  takes a token; a transition with only non-consuming edges
  (`MutateWhereEdge`, `SurveyEdge` and the read edges among them) fails,
  and so, since slice 22, does one whose only consuming input is a
  `DrainEdge`, which fires for ever on an empty place.
  First offender in declaration order → `BuildError::SourceTransition`.
- Both run after the always-on checks, source transitions before cycles.
- The pair is a static termination proof; each alone proves less, and the
  docs say so.

Compile-time (no check needed): edge kinds vs parameter types, positional
return position types and count, non-unit return with zero output places, seeded token
types, `Distribution::to` argument types.

## 13. Drivers

Thin functions over the stages, in `petritype::drivers`:

```rust
pub enum Mode { Sequential, Concurrent, ConcurrentWith { workers: NonZeroUsize } } // Default: Sequential
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
  sequential mode a tick is one firing then a sleep (one firing per frame,
  as Python's `_tick_sequential`), a `step_deep` since slice 15; in
  concurrent mode it is one launch sweep,
  and the wait on the completion channel *is* the sleep, so a completion is
  distributed the moment it lands. A Bevy system per frame is this loop with
  the frame as the tick. No `Limits`: an indefinite run has no firing budget.
- `Mode::Concurrent` uses `std::thread::scope` and an `mpsc` channel of
  `Completed`. It launches everything fireable, each launch the net's own
  choice (§6.4, since 2026-11-28; until then in declaration order, the
  selector and `priority` shaping sequential runs only). Non-detachable
  firings run inline, have one turn a sweep, and trigger another sweep.
- **Added 2026-11-28: bounded threads** (proposal 3.1, first piece). The
  bodies run on a lazy pool of scoped worker threads, `std` only, where
  each launch had a thread of its own before: a job queue
  (`Mutex<Receiver<Firing>>`) the workers take from in turn, and a count of
  the jobs queued or running that a worker counts down as its body returns.
  A worker is spawned only when a job is queued, that count is above the
  workers there are (no worker is free) and the cap is not reached. The cap
  is `available_parallelism()` (1 if unknown) for `Mode::Concurrent`, asked
  the first time a worker is wanted, and `workers` for
  `Mode::ConcurrentWith`, a variant of `Mode` rather than a field of
  `Limits` because `run_indefinitely` takes no `Limits`. With a cap of 1
  the bodies run one at a time, in launch order, off the driver's thread.
  A thread that cannot be spawned is no error: the job waits for a worker
  there is, and with none the body runs on the driver's thread. The
  workers go home when the driver drops its end of the queue, so every one
  is joined before the driver returns. The reusable `WorkerPool` with
  `run_with_pool` (the proposal's second piece) is not built.
- **Added 2026-11-28: a panic on the driver's thread.** A guard, priority,
  selector, key extractor, `Clone` or `each_tick` that panics stops the
  launching as an error does: the loop runs under `catch_unwind`, every
  firing airborne is then waited for and distributed (a landing that
  panics is kept and the others still land), and the first panic goes on
  with `resume_unwind`. So a driver never leaves a transition in flight,
  panic or not. `launch` puts the body back when stage one panics, and
  `nested_scope` and `subnet_scope` index, lift and check on a panic as on
  a return (§17).
- **Added 0.4.0: stage one takes all its tokens or none.** It runs in
  three passes: `plan` reads the net and runs every key extractor and
  `Clone` stage one needs (the keys of each token to be taken, the clones
  a restoring transition keeps, a snapshot's, a survey's, a parameter's),
  changing nothing; `lift_and_mark` and `mark_lent` run `Nested::nets_mut`
  on the tokens where they lie; `take` moves the tokens and runs none of
  the net's own code. So a panic of any of it leaves every token where it
  was, with its id and its index entries; inner events already read out
  are emitted as `Net::lift` emits them for a token at rest, and the marks
  lapse with the firing's dropped hold. No token is cloned for it. Past
  that point is only an inline body's own aftermath, the extractor that
  reindexes a token a mutating edge lent and the `nets_mut` that unmarks
  its nets: a panic there loses what the firing took, which the body
  already owns.
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
  inline, as `step_deep` would choose it, and sweeps again (an outer guard
  may read an inner marking). Inner nets never get threads of their own in
  this version, and the ordering guarantee is the sequential driver's.
  (Superseded in 0.4.0: inner nets launch beside the outer net, below.)
- Both limits count every launch, inner ones included, and the fuse blames
  the first fireable transition at any depth. `RunSummary.fired_count`
  counts inner successes as well as the outer net's: an inner firing is not
  in the outer `fired_count`, so the driver counts it as it lands.
- `OnFailure::Disable` switches off an inner transition where it is, found
  by the failure's `path`; its `Toggled` is lifted like any inner event.

**Done, slice 21** (the deep policy, §6.6; 2026-10-12), **and the stepping
rule since 2026-11-21**:

- `Mode::Sequential` is `step_deep` in a loop, in `run` and
  `run_indefinitely`, so it follows the stepping rules as they stand; the
  loop did not change.
- (Superseded in 0.4.0 by deep concurrent firing, below; the rules kept
  their meaning.) `Mode::Concurrent` takes a deep step's three parts as
  `step_deep` does, a launch sweep being the outer net's own turn. Each
  time round the loop is one turn: an inline firing among the turns ahead of the outer net's (its
  inner-first places, then the ring's members before it), else the sweep
  (which, when it launches anything, makes the outer net the ring's cursor,
  through `Net::took_own_turn` inside the crate), else an inline firing
  among the turns behind it (the rest of the ring, then the outer-first
  places, which the net offers only while nothing of its own is airborne).
  After an inner turn the loop takes whatever has landed without waiting
  and goes round again. `run_indefinitely` takes one such turn a tick. So
  an inner-first net comes to rest before the outer net launches, a
  take-turns net has its turns whether or not outer bodies are airborne,
  and an outer-first net waits for the sweeps to find nothing and for
  every body to land, which is how every inner net ran here before slice
  21. The loop has no rule of its own: the parts are
  `Net::take_turn(Part::Ahead)` and `Net::take_turn(Part::Behind)`, inside
  the crate, where slice 21 had the round's two ranges (`Net::round`,
  `Net::inner_turn`) and a flag for the interleaved policy.
- The fuse blames what the round would try next: `first_fireable_deep`
  takes the parts in the same order, each member's own first one level
  down. In the concurrent driver (since 2026-11-28) each turn blames its
  own: an inner turn the first fireable of its part (`first_fireable_in`),
  a sweep the transition it chose, so a selector that declines attempts
  nothing and trips nothing, as in Python's concurrent loops. Since 0.4.0
  every net's sweep blames the transition it chose, at whatever depth.

**Added 0.4.0: deep concurrent firing** (proposal 3.2, built to the
owner's answers to the questions it left). Asked for by programs whose
inner nets are many and alike, a game's characters or a room's machines,
whose bodies queued behind one another on the driver's thread while the
outer net's ran in parallel.

- **A pass.** Each time round its loop `Mode::Concurrent` makes one pass
  over the whole tree (`Pass::round` in `drivers.rs`): every net takes its
  round, and a net's own turn in it is a launch sweep of its own
  transitions (`Pass::own`), each launch its own choice by its selector,
  priorities and tie-break, its round robin's cursor moving, a detachable
  body handed to the call's one crew of workers, an inline one landed on
  the spot. The cap is the call's, for every level. `run_indefinitely`
  makes one pass a tick.
- **The rules keep their meaning** (§6.6); what changes is that the nets a
  band lets go go together. A net is *at rest* in a pass when its round
  launched nothing at any depth and nothing the driver launched inside it
  is in flight. Band 1: every inner-first member's round; the rest waits
  until every one is at rest, so the net around them fires only with the
  inside at rest, as in a deep step. Band 2, the ring, from where its
  cursor says: the net's own sweep and every take-turns member's round,
  each in every pass, none waiting for another to come to rest; the cursor
  moves to each that launched, so a member that could not go this pass
  goes first in the next. Band 3: every outer-first member's round, once
  the ring is at rest and nothing of the net's own is in flight, which is
  the rule's condition on what is in flight, unchanged. Passive members,
  and nets a body holds, are never entered. A subnet held still by the
  in-flight rule of §6.7 takes no turn and keeps its place in the ring.
- **Addressing.** A detachable firing goes to the crew with its *way*, one
  `deep::Level` per net on the way down: the `PathStep` a path names it by
  and the net's nested id. It lands by it (`Net::within_way`): each level
  found by its place, its token's id and its net's id, the completion
  distributed in the net at the end, and on the way back up what each net
  logged lifted into the net around it (`Event::Nested`, `Event::Subnet`)
  and each token indexed again, as a deep step lifts a member's firing.
  The ids tell apart two nets of one name in one token. `PathStep` itself
  is unchanged; `FiringFailed` carries the ids beside its path
  (`nested_ids`, as `InnerBody` already did), which is what
  `OnFailure::Disable` follows in both modes.
- **Busy tokens.** The crew counts, per net id, the firings airborne that
  were launched in that net or below it. A token one of whose nets is
  counted is busy (`deep::Busy`) until every one has landed. Every edge
  that takes or changes a token (`EdgeKind::takes_or_changes`) minds it.
  A take, a take-where, a mutate or a mutate-where edge passes it over as
  if it were not in its place, through fireability, the pairing of bound
  edges, shared places and stage one alike, and a transition that could
  only use busy tokens is not fireable for now. A take-all or a drain
  (`EdgeKind::takes_every`) takes every token of its place, as it is
  drawn, so its transition is not fireable while any of them is busy, and
  fires once they have all landed, holding the place meanwhile (below); a
  drain on a place of busy tokens alone waits for them too. Read edges and
  guards see the token as it stands, the firing in flight and all, as a
  guard already sees its own net while one of its own firings is in
  flight. The rule is what keeps a way good and keeps a net with a firing
  in flight out of a body's hands. It is the concurrent driver's alone:
  `fireable`, `launch`, the sequential driver and the protocol runner
  choose with no busy set, and so as they always did.
- **A take-all or a drain holds its place** (the owner's decision,
  0.4.0). A transition that would be fireable but for the busy tokens of a
  place it takes every token of, its switch, its guard and its other
  inputs all letting it (`Net::waiting_with`), holds every place of nets
  it takes every token of: the driver launches nothing new inside the nets
  their tokens carry, at any depth, until the transition has had its turn.
  What is in flight there lands, the place comes to rest, and the
  transition is fireable and offered to the net's ordinary choice, its
  selector, priorities and tie-break, among whatever else is fireable
  then. A net sets what it holds at the end of each of its own sweeps
  (`deep::Holds`, kept on the call's crew from pass to pass): a transition
  that was offered in that sweep, chosen or not, holds nothing, and
  neither does one that is no longer fireable otherwise (its guard false,
  another input taken, switched off), so its hold lapses there and the
  nets go on. A subnet's port, reached through its window, is held in the
  subnet, whose round steps the nets there. Only those nets are held: the
  net's other transitions, readers, guards and takes of that very place
  among them, its other places and the nets inside them go on as before,
  and a pass in which a hold kept nets from their round is never the one
  that calls the net quiescent.
- **Why the hold.** The firings that make a token busy are bodies on the
  crew, which land without the waiting transition, so the wait ends once
  they have landed, unless a token is made busy again before the net's own
  sweep looks. Without the hold that was what happened to a take-all or a
  drain on a take-turns or an outer-first place whose nets never come to
  rest by themselves, two or more of them or one with two firings in
  flight at once: one net landed and launched again while another was in
  flight, a token of the place was busy at every sweep, and the transition
  waited for as long as they ran, where the sequential driver fires it
  between their steps. With the hold no token of its place becomes busy
  again before its turn. An inner-first place needs none: the net's own
  sweep comes only once every net there is at rest.
- **What the hold leaves to the program.** A wait can still last for good
  if a body in flight waits, by some means of the program's own, for the
  waiting transition or for a launch the hold keeps back. A selector that
  declines the transition when it is offered ends the hold as surely as
  one that chooses it: the nets go on, and the next time the transition
  waits they are held again. Under a selector that never chooses it, those
  nets come to rest at each of its waits and go on after each offer,
  slower but never stopped for good, and the pause is the selector's doing.
  A net that leaves the tree while it holds a place sets nothing again;
  what it held is dropped once such entries pile up (`Holds::prune`).
- **Closed nets.** The firings of an inner net are its own moves (§6.9):
  the pass and the landing reach the nets inside directly, as a deep step
  does, not through `nested_scope` or `subnet_scope`, so no shield and no
  body's mark stands in their way, and they launch and land with
  `launch_for_driver` and `distribute_for_driver`. A body that drives a net
  it holds with the concurrent driver gets the same deep firing in that
  net, on its own call's crew.
- **Failures and limits.** `stop_after_n_firings` and
  `error_after_n_firings` count every launch at every level, checked before
  each; the fuse blames the transition the net chose. A failure inside
  carries its way as its path and ids; `Stop` keeps the first,
  `Disable` switches the transition off where it is as it lands,
  `Continue` counts it. A choice an inner net cannot make stops the run
  under any `OnFailure`, its path naming the net. Whatever ends the loop,
  an error or a panic of the net's own code at any depth, everything in
  flight lands first (`Crew::land_all`). A panic inside a member's round,
  or inside a deep step's member turn, goes on only once what that net had
  logged is lifted into the net around it, at every level; a member's
  events are lifted before its token is indexed again, so a key extractor
  that panics loses none of them. Until then such events stayed in the
  inner net's log.
- **Waiting.** After a pass that landed a firing on the spot, at any
  level, the driver passes again at once, taking what has landed meanwhile,
  rather than waiting for a body in flight: the marking moved. Until 0.4.0
  only an inner turn did so; an outer inline firing waited for a landing.
- **A token moved under a firing in flight.** Only `each_tick` of
  `run_indefinitely` could move one, since no edge chooses a busy token. A
  completion whose way no longer leads to its net is landed in the net
  found by its id anywhere in the tree (`Net::find_way`); a net no longer
  in the tree has gone with its token, and the completion with it.
- **Evidence.** `tests/deep_concurrent.rs`: sibling nets, subnets and nets
  three levels down whose bodies wait for each other to start, under each
  rule, on a deadline rather than a sleep; the cap holding across levels;
  the busy rule for each kind of edge, readers and guards included, a
  take-all and a drain waiting for every token of their place, a drain on
  busy tokens alone among them; the hold, on take-turns and outer-first
  places of two nets that never rest or of one with two firings in flight,
  under each mode, on a clock and through a subnet's port, each ending with
  the drain taking the whole place, where before the hold each concurrent
  run went on to its firing limit; a drain
  whose guard is false holding nothing, a hold lapsing when the guard
  turns false or another transition takes the drain's other input, two
  drains holding their own places while a third place's net goes on, the
  held place read and taken from meanwhile, a selector that declines the
  drain three times and one that never chooses it, and the limits and
  failures while a place is held; each
  rule's order beside the sequential driver's and no member starved, a
  take-turns subnet and the net around it sharing a port included; a
  confluent net three levels deep with ports, ending with the sequential
  driver's marking and firings under caps 1, 2, 3, 8 and the default, and
  sixty times more; two nets of one name under `Disable`; a failure, a
  choice and a panic inside, each landing what is in flight; the limits;
  and a body driving a net it holds. The protocol runner does its own
  stepping, and its shared transcripts replay unchanged.
- **Considered and not done.** Letting the net around inner-first nets
  launch once their launches were handed over (the proposal's second
  reading): the outer net would fire with the inside not at rest, which is
  what inner first rules out. Holding busy tokens back from readers and
  guards too: the owner kept them readable. Letting a take-all or a drain
  take the tokens that are not busy and leave the others, as first built:
  it took part of its place, which is not what it draws, and a drain on
  busy tokens alone fired and took nothing. Holding, of a transition with
  two such places, only the one whose tokens are busy: the nets of the
  other could launch meanwhile and keep it waiting in turn. Keeping a hold
  past an offer the selector declined, until the transition fires: a
  selector that never chooses it would stop those nets for good. Limiting
  a ring member's turn
  to one launch, as a deep step's turn is one firing: every net would then
  wait for every other's landings, which is what the change is for
  avoiding; where two members take from one port the cursor makes them
  alternate. A lock per inner net or per token: a launch takes its tokens
  at once on the driver's thread, which is the whole of the conflict
  resolution, as at the top. A switch back to the old inline stepping: not
  built; `ConcurrentWith { workers: MIN }` keeps the bodies from
  overlapping but not the old order, and `Sequential` keeps a fixed one.
  The reusable `WorkerPool` (proposal 3.1, second piece) is still not built.

## 14. Crate layout

Cargo workspace, edition 2024, MSRV = current stable minus two (must be
≥ 1.78 for `#[diagnostic::on_unimplemented]`).

| crate | contents | deps |
|---|---|---|
| `petritype` | builder, `Net`, events, checks, `drivers` | `thiserror`; feature `serde` adds `serde`, `serde_json` |
| `petritype-derive` | `#[derive(Routed)]` on enums (variants) and structs (leaf); `#[derive(Inputs)]` and `#[derive(Outputs)]` on structs (slice 23, §4.4) | `syn` 2, `quote`, `proc-macro2`; re-exported by `petritype` unconditionally as `petritype::Routed`, `petritype::Inputs` and `petritype::Outputs` |
| `petritype-dot` | Graphviz DOT text of a net and its marking (**done, slice 12**) | `petritype` only |
| `petritype-layout` | the layered layout: ranks, orders, groups open in frames or closed as one box each (§6.8), ghosted hubs, and subnet places as one box each holding its ports' circles (§6.7), in its own `Point` and `Bounds` (moved out of `petritype-bevy` 2026-10-19, so a renderer that is not Bevy can share it) | `petritype` only |
| `petritype-activity` | the activity model behind the viewer's thumbnail: a pulse and an ember per place and transition, decaying on wall time or on firings, with flow, failure and a base glow from the token count; `ActivitySettings`; the `Grid` of cells from a whole-net layout; the `Thumbnail`'s colour per cell and its SVG (2026-10-19; since then a record of activity over time and an event log, each with a player) | `petritype`, `petritype-layout`; feature `serde` adds `serde`, `serde_json` |
| `petritype-bevy` | `NetRes`, mirror systems, token entities, in-flight highlighting, task-pool driver | pins one Bevy version |
| `petritype-tokio` | only if a need appears | |
| `petritype-runner` | turns a user's binary into a net process speaking the shared protocol (**done, slice 14**, §19.3) | `petritype` with `serde`, `serde_json`, `rustix` on Unix |

Examples (each also an integration test), mirroring the Python acceptance set:
quickstart, parcel distribution, caching (union return → routed enum),
match-up tokens (`TakeAllEdge` + guard + positional), coloured balls
(`Distribution`), web scrape (routed enums), execution modes (sequential vs
concurrent), the scaler of `read_edge_scaler.rs` (`SnapshotEdge` + `MutateEdge`,
two non-consuming edges; the file keeps its older name). A ninth,
transit (slice 15), is no port: it is the nets-as-tokens proposal's example
(§6.6), which ran the proposal's acceptance script until 2026-10-10 and has
run its journey on its own since. Since slice 16 it is also the protocol's
conformance net `transit`, in two files (`examples/transit.rs` for the
journey and its tests, `examples/transit/net.rs` for the net, which a
runner's example can include), and needs the `serde` feature
(`required-features`), since its vehicles save their cabins as JSON.
A tenth, wildlife (slice 18), is the token-keys proposal's example and the
protocol's conformance net `wildlife`, split the same way
(`examples/wildlife.rs` for the proposal's questions and their answers as
tests, `examples/wildlife/net.rs` for the net) and needing `serde` for the
same reason: its species are JSON and its `season` is a wire parameter.
Since slice 19 its `Migrate` is a keyed edge (§8.3), and since its second
edition (2026-10-13, §8.3) every transition but `Settle` is, and a plain
run ends after nineteen firings.
An eleventh, shop (slice 22), is the edges-on-empty-places proposal's
example and the protocol's conformance net `shop`, split the same way
(`examples/shop.rs` for its script and its table as tests,
`examples/shop/net.rs` for the net) and needing `serde` because every place
is marked serializable for the wire's `inject`; its net depends on
`petritype` alone. Since 2026-10-16 it shows the survey edge alone, and the
drain edge has an example of its own, traffic light, the protocol's
conformance net `traffic-light`, split and built the same way
(`examples/traffic_light.rs`, `examples/traffic_light/net.rs`): cars
waiting at a light that cycles red, green and amber, all going when it
turns green.
Levels (2026-11-21, §6.7) is the subnet places example, in one file
(`examples/levels.rs`): a world whose `Level 1` and `Level 2` are subnet
places built from one template, heroes entering each by its `Lobby` port
and leaving by its `Exit Hall` port. It saves the world part of the way and
restores it, so it needs `serde`.

**Done, slice 12.**

- `petritype-dot` renders through a `DotSource` trait implemented for `Net`
  and `Snapshot` (a render thread can own a snapshot), takes `&` only, and
  is a pure function of source and `Style { rankdir, show_tokens,
  max_tokens, highlight, graph_name }`. Node ids are `p<i>` / `t<i>`; names
  live in labels, escaped. The styling mirrors the Python renderer (ovals
  deepskyblue, boxes lightgreen, gold for in flight, magenta outline for the
  last firing via `Highlight::from_event(&Event)`, dashed for `SnapshotEdge`,
  dotted-both-ends for `MutateEdge`) with four additions: disabled
  transitions grey and dashed, `*` on a `TakeAllEdge`, return positions numbered on a
  transition with more than one output, red for a `Failed` event. Since
  0.4.0 the places of a net declared open are dashed, a port and a group's
  frame dotted, in both renderers (§6.9). A
  highlighted non-consuming edge keeps its shape and only changes colour. A subnet
  place is one `Mrecord` box, its ports cells of it (§6.7).
- The examples live in `petritype/examples/` as cargo examples with
  `test = true`, so `cargo test` runs their assertions and `cargo run
  --example` shows them; they draw their nets with `petritype-dot`, which
  makes it a path-only dev-dependency of `petritype`, a cycle cargo
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
then by version. First cut is layers (a) and (b) below (the resource, a
per-frame stepping system, and the event mirror) with a plain grid
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
deferred (bevy DESIGN §3.6); 213 tests with `http`. **2026-10-10:** inner views (bevy DESIGN §3.5): one
`RemoteNet` follows the outer net and any number of inner nets by `nested_id`,
shown in place (`Enter`, double-click), in a side panel (`P`), in windows of
their own and as a peek (`get_state {nested_id, token_limit: 0}`), with outer
firings that touch an inner net named and flashed inside; none over HTTP; the
example runner reports a step's firing as a one-piece `fired` after its inner
effects, which the viewer attributes; 232 tests with `http`.

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
- Controls and token styles are on the net (2026-10-16, §18): a legend lists
  `net.controls()` and sends `net.press(name, value)`; a token's colour is
  `net.style(place, token)`, a place's fill rule `PlaceInfo::style`.

## 16. Rust features used, and why

| problem | feature |
|---|---|
| heterogeneous typed places in one net | `Box<dyn Any + Send + Sync>` storage + `TypeId`; typed handle `PlaceRef<T>` with `PhantomData<fn() -> T>` |
| per-type helpers without trait objects on tokens | monomorphised generic fns coerced to `fn` pointers, stored in `PlaceMeta` |
| bodies of any arity in one list | trait over tuples, `macro_rules!` impls for arity 0..=16 (as Bevy `IntoSystem`, axum handlers, which stop there too) |
| `&mut T` parameters from `MutateEdge` | generic associated type `type Arg<'a>` on the edge-kind trait |
| readable mismatch errors | `#[diagnostic::on_unimplemented]` |
| body may return plain or `Result` | trait `IntoOutcome<Outputs, Marker>` over the return type; marker tags (`Positional`, `Failing<M>`, later `Routed` and `Named`) keep the impls from overlapping, as Bevy's `IntoSystem<Marker>` |
| a return position, or a named output field, may hold one / maybe one / many | trait `IntoTokens<T>` for `T`, `Option<T>`, `Vec<T>`; `Accepts<V>` for `PlaceRef<T>` over it, for a field (slice 23) |
| routing by payload type without runtime reflection | trait `RoutedOutput` (`describe` lists `TypeId`s, `emit` pushes into a `RouteSink`); `#[derive(Routed)]` proc macro names `petritype::routing`, so that module is public; table resolved from types at `transition()`, problems reported at `build()`. Cost of marker-based form selection: a return type that fits neither form is blamed on `IntoOutcome`, whose notes therefore describe both forms |
| one input position binding several edges | `EdgeSealed::describe` appends to a `Vec<InputSpec>` and `extract` takes from `ErasedCursor`, an `IterMut` over the erased inputs whose items borrow the inputs rather than the cursor; everything after the builder sees the flat list (slice 23) |
| a function's inputs and outputs bound by field name, its structs free of places and edge kinds | `#[derive(Inputs)]` / `#[derive(Outputs)]` generate a struct of edges with one type parameter per field, bounded by `Reads<V>` / `Accepts<V>`, the bound spanned at the field so that a wrong kind is reported against it (slice 23) |
| a `&'a mut T` field filled for one call | `Reads<&'a mut T>` for any `'a`, and the hidden `Extract<'a, V>` asked for `for<'a>`, which ties the field's lifetime to the cursor's; stable, no `unsafe` (slice 23) |
| derived impls of sealed engine traits from another crate | a hidden public module, `petritype::binding`, re-exporting `EdgeSealed` and `PlaceTupleSealed`: the seal's one opening, as `petritype::routing` is public for `Routed`. Concrete impls rather than blankets over a hidden trait, which would have rustc blame that trait for every non-edge in an input tuple (slice 23) |
| a named return beside the positional and routed ones | marker `Named`; the routed impl asks `Outputs: TupleOfPlaces` (sealed, tuples only), so a type deriving both `Routed` and `Outputs` is never ambiguous (slice 23) |
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
  being distributed loses the body for good. Panics no longer strand
  (caught since slice 7), and the drivers (slice 9) never drop one: they
  drain everything airborne before returning, on the error path too, and
  since 2026-11-28 on a panic of the net's own code on the driver's thread
  as well; a launch whose stage one panics puts the body back, and since
  0.4.0 takes none of its tokens (§13). **Done 0.4.0**: a rejected
  `distribute` (`HeldByBody`, `ForeignFiring`, `NotInFlight`) hands the
  `Completed` back in its `DistributeError` (§6.2). It changed
  `distribute`'s error type rather than adding the `try_distribute`
  decided 2026-09-20: one method keeps one shape, and `?` into a
  `NetError` reads as before.
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
13. `petritype-bevy`, first cut done 2026-09-20 in its own repository
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
    7 (a script that gave way on 2026-10-10 to a city that runs on its own,
    below), and `tests/slice15.rs`. Phase 2 (serialising nested tokens,
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
21. Engine policies and the protocol's wording gaps (§6.1, §6.6, §13,
    §19.3.13), 2026-10-12, decisions A and B of the phase, shared with the
    Python track: `TieBreak::RoundRobin` with its cursor and `Net::choose`;
    `DeepPolicy::{OuterFirst, Interleaved}`, set with
    `NetBuilder::deep_policy` and `Net::set_deep_policy`, read with
    `Net::deep_policy`, with its own cursor, and `Net::step_deep_staged`
    with `StagedStep` for a driver that fires in stages; `step_deep` and both
    drivers honour it. Defaults unchanged, neither cursor saved. The runner:
    `inject` into an inner net answers with its address (A2), `null` counts
    as absent for every optional field (A5), an inner net that an outer
    firing produces has its fireable set taken at the landing (A7), and
    `step` and a running net follow the loaded net's policies;
    `tests/slice21.rs`. The deep policy, and slice 15's passive mark, gave
    way on 2026-11-21 to the stepping rule per place (§6.6;
    `tests/stepping.rs`).
22. Edges on empty places (§4.4, §6.1, §12, §19.3.14), 2026-10-14, R1 to R6
    of `dev-docs/EDGES_ON_EMPTY_PLACES.md`, shared with the Python track:
    `DrainEdge` and `SurveyEdge`, body parameter `Vec<T>`, `EdgeKind::
    {Drain, Survey}` and `EdgeKind::requires_presence`; the fireability
    scan skips the presence check for them; a drain launches as a take-all
    (and restores as one), a survey clones every token at launch and moves
    nothing; a survey of a no-clone place is `SnapshotOfNoClonePlace`; the
    source rule and the acyclicity walk count only the edges that always
    take a token, so a drain alone is a source. `petritype-dot` draws the
    two with a hollow arrowhead. The example `shop`, the protocol's
    conformance net of that name, runs the proposal's script to its table;
    the runner sends `"drain"` and `"survey"`, carries `shop` (the core
    example, included by path), and the shared `07_empty_place_edges.json`
    replays unchanged. `tests/slice22.rs`, two `trybuild` cases. The viewer
    (R7) is petritype-bevy's.
23. Binding by name (§4.4, §4.5, §5, §16), 2026-10-15, R2 to R9 of
    `dev-docs/BINDING_BY_NAME.md` (R1, the tuple cap, is a branch of its
    own). An input position may bind several edges: `EdgeSealed` describes
    into a vec and extracts from a cursor, and nothing after the builder
    changed. `#[derive(Inputs)]` generates a struct of edges bounded by
    `Reads<V>`, one input position whose edges are its fields' in field
    order, `&'a mut T` fields included; `#[derive(Outputs)]` generates a
    struct of outputs bounded by `Accepts<V>`, the whole of the outputs, and
    returning the struct is the named form, marker `Named`, positional for
    the read API and the wire. The engine traits the derives implement are
    named through the hidden `petritype::binding`; the routed form asks for a
    tuple of outputs, so `Routed` and `Outputs` on one struct never clash. A
    lone struct without the tuple (R6) was tried and dropped: it made the
    diagnostics of every input tuple worse. The example `pricing` wires one
    library function into a shop and a wholesaler, with the mixed form;
    `tests/binding.rs` holds the rest and the till of
    `TRANSITION_ARITY.md` §2 by name, fourteen inputs and thirteen outputs;
    eight `trybuild` cases. No wire, runner, protocol or Python change.

**2026-10-10: the `transit` example runs its own journey** (no slice: an
example, a conformance net and a transcript changed, the crates' API did not).
The Bevy viewer surfaced it: stepped there with nothing switched off, the city
of slices 15 to 17 boarded its three passengers at `Depot` and alighted them
again in turn, for ever, and no cabin fired once. `step_deep` fires the outer
net first and an inner net only when the outer has nothing; the default
tie-break took the first declared of `Board Depot` and `Alight Depot`, and the
two kept each other fireable, the passengers going between the platform and
the bus's `Door`. The script had hidden it by holding `Alight`, `Depart` and
`Arrive` back and firing them by hand, and an example is meant to show a net
that runs its process correctly on its own, not a net and a script that makes
it work. So a vehicle's state became what the Petri net idiom makes it, where
tokens are in its cabin: every template has two places, `Doors Open` and
`Doors Closed`, holding one token per door (a ferry has two), and with the
announcement they are the vehicle's phase. The city's guards count them, and
`Close Doors` and `Arrive` move the doors from one to the other, `Arrive` in
its own body because a cabin that opened its own doors would leave `Depart`
fireable until the cabin's turn came. (The first cut, the same day, kept the
phase as a word in one token of a single place; the viewer drew that place
holding one token, `Close Doors` a loop on it, and showed nothing of the
phase. As two places the phase is where the tokens are, each change of phase
is an arrow, and more doors are more tokens.) Alighted passengers go to a new
place `Arrived` rather than back to a platform, where they would board again;
vehicles follow a fixed loop, the parameter `destination` now an override
(`null` as built); Dara, Eve, Finn and Gwen wait at `Central` and `Harbour`.
From the seed all seven arrive in 45 firings of plain `step_deep`, under
either tie-break; `City::hold_back`, `City::order` and `City::run_to_quiet`
are gone (§6.6). PROTOCOL.md §11 changed with it, the Python track recorded
the shared `05_nested_nets.json` again with nothing held back, and it replays
here after one runner fix: an injected token's inner nets have their fireable
sets taken silently at the inject (§19.3.11). Recorded again for the doors as
two places, it replays here with no runner change.

**2026-10-16: the shop reads, the traffic light drains** (no slice: examples,
conformance nets and transcripts changed, the crates' API did not). Decided by
Oleg: the shop of slice 22 mixed two mechanics, and draining promotions read
badly. The shop keeps its survey alone: `Price Order` reads every promotion
running and leaves them running, and `Day End`, `Expired` and `Close Day` are
gone. A new example, the traffic light, shows the drain, with a plain snapshot
beside it as the simpler kind of read: `Go` takes every car waiting while the
light is green and reads the light; `Change` moves the light on, for ever
(the shape as it ended the day, below). Both are conformance nets of
PROTOCOL.md §11, and the runner's example carries `traffic-light` beside
`shop`. The Python track recorded `07_empty_place_edges.json` again and
`08_traffic_light.json` new; both replay here with no runner change. The
pricing example's wholesaler surveys its promotions rather than draining them.
The drain tests of `tests/slice22.rs` and `petritype-dot` moved from the
shop's `Close Day` onto the traffic light; the drain edge itself is unchanged.

**2026-10-16, later: the traffic light is one place.** Decided by Oleg the
same day: three places for the light (`Red`, `Green`, `Amber`, a token
`"light"` standing in one) and four transitions would make a junction of
several lights unreadable. `Light` holds one token whose value is the colour,
and `Change` takes from and writes back to it, which needed no core change.
`08_traffic_light.json` was recorded again and replays with no runner change.

**2026-10-16, later still: two transitions.** Oleg expected two, one for the
light and one for the cars: the shape went three places → one place → two
transitions, `Go` (guarded on green and someone waiting, a drain beside a
read) and `Change` (unguarded); `08_traffic_light.json` was recorded again and
replays with no runner change.

**2026-10-16, last: net-declared controls and token styles.** Decided with
Oleg the same day: a net declares, as data, the buttons it wants and how its
tokens look; a viewer renders that with widgets it already has and never
names a net, and a press runs code written beside the net, in the net's
language. The core gained two public modules. `style`: `Rgb` (written and
read as `#rrggbb`), `Style { colour }` (`#[non_exhaustive]`, `Default`; a
glyph and an image join it later), the trait `TokenStyle`, and `PlaceStyle {
fill: Fill::Token }`, a rule the viewer evaluates (the place filled with the
colour of its one token). `NetBuilder::style(place, f)` keeps `f` in the
place's vtable as `keys` keeps its extractor, an `Arc<dyn Fn(&dyn Any) ->
Style>`; `styled(place)` is that with `T::style`, the one line that opts a
place in, since places have no trait bound and Rust no specialisation to
default one; `place_style` gives the place its rule. `Net::style(place,
token)` reads one token's style, `None` for a place without one, and
`PlaceInfo::style` and `TopologyPlace::style` carry the place's, the latter
`{"fill": "token"}` under `serde`. `control`: a `Control` is a name, a label,
a `ControlKind` (button, toggle, slider, select, text, each with its
settings), an optional `Node` and `Action` (inject, fire, enable) for the
viewer's lighting and hover, an initial `Value`, and a handler `Fn(&mut Net,
Value) -> Result<Pressed, String>`; there is a constructor per kind and one
per action, `Control::fire` answering `Pressed::Nothing("<transition> is not
fireable")` where `Net::fire` refuses. `NetBuilder::control` declares one,
`BuildError::DuplicateControlName` for a name used twice, and the net carries
them: `Net::controls` in declaration order, `Net::press(name, value)`, which
checks the value against the kind (`PressError::WrongKind` covers an option
not listed and a number outside a slider's bounds, so a runner only decodes
JSON), clones the handler's `Arc` and calls it with the net, `Refused` with
its message for an `Err`. Carrying them on the net is the one point where
Rust differs from Python's control map kept off it: the `Net` is what both a
runner's factory and the viewer's `NetRes` hand over. Every effect of a press
goes through the public API, so it is an ordinary event. The runner serves
the outer net's `topology.controls` (`node`, `action` and `config` only when
there are some, `config.value` only for an initial value) and
`places[].style`; `press {control, value?}` joins `METHODS` and
`OUTER_ONLY`, flushes the events as `inject` does, logs and counts each
firing as a step's, and answers `{status: "pressed" | "nothing", reason?,
marking_version}`, with the new code `unknown_control`, `invalid_params` for
a value the kind cannot take, and `internal` for a handler that refuses or
panics. And Rust now sends a token's `color` where its place's style gives
one: in token entries, in `added`, and fourth in `produced` (three elements
with neither colour nor keys, four with a colour, five with keys, the colour
or `null` fourth). The traffic light's token became the example's own
`Colour`, `"red"` on the wire still, styled with the lamps' hexes (red
`#ff6347`, green `#32cd32`, amber `#ffa500`) and filling `Light`, with two
controls, `car` (`+ Car`, a text box on `Waiting`) and `change` (`Change`, a
button firing `Change`). `Control::inject`'s `make` is handed the value and
the net as it stands, `Fn(Value, &Net) -> T`, as Python's `make(value,
graph)` may be, so `car` is an inject given a text box whose `make` names an
untyped car `Car N` from the marking. The Python track recorded
`08_traffic_light.json` again with six `press` exchanges at the end; it
replays here once the runner knew `press`. `tests/controls.rs`.

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
  the server's own docs, which are not yet public.
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
| edges an empty place satisfies, `kind: "drain"` / `"survey"` (§5.1) | `EdgeKind::{Drain, Survey}` (slice 22, §4.4, §19.3.14), no `filter` |
| `topology.places[].nested`, `stepping` (§5.1, §12.2, §12.3) | `TopologyPlace::nested`, written only when true; `TopologyPlace::stepping` as `protocol::Stepping`, written for every place that holds nets, the default included (protocol version 2, §19.3.16; version 1 had `passive`, written when the rule was `Stepping::Passive`) |
| `path`, `nested_id` (§12.1) | a path is a list of steps (`protocol::PathStep`, since protocol version 2), read by `protocol::parse_path` and resolved one step at a time through `net.nested(place, token)`; a nested id is the inner net's `NetId` printed with its `Display`, read back as a `u64` in that spelling alone (since 0.4.0; compared as text until then) and found by walking every nested place at every depth; mutable reach (`inject`) through `nested_scope` at each level (§19.3.15) |
| a token's `nets` (§12.2) | `net.nested(place, token)`: each net's name, `id()`, its path (the path of the net the token is in and one step more), `template()`, and the sum of its own places' counts, ports included and the places it has for its subnets' ports not (`translate::own_count`, §19.3.17) |
| a subnet place, its `ports`, `subnet` and `port` (§5.1, §12.7) | `TopologyPlace::subnet`; its `type_name` is `net.subnet(place)?.template()`, or `""`; its `ports` are the subnet's own places with `TopologyPlace::port`; the places the net has for them (`TopologyPlace::port_of`, its windows) are not listed, and an edge on one names `place` and `port`; `port: true` only in a subnet's own topology (§19.3.17) |
| `topology.groups`, a place's, subnet place's and transition's `group` (§5.1) | `Topology::groups`, `TopologyPlace::group` and `TopologyTransition::group` of the net read, each written only when set; a port in a subnet place's `ports` carries none (§6.8, §19.3.18) |
| a port named by the pair (§7.1, §8, §12.7, §13.2) | `protocol::PlaceName`, from a window's `port_of` |
| a subnet place's entry `{subnet, ports}` (§12.7) | `net.subnet(place)`: its id, path, template and own count, and each port read in the subnet, under the subnet's projection |
| `subnet_marking_versions`, `outer_marking_version` (§12.7) | the runner's, from the places a core event took, put or restored, or a token event's place (§19.3.17) |
| `nested` notification (§12.4) | `Event::Nested` and `Event::Subnet`, unwrapped to the event the innermost net logged: one wrapper with the full path, the inner notification built with that net's names and version |
| inner `marking_version` (§12.3) | the runner's counter per inner net, one per state notification of that net; the outer one does not move |
| `step` or a running net, inside (§12.3) | `step_deep_staged()` → `StagedStep::Inside`, inline: `nested` around `fired` (slice 21; `select()` and then `step_inside()` before) |
| `topology.places[].key_names`, a token's `keys` (§13.1) | `TopologyPlace::key_names`, written only when non-empty; `Net::keys`, read when the line is written (slice 20, §19.3.12) |
| a `produced` entry's fourth and fifth elements (§7.1) | `protocol::Produced`: three elements, or five with `null` for the colour when the place declares keys; since 2026-10-16 the colour `Net::style` gives is fourth, so four elements without keys, and the colour rather than `null` with them |
| a request's filter (§13.2) | `protocol::Filter::parse`, then place names resolved to the named net's ids: a `Filter` with `Clause::{In, IsParam}` |
| `query`, `distinct` (§13.3) | `Net::query_ids` (slice 20), rendering only the matches written; `Net::distinct_values` |
| `set_filter`, `matching`, `hidden` (§13.4) | a `Projection` per net, the runner's (§19.3.12): `Net::query_ids` for reads, `Net::matches` for each token an event names, a set of what was shown for the tokens that are gone |
| `transitions[name]` | `is_enabled`, `is_in_flight`, membership of `fireable()`, `priority_of` |
| `marking` | `tokens(place)` ids; the wire `label` is `debug(place, token)` cut by the runner at the protocol's 1000 characters, whatever the net's own `label_limit` |
| token id `"17"` | `TokenId` printed with its `Display` (the bare number) |
| `progress.step_count` | `fired_count()` of the net read: per net, since protocol version 2 (§5.4, §19.3.15); from slice 20 until then the outer net's also counted the firings the runner made inside its tokens |
| `progress.last_fired`, `fired_counts`, `last_progress_at` | kept by the runner from the events, per net, each under the net's own transition names: the core dropped them on purpose (§2) and does not get them back |
| `marking_version` | the runner's own counter, one per state notification. Not `state_counter`, which starts at 0 again when `reset` rebuilds the net |
| `generation` | the runner's counter of loads and resets; a rebuilt `Net` numbers its tokens from the start again, which is why the protocol has generations at all |
| state notifications | `Event` variants, ids resolved to names through the read API, produced tokens labelled with `label()`; `ParamChanged { type_name }` reported under the parameter's wire name from `serializable_param` when it has one |
| `fireable` notification | the runner compares `net.fireable()` after each batch of events |
| `step`, `start` | `step_deep_staged()` → `StagedStep::Chosen(t)` in the net's deep policy's turn (slice 21; `select()` before), `launch`, the body on a thread of its own, `distribute`; one firing at a time, so `Launched` then `Distributed` |
| `inject {place, token}` | `apply_json` of `JsonCommand::Extend` with one token; the new id is read off the `Added` event. Needs a `serializable` place, else `not_serializable`. Into an inner net through `nested_scope` at each level, answered with that net's version, `path` and `nested_id` (slice 21) |
| `enable`, `disable` | `net.enable(t)`, `net.disable(t)` (`Toggled`) |
| `set_param {param, value}` | `JsonCommand::SetParam`; `transition` ignored; `invalid_params` for a name the net has no parameter under (slice 17; `unsupported` before), `unsupported` for a parameter with no wire name |
| `get_token {id}` | `net.json()` gives `kind: "json"`; otherwise, or for a token with no JSON form (a NaN or an infinity in it), `debug()` gives `kind: "text"`, untruncated |
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

**Designed and built 2026-10-03**; as-built notes in §19.3.10, nested nets in §19.3.11, keys in §19.3.12, the protocol's wording gaps and the engine policies in §19.3.13, protocol version 2 in §19.3.15 and §19.3.16, subnet places in §19.3.17, groups in §19.3.18, four more shared transcripts and `--net` in §19.3.19. A library crate in this
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
   threads, takes the time of day only from the `now` it is handed (its one
   other clock is a stopwatch round a body it runs inline), and writes
   nothing, which is
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
   reports end of file. The loop holds a tick back until nothing else
   waits in the channel (§19.3.5).

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
| `progress: {step_count, last_fired, fired_counts, last_progress_at}` | `progress`, the outer net's own; an inner net's is in its `Inner` (per net since protocol version 2, §19.3.15) |
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
`color` was not either until 2026-10-16, when a place's style came to give
one (`Net::style`, written `#rrggbb`, §18). What a token has besides, since
slice 18, is keys (§8.3): named values its place derives from it, which the
protocol's §13 puts on a token entry as `keys: {name: value}` (from
`Net::keys`) and on a place as `key_names` (from
`TopologyPlace::key_names`), and the runner writes them since slice 20
(§19.3.12). A transition's `detail` is
`{params, detachable}`; `priority` is `priority_of` behind `catch_unwind`
and omitted if the closure panicked, returned NaN or an infinity.

#### 19.3.5 The firing cycle

```
step / start tick:   select() -> launch(t)          (since slice 21: step_deep_staged(), §19.3.13)
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
  normally and is reported normally. A `stop` that comes in while a body
  runs inline is read when that body lands, before the next tick, so it is
  answered after that one body and the net fires nothing more first (since
  2026-11-22, below).
- `heartbeat` (default every 30 s) is written by a thread of its own, not
  by `Owner`, so it keeps going while an inline body has the owner blocked,
  which is exactly when a host most wants it. Its `marking_version` is
  stamped by the writer, since only the writer knows which lines went out
  before it (since 2026-11-28). The owner records the
  airborne or inline transition and its start time in a shared status cell
  before any launch and clears it on landing. A firing inside an inner net
  runs inside the deep step, so the owner steps with
  `step_deep_staged_with` (§6.6) and records that firing from the observer,
  just before its body starts: the transition's name in its own net, the
  handle's `now`, and the inner net's address, its wire `path` and
  `nested_id`, built from the `InnerBody` and the records of the nets on
  the way (`inner_address`) as a `step` result's are, without reading the
  net the step holds; it clears the cell when the step returns, the body
  having fired or failed (since 2026-11-22; before, the cell named no inner
  body). The cell is `Option<(String, SystemTime, Option<InnerAddress>)>`.
  The heartbeat thread reads it and sends `{step_started_at,
  executing_transition}`, with `path` and `nested_id` for a body in an
  inner net, or two nulls; hosted mode sends two nulls always. No
  heartbeat names a firing a body makes inside its own tokens' nets (it
  names that body), nor one a press makes. The heartbeat's `running` is
  read from a gauge the owner mirrors after every `handle` and, since
  2026-11-22, just before a body runs inline (`mirror_running`), so it is
  true during the first body of a run, which runs inside the `start` that
  began it; the version gauge is left as the last `handle` mirrored it,
  the version of the lines written so far.
- `Tick` is the ticker's input to `Owner` (same period) and only drives
  the run loop: a `Continuous` net re-checks `select()` on it. The
  heartbeat thread sends one every period, busy owner or not, so the loop
  does not hand ticks to the owner as they come (`next_message`): a tick
  sets a flag, and the owner gets `Tick` only when the channel is empty.
  The ticks that came in while the owner was busy count as one, and a
  request, a landing or end of file that came in during an inline body is
  handled when that body lands, before any tick fires the net again. The
  tick `wants_tick` asks for sets the same flag. A run with nothing waiting
  fires one inline body per `handle` back to back, as it did when each tick
  was handed over in turn. Before 2026-11-22 each tick queued ahead of a
  `stop` started another inline firing, and a `stop` sent during a
  one-second body with a 200 ms period was answered after three firings.

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
colours (until 2026-10-16, when a place's style came to give them, §18) and
position hints (nothing in the core produces them); `get_state
{token_limit}` beyond the default 200 and `get_place_tokens` paging are in,
but no complete-snapshot cache, because reads are answered from the live net.

#### 19.3.10 As built, 2026-10-03

Where the implementation settled something the design left open, or chose
differently, with the reason:

- **Inline firings** still log `Launched` then `Distributed` in the core;
  the runner folds the pair into one `fired`, whose `duration_ms` is the
  time the body took, measured by the owner round the call that ran it
  (it was `0` before 0.2.0, since the owner has one `now` for the whole
  `handle`).
- **`fireable` is not compared after a detached launch**, only after the
  landing, so no `fireable` line falls between `launched` and
  `distributed`. At `load` and `reset` the set is recorded silently; the
  `reloaded` already means "refetch".
- **Inline runs stay interruptible.** A running net whose next firing is
  inline fires one per `handle` and `Owner::wants_tick()` asks the loop for
  a `Tick`, which it hands over once nothing else waits, so a `stop` between
  firings is heard before the next one (§19.3.5).
- **Answered during a body**: the five reads and `stop`, plus `describe`
  and `shutdown`.
- **Heartbeat** carries `marking_version` and `running` like every
  notification, read from atomics the owner mirrors (`running` also before
  an inline body, §19.3.5); sent only while a net is loaded. Period from `PETRITYPE_RUNNER_HEARTBEAT_MS` when set, for
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
copy in `tests/protocol/`. It was recorded again on 2026-10-10 for the net
that runs its own journey (§6.6, §18), with nothing held back, and replays
unchanged too, after one runner fix: an injected token's inner nets get
their records at the inject (below). Recorded again the same day for the
doors as two places, `Doors Open` and `Doors Closed`, it replays with no
runner change.

- **Addressing** (§12.1). Every read and `inject` takes `path` or
  `nested_id`; both is `invalid_params`, neither the outer net. A path is
  walked level by level through `Net::nested`. Until protocol version 2
  (2026-11-21, §19.3.15) it was text: the entry's name, then per level the
  place, `#` and the token id, and the net's name in the token, joined with
  `/`; since then it is a list of steps, `[]` the outer net. A
  nested id is the inner net's `NetId` as its `Display` prints it, read
  back as a `u64` in that spelling alone (since 0.4.0), so `"07"` names no
  net, and found by walking every nested place, passive ones included, at
  every depth. No
  match is `unknown_net`, and the message names the step that failed (by
  its index from 0) or the id. `step`,
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
  walking the marking, at an `inject` for the nets inside the token it
  brings (since 2026-10-10, as the Python owner does: the transcript's
  injected shuttle announces the empty set its first firing leaves), or
  when a request names the net (in each case the fireable set is taken
  silently, as `load` takes the outer one), or when one of its events
  arrives first (its fireable set empty, so a non-empty one is announced).
  Dropped eagerly, at the first landing or inner firing after which its id
  no longer resolves: one walk of the nested places per landing, never
  while a body holds a token, so a vehicle in transit keeps its record and
  its version. Since 2026-11-28 the walk that makes records for new nets
  and drops those of nets gone is one walk, and a step, a landing or an
  inject takes it only when its events, at some depth, put, took, added,
  removed or changed a token of a place that holds nets; a firing that
  moves plain tokens inside an inner net walks nothing. A press and the
  host program's changes walk every time, since their code can put a net
  in place of another through `nested_scope` and log nothing of it. An
  inner net whose fireable set is compared is looked for by the path its
  record last saw it at, and by a walk only when that path no longer leads
  to it. `load` and `reset` drop them all.
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
  `step_deep`'s order with `Net::step_inside()` (§6.6; since slice 21 both
  through `Net::step_deep_staged`, in the net's deep policy's order,
  §19.3.13), inline: the inner
  net logs `Fired`, and the wire says `nested` around `fired`, one of the
  two shapes §12.4 allows; a harness folds the other into it. The `step`
  result is `{fired: true, transition, path, nested_id, marking_version:
  <outer>, running}`, with `failed: true` after a `nested` `failed`;
  `fired: false` means nothing fireable anywhere outside passive places. A
  running net does the same, one inner firing per `handle` with
  `wants_tick` asking for the next, so `execution_stopped {reason:
  "no_fireable_transitions"}` and `idle` mean deep quiescence, and an inner
  failure stops a run as an outer one does. An inner net's `progress`
  counts its own firings (its `step_count` is its `fired_count`), and so
  does the outer net's: progress is per net (PROTOCOL.md §5.4, since
  protocol version 2, §19.3.15). From slice 20 until then the outer net's
  also counted each inner firing the runner made, under the inner net's
  text path without the net's name and then the transition, as the Python
  runner did. A failed inner firing counts in neither.
- **Inner `fireable`** (§7.2, §12.4). After a batch, after the outer
  `fireable`, each inner net that had events (in that batch, or since the
  last compare, across a launch) is compared with the last set sent for it,
  and `nested` around `fireable` goes out when it differs. It moves no
  version. As `05_nested_nets.json` has it: the cabin's `["Sit"]` after the
  first board, nothing at load.
- **Reads** (§12.2, §12.3). `topology.places[]` carry `nested` and
  `passive` when true (since protocol version 2 a place that holds nets
  carries `stepping` in place of `passive`, §19.3.16). Every token entry (`get_state`, `get_place_tokens`,
  `get_token`) carries `nets: [{name, nested_id, path, template?, count}]`
  when its token holds nets; `produced`, `added` and `mutated` carry none.
  An inner `get_state` is that net's state: `net` its name in its token,
  `generation` 1, its own version, topology, transitions, marking and
  progress, plus `path`, `nested_id` and `template`; `get_place_tokens` and
  `get_token` echo `path` and `nested_id`. `inject` into an inner net
  answers with that net's version and no address, as the shared transcript
  has it (until 2026-10-12: now with `path` and `nested_id` too, §19.3.13). While a body is airborne, a read whose address does not resolve
  waits for the landing (the net may be inside the token the body took) and
  is answered then, or `unknown_net`; every other inner net is read at
  once. Inner bodies run inline on the owner thread, so nothing is read
  during one.
- **The log** (§8) is one ring. An inner firing's entry, written when its
  `Fired`, `Distributed` or `Failed` is flushed (`started_at` the handle's
  `now`, `duration_ms` the time its body took when the runner ran it inline
  and 0 when a body or the host program made the firing), carries `path` and
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
  (§4.5), so that `slot` is written for positional outputs alone:
  `transit`'s `Arrive` distributes to one of four stations, and the shared
  transcript has no slots on its edges.
- **The harness** (`tests/transcripts.rs`) replays a transcript over a
  conformance net against that net alone, wherever the file is. It gained
  the `<path>` mask, captures substituted inside a longer string when
  sending (a text path's token, both gone with protocol version 2,
  §19.3.15), and two folds: a `nested`
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
  entry (in `get_state`, `get_place_tokens` and `query`) carries `keys`
  (`protocol::Keys`, an object in declaration order) from `Net::keys` when
  its place declares keys, and so do `added` and `mutated`, read when the
  line is written, as labels are, so a `mutated` has the keys after the
  change. `get_token`'s answer is not a token entry and has none, as the
  transcript has it. A `produced` entry is `protocol::Produced`: three
  elements, or five when its place declares keys, `null` in the colour's
  place, since a Rust token had no colour (and so never four). Since
  2026-10-16 a token whose place's style gives it a colour has it fourth:
  four elements without keys, and the colour in place of `null` with them
  (§18).
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
  from the key index, so a reader renders (label, keys, nets, JSON) only
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
  token; a `mutated` by where it crosses: kept before and after,
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
- **The outer progress** counted the runner's inner firings (§19.3.11),
  for parity with the Python runner, until protocol version 2 made
  progress per net in both (§19.3.15).
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
  this runner's inline firing tells `hidden`, `fired`. (Since the net's
  second edition, §8.3, the transcript shows the same of `Decline Forest`,
  and a keyed mutate with its produced copy in `Spread Savanna Wetland`.)
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
  so it keeps the token's view as it was. (2026-10-12: the phase's
  decisions for PROTOCOL.md's next revision settle the first and the
  fourth as this runner had them, A6 and A5; §19.3.13.)

#### 19.3.13 The protocol's wording gaps and the engine policies, 2026-10-12

Slice 21: the runner's half of the phase's decisions, A (eight wording gaps
in PROTOCOL.md, which the Python track writes into its next revision, cited
here by the sections they amend) and B (the engine policies, §6.1 and
§6.6), in the same words as the Python runner.

- **A2, `inject` into an inner net answers with its address** (§12.3,
  §12.5): `{status, id, marking_version, path, nested_id}`, the version the
  inner net's, as every result about an inner net carries `path` and
  `nested_id`. The path is the one the address resolved to, whichever of
  the two the request gave.
- **A3, `step` is outer-only** (§12.3): it was already refused with
  `invalid_params`, as `enable`, `disable`, `set_param`, `start`, `stop`
  and `reset` are (§19.3.11); a test now sends it a `path` as well as a
  `nested_id`.
- **A4, an undeclared `set_param` name is `invalid_params`** (§6.4): so
  since slice 17, and tested.
- **A5, `null` for an optional request field counts as absence** (§2,
  §13.3), except `inject.token` and `set_param.value`, which are values
  and may be `null`: one helper, `optional`, reads every optional field (`path`,
  `nested_id`, `filter`, `token_limit`, `offset`, `limit`, `after`,
  `places`, and `load`'s `net` and `params`), so `query {filter: null}` is
  `query {}`, the connection's filter if it set one, and `step {path:
  null}` is a `step`, where the outer-only check used to see the key and
  refuse it. A required field is read directly: `inject`'s `token: null`
  is a token for a place whose type takes it, and `set_param`'s `value:
  null` a value, which `transit`'s `destination` takes. A request's
  `params: null` is `{}` in `Owner` as it already was off the wire. Tests
  for `query`, `get_state` and the rest.
- **A6, `failed.consumed` is projected like `launched.consumed`** (§13.4:
  "`launched.consumed`, `failed.consumed`, `distributed.produced` and
  `fired`'s lists hold only matching tokens (a `failed` lists what its
  `launched` listed)"): so since slice 20, and tested (§19.3.12).
- **A7, when an inner net's baseline fireable set is taken** (§7.2,
  §12.3: at `load` for seeded inner nets, at the `inject` that carried the
  token, at the landing of the outer firing that produced the token, and
  `reset` and `load` retake them): at `load`, `reset` and an `inject` it
  already was; at the landing of the outer firing that produced its token
  it was not. A net a body
  built arrived through its lifted events, its record made with an empty
  set, and its first `fireable` announced the baseline. `land`, and the
  report of an inner firing, now call `Loaded::know_new` before the flush
  (since 2026-11-28 only when the flush's events touch a place that holds
  nets), so the set is taken there, silently; the test's `hatchery` has a body
  build a truck and its bin. No transcript has such a firing.
- **A8, a parameter value decodes by its declared type** (§6.4, §11):
  `set_param` goes through the decoder registered for the wire name
  (§19.3.12), so `transit`'s `Destination(Option<Location>)` takes a
  station or `null` in either order and refuses a number; a test sends
  both orders.
- **The policies are honoured** (B1, B2). `step` and a running net step
  through `Net::step_deep_staged`: `StagedStep::Chosen(t)` is the outer
  net's turn, launched in stages as before, and `StagedStep::Inside` an
  inner firing made inline and reported as before. Under the default
  policy that is exactly `select()` and then `step_inside()`, which is why
  the transcripts replay; under `Interleaved` the outer net's staged
  launch takes its turn in the cycle. The choice is taken as `Net::choose`
  takes it, so a round-robin net alternates over the wire too (through
  `select()` it would have chosen the same transition every time). A reset
  rebuilds the net, so both cursors start afresh, as a load does (Python's
  owner calls `forget_cursors()`). PROTOCOL.md §12.3's words are quoted in
  §6.6: the policy is the runtime's own, and nothing on the wire changes.
  Tests: `rivals` (round
  robin) and `carousel` (an always-live `Spin` and a worker, the policy a
  load parameter) stepped and run under each policy, and a reset.
- **`transit` takes `destination` as a load parameter.** A8's consequence
  on the Python side, pinned by the shared transcript: the Python factory took
  `destination: str | None = None` as a keyword for A8, so its `describe`
  lists it, and `05_nested_nets.json`, recorded again for A2 (its inject of
  `Hal` into the bus's cabin answers with the address now), pins
  `describe` in full (§11). The example binary's `transit` entry declares
  `destination` (`Option<Location>`, default `null`), a typed entry whose
  factory builds the city with it as the net parameter's value
  (`build_city_with`), so a number is `invalid_params` at `load` as at
  `set_param`.
- **Transcripts.** The re-recorded `05_nested_nets.json` replays here
  unchanged, and replaces the verbatim copy in `tests/protocol/`; our own 01
  lists `transit`'s new parameter; 02 to 04 and 06 are untouched, and every
  one replays. Against the sibling checkout's shared set the old 05 fails
  on exactly those two lines (`describe` and the inner inject) until the
  Python track's re-recorded file lands there; the set with the new 05
  (`PETRITYPE_PROTOCOL_TRANSCRIPTS`) replays in full.

#### 19.3.14 Edges on empty places on the wire, 2026-10-14

Slice 22, the runner's half of the edges-on-empty-places proposal (its R6;
§4.4 has the engine's half). PROTOCOL.md §5.1, revised by the Python track,
adds two values to `edges[].kind`: `"drain"`, an input edge that consumes
every token of its place, zero or more, and `"survey"`, one that reads
every token without consuming any, zero or more; neither holds its
transition back on an empty place, and neither carries `filter`. Nothing
else on the wire changes: `fired.consumed` (and `launched.consumed`) lists
what a drain took, none from an empty place, and nothing a survey read.

- **As this runtime implements it.** `protocol::EdgeKind` gained `Drain`
  and `Survey` (snake_case on the wire, as the core's `EdgeKind`), and
  `translate::edge_kind` maps the core's two kinds onto them; before this a
  kind the runner did not know went out without `kind`, which a reader
  draws as a plain edge (§5.1), and that is still what an older runner of
  ours would send. A survey's transition stays detachable (`detail.
  detachable: true`), so it is launched in stages like any other; a drain's
  tokens are in `launched.consumed` in the order they were taken.
- **`fireable`** (PROTOCOL.md §5.2, amended the same day: "every input
  place non-empty (but one read through a `drain` or `survey` edge, §5.1)")
  is `Net::fireable` as it always was, so the runner's `fireable`
  notifications and `transitions[].fireable` follow the new rule with no
  change of their own: an inject into `Promotions` alone announces nothing,
  and one into `Orders` makes `Price Order` fireable whatever `Promotions`
  holds.
- **The conformance net `shop`** (PROTOCOL.md §11): the example binary
  includes `petritype/examples/shop/net.rs` as it includes transit's and
  wildlife's, under the description the Python factory gives it ("Price
  orders with whatever promotions are running, and end them at the
  close"), manual hosting, no load parameters and no net parameters.
  `nets::shop_registry` carries it alone, for the transcripts over it.
- **Transcripts.** The Python track recorded `07_empty_place_edges.json`
  (the proposal's §2 script over the wire: `describe`, `load`, `get_state`
  with the two kinds in the topology, the script's injects and steps, a
  step with nothing fireable, a final `get_state`, and a `get_token` of
  each of the six output tokens, which is what pins the §6 values, labels
  being masked); its copy in `tests/protocol/` is byte for byte the shared
  file and replays with no runner change beyond the two kinds and the
  entry. Our own `01` lists `shop` in
  `describe`, which lists everything the example carries; 02 to 06 are
  untouched. A test drives a net of a take, a survey and a drain through
  `Owner` (`drain_and_survey_edges_go_out_with_their_kinds`).
- **2026-10-16.** The shop was split in two (§18): `shop` keeps the survey,
  under the description "Price orders with whatever promotions are
  running", and the drain went to a new conformance net, `traffic-light`
  ("Cars wait at a light that cycles red, green and amber, and all go when
  it turns green"), which the example binary includes from
  `petritype/examples/traffic_light/net.rs` and `nets::traffic_light_registry`
  carries alone. `07_empty_place_edges.json` was recorded again over the
  reduced shop and `08_traffic_light.json` is new; their copies here are
  byte for byte the shared files, and our own `01` lists both nets.
  `08_traffic_light.json` was recorded again the same day, for the light
  as one place and then for its two transitions (§18); its copy here is
  again the shared file each time, and it replays with no runner change.

#### 19.3.15 Protocol version 2: structured addresses, 2026-11-21

The runner's half of step 2 of the subnet places change
(`dev-docs/subnet-place-spec.md` §2.1, §4), on PROTOCOL.md version 2, which
the Python track wrote, built and recorded first.

- **The version.** `protocol::PROTOCOL_VERSION` is 2, in `describe` and in
  every `get_state`. A reader of version 2 does not read version 1 (§10).
- **A path is a list of steps** (§12.1): `protocol::PathStep::Token {
  place, token, net }` for a net carried by a token, written `{"place",
  "token", "net"}`, and `PathStep::Subnet { place }` for a subnet, written
  `{"place"}`; `[]` is the loaded net. It is the type of every `path` the
  runner writes: a token's `nets[]`, the results about an inner net
  (`get_state`, `get_place_tokens`, `get_token`, `inject`, `step`), the
  `nested` notification and the log entries.
- **Reading a request's path** is two questions, in this order.
  `protocol::parse_path` reads the shape: a list, each step an object with
  a string `place` and both or neither of `token` and `net`, each a
  string, a `null` being a value of the wrong type and any other key
  ignored. A wrong shape is `invalid_params`, its message naming the step
  by its index from 0, before anything is resolved. Then `resolve_path`
  takes one step at a time (`descend_step`): the place by its name
  (`Net::place_named`, so never the place a net has for a port), whose
  tokens must carry nets; the token by its id in its one canonical form
  (`translate::token_id`, so `"01"` names no token); the net by its name
  in the token. A step that names nothing is `unknown_net`, its message
  starting `step N of the path:` as the Python runner's does. A step of the
  subnet form takes a level into the subnet of a subnet place (§19.3.17);
  until then it was read and answered `unknown_net`.
- **Nothing is spelt as text.** `translate::level`, which joined a level
  with `/` and `#`, and the `resolve_path` that split one, are gone;
  `translate::step` builds a step from the names taken whole, so a place
  named `#Bay/1` or a net named `#a/b` is addressed as any other. The walk
  keeps its path as a stack of steps. Nothing in the runner checked names
  for `/` or `#`: the rule that such names cannot be spelt was the text
  form's, and went with it. A message that shows a path writes it as JSON.
- **Progress is per net** (§5.4). `ProgressKept::inside` and its
  `fired_inside` are gone: the outer net's `step_count` is its own
  `fired_count` and its `fired_counts` name its own transitions, and an
  inner firing is counted in its own net's progress alone, whether a
  `step`, a run or the program behind a hosted net made it.
- **Subnet places came to the wire later the same day** (§19.3.17). Until
  then `load` and `reset` refused a net that held a subnet place, or whose
  tokens held a net that did, as `unsupported`, and the walk skipped the
  places a net has for ports, as it still does.
- **The harness** follows §11: the `<path>` mask is gone, and a capture is
  substituted whole, a token id in a step of a path included, never inside
  a longer string.
- **Transcripts.** The shared `05` to `08` are copied verbatim from the
  Python track's recording for version 2 (each pins `protocol_version` 2,
  and `05` spells its paths as lists of steps) and replay with nothing
  more than the above. Of our own, `01` and `04` pin `protocol_version` 2
  in place of 1, and `02` and `03`, which carry no version, are unchanged.

#### 19.3.16 The stepping rule on the wire, 2026-11-21

The runner's half of step 3 of the subnet places change
(`dev-docs/subnet-place-spec.md` §2.3), on PROTOCOL.md §5.1 and §12.3 as the
Python track rewrote them. The rule per place is the core's (§6.6), and
the runner already stepped by it through `Net::step_deep_staged`; only the
wire changes.

- **`stepping` in the topology** (§5.1). `protocol::Place` has `stepping:
  Option<protocol::Stepping>`, the four protocol names, `inner_first`,
  `take_turns`, `outer_first` and `passive`, in place of `passive: bool`.
  `translate::topology` writes `TopologyPlace::stepping`, which the core
  gives exactly for a place that holds nets, so every nested place carries
  its rule, the default included, and a plain place carries none. A rule
  the runner does not know would go out as none. Nothing on the wire sets a
  rule.
- **While a body of a net is in flight** the nets of its outer-first places
  wait (§12.3). The core's deep step does that already, and the runner never
  takes a deep step while a firing of its own is airborne.
- **`transit`** states outer first on its places of vehicles (§6.6), so its
  topology now shows `stepping: "outer_first"` on them, and the shared
  `05_nested_nets.json`, recorded again with those fields, replays.
- **The conformance net `stepping-rules`** (§11): the outer places `Late`
  (outer first), `Ring` (take turns), `Early` (inner first) and `Shelf`
  (passive), each holding one net, then `Inbox` and `Outbox` with `Forward`
  between them; the team at `Early` is the second level, its `Helpers`
  taking turns with it. Each token of a place of nets is a bare `Net`, so a
  step of a path to one names its net `""`. The nets are built from two
  templates, `Worker` (its `jobs` read from its parameters) and `Team`, and
  every place of nets is serializable, so a token's `nets[]` names its
  template. The net is written in the example's `nets.rs`, as
  `parcel-distribution` is, since the core has no example of it, and
  `nets::stepping_rules_registry` carries it alone for the transcript over
  it. It is hosted `manual`, with no parameters.
- **Transcripts.** `10_stepping_rules.json` is new and `05_nested_nets.json`
  is replaced, both verbatim from the Python track; `06` to `08` did not
  change. Both replay with no runner change beyond the field and the net.
  Our own `01` lists `stepping-rules` in `describe`, which lists everything
  the example carries; none of our own showed `passive`.

#### 19.3.17 Subnet places on the wire, 2026-11-21

The runner's half of step 7 of the subnet places change
(`dev-docs/subnet-place-spec.md` §2.2, §4), on PROTOCOL.md §12.7 and its
mentions elsewhere, which the Python track wrote, built and recorded first.
The core's subnet places are §6.7; the runner serves them with no addition
to the core.

- **A subnet is one more inner net.** A level of a path is
  `Level::Token { place, token, index }` or `Level::Subnet { place }`: the
  walk visits a subnet place's subnet at its place's position (and never
  the places a net has for ports, its windows, whose tokens are reached in
  the subnet), `descend` reads it with `Net::subnet`, `descend_mut` reaches
  it through `Net::subnet_scope`, and `Lifted` unwraps `Event::Subnet` as it
  unwraps `Event::Nested`. So a subnet has an `Inner` record like a token's
  net: made by the walk at `load` and `reset`, its fireable baseline taken
  silently, and never pruned while its net lives, with its own version,
  topology, progress, projection and log entries (their chain holds its
  nested id). Its name, what its `get_state` calls it, is its subnet
  place's. Each record also keeps `around`, the net that holds its subnet
  place (`Around::Loaded`, or `Around::Inner` by nested id), which never
  changes, and each net, the loaded one in `Loaded`, keeps its subnet places
  as `Seat`s: the place, the subnet's nested id, and each window with the
  port's own id in the subnet. A subnet built without a template is served
  as any other.
- **Addresses** (§12.1). `descend_step` takes `{place}` to the subnet of a
  subnet place; `place X is not a subnet place` and `place X is a subnet
  place, whose step names the place alone` are its refusals, as the Python
  runner's, under `unknown_net`.
- **Topology** (§5.1, §12.7). `translate::topology` is given the net and
  whether it is a subnet. Windows are not listed. A subnet place is
  `{name, type_name, subnet: true, ports, stepping}`: its `type_name` the
  subnet's template or `""`, its `ports` the subnet's own places marked
  `port`, as the subnet's own topology has them (a nested port with its
  `stepping`), and no `serializable`, so `protocol::Place::serializable` is
  an `Option`. An edge on a window names the subnet place and the port
  (`protocol::Edge::port`). `port: true` is written in a subnet's own
  topology alone: a net loaded alone, or carried by a token, has no net
  around it, whatever its definition marks.
- **References** (§7.1, §8, §12.7, §13.2). `translate::Names` knows each
  window's subnet place, so a window is named by the pair: `port` beside
  `place` in `added`, `removed`, `mutated`, `hidden`, a page of tokens and a
  query's match, and `protocol::PlaceName` (`"Door"` or `{"place",
  "port"}`) in `consumed`, `produced`, the log's pairs and a filter's and a
  `distinct`'s `places`, read strictly by `PlaceName::parse`.
- **State** (§12.7). A subnet place's entry is `PlaceEntry::Subnet`,
  `{subnet: {nested_id, path, template?, count}, ports}`, each port read in
  the subnet, so that the paths of its tokens' nets go through it, under the
  subnet's projection. `count` is `translate::own_count`: the net's own
  places, its ports included, its windows and subnet places not. A token's
  `nets[].count` is that count too, which before it counted a window's
  tokens a second time.
- **Requests.** `resolve_place` reads `place` and `port` with §12.7's
  errors: a subnet place without `port` is `invalid_params`, a `port` its
  subnet place lacks or beside a place that is no subnet place
  `unknown_place`, and a `port` that is not a string `invalid_params`.
  `inject` by the pair decodes the token as the subnet's port does
  (`Command::from_json` on the subnet) and puts it into the window, so the
  net around tells it, under the subnet's id; addressed to the subnet it is
  an inject like any other. `get_token` searches the net's own places only,
  a port's tokens carrying the subnet's ids, and reads an id in its one
  canonical form (it took `"01"` and `"+1"`, where the Python runner and a
  path step do not). `query` and `distinct` cover the windows at their
  subnet place's position (the core's queries read through a window), and
  `query` without a `filter` takes each window's tokens by its subnet's
  projection and the net's own places by the net's.
- **One place, two views: the projections** (§12.7, §13.4). A net's own
  projection is kept to its own places (`projection::own_places`), and a
  `projection::Lens`, taken out of the records for one event and put back,
  routes each token an event names to the projection that covers its place:
  the net's own, or for a window its subnet's, under the port's own id. A
  lens's `landed` clears every projection it holds, since one firing happens
  at a time across the tree.
- **Versions** (§4, §12.7). `changed_places` reads what a core event took,
  put, or put back when it restored, or a token event's place; an inline
  firing's one `fired` counts its `launched`'s places too. `Loaded::cross`
  then raises, once each, the version of each subnet one of whose windows
  changed, and, when the net is a subnet and one of its own ports changed,
  the version of the net around it, the loaded net's being the owner's own
  counter; `Notification::across` writes them as `subnet_marking_versions`
  and `outer_marking_version`. A `nested` wrapper carries the loaded net's
  version after the event. The quiet path of a hosted net crosses alike, so
  quiet and listening agree, and so do a filter's coming-into-view lines.
- **Fireable sets** (§12.4, §12.7). A crossing touches each net it moved, so
  both nets' sets are compared after a change of a port, and the inner sets
  are compared shallower nets first, so that a net's line precedes those of
  the nets inside it.
- **Reads during a firing** (§12.3). `held_by_the_body` holds a read
  addressed to a subnet one of whose windows the airborne transition has an
  edge on (`Loaded::reaches`) until it lands; a read of any other subnet,
  and of the loaded net, is answered at once, the window showing what the
  launch took. A subnet's own firings are inline, as every inner firing
  here, so the net around one is never read while a subnet's body runs.
- **One piece inside.** A subnet's firing is one `fired`, which may carry
  both fields, where the Python runner tells a `launched` and a
  `distributed` that carry one each. The transcript harness folds the two
  fields from both pieces of a firing as §11 says, so one transcript reads
  the same for both.
- **The host server** has nothing of its own: a hosted net's subnets are
  walked, told and kept quiet as its other inner nets are, a change the
  program makes through `subnet_scope` told by the subnet.
- **The conformance net `levels`** (§11) is written in the example's
  `nets.rs`, as `stepping-rules` is, since the core's `levels` example is
  another story, and `nets::levels_registry` carries it alone for the
  transcript over it. `nets::registry()`, what the example binary carries,
  lists it too, and a process test loads it over real pipes.
- **Transcripts.** `09_subnet_places.json` is copied verbatim from the
  Python track's recording and replays with nothing but the harness's fold
  of the two version fields; `05` to `08` and `10` did not change, and our
  own are untouched. `tests/subnets.rs` has the rest: the versions in each
  case, at two depths, with a subnet inside a token's net and a token's net
  inside a port, keys and filters on a port, a failed firing that restores
  into one, non-consuming edges and an empty drain that change nothing there, the
  read that waits, and a hosted world quiet and told.
- **What a reader of the protocol types meets** (the viewer, §6.1 of the
  spec): `Notification`'s firing and token variants gained `port` and the
  two version fields and name a place by `PlaceName`, as do `Produced`,
  `LogEntry` and `Filter::places`; `Marking::places` holds `PlaceEntry`s;
  `Place` gained `subnet`, `ports` and `port`, and its `serializable` is an
  `Option`.

#### 19.3.18 Groups on the wire, 2026-11-22

The runner's half of step 3 of the groups change (`dev-docs/group-spec.md`
§3), on PROTOCOL.md §5.1 and the conformance net `groups` of §11, which the
Python track wrote, built and recorded first. The core's groups are §6.8;
the runner serves them with no addition to the core, and the protocol stays
version 2.

- **Topology** (§5.1). `translate::topology` writes the core topology's
  `groups` as `protocol::Topology::groups`, and each place's, subnet
  place's and transition's group as `protocol::Place::group` and
  `protocol::Transition::group`, each written only when set. Every net's
  topology, the loaded net's and each inner net's, a subnet's and a token's
  net's, is written from that net's own core topology, so each shows its
  own groups and none of the net around it. A port listed in a subnet
  place's `ports` is given no `group`: its group is the subnet's, shown in
  the subnet's own topology. A window is not listed, and the core puts it
  in no group.
- **The rest of the wire is unchanged.** Requests, notifications, marking
  entries and log entries carry no group, and the engine reads none, so
  firing, stepping, ids and versions are those of the same net with no
  groups.
- **The conformance net `groups`** (§11) is written in the example's
  `nets.rs`, as `levels` is, its subnet built through a `Templates` that
  registers `Box`, so that the subnet is stamped with that name.
  `nets::groups_registry` carries it alone for the transcript over it, and
  `nets::registry()`, what the example binary carries, lists it after
  `levels`; a process test loads it over real pipes.
- **Transcripts.** `11_groups.json` is copied verbatim from the Python
  track's recording and replays with no change to the harness; `05` to
  `10` did not change, and our own `01_describe_load_state.json` lists
  `groups` in `describe`. `tests/groups.rs` has the rest: the fields of the
  loaded net and of its subnet by path and by nested id, a token's net with
  groups of its own in a net with none, nets with none at any depth
  (`levels`, `transit`), and firings, a page of tokens and the log, which
  name no group.

#### 19.3.19 Four more shared transcripts, 2026-11-22

The Python track recorded four shared transcripts over four new
conformance nets, each fixed in words in PROTOCOL.md §11 so that it can be
built from the text alone. The runner replays them with nothing on the wire
changed and no new rule in the harness.

- **The conformance nets** are written in the example's `nets.rs`, as
  `levels` and `groups` are, each carried alone by its own registry for the
  transcript over it. `workshop` (`12_failures.json`): its load parameter
  `restore`, a `bool` that defaults to `false`, gives both transitions
  `restore_tokens_on_failure`, since the Python library sets that for a
  whole net and the core for each transition; each body returns an error
  for a pot whose text begins with `"Cracked"`. `cellar`
  (`13_ports_from_outside.json`): its key `years`, an integer, is declared
  on the outer `Table` and on the port `Rack` inside the subnet, and the
  template seeds the port's bottle. `harbour`
  (`14_subnet_in_a_token.json`): `Quay` and `Sea` hold ships, nets the
  template `Ship` builds, each holding a subnet the template `Hold` builds,
  and both places are serializable through those templates. `odd-names`
  (`15_odd_names.json`): every name, the template's included, holds a
  slash, a hash, a space and a double quote, and the core takes them as
  they are. Each subnet is built through a `Templates` that registers its
  template's name, so that it is stamped with that name.
  `nets::registry()`, what the example binary carries, lists the four
  after `groups`, and process tests run `workshop`'s failures, without and
  with `restore`, and `odd-names` over real pipes.
- **A failure's text.** A shared transcript masks a failure's `error`, in
  a `failed` and in a log entry, as `"<label>"`, since the Python runner
  writes `"Type: message"` and this one the error's `Display` (§11, since
  2026-11-22). The harness's string masks match it already, as the Python
  harness's do, and `the_masks` pins that.
- **Transcripts.** The four are copied verbatim from the Python track's
  recording and replay with no runner change; `05` to `11` did not change,
  and our own `01_describe_load_state.json` lists the four nets in
  `describe`. Every failure in `12_failures.json` is a firing of the loaded
  net with no mutate edge, which both runners report as `launched` then
  `failed`. `cellar`'s `Age` has a mutate edge, which this runner reports
  in one piece and the Python runner in two; the fold gives the same lines.
- **One net alone over pipes.** Twelve of the fifteen shared transcripts
  pin `describe` in full, and the example binary carries every net, so a
  host replaying a shared transcript against it over pipes could not match
  `describe`. `main` takes `--net <name>`, with `--describe` or without,
  in either order: it keeps the entry registered as `<name>`, the first if
  several are, as `load` finds one, and drops the rest before anything
  else, so that `describe` is that of the net's own registry, the one the
  replay without a process uses. A name the binary does not carry exits 2
  with `unknown net "<name>" for --net` and the names it carries, on
  stderr; `--net` with no name, or an option given twice, is the usage
  error. Every binary built on `main` takes it, the example's included.
  `the_shared_transcripts_hold_over_real_pipes` replays every shared
  transcript in `tests/protocol/` against the binary started with `--net`,
  with the harness's own rules, reading each exchange's lines up to its
  response and then, while fewer lines have come than it expects or a
  `launched` waits for its other piece, what follows, as a running net's
  firings follow the answer to `start`.

#### 19.3.20 Choosing by another input on the wire, 2026-11-23

A keyed edge that chooses its token by another input (§8.3, "Choosing by
another input") needs three things of the runner, and nothing else on the
wire changes; the protocol stays version 2, the clause being an optional
new shape shown as declared (PROTOCOL.md §13.2).

- **Writing it.** `filter_to_wire` writes a `Clause::IsInput` as
  `protocol::Clause::IsInput { key, input: protocol::InputKey { place,
  port, key } }`, `{"key", "input": {"place", "port"?, "key"}}`, the source
  named by `Names::split` as an edge's place is, so a port's source gives
  its subnet place and `port`. The derived, lenient `Deserialize` reads it
  back.
- **Refusing it in a request.** It has no meaning outside a firing, so
  `protocol::Filter::parse` refuses a clause with `input` in a `query` or
  `set_filter`, and `filter_from_wire` refuses one that reaches it by
  another road; both are `invalid_params` with a message that says why.
- **The conformance net** `couriers` is written in the example's `nets.rs`
  from PROTOCOL.md §11's words, carried alone for the transcript over it,
  and listed last in `nets::registry()`, so our own
  `01_describe_load_state.json` lists it in `describe`. Its `Load` has a
  mutate edge, so this runner reports each of its firings in one piece and
  the Python runner in two; the fold gives the same lines. Its `Dispatch`
  failure is a firing with no mutate edge, `launched` then `failed` from
  both. `16_keyed_by_input.json` is copied verbatim from the Python track's
  recording and replays with no rule added to the harness.

#### 19.3.21 Read edges on the wire, and readers that do not know a kind, 2026-11-23

- **Writing them.** `edge_kind` maps the three read kinds to `"read"`,
  `"read_where"` and `"read_all"`, and a read-where carries its filter as
  any keyed edge does. A body with a read edge runs inline, so the runner
  reports its firing in one piece, as it does a mutate edge's; the Python
  runner reports it in two, and the fold gives the same lines. No `mutated`
  is sent for a token read.
- **Reading tolerantly.** Until now the runner's `protocol` types failed a
  whole topology over one edge kind or one clause shape they did not know,
  which is what a reader built on 0.2.x still does. `protocol::EdgeKind`
  gains an untagged `Other(String)` and `protocol::Clause` an untagged
  `Other(Value)`, each last, so that a name or a shape this version does
  not know is kept as written, written back unchanged, and the rest is
  read. A request's filter is still read strictly (`Filter::parse`), and
  `filter_from_wire` refuses an `Other` clause as `invalid_params`.
  `TransitionDetail` is the Rust runner's own `detail` and stays strict, so
  these types still cannot read a topology the Python runner wrote, whose
  `detail` has other keys (PROTOCOL.md §5.1 lets a reader ignore it).
- **The conformance net** `roll-call` is written in the example's `nets.rs`
  from PROTOCOL.md §11's words: `Barracks` holds squads, each a net the
  template `Squad` builds and then given its members, keyed by `leader`,
  and read by `Call the Roll` (read-all), `Check the Front` (read) and
  `Inspect` (read-where, bound to `Inspections` by `leader`).
  `17_read_edges.json` is copied verbatim from the Python track's
  recording.

#### 19.3.22 Ports on every net, and an edge's reach on the wire, 2026-11-23

- **Ports.** `topology` writes `port: true` on a marked place of whatever
  net it writes, where it wrote it only on a subnet's (`View::subnet` and
  `Inner::is_subnet` are gone with the rule). No shared transcript before
  `18` read a loaded net or a token's net that marks a port, so none
  changed.
- **Reaches.** `protocol::Edge::reaches`, written only when not empty, from
  `EdgeInfo::Input::reaches`. The `protocol` types read an edge field they
  do not know by passing over it, as serde does by default, so a reader
  built on 0.2.x reads a topology carrying `reaches` and drops the names;
  Python's wire models refused unknown fields until this step.
- **The conformance net** `trays`, from PROTOCOL.md §11: `Hand Over` puts an
  order into the front unit's `Orders In` (`MutateEdge(field)
  .reaching(["Orders In"])`), the unit's `Fire` shoots inside it, and
  `Collect` takes the shots out of `Shots Out` into `Reports`, guarded on
  there being one. `18_ports_on_a_token.json` is copied verbatim from the
  Python track's recording.

#### 19.3.23 Closed nets on the wire, 2026-11-23

- **`inject`.** Refused outside the ports of a closed net (§6.9), at the
  top level and by path, it answers the new error code `closed_net`
  (`ErrorCode::ClosedNet`, mapped from `NetError::ClosedNet` in
  `errors.rs`), the net unchanged and nothing told. A window of a closed
  net is never one of its ports, so an `inject` with `port` beside `place`
  into a closed net's subnet place is `closed_net` too. An older client
  sees a code it does not know, which PROTOCOL.md §3 tells it to treat as a
  failed request. Since 2026-11-28 an `inject` by `path` or `nested_id`
  into a net inside a closed net is `closed_net` at any place, a port
  included: `descend_mut` reaches it through `nested_scope` and
  `subnet_scope`, which shield it, so the runner needs no check of its
  own. `unknown_place` and `invalid_token` are answered first. The other
  requests that change a net (`enable`, `disable`, `set_param`, `press`)
  are knobs, which the rule leaves to the program and the runner.
- **`open`.** `protocol::Topology::open`, written as `open: true` only for
  a net whose definition declares it open, from `Net::is_open`. A reader
  knows a net is closed when its topology shows no `open` (since
  2026-11-28; from 2026-11-23, a `port` and no `open`).
  `protocol::Topology` gained a field, so code that builds one by struct
  literal adds `open: false`. The `fireable`, `launched` and `failed`
  reporting is unchanged: a body's refusal is an ordinary `failed` firing
  whose `error` the fixtures mask.
- **The conformance net** `post-office`, from PROTOCOL.md §11: `Posts`
  holds a `Post`, closed since it marks `Inbox` a port, and an `Open
  Post`, the same declared open. `Deliver` puts a letter into the front
  post's `Inbox` (`MutateEdge(posts).reaching(["Inbox"])`); `Forge` takes a
  forgery and the front post and writes the forgery into the post's
  `Ledger`, which the closed post refuses, failing the firing and losing
  both tokens, since neither transition restores its inputs (a consuming
  edge on a place of nets cannot, §6.9). `19_closed_nets.json` is copied
  verbatim from the Python track's recording. The protocol stays version
  2: `open` is an optional new field and `closed_net` a new code, both of
  which an older reader passes over or treats as a failed request.

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
