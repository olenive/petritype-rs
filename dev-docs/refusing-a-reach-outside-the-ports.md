# Refusing a reach outside the ports

**2026-11-28: stage two is built in Rust.** Every net is closed unless its
definition declares it open, so a net that marks no port takes no token
from outside, and a body is refused the knobs, the controls and firing by
name of every net it holds. `UNDER_RULE_WITHOUT_PORTS` and
`Net::is_under_rule` are gone. The owner approved it on 2026-11-27, ahead
of the notice to the games' agents planned below, so that a drawing and
the code cannot disagree (`DESIGN.md` §6.9 has the reason). Holding a body
to its edge's `reaches`, refusing it every closed net inside the one it
holds, and shielding the nets inside a closed net were built here the same
day, after Python. What follows is the record as it was.

## What was decided, and what stage one built (2026-11-23)

Oleg read the findings below and went further than their recommendation
(choice A, bodies only). His words: "in general, we don't want to be
injecting tokens arbitrarily into places in the Petri net... definitely
the bodies should not be doing that. I'd say unless a net has been
specifically flagged as one where we expect to inject tokens, that should
also throw an error if your program does it. But if the user wants to
override that by specifying that in the net definition, I think that's
fine. I would rather err on the side of being too strict here."

It lands in two stages. **Stage one, built the same day in both runtimes:**
a net that marks any place as a port is **closed**, and from outside it
tokens are put, taken, removed, replaced or set only at its ports, for
everyone outside: a body of the net around it, the program and its
drivers, and the runner's `inject`, at the top level and by path. A net
that marks no port stays open. A net may declare itself **open** in its
definition (`NetBuilder::open`, Python `open=True`), which lets anyone
outside write to any of its places, and the wire shows `open: true` on its
topology. **Stage two**, not built, closes every net that does not declare
itself open, after the games' agents have had a note and time to mark
their ports; in Rust it is one constant (`UNDER_RULE_WITHOUT_PORTS` in
`petritype/src/closed.rs`).

What stage one is, in Rust (`DESIGN.md` §6.9 has it in full):

- **Tokens.** The check is on the net, in every public token-writing
  method (`put`, `extend`, `set_tokens`, the new `try_put`, `try_extend`
  and `try_set_tokens`, `remove`, `replace`, `apply`), so it holds whoever
  calls, a body by any edge, `nested_scope` and `subnet_scope` closures
  included. Seeds, outputs, restores, imports, forks, deep steps and a
  net's own `press` are inside and go round it. A window (a net's place for
  a port of its subnet place) is a place of the holding net, judged by that
  net's rule first.
- **Knobs and firing by name** stay with the program, its drivers and the
  runner on any net. A body is refused them on a net it holds that marks a
  port, closed or open, through the mark of §4 below, which a firing sets
  on each net it lends or hands its body and which lapses when the firing
  ends. A body on a net with no port keeps them in stage one, so the
  transit cabins' `stopped` parameter, set by the city's bodies, is
  unchanged.
- **A refusal** is `NetError::ClosedNet` for a token and
  `NetError::HeldByBody` for a knob (the note's `OutsidePorts` became the
  two), each naming the net and the act, the first also the place, and
  saying how to put it right. For a body it is an ordinary failed firing
  with that error, also when the body dropped the `Err`; `put` and its
  kin panic with its message for the program; the runner answers
  `closed_net`. What a failure restores is unchanged, and a consuming
  firing that takes a token carrying a net cannot restore it in the Rust
  core (`RestoreOfNoClonePlace`).
- **What broke in petritype-rs:** one runner test, which injected into a
  level's `Arena` by path; its hero now walks in from the lobby. The three
  `subnet_saving` tests and the drivers' `OnFailure::Disable` set knobs from
  the program and stay allowed. The eighteen recorded fixtures replay
  unchanged, and `post-office` with the nineteenth shows the rule.

The sections below are the findings as they were written for the decision,
and their counts stay as the record of what was found. Where they say
"nothing here is built", or that the program and the runner stay outside
the rule, the decision above has replaced them.

## The findings, as written for the decision

Written 2026-11-23 for Oleg's agreement, as step 3c of the edges spec.
Nothing here is built. The rule agreed in outline: a net that marks any
place as a port can be reached from the net around it only through its
ports, a body that touches another of its places fails the firing, and a
net that marks no port stays open. This note says what the rule would cost,
what it covers and how it would be built, and asks for one choice (§6).

## 1. What reaches into a net from outside today

| Way in | Rust | Python | What it can do inside |
|---|---|---|---|
| Mutate or mutate-where edge on a place of nets | `&mut T`, so `&mut Net` | the live token | anything: every `Net` method, or swap the net |
| Take, take-where, take-all or drain edge | the token, owned | the token | anything, then return it to an output place |
| Read edges | `&T`, `Vec<&T>` | the live token | read (in Python anything, on the author's promise) |
| Snapshot, survey | a build error | a deep copy under a new `net_id` | nothing: it holds a copy |
| Guard, key function | `&Net`, `nested`, `subnet` | the live graph | read |
| The program | `nested_scope`, `subnet_scope`, the drivers | `NestedNets.put/take/set_param`, `subnet_scope`, the graph | anything |
| The runner, by `path` or `nested_id` | `inject`, and the reads | the same | append a token to any place |
| A control's handler | `&mut Net` of its own net | a command on the outer net | what the program can |

The runners' `step`, `enable`, `disable`, `set_param` and `press` take no
`path` (`invalid_params`). A hosted net (`HostServer`) answers reads only.

## 2. Who writes outside the ports

Counted from the code on the `edges-spec` branches, 3b's `trays` net
included. A write is a put, take, removal, replacement or mutation of a
token, a parameter, a switch, or a firing by name.

| Where | Bodies that write inside a net | into a net with ports | outside its ports |
|---|---|---|---|
| petritype-rs | 16 | 5: `trays` Hand Over and Collect, `tests/reaches.rs` twice, an `edge.rs` doc | 0 |
| petritype (Python) | 9 | 1: Hand Over in `test_ports_and_reach.py` | 0 |
| graph-tactics, firing range | 4 per node, 4 in a unit | 0: units and soldiers mark no port | 0 |
| graph-tactics, main game | 0: its tokens carry no nets | | |
| Unfamiliar | 0 | | |
| Cyberpunk2020-Simulation | 8: six tell an ICE ladder, two rebuild units | 0: a ladder marks no port | 0 |

**No body breaks.** Every place graph-tactics' bodies write in a unit or a
soldier is one of its trays (a unit's orders, shots out, hits, leaving for
and arrivals; a soldier's orders, shots fired and hits), so the game can
mark its trays as ports and keep working. Cyberpunk builds against a pinned
clone of petritype-rs at `224bcbe`, and meets the rule when it moves on.

The writes outside the ports of a net with ports come from the program and
the runner, which §5 leaves outside the rule:

- petritype-rs: three tests in `tests/subnet_saving.rs` set a subnet's
  parameter, tie-break or switch inside `subnet_scope`
  (`a_restored_net_steps_as_the_original_would_at_every_level`,
  `a_fork_copies_every_level_and_is_independent_of_the_original`,
  `an_import_tells_each_subnets_resync_and_then_its_own`); the drivers'
  `OnFailure::Disable` switches off a failing transition inside a subnet
  (`disable_at` in `drivers.rs`, tested by
  `the_drivers_switch_off_a_failing_transition_inside_a_subnet_and_count_its_firings`);
  and the runner test
  `a_subnet_place_s_entry_holds_its_subnet_and_its_ports_under_the_token_limit`
  injects into `Arena` by path.
- Python: that test's twin in `test_runner_subnets.py` does the same.
- graph-tactics: `GameNet::deal` puts units into the map's windows on its
  locations' ports, which are not ports of the map, and `GameNet::clear`
  empties every place of the map and its locations. `Game::new` and
  `Game::restart` run them.
- Both libraries: loading, importing, forking and resetting a marking.
- Stepping, which §3 keeps:
  `the_index_of_a_port_is_its_subnets_and_both_stay_right` and
  `a_subnet_steps_under_each_rule` in Rust,
  `test_a_passive_subnet_is_the_programs_to_step` in Python, and
  graph-tactics' `Game::ask_order`.

Writes into nets with no port, which the rule leaves alone: 33 in Rust (11
by bodies, 10 by the program, 12 by the runner), 23 in Python (8, 11, 4),
graph-tactics' trays bodies, Cyberpunk's eight, and Unfamiliar's 12 calls
of `WorldNet::with` into its metabolism nets.

## 3. What touching covers

From outside, on a net that marks a port:

- **Refused:** putting, taking, removing, replacing or mutating a token in a
  place that is not a port; setting a parameter; switching a transition;
  firing one by name (`fire`, `launch`, `distribute`); setting a stepping
  rule, a selector or a tie-break; importing a marking; emptying its log,
  from which the outer log is lifted; `subnet_scope`, since a subnet place
  is never a port; `nested_scope` on a place that is not a port.
- **Kept:** anything at a port, and `nested_scope` on a port of
  net-carrying tokens, whose nets are then under the rule against their own
  ports. Reading anything: counts, tokens, keys, `fireable`, `query`,
  `nested`, `subnet`. Guards and key functions read inside today, and a read
  leaves the net as it was. Stepping (`step`, `step_deep`): the net's own
  rules choose what fires, and a passive place exists so that an outer body
  can step its nets, as Cyberpunk's bodies step an ICE ladder.

**`subnet_scope` is the hard case.** A body is handed tokens, never its
own net, so the callers of `subnet_scope` are the program, the drivers and
the runner's `inject`. It stays their tool for setting up, resetting,
switching off a failure and stepping a subnet alone, as graph-tactics uses
it. A body could call it only on a token's net that holds a subnet, two
levels up; none does, and the rule refuses it when the token's net marks a
port. A subnet may also mark no port at all, and then only the program
reaches it.

## 4. Where the rule lives, and what a refusal is

**Rust.** A private mark on `Net`, set through `Nested::nets_mut` on each
net of a token a body holds: by `borrow_mutated` for a mutate edge until
the body returns, and by `consume` for a consuming edge until the token
lands in a place again (`insert_erased`, `restore_consumed`), so it travels
with the token to a worker thread. Every public writing method of `Net`
checks it first, at the cost of a field and the place's `port` flag. Those
that return `Result` (`remove`, `replace`, `set_param`, `fire`, `enable`
and the rest) return a new `NetError::OutsidePorts` naming the net and the
place or act. `put`, `extend` and `set_tokens` panic with it, as they
already panic on another net's handle, and `call_body` turns the payload
back into that error. The mark keeps the first refusal, and the firing
fails with it after the body returns, also when the body dropped the `Err`.
No global state, no unsafe code. A body that swaps a token's net for one it
built has made a new net, and is let through (Cyberpunk's Settle Fortress
and Repair build new units).

**Python.** The same mark, as an attribute on each inner graph that stage 1
hands a body by any edge, read edges included, cleared in stage 3.
`NestedNets.put/take/set_param`, `NetParams.set`, `subnet_scope` and firing
by name check it and raise `OutsidePortsError`. A body holds the live graph
and can edit `place.tokens`, `graph.params` or `graph.disabled` directly,
so stage 1 also records the token ids and token objects of each private
place of a lent net, with its parameters and switches, and stage 3 fails
the firing on any difference. A change inside a token object of a private
place goes unseen, and stays the author's promise, as a write through a
read edge does.

**A refusal** is an ordinary failed firing in both runtimes: `Failed` in
the log, `failed` on the wire with the error, `FiringFailed` to the caller,
and nothing distributed. Consumed tokens go back under
`restore_tokens_on_failure`, and are lost without it, a drained unit
included, as in any failure. What the body did before the refusal stays
done and is reported; the refused call itself changed nothing. Rolling the
inner net back would need a copy of it per firing: a fork in Rust, which
needs templates and `serde`, a deep copy in Python.

## 5. Who is outside

| Who | Under the rule |
|---|---|
| A body of a transition of the net around it, by any edge | Yes. A read edge in Rust has nothing to refuse. |
| A body two levels up | Yes, through the middle net's mark, then the inner one's. |
| A guard or key function of the net around it | Yes, and it only reads. |
| The net's own bodies, guards and keys | No: they are inside. |
| The program, its drivers and a control's handler | No. `subnet-place-spec.md` §1.1 already lets the program step or change a subnet inside a scope. |
| The runner's requests by path | No. Only `inject` writes. It is a person at the viewer trying a net, and goes on landing in any place. |

## 6. The choices

**A. On for every net that marks a port, for bodies.** The spec's form.
Breaks nothing counted in §2. Protects every net with ports from the day it
marks them, with nothing to switch on, and makes its drawing true: the
ports are marked (one dotted outline since 2026-11-28, a second ring before)
and an edge's `reaches` names where it ends inside.

**B. On only where asked.** A net says so (`b.sealed()`, `sealed=True`),
or an edge's `reaches` becomes a limit on its body. Breaks nothing until
used. Protects only what asks; in the edge form a net's author cannot
insist. It adds a second word beside `port` for one idea, and a field on
the wire.

**C. Nets in tokens now, subnets later.** A subnet is held to its ports
for bodies already, when the net is built: an outer edge can name only a
port. So for bodies C is A. What it leaves for later is holding the program
and the runner too, which would break the five uses in petritype-rs, the
one in Python and graph-tactics' `deal` and `clear` listed in §2, and the
loading of a marking.

**Recommendation: A.** One rule with no switch, the one subnets already
have at build, at no cost to any repository counted, and in place before
graph-tactics marks its trays. Holding a body to the ports its own edge
names is cheap on top of the mark, if wanted later.

## 7. Building A

1. Rust core: the mark, the checks, `NetError::OutsidePorts`, the payload
   in `call_body`, the check after the body. Tests for each method and edge
   kind, for stepping and reading kept, and for a deeper net marked through
   `nested_scope`.
2. If 3b leaves it open: a name in an edge's `reaches` that is not a port
   of the token's net fails the firing, named, when it fires.
3. Python: the mark, the checks, the record in stage 1 and the comparison in
   stage 3, `OutsidePortsError`, the same tests.
4. A conformance net and the nineteenth recorded fixture: an outer body that
   writes to a tray and lands, one that writes to a private place and fails
   (`failed`, with `restored`), and an `inject` by path into the private
   place that lands.
5. `PROTOCOL.md` §12, DESIGN §6.6, the READMEs and both changelogs. Then
   graph-tactics may mark its trays as ports, five in a unit and three in a
   soldier.

The wire stays as it is. A refusal is a `failed` firing, whose `error` the
fixtures write as `<label>`, and the rule follows from `port`, shown since
3a. The protocol stays version 2. The eighteen recorded fixtures re-record
to the same bytes: every write they hold into a net with ports lands at a
port (`Exit Hall` by path in `09_subnet_places.json`, the trays in the
eighteenth).
