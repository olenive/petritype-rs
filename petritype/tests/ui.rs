//! Compile-fail tests: the mistakes this crate promises to catch when the net
//! is declared, and the messages it catches them with.
//!
//! The expected output lives in `tests/ui/*.stderr` next to each case, and is
//! tied to one compiler version: regenerate with
//! `TRYBUILD=overwrite cargo test --test ui` after a toolchain bump, and read
//! the diff — a message that stops naming edges, slots or places is a
//! regression even when it still fails to compile.
//!
//! Two things make these files move for reasons other than a bug. The paths
//! in a message are feature-sensitive, so a case whose types differ between
//! feature sets has to be written to survive both. And a mismatched *output*
//! is now reported one level higher than it was: with three output forms
//! competing for the same `IntoOutcome` bound, no impl applies when the
//! return type fits none of them, so the compiler blames `IntoOutcome` rather
//! than the `PositionalOutputs` or `IntoTokens` obligation inside it. That is
//! why `IntoOutcome`'s notes spell out all three forms and name the output
//! places: the message has to carry what the deeper one used to.
//!
//! The impls for a body with an output distribution function after it carry
//! `#[diagnostic::do_not_recommend]`, or every mismatch anywhere near
//! `transition()` would be followed by the twenty-six ways
//! `WithOutputDistribution` implements `TransitionBody`.

#[test]
fn declarations_that_must_not_compile() {
    let t = trybuild::TestCases::new();
    t.pass("tests/ui/pass_*.rs");
    t.compile_fail("tests/ui/fail_*.rs");
}
