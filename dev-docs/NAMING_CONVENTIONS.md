# Naming conventions

Shared vocabulary for petritype-rs, and the rename backlog for Python petritype so
the two libraries converge. Decided 2026-09-19.

## Principle

Keep the Python name unless one of these holds:

1. **Rust forces a change** (no reflection, ownership, a tuple return is
   positional by nature).
2. **The Python name came from a library or implementation detail**, not from
   the concept (`Node` from the graph library, `List` from the container).
3. **The Rust name is agreed to be better**, in which case Python is renamed to
   match (aliases kept for a deprecation period).

"Edge", not "arc". Users know graph vocabulary better than Petri net
vocabulary, and `Arc` is a Rust smart pointer. Edge types carry the `Edge`
suffix in code, as in Python, for readability at the declaration site.

"Net", not "graph": it is a Petri net. Python's `ExecutableGraph` goes on the
rename backlog.

## Vocabulary

| concept | Rust | Python today | Python backlog |
|---|---|---|---|
| the whole thing | `Net`, `NetBuilder` | `ExecutableGraph`, `construct_graph` | rename to `Net`, alias kept |
| typed container of tokens | place, `PlaceRef<T>`, `PlaceId` | `ListPlaceNode` | `Place` alias |
| executable node | transition, `TransitionId` | `FunctionTransitionNode` | `Transition` alias |
| connection | edge | edge | — |
| consuming input, one token | `TakeEdge(place)` | `ArgumentEdgeToTransition` | — |
| consuming input, all tokens | `TakeAllEdge(place)` | `ArgumentEdgeToTransition` + `list[T]` annotation | — |
| non-consuming read, cloned | `SnapshotEdge(place)` | `SnapshotEdge` | — |
| non-consuming read, live `&mut` | `MutateEdge(place)` | `MutateEdge` | — |
| output connection | output edge (positional in the builder tuple) | `ReturnedEdgeFromTransition` | — |
| author-written router after the body | output distribution function; body method `body.output_distribution(f)` (extension trait `OutputDistribution`, wrapper `WithOutputDistribution`) | `output_distribution_function` field | rename `output_distribution` (alias kept); `guard` / `priority` already omit `_function` |
| value returned by an output distribution function | `Distribution` (`Distribution::new().to(place, token)`, chainable; `push` for loops; one constructor only) | the `{name: token}` dict | — |
| a `Distribution` pair the net cannot apply | `DistributionError::{NotAnOutput, WrongType}` inside `FiringFailed` | `KeyError` / type check | — |
| one execution of a transition | firing | firing | — |
| number of completed firings | `fired_count` | `step_count` | rename |
| a firing's body raised / returned `Err` | `FiringFailed` | `TransitionFailedError` | rename |
| fuse spent | `TooManyFirings` | `TooManyFiringsError` | — |
| put consumed tokens back on failure | `restore_tokens_on_failure` | `restore_tokens_on_failure` | — |
| static check: no cycles | `assert_acyclic` | `assert_acyclic` | — |
| static check: every transition consumes | `assert_no_source_transitions` | `assert_no_source_transitions` | — |
| lookup by name | `place_named`, `transition_named` | `place_named` | add `transition_named` |
| execution mode | `Mode::Sequential`, `Mode::Concurrent` | `ExecutionMode.SEQUENTIAL`, `.CONCURRENT` | — |
| fire stage 1: consume inputs, take body | `launch` | `stage_1_extract_argument_tokens_from_places` | rename `launch` |
| fire stage 2: call the body | `run` | `stage_2_call_transition_function` | rename `run` |
| fire stage 3: place outputs | `distribute` | `add_tokens_to_places` | rename `distribute` |
| one firing, all stages inline | `step` | `Runner.step` | — |
| run until quiescent or limit | `run` | `Runner.run`, `execute_graph`, `run_to_completion` | consolidate |
| run on a clock until stopped | `run_indefinitely` | `Runner.run_indefinitely` | — |
| stop-without-error limit | `stop_after_n_firings` | `stop_after_n_firings` | — |
| raise-past-this limit | `error_after_n_firings` | `error_after_n_firings` | — |
| summary of a run | `RunSummary { quiesced, fired_count }` | `RunSummary { quiesced, n_fired, fired }` | rename `n_fired` |
| transitions whose bodies are running | `in_flight`, `is_in_flight` | `in_flight` | — |
| stage-1 result, owns tokens + body | `Firing` (`transition`, `is_pending`, `run`) | the tuple `stage_1_…` returns | — |
| stage-2 result, carries the body home | `Completed` (`transition`, `succeeded`) | — | — |
| no `MutateEdge` input, so the body may run elsewhere | `detachable` | — | — |
| a `Completed` from another net / for a transition not in flight | `NetError::ForeignFiring`, `NetError::NotInFlight` | — | — |
| switch a transition off/on | `enable`, `disable`, `set_enabled`, `is_enabled` | `Enable`, `Disable` commands | — |
| append tokens | `extend` (many), `put` (one) | `Extend` | — |
| replace all tokens | `set_tokens` | `SetTokens` | — |
| remove one token by id | `remove` | — (no ids) | — |
| replace one token by id | `replace` | — (no ids) | — |
| change counters | `topology_counter`, `state_counter` | — | add if useful |
| enabling predicate | `guard` | `guard` | — |
| can fire now: switched on, inputs present, guard passes | `fireable`, `is_fireable` | — | — |
| selection hint | `priority` | `priority` | — |
| custom chooser | `set_selector`, type `Selector` | `transition_selector` | — |
| per-run value a transition declares it needs | `Param::<T>`, `b.param(v)`, `net.set_param(v)` | `kwargs`, `SetParam` | — |

## Rust-specific conventions

- Getters are bare nouns (`enabled`, `count`); setters are `set_` (`set_enabled`).
  `enable` / `disable` are sugar over `set_enabled`.
- Error types have no `Error` suffix when they are variants; top-level enums do
  (`BuildError`, `NetError`, `CommandError`). `FiringFailed` and
  `TooManyFirings` are variants.
- Ids: `PlaceId`, `TransitionId`, `TokenId`. Typed handle: `PlaceRef<T>`.
- Events are past tense: `Fired`, `Launched`, `Distributed`, `Failed`, `Added`,
  `Removed`, `Mutated`, `Toggled`.
- Output forms: *positional* (tuple), *routed* (by type), *distribution*
  (explicit `Distribution` value).
- One constructor per type (`new()` plus chainable methods); no second spelling
  for the same thing. Sugar, if ever wanted, is a macro (like `vec!`), not a
  second constructor.
