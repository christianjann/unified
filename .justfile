default:
  just --list

set shell := ['bash', '-cu']

fmt:
    cargo fmt --all

clippy:
    cargo clippy --all-targets -- -D warnings

fix:
    cargo clippy --all-targets --fix --allow-dirty

build:
    cargo build

release:
    cargo build --release

clean:
    cargo clean

doc:
    cargo doc --open

cargo-test:
    cargo test

test-verbose:
    cargo test -- --nocapture

fmt-check:
    cargo fmt --check

check: clippy fmt-check
    cargo check

test: test-setup cargo-test sync test-clean 

# Setup test environment (creates test git repo)
test-setup:
    ./crates/un-cli/tests/setup-test-env.sh

# Clean test environment (removes test data, cache, repos)
test-clean:
    ./crates/un-cli/tests/clean-test-env.sh

# Clean test environment and setup fresh one
test-reset: test-clean test-setup

# Run sync command
sync:
    cargo run -p un-cli -- sync 

# Build and install the un command system wide
install:
    cargo install --path crates/un-cli
