# Changelog

## Unreleased

- `petritype-runner` serves a net the host program owns. A program that builds a net and fires it from its own code (a game, a simulation) gives it to a `HostServer`, which listens on a TCP port; any number of viewers attach and are told what the program does to the net, inner nets included. `Host` is the same without the listener, for a program with a transport of its own. The connection reads the net and nothing else: `load`, `reset`, `shutdown`, `step`, `start`, `stop`, `inject`, `enable`, `disable`, `set_param`, `press` and `set_filter` are `unsupported`.
- A host nobody is listening to does none of the work of telling: while no viewer has made its first request, a `HostServer`'s `change` makes no lines and compares no fireable set, and keeps the log, the progress and the marking version as before. `Host::set_listening` is the same switch for a program with its own transport; it is on unless turned off.

## 0.1.0, 2026-10-23

The first release, early: the API will change.

- `petritype`, the core crate: typed places, transitions whose bodies are plain functions, eight kinds of input edge (take, take all, snapshot, mutate, keyed take and mutate, drain and survey), and outputs by position, by name (`#[derive(Inputs)]` and `#[derive(Outputs)]`), by type (`#[derive(Routed)]`) or as an explicit `Distribution`.
- Firing inline (`step`, `fire`) or in three stages (`launch`, `run`, `distribute`) so that bodies may run on other threads; a body's `Err` or panic fails the firing, with optional restoring of the consumed tokens. The `drivers` module runs a net to quiescence or on a clock, sequentially or with bodies on threads.
- Every change is an event, and every token has a stable id. Commands feed a running net from another thread, `Param::<T>` values are set per run, `net.snapshot()` hands another thread an owned copy, and `assert_acyclic` and `assert_no_source_transitions` check a net's structure when it is built.
- Nets as tokens: a token may carry nets of its own, which the outer net runs and logs. Token keys, queries and filters: a place derives keys from its tokens and the net answers queries from an index. Net-declared controls and token styles.
- With the `serde` feature: commands as JSON, and a net saved and loaded as JSON, its tokens' inner nets rebuilt from templates registered by name.
- `petritype-dot` draws a net or a snapshot as Graphviz DOT; `petritype-layout` lays a net out for any renderer; `petritype-activity` tracks a net's activity and draws its activity thumbnail.
- `petritype-runner` serves a net over the net protocol (version 1) on stdin and stdout, and replays the protocol transcripts shared with the Python runtime.
- Examples that port the Python acceptance set, each also a test, and the conformance nets `transit`, `wildlife`, `shop` and `traffic-light`.
