# petritype-rs

Typed, executable, observable Petri nets in Rust. A port of
[petritype](https://github.com/olenive/petritype) (Python).

*Work in progress: declaring a net, all three output forms, and firing either
inline (`step`, `fire`) or in three stages (`launch`, `run`, `distribute`, so
bodies may run on other threads) work, and failures (an `Err`, a panic) are caught with optional restoring of
the consumed tokens (slices 1–7 of `DESIGN.md` §18); the opt-in build-time
checks, drivers and the visualiser are still to come. See `DESIGN.md` for the design and
`dev-docs/NAMING_CONVENTIONS.md` for vocabulary shared with the Python library.*

## Core Idea

A Petri net has two kinds of nodes connected by directed edges:

- **Places** hold tokens of one declared type.
- **Transitions** consume tokens from input places, run a function, and
  distribute the results to output places.

A transition can fire when every input place has a token. Firing consumes the
inputs, runs the body, and distributes the outputs. The tokens across all
places are the live state of the system.

## Anatomy of a transition

| term | what it is | who writes it |
|---|---|---|
| **transition** | name + input edges + output edges + body + optional output distribution function + optional guard and priority | you, in the builder |
| **body** | the domain function; knows nothing about places | you, usually it already exists |
| **output distribution function** | turns the body's result into "which token goes to which place"; optional; composed onto the body with `body.output_distribution(f)` | you, next to the net, where the place handles are in scope |
| `Distribution` | the value an output distribution function returns: typed (place, token) pairs | returned, not written |
| `distribute` | the engine step that applies whatever the body or distribution function returned | nobody; internal |

Where outputs go is decided by the return type of the body, or of the output
distribution function if there is one:

- **Positional**: a tuple. Slot *i* goes to output place *i*. Use when several
  output places share a type.
- **Routed**: any non-tuple value. Each emitted value goes to the output place
  of its type; order of output places doesn't matter. Use when types are
  distinct.
- **Distribution**: explicit (place, token) pairs. Use for data-dependent
  routing among same-typed places, or dynamic fan-out.

The output distribution function exists so domain functions stay unchanged when
the net's connectivity changes: the routing decision lives beside the net, not
inside the function that computes the result.
