# Notify observers when a transition goes in flight, in sequential mode too

Written 2026-10-17 for the petritype owner on behalf of an in-process
consumer of the Python runtime, after reviewing petritype at v0.3.0 (1226a85)
and petritype-rs at a12e13c. The owner decides; what follows is a proposal
shaped by one consumer. It is kept in petritype-rs's `dev-docs/` because the
Rust engine already has the launch event this asks for and the two runtimes
are meant to agree. `NOTE_ON_IN_FLIGHT.md` records what was built on 17
October 2026.

## 1. What the consumer shows, and what it cannot

The consumer runs small nets through `Runner.run` in process, one graph per
call, and a dashboard draws each run as a strip of tiles. Its observer
switches `record_events` on and, at every notification, drains the engine's
log and publishes two events per `Fired` record: one that marks the
transition as the one working, and one that clears it and shows the body's
duration.

Because the log holds a firing only once it is complete, the two events are
published back to back and the "working" state lasts no time at all. A body
that runs for seconds shows nothing until it is over. The tool built to show
the process is blind during exactly the part worth watching.

Some of its nets run in concurrent mode, where its observer can already
notice a launch by reading `graph.in_flight`. In sequential mode there is
nothing to notice, which is this request.

## 2. What the library does now, verified at v0.3.0

- `ExecutableGraphOperations.fire_one_transition` assigns
  `executable_graph.in_flight = {transition.name}` after stage one and
  before stage two, and clears it in a `finally`
  (`petritype/core/executable_graph_components.py:2509` and `:2535`).
  petritype-rs's `NOTE_ON_IN_FLIGHT.md` records why this is a field on the
  graph rather than a callback: a fact with a home on the graph is read by
  every reader from there, and a heartbeat on another thread can poll it.
- `Runner._run_sequential` (`petritype/runtime.py:837` to `:852`) notifies
  observers at the initial marking, after inbox input, and after each call
  to `_fire_one` returns. No notification falls between the assignment of
  `in_flight` and the call of the body. An observer on the same event loop
  therefore never sees a non-empty `in_flight` in sequential mode. The tests
  in `TestSequentialInFlight` (`tests/test_runtime_observers.py:151`) read
  the field from inside the body or from a watcher task, never through an
  observer.
- `Runner._run_concurrent` notifies when the in-flight set changes
  (`petritype/runtime.py:904` to `:906`, "newly in-flight transitions light
  up as working") and again at deposits. The two modes give observers
  different pictures of the same fact.
- The engine's log reports a firing once, as `Fired`, when it completes;
  `petritype/core/events.py` says so and adds that Python emits no
  launched/distributed pair. The protocol runner reports `launched` to its
  clients all the same, because it stages the firing itself and keeps its
  own `_InFlight` record (`petritype/runner/owner.py`). petritype-rs's engine
  logs `Event::Launched` for a staged firing and `Event::Fired` for its
  sequential path, "where no time passes between the two"
  (`petritype/src/event.rs`).

## 3. The request

Notify the observers the runtime already has at the moment `in_flight`
changes, in sequential mode as the concurrent loop already does. No new
channel and no new reader API: observers read `graph.in_flight`, which is
already the documented home of the fact. One notification per firing before
the body runs, in addition to the one after it returns.

This keeps to the rule in `NOTE_ON_IN_FLIGHT.md`. That note chose the field
over a callback so that readers would not each need a channel; it did not
decide that the runtime's observers, who are notified on every other change
of the graph, should be the one reader left out.

## 4. Two ways to do it

**A. A launch notification inside the sequential primitive.** The smallest
change. `fire_one_transition` takes an optional awaitable `on_launch`,
awaited once right after `in_flight` is assigned and before stage two.
`_fire_one` passes the runtime's `_notify(ctx)`; `Runner.step` gets it
through the same path; `_fire_now` (the `Fire` command) passes nothing or
the same, as the owner prefers. Nested nets: an inner firing reached by
descent goes through the same function, so the outer observer would be
notified for inner launches too. That matches how inner firings already
count as firings of the outer net; if it is unwanted, pass `on_launch` only
when `descend` is at the outer level.

**B. One staged path for both modes.** The sequential loop fires through the
primitives the concurrent loop uses (`_launch`, await the task, `_complete`,
`petritype/runtime.py:614` and `:644`), with a notification between launch
and await, so both modes share one firing path and one notification pattern.
Bigger: the sequential primitive also carries nested descent, the pinned
selector behind `Fire`, net parameters and restore on failure, and all of
that would need to move or be duplicated.

A is enough for the consumer and leaves nothing to migrate. B is worth it
only if the owner wants one path for its own sake.

Cost either way: one more observer call per firing. Observers are already
called once per firing, and the consumer's drains an empty log and publishes
token counts, which is cheap.

## 5. Optional, and separable: a `Launched` record in the log

A second, smaller request that stands or falls on its own. Write
`Launched(transition, consumed)` to the engine's log when `in_flight` is
assigned, in both modes, and leave `Fired` as it is: the whole firing,
reported at completion. Reasons for it:

- An observer that drains the log, as the consumer's does, learns the launch
  from the log, without diffing `in_flight` between notifications.
- At launch only the engine knows which tokens left which places. The net's
  argument edges name a transition's declared inputs, but a drain edge may
  have taken nothing, and the declared set is not the taken set.
- Parity with petritype-rs's `Event::Launched` and the protocol's
  `launched`, both of which already carry exactly `transition` and
  `consumed`.

Against it: the statement in `events.py` that a firing is reported once
would need rewording, and any reader that counts records as firings would
count double. The consumer counts `Fired` only. The notification in section 3
unblocks the consumer by itself; this record would make the consumer's code
simpler and more exact.

## 6. Tests and acceptance

In `tests/test_runtime_observers.py`, beside `TestSequentialInFlight`:

- A one-transition net run sequentially with an observer that appends
  `set(graph.in_flight)` on each call sees `[set(), {"Work"}, set()]`:
  initial marking, launch, completion.
- A raising body: the observer sees the launch; after the raise the set is
  empty and `TransitionFailedError` still propagates.
- `RunSummary.n_fired`, `fired_counts`, `step_count` and the log are
  unchanged by the extra notification.
- `Runner.step` gives the same launch notification as `Runner.run`.
- Whatever is decided for inner nets and for `Fire` is pinned by a test.
- If section 5 is adopted: the log of one firing reads `Launched` then
  `Fired`, or `Launched` then `Failed`, in both modes.

Acceptance: an observer attached to `Runner.run` in `SEQUENTIAL` mode is
called with a non-empty `in_flight` once per firing, before the body runs,
and everything else about the run is as before. The concurrent loop's
behaviour does not change.

## 7. For petritype-rs

The Rust engine already reports `Launched` on its staged path and
`in_flight` is already a field there, so the question is the same one in
different clothes: does a viewer attached to the sequential `step` or `fire`
path get a moment to draw the working transition before a long body
returns? If the body runs inline and the only event is `Fired` afterwards,
a long body is invisible to that viewer in the same way. The two rules in
`NOTE_ON_IN_FLIGHT.md`, assign rather than mutate and clear on failure,
apply to whatever is added.
