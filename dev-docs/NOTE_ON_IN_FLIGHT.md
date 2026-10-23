# Why sequential firing marks the running transition in `in_flight`

Written 28 September 2026, when the change was made in the petritype library
(commit 58c6405 there, released after 0.1.1). Kept here, in petritype-rs's
`dev-docs`, so that the lesson carries into the Rust engine.

## The problem this solved

A transition body can run for a long time: a network call to a slow API, a
model fit, a scan over years of data. In Petritype the firing of one
transition has three stages: take the input tokens out of their places, call
the transition's function, put the results into the output places. In
sequential mode one call performs all three, and the caller learns which
transition fired only after the call returns, from `graph.last_fired`. While
the function runs, nothing on the graph says which transition is working.

The Petritype server runs each net in a subprocess and offloads synchronous
bodies to a thread pool so the event loop stays responsive. A heartbeat
thread reports "still busy" every thirty seconds while a step runs. The
platform wanted to answer one question from outside: is this net stalled,
and if so in which transition? The heartbeat could say that a step had been
running since a given time. It could not say which transition, so a
diagnosis read "a transition body has been executing for fifteen minutes,
the last one to finish was StartCycle", and the reader had to infer the
next transition in the cycle. Naming it directly, "FetchTraffic has been
executing for fifteen minutes", is the same verdict with one inference
fewer, and that inference is exactly the one a person at three in the
morning gets wrong.

## Why the answer is a field on the graph, not a callback

The graph already had `in_flight: set[str]`. Concurrent mode maintains it:
a transition's name is in the set from the moment its body is launched
until its results are deposited, so an observer can show working
transitions. Sequential mode never touched it, which was an inconsistency
rather than a design choice. The change makes sequential firing set
`in_flight` to the chosen transition's name after stage one and before
stage two, and clear it after the body returns or raises. Observers now see
the same signal in both modes. The change lives in
`ExecutableGraphOperations.fire_one_transition`, the one function where the
transition is chosen, its tokens are taken and its body is called, so every
sequential entry point gets it.

A hook or callback before the body would have served the server, but it
would have added a second mechanism for a fact the graph already had a
place for. The rule applied: when a run-time fact has a home on the graph,
put it there and let every reader read it, rather than adding a channel
per reader.

## Two details that matter in any implementation

**Assign, never mutate.** The server's heartbeat runs on a different thread
from the firing. Replacing the set (`graph.in_flight = {name}`, then
`graph.in_flight = set()`) is an atomic reference swap in CPython, so a
reader sees either the old set or the new one, never a half-updated set.
Mutating one set in place would race. In Rust the same concern appears as
shared mutable state across threads: the field wants an atomic swap of an
immutable value (or a lock), and a reader must never observe a partially
written value.

**Clear on failure.** A body that raises must still leave `in_flight`
empty, so the clearing sits in a `finally`. A stale name after an
exception would make the next diagnosis blame a transition that finished
long ago. The Rust equivalent is a guard object whose drop clears the
field, so early returns and panics behave the same as normal completion.

## What this does not do

It does not interrupt a hung body. A synchronous body on a thread cannot be
cancelled, and its input tokens are already consumed by stage one, so the
only honest action is to report. Killing and recycling is a platform
policy, decided per net and above the library.

It does not record when the body started. The library reports the fact
"this transition is running"; the timing belongs to the caller, who knows
when it asked for the step. Keeping timestamps out of the graph keeps the
graph a description of the net's state rather than a log.

## The runtime's observers, 17 October 2026

One reader had been left out. The Python runtime notifies its observers on
every change of the graph, and in concurrent mode that includes the moment
the in-flight set changes, but its sequential loop notified only after a
firing had completed, so an observer on the event loop never saw the field
non-empty in that mode, even though the field was set for the length of
every body. An in-process consumer that draws a run as a strip of tiles
found that a body running for seconds was never shown as working. On 17
October 2026 the sequential path gained one more notification per firing,
right after the assignment and before the body, through an optional hook on
the one firing primitive, which the runtime's sequential paths pass their
observer call to. The rule above holds: the fact lives on the graph and is
read from there; what changed is that the readers the runtime already
notifies are told when it changes, in both modes alike. No launch record was
added to the engine's log, which keeps one record per firing.

In Rust this needs nothing. The engine has no observer mechanism, and a
caller who wants the working moment uses the staged firing API, `launch`,
then the body, then `distribute`, which is how the Bevy viewer and the
runner fire and which already reports `Launched` before the body. The
sequential `step` stays what it is, the three stages in one call with one
`Fired`. If an in-process Rust consumer ever drives the sequential driver
and wants the moment, that driver can stage with a hook between launch and
run, a small change with no consumer today.
