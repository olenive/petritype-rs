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

# Clippy with warnings denied, then the formatting check
lint:
    cargo clippy --workspace --all-targets --features serde -- -D warnings
    cargo fmt --all --check

# Format the workspace in place
fmt:
    cargo fmt --all

# The API docs with warnings denied
docs:
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --features serde
