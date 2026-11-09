# Every command runs in a container (deploy/compose.dev.yaml): the host only
# needs Docker, git and just. Every CI step is a recipe here; GitHub Actions
# only calls them. Run `just` to list the recipes.
# L01-US07 completes this file (licenses, e2e, CI).

set shell := ["bash", "-euo", "pipefail", "-c"]

compose := "docker compose -f deploy/compose.dev.yaml"

# Containers run with the host user's ids, so generated files belong to you.
export DEV_UID := `id -u`
export DEV_GID := `id -g`

# One-off command in the Rust toolchain container (starts PostgreSQL if needed).
be := compose + " run --rm --user $DEV_UID:$DEV_GID backend"

# One-off command in the Node toolchain container, from web/.
fe := compose + " run --rm --no-deps --user $DEV_UID:$DEV_GID web"

# One-off command in the Playwright container, from web/.
pw := compose + " --profile e2e run --rm --no-deps --user $DEV_UID:$DEV_GID e2e"

# List the recipes
default:
    @just --list

# --- Development environment ------------------------------------------------

# Build the development images
images:
    {{compose}} --profile tools --profile web --profile e2e build

# Start PostgreSQL, the API (http://127.0.0.1:18080) and the internal
# application (http://127.0.0.1:4200), both with live reload
dev: web-deps
    {{compose}} --profile web up --build api web

# Stop every development container (data is kept)
down:
    {{compose}} --profile tools --profile web --profile e2e down

# Start only PostgreSQL (host port 54329, for database tools)
db-up:
    {{compose}} up -d --wait postgres

# Create the development tenant "default" (until tenant administration exists, L02)
seed: migrate
    {{compose}} exec -T postgres psql -v ON_ERROR_STOP=1 -U mlbpp_owner -d mlbpp -c "INSERT INTO tenant (id, slug, name) VALUES (uuidv7(), 'default', 'Development') ON CONFLICT (slug) DO NOTHING"

# Delete the development database and build caches
reset:
    {{compose}} --profile tools --profile web --profile e2e down --volumes

# Open a shell in the Rust toolchain container
shell:
    {{be}} bash

# Run any cargo command in the Rust toolchain container, e.g. `just cargo add -p api serde`
cargo *args:
    {{be}} cargo {{args}}

# Run any npm command in the Node toolchain container, e.g. `just npm ls`
npm *args:
    {{fe}} npm {{args}}

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

# --- API contract and generated client ---------------------------------------

# Write the OpenAPI contract generated from the code to backend/openapi.json
openapi:
    {{be}} bash -c 'cargo run --locked --quiet -p api -- openapi > /tmp/openapi.json && mv /tmp/openapi.json openapi.json'

# Regenerate the contract, then the Angular client in web/projects/ui/src/lib/api/generated
generate-client: openapi web-deps
    {{fe}} npx ng-openapi-gen --config ng-openapi-gen.json

# Fail, showing the diff, if the contract or the client on disk (in CI: the
# committed ones) is out of date with the code
client-check:
    #!/usr/bin/env bash
    set -euo pipefail
    before=$(mktemp -d)
    trap 'rm -rf "$before"' EXIT
    mkdir -p "$before/generated"
    cp backend/openapi.json "$before/openapi.json" 2>/dev/null || true
    cp -r web/projects/ui/src/lib/api/generated/. "$before/generated/" 2>/dev/null || true
    just generate-client
    if ! diff -ruN "$before/openapi.json" backend/openapi.json \
        || ! diff -ruN "$before/generated" web/projects/ui/src/lib/api/generated; then
        echo "The OpenAPI contract or the generated client was out of date: it is now regenerated, review and commit it." >&2
        exit 1
    fi

# --- Front end ----------------------------------------------------------------

# Install web/node_modules from the lock file when it changed
web-deps:
    {{fe}} bash -c '[ node_modules/.package-lock.json -nt package-lock.json ] || npm ci --no-audit --no-fund'

# Lint TypeScript and templates (ESLint, accessibility rules included) and styles
# (stylelint: no hard-coded color, font, radius or shadow)
web-lint: web-deps
    {{fe}} bash -c 'npx eslint . && npx stylelint "projects/**/*.scss" && node scripts/check-style-rules.mjs'

# Fail if a translation key is missing in fr.json or en.json
web-i18n: web-deps
    {{fe}} node scripts/check-i18n.mjs

# Run the front-end unit and integration tests (Vitest)
web-test: web-deps
    {{fe}} bash -c 'for project in ui app portal; do npx ng test "$project" --watch=false || exit 1; done'

# Build the library and both applications (production)
web-build: web-deps
    {{fe}} bash -c 'npx ng build ui && npx ng build app --stats-json && npx ng build portal --stats-json'

# Fail if an npm dependency has a license that is not allowed (CLAUDE.md)
web-licenses: web-build
    {{fe}} node scripts/check-licenses.mjs

# Run the end-to-end tests (Playwright, axe-core) against the production build
e2e *args: web-deps
    {{pw}} npx playwright test {{args}}

# Everything the CI runs for the front end
web-check: web-lint web-i18n client-check web-licenses web-test e2e

# --- Production stack (deploy/compose.yaml) ------------------------------------
# Run locally with compose.local.yaml: images built from the sources,
# https://localhost:18443 with a self-signed certificate. See deploy/README.md.

stack := "MLBPP_DOMAIN=localhost MLBPP_REGISTRY=mlbpp-local MLBPP_VERSION=local MLBPP_PUBLIC_PORT=:18443 docker compose -f deploy/compose.yaml -f deploy/compose.local.yaml"

# Lint the deployment scripts (shellcheck) and validate the compose files
deploy-lint: compose-secrets
    docker run --rm -v "$PWD:/mnt:ro" -w /mnt koalaman/shellcheck:v0.11.0 -x -P deploy/tests deploy/tests/*.sh deploy/scripts/*.sh deploy/postgres/*.sh
    {{stack}} --profile sso --profile observability config --quiet

# Create the missing secret files of the stack in deploy/secrets/
compose-secrets:
    deploy/scripts/generate-secrets.sh

# Build the production images (api, worker, mcp, web, postgres)
images-build:
    {{stack}} build web api worker mcp postgres

# Test the images: health alone, non-root, no secret, SBOM (deploy/build/sbom/)
images-test: images-build
    deploy/tests/images.sh

# Start the production stack locally (https://localhost:18443)
compose-up: compose-secrets
    {{stack}} up -d --build --wait --wait-timeout 900

# Stop the local production stack, profiles included (data is kept)
compose-down:
    {{stack}} --profile sso --profile observability down

# Delete the local production stack and its data
compose-reset:
    {{stack}} --profile sso --profile observability down --volumes

# End-to-end tests against the local production stack (nginx, its
# Content-Security-Policy, Traefik), from the e2e container on its network
compose-e2e *args: web-deps
    {{compose}} --profile e2e build e2e
    docker run --rm --network mlbpp_edge --ipc host --user $DEV_UID:$DEV_GID \
        -v "$PWD:/workspace" -w /workspace/web \
        -e E2E_BASE_URL=https://localhost -e "E2E_HOST_RULES=MAP localhost traefik" \
        mlbpp-dev-e2e npx playwright test {{args}}

# Deployment tests: smoke, backups, scaling, optional profiles, end to end
compose-test: compose-up
    deploy/tests/smoke.sh
    deploy/tests/backup.sh
    deploy/tests/scale.sh
    deploy/tests/profiles.sh
    just compose-e2e
