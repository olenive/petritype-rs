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
| consuming input, the oldest token a filter accepts | `TakeWhereEdge(place, filter)` (the spec's `TakeWhere`, with the crate's `Edge` suffix), `EdgeKind::TakeWhere`, `"take_where"` | `TakeWhereEdge` | — |
| non-consuming read, live `&mut` of the oldest token a filter accepts | `MutateWhereEdge(place, filter)`, `EdgeKind::MutateWhere`, `"mutate_where"` | `MutateWhereEdge` | — |
| consuming input, every token, zero or more: never holds its transition back (a *reset arc*) | `DrainEdge(place)`, `EdgeKind::Drain`, `"drain"` (the `Vec::drain` idiom) | `DrainEdge`, argument annotated `list[T]` | — |
| non-consuming read, clones of every token, zero or more: never holds its transition back | `SurveyEdge(place)`, `EdgeKind::Survey`, `"survey"` | `SurveyEdge` | — |
| whether an input edge needs something in its place before its transition may fire | **presence**; `EdgeKind::requires_presence` (false exactly for drain and survey) | — | — |
| output connection | output edge (positional in the builder tuple) | `ReturnedEdgeFromTransition` | — |
| author-written router after the body | output distribution function; body method `body.output_distribution(f)` (extension trait `OutputDistribution`, wrapper `WithOutputDistribution`) | `output_distribution_function` field | rename `output_distribution` (alias kept); `guard` / `priority` already omit `_function` |
| value returned by an output distribution function | `Distribution` (`Distribution::new().to(place, token)`, chainable; `push` for loops; one constructor only) | the `{name: token}` dict | — |
| a `Distribution` pair the net cannot apply | `DistributionError::{NotAnOutput, WrongType}` inside `FiringFailed` | `KeyError` / type check | — |
| one execution of a transition | firing | firing | — |
| number of completed firings | `fired_count` | `step_count` | rename |
| a firing's body raised / returned `Err` | `FiringFailed` | `TransitionFailedError` | rename |
| fuse spent | `DriverError::TooManyFirings { limit, transition, summary }` | `TooManyFiringsError` (summary attached) | — |
| a run stopped on a failed firing | `DriverError::FiringFailed { summary, error }` | `TransitionFailedError` propagating out of `Runner.run` | — |
| what a driver does about a failed firing | `OnFailure::{Stop, Disable, Continue}` | raise (the only behaviour) | — |
| put consumed tokens back on failure | `restore_tokens_on_failure` | `restore_tokens_on_failure` | — |
| a body panicked | `BodyPanicked` (inside `FiringFailed`) | the exception inside `TransitionFailedError` | — |
| the tokens a failed firing took | `Event::Failed { consumed, restored }` (same word as `Fired`/`Launched`) | `TransitionFailedError(consumed, restored)` | — |
| static check: no cycles | `assert_acyclic` → `BuildError::Cycle { path }` (first cycle, `" -> "`) | `assert_acyclic` (all cycles, `" → "`) | — |
| one edge, as a diagram draws it | `EdgeInfo::{Input { place, transition, kind, filter }, Output { transition, place, slot }}`, `net.edges()` | plotting-layer edge structs (`return_index` = `slot`) | — |
| DOT text of a net | `petritype_dot::{dot, dot_with, Style, RankDir, Highlight::from_event, DotSource}` | `RustworkxToGraphviz.activation_coloured_attr_functions`, `SimpleGraphvizVisualization` | — |
| static check: every transition consumes | `assert_no_source_transitions` → `BuildError::SourceTransition` (first offender) | `assert_no_source_transitions` (all offenders) | — |
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
| summary of a run | `RunSummary { quiesced, fired_count, failed_count }` | `RunSummary { quiesced, n_fired, fired }` | rename `n_fired`; add `failed_count` |
| run limits (per call, count launches) | `Limits { stop_after_n_firings, error_after_n_firings }` | `stop_after_n_firings` kwarg (per call), `RunContext.error_after_n_firings` (per context) | — |
| per-tick hook for input | `each_tick: FnMut(&mut Net)` on `run_indefinitely` | `RunContext.inbox`, `drain_inbox` | — |
| transitions whose bodies are running | `in_flight`, `is_in_flight` | `in_flight` | — |
| stage-1 result, owns tokens + body | `Firing` (`transition`, `is_pending`, `run`) | the tuple `stage_1_…` returns | — |
| stage-2 result, carries the body home | `Completed` (`transition`, `succeeded`) | — | — |
| no `MutateEdge` input, so the body may run elsewhere | `detachable` | — | — |
| a `Completed` from another net / for a transition not in flight | `NetError::ForeignFiring`, `NetError::NotInFlight` | — | — |
| switch a transition off/on | `enable`, `disable`, `set_enabled`, `is_enabled` | `Enable`, `Disable` commands | — |
| append tokens | `extend` (many), `put` (one) | `Extend` | — |
| replace all tokens | `set_tokens` | `SetTokens` | — |
| erased input crossing a thread | `Command::{Extend, SetTokens, Remove, Replace, Enable, Disable}`, typed constructors `Command::{extend, put, set_tokens, remove, replace, enable, disable}` | `Extend`, `SetTokens`, `Enable`, `Disable` dataclasses (by name) | — |
| carry out one command | `net.apply(cmd)` → `CommandError` (atomic) | `_apply_command` (checks all tokens first, returns bool) | — |
| JSON form of a command | `JsonCommand` (`{"command": "extend", "place": "Inbox", "tokens": [..]}`), `Command::from_json`, `Command::to_json`, `net.apply_json` | — | — |
| remove one token by id | `remove` | — (no ids) | — |
| replace one token by id | `replace` | — (no ids) | — |
| change counters | `topology_counter`, `state_counter` | — | add if useful |
| enabling predicate | `guard` | `guard` | — |
| can fire now: switched on, inputs present, guard passes | `fireable`, `is_fireable` | — | — |
| selection hint | `priority` | `priority` | — |
| custom chooser | `set_selector`, type `Selector`; `clear_selector` restores the default | `transition_selector` | — |
| per-run value a transition declares it needs | `Param::<T>::new()` in the input tuple, `b.param(v)`, `net.set_param(v)`, `net.param::<T>()` | `kwargs`, `SetParam` | — |
| parameter on the wire | `b.serializable_param::<T>("name")`, `JsonCommand::SetParam { param, value }`, `Command::set_param(v)` | `SetParam(transition, kwargs)` (by transition, merged) | — |
| declared parameter without a value | `BuildError::MissingParam { transition, type_name }` | missing kwarg at call time | — |
| owned copy of a net's state | `Snapshot`, `net.snapshot()` → `NetError::NoClonePlace { place }` | `dict(graph.fired_counts)` copies only | — |
| a net carried by a token / the net holding that token | **inner net** / **outer net** (DESIGN §6.6) | — | same words (nets-as-tokens Python track) |
| a token type that owns nets | trait `Nested` (`nets`, `nets_mut`: each net with a name, `""` for a bare `Net`) | — | `__petritype_nets__` (proposal) |
| place whose tokens carry nets | **nested place**, `b.place_nested::<T>(name)`, `PlaceInfo::nested` | — | `nested=True` on the place (proposal) |
| nested place whose inner nets only an outer body steps | **passive**, `b.passive(place)`, `PlaceInfo::passive` → `BuildError::NotNested` on a plain place | — | — |
| one firing here, or else in the first inner net that has one | **deep step**, `net.step_deep()` → `DeepStep::{Here, Inside}`; `net.is_quiescent_deep()` | — | — |
| moving an inner net's events into the outer log | **lift** (the net does it, at six fixed moments) | — | — |
| an inner net's event in the outer log | `Event::Nested { place, token, net, nested_id, event }` | — | wrapping `Nested` record (proposal) |
| the id that follows a token's inner net across moves | `nested_id`: the inner net's own `NetId` (`Display` and `Serialize` as a bare number) | — | — |
| where an inner failure happened | `FiringFailed::path`: `(place, token, net name)` per level, outer to inner | — | — |
| mutable reach into a token's nets from outside a body | `net.nested_scope(place, token, f)` (lifts when `f` returns); read-only `net.nested(place, token)` | — | — |
| every place's tokens as JSON, by place name, oldest first | `Marking` (`serde`; transparent over a name-ordered map) | `dict[str, list]` | — |
| write / replace a whole marking | `net.export_marking()` → `NetError::NotSerializable`; `net.import_marking(&marking)` (wholesale, fresh token ids, atomic) | `graph.export_marking()`, `graph.import_marking(...)` (`NetJson`) | — |
| the marking was replaced and cannot be told as events | `Event::Resynced` (no fields, one per import) | `Resynced(place)` (one per place) | — |
| a function that builds a net, registered by name | **template**; registry `Templates` (`register`, `names`, `build` → `TemplateError`), `b.template(name, f)`, `b.templates(&registry)`, `net.templates()` | `Templates` (`register`, `template` decorator, `build`) | — |
| what a factory is given | `Params` (`serde_json::Map<String, Value>`, the runner's type) | keyword arguments | — |
| the mark a net built from a template carries | the **stamp** (template name, its `Params`, the factory); `net.template()` | `template=` on `construct_graph` | — |
| the parameters a saved net's factory is given | its saved `params`, by wire name; there is no separate `template_params` (PROTOCOL §12.5) | same | — |
| a registry in effect while nets are deserialised | `Templates::scope(f)` (thread-local stack, innermost first), `templates.restore(value)` | — (a process-wide registry) | — |
| a net's saved JSON form | `{"template", "marking", "params", "enabled"}`: `Serialize` / `Deserialize` for `Net`, `net.to_json()` | `NetJson.save`, `NetJson.restore` | — |
| a copy of a net, built again from its template | `net.fork()` → `NetError::{NoTemplate, InFlight, RebuildFailed}`; per token type `Nested::fork` (default `NotForkable`); in a snapshot `NetError::CannotFork`, read with `snapshot.nested(place, token)` | `fork(token)` (a deep copy) | — |
| an inner net's address on the wire | **path**: `<net>/<place>/#<token>/<name>` per level, a bare net's name empty so its level ends in `/` (PROTOCOL.md §12.1) | same | — |
| an inner net's stable name on the wire | **nested id**: its `NetId` as `Display` prints it, opaque to a reader | `net_id`, as a string | — |
| an inner net's event on the wire | the `nested {path, nested_id, event}` notification, `protocol::Notification::Nested` | `NestedEvent` | — |
| the nets a token holds, on a token entry | `nets: [{name, nested_id, path, template?, count}]`, `protocol::InnerNet` | `Tokens.nets` | — |
| one inner firing, without asking this net's own selector | `net.step_inside()` (the second half of `step_deep`) | `_select_inner` in the runner | — |
| the wire name behind a `ParamChanged` | `net.param_name(type_name)` | — (Python's events carry the name) | — |
| whether a body returns its outputs by position (and so what a `slot` is) | `TransitionInfo::positional`, `TopologyTransition::positional` | a return edge's `return_index` being set | — |
| a named attribute a place derives from each of its tokens | **key** (DESIGN §8.3); the place's names `PlaceInfo::key_names`, `TopologyPlace::key_names`; `key_names` on the wire | key, `place.key_names` | — |
| the value of one key: a string, an integer or a boolean, compared by kind | `KeyValue::{Str, Int, Bool}` (`1`, `"1"` and `true` differ) | `KeyValue` (a `Union`), `same_value` | — |
| one token's keys, in declaration order | `Keys` (`get`, `iter`, `names`), `net.keys(place, token)` by value | `Keys.of(place, token)` (a `dict`) | — |
| declaring a place's keys | `b.keys(place, names, extractor)`: names static, the extractor returning as many values → `BuildError::{DuplicateKeyName, KeysAlreadyDeclared}` | `ListPlaceNode(..., keys={name: fn})` or `keys=(attr, ..)` | — |
| which tokens to keep | `Filter { clauses, places, prefix }`: `Filter::new().key_in(..).key_is_param(..).in_places(..).with_prefix(..)`, `filter.matches(&keys)`, `net.matches(&filter, place, token)` | `Filter(clauses, places, prefix)`, `Filter.where(class_=..)`, `in_places`, `with_prefix` | — |
| one condition on one key | `Clause::In { key, values }` (OR within), `Clause::IsParam { key, param }` (a net parameter's current value, by wire name) | `Clause(key, values)`, `Clause(key, param=name)` | — |
| every token a filter keeps, in place then token order | `net.query(&filter)` → `Vec<Match { place, token, keys, label, json }>` | `Query.matches(graph, filter)` → `Match` | — |
| the same tokens as ids alone, nothing rendered | `net.query_ids(&filter)` → `Vec<(PlaceId, TokenId)>` (what the runner's projection counts and pages) | — | — |
| a key's values in order of first appearance | `net.distinct_values(key, places)` | `Query.distinct(graph, key)` | — |
| per key, per value, the tokens holding it, kept by the net | the **index** (crate-private; never saved, rebuilt on import) | — (Python scans) | — |
| an input edge that chooses its token with a filter | **keyed edge**; `EdgeKind::is_keyed`, its filter in `TransitionInfo::filters` and on `EdgeInfo::Input` | keyed edge | — |
| the token a keyed edge would choose now | `net.first_match(place, &filter)` (the first in queue order whose keys pass; places and prefix not consulted) | — | — |
| a keyed edge on a place that declares no keys | `BuildError::KeyedEdgeOnUnkeyedPlace { transition, place }` | — | — |
| the tokens a filter keeps, asked over the wire | the `query {filter?, token_limit?}` method → `{marking_version, count, matches: [{place, token, json?}]}`, `protocol::QueryResult` (PROTOCOL.md §13.3) | `query`, `wire.QueryResult` | — |
| a key's values, asked over the wire | the `distinct {key, places?}` method → `{marking_version, key, values}`, `protocol::DistinctResult` | `distinct` | — |
| a connection's filter, through which every read and notification of one net is projected | `set_filter {filter?}` (without `filter`: cleared); the runner's `Projection`, one per net (PROTOCOL.md §13.4) | `set_filter`, `_Net.filter` | — |
| the line a connection gets in place of an `added`, `removed` or `mutated` about a token its filter hides | the `hidden {kind, place}` notification, `protocol::Notification::Hidden`, `HiddenKind` | `wire.Hidden` | — |
| how many of a place's tokens the connection's filter lets through | `matching` beside `count`, written only while a filter is set; `PlaceMarking::matching`, `PlaceTokens::matching` | `matching` | — |
| a token's keys on the wire / a `produced` entry with them | `protocol::Keys` (an object in declaration order) / `protocol::Produced` (`[place, id, label]`, or five elements with `null` for the colour) | `keys`, `Tokens.produced` | — |

## Rust-specific conventions

- Getters are bare nouns (`enabled`, `count`); setters are `set_` (`set_enabled`).
  `enable` / `disable` are sugar over `set_enabled`.
- Error types have no `Error` suffix when they are variants; top-level enums do
  (`BuildError`, `NetError`, `CommandError`). `FiringFailed` and
  `TooManyFirings` are variants.
- Ids: `PlaceId`, `TransitionId`, `TokenId`. Typed handle: `PlaceRef<T>`.
- A runtime error about an existing place or transition carries its id *and*
  a `name: String`; the name is for the message, the id is what a program
  matches on (`DistributionError` names both its transition and its place).
  Not-found errors carry the id only; `BuildError` names only; events ids
  only.
- Events are past tense: `Fired`, `Launched`, `Distributed`, `Failed`, `Added`,
  `Removed`, `Mutated`, `Toggled`.
- Output forms: *positional* (tuple), *routed* (by type), *distribution*
  (explicit `Distribution` value).
- One constructor per type (`new()` plus chainable methods); no second spelling
  for the same thing. Sugar, if ever wanted, is a macro (like `vec!`), not a
  second constructor.
