# Spec: subnet places, port places, the stepping rule and structured addresses

Agreed with Oleg on 2026-11-21, point by point. Nothing here is built. The
concept, with the reasons for each choice, is `subnet-place-concept.md`; this
document says how it is to be built, in both runtimes, on the wire and in the
viewer.

## 0. The principle

A place of a net may hold a net of its own: a **subnet place** holds a
**subnet**. The subnet is separate and isolated. The net around it reaches it
only through **port places**, which are ordinary places of the subnet that its
definition marks as ports. An edge of the outer net to or from a port place is
an ordinary edge.

Between a net and the nets inside it, the order of firing is one rule per
place that holds nets, a subnet place or a place whose tokens carry nets:
inner first (the default), take turns, outer first, or passive.

Three working rules run through the whole document:

- **Prefer what can be modelled in the net over a rule hidden in the engine.**
  There is no turn count, no handover firing, and no edge that crosses two
  borders.
- **No character in a name has a special meaning.** Nothing is addressed by
  text joined with a separator. A reference to something inside another thing
  is structured: a pair, or a list of steps.
- **One way to say each thing, and no old behaviour carried along.** What this
  replaces is removed in the same change, and its users are fixed with it.

## 1. The rules, for both runtimes

### 1.1 Subnet places and port places

- A subnet place holds exactly one subnet, given when the outer net is built.
  It holds no tokens of its own, and no edge may name it.
- The subnet's definition marks some of its places as ports. A port place must
  be an ordinary place or a place whose tokens carry nets. It may not be a
  subnet place.
- The outer net knows each port of each of its subnet places, and refers to
  one by a pair: the subnet place and the port. There is no combined name.
  Place names stay unique within a net, and port names within a subnet.
- A port place has one queue of tokens and one set of token ids, minted by the
  subnet, whichever net is looking at it.
- The outer net's edges use a port place like any other place: every input
  edge kind, and every way of choosing an output (by position, by type, by an
  output distribution function). Guards, queries and external input work on
  it too.
- Nothing else of the subnet can be named by the outer net's transitions. The
  program itself may still read a subnet, and may step or change it inside a
  scope, as it may a token's net today.

### 1.2 Tokens, events, failure

- A token crosses the border only through a firing of an outer transition, so
  it gets a new identity as in any firing.
- A change is told once, by the net whose transition made it. An outer firing
  that puts into a port reports the pair. An inner firing that takes from it
  reports the place by its own name, lifted into the outer log as inner
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
- Inner first is new, and is the default for both kinds of place. Today's
  default is outer first.
- The ring holds one level. Today's interleaved ring holds every net of the
  tree.
- The per-net policy and the separate passive setting are removed. The rule
  per place is the only way to say it.

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
  tokens are written once, in the subnet's marking.
- The stepping rule is part of the definition and is not saved.

## 2. The wire (PROTOCOL.md): version 2

Structured addresses change the meaning of an existing field, so the protocol
becomes **version 2**, reported in `describe` and `get_state`. A reader of
version 2 does not read version 1. The desktop viewer speaks version 2 only,
and says so plainly when a runtime reports another version.

### 2.1 Addresses

An inner net's `path` becomes a list of steps, one per level, from the loaded
net down. An empty list is the loaded net.

| Level | Step |
|---|---|
| a net carried by a token | `{"place": "Depot", "token": 17, "net": "cabin"}` |
| a subnet | `{"place": "Level 3"}` |
| a token's net in a subnet's port | `[{"place": "Level 3"}, {"place": "Lobby", "token": 4, "net": "metabolism"}]` |

- This holds wherever a path appears today: the addressed requests, their
  results, a token's `nets[]`, the `nested` notification, a `step` result and
  the log.
- A place may be named anything. The rule that a name containing `/`, or a
  place name starting with `#`, cannot be spelt goes.
- `nested_id` stays as it is, the short way to name one inner net.
- A viewer builds its own breadcrumb text from the list.

### 2.2 Subnet places and port places

- **Topology.** A subnet place carries `subnet: true`, its template's name as
  `type_name` (or `""`), and `ports`: a list with, for each port, the fields a
  place has (`name`, `type_name`, `serializable`, `nested?`, `key_names?`,
  `style?`). In the subnet's own topology the place carries `port: true`.
- **References.** Wherever the outer net names a port, the object that names a
  place gains `port`: an edge is
  `{"place": "Level 3", "port": "Lobby", "transition": "Enter", ...}`, and a
  consumed, produced, added, removed or mutated token is told the same way.
  An edge is keyed by its place, its port if any, and its transition.
- **State.** In `get_state`, a subnet place's entry carries
  `subnet: {nested_id, path, template?, count}`, the fields a token's
  `nets[]` entry has without `name`, and for each port its count and tokens
  as a place has them. `count` is the number of tokens inside, ports
  included. The `nested_id` lives until reset or load.
- **Requests that name a place** (`get_place_tokens`, `inject`, `query` and
  the rest) take `port` beside `place` in the outer net, or address the
  subnet and name the place alone. Filters (§13) apply per net as now: a port
  place is filtered in its subnet.
- **One place, two views.** A port place's tokens appear in both nets'
  states, with the same ids. A change is told once. A reader that shows both
  nets applies a change of either to both. After a change of a port place,
  the runtime compares the fireable sets of both nets and tells what changed.
  How each net's `marking_version` moves for a change the other net told is
  pinned down when this section of the protocol is written, before any code.
- **Reads, events and steps** keep their shape. An inner event arrives as
  `nested` with the subnet's path. A `step` that fired inside a subnet
  answers with `path` and `nested_id`. A subnet's fireable baseline is taken
  at load.

### 2.3 The stepping rule

A place that holds nets carries `stepping`, one of the four names of §1.3.
The `passive` field goes. §6.3 and §12.3 are rewritten to describe §1.3.

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
let lobby = b.port_of::<Hero>(level3, "Lobby");              // -> PlaceRef<Hero>
b.transition("Enter", (TakeEdge(door),), (lobby,), enter);
b.stepping(level3, Stepping::TakeTurns);                     // the default is InnerFirst
```

- `NetBuilder::port(place)`, `subnet_place(name, net) -> SubnetRef`,
  `port_of::<T>(subnet, name) -> PlaceRef<T>`,
  `stepping(place_or_subnet, Stepping)`, and `Net::set_stepping`.
- `enum Stepping { InnerFirst, TakeTurns, OuterFirst, Passive }`,
  `#[non_exhaustive]`, default `InnerFirst`.
- **Removed:** `NetBuilder::deep_policy`, `Net::set_deep_policy`,
  `Net::deep_policy`, `DeepPolicy`, and `NetBuilder::passive`. The examples
  and tests move to `stepping`. The transit example, written around outer
  first, states that rule on its places at every level.
- New `BuildError`s: `NoSuchPort`, `PortTypeMismatch`, `PortIsSubnetPlace`,
  `EdgeOnSubnetPlace`, and `NotNested` widened to cover `stepping` on a place
  that holds no nets.
- Reading: `net.subnet(place) -> Option<&Net>`,
  `net.subnet_scope(place, f)` for the program to step or change a subnet
  (its events are lifted afterwards, as `nested_scope` does),
  `net.port_ref::<T>(subnet, name)`, and `PlaceInfo` gains `subnet`, `port`,
  `port_of` and `stepping` in place of `passive`. A port's `PlaceInfo` in the
  outer net has the port's own name and the subnet place it belongs to.

### 3.2 Types that change

- `Event` gains a variant for an event lifted from a subnet,
  `Subnet { place, nested_id, event }`. `Event::Nested` keeps its shape.
- `DeepStep` gains `Subnet { place, nested_id, step }` and becomes
  `#[non_exhaustive]`.
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

This is the part most likely to meet a surprise. Its commit builds only this,
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

- Addresses are lists of steps: the wire types, `resolve_path`, the lifted
  events and the paths of inner nets.
- The topology and `get_state` follow §2.2 and §2.3.
- A subnet is one more inner net in the owner's records: its own version,
  topology, progress and fireable set, known at load and never pruned.
- After a change of a port place, the fireable sets of both nets are compared.
- The host server needs no change of its own: a hosted net's subnets are
  walked, reported and kept quiet as its other inner nets are.

## 5. Python

- `SubnetPlaceNode(name, graph, stepping="inner_first")` declares a subnet
  place. `ListPlaceNode(..., port=True)` marks a port in the subnet's graph.
  `ListPlaceNode(..., nested=True, stepping=...)` sets the rule for a place
  whose tokens carry nets.
- An edge names a port with `Port("Level 3", "Lobby")` where it would name a
  place.
- **Removed:** `construct_graph(deep_policy=...)`, `ExecutableGraph.deep_policy`
  and `passive=True`.
- The two views of a port place share one list of tokens. Ids are minted by
  the subnet.
- Stepping follows §1.3 in `fire_one_transition(descend=True)` and
  `NestedNets`, and in the concurrent runtime's inner firing.
- Saving follows §1.4 in `NetJson`.
- The runner follows §2: structured addresses in the owner's addressing and
  in `Tokens.nets`, the topology in `Topologies.of`, and the records.
- **Graphviz:** a subnet place is drawn as one rounded box with a cell per
  port, showing each port's name and count. An edge ends at its port's cell
  where the drawing library allows it, and at the box otherwise. The port
  places are not drawn as separate nodes.

## 6. The viewer and the layout crate

### 6.1 The protocol

- The wire types, `Address`, the mirror's records and the breadcrumbs take a
  path as a list of steps. Breadcrumb text is built by the viewer.
- A runtime that reports a version other than 2 is refused with a clear
  message in the status line.

### 6.2 `petritype-layout`

- The layout is told which places are subnet places and which ports belong to
  each. A subnet place takes part in ranking as **one node**: edges to its
  ports count as edges to it.
- Nodes get a size of their own. A closed subnet place is wide enough for its
  port circles and its title.
- Port circles are placed inside the box, ordered by the positions of the
  transitions they join, and `position_of_place(port)` answers with the
  circle's position. A port circle has its own radius, about a ghost's.

### 6.3 Closed

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

### 6.4 Opened

- Enter, a double-click, **Descend**, **Panel** and **Window** act on a
  selected subnet place as they act on a selected token that holds a net. The
  entry points that take a place and a token today gain a form that takes a
  place alone.
- A remote view is addressed by the `nested_id` in the place's `subnet`
  entry. A local one reads `net.subnet(place)`.
- A subnet's view closes only on reset or reload, since a subnet lives as
  long as its net.
- In the opened net, a port place is drawn as a place with a second ring, so
  that the border is visible from inside.
- A change of a port place is applied to both views, by §2.2.

### 6.5 The rest

- The inspector shows a place's stepping rule where it showed `passive`.
- The thumbnail gives a closed subnet place one cell, lit by activity at its
  ports and inside it. The history player shows counts at the port circles.
  The token filter and the view filter treat a port place as a place.
- Unfolding the box inside the outer drawing is not in this round.
- The web app and the share page are not changed. They read version 1 and
  stay on the release they pin until they are moved, which is separate work.

## 7. Tests

- **Core:** every input edge kind and every way of choosing an output on a
  port; ids and keys across the border; a failed firing that took from a
  port; the bands of §1.3, with mixed rules and at two depths; the in-flight
  rule; saving, restoring and forking a net with subnets; the build errors.
- **Both runners:** structured addresses in every request and result that
  carries one; the topology and state of §2.2; a port's two views; fireable
  sets across a port; the `stepping` field.
- **Shared transcripts**, recorded by the Python runner and copied to Rust:
  `05_nested_nets.json` re-recorded for version 2, with the transit example
  stating outer first so that its story still holds; and two new ones,
  `09_subnet_places.json` and `10_stepping_rules.json`. Every other
  transcript is re-recorded where it carries the version number.
- **Viewer:** the existing suites moved to version 2; a new suite beside
  `tests/nested.rs` for subnet places, with a scripted transport, the Rust
  owner in process and a local net, and one test over each real runner
  process.
- **An example** in each runtime that runs its own process: a small world
  with two levels built from one template, heroes entering through one port
  and leaving through another.

## 8. Order of work

The whole change is built before it is merged or tested by hand. It is one
branch, `subnet-places`, in each of the three repositories (Python, Rust,
viewer), with one commit per step in each repository the step touches. **Every
commit passes its repository's gates on its own**, so that any of them can be
gone back to and tested. Nothing reaches `main` in any repository until all of
it is built and Oleg has tested it; then the three merge together.

| Step | What | Where |
|---|---|---|
| 1 | The protocol text for version 2: §2 written into `PROTOCOL.md` and agreed | Python repo |
| 2 | Structured addresses and the version number, with the existing transcripts re-recorded | both runners, the viewer |
| 3 | The stepping rule per place, inner first as the default, the old policy and `passive` removed; `10_stepping_rules.json` | both runtimes, both runners, the viewer's inspector |
| 4 | Subnet places and port places in the Rust core, starting with §3.3 alone | Rust core |
| 5 | The rest of the Rust core: saving, forking, reading, depth, failure, the example, DESIGN and README | Rust core |
| 6 | The same in Python, with the Graphviz symbol | Python |
| 7 | Both runners, and `09_subnet_places.json` | both runners |
| 8 | The composite node in the layout crate | `petritype-layout` |
| 9 | The viewer: closed, opened, the two views kept level | viewer |

Consumers on this machine build against these checkouts by path. One of them
sets the old per-net policy in code and is fixed when step 3 exists; another
uses the old `passive` call and is fixed when it next updates. Whether the
branches are built in the main checkouts or in a separate worktree, so that
the consumers keep building against `main` meanwhile, is decided before work
starts.

## 9. Out of scope

Firing in parallel across levels. A step limit against a net that never
rests. Unfolding a subnet inside the outer drawing. A builder shorthand for a
forwarding transition. A place shared by sibling subnets. Moving the web app
and the share page to version 2.

## 10. Decisions taken while writing this

| Point | Decision |
|---|---|
| What a port place is called from outside | No combined name. A pair: the subnet place and the port. |
| Addresses built from text | Replaced by lists of steps, in this change. The protocol becomes version 2. |
| The old per-net policy and the passive setting | Removed in this change, with no period of deprecation. Their users are fixed. |
| The stepping rule on the wire | Yes, as `stepping`, replacing `passive`. |
| The names an author types | As in §3.1 and §5. |
| The Graphviz symbol | A rounded box with a cell per port. |
| How it is built | One change, in commits that each pass the gates, merged when all of it is built and tested. |
