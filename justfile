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

# Start PostgreSQL, Valkey, Mailpit (:18025), the API (:18080), the worker and the app (:4200)
dev: web-deps
    {{compose}} --profile web up --build api worker web

# Stop every development container (data is kept)
down:
    {{compose}} --profile tools --profile web --profile e2e down

# Start only PostgreSQL (host port 54329, for database tools)
db-up:
    {{compose}} up -d --wait postgres

# Bootstrap the development tenant "default" and its platform super admin
# dev@example.test, password "development only" (sign in on http://127.0.0.1:4200)
seed: migrate
    {{be}} bash -c 'DATABASE_MIGRATION_URL="$DEV_DATABASE_OWNER_URL" BOOTSTRAP_TENANT_NAME=Development BOOTSTRAP_ADMIN_EMAIL=dev@example.test BOOTSTRAP_ADMIN_NAME="Dev Admin" BOOTSTRAP_ADMIN_PASSWORD="development only" cargo run --locked --quiet -p api -- migrate'

# Delete the development database and build caches
reset:
    {{compose}} --profile tools --profile web --profile e2e down --volumes

# The compiled Rust crates of the development containers reach tens of GB,
# and image builds leave an unused BuildKit cache (shared by every project
# on this machine). This stops the development containers; the database,
# the npm cache and the downloaded crates are kept, and the next build
# starts from scratch.

# Free the disk space of the build caches (Rust target volumes, BuildKit cache)
clean-cache:
    {{compose}} --profile tools --profile web --profile e2e down
    docker volume rm --force mlbpp-dev_cargo-target mlbpp-dev_api-target mlbpp-dev_worker-target
    docker builder prune --force

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

# (sqlx-cli rather than `api migrate`, which needs the new queries' metadata to compile)
# Apply pending migrations to the development database
migrate:
    {{be}} bash -c 'sqlx migrate run --source migrations --database-url "$DEV_DATABASE_OWNER_URL"'

# Refresh backend/.sqlx after adding or changing a query (builds use it offline)
sqlx-prepare: migrate
    {{be}} bash -c 'SQLX_OFFLINE=false DATABASE_URL="$DEV_DATABASE_OWNER_URL" cargo sqlx prepare --workspace -- --all-targets'

# Fail if backend/.sqlx is out of date with the queries
sqlx-check: migrate
    {{be}} bash -c 'SQLX_OFFLINE=false DATABASE_URL="$DEV_DATABASE_OWNER_URL" cargo sqlx prepare --workspace --check -- --all-targets'

# (also checks that the license policy still rejects GPL code)
# Check licenses, security advisories and crate sources of the Rust dependencies
deny:
    {{be}} cargo deny --locked check
    {{be}} scripts/check-deny-policy.sh

# Backend quality gates: format, lint, domain purity, query metadata, tests
check: fmt-check lint check-domain sqlx-check test

# --- API contract and generated client ---------------------------------------

# Write the OpenAPI contract generated from the code to backend/openapi.json
openapi:
    {{be}} bash -c 'cargo run --locked --quiet -p api -- openapi > /tmp/openapi.json && mv /tmp/openapi.json openapi.json'

# Regenerate the contract, then the Angular client in web/projects/ui/src/lib/api/generated
generate-client: openapi web-deps
    {{fe}} npx ng-openapi-gen --config ng-openapi-gen.json

# (CI job "web", without Rust: the unit test committed_contract_is_up_to_date of the
# api crate ties backend/openapi.json to the code, in the backend job)
# Fail, showing the diff, if the generated client is out of date with backend/openapi.json
client-check: web-deps
    #!/usr/bin/env bash
    set -euo pipefail
    before=$(mktemp -d)
    trap 'rm -rf "$before"' EXIT
    cp -r web/projects/ui/src/lib/api/generated/. "$before/" 2>/dev/null || true
    {{fe}} npx ng-openapi-gen --config ng-openapi-gen.json
    if ! diff -ruN "$before" web/projects/ui/src/lib/api/generated; then
        echo "The generated client was out of date with backend/openapi.json: it is now regenerated, review and commit it." >&2
        exit 1
    fi

# --- Front end ----------------------------------------------------------------

# Install web/node_modules from the lock file when it changed
web-deps:
    {{fe}} bash -c '[ node_modules/.package-lock.json -nt package-lock.json ] || npm ci --no-audit --no-fund'

# (ESLint with accessibility rules; stylelint: no hard-coded color, font, radius or shadow)
# Lint TypeScript, templates and styles
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
    {{fe}} node --test scripts/check-licenses.test.mjs
    {{fe}} node scripts/check-licenses.mjs

# Run the end-to-end tests (Playwright, axe-core) against the production build
e2e *args: web-deps
    {{pw}} npx playwright test {{args}}

# (none allowed in what ships; none high or critical in the build and test
# tools, but the dated exceptions of web/audit-exceptions.json)
# Fail on known vulnerabilities of the npm dependencies
web-audit: web-deps
    {{fe}} npm audit --omit=dev --audit-level=low
    {{fe}} node --test scripts/check-audit.test.mjs
    {{fe}} node scripts/check-audit.mjs

# Accessibility checks only (axe-core in the end-to-end tests)
a11y: (e2e "--grep" "axe-core")

# Front-end quality gates: lint, translations, client, licenses, tests, e2e
web-check: web-lint web-i18n client-check web-licenses web-test e2e

# --- Production stack (deploy/compose.yaml) ------------------------------------
# Run locally with compose.local.yaml: images built from the sources,
# https://localhost:18443 with a self-signed certificate. See deploy/README.md.

# (CI: MLBPP_BUILD_CACHE reads a registry cache of the Rust build, and
# MLBPP_BUILD_CACHE_WRITE=true writes it; see deploy/compose.ci-cache*.yaml.)
# Writing goes through the builder of `just ci-builder`.
cache_write := if env("MLBPP_BUILD_CACHE", "") == "" { "false" } else { env("MLBPP_BUILD_CACHE_WRITE", "false") }
stack_cache := if env("MLBPP_BUILD_CACHE", "") == "" { "" } else if cache_write == "true" { " -f deploy/compose.ci-cache.yaml -f deploy/compose.ci-cache-write.yaml" } else { " -f deploy/compose.ci-cache.yaml" }
stack_builder := if cache_write == "true" { "BUILDX_BUILDER=mlbpp-ci " } else { "" }
stack := "MLBPP_DOMAIN=localhost MLBPP_REGISTRY=mlbpp-local MLBPP_VERSION=local MLBPP_PUBLIC_PORT=:18443 BOOTSTRAP_ADMIN_EMAIL=root@example.test " + stack_builder + "docker compose -f deploy/compose.yaml -f deploy/compose.local.yaml" + stack_cache

# Lint the scripts (shellcheck) and the CI workflows (actionlint), validate the compose files
deploy-lint: compose-secrets
    docker run --rm -v "$PWD:/repo:ro" -w /repo rhysd/actionlint:1.7.12 -no-color
    docker run --rm -v "$PWD:/mnt:ro" -w /mnt koalaman/shellcheck:v0.11.0 -x -P deploy/tests deploy/tests/*.sh deploy/scripts/*.sh deploy/postgres/*.sh scripts/*.sh backend/scripts/*.sh
    {{stack}} --profile sso --profile observability config --quiet

# Create the missing secret files of the stack in deploy/secrets/
compose-secrets:
    deploy/scripts/generate-secrets.sh

# Build the production images (api, worker, mcp, web, postgres)
images-build:
    {{stack}} build web api worker mcp postgres

# Push the production images as VERSION and latest (release workflow; needs a registry login)
images-publish version registry="ghcr.io/argoleon":
    #!/usr/bin/env bash
    set -euo pipefail
    [[ "{{version}}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "version must be vX.Y.Z, got '{{version}}'" >&2; exit 1; }
    for name in api worker mcp web postgres; do
        for tag in "{{version}}" latest; do
            docker tag "mlbpp-local/mlbpp-$name:local" "{{registry}}/mlbpp-$name:$tag"
            docker push "{{registry}}/mlbpp-$name:$tag"
        done
    done

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

# (nginx, its Content-Security-Policy, Traefik; from the e2e container on its
# network. Every test by default; Playwright options pass through, e.g.
# `just compose-e2e --grep=axe-core`.)
# Run the end-to-end tests against the local production stack
[positional-arguments]
compose-e2e *args: web-deps
    {{compose}} --profile e2e build e2e
    deploy/tests/e2e.sh "$@"

# What compose-test runs on the stack: the tests that need it (*-stack), and
# each screen's axe-core pass in one theme and language, which loads it
# behind nginx and its Content-Security-Policy. Every test, in every theme
# and language, already runs on the production build (`just e2e`, CI job
# "web"): the mocked ones gain nothing more here.
stack_e2e := "--grep=-stack\\.spec\\.ts|light mode, fr .*axe-core"

# Deployment tests: smoke, backups, scaling, optional profiles, end to end
compose-test: compose-up
    deploy/tests/smoke.sh
    deploy/tests/backup.sh
    deploy/tests/scale.sh
    deploy/tests/profiles.sh
    just compose-e2e {{quote(stack_e2e)}}

# --- Continuous integration ---------------------------------------------------
# GitHub Actions runs one job per ci-* recipe (.github/workflows/ci.yml) and
# nothing else: `just ci` reproduces the whole pipeline locally. The ci-disk-*
# and ci-dev-image recipes only prepare a GitHub runner.

# Secret scan of the git history (gitleaks), with a self-test
gitleaks:
    scripts/gitleaks.sh

# (GitHub runners only) Start deleting the unused preinstalled toolchains, in the background
ci-disk-free:
    scripts/ci-disk.sh free

# (GitHub runners only) Wait until ci-disk-free is done
ci-disk-wait:
    scripts/ci-disk.sh wait

# (Tagged with its Dockerfile's hash: an image is never reused for another
# Dockerfile. DEV_IMAGE_PUSH=true publishes it when built: pushes to the
# long-lived branches only, so a pull request never writes what they use.)
# Pull the development image of the Rust toolchain, or build it when none is published for its Dockerfile
ci-dev-image registry="ghcr.io/argoleon":
    #!/usr/bin/env bash
    set -euo pipefail
    tag=$(sha256sum deploy/dev/backend.Dockerfile | cut -c1-16)
    ref="{{registry}}/mlbpp-dev-backend:$tag"
    if docker pull --quiet "$ref" > /dev/null 2>&1; then
        docker tag "$ref" mlbpp-dev-backend
        echo "mlbpp-dev-backend: pulled $ref"
        exit 0
    fi
    echo "mlbpp-dev-backend: $ref not published, built"
    {{compose}} build backend
    if [ "${DEV_IMAGE_PUSH:-false}" = true ]; then
        docker tag mlbpp-dev-backend "$ref"
        docker push --quiet "$ref"
    fi

# (The default docker driver can read a registry cache, not write one.)
# BuildKit builder that writes the cache of the Rust images, when MLBPP_BUILD_CACHE_WRITE=true
ci-builder:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "{{cache_write}}" = true ]; then
        docker buildx inspect mlbpp-ci > /dev/null 2>&1 \
            || docker buildx create --name mlbpp-ci --driver docker-container --bootstrap
    fi

# CI job "backend": quality gates and dependency policy of the Rust code
ci-backend: check deny

# CI job "web": quality gates, vulnerabilities and accessibility of the front end
ci-web: web-check web-audit

# CI job "deploy": images, compose stack and end-to-end tests on it
ci-deploy: ci-builder deploy-lint images-test compose-test

# CI job "security": secrets in the repository
ci-security: gitleaks

# The whole CI pipeline, as GitHub Actions runs it
ci: ci-security ci-backend ci-web ci-deploy
