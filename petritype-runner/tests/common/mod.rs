//! What the tests that start an example binary share: building it.
//!
//! The examples are built once per test binary, by the first test that
//! asks for one, and every test then runs the binaries that one build made
//! (since 2026-11-28). Until then each test ran a build of its own, and one
//! test's build could replace a binary while another test was running it,
//! which killed that test with a signal now and then.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// Every example a test starts, all made by the one build.
const EXAMPLES: [&str; 2] = ["parcel_runner", "slow_bodies"];

/// The path of the binary of the example `parcel_runner`, built if this
/// test binary has not built it yet.
pub fn example_binary() -> PathBuf {
    example_named("parcel_runner")
}

/// The path of the binary of the example `name` of this crate, built if
/// this test binary has not built it yet.
pub fn example_named(name: &str) -> PathBuf {
    assert!(
        EXAMPLES.contains(&name),
        "the example {name} is not one the tests build: add it to EXAMPLES"
    );
    let path = built().join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    assert!(path.exists(), "no binary at {path:?}");
    path
}

/// The folder of the example binaries, after the one build. A test that
/// asks while the build runs waits for it, and a build that fails is tried
/// again by the next test that asks.
fn built() -> &'static Path {
    static FOLDER: OnceLock<PathBuf> = OnceLock::new();
    FOLDER.get_or_init(|| {
        let mut build = Command::new(env!("CARGO"));
        build.args(["build", "-p", "petritype-runner", "-q"]);
        for name in EXAMPLES {
            build.args(["--example", name]);
        }
        let status = build.status().expect("cargo runs");
        assert!(status.success(), "the examples {EXAMPLES:?} build");
        let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(
            || {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .expect("the crate is in a workspace")
                    .join("target")
            },
            PathBuf::from,
        );
        target.join("debug").join("examples")
    })
}
