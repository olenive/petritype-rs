> **Review outcome, 2026-10-15.** Reviewed by the planning thread with the
> owner. R3 (the cap to 16) stands. R1, R2, D1 and D4 (`Group`) are replaced
> by binding by name, `dev-docs/BINDING_BY_NAME.md`, which takes this
> document's §1 problem and §2 till as its own. Implementers start there.

# Transitions with more than twelve inputs or outputs

Written 2026-10-15 for the petritype owner by the agent working on the
Cyberpunk2020-Simulation game, after the fight refactor met the cap at
petritype-rs c93ec03 (petritype-bevy 5f419c0, Python petritype d731f81). The
owner decides; what follows is the problem as one consumer meets it, the
anchors, and a proposal with defaults. It is meant to be enough for another
agent to implement from alone.

## 1. The problem

A transition declares its inputs and its outputs as tuples, and the traits
over those tuples (`InputEdges`, `OutputPlaces`, `TransitionBody`) are
implemented by `macro_rules!` for every arity from 0 to 12
(`petritype/src/arity.rs`; DESIGN §5: "Input arity is capped at 12. Output
arity likewise"). A `Param<T>` input counts as one position. The cap was set
where "a tuple is still the right shape for the job", and for a net whose
transitions read two or three places that is true.

The game's firefight is past that point, by design rather than by accident.
The design principle there is that the net owns control flow and game state
and the viewer shows it, so what used to be fields on a fighter token are
now actor-keyed places: nine mechanics states (overwatch, defence, aim,
waiting, ambush, fast draw, fast draw next, tangle, jam) read through
`SurveyEdge` and changed through `DrainEdge`, beside the baton, the fighters,
the arena, the dice and the dice-mode parameter. After that move
(2026-10-14, the game's `docs/world-net-design.md` §5.1b):

| Transition | Inputs | Outputs | How it fits today |
|---|---|---|---|
| `Cmb/Step Attack`, `Cmb/React` | 12 | ~9 | exactly at the cap |
| `Cmb/Resolve Melee Attack`, `... Extra`, `... Defence` | ~10 | 12 | exactly at the cap |
| `Cmb/Resolve Shot To Hit`, `... Extra` | ~10 | 12 | the arena is surveyed, not taken, to stay under 12 outputs; the tangle state is only surveyed |

The next slice, the character condition cluster, adds to most of these
transitions one surveyed place for the shared character body and one drained
place per condition they may set (stunned, stabilised, doomed, broken limb),
so the busiest would need 14 to 16 inputs and 13 to 15 outputs. The choices
inside the cap are all worse than the cap:

- **Bundle** several states into one token per character (one place
  `Stances` holding `{aim, defence, ...}`). This hides from the viewer what the
  places were created to show, and a transition that changes one state must
  drain and re-emit the bundle.
- **Split** a transition into two firings so each stays under the cap. The
  net then shows a stage that exists only to dodge a tuple limit.
- **Read inside the body** through the net. Not possible (bodies have no
  net), and not wanted (an edge is what the viewer draws and what
  `restore_tokens_on_failure` protects).

The Python runtime has no cap (edges are lists), so the two runtimes already
differ here; a net that compiles in Rust is a subset of what Python runs.

## 2. The example: a till

A small net the acceptance test builds. Places hold strings except where
said. One transition reads fourteen places and writes thirteen.

- Inputs: `Orders` (`TakeEdge`, the customer's order), `Till Open`
  (`SnapshotEdge`, a marker), `Prices` (`Param<Prices>`), and eleven
  surveyed stock shelves `Shelf 1` .. `Shelf 11` (`SurveyEdge`, each a list
  of item names, possibly empty).
- Outputs: `Receipts` (one per order), eleven `Restock 1` .. `Restock 11`
  (an item name for every shelf the order emptied, usually none), and
  `Served` (the customer's name).
- Body: `Serve` prices the order from whatever is on the shelves, writes one
  receipt, one `Served`, and a restock request for each shelf that is now
  empty.

Seed every shelf with two items and `Orders = [Ada: milk, bread]`. Step:
one receipt, `Served = [Ada]`, no restock. Seed a shelf with one item and an
order that takes it. Step: that shelf's restock place gets one token, the
others none. With `Orders` empty, `Serve` is not fireable however full the
shelves are. The test exists to prove arity, so it should declare the
fourteen and thirteen as plainly as possible, in whichever form section 4
settles on.

## 3. What exists today (anchors for the implementer)

petritype-rs (c93ec03):

- `petritype/src/arity.rs`: `for_each_arity!` invokes a macro once per arity
  0..=12 with `(type, value, index)` triples; its module doc states the cap
  and the reason.
- `petritype/src/edge.rs:743` `for_each_arity!(impl_edges)`: `InputEdges`
  for tuples of `InputEdge`s (`impl_edges` at ~530: `describe` into
  `Vec<InputSpec>`, the consumed/borrowed values handed to the body).
  `InputEdge` is "one input position of a transition"; `Param<T>` implements
  it without naming a place (module doc, lines 21-27).
- `petritype/src/body.rs:527` `for_each_arity!(impl_body)` (`TransitionBody`
  over the edge tuple, the output tuple and a `Marker` for the positional or
  routed return form) and `:573` `for_each_arity!(impl_outputs)`
  (`OutputPlaces`; positional slot *i* of the returned tuple goes to output
  place *i*, each slot `T`, `Option<T>` or `Vec<T>`; DESIGN §4.5).
- `petritype/src/builder.rs:736` `NetBuilder::transition<Edges: InputEdges,
  Outputs: OutputPlaces, Body: TransitionBody<Edges, Outputs, Marker>>`.
  Build checks that matter here: `DuplicateInputPlace` (one edge per place
  per transition), the arena/detachable rules for mutating edges, the
  trybuild UI tests under `petritype/tests/ui/` (what a wrong input tuple
  reports).
- DESIGN §4.4 (edges), §4.5 (outputs: positional and routed), §5 lines
  529-535 (the cap and the trailing-comma rule), §19 line 1928 (the trait over
  tuples compared with Bevy `IntoSystem` and axum handlers, which stop at 16).
- The wire and the viewer list edges, so neither has a cap:
  `TransitionInfo::inputs`, `EdgeInfo`, PROTOCOL.md §5.1 `edges[]`,
  petritype-bevy `reader.rs`/`render.rs`.

## 4. Requirements

**R1. Grouped inputs: one position that holds several edges.** A wrapper
`Group<G: InputEdges>(G)` implements the one-position trait (`InputEdge`),
as `Param<T>` does, so `(TakeEdge(a), Group((SurveyEdge(b), SurveyEdge(c),
SurveyEdge(d))), Param::<P>::new())` is a three-position input tuple whose
body receives `(A, (Vec<B>, Vec<C>, Vec<D>), P)`. `describe` flattens the
group's edges into the transition's `InputSpec` list in order; `launch`,
restore and the index treat each inner edge exactly as if it were declared
flat; `DuplicateInputPlace` is checked across groups; `TransitionInfo::inputs`,
`EdgeInfo`, the topology and the wire see only the flat list, so nothing
downstream changes. Groups may nest. The body's parameter for a group is a
plain tuple, which a closure destructures: `|order, (b, c, d): (Vec<B>,
Vec<C>, Vec<D>), prices|`.

**R2. Grouped outputs: one slot that holds several places.** The same
wrapper over output places, `Group((p, q, r))`, is one `OutputPlaces` slot
whose positional value is a tuple `(X, Y, Z)` with each element `T`,
`Option<T>` or `Vec<T>` for its place, as the flat form has it. The flat
list of output places is what the topology and a routed body's target set
see; a `Distribution` may push to any place inside a group by its
`PlaceRef`. Nesting as for inputs.

**R3. Raise the flat cap to 16.** Independent of R1/R2: `for_each_arity!`
to 0..=16, which is where Bevy and axum stop, so a transition with thirteen
to sixteen positions needs no group. Measure the compile-time cost of the
crate before and after (R3 is cheap in `cargo build` terms only if it is;
report the numbers) and keep 12 if 16 costs more than a few seconds.

**R4. Diagnostics.** A `Group` whose inner tuple is not a tuple of edges, or
a body whose parameter for a group is not the matching tuple, gets a UI test
(`tests/ui/`) with a readable message, as `fail_not_an_edge` and
`fail_drain_without_vec` have.

**R5. Docs.** DESIGN §4.4 and §4.5 gain the group form with one example
each; §5's cap sentence becomes "16 positions, each possibly a group"; §18
a slice entry; the `shop` or `wildlife` example need not change. PROTOCOL.md
needs nothing: the wire never saw tuples.

**R6. Python.** Nothing: lists have no cap. Say so in the As-built.

**R7. Non-goals.** A struct-of-edges derive macro (`#[derive(Inputs)] struct
ShotInputs { baton: TakeEdge<..>, ... }` with the body receiving a struct of
values) would read best for the largest transitions but needs a proc-macro
crate; it is decision D3, not asked for now. No change to the "one edge per
place per transition" rule. No dynamic (erased, untyped) transition form.

## 5. Suggested order

(1) R3 alone, measured, as its own commit: it may be all that is needed for
a while. (2) R1 with the describe/launch/restore paths and the UI tests.
(3) R2. (4) R5. Python: nothing.

## 6. Expected results

The §2 net builds and runs with fourteen inputs and thirteen outputs, in
whichever of the two forms (flat at 16, or groups) the implementer chooses
to write it, and ideally both: `Serve` fires on one order, writes one
receipt and one `Served`, restocks only an emptied shelf, and is not
fireable while `Orders` is empty whatever the shelves hold. The topology
lists fourteen input edges and thirteen output edges for it, with no trace
of grouping. `assert_no_source_transitions` still sees the one `TakeEdge`.
A failing `Serve` body restores the order (a `Take`) and nothing else (the
surveys consumed nothing). A `Group` of one edge behaves like the edge.

## 7. Decisions (defaults the implementer may take if the owner says nothing)

- **D1 Form.** Groups (R1, R2) plus a raised flat cap (R3). Alternatives:
  raise the cap only (to 16, 24 or 32; each arity is one impl per trait,
  and 32 would be 96 impls of some size); groups only (keep 12, since a
  group makes any arity reachable).
- **D2 The flat cap.** 16. Alternative: 24, if compile time allows.
- **D3 A derive macro for a struct of inputs.** Later, if the groups read
  badly in practice.
- **D4 Name.** `Group`. Alternatives: `Bundle`, `Edges`, `Many`.

## 8. Rules for the implementing agent

Work in the library repo only: do not touch `~/work/games/**` or
`~/work/petritype-bevy/TODO_TEMP.md`. One branch per phase, tests green at
each, the owner merges. When done, append an "As built" section to THIS file
naming what differs from sections 2, 4 and 6 and why, with the commits and
the compile-time numbers from R3.

## 9. Prompt

> Implement `~/work/petritype-rs/dev-docs/TRANSITION_ARITY.md`: let a
> transition declare more than twelve inputs or outputs, by raising the flat
> tuple cap to 16 (measured) and by adding `Group`, a wrapper that makes a
> tuple of edges one input position and a tuple of places one output slot,
> flattened everywhere downstream. Read the whole document first, then the
> anchors in section 3. Take the defaults in section 7 unless told otherwise.
> Build the section 2 net as the acceptance test and check section 6. Append
> an "As built" section to the document when finished. Do not touch
> `~/work/games/**` or `~/work/petritype-bevy/TODO_TEMP.md`.

## As built (R1, arity-16)

Built 2026-10-15 on branch `arity-16`, as one commit on 3e76c11: the commit
that adds this section (a commit cannot name its own hash; `git log
arity-16` has it). Of this document only R3 is built, under its new name,
R1 of `BINDING_BY_NAME.md`; groups (R1, R2 here) were replaced by binding
by name and are not built.

**What changed.** `for_each_arity!` in `petritype/src/arity.rs` now runs
its macro for arities 0 to 16, the four new arities written out like the
twelve before them. The edge, body and output macros are untouched: each
emits its impls for four more tuple sizes, twelve impls per arity in all.
Every doc that gave twelve as the cap now gives sixteen: the module doc of
`arity.rs` (with the reason, and why it was 12), DESIGN §5's conventions
bullet, DESIGN §16's "trait over tuples" row, the docs of `InputEdges`,
`OutputPlaces`, `TransitionBody` and the `body` module, and `RouteTable`'s
note on why a vector beats a map. Twelves that are not the cap stay: slice
2's `a_body_may_have_no_inputs_or_twelve` and `..._outputs_or_twelve` still
pass and still mean something, twelve being inside the range now rather
than its end, and the dev-docs proposals describe c93ec03 as it was. No
API, behaviour, wire or diagnostic changed; no trybuild `.stderr` moved.

**Tests.** `petritype/tests/arity16.rs`: the §2 till written flat, one
transition `Serve` with fourteen input positions and thirteen outputs, and
the §6 checks (one order makes one receipt and one `Served` and restocks
nothing; a shelf the order empties is restocked and no other; `Serve` waits
for an order whatever the shelves hold; an out-of-stock order fails the
body and `restore_tokens_on_failure` puts back the order alone; the
topology in declaration order). Beside it a smoke test at the cap: sixteen
`TakeEdge`s in, sixteen places out, the body returning its inputs reversed
so that positions 12 to 15 must land where they were declared.

**One difference from §6.** "The topology lists fourteen input edges"
counts the `Param<Prices>` as an edge. It is an input position but not an
edge of the net (DESIGN §10): the topology lists thirteen input edges (one
take, one snapshot, eleven surveys) and the price list under `params`. The
test asserts that, and that the two add up to fourteen.

**Compile time.** `cargo clean -p petritype`, then `cargo build -p
petritype --features serde` (debug, dependencies already built), wall
clock in seconds; rustc 1.98.1, native arm64, 14 cores. The machine was
shared with another agent's builds (load average 4 to 15), so a second
round ran before and after back to back under the same load. The tests
build is `--tests` after the same clean, and was taken before
`arity16.rs` existed so that both sides compile the same tests.

| build | cap 12 (before) | cap 16 (after) |
|---|---|---|
| library, round 1 | 1.33, 1.30, 1.31 | 1.38, 1.39, 1.39 |
| library, round 2 | 1.33, 1.32, 1.32 | 1.46, 1.42, 1.43 |
| library, mean of six | 1.32 | 1.41 |
| `--tests`, rounds 1 and 2 | 3.88, 4.16 | 4.50, 4.01 |

The library pays about 0.1 s (7%) for the four arities; the tests build
moves less than its own noise. R3's condition ("keep 12 if 16 costs more
than a few seconds") is met with room to spare. `BINDING_BY_NAME.md` §3
gives 2.5 s for a full recompile; on this toolchain and machine it is
1.3 s.

**Python.** Nothing: its edges are lists and have no cap.
