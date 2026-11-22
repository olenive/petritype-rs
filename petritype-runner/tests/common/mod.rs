//! What the tests that start the example binary share: building it.

use std::path::PathBuf;
use std::process::Command;

/// Builds the example `parcel_runner` and returns the path of its binary.
pub fn example_binary() -> PathBuf {
    let status = Command::new(env!("CARGO"))
        .args([
            "build",
            "--example",
            "parcel_runner",
            "-p",
            "petritype-runner",
            "-q",
        ])
        .status()
        .expect("cargo runs");
    assert!(status.success(), "the example builds");
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(
        || {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .expect("the crate is in a workspace")
                .join("target")
        },
        PathBuf::from,
    );
    let path = target
        .join("debug")
        .join("examples")
        .join(format!("parcel_runner{}", std::env::consts::EXE_SUFFIX));
    assert!(path.exists(), "no binary at {path:?}");
    path
}
