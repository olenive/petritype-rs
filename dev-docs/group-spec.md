# Spec: groups, a drawing-only label on parts of a net

Draft of 2026-11-21, for both runtimes, the wire, the layout crate and the
viewer. Not built. It replaces the viewer's clusters by name prefix.

## 0. What a group is, and what it is not

A **group** is a named set of places and transitions of one net, stated in
the net's definition. Only drawings read it. The engine never does: firing,
stepping, ids, saving and the events are exactly those of the same net with
no groups.

A group is not a subnet place. The two are kept apart in name, in the
definition and in the picture.

| | Group | Subnet place |
|---|---|---|
| What it is | a label on parts of one net | a separate net held by a place |
| Token logic | the flat net's, unchanged | its own net, its own ids and turns |
| Shared places | yes, freely | no, only ports |
| What it is for | reading a large flat net | a part that is really separate |

Three rules carry over from the subnet places work: no character in a name
has a special meaning; one way to say each thing, with what it replaces
removed in the same change; the two runtimes behave the same.

Why it is wanted: a net whose rules all read and change one shared place
cannot be split into subnets without moving those rules to the outer net,
which leaves the first view no simpler (Unfamiliar's trial of 21 November,
`unfamiliar/dev-docs/PETRITYPE_GROUPS_REQUEST.md`). Such a net stays flat,
and is read by gathering its parts under names.

## 1. The rules

- A group belongs to one net. A subnet, or a net carried by a token, has
  groups of its own, stated in its own definition.
- A place or a transition is in at most one group, or in none. A subnet
  place may be in a group, as any place may.
- Groups are flat: a group does not hold a group.
- A group has a name, unique among its net's groups, and nothing else. Names
  may hold any characters.
- Groups have an order, the order they are declared in. Drawings place them
  in that order.
- Groups are part of the definition. They are not saved in a marking, and a
  fork or a template carries them as it carries the rest of the definition.
- Being in a group changes nothing about a node's own name. Node names stay
  unique within the net, as now.

## 2. What an author writes

Rust:

```rust
let walk = b.group("Walk");                       // -> GroupRef
let paid = b.place::<Energy>("Walk Paid");
b.in_group(walk, paid);                           // a place or a subnet place
b.transition("Fund Walk", (MutateEdge(level),), (paid,), fund_walk)
    .group(walk);                                 // a transition, where it is declared
```

- `NetBuilder::group(name) -> GroupRef`, `NetBuilder::in_group(group, place)`
  for a place handle or a subnet handle, and `TransitionBuilder::group(group)`.
- Build errors: two groups of one name; a node put in two groups; a group
  handle from another builder; a port's outer handle put in a group (a port
  is drawn in its subnet place's box and goes where the box goes).
- Reading: `Net::groups()` in declared order, `PlaceInfo::group`,
  `TransitionInfo::group`, and the same in `Topology` (a list of group names,
  and `group` on each place and transition, written only when set).

Python:

```python
ListPlaceNode("Walk Paid", Energy, group="Walk")
SubnetPlaceNode("Level 1", level, group="Dungeon")
FunctionTransitionNode("Fund Walk", fund_walk, group="Walk")
```

- `group=` on the three node classes. A group exists from its first mention,
  and that is its place in the order.
- The same errors where they can arise.

Declaring a group never changes the order in which places and transitions
are declared, since that order has meaning for stepping and for ties.

## 3. The wire

Protocol version 2 stays: these are optional fields, and a reader that does
not know them ignores them (§10 of `PROTOCOL.md`).

- `topology.groups?`: the net's group names, in declared order. Absent when
  the net has none.
- `group?` on a place, on a subnet place and on a transition in
  `topology`: the name of its group.
- Every net's own topology carries its own groups, so an inner net read by
  `path` or `nested_id` shows its own.
- Nothing else on the wire changes. No request, notification or state entry
  names a group.

A new shared recorded fixture, `11_groups.json`, over a small conformance
net with two groups, one place in none, and a subnet with a group of its own:
`describe`, `load`, `get_state` of the net and of the subnet.

## 4. The layout crate

- `Clustering`, `NamePrefix`, `prefix_of` and `display_name` go. Names are
  shown whole.
- The layout is given each node's group: `LayoutPlace` gains `group`, and
  transitions get an input type of their own with a name and a group (plain
  `(id, name)` pairs still convert, with no group). `compute(&net)` reads the
  groups from the net.
- **An open group** is laid out as a cluster is today: on its own, packed in
  declared order, with a frame and room for its title. An edge that crosses
  a frame takes no part in the layout, as now.
- **Nodes in no group** are laid out together as one block with no frame.
  Today they share an untitled frame.
- **A closed group** is one node, a box with its name and the count of
  tokens inside. The layout is told which groups are closed. A closed group
  joins the block of nodes in no group and takes part in its ranking: an
  edge between a member and a node outside counts as an edge of the box, and
  an edge wholly inside is not drawn. Several edges between the box and one
  outside node become one arrow in each direction. With every group closed,
  the layout is one layered drawing of boxes and the nodes in no group.
- The result answers: the groups' names and frames (`group_names`,
  `group_bounds`, `group_of`), and for a closed group its box (centre, size,
  room for the title and the count), as `subnet_box` does for a subnet place.
- A net with no groups lays out exactly as a net with no clustering does
  today, to the same numbers.
- A part of the net shown alone (`compute_subgraph`) keeps its meaning, with
  group names where it took cluster names.

## 5. The viewer

- **Every view draws groups**: the canvas, the side panel and the inner
  windows, for local and remote nets, because the groups come with the net.
  No setting turns them on, and the `remote` example needs no flag.
- **Open**: a frame with the group's name as its title, as a cluster is
  drawn today, with a dashed outline so that it does not read as a subnet
  place.
- **Closed**: a box with a dashed outline, the group's name, and the count
  of tokens in its places. It has no port circles. Arrows end at its edge.
  It flashes when one of its transitions fires. Tokens are not drawn inside
  it: a token going in travels its arrow and vanishes at the box.
- **Closing and opening** happens in place and the view is laid out again.
  A click on a frame's title or on a closed box selects the group. One key
  or a double-click toggles the selected group, and there is a command to
  close all and one to open all. The state is kept per view, by the viewer
  alone: it is not in the definition, not on the wire, and not saved.
- **At the start** every group is open. The viewer's configuration can ask
  for all closed, and the `remote` example takes a flag for it.
- **The inspector**, for a selected group: its name, how many places and
  transitions it has, and the count of tokens inside.
- **The view filter** names groups where it named clusters.
- The thumbnail packs groups as it packs clusters today, and takes no notice
  of closed. The history player shows a closed box's count.
- A subnet place in a group is drawn inside the frame as it is drawn now,
  and is hidden with the rest when the group is closed.

## 6. Graphviz, in both runtimes

A group is drawn as a Graphviz cluster: a dashed frame with the group's name.
Closing is for the viewer and is not drawn here.

## 7. What is removed, and who has to change

- The viewer's and the layout crate's clustering by name prefix, with its
  separator and the shortening of names inside a frame.
- Inside the three repositories only tests use it. They move to declared
  groups.
- Cyberpunk is the one program that relies on it: eleven prefixes over about
  130 names, a `--clusters` flag and keys of its own built on the view
  filter. It builds against pinned copies, so nothing changes for it until
  it moves its pins. Then it declares eleven groups, its names lose their
  prefixes, and its flag and keys name groups.
- Unfamiliar and graph-tactics use no clustering. Unfamiliar declares its
  four groups and needs nothing else.

## 8. Tests

- Core, both runtimes: declaring, the errors, reading, the topology, that a
  net steps and saves the same with and without groups, fork and templates.
- Runners: the fields in the topology of a net and of an inner net; the new
  shared recorded fixture replays in both.
- Layout: open groups against today's clusters on the same nets, the block
  with no frame, closed groups (ranking, merged arrows, sizes, no overlaps),
  every group closed, a subnet place in a group, and nets with no groups
  unchanged to the number.
- Viewer: frames in the canvas, the panel and a window; closing and opening
  with tokens in flight; counts; selection; the filter; both real runners.
- Graphviz: the DOT source, in both runtimes.

## 9. Order of work

One branch, `groups`, in each of the three repositories, one commit per
step, every commit passing its repository's gates on its own.

| Step | What | Where |
|---|---|---|
| 1 | The protocol text | Python repo |
| 2 | Declaring and reading groups, the Graphviz frames | Rust core, Python library |
| 3 | The topology fields, the conformance net, `11_groups.json` | both runners |
| 4 | Groups in place of clustering, and closed groups | `petritype-layout` |
| 5 | Frames in every view, closing and opening, the filter | viewer |

## 10. Out of scope

A group inside a group. A group that spans nets. A colour or a description
for a group. The definition saying which groups start closed. A place shared
by sibling subnets. Unfolding a subnet place inside the outer drawing.

## 11. Decisions taken while writing this

| Point | Decision |
|---|---|
| The name | Group. |
| How many groups a node may be in | One, or none. |
| Groups inside groups | No. |
| How it is declared | A handle in Rust, a name in Python, set on the node; declaration order of nodes is never touched. |
| Nodes in no group | Drawn together with no frame. |
| What a closed group is in the layout | One node that takes part in ranking, with merged arrows. |
| Where the closed state lives | In the viewer, per view. All open at the start, with a setting and a flag for all closed. |
| Graphviz | Frames only. |
| The protocol version | Stays 2: optional fields only. |
