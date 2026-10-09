# petritype-rs

Typed, executable, observable Petri nets in Rust. A port of
[petritype](https://github.com/olenive/petritype) (Python).

*Work in progress: declaring a net, all three output forms, and firing either
inline (`step`, `fire`) or in three stages (`launch`, `run`, `distribute`, so
bodies may run on other threads) work, and failures (an `Err`, a panic) are caught with optional restoring of
the consumed tokens, the `drivers` module runs a net to quiescence or on a
clock, sequentially or with bodies on threads, erased `Command`s (JSON with
the `serde` feature) feed a running net from another thread, transitions
declare per-run `Param::<T>` values that never appear in the diagram, and
`net.snapshot()` hands another thread an owned copy of the state, and
`petritype-dot` draws a net or a snapshot as Graphviz DOT (slices 1–12 of
`DESIGN.md` §18, including the opt-in `assert_acyclic` /
`assert_no_source_transitions` checks). A token may carry Petri nets of its
own, which the net runs after its own transitions and reports on in its event
log (slice 15, nets as tokens). With `serde`, such a net is saved and loaded
as JSON, its tokens' inner nets rebuilt from templates registered by name,
and copied with `fork`, which is what lets it be snapshotted (slice 16). Tokens may carry keys, named values
their place derives from them, and a net answers queries by them from an index it keeps (slice 18); a keyed edge takes
or mutates the oldest token a filter accepts, leaving the others where they are (slice 19). The Bevy visualiser lives in its own
repository, `petritype-bevy`, and depends on this crate. See `DESIGN.md` for the design and
`dev-docs/NAMING_CONVENTIONS.md` for vocabulary shared with the Python library.*

## Quickstart

```rust
use petritype::drivers::{self, Limits, Mode, OnFailure};
use petritype::{NetBuilder, TakeEdge};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut b = NetBuilder::new();
    let input = b.place::<i64>("Input");
    let output = b.place::<i64>("Output");
    b.seed(input, [1, 2, 3]);
    b.transition("Double", (TakeEdge(input),), (output,), |x: i64| (x * 2,));
    let mut net = b.build()?;

    println!("{}", petritype_dot::dot(&net)); // Graphviz DOT of the net and its marking

    let summary = drivers::run(&mut net, Mode::Sequential, Limits::default(), OnFailure::Stop)?;
    let doubled: Vec<i64> = net
        .tokens(output.id())
        .filter_map(|(_, value)| value.downcast_ref::<i64>().copied())
        .collect();
    assert_eq!(doubled, [2, 4, 6]);
    assert_eq!(summary.fired_count, 3);
    Ok(())
}
```

Condensed from `petritype/examples/quickstart.rs`; `cargo run -p petritype
--example quickstart` runs the full version. The other examples in `petritype/examples/` port the
Python acceptance set (parcel distribution, caching, match-up tokens,
coloured balls, web scrape, execution modes, read-edge scaler); each is also
a test.

## Nets as tokens

A token can carry nets of its own: say which, and declare its place with `place_nested`.

```rust
impl Nested for Vehicle {
    fn nets(&self) -> Vec<(&str, &Net)> { vec![("cabin", &self.cabin)] }
    fn nets_mut(&mut self) -> Vec<(&str, &mut Net)> { vec![("cabin", &mut self.cabin)] }
}
let depot = b.place_nested::<Vehicle>("Depot");   // run by `step_deep` and the drivers, read by `net.nested(place, token)`
```

Inner events arrive in the outer log as `Event::Nested`, with the place, the token and the inner
net's id. With the `serde` feature, an inner net built by a registered template (`Templates::build`)
saves as `{"template", "marking", "params", "enabled"}` inside its token, `export_marking` and
`import_marking` save and load a whole outer net through JSON text, and `net.fork()` rebuilds a copy,
so `net.snapshot()` works on a net of vehicles too. `petritype/examples/transit.rs` is a city of buses
and ferries built this way (`cargo run -p petritype --example transit --features serde`); `DESIGN.md` §6.6 has the rules.
Served by `petritype-runner`, an inner net is addressed by its path (`transit/Depot/#0/cabin`) or its
stable nested id, and its events arrive wrapped in `nested` notifications (PROTOCOL.md §12, `DESIGN.md` §19.3.11).

## Keys and queries

A place can declare *keys*: named values, each a string, an integer or a boolean, that it derives from every
token. The token type needs nothing for them, and the net can then be asked for its tokens by them.

```rust
b.keys(forest, ["species", "class", "diet"], |s: &Species| [s.name.as_str().into(), s.class.into(), s.diet.into()]);

let birds = net.query(&Filter::new().key_in("class", ["Bird"]))?;   // each match: place, token, keys, label, JSON
let tabs = net.distinct_values("class", None);                      // Bird, Mammal, Reptile, Fish
```

A filter's clauses are ANDed and each accepts any of its values; it may be restricted to some places or to the
places whose name starts with a prefix, and a clause may compare a key with a net parameter. Keys are derived,
never stored, so a mutation changes them; the net keeps an index of them, so a query costs what it finds.

A *keyed edge* chooses its token by them: `TakeWhereEdge(forest, Filter::new().key_in("class", ["Bird"]))` takes
the oldest bird, wherever it stands, and `MutateWhereEdge` lends it to the body in place. Every other token stays
where it is under its id, so taking one token out of a crowd no longer means taking the crowd and putting it back.

`petritype/examples/wildlife.rs` asks the token-keys proposal's five questions of species across habitats and
migrates its birds through a keyed edge (`cargo run -p petritype --example wildlife --features serde`);
`DESIGN.md` §8.3 has the rules.
Served by `petritype-runner`, every token carries its keys on the wire, and a host can `query` the net, list a key's
`distinct` values, or `set_filter` so that it reads and hears of only the tokens a filter keeps, the rest told as
`hidden` (PROTOCOL.md §13, `DESIGN.md` §19.3.12).

## Crates

| crate | what it is |
|---|---|
| `petritype` | the builder, `Net`, events, checks, drivers, commands, snapshots |
| `petritype-derive` | `#[derive(Routed)]`, re-exported as `petritype::Routed` |
| `petritype-dot` | Graphviz DOT text of a `Net` or a `Snapshot` |
| `petritype-runner` | turns your binary into a net process speaking the shared [net protocol](https://github.com/olenive/petritype/blob/main/PROTOCOL.md) on stdin and stdout, for petritype-server and the Bevy visualiser |
| `petritype-bevy` | separate repository: a Bevy visualiser that mirrors a running net into entities |

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

## Licence

MIT. See [LICENSE-MIT](LICENSE-MIT).
