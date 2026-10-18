# Working here
- Design first: a short spec agreed before code. Examples show a net running its own process.
- Hand large coding chunks and routine runs (tests, gates) to an Opus subagent. A subagent that writes memory claims its own judge and passes its token.
- Branch in this checkout, no worktrees. Never merge, push, tag or publish without asking.
- Commit messages: short plain subjects, no attribution trailers.

# Code
- The core crate knows nothing of viewers or runtimes; serde-only things sit behind the `serde` feature.
- No global mutable state; pass configuration as values.
- Gates before hand-over: `just check` (the workspace suite with serde, the core crate alone, clippy with no warnings, `cargo fmt --check`, `cargo doc` with `-D warnings`); `just` lists the recipes.
- Conformance transcripts are shared with `../petritype`: change both runtimes together, re-record there, copy verbatim here.

# Writing
- Plain English: technical terms yes, jargon no. Commas or a second sentence, never em-dashes. Say what is true rather than what is not. Name the thing, never an issue number.

# Memory
- Epimemer graph `petritype-rs-dev`. Stores stay high level, never verbatim code or docs. Read the server's guide before nontrivial memory work.
