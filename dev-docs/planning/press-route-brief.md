# Brief: `press` on petritype-server and in the Bevy HTTP transport

For one Opus agent. Written 2026-10-19 from the spec at
`/private/tmp/claude-501/-Users-admin-work-petritype-rs/1c5160ee-1703-4436-bf19-73b935fde528/scratchpad/press-route-spec.md`,
sections 0, 1, 2 and 4, with the decisions of 18 October applied. Read the
spec first; this brief says what has changed since it was drafted and what
the rules are. Where the spec and the code disagree, say so in your report
rather than guessing.

## What this round is

A net declares controls; a viewer presses one; the runtime answers
`pressed` or `nothing`. The wire already has `press` (PROTOCOL.md §6.6) and
the Python runtime answers it since petritype 0.3.0, on PyPI as 0.3.1 and
locked in the server. What is missing is the HTTP path: the server has no
`press` route on the control plane or the worker, and the Bevy viewer's HTTP
transport has no route for it. You add both.

## Since the spec was drafted

- The production dependency is settled: petritype 0.3.1 is on PyPI and in
  the server's lock. Spec §1.4's "unreleased" is out of date; the README
  line you write can say a worker needs petritype 0.3.0 or later for
  `press`.
- Branch bases are `main` in both repositories; the earlier branches are
  merged.
- No commit in any repository carries attribution trailers of any kind: no
  co-author line, no session line, nothing. Short plain subjects. This
  replaces spec §4's line about trailers on petritype-bevy.
- Gates are `just check` in each repository (`just` lists the recipes). The
  server's is its unit suite; the viewer's is its suite under both runners
  plus clippy and `cargo fmt --check`.

## The work

1. petritype-server, branch `press-route` from `main`: spec §1.1 (the
   route on the control plane and the worker, the error mapping, the
   timeout row, the scope row), §1.2 (the tests, including the pass-through
   of `topology.controls` and a place `style` on both paths, and the real
   runtime e2e module extended for the traffic light's controls), §1.3 (the
   docs). One commit, or two if the docs read better apart.
   - A worker whose runtime is older than 0.3.0 answers `press` with the
     runtime's own protocol error. Make sure that comes back as a clear
     400 with the reason, and say in your report whether
     `MINIMUM_PETRITYPE` in the subprocess manager should rise to 0.3.0.
     Do not raise it yourself; that is Oleg's call.
   - The e2e module's messages mention a 0.2.0 checkout; correct them if
     they are wrong against what the module needs now.
2. petritype-bevy, branch `press-route` from `main`: spec §2. `route()` in
   `src/remote/http.rs` and its `OUTER_ONLY` list are where the spec says.
   One commit.

Local checkouts for the e2e module: `PETRITYPE_PYTHON_SRC` is
`/Users/admin/work/petritype`; `PETRITYPE_RUNNER_BIN` is whatever the
module's own default names under `/Users/admin/work/petritype-rs/target`,
built with `cargo build --example` of the right example if absent. Read the
module before setting either.

## Rules

- Never run anything under the server's `scripts/`, never deploy, never
  create, touch or talk to a worker on Fly or Sprites, never call their
  APIs. `uv run pytest -q`, `just check`, and the e2e module with local
  checkouts are the whole of what you run on the server.
- The server tree holds Oleg's own uncommitted work: edits to
  `AUTHENTICATION.md` and `PROD_DEPLOYMENT.md` and an untracked
  `reachability-cache.tgz`. Never stage, stash, revert, commit or read
  them. Stage your files by name; never `git add -A` or `git add .`.
- The server tree and its environment hold tokens. Never read, print or
  copy any token or secret, and never read `~/.config/petrify`.
- Branch in the checkout, no worktrees. Never merge, push, tag or publish.
- Plain English in docs and messages: no em-dashes, say what is true.
- A local end-to-end look, the Bevy HTTP viewer and the nets page against
  a control plane on this machine, is the planning thread's job with Oleg,
  after your hand-over. Do not attempt it. If you learn, while reading,
  whether a worker can run on this machine against a local control plane
  without Fly or Sprites, say what you found in your report, without
  running it.

## Hand-over

Report: the commits on each branch with their subjects; `just check` output
tails for both repositories; the e2e module's result with the two runtimes;
every place where the spec and the code disagreed and what you did; the
`MINIMUM_PETRITYPE` question; anything about a local worker. Under 80 lines.
