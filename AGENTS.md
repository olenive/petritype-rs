# Working here
- Design first: a short spec agreed before code. Examples show a net running its own process.
- Commit messages: short plain subjects, no attribution trailers.

# Code
- The core crate knows nothing of viewers or runtimes; serde-only things sit behind the `serde` feature.
- No global mutable state; pass configuration as values.
- Gates before hand-over: `just check` (the workspace suite with serde, the core crate alone, clippy with no warnings, `cargo fmt --check`, `cargo doc` with `-D warnings`); `just` lists the recipes. CI runs the same `just check` on every push and pull request, and a weekly RustSec advisory audit; Dependabot proposes crate and action updates.
- Conformance transcripts are shared with `../petritype`: change both runtimes together, re-record there, copy verbatim here.

# Writing
- Plain English: technical terms yes, jargon no. Commas or a second sentence, never em-dashes. Say what is true rather than what is not. Name the thing, never an issue number.
