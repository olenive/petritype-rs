# Binding a transition's inputs and outputs by name

Written 2026-10-15 by the planning thread for the petritype owner, after the
review of `TRANSITION_ARITY.md` (the game agent's proposal) and the owner's
questions that followed. It takes that document's problem (§1) and its cap
requirement (its R3), and replaces its `Group` (its R1, R2, D1, D4) with a
form the owner chose instead: a transition's inputs and outputs bound to a
function's fields **by name**, with the function's structs free of places
and edge kinds so that one function serves many nets. It is meant to be
enough for another agent to implement from alone. Anchors are at
petritype-rs c93ec03.

## 1. The problem, and the shape of the answer

Today a transition binds its body to the net **by position**: the input
tuple is the body's parameter list in order, and a positional return is the
output tuple in order. Two things go wrong at the sizes the game has reached
(`TRANSITION_ARITY.md` §1: twelve to sixteen positions each way):

- Every insertion shifts every later parameter, and two same-typed
  positions swap silently.
- A struct of edges, the obvious fix, couples the function to *how* each
  place is read, so the function cannot be reused in a net that reads
  differently. The owner's requirement: a library of domain functions,
  each wired into several nets in slightly different ways.

The output side already has the answer in one form: DESIGN §4.5's output
distribution function exists "so that domain functions stay unchanged".
The input side has it less visibly: a plain `fn` with typed parameters is
a valid body, and each net picks any edge kind whose argument type fits
each parameter. Binding by name keeps that property and adds names.

Three ways to bind a function to a net, both sides, is the vocabulary
this document uses and DESIGN should adopt: **by position** (tuples, as
today), **by name** (this document), **by type** (routed outputs, as
today). Inputs and outputs are both *edges with a kind*; the input side
has eight kinds, the output side one (put) today, with more possible
later (replace, put at the front, upsert by key, broadcast). Nothing here
adds an output kind, but the output wiring is shaped so that one can be
added as a struct, exactly as drain and survey were added on the input
side. The word "slot" is retired from the prose: say "return position".
The wire's `slot` field (PROTOCOL.md §5.1) keeps its name.

Python already binds inputs by name (`ArgumentEdgeToTransition.argument`)
and outputs by type or `return_index`; this brings the Rust runtime to the
same shape. Python needs nothing.

## 2. The example

A pricing library and two nets that wire it differently. The library side
names no place and no edge kind:

```rust
use petritype::{Inputs, Outputs};

#[derive(Inputs)]
pub struct Pricing {
    pub order: Order,
    pub promos: Vec<Promo>,
    pub prices: Prices,
}

#[derive(Outputs)]
pub struct Priced {
    pub order: Order,        // one token
    pub note: Option<Note>,  // maybe one
    pub restock: Vec<Item>,  // zero or more
}

pub fn price(p: Pricing) -> Priced {
    let discount: f32 = p.promos.iter().map(|promo| promo.rate).sum();
    Priced {
        order: p.order.priced(&p.prices, discount),
        note: (discount > 0.0).then(|| Note::applied(discount)),
        restock: vec![],
    }
}
```

The derives generate `PricingEdges` and `PricedEdges`: one field per value
field, each accepting any edge that *reads* that field's type (inputs) or
any output edge that *accepts* it (outputs). The shop net:

```rust
b.transition(
    "Price Order",
    PricingEdges { order: TakeEdge(orders), promos: SurveyEdge(promos), prices: Param::new() },
    PricedEdges  { order: priced, note: log, restock },
    price,
);
```

A wholesale net, same function: trade orders by key, each promotion used
once, prices read off a place, different output places:

```rust
b.transition(
    "Price Trade Order",
    PricingEdges {
        order:  TakeWhereEdge(orders, Filter::new().key_in("tier", ["trade"])),
        promos: DrainEdge(promos),
        prices: SnapshotEdge(price_list),
    },
    PricedEdges { order: invoiced, note: audit, restock: warehouse },
    price,
)
.restore_tokens_on_failure();
```

An input struct is **one position**, so it mixes with the tuple form and
two library structs sit side by side:

```rust
b.transition(
    "Price and Stamp",
    (PricingEdges { order: TakeEdge(orders), promos: SurveyEdge(promos), prices: Param::new() }, TakeEdge(stamps)),
    PricedEdges { order: priced, note: log, restock },
    |p: Pricing, stamp: Stamp| price(p).stamped(stamp),
);
```

An output struct is the **whole return**, one of the return forms beside
positional and routed; it does not sit inside an output tuple.

The acceptance tests are this library in both nets, and the till of
`TRANSITION_ARITY.md` §2 written by name with its fourteen inputs and
thirteen outputs (and, after R1, also flat at sixteen).

## 3. What exists today (anchors)

- `petritype/src/arity.rs`: `for_each_arity!`, 0..=12, with the cap's
  reason in the module doc.
- `petritype/src/sealed.rs`: the engine halves. `EdgeSealed` is strictly
  one position to one erased input: `type Arg<'a>` (a generic associated
  type, there for `MutateEdge`'s `&'a mut T`), `fn extract(&mut
  ErasedInput) -> Arg<'a>`, `fn spec(&self) -> InputSpec`.
  `EdgeTupleSealed::describe(&self, &mut Vec<InputSpec>)`.
  `PlaceTupleSealed::{describe, describe_types}`, `PositionalSealed<Out>`,
  `OutcomeSealed<Out, M>`, `BodySealed<E, O, M>`.
- `petritype/src/edge.rs`: `InputEdge` (line ~531, with its
  `on_unimplemented` listing the nine forms), the nine `EdgeSealed` impls
  (`edge_spec!`), `InputEdges` (~725) and `impl_edges` (~733).
  `petritype/src/param.rs:127`: `Param<T>` is the one implementor naming no
  place; it still describes one spec and takes one erased input.
- `petritype/src/body.rs`: `IntoTokens<T>` (111) for `T` / `Option<T>` /
  `Vec<T>`; `OutputPlaces` (156) over tuples of `PlaceRef<T>`;
  `PositionalOutputs` (173); `TransitionBody` (344); `impl_body` (370–527),
  six impls per arity, each pulling `args.next()` once per position;
  `impl_outputs` (531–573), four impls per arity, pushing
  `OutputTarget::Index($i)` with a literal index. The `marker` module's
  tags (`Plain`, `Fallible`, `Positional`, `Routed`) keep the return forms
  from overlapping (DESIGN §16).
- `petritype/src/routing.rs`: `RoutedOutput`, the by-type form; `Option`
  (304) and `Vec` (322) of a routed value. The public `petritype::routing`
  module exists so that `#[derive(Routed)]`'s generated code can name the
  trait: the pattern to copy for the engine traits the new derives need.
- `petritype-derive/src/lib.rs`: `#[proc_macro_derive(Routed)]` (62), with
  its refusals (named fields, unions, lifetimes) as the style for new
  refusals. `syn`, `quote`, `proc_macro2` already depended on.
- `petritype/src/builder.rs:736` `NetBuilder::transition<Edges: InputEdges,
  Outputs: OutputPlaces, Body: TransitionBody<Edges, Outputs, Marker>>`:
  loops over the flat `described` specs into `edges`, `filters`, `params`,
  `args` (`ArgSource`), so anything that describes flat is invisible from
  here on. `builder.rs:1082` `DuplicateInputPlace` over the flat list.
- `petritype/src/transition.rs`: `guard` (294), `priority` (315),
  `restore_tokens_on_failure` (402). `petritype/src/distribution.rs:322`
  `WithOutputDistribution`.
- `petritype/tests/ui/`: trybuild, `fail_*` with `.stderr`, `pass_*`.
- DESIGN §4.4 (edges, line 253), §4.5 (outputs, 373), §5 (builder, 476;
  the cap and trailing-comma rule at ~531), §16 (the "trait over tuples"
  row, ~1925), §18 (slices, 1987). PROTOCOL.md §5.1 (`../petritype`,
  lines ~196–206): `kind?` on input edges, `slot?` on output edges.
- Compile time, measured 2026-10-15: a full recompile of `petritype`
  (debug, `serde`) takes 2.5 s; each arity costs twelve impls.

## 4. Requirements

**R1. Cap to 16.** `for_each_arity!` to 0..=16. Measure the crate's full
recompile before and after and put the numbers in the As-built. Update the
module doc, DESIGN §5 and §16. Own commit, own branch (`arity-16`), so the
owner can merge it alone.

**R2. An input position may bind several edges.** The engine change
underneath the derive. `EdgeSealed` describes into a vec (`describe(&self,
&mut Vec<InputSpec>)`, the nine kinds and `Param` pushing one) and extracts
from a cursor over the erased inputs (`extract(cursor) -> Arg<'a>`, the
nine kinds and `Param` taking one). `impl_body` hands the cursor to each
position instead of calling `args.next()` itself. Launch, restore, the
index, `DuplicateInputPlace`, `TransitionInfo::inputs`, the topology and
the wire see only the flat list, in declaration order, and do not change.
A nested tuple of edges may fall out of this as an `InputEdge`; it is
**not** public API (no docs, no example; an engine test is fine).

**R3. `Reads<V>`: the public bound an input wiring field carries.**
Implemented in the core, one line per kind: `TakeEdge<T>`,
`SnapshotEdge<T>`, `TakeWhereEdge<T>` and `Param<T>` read `T`;
`TakeAllEdge<T>`, `DrainEdge<T>` and `SurveyEdge<T>` read `Vec<T>`;
`MutateEdge<T>` and `MutateWhereEdge<T>` read `&'a mut T`. How `Reads<V>`
relates to `Arg<'a>` is the implementer's: the acceptance is that a
generated wiring struct can bound each field so that (a) any kind reading
the field's type is accepted, (b) the extracted value assigns into the
field, and (c) a wrong kind fails to compile at the literal with a message
that names the field and the type it must read. The one known risk is the
mutate kinds: the bound must relate a `&'a mut T` field to the cursor's
lifetime (an `for<'a>` equality on the generic associated type, or a
lifetime on `Reads` itself). If that compiles on stable 1.98, mutate fields
are in; if it does not within reasonable effort, v1 takes owned fields
only, a UI test says so in its message, and the As-built records why.

**R4. `#[derive(Inputs)]`.** In `petritype-derive`, re-exported by
`petritype` as `Inputs`. On a struct with named fields and at most one
lifetime parameter (for `&mut` fields); refused, with messages in the
style of `Routed`'s, on enums, unions, tuple structs, type parameters, and
a struct with no fields. Generates:

- the wiring struct `<Name>Edges<F0, F1, ..>` with the same field names,
  one type parameter per field, bounds `Fi: Reads<field type>`, `pub` as
  the value struct is; the name overridable with `#[binding(edges =
  "Other")]`;
- the engine impls for the wiring struct: `InputEdge` with `Arg<'a>` the
  value struct (with its lifetime, when it has one); describe in field
  order; extract building the value struct field by field from the cursor.

Field order is the wire order of the edges. The engine traits the
generated code names live in a public, `#[doc(hidden)]` module
(`petritype::binding`), as `petritype::routing` does for `Routed`.

**R5. Named outputs.** `Accepts<V>` on the output side, implemented in the
core for `PlaceRef<T>` with `T`, `Option<T>` and `Vec<T>`, through
`IntoTokens`; this is where a future output kind (`ReplaceEdge<T>` and the
like) will add its impls, and nothing here adds one. `#[derive(Outputs)]`
on the returned value struct, same refusals as R4 (no lifetime at all: a
token is owned), generates `<Name>Edges<F0, ..>` with bounds `Fi:
Accepts<field type>`, an `OutputPlaces` impl for it (describe places and
types in field order), and the return-form impl for the value struct: a
new marker `Named` beside `Positional` and `Routed`, so that the three
forms do not overlap; tokens go to `OutputTarget::Index(i)` by field
order. `Result<Value, E>` wraps it as it wraps the other forms. A named
return reports `positional() == true` and the runner writes `slot` by
field order: the order is real and the wire has no third value (D4). An
output struct is the whole return, never a position inside an output
tuple.

**R6. A lone struct without the tuple (optional).** `b.transition("x",
PricingEdges { .. }, PricedEdges { .. }, price)` without `( .. ,)`: a
blanket making any `InputEdge` an `InputEdges` of one, with the body
forwarded. Coherence should allow it (tuples are not `InputEdge`). Attempt
it; if coherence or the diagnostics suffer, drop it and say so.

**R7. Diagnostics.** UI tests: a field wired with a kind of the wrong
shape (`TakeEdge` on a `Vec<Promo>` field); an output field wired to a
place of another type; a derive on a struct with a type parameter; a
missing field (the ordinary error, as a `pass`/`fail` pair showing it is
readable); mutate field pass or documented fail (R3). The
`on_unimplemented` texts on `InputEdge`, `InputEdges` and the output
traits gain the named form.

**R8. Docs.** DESIGN §4.4 and §4.5: the three bindings vocabulary, the
named form with one example each, the statement that outputs are edges
with one kind today; §5: the cap sentence (16), the named form beside the
tuple form, "slot" replaced by "return position" throughout the prose;
§16: the rows; §18: a slice entry; README: a row and a short example.
`examples/`: the pricing library and its two nets as a new example
(`examples/pricing.rs` with `examples/pricing/`), small, no new
conformance net, no PROTOCOL change. Python: nothing (say so).

**R9. Non-goals.** `Group` or nested tuples as public API. Any new output
kind. Arrays of edges `[E; N]` as a position (maybe later). An output
struct as a position inside an output tuple. A derive on enums. Changes to
the wire, the viewer, the runner or the protocol transcripts.

## 5. Order

Branch `arity-16`: R1 alone. Branch `binding-by-name` from main (not from
`arity-16`; the two are independent and the owner merges each): (1) R2 with
the engine test and the full suite green; (2) R3 and R4 with the pricing
example's shop net and the UI tests; (3) R5 with the wholesale net and the
till; (4) R6; (5) R8. One commit per step, tests green at each.

## 6. Expected results

- The pricing library's `price` is wired into the shop net and the
  wholesale net unchanged, with the kinds and places of §2, and both nets
  run: an order with a running promotion comes out priced with a note; a
  drained promotion is gone after the wholesale firing and is restored by
  `restore_tokens_on_failure` when the body fails; the mixed form compiles
  and fires.
- The till builds by name with fourteen inputs and thirteen outputs and
  behaves as `TRANSITION_ARITY.md` §6 says; with R1 merged it also builds
  flat at sixteen.
- `TransitionInfo::inputs`, `EdgeInfo` and the topology list the edges in
  field order with no trace of structs; `positional` is true for a named
  return and the runner's `slot` runs 0.. in field order; the protocol
  transcripts still replay (the Rust gate reads `../petritype/tests/protocol`).
- `assert_no_source_transitions` still counts only the always-consuming
  edges inside a struct.
- Every `fail_*` UI test has a message a user can act on.
- The compile-time numbers for R1 are in the As-built.

## 7. Decisions (defaults the implementer takes if the owner says nothing)

- **D1** Derive names `Inputs` and `Outputs`; generated wiring suffix
  `Edges`. Alternatives: `Wiring`, `Binding`.
- **D2** Trait names `Reads<V>` and `Accepts<V>`. Alternative: `InputOf`,
  `OutputOf`.
- **D3** Mutate fields in v1 if the bound compiles on stable, else owned
  only (R3).
- **D4** A named return is positional for the read API and the wire,
  `slot` by field order.
- **D5** The lone-struct form (R6) is attempted and dropped if it costs
  coherence or diagnostics.
- **D6** The hidden engine module is `petritype::binding`.

## As built

Slice 23, 2026-10-15, branch `binding-by-name` of petritype-rs, cut from
main at 3e76c11: R2 to R9, in the five commits of §5's second branch. R1
(the cap to 16, its compile-time numbers and DESIGN §5's cap sentence and
§16's arity row) is the `arity-16` branch's and is not here; where these
docs need the cap they say "the tuple cap (§5)". DESIGN §4.4 ("Binding by
name"), §4.5 ("Named"), §5, §16 and §18 (slice 23) describe what was built;
this section names what differs from §2, §4 and §6, and why.

**§2, the example.** Built as `petritype/examples/pricing.rs`, with the
library in `examples/pricing/library.rs` and the nets in
`examples/pricing/nets.rs`; it needs no feature and is no conformance net.
Four differences from the text:

- Every struct of edges is written inside the input tuple,
  `(PricingEdges { .. },)`, because R6 was dropped (below).
- `Promo` carries `percent: u32` rather than `rate: f32`, so that the tests
  compare totals exactly; `restock` holds the items the price list does not
  carry, which the order is not charged for, rather than always `vec![]`, so
  that the `Vec` field carries tokens in a test.
- The mixed form is a second transition of the shop net, `Price and Stamp`
  (gift orders beside a `Stamps` place), not a third net; it writes through
  the same `PricedEdges` value as `Price Order`, a struct of outputs being
  `Copy` when its places are.
- The wholesaler is built by `build_wholesale_with(prices, body)`, generic
  over the body, with `build_wholesale` passing `price`: §6 asks for a
  failing body to show the restore, and `price` cannot fail. The wiring's
  types are named in the example (`WholesaleInputs`, `WholesaleOutputs`)
  to show that a wiring is a type like any other. The price list is a
  token seeded on `Price List`.

**§4, the requirements.**

- **R2** as written, with three choices. `EdgeSealed::spec` became
  `describe(&self, &mut Vec<InputSpec>)` and `extract` takes an
  `ErasedCursor`, a wrapper over the `IterMut` the wrapper used before, so
  `&mut` arguments can still be alive together. The wrapper calls
  `cursor.finish()` after the body, which panics, and so fails the firing,
  if a position took fewer inputs than it described: a token never handed
  to the body would otherwise be lost silently. `ErasedInput` became
  crate-private, no sealed signature naming it any more. No nested tuple
  is an `InputEdge`: it would collide with R6's blanket, and nothing needs
  it; the engine test binds two edges through a hand-written `Pair`
  position inside `body.rs`'s tests (describe order, extraction with a
  borrow, the flat read API, restore, `DuplicateInputPlace` and the source
  rule).
- **R3.** `Reads<V>` is as specified, one impl per kind (written out, not
  generated, so that rustc's "but `Reads<Promo>` is implemented" help shows
  the impl rather than a macro). Its relation to `Arg<'a>` is a hidden
  subtrait, `binding::Extract<'a, V>: Reads<V>`, implemented beside each
  `Reads` impl: `TakeEdge<T>: Extract<'a, T>` for every `'a`,
  `MutateEdge<T>: Extract<'a, &'a mut T>`. The wiring struct is bounded by
  `Reads<V>` (`for<'a> Reads<&'a mut T>` for a borrowing field), where the
  message is written; its impls ask `for<'a> Extract<'a, V>`, which ties
  the field's lifetime to the cursor's. A blanket `Extract` over
  `EdgeSealed<Arg<'a> = V>` compiled too, but a wrong kind then also
  produced an E0271 naming `EdgeSealed`.
- **D3: mutate fields are in.** The bound compiles on stable 1.98, no
  `unsafe`: `pass_inputs_mutate.rs` and `tests/binding.rs` fill
  `&'a mut T` fields from a `MutateEdge` and a `MutateWhereEdge`, with a
  function and with a closure taking `Tally<'_>`.
- **R4.** The derive implements `EdgeSealed` and `InputEdge` directly for
  the wiring, naming `EdgeSealed` through `petritype::binding`, rather than
  the core holding a blanket over a hidden "wiring" trait: with such a
  blanket in reach, rustc explained the plainest mistake, a place handle in
  the tuple, as "`PlaceRef<u32>: InputWiring` is not satisfied". So the
  seal has one opening, the hidden module, which re-exports `EdgeSealed`
  (and, for R5, `PlaceTupleSealed`). Beyond the spec: the derive also
  refuses a `where` clause; besides `#[binding(edges = "..")]` it reads
  `inputs = ".."` and `outputs = ".."`, so that one struct can derive both
  bindings without their wirings colliding; the wiring derives `Clone`,
  `Copy` and `Debug`, and its fields take the struct's visibility (a library
  struct with private fields can still be wired); a type parameter named
  `F0` in a field's type is dodged (`_F0`). The generated items are spanned
  at the struct's name and each bound at its field, so that "required by
  this bound" points at the field, and no note points into the derive.
- **R5** as written. `Accepts<V>` is one blanket impl for `PlaceRef<T>` over
  `IntoTokens<T>`, marked `#[diagnostic::do_not_recommend]` so that a wrong
  place is reported on `Accepts` ("`PlaceRef<Order>` cannot take a field of
  type `Option<Note>`") rather than on `IntoTokens`. The return-form impl
  is through a hidden `binding::NamedOutputs<Outputs>`, which the derive
  implements for the struct and its own wiring only, under the marker
  `Named`; the fields push to `OutputTarget::Index(i)` through an `Emit`
  sink that counts them. The ambiguity with `Routed` was real (both impls
  applied to a struct deriving both); the routed form now asks
  `Outputs: TupleOfPlaces`, a sealed marker of the tuples of place handles,
  so the struct is routed whole by a tuple and bound by name by its own
  wiring, tested both ways. Nothing existing changes, a tuple having been
  the only `OutputPlaces` until now.
- **R6 dropped (D5).** The blanket is coherent and a lone struct then
  compiles, but fifteen UI cases got worse: `(inbox,)` was reported as "a
  transition's inputs must be a tuple of input edges", a closure's wrong
  parameter or arity lost rustc's expected/found signatures (E0631 and
  E0593 became a generic "body parameters do not match"), and every
  mismatch of the outputs (a wrong number or type of return positions, an
  `Err` type that is not an error, a wrong place in a struct of outputs)
  was reported as a mismatch of the inputs. Two impls in reach of every input tuple is the
  cause, and `#[diagnostic::do_not_recommend]` does not remove it. A
  derive-generated conversion trait (`IntoInputEdges`, the struct
  converting itself to its one-tuple) was tried in a prototype: it kept
  those messages but moved the place handle's error from the element to
  the call and named the hidden trait for a wrong field, so it was not
  pursued. `fail_inputs_without_tuple.rs` pins what a lone struct is told
  (the note, and rustc's own "use a unary tuple instead").
- **R7.** UI cases: `fail_inputs_wrong_kind`, `fail_inputs_type_parameter`,
  `fail_inputs_missing_field` with `pass_inputs_every_field`,
  `pass_inputs_mutate`, `fail_outputs_wrong_place`, `fail_outputs_in_tuple`
  (R9's non-goal, which the `OutputPlaces` note answers) and
  `fail_inputs_without_tuple`; the derives' other refusals are unit tests in
  `petritype-derive`. The `on_unimplemented` texts of `InputEdge`,
  `InputEdges`, `TransitionBody`, `OutputPlaces`, `PositionalOutputs` and
  `IntoOutcome` gained the named form; `Reads`, `Extract` and `Accepts`
  are new. A wrong kind is reported four times (the literal, the call's
  generic argument, `InputEdges`, the body), each with the same message;
  the first points at the field.
- **R8.** DESIGN as listed, with "slot" made "return position" in §4.5 and
  in §16's `IntoTokens` row; §5's prose had no "slot" left outside the cap
  sentence, which is untouched, and the wire's `slot` keeps its name.
  PROTOCOL.md: nothing. **Python: nothing**; it binds inputs by name
  already and outputs by type or `return_index`, and the wire did not change.
- **R9** held: no `Group`, no public nested tuples, no new output kind, no
  arrays of edges, no struct of outputs inside a tuple, no derive on enums,
  no change to the wire, the viewer, the runner or the transcripts.

**§6, the expected results.** All hold, with two notes. The till is built
by name in `tests/binding.rs`, one struct of fourteen inputs and one of
thirteen outputs, past the tuple cap without R1; it builds flat at sixteen
only on `arity-16`, so that half is that branch's. Its fourteen inputs are
thirteen edges and the `Param` price list, so `TransitionInfo::inputs` and
the topology list thirteen input edges and one parameter, not "fourteen
input edges" as `TRANSITION_ARITY.md` §6 counts them. The runner's `slot`
for a named return is checked by a unit test in its `translate` module
(field order, 0 and 1); the shared transcripts replay unchanged through the
suite's gate.

**Commits**, on `binding-by-name`: 8850d43 (R2), debe8e0 (R3 and R4),
f7c4521 (R5), 1848ea1 (R6, attempted and dropped), and the documentation
commit that adds this section (R8).

**2026-10-16: the wholesaler surveys.** The owner asked for promotions never
to be drained, so the wholesaler's `promos` field is a `SurveyEdge(promos)`
(`WholesaleInputs` names `SurveyEdge<Promo>`): it reads every promotion and
leaves them running, as the shop does, and the nets still differ in the
order (a filter) and the prices (a token on a place). The failing-body test
now shows the order restored and the promotions never taken; §2 and §6 above
are left as written. The drain edge's example is the traffic light
(`petritype/examples/traffic_light.rs`).
