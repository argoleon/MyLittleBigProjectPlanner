# Every CI step is a recipe here; GitHub Actions only calls them.
# Run `just` to list the recipes. L01-US07 completes this file (web, licenses, e2e).

set shell := ["bash", "-euo", "pipefail", "-c"]

backend := "backend"

# List the recipes
default:
    @just --list

# Build every crate; warnings are errors
build:
    cd {{backend}} && RUSTFLAGS="-D warnings" cargo build --workspace --all-targets --locked

# Check formatting
fmt-check:
    cd {{backend}} && cargo fmt --all -- --check

# Format the code
fmt:
    cd {{backend}} && cargo fmt --all

# Lint with clippy; warnings are errors
lint:
    cd {{backend}} && cargo clippy --workspace --all-targets --locked -- -D warnings

# Run unit and integration tests
test:
    cd {{backend}} && cargo test --workspace --locked

# Fail if the domain crate depends on any crate (it must stay free of I/O)
check-domain:
    #!/usr/bin/env bash
    set -euo pipefail
    cd {{backend}}
    deps=$(cargo tree -p domain --edges normal,build --prefix none --depth 1 --locked | tail -n +2 | sort -u)
    if [ -n "$deps" ]; then
        echo "crates/domain must not depend on other crates; found:"
        echo "$deps"
        exit 1
    fi
    echo "crates/domain has no dependency"

# Everything the CI runs for the backend
check: fmt-check lint check-domain test
