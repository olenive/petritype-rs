# petritype-rs

Typed, executable, observable Petri nets in Rust. A port of
[petritype](https://github.com/olenive/petritype) (Python).

Version 0.1.0, an early release.
The API will change.

## Installation

The crates are not yet on crates.io, so depend on this repository with git:

```toml
[dependencies]
petritype = { git = "https://github.com/olenive/petritype-rs" }
```

Add `features = ["serde"]` for JSON commands and for saving and loading nets. The
quickstart below also prints its net as Graphviz DOT, which needs `petritype-dot` as well:

```toml
petritype-dot = { git = "https://github.com/olenive/petritype-rs" }
```

## Requirements

Rust 1.96 or newer, edition 2024. 1.96 is the minimum supported Rust version, and CI
checks the workspace against it.

## Build and test

```sh
cargo test --workspace --features serde
```

`just check` runs the full gate: that suite, the core crate alone, clippy with warnings
denied, the formatting check and the docs with warnings denied. `just` comes from cargo
(`cargo install just`) or Homebrew (`brew install just`).

The runner's tests also replay the protocol transcripts shared with the Python runtime,
read from `../petritype/tests/protocol` (the Python repository checked out beside this
one) or from the directory in `PETRITYPE_PROTOCOL_TRANSCRIPTS`. With neither, that test
passes with a note and replays nothing; CI checks out both repositories side by side, so
it runs there.

## Documentation

```sh
cargo doc --workspace --no-deps --features serde --open
```

`DESIGN.md` has the design, and `dev-docs/NAMING_CONVENTIONS.md` the vocabulary shared
with the Python library.

## Running the runner example

```sh
cargo run -p petritype-runner --example parcel_runner -- --describe
cargo run -p petritype-runner --example parcel_runner
```

The first prints the nets the example carries. The second serves them over the net
protocol: it reads one JSON request per line on stdin, such as
`{"id":"1","method":"load","params":{"net":"parcel-fleet","params":{"trucks":3}}}`, and
writes its answers and notifications on stdout.

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
and ferries built this way (`cargo run -p petritype --example transit --features serde`), whose journey runs under plain
`step_deep` because each vehicle's state is where the tokens in its cabin are, each door in `Doors Open` or
`Doors Closed`, which the city's guards read; `DESIGN.md` §6.6 has the rules.
A net whose outer transitions cannot wait on what is inside like that sets `b.deep_policy(DeepPolicy::Interleaved)`,
and `step_deep` and the drivers then take it and each inner net in turn, so that an outer net that can always fire
no longer starves its inner nets; `b.tie_break(TieBreak::RoundRobin)` takes a net's own transitions of equal
priority in turn the same way. Both default to what they always did.
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

`petritype/examples/wildlife.rs` asks the token-keys proposal's five questions of species across habitats, migrates
its birds through keyed take edges, and spreads the egret and thins the deer and the lemmings through keyed mutate
edges, each guard reading the token its edge would take (`cargo run -p petritype --example wildlife --features serde`);
`DESIGN.md` §8.3 has the rules.
Served by `petritype-runner`, every token carries its keys on the wire, and a host can `query` the net, list a key's
`distinct` values, or `set_filter` so that it reads and hears of only the tokens a filter keeps, the rest told as
`hidden` (PROTOCOL.md §13, `DESIGN.md` §19.3.12).

## Input edges

An input edge says how a transition reads a place, and so what its body is handed:

| edge | consumes | body parameter | the place must hold |
|---|---|---|---|
| `TakeEdge(p)` | the front token | `T` | a token |
| `TakeAllEdge(p)` | every token | `Vec<T>` | a token |
| `SnapshotEdge(p)` | nothing (a clone of the front token) | `T` | a token |
| `MutateEdge(p)` | nothing (the front token, borrowed) | `&mut T` | a token |
| `TakeWhereEdge(p, filter)` | the oldest token the filter accepts | `T` | a token the filter accepts |
| `MutateWhereEdge(p, filter)` | nothing (that token, borrowed) | `&mut T` | a token the filter accepts |
| `DrainEdge(p)` | every token, zero or more | `Vec<T>`, possibly empty | nothing |
| `SurveyEdge(p)` | nothing (clones of every token, zero or more) | `Vec<T>`, possibly empty | nothing |

The last two are for a place a transition only needs to read, or to empty, when something is there. A shop prices an
order with whatever promotions are running, and a shop with none running still sells things; a traffic light lets
everyone waiting go in one firing while it is green:

```rust
b.transition("Price Order", (TakeEdge(orders), SurveyEdge(promotions)), (priced, discounts_applied),
    |order: String, promotions: Vec<String>| { /* one discount per promotion, maybe none */ });
b.transition("Go", (DrainEdge(waiting), SnapshotEdge(light)), (passed,),
    |cars: Vec<String>, _green: Colour| { /* every car waiting goes; the light is only read */ })
    .guard(move |net| colour(net, light) == Some(Colour::Green) && net.count(waiting.id()) > 0);
```

Without them each transition needs an "empty twin" guarded on the place being empty. A drain consumes but may take
nothing, so `assert_no_source_transitions` does not count it: a transition whose only input is a drain fires for ever,
and `Go`, whose other edge only reads, would fire at every green taking nothing, which is why its guard asks for someone
waiting as well. `petritype/examples/shop.rs` runs the shop and `petritype/examples/traffic_light.rs` the light, one
place whose token is its colour, moved on by an unguarded `Change`
(`cargo run -p petritype --example shop --features serde`, and `--example traffic_light`); `DESIGN.md` §4.4 has the
rules, and on the wire the two are `"drain"` and `"survey"` (PROTOCOL.md §5.1).

## Controls and token styles

A net can declare, as data, how its tokens look and which buttons it wants. A viewer draws them with widgets it
already has and never names the net; pressing one runs code written beside the net, through the net's own API, so
whatever it does is an ordinary event. The traffic light colours its token as the lamp it lights and fills its place
with it, and offers two controls, a text box that adds a car and a button that changes the light:

```rust
b.styled(light);                                   // Colour: TokenStyle, red #ff6347, green #32cd32, amber #ffa500
b.place_style(light, PlaceStyle::fill_token());    // Light is drawn in its token's colour
b.control(Control::fire("change", "Change", change));
b.control(car);                                    // + Car: a text box on Waiting, `Car N` when nothing is typed

net.press("car", Value::Text("Taxi".into()))?;     // Pressed::Applied: an `Added` in Waiting
net.style(light.id(), token)?.colour;              // Some(Rgb(0xff, 0x63, 0x47)) while it is red
```

A press's value is checked against the control's kind (a button none, a toggle a boolean, a slider a number in range,
a select one of its options, a text box a string) before the handler runs. Served by `petritype-runner` the controls
are `topology.controls`, a press is `press {control, value?}`, and a styled token carries its `color` (PROTOCOL.md
§5.1, §6). `petritype/examples/traffic_light.rs` presses both after its script (`cargo run -p petritype --example
traffic_light --features serde`); `DESIGN.md` §18 has the decisions.

## Binding by name

A body is bound to a net by position (a tuple of inputs is its parameter list, a returned tuple fills the outputs in
order), by type (a routed return), or by name: a library function takes and returns structs that name no place and no
kind of edge, and each net wires their fields its own way.

```rust
#[derive(Inputs)]
pub struct Pricing { pub order: Order, pub promos: Vec<Promo>, pub prices: Prices }

#[derive(Outputs)]
pub struct Priced { pub order: Order, pub note: Option<Note>, pub restock: Vec<Item> }

pub fn price(p: Pricing) -> Priced { /* knows nothing about places */ }

b.transition(
    "Price Order",
    (PricingEdges { order: TakeEdge(orders), promos: SurveyEdge(promos), prices: Param::new() },),
    PricedEdges { order: priced, note: notes, restock },
    price,
);
```

`PricingEdges` and `PricedEdges` are generated: each field takes any input that reads its type (`TakeWhereEdge` or
`SnapshotEdge` would do for the wholesaler next door) or any place that accepts it, the order of the fields is the order
of the edges the net and the wire see, and a wrong kind is a compile error at the field. A struct of inputs is one
position of the input tuple; a struct of outputs is all of the outputs. Neither has the tuples' cap.
`petritype/examples/pricing.rs` wires the library into a shop and a wholesaler (`cargo run -p petritype --example
pricing`); `DESIGN.md` §4.4 and §4.5 have the rules.

## Crates

| crate | what it is |
|---|---|
| `petritype` | the builder, `Net`, events, checks, drivers, commands, snapshots |
| `petritype-derive` | `#[derive(Routed)]`, `#[derive(Inputs)]` and `#[derive(Outputs)]`, re-exported as `petritype::Routed`, `petritype::Inputs` and `petritype::Outputs` |
| `petritype-dot` | Graphviz DOT text of a `Net` or a `Snapshot` |
| `petritype-layout` | the layered layout of a net (ranks, orders, clusters, ghosted hubs) in plain `Point`s and `Bounds` any renderer converts; the Bevy visualiser (not yet public) draws with it |
| `petritype-activity` | a net's activity as a fast and a slow level per place and transition, fed by its events and decaying on wall time or on firings, and the activity thumbnail it lights: a grid of cells from the layout, a colour per cell and an SVG writer, with no graphics dependency |
| `petritype-runner` | turns your binary into a net process speaking the shared [net protocol](https://github.com/olenive/petritype/blob/main/PROTOCOL.md) on stdin and stdout, for any host that spawns it |
| `petritype-bevy` | separate repository, not yet public: a Bevy visualiser that mirrors a running net into entities |

## Core Idea

A Petri net has two kinds of nodes connected by directed edges:

- **Places** hold tokens of one declared type.
- **Transitions** consume tokens from input places, run a function, and
  distribute the results to output places.

A transition can fire when every input place has a token (or, for a drain or a
survey edge, whatever it holds, nothing included). Firing consumes the inputs,
runs the body, and distributes the outputs. The tokens across all places are
the live state of the system.

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

- **Positional**: a tuple. Return position *i* goes to output place *i*. Use
  when several output places share a type.
- **Named**: a struct that derives `Outputs`, whose fields go to the places
  wired to their names. Use for a library function more than one net wires.
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
