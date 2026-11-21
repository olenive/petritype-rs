# A net that stands where a place stands: the concept

Written 2026-11-21. This records a direction agreed in conversation that day.
It is a concept, not a spec: nothing here is designed in detail or built, and
the names are working names.

## What was decided

petritype gets real nesting, in one form: **a place of a net may own a net of
its own**. Working name: a **net place**.

- From the outer net it stands where a place stands. Transitions put tokens
  into it and take tokens out of it, along ordinary edges.
- It is not one place. Inside is a whole net, and a token that arrives goes to
  one of its **entry places**, chosen by the token's type. A transition that
  takes from it takes from one of its **exit places**.
- The inside is a **separate net**. It has its own places and transitions, it
  keeps its state for as long as the outer net lives, and it fires in steps
  decided at run time. Nothing is run to completion when a token arrives.
- The inside is **isolated**. It cannot read or write the outer net. Tokens
  cross only through the entry and exit places.
- One net place owns **one net**, shared by every token that arrives.

## Why this form

What the consumers need is a container for state that changes over time, on
its own, between the firings of the net around it. In a Petri net, state lives
in places. So the nested thing is a place.

A game's levels are the example. The outer net says which level a character is
in. Each level is a net place, and looking inside one shows what is happening
in that level.

## How it relates to what exists

| | Lives | Copies | Status |
|---|---|---|---|
| A net carried by a token (a nested place, DESIGN §6.6) | as long as the token | one per token, made at run time | built |
| A net place | as long as the outer net | one, fixed when the net is built | this concept |

The two go together. Several fights at once, one per location, are tokens that
each carry a net. A level that everyone passes through is a net place.

A net place should be built on the machinery that nets carried by tokens
already have: events lifted into the outer log, paths on the protocol, deep
stepping and its policy, templates, and the viewers' ways of looking inside.

## How it is drawn

A net place needs a symbol of its own, since it is not a single place.

- **Closed:** a box with rounded corners holding a small circle for each entry
  and exit place. The outer net's arrows end at the circle they deliver to or
  take from, so the drawing shows which kind of token goes where. It stays
  compact, about the size of a few places.
- **Open:** selecting it shows the whole inner net, with its own places,
  transitions and firings, the way a viewer descends into a token today.
- While closed it still shows that something is happening inside: counts, and
  a flash when the inside fires.

## What it does not cover

- **Places shared across the border.** An isolated net cannot touch a place of
  the outer net. A large flat net whose subsystems all read a few hub places
  stays a flat net, grouped by name prefix and drawn in clusters.
- **A net run as a pure function inside a transition.** A body can already
  build and run a net to completion, unseen. Making that visible is a
  different feature and is not planned.

## Considered and set aside

| Form | Why not now |
|---|---|
| A net inside a transition, one run per firing | Models a step that starts and ends, not state that lasts. It needs new firing rules (a firing that stays in flight, saving mid-run, failure). |
| A module pasted into the same net (the literature's substitution transition) | No isolation and no stepping of its own. Its one advantage is shared places. |
| Folding name-prefix clusters in the drawing | Looks like nesting without being it. Clusters stay a drawing aid. |

In the literature's terms, tokens that carry nets are *nets within nets*
(object nets). A net place is nearest to the *substitution place* of the
hierarchical coloured nets, with the difference that the inside here is a net
of its own and not a part of the outer net's structure.

## Open questions for the spec

1. **Names.** "Net place", "entry place" and "exit place" are working names.
   "Nested place" is taken.
2. **Choosing the entry place.** By type alone, or by name when two entry
   places hold the same type.
3. **Taking from it.** Which input edge kinds an exit place supports (take,
   keyed take, drain, the reading kinds), and that a transition is fireable
   when its exit place holds what the edge asks for.
4. **Stepping.** How the outer net's deep stepping and its policy apply, and
   whether a net place can be passive.
5. **Token identity.** Whether a token keeps its id as it crosses the border,
   which the viewers need to animate an arrival.
6. **Events and the log.** How an arrival and a departure are told: the outer
   firing, and the inner net's added or removed token.
7. **Definitions.** Whether the inner net is built from a template, so that
   one definition can stand behind several net places, and how it is saved,
   restored and forked.
8. **Depth.** A net place inside a net place, and tokens that carry nets
   inside either.
9. **The protocol and the second runtime.** How the topology describes a net
   place, and that the Python runtime gains it in the same round, since the
   two share the protocol and its transcripts.
10. **Viewers.** The closed symbol's layout, and what the thumbnail, the
    history, the inspector and the filters do with it.
11. **Bounds and failure.** Token bounds on a net place, and what a failed
    firing restores when it had delivered into one.
