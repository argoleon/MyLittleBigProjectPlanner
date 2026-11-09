# Every CI step is a recipe here; GitHub Actions only calls them.
# Run `just` to list the recipes. L01-US07 completes this file (web, licenses, e2e).

set shell := ["bash", "-euo", "pipefail", "-c"]

backend := "backend"
compose_dev := "docker compose -f deploy/compose.dev.yaml"

# Development database (deploy/compose.dev.yaml). Override in CI with real secrets.
dev_owner_url := env("DATABASE_MIGRATION_URL", "postgres://mlbpp_owner:mlbpp_owner_dev@127.0.0.1:54329/mlbpp") # gitleaks:allow

# Builds and lints use the query metadata committed in backend/.sqlx, so they
# need no database. `just sqlx-prepare` refreshes it after changing a query.
export SQLX_OFFLINE := "true"

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

# Run unit and integration tests (integration tests need `just db-up`)
test: test-unit test-integration

# Run unit tests only (no database)
test-unit:
    cd {{backend}} && cargo test --workspace --locked --lib --bins

# Run integration tests (each test gets its own database on the `just db-up` server)
test-integration:
    cd {{backend}} && cargo test --workspace --locked --test '*' -- --test-threads 8

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

# Start the development PostgreSQL (port 54329)
db-up:
    {{compose_dev}} up -d --wait postgres

# Stop the development PostgreSQL (data is kept)
db-down:
    {{compose_dev}} down

# Delete the development database volume
db-reset:
    {{compose_dev}} down --volumes

# Apply pending migrations to the development database
migrate:
    cd {{backend}} && DATABASE_MIGRATION_URL="{{dev_owner_url}}" cargo run --quiet --locked -p api -- migrate

# Refresh backend/.sqlx after adding or changing a query (needs `just db-up` and `just migrate`)
sqlx-prepare:
    cd {{backend}} && SQLX_OFFLINE=false DATABASE_URL="{{dev_owner_url}}" cargo sqlx prepare --workspace -- --all-targets

# Fail if backend/.sqlx is out of date with the queries
sqlx-check:
    cd {{backend}} && SQLX_OFFLINE=false DATABASE_URL="{{dev_owner_url}}" cargo sqlx prepare --workspace --check -- --all-targets

# Everything the CI runs for the backend (needs `just db-up` and `just migrate`)
check: fmt-check lint check-domain sqlx-check test
