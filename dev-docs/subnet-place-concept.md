# A subnet place: the concept

Written 2026-11-21, and revised the same day after its open questions were
gone through one at a time. This replaces `net-place-concept.md`, whose
working name "net place" read as "a place in a net". The concept is agreed.
It is not a spec: nothing here is built, and the builder's method names are
not chosen.

## The three terms

| Term | Meaning |
|---|---|
| place | an ordinary place that holds tokens |
| **subnet place** | a place that has a subnet inside it |
| **port place** | an ordinary place inside a subnet that the net around it can also reach |

```
world
└─ Level 3            (a subnet place)
   ├─ Lobby           (a port place of Level 3)
   ├─ Arena
   └─ Treasury        (a subnet place inside Level 3's subnet)
      ├─ Deposit      (a port place of Treasury)
      └─ Vault
```

"Port place" is the literature's word. The net inside a subnet place is also
an *inner net*, the word already used for a net carried by a token.

## What a subnet place is

- From the outer net it stands where a place stands. Transitions put tokens
  into it and take tokens out of it along ordinary edges.
- It is not one place. Each edge names the port place it delivers to or takes
  from.
- The subnet is a **separate net**, built from a template. It keeps its state
  for as long as the outer net lives.
- The subnet is **isolated**. It cannot read or write the net around it.
  Tokens cross only through port places, and every other place of the subnet
  is out of reach from outside.
- One subnet place owns **one subnet**, shared by every token that arrives.

## Why this form

The consumers need a container for state that changes over time, between the
firings of the net around it. In a Petri net, state lives in places, so the
nested thing is a place. A game's levels are the example: the outer net says
which level a character is in, each level is a subnet place, and looking
inside one shows what is happening there.

## The rules agreed

### Port places

- A port place is an ordinary place of the subnet that the subnet's
  definition marks as a port. The definition owns its interface: every subnet
  place built from one template has the same ports, and the outside cannot
  open a place the author did not expose.
- There is one kind of port place. A place is not an entry or an exit:
  direction comes from the edges that touch it.
- **A port place belongs to both nets.** The outer net lists it among its own
  places, marked as a port of that subnet place, and the subnet lists it too.
  There is one set of tokens.
- An edge of the outer net to or from a port place is an ordinary edge. Every
  way of choosing an output place applies (by position, by type, by an output
  distribution function), and so does every kind of input edge.

### Tokens, events and failure

- A token crosses the border only through a firing of an outer transition, and
  a firing consumes tokens and produces new ones. So no token keeps its
  identity across the border, as none keeps it through any firing.
- Each change is told once, by the net whose transition made it. An outer
  firing that puts a token into a port place reports it as for any place.
- Failure works as it does today at each level. A port place is an ordinary
  place, so a failed firing restores what it took in the usual way, and has
  delivered nothing.

### Definitions and saving

- A subnet is built by a template: a named function that builds a net,
  optionally from parameters. One template can stand behind several subnet
  places.
- A subnet built from a template is saved, restored and forked with the outer
  net, as a token's net is today. One built without a template runs and cannot
  be saved, which is today's rule as well.

### Depth

- A subnet may contain subnet places of its own, to any depth.
- A token that carries a net can sit in a subnet, and can arrive through a
  port place: a character with a metabolism enters a level and keeps it.
- **A port place must be an ordinary place, not a subnet place.** No edge
  crosses two borders. Each level hands things inward with a transition of
  its own, which keeps the path visible in the drawing and the log. A builder
  shorthand that writes such a forwarding transition may come later.

### Stepping

- **Inside one net nothing changes.** Each net picks its next transition by
  priority and then declaration order, and a subnet is a net.
- **Between the levels there is one rule, set per place that holds nets**,
  whether a subnet place or a place whose tokens carry nets:

  | Rule | Meaning |
  |---|---|
  | inner first (the default) | the inside fires until it is at rest before the outer net fires again |
  | take turns | one firing outside, one inside, alternating |
  | outer first | the inside fires only when the outer net has nothing to fire |
  | passive | neither: the program steps the inside itself |

- Inner first is new, and it becomes the default for both kinds. It gives
  nesting a meaning that fits in one line: whenever the outer net fires, the
  inside is at rest. The rule decides which transition fires next and nothing
  more. A program that asks for one step per tick still sees the inside
  advance one firing at a time.
- Places with different rules combine as bands: inner-first places go before
  the outer net, take-turns places alternate with it, outer-first places go
  after it.
- **This changes the present default for nets carried by tokens**, which is
  outer first. The transit example, the transcripts the two runtimes share and
  the Python runtime's default change with it.
- An inside that never comes to rest stops the outer net under inner first.
  Such a place is set to take turns or passive. A net driven by time is given
  time as tokens, a beat on every tick, and is at rest between beats.
- **Nothing finer goes into the engine.** Control is handed over with tokens
  through port places: a `Go` token the inside needs, a `Done` token the
  outside waits for, or a budget of tokens that each inner move takes one of.
  That is visible in the drawing and in the log, where an engine setting is
  not. The principle: prefer what can be modelled in the net over a rule
  hidden in the engine.
- Firing in parallel across levels is left out of a first version: one firing
  at a time across the whole tree.

### The protocol and the two runtimes

- The change is additive. A place in the topology gains optional marks: that
  it is a subnet place, with its template's name, or that it is a port place.
  A subnet is read by path, as a token's net is. A net without subnet places
  describes itself as it does today.
- The Python runtime gains subnet places, port places and the per-place
  stepping rule in the same round as the Rust one. The two are kept in step.

### Viewers

- **Closed**, a subnet place is a box with rounded corners holding a small
  circle for each port place. The outer net's arrows end at the circle they
  deliver to or take from. It shows counts, and a flash when the inside fires.
- **Opened**, it shows the subnet in the three ways the desktop viewer already
  shows a token's net: in place on the canvas, in the side panel, or in a
  window of its own.
- Unfolding the box inside the outer drawing, so that the subnet appears in a
  frame among the outer net's nodes, is for a second round.
- First round: the desktop viewer in full, and the closed symbol in the Python
  library's Graphviz drawing. The web app and the share page get nothing new
  and keep working, since the protocol change is additive. Both are expected
  to move to the desktop viewer in time.

## How it relates to what exists

| | Lives | Copies | Status |
|---|---|---|---|
| A net carried by a token (DESIGN §6.6) | as long as the token | one per token, made at run time | built |
| A subnet place | as long as the outer net | one, fixed when the net is built | this concept |

The two go together. Several fights at once, one per location, are tokens that
each carry a net. A level that everyone passes through is a subnet place. A
subnet place should be built on the machinery nets carried by tokens already
have: lifted events, paths, deep stepping, templates, and the viewers' ways of
looking inside.

## What it does not cover

- **A place shared by sibling subnets.** A port place is shared by a subnet
  and the net around it, and by no one else. A large flat net whose subsystems
  all read a few hub places stays a flat net, grouped by name prefix and drawn
  in clusters.
- **A net run as a pure function inside a transition.** A body can already
  build and run a net to completion, unseen. Making that visible is a
  different feature and is not planned.

## Considered and set aside

| Form | Why not |
|---|---|
| A net inside a transition, one run per firing | Models a step that starts and ends, not state that lasts, and needs new firing rules. |
| A module pasted into the same net (the literature's substitution transition) | No isolation and no stepping of its own. |
| Folding name-prefix clusters in the drawing | Looks like nesting without being it. Clusters stay a drawing aid. |
| Entry places and exit places as two kinds | Direction is a matter of edges, and a place may be read, added to and taken from by several transitions. |
| Turn counts, or a firing that hands control over, in the engine | The same is modelled with tokens through port places, in plain sight. |
| A port place that is itself a subnet place | Only saves a forwarding transition, and hides the path. |

In the literature's terms, tokens that carry nets are *nets within nets*
(object nets). A subnet place is nearest to the *substitution place* of the
hierarchical coloured nets, with port places as there, and with the difference
that the inside here is a net of its own with its own stepping.

## Left for the spec

- The builder's names for declaring a subnet place and a port place.
- The order among several places that share a stepping rule.
- The protocol's field names, and the new and re-recorded transcripts.
- The closed symbol's layout, and what the inspector, the thumbnail, the
  history and the filters show for a subnet place.
- How the change of the stepping default is announced to existing users.
