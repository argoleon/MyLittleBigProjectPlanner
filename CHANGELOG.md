# Changelog

All notable changes of My Little Big Project Planner. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses [Semantic Versioning](https://semver.org/). Each release finishes a work package (lot) of the MVP (specification §20).

## [0.1.0] - 2026-11-10

Work package **L01 Technical foundation**: the base every later work package builds on. It ships no business feature yet: the screens are placeholders, and the only API resource is the reference resource that new endpoints copy.

### Added

- **Backend** (Rust workspace: `domain`, `infra`, `api`, `worker`, `mcp`; #1)
  - Configuration from environment variables, with secrets read from files (`*_FILE`); graceful shutdown.
  - PostgreSQL 18 with migrations, two roles (owner, runtime without `BYPASSRLS`) and row-level security on every business table; the tenant is set per transaction and cannot be forgotten (`TenantTx`); single- or multi-tenant mode (#5).
  - RFC 9457 Problem Details errors with a catalog of stable codes, including for framework errors; reference resource `/api/v1/reference-items` showing every API convention: validation with field errors, `ETag`/`If-Match`, `Idempotency-Key`, cursor pagination, field selection, translated labels (#10).
  - OpenAPI 3.1 contract generated from the code, served at `/api/v1/openapi.json`, with self-hosted interactive documentation at `/api/v1/docs/` (#14).
  - Health endpoints `/health/live` and `/health/ready` in every service; JSON logs with a correlation ID (`x-request-id`); OpenTelemetry traces and metrics exported in OTLP when configured (#31).
- **Front end** (Angular 22 workspace: `app`, `portal`, `ui`; #14, #17)
  - Typed API client generated from the contract, with errors turned into translated `ApiProblem`s.
  - Material 3 theme with light and dark modes from tokens only; self-hosted Roboto and Material Symbols.
  - Application shell with the navigation of the mockups, translations in French and English (Transloco), locale-aware date and number formatting (planning dates never converted through UTC).
  - Shared status and type chips, empty, error and loading states.
- **Deployment** (#22)
  - Production images: non-root, distroless for the Rust services, nginx with a strict Content-Security-Policy for the front end, PostgreSQL with pgBackRest; an SBOM for each.
  - Docker Compose stack: Traefik (TLS, load balancing), web, api, worker, mcp, PostgreSQL, Valkey, SeaweedFS, ClamAV; profiles `sso` (Keycloak) and `observability` (Prometheus, Loki, Tempo, Alloy, Grafana with a dashboard); `api` and `worker` scale horizontally.
  - Scheduled, encrypted pgBackRest backups with WAL archiving; technical log retention of 6 months by default.
- **Quality** (#26)
  - Unit, integration (real PostgreSQL), end-to-end (Playwright) and accessibility (axe-core) tests.
  - License policy enforced on Rust crates and npm packages, RustSec and npm advisories, secret scan of the git history.
  - CI: every step a `just` recipe (`just ci` locally), GitHub Actions only calls them; release workflow publishing the images to GitHub Container Registry.

[0.1.0]: https://github.com/argoleon/MyLittleBigProjectPlanner/releases/tag/v0.1.0
