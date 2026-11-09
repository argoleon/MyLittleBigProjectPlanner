# CLAUDE.md

Guidance for Claude Code working in this repository.

## Project

My Little Big Project Planner is an open source (MIT) project and ticket management tool: Jira-like configuration, request portals, portfolios and programs, Gantt, fine-grained resource workload, REST API and MCP server.

- **Source of truth**: the specification, in `docs/en/specification.md` (English) and `docs/fr/specification.md` (French original; it prevails if the two differ). Read the relevant section before implementing a feature. If the code and the specification disagree, stop and ask; do not silently pick one.
- **Mockups**: `docs/mockups/` (25 screens, Material Design 3, interface in French). See "Using the mockups" below.
- **MVP scope and lots**: section 20 of the specification. Anything listed in section 21 is out of the MVP: do not build it unless asked.

## Language

- Code, identifiers, comments, commit messages, pull requests and new documentation: **English**.
- User-facing text: never hard-coded. Use translation keys (Transloco), with French and English provided.
- The mockups are in French; use English identifiers (e.g. "portefeuille" → `portfolio`, "charge" → `workload`, "affectation" → `assignment`).

## Repository layout (target)

```
backend/        Rust workspace
  crates/domain   business rules, no I/O (capacity, workload, recurrences, workflows, query language)
  crates/infra    PostgreSQL, S3 storage, email, Valkey
  crates/api      REST API (Axum), sessions, OAuth 2.1 server
  crates/worker   background jobs (SKIP LOCKED queue)
  crates/mcp      MCP server, calls the public API
web/            Angular workspace
  projects/app    internal application
  projects/portal request portal
  projects/ui     shared library: theme, components, generated API client
deploy/         docker-compose and profiles (sso, observability)
docs/           specification (en, fr) and mockups
```

Every CI step is a `just` recipe; GitHub Actions only calls them (one `ci-*` recipe per job, see `docs/en/developer-guide/ci.md`). Run the same recipes locally before declaring work done: `just ci` is the whole pipeline. Never put logic in the workflow files: add or change a recipe instead.

## Development environment (containers only)

**Nothing runs natively on the development machine.** The host only needs Docker, git and `just`; builds, tests, linters, code generators, database tools and dev servers all run in the containers of `deploy/compose.dev.yaml`.

- `just dev` starts PostgreSQL, the API (http://127.0.0.1:18080) and the internal application (http://127.0.0.1:4200, `/api` proxied to the API), all with live reload.
- `just build`, `just lint`, `just test`, `just migrate`, `just sqlx-prepare`… run in the `backend` toolchain container; `just shell` opens a shell in it.
- `just web-lint`, `just web-test`, `just web-build`, `just generate-client`… run in the `web` container; `just e2e` runs Playwright in the `e2e` container. Before a pull request: `just ci-backend` and `just ci-web` (quality gates, licenses, vulnerabilities), and `just gitleaks`.
- The production stack (`deploy/compose.yaml`, see `deploy/README.md`) also runs locally: `just compose-up` (https://localhost:18443), `just compose-test`, `just images-test` (images and SBOMs), `just deploy-lint`, `just compose-down`. A change to a Dockerfile, the compose file or a service's configuration must keep these green.
- Never install a toolchain (Rust, Node, sqlx-cli, Playwright…) on the host for project work, and never call `cargo` or `npm` directly from the host. A new tool goes into the matching dev image (`deploy/dev/*.Dockerfile`) and is reached through a `just` recipe.
- Containers run with the host user's uid and gid, so generated files (lock files, `.sqlx`, formatted code) belong to the developer.

## Tests (mandatory)

Unit tests and integration tests are **mandatory** for every development. A feature without them is not done and must not be merged.

- **Unit tests**: every business rule, service, component and utility gets unit tests. Backend rules in `crates/domain` are tested without a database; Angular components and services are tested in isolation.
- **Integration tests**: every API endpoint, repository, worker job and MCP tool gets integration tests against real dependencies (real PostgreSQL with RLS enabled, S3-compatible storage, Valkey), never against mocks of them. Test the error cases (Problem Details codes, permission denials, cross-tenant isolation) as well as the happy path.
- Key user flows also get a Playwright end-to-end test.
- A bug fix starts with a test that reproduces the bug.
- Never delete, skip or weaken an existing test to make the build pass; fix the code, or stop and ask if the test itself looks wrong.
- All tests run in CI through `just` and must pass before a feature is closed.

## Git workflow (Gitflow)

The project follows **Gitflow**.

- `main`: production. Contains only released, working versions; every commit on `main` is a release tagged `vX.Y.Z`. Never commit directly to it.
- `develop`: integration branch. Never commit directly to it either.
- `feature/<package>-<short-description>` (e.g. `feature/L02-role-permissions`): **every development is a feature**, branched from `develop`. Once it is validated (review approved, unit and integration tests written and passing, CI green), the feature is finished: merged into `develop` and the feature branch is deleted.
- `release/X.Y.Z`: when a work package (lot, see section 20 of the specification) is complete and working on `develop`, a release branch is created from `develop`. Only stabilization fixes go there. The release is then finished: merged into `main`, tagged `vX.Y.Z`, merged back into `develop`, and deployed.
- `hotfix/X.Y.Z`: urgent production fix, branched from `main`, merged into both `main` (tagged) and `develop`.
- Commit messages in English, using Conventional Commits (`feat:`, `fix:`, `test:`, `docs:`, `refactor:`, `chore:`).
- Before starting a development, check you are on the right feature branch; if none exists, create it from an up-to-date `develop`.

## Licensing (blocking)

The project is MIT. Only add dependencies under MIT, Apache-2.0, BSD, ISC, 0BSD, Zlib or Unicode-3.0 (the last two decided on 2026-11-09: permissive, and unavoidable in Rust through `foldhash` and the Unicode/ICU4X crates). Fonts may be OFL-1.1. Never add GPL, LGPL, AGPL, SSPL or commercial dependencies; `cargo-deny` and the npm license check fail the build otherwise. If a needed library has an incompatible license, stop and ask.

Accepted exception (decided 2026-11-09): npm packages that are build or test tools and never end up in the shipped bundles may also be under Python-2.0, MPL-2.0, CC-BY-4.0, BlueOak-1.0.0, CC0-1.0 or MIT-0. Today these are, among others, `argparse` (via ng-openapi-gen and Transloco's config tooling), `lightningcss` (via Vite), `axe-core` (accessibility tests), `caniuse-lite`, `lru-cache`, `sax`, `mdn-data` and the `@csstools/*` packages of stylelint. `just web-licenses` enforces it on the bundles themselves (esbuild metafiles of `ng build --stats-json`). Everything shipped stays limited to the list above.

## Backend rules (summary)

- Axum, Tokio, SQLx with compile-time checked queries, serde, utoipa, tracing. Keep Rust simple: few generics, no custom macros.
- Every business table has `tenant_id` and is protected by PostgreSQL Row-Level Security. The tenant context is set per transaction by the shared request layer (`SET LOCAL`); never bypass it.
- UUID v7 keys, `timestamptz` in UTC, planning dates as `date` (local to the resource calendar), translatable labels as JSONB `{"fr": …, "en": …}`.
- Business rules live in `crates/domain` and are unit-tested without a database. Workload and recurrence rules need tests for DST changes, part-time capacity and public holidays.
- Errors use RFC 9457 Problem Details with stable codes. The OpenAPI contract is generated from the code; the Angular client is generated from it.

## Frontend rules

Stack: latest Angular, standalone components, signals, zoneless change detection, `OnPush`. UI components come from **Angular Material (Material Design 3) and the CDK only**; no other component library.

### Theme

- The theme lives in `web/projects/ui/styles/` and is included once per application: `@use 'ui/styles/theme';` (add `"stylePreprocessorOptions": { "includePaths": ["projects"] }` in `angular.json`).
  - `_palettes.scss`: M3 tonal palettes generated from seed `#769CDF`. Do not hand-edit.
  - `_theme.scss`: `mat.theme()` with light and dark modes (`color-scheme: light dark`, overridable with `<html data-theme>`), Roboto, density 0.
  - `_app-tokens.scss`: `--app-*` tokens for what M3 does not cover (status categories, issue types, heatmap levels, Gantt, success, spacing).
- **Never hard-code a color, font, radius or shadow.** Use `--mat-sys-*` (colors, typescale such as `--mat-sys-title-medium`, shapes such as `--mat-sys-corner-medium`, elevation) or `--app-*`. If a value is missing, add a token to `_app-tokens.scss` first.
- Portals are themed at runtime from their brand color with `applyPortalTheme()` (`web/projects/ui/src/lib/theming/portal-theme.ts`).
- Fonts and icons are self-hosted (`@fontsource/roboto`, `material-symbols` npm packages). No Google Fonts or other CDN: users' IP addresses must not leak to third parties (GDPR).

### Using the mockups

`docs/mockups/source/*.dc.html` are design files, not code. Each one contains the screen markup with inline styles, and a `renderVals()` block holding **fake data**. A PNG preview of every screen is in `docs/mockups/previews/` (the table in `docs/mockups/README.md` maps each source file to its PNG); look at it to check the visual result.

- Use them as a visual and structural reference. Never copy their HTML, inline styles or CSS classes into the application.
- Map each mockup element to its Angular Material equivalent:

| Mockup element | Implementation |
| --- | --- |
| Navigation drawer (`nav-item`) | `mat-sidenav` + `mat-nav-list` with `routerLinkActive` |
| `.btn.filled` / `.tonal` / `.outlined` / `.text` | `<button matButton="filled">` / `"tonal"` / `"outlined"` / `matButton` |
| `.fab` (extended) | `<button matFab extended>` |
| `.icon-btn` | `<button matIconButton aria-label="…">` |
| `.seg` (segmented button) | `mat-button-toggle-group` |
| `.chip` (filter chips) | `mat-chip-listbox` + `mat-chip-option` |
| `.tf` + `.tf-in` (outlined field) | `mat-form-field appearance="outline"` + `matInput` / `mat-select` |
| `.tab` | `mat-tab-group`, or `mat-tab-nav-bar` for routed tabs |
| `.sw` (switch) | `mat-slide-toggle` |
| Dialogs on a grey background | `MatDialog` |
| `.tbl` tables | `mat-table` (+ `cdk-virtual-scroll-viewport` for long lists) |
| `.lbl` status / type labels | shared `ui-status-chip` / `ui-type-chip` components using `--app-status-*` / `--app-type-*` |
| Kanban columns and backlog | CDK drag and drop |
| `.ms` icons | `mat-icon` with the Material Symbols Outlined font |
| Progress bars | `mat-progress-bar` |

- Replace fake data with signals fed by the generated API client. Never call `fetch` or `HttpClient` directly from components.
- The workload heatmap and the Gantt are **custom components**: the mockups only define their appearance. Implement them with virtualized rendering (canvas or SVG, only visible rows drawn), keyboard navigation, and an equivalent accessible table view. Targets: 5,000 Gantt rows with smooth scrolling; heatmap of 200 resources over 3 months in under 1 s.

### Accessibility and i18n

- Target RGAA 4 / WCAG 2.1 AA. Real `<button>` and `<a>` elements, labels on every field, `aria-label` on icon-only buttons, visible focus. Contrast is guaranteed by the tokens: do not override colors locally.
- axe checks run in CI; fix violations, do not suppress them.
- Dates and numbers are formatted with the user's locale and time zone. Planning dates are local dates, never converted through UTC.

### Workflow for a screen

1. Read the matching section of the specification and the mockup source and preview.
2. Build or reuse shared components in `projects/ui` first (shell, chips, empty states), then the screen.
3. Wire real data through the generated client; handle loading, empty and error states (the mockups only show the filled state).
4. Add unit tests and integration tests (mandatory, see "Tests"), and a Playwright test for key flows.
5. Run lint, tests and accessibility checks through `just` before finishing.
