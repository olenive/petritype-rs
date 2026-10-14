# Input edges that accept an empty place

Written 2026-10-14 for the petritype owner by the agent working on the
Cyberpunk2020-Simulation game, after reviewing petritype-rs at ac80731,
petritype-bevy at 6d5813b and the Python petritype at 85f5eb2. The owner
decides; the defaults below are a proposal shaped by one consumer. It is
meant to be enough for another agent to implement from alone.

Revised 2026-10-14 after the owner's review: the example (§2, §6) is a shop
rather than overwatch, so that it reads without wargame vocabulary; the names
are `DrainEdge` / `SurveyEdge` (D1, taken); §1 says when *not* to use the
feature; R3 and R8 carry the Python shape; §5 orders the work after the
pending merge.

## 1. The idea

Every input edge petritype has today holds its transition back until its place
has a token: the four plain kinds need the place non-empty, the two keyed kinds
need a token their filter accepts (`petritype/src/edge.rs:15-18`, DESIGN §4.4
table, §6.1). That is right for a token the transition works *on*. It is wrong
for a place the transition only needs to *look at* when something is there:
"every promotion running", "every subscriber to notify", "every warning on
this build", "everyone currently on overwatch", where *none* is a perfectly
good answer.

Today a transition that must read such a place and still fire when it is empty
needs an "empty twin": a second transition with the same inputs minus that
edge, enabled by a guard that checks the place is empty (the game's design
record has carried this as a known cost since its first week:
`Cyberpunk2020-Simulation/docs/world-net-design.md:21`). One optional place
doubles a transition; *k* optional places read by one transition need 2^k
twins. That is where the game's fight refactor stopped on 2026-10-14
(`docs/world-net-design.md` §5.1b, the note on fighter state places): the
stage moves of a firefight read several usually-empty state places each.

The other workaround is the coloured-net one: keep the collection as a single
token holding a list, so the place is never empty (the game's aggregate
singletons, `docs/world-net-design.md` §8.1 P3). It is the right answer while
the collection is an aggregate. It stops being one when the members must be
individual tokens: a keyed edge selects them, they migrate to other places, a
viewer shows them one by one. That is the case this feature is for; a reader
who has an aggregate should keep the list token.

The feature: two input edge kinds that read **every token in a place, zero or
more**, and never hold the transition back. `DrainEdge` consumes, `SurveyEdge`
does not. In Petri-net terms the first is a *reset arc*, a standard extension
that leaves the enabling rule alone; the second is a pure data dependency with
no effect on enabling at all.

## 2. The example: a shop

A small net the acceptance test builds. Places hold strings. A shop prices
orders as they come in, applying whatever promotions happen to be running,
and ends every promotion when the day closes. A shop with no promotion
running still sells things: that is the whole point.

| Place | Tokens |
|---|---|
| `Orders` | orders waiting to be priced |
| `Promotions` | promotions running right now (usually none) |
| `Day End` | one marker token when the shop closes |
| `Priced` | output: one per order priced |
| `Discounts Applied` | output: one per promotion applied to an order |
| `Expired` | output: one per promotion that ended at the close of the day |

Transitions:

- **`Price Order`**: `TakeEdge(Orders)`, `SurveyEdge(Promotions)` →
  `(Priced, Discounts Applied)`. The body gets the order (`String`) and the
  promotions (`Vec<String>`, oldest first, possibly empty) and returns the
  order for `Priced` and one `"{promotion} on {order}"` per promotion for
  `Discounts Applied`. The promotions stay where they are.
- **`Close Day`**: `TakeEdge(Day End)`, `DrainEdge(Promotions)` →
  `(Expired,)`. The body gets the marker and every promotion (`Vec<String>`,
  possibly empty), consumes them all, and returns one `"{promotion} expired"`
  per promotion.

Script: seed `Orders = [Order 1]`, `Promotions = []`. Step: `Price Order`
fires with no promotions; `Priced = [Order 1]`, `Discounts Applied = []`.
Seed `Promotions = [10% off, Free shipping]`, `Orders = [Order 2]`. Step:
`Price Order` fires; `Discounts Applied = [10% off on Order 2, Free shipping
on Order 2]`, `Promotions` still `[10% off, Free shipping]`. Seed `Day End =
[close]`. Step: `Close Day` fires; `Expired = [10% off expired, Free shipping
expired]`, `Promotions = []`. Seed `Day End = [close]` again. Step: `Close
Day` fires with nothing to expire; `Expired` unchanged, the marker consumed.
The net is then quiescent. With `Orders` empty, `Price Order` is not fireable
however many promotions are running.

The game's own case, a firefight's overwatch (fighters covering an area react
to every shot; their watch lapses at the round end), has the same shape:
`Price Order` is `Fire`, `Close Day` is `Lapse Watch`, `Promotions` is `On
Overwatch`.

## 3. What exists today (anchors for the implementer)

The anchors are against main. The `engine-policies` branch (slice 21, merged
first, §5) moved code in `firing.rs` and `nested.rs`; re-anchor there before
starting.

petritype-rs (ac80731):

- `petritype/src/edge.rs`: the module doc (lines 10-27) states the presence
  rule; `EdgeKind` (line 58, `#[non_exhaustive]`, serde `snake_case`) with
  `consumes()`, `is_keyed()`, `mutates()` (lines 76-104); the six edge structs
  (`TakeEdge` 222, `TakeAllEdge` 232, `SnapshotEdge` 246, `MutateEdge` 266,
  `TakeWhereEdge` 321, `MutateWhereEdge` 340); `InputEdge`, `InputSpec`, the
  `impl_edges!` arity macro (line 530; arity from `src/arity.rs`, 12).
- `petritype/src/builder.rs`: `assert_no_source_transitions` (~line 880-915):
  a transition is a source unless it has a consuming edge; read edges do not
  count. `SnapshotOfNoClonePlace` (`error.rs`) refuses a snapshot of a
  `place_no_clone` place. `DuplicateInputPlace`: one edge per place per
  transition.
- Fireability scan: DESIGN §6.1 ("every input place non-empty ... and a keyed
  edge one its filter accepts"); the launch stage consumes per edge kind
  (§6.2); `restore_tokens_on_failure` puts consumed tokens back at the front.
- DESIGN §4.4 (the edge table and `TransitionSlot`), §8.3 (keyed edges, the
  only precedent for a new edge kind: slice 19 added `TakeWhere` and
  `MutateWhere` across core, info, wire and viewer), §18 (slices; the last is
  20), §19.3 (the wire as this runtime implements it).
- Nested places (DESIGN §6.6): snapshot edges from a nested place are a build
  error because the clone path may not fail.

petritype-runner: the topology lists `edges[].kind` straight from
`EdgeKind`'s serde names; the two keyed kinds carry `filter`.

PROTOCOL.md (in the Python repo, `~/work/petritype/PROTOCOL.md`, §5.1 lines
189-196): `kind?` is one of `take`, `take_all`, `snapshot`, `mutate`,
`take_where`, `mutate_where`; "a reader that meets another value draws a
plain edge".

petritype-bevy (6d5813b): `src/remote/mirror.rs:189-194` maps the wire kind
to `EdgeKind` (unknown → `Take`); `src/render.rs:879-889` colours a
non-consuming edge as a read edge and draws a keyed edge dashed;
`src/inspector.rs:421` prints the kind and a keyed edge's filter.

Python petritype (85f5eb2), `petritype/core/executable_graph_components.py`:
`ArgumentEdgeToTransition` (248, consuming), `TakeWhereEdge` (260),
`SnapshotEdge` (277: a deep copy of the place's tokens; "fireability requires"
presence), `MutateEdge` (286), `MutateWhereEdge` (295); the presence check
near line 1275 ("read edges require presence too"); the source and cycle
analyses near 1462 and 1513 treat read edges as moving nothing. Python has
no take-all *kind*: the plain argument edge hands the body every token as a
list when the parameter is annotated `list[T]`, and the runner reports that
edge as `take_all` on the wire (`petritype/runner/translate.py`,
`Topologies.takes_all`). The source rule works off the set of argument edges.

## 4. Requirements

**R1. Two edge kinds.** `DrainEdge<T>(place)`: consumes every token the
place holds at launch, oldest first, zero or more; the body parameter is
`Vec<T>`. `SurveyEdge<T>(place)`: clones every token, oldest first, zero or
more, consumes nothing; the body parameter is `Vec<T>`; refused at build on a
`place_no_clone` place (as `SnapshotEdge` is) and from a nested place (as
snapshot edges are). Both read one place through one edge
(`DuplicateInputPlace` applies). `EdgeKind::{Drain, Survey}`; `consumes()` is
true for `Drain` and false for `Survey`; `is_keyed()` and `mutates()` false
for both. The names are decision D1, taken.

**R2. They never hold a transition back.** Add `EdgeKind::requires_presence()`
(true for the six existing kinds, false for the two new ones) and make the
fireability scan (DESIGN §6.1) skip the presence check for an edge that does
not require it. A guard can still hold the transition back.

**R3. The source rule treats them as reads.** `assert_no_source_transitions`
asks for a consuming edge that is guaranteed to consume; a `DrainEdge` may
consume nothing, so it does not satisfy the rule (a transition whose only
place-backed input is a `DrainEdge` is a source and fires for ever on an
empty place). Say so in its doc, with the example. The same applies to any
other analysis that assumes a consuming edge moves a token (the acyclicity
check's reasoning, DESIGN §4.4 / `builder.rs` module doc). In Python the
drain edge is an argument edge by class (R8), so the source rule and the
cycle analysis must exclude it by hand rather than by type.

**R4. Firing.** Launch consumes a `Drain` place's tokens as `TakeAll` does;
`Event::Fired.consumed` lists them, which may be none from that place;
`restore_tokens_on_failure` puts them back at the front, oldest first, with
their ids, as every restore does. `Survey` emits nothing and moves nothing;
a firing whose only reads are snapshots or surveys stays detachable. Nothing
new in `Marking`, `Snapshot` or `export_marking`.

**R5. Info and topology.** `TransitionInfo::inputs` and `EdgeInfo::Input`
carry the new kinds as they carry the others; `filters` stays `None` for
them. DESIGN §4.4's table gains two rows (presence required: no).

**R6. Wire.** PROTOCOL.md §5.1 `edges[].kind` gains `drain` and `survey`
(the serde names fall out of the enum). The rule that an
unknown kind draws a plain edge already covers old readers. `fired.consumed`
needs no change. If only the Rust track runs (decision D2), the Rust agent
writes the proposed §5.1 wording into petritype-rs DESIGN §19.3 for the
protocol's owner rather than editing PROTOCOL.md.

**R7. Viewer.** `mirror.rs` maps the two strings; `render.rs` already colours
by `consumes()` and dashes by `is_keyed()`, so `Survey` draws as a read edge
and `Drain` as a consuming one. Mark an edge that accepts an empty place so
a reader can tell it from `TakeAll`/`Snapshot`: a hollow arrowhead (D5,
taken). The inspector prints `Drain` / `Survey`.

**R8. Python parity** (decision D2): the same two kinds in
`executable_graph_components.py`, the presence check skipping them, the
source and cycle analyses treating them as reads, the Python runner's
topology emitting the kind strings, the visualiser drawing them, and the
shared transcript suite gaining a case. The shape follows the Python
precedents: `DrainEdge` subclasses `ArgumentEdgeToTransition` (as
`TakeWhereEdge` does) and its argument must be annotated `list[T]`, checked
when the graph is built, since a plain `T` has nothing sensible to receive
from an empty place; `SurveyEdge` subclasses `SnapshotEdge`, which already
yields every token, and differs from it only in not requiring presence,
which the docstring must say.

**R9. Non-goals.** No keyed zero-or-more variants (`DrainWhereEdge`: every
token the filter accepts, zero or more) unless they fall out for free
(decision D4). No mutating zero-or-more edge (decision D3). No cap on how many tokens an edge takes. No
way to return a token to the *front* of a place from a body (a separate want
of the game's, noted in D6; not this feature).

## 5. Suggested order

First the pending merge: the five next-phase branches (petritype
`protocol-gaps`, petritype-rs `engine-policies`, petritype-bevy
`viewer-followups`, petritype-server `relay-nested-keys`, petrivelte
`nested-and-keys`) land on main before this starts, and the Rust work is
slice 22. Then, as the last two features ran, one spec and two tracks.

Rust: (1) core: the two edge structs, `EdgeKind` variants and predicates, the
scan and launch changes, builder errors, the source rule, unit tests incl.
the §2 net and a `trybuild`-style check if the crate has one for edges;
(2) info, wire, runner topology, DESIGN §4.4/§6.1/§18/§19.3; (3) petritype-bevy
mirror, render, inspector, a process test over the §2 net. Python, if in
scope: core, analyses, runner topology, visualiser, transcript. The two tracks
are independent and may run in parallel; the wire strings are the one shared
contract.

## 6. Expected results of the §2 script

| After | `Priced` | `Discounts Applied` | `Expired` | `Promotions` |
|---|---|---|---|---|
| Price Order (no promotions) | [Order 1] | [] | [] | [] |
| Price Order (two running) | [Order 1, Order 2] | [10% off on Order 2, Free shipping on Order 2] | [] | [10% off, Free shipping] |
| Close Day | same | same | [10% off expired, Free shipping expired] | [] |
| Close Day (none running) | same | same | same | [] |

Also: `Price Order` is absent from `fireable()` while `Orders` is empty
whatever `Promotions` holds; `assert_no_source_transitions` rejects a
transition whose only input is a `DrainEdge`; a `SurveyEdge` on a
`place_no_clone` place fails to build with the no-clone error; a failing
`Close Day` body restores the promotions oldest first with their ids; the
topology lists the two kinds; the viewer draws them.

## 7. Decisions (taken where marked; otherwise defaults the implementer may take)

- **D1 Names.** Taken 2026-10-14: `DrainEdge` / `SurveyEdge`, wire `drain` /
  `survey`. "Drain" is the `Vec::drain` idiom: empties the place, yields zero
  or more. "Survey" looks over whatever is there, possibly nothing. The
  proposed `TakeAnyEdge` / `SnapshotAnyEdge` were rejected: in Rust `any`
  means at least one, and `TakeAll` beside `TakeAny` reads as all beside one,
  which is `TakeWhere`'s job. Also considered: `TakeAllOrNoneEdge` /
  `SnapshotAllOrNoneEdge` (unambiguous, long), and a modifier on the existing
  kinds (`TakeAllEdge(p).or_empty()`), which keeps the enum smaller but hides
  the difference from `match` arms and from the wire.
- **D2 Python parity** in the same go (two independent tracks, as the last two
  features) or Rust first and Python later. Taken: both.
- **D3 A mutating zero-or-more edge** (borrow every token in place, ids kept).
  Default: no. The game's own list (`docs/world-net-design.md` §8.3, item 2)
  rates it high, but a whole-queue borrow is a larger change than this
  feature; the presence predicate (R2) leaves room for it later.
- **D4 Keyed zero-or-more variants** (`DrainWhereEdge`, `SurveyWhereEdge`).
  Default: no; the predicate of R2 is what they would reuse.
- **D5 Viewer marking** of an edge that accepts an empty place. Taken: hollow
  arrowhead, cheap enough; the inspector text alone is the fallback.
- **D6 Returning a token to the front of a place** from a body (the game's
  re-entrant plan steps take all step tokens and put them back in order to get
  this). A separate feature if wanted; recorded here only so it is not lost.

## 8. Rules for the implementing agent

Work in the library repo(s) only: do not touch `~/work/games/**` or
`~/work/petritype-bevy/TODO_TEMP.md`. One branch per phase, tests green at
each, the owner merges. When done, append an "As built" section to THIS file
naming what differs from sections 2, 4 and 6 and why, plus the commits.

## 9. Prompt

> Implement `~/work/petritype-rs/dev-docs/EDGES_ON_EMPTY_PLACES.md`: two input
> edge kinds that read every token of a place, zero or more, and never hold the
> transition back; `DrainEdge` consumes, `SurveyEdge` does not. Read the
> whole document first, then the anchors in section 3. Section 7's decisions
> are taken where marked; take the defaults elsewhere. Rust track: petritype-rs core, info, wire,
> runner topology and DESIGN, then petritype-bevy. [Python track: the same in
> `~/work/petritype`, with PROTOCOL.md §5.1.] Build the section 2 net as the
> acceptance test and check section 6. Append an "As built" section to the
> document when finished. Do not touch `~/work/games/**` or
> `~/work/petritype-bevy/TODO_TEMP.md`.

## As built (Rust)

Slice 22, 2026-10-14, branch `empty-place-edges` of petritype-rs, cut from
main at 6e8ef4d (after the merge of §5): the Rust track's §5 steps 1 and 2,
R1 to R6. The viewer (R7, step 3) is petritype-bevy's and is not part of
this branch. DESIGN §4.4 ("Edges on empty places"), §6.1, §12, §18 (slice
22) and §19.3.14 describe what was built; this section names what differs
from §2, §4 and §6, and why.

**§2, the shop.** Built as written, in `petritype/examples/shop/net.rs`
(the net, `build_shop`, which a runner includes by path) and
`petritype/examples/shop.rs` (the script and the §6 table, printed and
tested), in the shape of `wildlife`. What the document left open was
settled with the Python track through the shared contract: every place is
`String` and serializable, so that the wire's `inject` reaches any of them;
the bodies are named functions (`apply_promotions`, `end_promotions`);
`Price Order` returns positionally, the order into slot 0 (`Priced`) and the
discounts into slot 1 (`Discounts Applied`), so the wire lists both slots;
the net's description is the Python factory's, "Price orders with whatever
promotions are running, and end them at the close". The example needs the
`serde` feature only because `serializable` does.

**§4, the requirements.**

- R1 as written. `DrainEdge<T>` needs no `Clone`; `SurveyEdge<T>` asks
  `T: Clone` at compile time, as `SnapshotEdge<T>` does, and the build
  refuses a survey of a place that gave cloning up. That refusal is
  `SnapshotOfNoClonePlace`, not a sibling: its message ("transition ...
  snapshots place ..., which cannot clone its tokens") is true of a survey,
  which snapshots every token, and a sibling error would have been a second
  name for one rule. A nested place is a no-clone place, so a survey of one
  is refused by the same check; since `SurveyEdge<T>` needs `T: Clone`, the
  build only meets a nested token type that is `Clone` (one that nests no
  net, say), and every other is refused by the compiler first. A drain of a
  no-clone place builds, unless its transition restores
  (`RestoreOfNoClonePlace`, empty place or not).
- R2 as written: `EdgeKind::requires_presence()` is public, false for
  exactly `Drain` and `Survey`; the scan skips the presence check for them.
- R3 as written for the source rule, and for the acyclicity walk too: both
  count only the edges that always take a token (a crate-private
  `EdgeKind::always_consumes`, consuming and requiring presence), so a
  drain counts in neither, as the Python track does by hand (R8). The
  document asked that "any other analysis that assumes a consuming edge
  moves a token" say the same; leaving the drain out of the walk is the
  choice that agrees with Python, and it keeps the termination proof sound:
  every transition still has an edge that always takes a token, fed only
  from upstream of it in the walked graph, so it fires no more often than
  that place is fed, however many tokens a drain carries round meanwhile. A
  loop closed only by a drain is therefore not reported as a cycle, which
  the docs of `assert_acyclic` say ("tokens only move forward along the
  edges the walk follows").
- R4 as written. A drain shares the take-all's launch code, so an empty
  place gives an empty batch, and the index and a nested token's inner log
  are handled as for any consuming edge; a restore puts the drained tokens
  back at the front, oldest first, with their ids, ahead of anything put in
  while the firing was in flight. A survey's clones are owned, so its
  transition stays detachable and runs on a worker under the concurrent
  driver.
- R5 as written: `TransitionInfo::inputs`, `EdgeInfo::Input` and the
  topology carry the kinds, `filters`/`filter` `None`.
- R6: both tracks ran (D2), so the wording of PROTOCOL.md §5.1 is the
  Python track's, which also amended §5.2 (`fireable`: every input place
  non-empty "but one read through a `drain` or `survey` edge") and §11 (the
  `shop` conformance net). DESIGN §19.3.14 records the same for this
  runtime: the runner's `protocol::EdgeKind` gained `Drain` and `Survey`,
  its `fireable` is the core's and needed no change, and the shared
  `07_empty_place_edges.json` replays byte for byte. Our own transcript
  01, which pins `describe` over everything the example binary carries,
  lists `shop` now.
- R7 is not on this branch (petritype-bevy's step). What it asks of the
  core is there: a viewer can mark an edge with `!kind.requires_presence()`.
  `petritype-dot`, this repository's renderer, was given D5's mark as an
  addition: a drain draws as a take-all and a survey as a snapshot, each
  with `arrowhead=empty` and `*`.
- R9 held: no keyed and no mutating zero-or-more edge.
- The InputEdge diagnostic now names `DrainEdge(p)` and `SurveyEdge(p)`
  among the edges an input must be (`tests/ui/fail_not_an_edge.stderr`),
  and §5's trybuild check is two cases: `pass_empty_place_edges.rs` (both
  hand over a `Vec<T>`) and `fail_drain_without_vec.rs` (a drain read as one
  `T`).

**§6, the expected results.** All hold: the table after each of the four
steps, `Price Order` absent from `fireable()` while `Orders` is empty
whatever `Promotions` holds, the source rule rejecting a transition whose
only input is a `DrainEdge`, a `SurveyEdge` on a `place_no_clone` place
failing to build with the no-clone error, a failing `Close Day` restoring
the promotions oldest first with their ids, and the topology listing the
two kinds (`tests/slice22.rs`, the example's tests, the runner's
`drain_and_survey_edges_go_out_with_their_kinds` and transcript 07). "The
viewer draws them" is petritype-bevy's to show; `petritype-dot` draws them.

**Python, for the record** (its own report has the details): the drain is
an argument edge and the survey a snapshot edge by class, both kinds
require their argument to be annotated `list[T]`, checked when the graph is
built, and the source and cycle analyses leave the drain out by hand.

**Commits.** 712b2db (the slice: core, example, runner, transcript 07,
tests and DESIGN), and the commit that adds this section.

## As built (Python)

**As built (Python), 2026-10-14**, petritype branch `empty-place-edges`: commit f23a2a6 (code, tests, transcript) and commit ade50aa (docs). `DrainEdge` subclasses `ArgumentEdgeToTransition` and `SurveyEdge` subclasses `SnapshotEdge`, as R8 asks.

**§2.** The shop is as written, with every place `str`. `Price Order` returns `(order, discounts)` through two indexed return edges (slots 0 and 1). `Close Day` returns its list into `Expired`, and an empty list deposits nothing. The net's `describe` description, which the document leaves open and the transcript pins, is "Price orders with whatever promotions are running, and end them at the close".

**§4.**
- R1: Python has no `place_no_clone`, so there is no no-clone error. A survey of a nested place is allowed, as a snapshot is, because a deep copy of a net mints a new net. Python has no `DuplicateInputPlace` either; nothing changed there.
- R1 and R8, the list argument: `SurveyEdge` also needs a `list[T]` argument, checked at build like the drain's. R8's "which already yields every token" holds in Python only for a list-annotated argument, and R1 gives the survey `Vec<T>`. For both kinds this exact check replaces the general argument type check, so a place of lists drains into `list[list[T]]`. A plain list-annotated take on such a place is still refused by the old check; that is unchanged.
- R2 is `ExecutableGraphCheck.requires_presence(edge)`.
- R3: both static checks leave drains out by hand, with the shop's examples in their docstrings. The source rule's message now reads "no ArgumentEdgeToTransition input other than a DrainEdge".
- R4: Python has no "detachable" to keep. `Fired.consumed` lists nothing from an empty drained place, and restore puts drained tokens back at the front with their ids.
- R5: Python has no `TransitionInfo`; the runner's topology is the info.
- R6: PROTOCOL.md was edited directly, since both tracks ran. §5.2's `fireable` ("every input place non-empty") was amended as well, since it would otherwise be false.
- R7: the Python viewer is the Graphviz renderer. Both kinds get a hollow arrowhead (`arrowhead="empty"`).
- R8: `@petri_net` declares no edges, so the decorator needed nothing.

**§6.** Everything is pinned except the no-clone build error, which does not apply in Python. The shared transcript adds a `get_token` of each output token after the final `get_state`, because labels are masked and only those reads pin the section 6 values.

**§8.** The work was done in the Python repository only, in its own worktree and branch. This section was written in the Python track's report and appended here by the owner's agent, since the document lives in the Rust repository.

## As built (Bevy)

**As built (Bevy), 2026-10-14**, petritype-bevy branch `empty-place-edges`, commit 83ced6d. This is viewer
slice 11, DESIGN §3.10.

R7 is built as written:
- `mirror.rs` maps `drain` and `survey`, and any other kind still draws as a plain edge.
- The colour still follows `consumes()`, so a survey reads as a snapshot does and a drain as a take.
- The dashes still follow `is_keyed()`.
- The inspector prints `Drain` and `Survey` with no filter. It needed no code change, because it prints the
  kind's `Debug`.

One addition R7 did not name: under a projection, the mirror's doubt rule counts a drain as taking any number of
tokens, as it counts a take-all. A firing through a drain may have taken hidden tokens, and its `consumed` list
leaves them out.

D5's hollow arrowhead is the open head's two strokes, the same size and angle, closed by a third stroke. The line
stops at that stroke, and the head is chosen by `!kind.requires_presence()`, separately from the dashes. The
head is *left empty*, not filled with a backdrop colour. Bevy's gizmos draw lines and fill nothing, so an empty
head shows whatever the edge is drawn on: the page, the side panel's backdrop or an inner window's, in either
theme, without the renderer needing to know which. That gives the outline-filled-with-backdrop look D5 asks for,
and it cannot pick the wrong backdrop.

To make the head testable, the per-edge drawing moved into `render::draw_edge`, which draws into a `GizmoBuffer`
that a test reads back. A dashed straight line now stops `TIP_LENGTH` short of the tip, instead of running under
the solid stretch into the head. This looks the same, and it keeps a hollow head empty if a keyed kind ever
accepts an empty place.

The acceptance net is built in the tests with the builder rather than included by path, since the sibling
checkout carries `shop` only from slice 22. One script, `tests/common::the_shop_script`, checks §6 three ways:
- as a net in process;
- on the Rust runner's state machine in process;
- against the runner's example binary. This test is Rust only, as the wildlife ones are, and skips when the
  binary has no `shop`.

All of §6 that concerns the viewer holds:
- the table after each step;
- `Price Order` absent from the fireable set while `Orders` is empty, whatever `Promotions` holds;
- the topology's two kinds;
- the viewer drawing them;
- a close with nothing to drain consuming its marker alone.
