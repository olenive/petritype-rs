//! Compile-fail tests: the mistakes this crate promises to catch when the net
//! is declared, and the messages it catches them with.
//!
//! The expected output lives in `tests/ui/*.stderr` next to each case, and is
//! tied to one compiler version: regenerate with
//! `TRYBUILD=overwrite cargo test --test ui` after a toolchain bump, and read
//! the diff — a message that stops naming edges, slots or places is a
//! regression even when it still fails to compile.

#[test]
fn declarations_that_must_not_compile() {
    let t = trybuild::TestCases::new();
    t.pass("tests/ui/pass_*.rs");
    t.compile_fail("tests/ui/fail_*.rs");
}
