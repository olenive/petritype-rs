# Spec: subnet places, port places and the stepping rule

Draft for review, 2026-11-21. Nothing here is built. The concept, with the
reasons for each choice, is `subnet-place-concept.md`; this document says how
it is to be built, in both runtimes, on the wire and in the viewer. Method and
field names here are proposals until §10 is answered.

## 0. The principle

A place of a net may hold a net of its own: a **subnet place** holds a
**subnet**. The subnet is separate and isolated. The net around it reaches it
only through **port places**, which are ordinary places of the subnet that its
definition marks as ports, and which the outer net also lists among its own
places. An edge of the outer net to or from a port place is an ordinary edge.

Between a net and the nets inside it, the order of firing is one rule per
place that holds nets, a subnet place or a place whose tokens carry nets:
inner first (the default), take turns, outer first, or passive.

Three things follow from "prefer what can be modelled in the net over a rule
hidden in the engine", and are not repeated below: there is no turn count, no
handover firing, and no way for one edge to cross two borders.

## 1. The rules, for both runtimes

### 1.1 Subnet places and port places

- A subnet place holds exactly one subnet, given when the outer net is built.
  It holds no tokens of its own, and no edge may name it.
- The subnet's definition marks some of its places as ports. A port place must
  be an ordinary place or a place whose tokens carry nets. It may not be a
  subnet place.
- For each port place the outer net has a place of its own, named
  `<subnet place>/<port>`, for example `Level 3/Lobby`. It has the port's
  type, keys and style. The two are one place under two names: one queue of
  tokens, one set of token ids, minted by the subnet.
- The outer net's edges use that place like any other: every input edge kind,
  and every way of choosing an output (by position, by type, by an output
  distribution function). Guards, queries and external input work on it too.
- Nothing else of the subnet can be named by the outer net's transitions. The
  program itself may still read a subnet, and may step or change it inside a
  scope, as it may a token's net today.

### 1.2 Tokens, events, failure

- A token crosses the border only through a firing of an outer transition, so
  it gets a new identity as in any firing.
- A change is told once, by the net whose transition made it. An outer firing
  that puts into `Level 3/Lobby` reports that place. An inner firing that
  takes from `Lobby` reports `Lobby`, lifted into the outer log as inner
  events are today.
- A failed outer firing restores what it took from a port place as from any
  place. A failed inner firing is reported as an inner failure is today.

### 1.3 The stepping rule

Each place that holds nets has one rule. The default is inner first.

| Rule | Wire and Python name | Meaning |
|---|---|---|
| inner first | `inner_first` | the nets in this place fire until they are at rest before the net around them fires |
| take turns | `take_turns` | the net around them and each net in this place fire in turn |
| outer first | `outer_first` | the nets in this place fire only when the net around them and its turn-takers have nothing to fire |
| passive | `passive` | deep stepping never enters this place; the program or a body steps its nets |

One **deep step** of a net `N` fires at most one transition somewhere in `N`
or below it. It tries, in this order, and stops at the first that fires:

1. Every net in a place of `N` whose rule is inner first, in walk order: a
   deep step of that net.
2. The ring of `N`: `N` itself and every net in a place of `N` whose rule is
   take turns, in walk order. The ring starts after the member that fired
   last and goes round once. `N`'s turn is one firing chosen by `N`'s own
   selector. Another member's turn is a deep step of that net.
3. Every net in a place of `N` whose rule is outer first, in walk order: a
   deep step of that net.

Walk order is today's: places in declaration order, a place's tokens oldest
first, a token's nets in their declared order. A subnet place counts as one
net at its place's position.

What changes from today:

- The rule is per place and applies at every level. Today one policy on the
  outermost net governs the whole tree, and every level below fires its own
  net before the nets inside it.
- Inner first is new.
- The ring holds one level. Today's interleaved ring holds every net of the
  tree. A net one level deep, with every place on take turns, steps exactly
  as it does under today's interleaved policy.

What does not change: a net is quiescent deep when it and every net in its
non-passive places are at rest, and a plain step fires only the net's own
transitions.

**While an outer firing that touches a port place is in flight** (launched and
not yet distributed), the subnet behind that port takes no turn. Firing in
parallel across levels is out of scope: one firing at a time across the tree,
except that an outer body may still be running on its own thread, as now.

### 1.4 Definitions and saving

- A subnet is any built net. Built through a template it is stamped, and can
  then be saved, restored and forked with the outer net. Unstamped, it runs
  and the outer net cannot be saved.
- In a saved marking, a subnet place's entry is a list of one: the subnet in
  its saved form (`template`, `marking`, `params`, `enabled`). A port place's
  tokens are written once, in the subnet's marking, and the outer marking has
  no entry for `Level 3/Lobby`.
- The stepping rule is part of the definition and is not saved, as the deep
  policy is not saved today.

## 2. The wire (PROTOCOL.md)

Everything is additive, so the protocol stays at version 1.

**Topology (§5.1).** A place gains three optional fields, omitted when they do
not apply:

- `subnet: true` on a subnet place. Its `type_name` is the subnet's template
  name, or `""`.
- `port_of: "<subnet place>"` on the outer net's place for a port, so on
  `Level 3/Lobby` it is `"Level 3"`.
- `port: true` on the port place in the subnet's own topology.

A reader that knows none of these sees `Level 3` as a place with no edges and
`Level 3/Lobby` as an ordinary place, which is a truthful drawing. A viewer
that clusters by name prefix frames them together.

**State.** In `get_state`, a subnet place's entry carries
`subnet: {nested_id, path, template?, count}`, the fields a token's `nets[]`
entry has, without `name`. `count` is the number of tokens inside, port
places included. The `nested_id` lives until reset or load.

**Paths (§12.1).** A level through a subnet place is one segment, the place's
name: `World/Level 3` is the subnet, `World/Level 3/Treasury` a subnet inside
it, and `World/Level 3/Lobby/#4/metabolism` a net carried by a token in the
subnet's `Lobby`. A reader of a path knows which form to expect because it
knows whether the place is a subnet place. The existing rule stands: a name
containing `/` cannot be spelt in a path and is reached by `nested_id`.

**Reads, events and steps (§12).** Unchanged in shape. The addressed methods
take a subnet's `path` or `nested_id`. An inner event arrives as `nested` with
the subnet's path. A `step` that fired inside a subnet answers with `path` and
`nested_id`. Fireable sets of a subnet are told as for any inner net, and its
baseline is taken at load.

**The two names of a port place.** New text: a port place's tokens appear in
both nets' states, under `Level 3/Lobby` in the outer one and `Lobby` in the
subnet's, with the same ids. A change is told once. A reader that shows both
nets applies a change of either name to both. After a change of a port place,
the runtime compares the fireable sets of both nets and tells what changed.
How each net's `marking_version` moves for a change the other net told is to
be pinned down when this section is written, before any code.

**The stepping rule** stays off the wire, as the deep policy is today, except
that `passive` is written as now. §6.3 and §12.3 are rewritten to describe
§1.3.

## 3. Rust: the core crate

### 3.1 What an author writes

```rust
// The subnet's definition. With `serde`, register it as a template.
fn level(_: &Params) -> Result<Net, BuildError> {
    let mut b = NetBuilder::new();
    let lobby = b.place::<Hero>("Lobby");
    let exit = b.place::<Hero>("Exit Hall");
    b.port(lobby);
    b.port(exit);
    // ... the level's own places and transitions ...
    b.build()
}

// The outer net.
let level3 = b.subnet_place("Level 3", level(&params)?);     // -> SubnetRef
let lobby = b.port_of::<Hero>(level3, "Lobby");              // -> PlaceRef<Hero>, named "Level 3/Lobby"
b.transition("Enter", (TakeEdge(door),), (lobby,), enter);
b.stepping(level3, Stepping::TakeTurns);                     // the default is InnerFirst
```

- `NetBuilder::port(place)`, `subnet_place(name, net) -> SubnetRef`,
  `port_of::<T>(subnet, name) -> PlaceRef<T>`,
  `stepping(place_or_subnet, Stepping)`, and `Net::set_stepping`.
- `enum Stepping { InnerFirst, TakeTurns, OuterFirst, Passive }`,
  `#[non_exhaustive]`, default `InnerFirst`.
- `passive(place)` stays, as a shorthand for `Stepping::Passive`.
- `NetBuilder::deep_policy`, `Net::set_deep_policy` and `DeepPolicy` stay for
  one release, deprecated. They set the rule of every place that holds nets
  and has no rule of its own: `OuterFirst` gives outer first, `Interleaved`
  gives take turns. Consumers that build against this checkout keep compiling
  and keep their behaviour.
- New `BuildError`s: `NoSuchPort`, `PortTypeMismatch`, `PortIsSubnetPlace`,
  `EdgeOnSubnetPlace`, and `NotNested` widened to cover `stepping` on a place
  that holds no nets.
- Reading: `net.subnet(place) -> Option<&Net>`,
  `net.subnet_scope(place, f)` for the program to step or change a subnet
  (its events are lifted afterwards, as `nested_scope` does), and `PlaceInfo`
  gains `subnet`, `port` and `port_of`.

### 3.2 Types that change

- `Event` gains a variant for an event lifted from a subnet,
  `Subnet { place, nested_id, event }`. `Event::Nested` keeps its shape.
- `DeepStep` gains `Subnet { place, nested_id, step }` and becomes
  `#[non_exhaustive]`. That is a breaking change for code that matches on it.
- `FiringFailed.path` holds `(PlaceId, TokenId, String)` today. Its element
  becomes a type that can name a level with no token.
- The cursor of the ring is kept per net, in memory only, as now.

### 3.3 How a port place works, to be proven first

The engine indexes one net's places everywhere, mints token ids per net, and
keeps a key index per net. The intended mechanism leaves all of that alone:

- A `PlaceSlot` can own a subnet. The outer net also has a real slot and
  `PlaceId` for each port, a **window**, whose tokens live in the subnet.
- **Reading through a window** (counts, fireability, guards, queries, the
  read API, export) resolves to the subnet's slot and the subnet's key index,
  by reference.
- **An outer firing that touches a window** borrows the port's queue for the
  length of the firing: it is moved into the window's slot at launch and back
  at the landing or the failure. In between, `consume`, the mutate borrow,
  `insert_erased` and `restore_consumed` work on the outer net's own slots as
  they do now.
- The two nets' token counters are brought level when a queue moves, so that
  every id minted into a port place is larger than those already there. The
  key index of the receiving net is brought up to date for that place.
- External input through the outer handle (`put`, a `Command`) is carried out
  by the subnet's own insert.

This is the part most likely to meet a surprise. Slice R1 builds only this,
with tests of every edge kind on a port, and reports before anything else is
built on it. If it does not hold, the fallback is to resolve a window in each
engine function that touches a place's tokens, which is more code and no
change of behaviour.

### 3.4 Stepping

`step_deep_staged` keeps its shape: turns ahead of this net's own choice, the
choice, turns behind it. Ahead are the inner-first places and the ring members
before this net; behind are the ring members after it and the outer-first
places. A member's turn is that net's own deep step. The sequential driver,
the concurrent driver, the drivers' fuse and the runner all go through the
same three parts, so they follow the new order without logic of their own. The
concurrent driver's `interleaved: bool` goes.

## 4. Rust: the runner

- The topology gains the three marks, and `get_state` the `subnet` entry.
- A subnet is one more inner net in the owner's records: its own version,
  topology, progress and fireable set, known at load and never pruned.
- Path resolution learns the one-segment level.
- After a change of a port place, the fireable sets of both nets are compared.
- The host server needs no change of its own: a hosted net's subnets are
  walked, reported and kept quiet as its other inner nets are.

## 5. Python

- `SubnetPlaceNode(name, graph, stepping="inner_first")` declares a subnet
  place. `ListPlaceNode(..., port=True)` marks a port in the subnet's graph.
  `ListPlaceNode(..., nested=True, stepping=...)` sets the rule for a place
  whose tokens carry nets. `passive=True` stays as a shorthand.
- Edges name a port place by its outer name, `"Level 3/Lobby"`.
- `construct_graph(deep_policy=...)` stays for one release, deprecated, with
  the meaning of §3.1.
- The two names of a port place share one list of tokens. Ids are minted by
  the subnet.
- Stepping follows §1.3 in `fire_one_transition(descend=True)` and
  `NestedNets`, and in the concurrent runtime's inner firing.
- Saving follows §1.4 in `NetJson`.
- The runner follows §2: `Topologies.of`, the owner's addressing and records.
- **Graphviz:** a subnet place is drawn as one rounded box with a cell per
  port, showing each port's name and count. An edge ends at its port's cell
  where the drawing library allows it, and at the box otherwise. The port
  places are not drawn as separate nodes.

## 6. The viewer and the layout crate

### 6.1 `petritype-layout`

- The layout is told which places are subnet places and which ports belong to
  each. A subnet place takes part in ranking as **one node**: edges to its
  ports count as edges to it.
- Nodes get a size of their own. A closed subnet place is wide enough for its
  port circles and its title.
- Port circles are placed inside the box, ordered by the positions of the
  transitions they join, and `position_of_place(port)` answers with the
  circle's position. A port circle has its own radius, about a ghost's.
- A transition that reads one port and writes another of the same box is one
  ordinary pair of edges to one node, drawn to the two circles.

### 6.2 Closed

- A rounded box with the place's name as its title, a small circle per port
  with that port's count beside it, and the count of tokens inside.
- The outer net's arrows end at the port circles. Routing uses each place's
  own radius.
- The box flashes when a transition fires inside it. That needs the viewer to
  know which place a subnet's events belong to, beside the map it keeps from
  nets to tokens.
- A click on the box selects the subnet place. A click on a port circle
  selects the port place, and the inspector shows its tokens as for any
  place. Tokens are not drawn inside a port circle.

### 6.3 Opened

- Enter, a double-click, **Descend**, **Panel** and **Window** act on a
  selected subnet place as they act on a selected token that holds a net. The
  entry points that take a place and a token today gain a form that takes a
  place alone.
- A remote view is addressed by the `nested_id` in the place's `subnet`
  entry. A local one reads `net.subnet(place)`.
- The breadcrumb for such a level is the place's name, `world > Level 3`.
- A subnet's view closes only on reset or reload, since a subnet lives as
  long as its net.
- In the opened net, a port place is drawn as a place with a second ring, so
  that the border is visible from inside.
- A change of a port place is applied to both views, by §2.

### 6.4 The rest

- The thumbnail gives a closed subnet place one cell, lit by activity at its
  ports and inside it. The history player shows counts at the port circles.
  The token filter and the view filter treat a port place as a place.
- Unfolding the box inside the outer drawing is not in this round.
- The web app and the share page are not changed.

## 7. Tests

- **Core:** every input edge kind and every way of choosing an output on a
  port; ids and keys across the border; a failed firing that took from a
  port; the bands of §1.3, with mixed rules and at two depths; the in-flight
  rule; saving, restoring and forking a net with subnets; the build errors.
- **Both runners:** the topology marks, the `subnet` entry, the one-segment
  path, a port place's two names, fireable sets across a port.
- **Shared transcripts:** `05_nested_nets.json` must hold unchanged. The
  transit example therefore states outer first on its nested places at every
  level, in both runtimes. Two new transcripts, recorded by the Python
  runner and copied to Rust: `09_subnet_places.json` and
  `10_stepping_rules.json`.
- **Viewer:** a new suite beside `tests/nested.rs`, with a scripted transport,
  the Rust owner in process and a local net, and one test over each real
  runner process.
- **An example** in each runtime that runs its own process: a small world
  with two levels built from one template, heroes entering through a port
  and leaving through another.

## 8. Order of work

Each slice ends green on `just check` in the repositories it touches, and is
merged before the next starts. Consumers on this machine build against these
checkouts, so no slice may break their build.

| Slice | What | Where |
|---|---|---|
| P0 | The protocol text of §2, agreed before code | Python repo, `PROTOCOL.md` |
| S1 | The stepping rule per place, the new default, the deprecated policy; transit states outer first; transcript 05 unchanged; `10_stepping_rules.json` | both runtimes, both runners |
| R1 | Subnet places and port places in the core: §3.3 proven, edges across the border, events, ids | Rust core |
| R2 | The rest of the core: saving, forking, reading, depth, failure, the example, DESIGN and README | Rust core |
| Y1 | The same in Python, with the Graphviz symbol | Python |
| W1 | Both runners, and `09_subnet_places.json` | both runners |
| L1 | The composite node in the layout crate | `petritype-layout` |
| V1 | The viewer: closed, opened, the two views kept level | `petritype-bevy` |

S1 comes first because it stands alone, changes behaviour that exists today,
and is what the other slices assume. R1 and Y1 can run side by side once P0 is
agreed.

## 9. Out of scope

Firing in parallel across levels. A step limit against a net that never
rests. Unfolding a subnet inside the outer drawing. A builder shorthand for a
forwarding transition. A place shared by sibling subnets. Any change to the
web app or the share page.

## 10. Open points for Oleg

1. **The outer name of a port place**, `Level 3/Lobby`. The slash makes the
   name agree with paths and lets a viewer that clusters by prefix frame the
   ports with their subnet place. The cost is that a subnet place or a port
   whose own name contains a slash is reached by `nested_id` alone.
2. **The builder's names:** `subnet_place`, `port`, `port_of`, `stepping`, and
   `Stepping::{InnerFirst, TakeTurns, OuterFirst, Passive}`; in Python
   `SubnetPlaceNode`, `port=True` and `stepping="inner_first"`.
3. **Keeping the old policy setting for one release**, deprecated, so that no
   consumer breaks on the day S1 lands.
4. **The stepping rule off the wire** in this round.
5. **S1 as a slice of its own, first.** It changes the default for nets
   carried by tokens before any subnet place exists.
6. **The Graphviz symbol** as a box with a cell per port.
