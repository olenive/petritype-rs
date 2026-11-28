# petritype-rs

Typed, executable, observable Petri nets in Rust. A port of
[petritype](https://github.com/olenive/petritype) (Python).

Version 0.3.0, an early release.
The API will change. 0.2.0 speaks version 2 of the net protocol, and 0.1.0
spoke version 1.

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
denied, the formatting check and the docs with warnings denied, clippy and the docs once
with serde and again for each crate that has the feature, built without it. `just` comes from cargo
(`cargo install just`) or Homebrew (`brew install just`).

The runner's tests also replay the protocol transcripts shared with the Python runtime,
read from `../petritype/tests/protocol` (the Python repository checked out beside this
one) or from the directory in `PETRITYPE_PROTOCOL_TRANSCRIPTS`. With neither, that test
passes with a note and replays nothing; CI checks out both repositories side by side, so
it runs there.

## Housekeeping

`just housekeeping` looks at the checkout and prints one line per check: the free space on
the disk, the size of `target`, how much of `target` was built by an earlier Rust and so can
never be used again, Time Machine's local snapshots, newer versions of the dependencies
(`cargo update --dry-run`), whether rustup knows of a newer Rust, and the advisory audit
when `cargo audit` is installed. It deletes nothing and changes nothing, and should a dry run
touch `Cargo.lock` it puts the file back byte for byte. It exits with 1 when something needs
attention and with 0 otherwise, so a scheduled run can stay quiet until then.
`just housekeeping --offline` skips the checks that need the network.

The limits are in the recipe: attention below 50 GB free, above 40 GB in `target`, and above
5 GB built by an earlier Rust. To change one for a single run, pass it again, as in
`just housekeeping --cargo-limit-gb 60`; to change it for good, edit the recipe in the
`Justfile`.

`just housekeeping-clean` lists the build output in `target` that the active Rust can never
use, because it was built before that Rust was installed, with its size, and deletes nothing.
`just housekeeping-clean --yes` deletes it. Adding `--older-than-days N` also takes the
per-variant folders, `target/<profile>/build/<crate>/<hash>/`, whose newest file is older than
N days; cargo builds them again when it needs them. The clean deletes files only inside the
profile folders of the build folder and never follows a link out of them. It stops while cargo
or rustc is running with the build folder (`--ignore-running` goes on anyway), and when the
build folder is outside the repository (`--outside-repo` allows that where `CARGO_TARGET_DIR`
names the folder). Time Machine snapshots are yours to delete: the report prints the command
when it counts some.

Both recipes run `scripts/housekeeping.py`, which needs Python 3.9 or newer and nothing else.
The same file serves the Python repository.

## Documentation

```sh
cargo doc --workspace --no-deps --features serde --open
```

[`DESIGN.md`](https://github.com/olenive/petritype-rs/blob/main/DESIGN.md) has the design, and
[`dev-docs/NAMING_CONVENTIONS.md`](https://github.com/olenive/petritype-rs/blob/main/dev-docs/NAMING_CONVENTIONS.md)
the vocabulary shared with the Python library.

## Running the runner example

```sh
cargo run -p petritype-runner --example parcel_runner -- --describe
cargo run -p petritype-runner --example parcel_runner
```

The first prints the nets the example carries. The second serves them over the net
protocol: it reads one JSON request per line on stdin, such as
`{"id":"1","method":"load","params":{"net":"parcel-fleet","params":{"trucks":3}}}`, and
writes its answers and notifications on stdout.

`--net <name>`, with either, carries the one net of that name alone, so that `describe` lists it only:

```sh
cargo run -p petritype-runner --example parcel_runner -- --net workshop
```

A shared protocol transcript is recorded against a runner carrying its own net alone, so this is how a host
replays one against the binary over pipes. A name the binary does not carry is refused with the names it does.
Every runner binary built on `petritype_runner::main` takes the option.

## A net inside your own program

A program that builds a net and fires it from its own code, a game or a simulation, can let a viewer watch it
while it runs. It gives the net to a `HostServer` and reaches it through the server from then on:

```rust
let mut server = petritype_runner::HostServer::bind("127.0.0.1:7878", "my-net", net)?;
// On every tick of the program:
server.change(|net| net.step())?; // fire, put tokens in, anything a `&mut Net` allows
server.serve(); // let viewers in and answer them; never blocks
```

`HostServer::bind_with` takes a `HostServerConfig` for its limits: viewers, line and backlog sizes, how much one
`serve` does, timeouts and the heartbeat. The server is for a viewer on the same machine; it authenticates and
encrypts nothing, so bind it to loopback.

A viewer connects to the port and speaks the net protocol line by line, starting with `get_state`. It reads the
net, inner nets included, and is told of every change; it cannot load, fire or change anything. The Bevy viewer
attaches with `cargo run --example remote -- --attach 127.0.0.1:7878` in `petritype-bevy`.

While no viewer has asked anything, the server makes no lines and compares no fireable sets, so `change` costs
less. It still keeps the log, the progress and the marking version, and a viewer that attaches later reads them as
if it had been watching all along.

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
coloured balls, web scrape, execution modes, and the scaler of `read_edge_scaler.rs`, whose snapshot and mutate
edges are non-consuming edges); each is also
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

Each nested place has a stepping rule, which says how the nets in it take their turns beside the outer net:

```rust
b.stepping(depot, Stepping::TakeTurns);   // or InnerFirst (the default), OuterFirst, Passive
```

Under inner first the nets in the place fire until they are at rest before the outer net fires. Under take turns the
outer net and each net in the place fire in turn, so an outer net that can always fire does not starve them. Under
outer first they fire only when the outer net has nothing to fire, which is the rule the transit city states on its
vehicle places, since its guards wait on what is inside. A passive place's nets are stepped by an outer body alone.
Each level follows the rules of its own places, and one `step_deep` fires one transition in the whole tree.
`b.tie_break(TieBreak::RoundRobin)` takes a net's own transitions of equal priority in turn the same way.
Served by `petritype-runner`, an inner net is addressed by its path, a list of steps from the loaded net down
(`[{"place": "Depot", "token": "0", "net": "cabin"}]`), or by its stable nested id, and its events arrive wrapped
in `nested` notifications (PROTOCOL.md §12, `DESIGN.md` §19.3.11 and §19.3.15).

## Subnet places

A place can hold one net of its own for the whole life of the net around it: a *subnet place*. The net around
it reaches the subnet only through the places the subnet's definition marks as ports, and its edges on a port
are ordinary edges.

```rust
let lobby = level.place::<Hero>("Lobby");
level.port(lobby);                                          // in the subnet's definition

let level_1 = b.subnet_place("Level 1", templates.build("Level", &Params::new())?);
let lobby_1 = b.port_of::<Hero>(level_1, "Lobby");          // a PlaceRef<Hero>, named by the pair
b.transition("Enter", (TakeEdge(door),), (lobby_1,), |hero: Hero| (hero,));
```

A port has one queue and one set of token ids, the subnet's, whichever net looks. A subnet steps under its
place's stepping rule like a token's net, its events arrive as `Event::Subnet`, `net.subnet(place)` reads it and
`net.subnet_scope(place, f)` steps or changes it. A subnet built by a template is saved, restored and forked with
the net around it, written in the marking as a list of one under its place. `petritype/examples/levels.rs` is a
world of two levels built from one template (`cargo run -p petritype --example levels --features serde`);
`DESIGN.md` §6.7 has the rules.
Served by `petritype-runner`, a subnet is addressed by the step `{"place": "Level 1"}`, the net around it names a
port by the pair (`{"place": "Level 1", "port": "Lobby"}`), and a change of a port moves the versions of both nets
that see it (PROTOCOL.md §12.7, `DESIGN.md` §19.3.17).

## Closed nets

Every net is **closed** unless its definition declares it open (since 2026-11-28; from 2026-11-23 only a net that
marked a port was): from outside it, tokens are put, taken, removed, replaced or set only at its ports, and a net that
marks no port takes no token from outside at all. A port is where tokens enter a net from outside, so a drawing shows
every way in: `petritype-dot` dots a port's outline and dashes every other place of a net declared open.
That holds for everyone outside: a body of the net around it, holding the net as a token by any edge, the program and
its drivers, and the runner's `inject`, at the top level and by path. Its own transitions, the handlers of its own
controls and the tokens its definition starts it with are inside, and the place a net has for a port of one of its
subnet places is a place of that net, so a write there from outside is refused unless that net is open.

```rust
b.port(inbox);                                   // from outside, only Inbox takes tokens
b.open();                                        // the override, in the definition: open, ports or not
post.put(ledger, letter);                        // panics on a closed net, Ledger not being a port
post.try_put(ledger, letter)?;                   // NetError::ClosedNet instead; also try_extend, try_set_tokens
```

`remove`, `replace` and an erased `Command` return `NetError::ClosedNet`, and `inject` answers `closed_net`. Since
2026-11-28 three more rules hold, so that the drawing shows every way a token comes in:

- **A body is held to its edge.** It writes a closed net its edge handed it only at the ports the edge names in its
  `reaching`; an edge that names none lets it write no port. Naming more than the body writes is no error.
- **A body writes no closed net inside the one it holds**, a net one of its tokens carries or a subnet, at a port or
  not: an edge names ports of the net it hands over only.
- **A closed net shields the nets inside it.** The program, its drivers and the runner write no place of a net inside
  a closed net, at any depth, at a port or not, through `nested_scope`, `subnet_scope` or `inject` by path or nested
  id, and import no marking into it. Declare every net around open, or give the closed net a port and a transition
  that carries the token in.

`ClosedNet::why` says which refusal it was (`ClosedRefusal`), and the message how to put it right. A body that is
refused fails its firing with the error, whether or not it passed the error on, and the firing restores what any
failure restores; a firing that took a token carrying a net cannot restore it in this core (`RestoreOfNoClonePlace`),
so a refused one loses it. A body that holds a net, closed or open, is also refused its knobs, its controls, firing
by name, draining its events, `choose`, a staged step and a driver's `OnFailure::Disable` (`NetError::HeldByBody`; a
staged step and `step_inside` answer `StepError::HeldByBody`, and the firing fails with the `NetError`).
Reading, stepping, driving and the whole-state acts stay allowed, and the knobs of a net inside a closed net stay with
the program. `DESIGN.md` §6.9 has the rules; `petritype-runner`'s conformance net
`post-office` shows them on the wire.

What a program changes:

- **How to tell a net is closed.** It does not declare itself open. On the wire its topology shows no `open`; in
  Rust, `Net::is_closed()`; in Python, `graph.is_closed`.
- **Three ways to make a refused write land.** Mark the place as a port; start the net with the token in its
  definition, as a seed or a template's token; or declare the net open (`b.open()`, Python `open=True`).
- **Knobs and firing by name** (a parameter, a switch, the tie-break, the stepping rule, a selector, `fire`, `launch`,
  `step_inside`, a control) stay with the program, its drivers and the runner on any net. A body of the net around is
  refused them on every net it holds, open or closed, since declaring a net open opens its places and not its knobs:
  give such a setting to a transition of the net's own, or leave it to the program.
- **A body's write at a port its edge does not name** is put right by naming the port in the edge's `reaching`. A
  write into a net inside the one the body holds goes to a port of the held net, and a transition of that net carries
  it further in.
- **A write or an import from outside into a net inside a closed net** is put right by declaring every net around
  open, or by giving the closed net a port and a transition that carries the token in.
- **Whole-state acts** (importing a marking, restoring, forking, resetting, saving) stay allowed for the program, but
  an import into a net inside a closed net is refused: import into the net around as a whole.

## Groups

A net whose parts all read and change one shared place cannot be split into subnets, so it stays flat and is read
by its parts: a *group* is a named set of a net's places and transitions that only drawings read.

```rust
let energy = b.place::<Energy>("Energy");                   // shared by both concerns, in no group
let walk = b.group("Walk");                                 // a GroupRef
let paid = b.place::<Leg>("Walk Paid");
b.in_group(walk, paid);                                     // a place or a subnet place
b.transition("Fund Walk", (TakeEdge(trail), MutateEdge(energy)), (paid,), |leg: Leg, energy: &mut Energy| {
    energy.0 -= LEG;
    (leg,)
})
.group(walk);                                               // a transition, where it is declared
```

A node is in one group or in none, and the net fires, steps, saves and logs exactly as it would with no groups.
`net.groups()` lists the names in declared order, `PlaceInfo::group` and `TransitionInfo::group` say which group
a node is in, and a subnet has groups of its own. `petritype-dot` draws each group as a dotted frame.
`petritype/examples/hike.rs` is a walker whose walking and eating both change its `Energy`, each concern a group
and the energy in none (`cargo run -p petritype --example hike`); `DESIGN.md` §6.8 has the rules.
Served by `petritype-runner`, each net's topology lists its own groups as `groups` and the `group` of each member,
for a viewer to draw (PROTOCOL.md §5.1, `DESIGN.md` §19.3.18). `petritype-layout` lays an open group out on its own
in a frame, the nodes in no group together with no frame, and a group named in `Layout::closed` as one box that
ranks with the nodes in no group, its edges merged into one arrow each way per neighbour.

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
A keyed edge may also choose by another input of the same firing, "the unit this order names":
`MutateWhereEdge(units, Filter::new().key_is_input("name", orders, "unit"))` beside `TakeEdge(orders)` hands the body
the oldest order whose unit is there, with that unit; an order whose unit is not there waits without holding back
the orders behind it.

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
| `ReadEdge(p)` | nothing (the front token, lent to look at) | `&T` | a token |
| `ReadWhereEdge(p, filter)` | nothing (the oldest token the filter accepts, lent) | `&T` | a token the filter accepts |
| `ReadAllEdge(p)` | nothing (every token, lent, zero or more) | `Vec<&T>`, possibly empty | nothing |

The edges that take nothing (snapshot, survey, mutate, the keyed mutate and the three read edges) are the
*non-consuming* edges. The read edges, since 2026-11-23, lend the body tokens to look at without taking or copying
them, so they ask no `Clone` of the token type and can read a place whose tokens carry nets; a transition with one runs
its body inline, as one with a mutate edge does.

Since 0.4.0 a transition may read one place through two or more take and take-where edges, each taking a token of its
own. Two oxygen radicals make a molecule, and the transition waits while the place holds one; a ballroom gives a couple
through two keyed edges, and the transition waits for a lady and a gentleman:

```rust
b.transition("Bond", (TakeEdge(radicals), TakeEdge(radicals)), (molecules,),
    |a: Radical, b: Radical| (Molecule(a, b),));
b.transition("Dance", (TakeWhereEdge(ballroom, role("lady")), TakeWhereEdge(ballroom, role("gentleman"))), (floor,),
    |lady: Dancer, gentleman: Dancer| ((lady, gentleman),));
```

The edges choose in the order they are declared, each the first token no edge before it took (a take the front one,
a take-where the first its filter accepts), and none gives its token up for a later one, as in the Python library: a
filter that accepts what a later filter needs goes after it. Any other kind beside another edge on its place is a build
error, `BuildError::EdgeCannotSharePlace`, and so is a second output edge of one transition to one place,
`BuildError::DuplicateOutputPlace`: one slot of `Vec<T>` puts several tokens there.

An edge on a place whose tokens carry nets may name the ports it reaches in them, `MutateEdge(field).reaching(["Orders
In"])`: the runner sends them as `reaches`, a drawing writes them on the edge, and since 2026-11-28 the body is held to
them. Every net shows its own ports on the wire, the nets that tokens carry included. A token's net is closed unless it
is declared open, so the body writes inside it only at the ports its edge names (see [Closed nets](#closed-nets)).

`DrainEdge` and `SurveyEdge` are for a place a transition only needs to read, or to empty, when something is there. A shop prices an
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
| `petritype-layout` | the layered layout of a net (ranks, orders, groups open in frames or closed as boxes, ghosted hubs, a subnet place as one box holding its ports) in plain `Point`s and `Bounds` any renderer converts; the Bevy visualiser (not yet public) draws with it |
| `petritype-activity` | a net's activity as a fast and a slow level per place and transition, fed by its events and decaying on wall time or on firings, and the activity thumbnail it lights: a grid of cells from the layout, a colour per cell and an SVG writer, with no graphics dependency |
| `petritype-runner` | turns your binary into a net process speaking the shared [net protocol](https://github.com/olenive/petritype/blob/main/PROTOCOL.md) on stdin and stdout, for any host that spawns it |
| `petritype-bevy` | separate repository, not yet public: a Bevy visualiser that mirrors a running net into entities |

## Core Idea

A Petri net has two kinds of nodes connected by directed edges:

- **Places** hold tokens of one declared type.
- **Transitions** consume tokens from input places, run a function, and
  distribute the results to output places.

A transition can fire when every input place has a token (a token for each
edge, when take edges share a place; or, for a drain or a survey edge, whatever
it holds, nothing included). Firing consumes the inputs,
runs the body, and distributes the outputs. The tokens across all places are
the live state of the system.

### Where this sits among Petri nets

In the literature a net whose tokens carry data is a *coloured Petri net*: a
token's value is its colour, and the type a place holds is its colour set. That
is what petritype builds, with Rust types as the colour sets, guards as guards
and a transition's body in place of arc expressions. Two things differ from
the textbook form: a place is a first-in, first-out queue where the textbook
has an unordered bag, and the net is run as the program itself, with no
state-space analysis. A token that carries a net of its own
([Nets as tokens](#nets-as-tokens)) is what the literature calls *nets within
nets*, or object nets. A place that holds a net of its own
([Subnet places](#subnet-places)) is nearest to the *substitution place* of the
hierarchical coloured nets, joined to the net around it through port places as
there, with a net of its own inside that steps by its own rule.

Colour in that sense has nothing to do with the display colour a token may be
given ([Controls and token styles](#controls-and-token-styles)); outside this
section, colour means the second.

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

MIT. See [LICENSE-MIT](https://github.com/olenive/petritype-rs/blob/main/LICENSE-MIT).
