# Changelog

All notable changes of My Little Big Project Planner. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses [Semantic Versioning](https://semver.org/). Each release finishes a work package (lot) of the MVP (specification §20).

## [Unreleased]

### Security

- OIDC sign-in: `jsonwebtoken` 10.4 (GHSA-h395-gr6q-cpjc, CVE-2026-25537: a time claim of the wrong type skipped its check). Its signatures are checked with `ring` through the application's own provider, keeping the `rsa` crate and `aws-lc` out; ID tokens not valid yet (`nbf`) are refused too.
- Development tools: `minimatch` 11 forced under eslint and typescript-eslint (denial of service through glob patterns); nothing of it ships.

## [0.2.0] - 2026-11-12

Work package **L02 Identities, roles, permissions**: accounts and tenants, sign-in (password, two-step verification, the organization's identity provider), members and groups, node roles and the permission engine, My account, and the GDPR rights.

### Added

- **Tenants** (#34)
  - Platform super admins create, change and delete tenants through `/api/v1/platform/tenants`. A deleted tenant stops resolving at once and is purged in the background.
  - Settings per tenant: language, time zone, default capacity, overload tolerance and mode, quotas.
  - The first tenant and its super admin are bootstrapped at the first start (`BOOTSTRAP_*`).
- **Local sign-in** (#38)
  - Global accounts with Argon2id passwords, and memberships per tenant (internal or external, tenant role).
  - Sessions in Valkey, behind `HttpOnly` cookies with a CSRF token.
  - Password reset by email, through the worker's job queue; sign-in and reset requests are rate-limited.
- **Two-step verification** (#44): authenticator app (TOTP) with 8 single-use backup codes; a tenant can require it.
- **Single sign-on** (#48)
  - OIDC providers per tenant (Entra ID, Google, Keycloak…), with encrypted client secrets.
  - Members are matched by their verified email or a directory claim.
  - A tenant can require SSO for its internal members; local accounts (contractors, the bootstrapped admin) keep their password.
- **Several tenants per account** (#52): the organization selector switches tenants through a one-time link, under the target tenant's policy. External accounts never reach the internal application.
- **Members and groups** (#55)
  - Administration › Members: search, filters, invitations by email (accepted on a page that creates or joins the account), access changes and removal. The last tenant admin always stays.
  - Administration › Groups.
- **Node roles and permissions** (#60)
  - A fixed permission catalog, and roles as sets of permissions edited in Administration › Roles. Four default roles per tenant.
  - The permission check answers 403 `permission.denied`; effective permissions are cached in Valkey and invalidated by changes. Grants on hierarchy nodes come with L03.
- **My account** (#64)
  - Language and time zone of the account, applied in every tenant.
  - Password change, which ends the other sessions.
  - Two-step verification, and the open sessions with their device and latest activity, ended one by one.
- **GDPR** (#67)
  - Export of one's personal data as JSON.
  - Erasure by anonymization: the account itself (with its password, or a recent SSO sign-in, and its email typed) or a platform super admin erases a global account; a tenant admin erases a member from their tenant only. Erased members appear as "Deleted user #N"; every erasure is audited.
  - Technical logs, which hold client IP addresses, are rotated and kept 6 months by Loki.
- **Development** (#487, #489, #491): `just clean-cache` frees the build caches; the local stack also answers on https://127.0.0.1:18443.

### Changed

- Debug builds keep line tables only, and no debug information for dependencies: the build caches are about three times smaller.

### Upgrading from 0.1.0

- **New secrets**, created by `deploy/scripts/generate-secrets.sh`:
  - `data_encryption_key`: encrypts the TOTP and OIDC client secrets; keep a copy with the backups;
  - `bootstrap_admin_password`;
  - `smtp_url`: a placeholder, to replace with your SMTP server.
- **New variables:** `PUBLIC_URL` (links of the emails) and `MAIL_FROM`; `BOOTSTRAP_ADMIN_EMAIL` to bootstrap the first super admin. The worker now also needs `VALKEY_URL` (compose provides it).
- **Migrations** run at start-up (`migrate` service). See `deploy/README.md` and `deploy/.env.example`.

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

[0.2.0]: https://github.com/argoleon/MyLittleBigProjectPlanner/releases/tag/v0.2.0
[0.1.0]: https://github.com/argoleon/MyLittleBigProjectPlanner/releases/tag/v0.1.0
