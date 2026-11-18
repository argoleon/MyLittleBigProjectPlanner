# Changelog

All notable changes of My Little Big Project Planner. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses [Semantic Versioning](https://semver.org/). Each release finishes a work package (lot) of the MVP (specification §20).

## [Unreleased]

## [0.5.1] - 2026-11-18

Security fix of 0.5.0: the email client had known vulnerabilities, and the dependency checks no longer passed.

### Security

- **lettre 0.11.24** (#537), the SMTP client sending every email (invitations, password resets, notifications): fixes RUSTSEC-2026-0560 to 0565. These cover STARTTLS response injection by a man in the middle, SMTP command injection through bare CR or LF, a quoted local part sent to another address, panics on an `EHLO` reply or a display name holding CR or LF, and unbounded buffering of an SMTP reply.

### Fixed

- **The status menu of a ticket** no longer closes by itself right after a transition (#526). The page reads the ticket again on the real-time echo of the member's own change; meanwhile the menu turned into a plain status for an instant, which closed it when it was open. It now keeps the transitions shown until the new ones are read.

### Changed

- **Dates and times** are handled by `time` instead of `chrono` (#531), which is no longer maintained (RUSTSEC-2026-0543). The API answers exactly as before: times in RFC 3339 (UTC), dates as `YYYY-MM-DD`, the same OpenAPI contract.
- **npm audit** (#535): a high or critical advisory with no fixed version may be accepted in the build and test tools only, until a review date (`web/audit-exceptions.json`). What ships still has no exception. Accepted until 2027-01-15: picomatch (GHSA-fc5q-39xc-vjpf, ReDoS on crafted glob patterns), which never ships and only reads the glob patterns of the project's own configuration.

### Upgrading from 0.5.0

- Nothing to do besides pulling the 0.5.1 images: no migration, no new secret, no configuration change.
- Signed-in members stay signed in: sessions written by 0.5.0 are read as they are.

## [0.5.0] - 2026-11-16

Work package **L05 Tickets and collaboration**: tickets with their fields, workflow and relations, rich comments with mentions, attachments scanned by an antivirus, the history of every change, notifications in the app and by email, real-time updates, and the migration of tickets when a configuration change is published.

### Added

- **Tickets** (#130)
  - Created from a project page ("New ticket") or from "My tickets", with the type's creation screen; keys per project (`PLAT-142`) allocated without gaps under concurrency; a fixed priority list of five levels.
  - The description is rich text (Tiptap editor), stored as a document and rendered by the server to sanitized HTML.
  - The ticket page (mockup 03): type and key, summary, status, description, details and custom fields, project.
- **Editing and moving** (#136)
  - An "Edit" dialog shows the type's view screen for the current status; saving checks the version read (`If-Match`, 412 when someone changed it meanwhile).
  - A ticket moves to another project with its sub-tasks: new keys, the former ones still open it; values of fields the target configuration lacks are listed, then dropped.
- **Links and sub-tasks** (#140)
  - Blocks, depends on, duplicates and relates to, each read from both tickets; sub-tasks added from their parent; the parent changes within the project, the type hierarchy enforced. A minimal ticket search (key, former key or summary) finds what to link.
- **Status transitions** (#515)
  - The transitions the workflow offers from the current status, as buttons on the ticket page; who may take each, the fields it asks for (transition screen), the fields required at the target status, then its post-functions (assign, set or clear a field, add a comment).
- **Comments** (#144)
  - Rich text with @mentions, internal by default or public; the author edits and deletes theirs, moderators delete any (a placeholder stays).
- **Attachments** (#148)
  - Uploaded and downloaded directly from the S3 storage with presigned URLs, on the application's own origin; every file is scanned by ClamAV before it can be downloaded, an infected one deleted. Image thumbnails, a size limit and a storage quota per tenant, one stored copy of a file uploaded twice.
- **History** (#153)
  - Every change of a ticket is recorded with who made it, when and from where (interface, API, MCP, import, system): the Activity tab tells it in sentences, a description before and after on demand, deleted comments for moderators only.
- **Notifications** (#157)
  - Ticket created, assigned, status changed, commented and mentioned notify the people the configuration's rules name, watchers included (Watch / Unwatch on the ticket page). A bell with the unread count, a notifications page, and preferences per event for the app and email, with an optional daily digest.
- **Real time** (#163)
  - The ticket page and the bell follow changes as they happen (Server-Sent Events); while a form is open, someone else's change shows a warning instead, the input kept.
- **Migrating tickets on a configuration change** (#125, #127)
  - The mapping wizard counts the tickets of every project of the configuration; once confirmed, the worker moves them batch by batch, each change in their history by the system, then publishes the draft. A migration interrupted midway resumes without moving a ticket twice. Publishing directly is refused while tickets use a removed status or type (`config.migration_required`).

### Upgrading from 0.4.0

- **Migrations** run at start-up (`migrate` service): tickets, links, comments, the history (`audit_log`) and the event outbox (both partitioned by month), attachments, watchers and notifications, all protected by Row-Level Security.
- **New secrets** `s3_access_key` and `s3_secret_key`: run `deploy/scripts/generate-secrets.sh` again. An installation from 0.4.0 already has `seaweedfs_s3_config`: the script takes both keys from it, so the storage keeps its identity.
- **Attachments** use the `seaweedfs` and `clamav` services and Traefik's route of the `/mlbpp-attachments/` prefix (all in `deploy/compose.yaml`). A proxy in front of Traefik must keep the Host header. On its first start ClamAV downloads its signatures for a few minutes: uploads stay "Scanning" meanwhile. Back up the `seaweedfs-data` volume besides the database (see "Attachments" in `deploy/README.md`).
- **New variables** (optional): `ATTACHMENT_MAX_BYTES` (50 MB by default, at most 100 MB); the worker now reads `PUBLIC_URL` for the links of notification emails, and `CLAMAV_ADDR` (set by `compose.yaml`).
- **Real time** goes through Valkey (one stream per tenant) and long-lived `GET /api/v1/events` connections: a proxy in front of Traefik must not buffer them.

## [0.4.0] - 2026-11-14

Work package **L04 Project configuration**: shared project configurations with their issue types, workflows, fields, transition rules, screens and notification rules, edited in a draft and published to every project at once, with the status and type mapping wizard.

### Added

- **Project configurations** (#95)
  - A configuration gathers a project's issue types, fields, workflows, screens and notifications; several projects share one, each project has exactly one. Every tenant gets a default configuration, which existing and new projects use.
  - Changes go to a draft shared by the configuration admins (tenant admins and configuration admins); publishing applies them to every project at once, after the list of the projects affected. A project detaches by duplicating its configuration.
  - Administration › Project configurations, the configuration page with its tabs, and the Configuration tab of a project's settings. Labels are translatable (French and English) everywhere.
- **Issue types** (#100)
  - Epic, Story, Task, Bug and Sub-task by default, each with its icon, color and the types it may sit under; sub-tasks always have a parent. Types are reordered by drag and drop or with the keyboard.
- **Workflows** (#109)
  - Statuses in three categories (To do, In progress, Done) with a portal label, and transitions from a status or from any; a standard workflow (mockup 11) for every type, more workflows added or copied.
  - Edited in tables, shown as a read-only graph. What keeps a workflow from being published (no initial status, a status nothing leads to…) is listed, and publishing waits for it (`workflow.invalid`).
- **Custom fields** (#104)
  - The 14 field types of the MVP with their options; the system fields Points and Remaining work.
  - Hidden, visible or required per issue type and per workflow step, in one grid per workflow; a published list value is deactivated rather than removed.
- **Transition rules** (#114)
  - Who may take a transition (roles on the project or groups), the fields it requires, and post-functions run in order: assign, set or clear a field, add a comment. Tickets (L05) apply them.
- **Screens** (#118)
  - General creation and view screens, screens for one type, and named transition screens attached to transitions; the field rules win over a screen (hidden fields left out, required ones added).
- **Notification rules** (#122)
  - Whom ticket created, assigned, status changed, commented and mentioned notify: the reporter, the assignee, watchers, the people mentioned, roles, groups or members; never the author of the change. Jira-style defaults; tickets (L05) send them.
- **Status and type mapping wizard** (#126, #128)
  - When a draft removes statuses or types that tickets use, publishing maps each to one the draft keeps (mockup 13; `mapping_incomplete` until all are) and migrates in the background, publishing once done.
  - Moving the tickets themselves comes with tickets in L05 (#125, #127, #129): until then no ticket needs moving.

### Upgrading from 0.3.0

- **Migrations** run at start-up (`migrate` service): the configuration tables (versions, types, workflows, fields and their rules, screens, notification rules, migrations), all protected by Row-Level Security. Every tenant gets its default configuration, which its projects use.
- No new secret nor variable. The worker runs the new `config.migration` job; nothing to configure.

## [0.3.0] - 2026-11-13

Work package **L03 Hierarchy**: portfolios, programs and projects, several weighted parents, roles granted on nodes and inherited, confidential projects, aggregated indicators, and archiving.

### Added

- **Portfolios, programs and projects** (#71)
  - Nodes with a name, a project key unique per tenant, description, objectives, dates, health (on track, at risk, off track) and owners, under `/api/v1/nodes`.
  - The attachment matrix and cycle prevention are enforced in the database; a closure table answers ancestors and descendants.
  - Settings of a node at `/nodes/:id/general`, and "Add a child", which creates the node and its link.
- **Several weighted parents** (#75)
  - A node is linked to several parents, each link with a weighting (0 to 100 %, default 100).
  - The "Attachments and permissions" tab: Parents card with the weighting and detach, a parent search that says why a candidate is refused, Children card.
  - Errors `node.parent_not_allowed` and `node.cycle` (409); two concurrent links closing a cycle are serialized.
- **Roles granted on nodes** (#80)
  - A role granted to a member or a group on a node applies to it and to everything below; with several parents, access is the union of every path.
  - Tenant admins, and members who manage the members of a node, grant roles within their own permissions only.
  - The "Effective access" card says where each role comes from (direct, or inherited from which node), and grants or revokes roles.
- **Confidential projects** (#84)
  - A confidential project only hears the roles granted on it, which still apply below it; the switch is audited.
  - A member who sees a parent without access to the project finds it veiled: "Confidential project", with no name, key, owner or real id. No name of a node a member cannot see leaves the API.
  - `node.confidential_lockout` (409): an administrator whose role is only inherited cannot shut themselves out.
- **Aggregated indicators** (#87)
  - A node shows its planned load, consumed capacity, open and late tickets over its distinct descendant projects: a project reached through several paths counts once, the weighting applies at its parent only, archived projects are left out, confidential ones counted apart.
  - Precomputed by the worker a couple of seconds after a change (`node.aggregates` job, changes in a burst share one job). See `docs/en/developer-guide/aggregates.md`.
  - `/nodes/:id` is now the view of a node (indicator cards and the tree of what is under it); the Portfolios page shows the top of the hierarchy as the same tree. `GET /api/v1/hierarchy` browses it one level at a time.
  - Tickets (L05) and workload (L08) will feed the figures; until then, indicators show the number of projects and zeros.
- **Archiving** (#92)
  - A project is archived or restored from the "Archiving" tab of its settings, after a confirmation; the change is audited.
  - An archived project is read-only (`node.archived`, 409, on every write on it, its links and its children), leaves the active lists and its ancestors' indicators, and comes back once restored. Sub-projects are archived first and restored after.
  - The Portfolios page switches between the hierarchy and the archived projects (`GET /api/v1/nodes?archived=true`).

### Fixed

- Deleting a group now forgets the cached permissions of its members at once (#498).

### Security

- OIDC sign-in: `jsonwebtoken` 10.4 (GHSA-h395-gr6q-cpjc, CVE-2026-25537: a time claim of the wrong type skipped its check). Its signatures are checked with `ring` through the application's own provider, keeping the `rsa` crate and `aws-lc` out; ID tokens not valid yet (`nbf`) are refused too.
- Development tools: `minimatch` 11 forced under eslint and typescript-eslint (denial of service through glob patterns); nothing of it ships.

### Upgrading from 0.2.0

- **Migrations** run at start-up (`migrate` service): the hierarchy, node grants and aggregates tables, all protected by Row-Level Security.
- No new secret nor variable. The worker runs the new `node.aggregates` job; nothing to configure.

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

[0.5.1]: https://github.com/argoleon/MyLittleBigProjectPlanner/releases/tag/v0.5.1
[0.5.0]: https://github.com/argoleon/MyLittleBigProjectPlanner/releases/tag/v0.5.0
[0.4.0]: https://github.com/argoleon/MyLittleBigProjectPlanner/releases/tag/v0.4.0
[0.3.0]: https://github.com/argoleon/MyLittleBigProjectPlanner/releases/tag/v0.3.0
[0.2.0]: https://github.com/argoleon/MyLittleBigProjectPlanner/releases/tag/v0.2.0
[0.1.0]: https://github.com/argoleon/MyLittleBigProjectPlanner/releases/tag/v0.1.0
