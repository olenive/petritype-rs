# Every gate to pass before a hand-over is `just check`. Each recipe also runs on its own.

# List the recipes
default:
    @{{just_executable()}} --list

# Every gate before a hand-over, in order: test, lint, docs
check: test lint docs

# The workspace suite with the serde feature, then the core crate alone without it
test:
    cargo test --workspace --features serde
    cargo test -p petritype

# Without serde, each crate that has the feature is checked in a run of its own, so that no
# other crate turns it on. petritype-activity's tests turn on the core's, for the example
# net they read; its docs run builds it with serde nowhere.

# Clippy with warnings denied, with serde and per crate without it; the formatting check
lint:
    cargo clippy --workspace --all-targets --features serde -- -D warnings
    cargo clippy -p petritype --all-targets --no-default-features -- -D warnings
    cargo clippy -p petritype-layout --all-targets --no-default-features -- -D warnings
    cargo clippy -p petritype-activity --all-targets --no-default-features -- -D warnings
    cargo fmt --all --check

# Format the workspace in place
fmt:
    cargo fmt --all

# The API docs with warnings denied, with serde and per crate without it
docs:
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --features serde
    RUSTDOCFLAGS="-D warnings" cargo doc -p petritype --no-deps --no-default-features
    RUSTDOCFLAGS="-D warnings" cargo doc -p petritype-layout --no-deps --no-default-features
    RUSTDOCFLAGS="-D warnings" cargo doc -p petritype-activity --no-deps --no-default-features

# Report on free space, build output and dependencies; deletes nothing
housekeeping *FLAGS:
    @python3 scripts/housekeeping.py check --free-limit-gb 50 --cargo-limit-gb 40 --stale-limit-gb 5 {{FLAGS}}

# List the build output this Rust can never use again; --yes deletes it
housekeeping-clean *FLAGS:
    @python3 scripts/housekeeping.py clean {{FLAGS}}
