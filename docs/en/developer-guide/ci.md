# Continuous integration

Every CI step is a `just` recipe. GitHub Actions only prepares the runner (checkout, `just`, caches, registry login) and calls them, so the pipeline runs the same way on a laptop and stays portable to another CI.

```sh
just ci            # the whole pipeline, as GitHub Actions runs it
just ci-backend    # or one job at a time
```

## Jobs

`.github/workflows/ci.yml` runs on every pull request to `develop` and `main`, and on pushes to `develop`, `main`, `release/**` and `hotfix/**`. Its four jobs run in parallel:

| Job | Recipe | Checks |
| --- | --- | --- |
| Secrets (gitleaks) | `just ci-security` | No secret in the whole git history (`just gitleaks`), with a self-test proving that a committed private key fails the scan |
| Backend | `just ci-backend` | `cargo fmt`, clippy (warnings are errors), domain purity, `.sqlx` up to date, unit tests, integration tests on a real PostgreSQL with RLS (`just check`); licenses, RustSec advisories and crate sources with cargo-deny, plus a fixture proving that a GPL dependency is rejected (`just deny`) |
| Front end | `just ci-web` | ESLint (accessibility rules included) and stylelint, translation keys, OpenAPI contract and generated client up to date, npm licenses (with fixture tests), Vitest, Playwright with axe-core (`just web-check`); `npm audit` (`just web-audit`) |
| Deployment | `just ci-deploy` | shellcheck, actionlint and compose validation (`just deploy-lint`); production images: health, non-root, no secret, SBOM (`just images-test`); the compose stack: smoke, backups, scaling, profiles, Playwright on the production build (`just compose-test`) |

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

Publishing to GitHub Container Registry uses the workflow's own `GITHUB_TOKEN`; nothing to configure.

### Branch protection

Settings › Rules › Rulesets (or Settings › Branches), for `main` and `develop`:

- Require a pull request before merging, with at least one approval; dismiss stale approvals when new commits are pushed.
- Require these status checks to pass, and the branch to be up to date: `Secrets (gitleaks)`, `Backend (fmt, clippy, tests on PostgreSQL, cargo-deny)`, `Front end (lint, tests, licenses, audit, e2e and axe-core)`, `Deployment (images, SBOM, compose stack, e2e)`.
- Block force pushes and deletions.
- Allow merge commits (Gitflow keeps the history of each feature); `main` only receives `release/*` and `hotfix/*` branches.

Branch protection on a private repository needs a paid GitHub plan (Pro, Team or Enterprise); on a public repository it is free.

## Duration and caches

The development containers keep their build caches in Docker volumes. In CI, `CARGO_HOME_MOUNT`, `CARGO_TARGET_MOUNT` and `NPM_CACHE_MOUNT` turn them into directories of the runner, saved by `actions/cache` between runs and keyed on the lock files.

The deployment job is the longest: it builds the release images and starts the whole stack (ClamAV included). Run it alone with `just ci-deploy`.
