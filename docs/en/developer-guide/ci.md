# Continuous integration

Every CI step is a `just` recipe. GitHub Actions only prepares the runner (checkout, `just`, caches, registry login, and the `ci-disk-*` and `ci-dev-image` recipes) and calls them, so the pipeline runs the same way on a laptop and stays portable to another CI.

```sh
just ci            # the whole pipeline, as GitHub Actions runs it
just ci-backend    # or one job at a time
```

## Jobs

`.github/workflows/ci.yml` runs on every pull request to `develop` and `main`, and on pushes to `develop`, `main`, `release/**` and `hotfix/**`. Its four jobs run in parallel:

| Job | Recipe | Checks |
| --- | --- | --- |
| Secrets (gitleaks) | `just ci-security` | No secret in the whole git history (`just gitleaks`), with a self-test proving that a committed private key fails the scan |
| Backend | `just ci-backend` | `cargo fmt`, clippy (warnings are errors), domain purity, `.sqlx` up to date, unit tests (among them, `backend/openapi.json` up to date with the code), integration tests on a real PostgreSQL with RLS (`just check`); licenses, RustSec advisories and crate sources with cargo-deny, plus a fixture proving that a GPL dependency is rejected (`just deny`) |
| Front end | `just ci-web` | ESLint (accessibility rules included) and stylelint, translation keys, generated client up to date with `backend/openapi.json` (no Rust in this job), npm licenses (with fixture tests), Vitest, Playwright with axe-core (`just web-check`); `npm audit` (`just web-audit`) |
| Deployment | `just ci-deploy` | shellcheck, actionlint and compose validation (`just deploy-lint`); production images: health, non-root, no secret, SBOM (`just images-test`); the compose stack: smoke, backups, scaling, profiles, Playwright on the production build behind nginx and Traefik (`just compose-test`, see below) |

Artifacts: the SBOM of each image (`sbom`, every run), and the Playwright traces and screenshots when an end-to-end test fails (`e2e-results`, kept 14 days).

`.github/workflows/release.yml` runs on a version tag (`vX.Y.Z`, set on `main` when a release is finished): it tests the images, publishes them to GitHub Container Registry (`ghcr.io/argoleon/mlbpp-*:vX.Y.Z` and `latest`) and keeps their SBOMs for a year.

### Policies enforced

- **Licenses** (CLAUDE.md, "Licensing"): `backend/deny.toml` for crates, `web/scripts/check-licenses.mjs` for npm packages, the latter checking the shipped bundles themselves for tools-only licenses.
- **Vulnerabilities**: any RustSec advisory or yanked crate fails; npm: any known vulnerability in what ships, high or critical ones in build and test tools.
- **Secrets**: a false positive is either annotated inline (`gitleaks:allow` with the reason) or, for a past commit, listed in `.gitleaksignore`.

## Configuring the repository

### Secrets

| Secret | Use |
| --- | --- |
| `DOCKERHUB_USERNAME`, `DOCKERHUB_TOKEN` | Recommended. A Docker Hub account (a free one is enough) with a read-only access token: anonymous pulls from shared CI runners quickly hit Docker Hub's rate limit, and the pipeline pulls about fifteen images. Without them, the login steps are skipped. |

Publishing to GitHub Container Registry uses the workflow's own `GITHUB_TOKEN`; nothing to configure. The CI also keeps there, besides the released images, the development image of the Rust toolchain (`ghcr.io/argoleon/mlbpp-dev-backend`) and the build cache of the Rust images (`ghcr.io/argoleon/mlbpp-build-cache`), see below.

### Branch protection

Settings › Rules › Rulesets (or Settings › Branches), for `main` and `develop`:

- Require a pull request before merging, with at least one approval; dismiss stale approvals when new commits are pushed.
- Require these status checks to pass, and the branch to be up to date: `Secrets (gitleaks)`, `Backend (fmt, clippy, tests on PostgreSQL, cargo-deny)`, `Front end (lint, tests, licenses, audit, e2e and axe-core)`, `Deployment (images, SBOM, compose stack, e2e)`.
- Block force pushes and deletions.
- Allow merge commits (Gitflow keeps the history of each feature); `main` only receives `release/*` and `hotfix/*` branches.

Branch protection on a private repository needs a paid GitHub plan (Pro, Team or Enterprise); on a public repository it is free.

## Duration and caches

The four jobs run in parallel; the deployment job is the longest. What keeps them short:

- **Build caches of the development containers.** They live in Docker volumes; in CI, `CARGO_HOME_MOUNT`, `CARGO_TARGET_MOUNT` and `NPM_CACHE_MOUNT` turn them into directories of the runner, saved by `actions/cache` between runs and keyed on the lock files.
- **Development image of the Rust toolchain** (sqlx-cli, cargo-deny… compiled from source: about five minutes). `just ci-dev-image` pulls `ghcr.io/argoleon/mlbpp-dev-backend:<hash of deploy/dev/backend.Dockerfile>`, or builds it when none is published for that Dockerfile. Only the backend job needs it.
- **Compiled dependencies of the Rust images.** `deploy/docker/backend.Dockerfile` compiles them in a layer of their own (cargo-chef), which only changes with `Cargo.lock`; `deploy/compose.ci-cache.yaml` reads the layers of earlier builds from a registry cache, so only our crates are compiled.
- **Disk space.** A runner with less than 40 GB free lacks room for the test binaries and the stack: `just ci-disk-free` then deletes unused preinstalled toolchains in the background while images and caches are fetched, and `just ci-disk-wait` waits for it before the heavy steps. Runners with a larger disk skip it.
- **End-to-end tests on the stack.** Every Playwright test, in both themes and both languages, runs in the front-end job on the production build (`ng serve --configuration production`). The deployment job runs on the stack only what it adds: the tests that need it (`*-stack.spec.ts`) and each screen's axe-core pass in one theme and language (light, French), which loads every screen behind nginx and its Content-Security-Policy. `just compose-e2e` alone still runs them all.

Who writes the shared layers: every run reads the development image and the build cache; only pushes to `develop`, `main`, `release/**` and `hotfix/**` publish them (`DEV_IMAGE_PUSH`, `MLBPP_BUILD_CACHE_WRITE`), so a pull request never changes what the long-lived branches build with. A pull request that changes the development Dockerfile builds its image itself. The release workflow uses neither: released images are always built from scratch.

Run a job alone with its recipe, e.g. `just ci-deploy`.
