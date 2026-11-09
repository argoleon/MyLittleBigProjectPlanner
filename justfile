# Every command runs in a container (deploy/compose.dev.yaml): the host only
# needs Docker, git and just. Every CI step is a recipe here; GitHub Actions
# only calls them. Run `just` to list the recipes.
# L01-US07 completes this file (web, licenses, e2e).

set shell := ["bash", "-euo", "pipefail", "-c"]

compose := "docker compose -f deploy/compose.dev.yaml"

# Containers run with the host user's ids, so generated files belong to you.
export DEV_UID := `id -u`
export DEV_GID := `id -g`

# One-off command in the Rust toolchain container (starts PostgreSQL if needed).
be := compose + " run --rm --user $DEV_UID:$DEV_GID backend"

# List the recipes
default:
    @just --list

# --- Development environment ------------------------------------------------

# Build the development images
images:
    {{compose}} --profile tools build

# Start PostgreSQL and the API with live reload (http://127.0.0.1:18080)
dev:
    {{compose}} up --build api

# Stop every development container (data is kept)
down:
    {{compose}} --profile tools --profile web down

# Start only PostgreSQL (host port 54329, for database tools)
db-up:
    {{compose}} up -d --wait postgres

# Create the development tenant "default" (until tenant administration exists, L02)
seed: migrate
    {{compose}} exec -T postgres psql -v ON_ERROR_STOP=1 -U mlbpp_owner -d mlbpp -c "INSERT INTO tenant (id, slug, name) VALUES (uuidv7(), 'default', 'Development') ON CONFLICT (slug) DO NOTHING"

# Delete the development database and build caches
reset:
    {{compose}} --profile tools --profile web down --volumes

# Open a shell in the Rust toolchain container
shell:
    {{be}} bash

# Run any cargo command in the Rust toolchain container, e.g. `just cargo add -p api serde`
cargo *args:
    {{be}} cargo {{args}}

# --- Backend quality gates --------------------------------------------------

# Build every crate; warnings are errors
build:
    {{be}} env RUSTFLAGS="-D warnings" cargo build --workspace --all-targets --locked

# Check formatting
fmt-check:
    {{be}} cargo fmt --all -- --check

# Format the code
fmt:
    {{be}} cargo fmt --all

# Lint with clippy; warnings are errors
lint:
    {{be}} cargo clippy --workspace --all-targets --locked -- -D warnings

# Run unit and integration tests
test: test-unit test-integration

# Run unit tests only (no database)
test-unit:
    {{be}} cargo test --workspace --locked --lib --bins

# Run integration tests (each test gets its own PostgreSQL database)
test-integration:
    {{be}} cargo test --workspace --locked --test '*' -- --test-threads 8

# Fail if the domain crate depends on any crate (it must stay free of I/O)
check-domain:
    {{be}} scripts/check-domain.sh

# Apply pending migrations to the development database. Uses sqlx-cli rather
# than `api migrate`, which needs the new queries' metadata to compile.
migrate:
    {{be}} bash -c 'sqlx migrate run --source migrations --database-url "$DEV_DATABASE_OWNER_URL"'

# Refresh backend/.sqlx after adding or changing a query (builds use it offline)
sqlx-prepare: migrate
    {{be}} bash -c 'SQLX_OFFLINE=false DATABASE_URL="$DEV_DATABASE_OWNER_URL" cargo sqlx prepare --workspace -- --all-targets'

# Fail if backend/.sqlx is out of date with the queries
sqlx-check: migrate
    {{be}} bash -c 'SQLX_OFFLINE=false DATABASE_URL="$DEV_DATABASE_OWNER_URL" cargo sqlx prepare --workspace --check -- --all-targets'

# Everything the CI runs for the backend
check: fmt-check lint check-domain sqlx-check test
