# Deployment

The whole stack runs with Docker Compose on a single machine (specification §17). `api` and `worker` scale horizontally behind Traefik.

| File | Use |
| --- | --- |
| `compose.yaml` | Production stack, with the optional profiles `sso` and `observability` |
| `compose.local.yaml` | Override running the same stack on a development machine (images built from the sources, self-signed certificate) |
| `compose.dev.yaml` | Development environment: toolchain containers, live reload (`just dev`); see `CLAUDE.md` |
| `docker/*.Dockerfile` | Production images |
| `.env.example` | Every variable, of the stack and of the services |

## Services

| Service | Image | Role | Instances |
| --- | --- | --- | --- |
| `traefik` | `traefik:v3.7` | Reverse proxy, TLS (Let's Encrypt), load balancing, access log | 1 |
| `web` | `mlbpp-web` | Both Angular applications, served by nginx: `/` (internal application), `/portal/` | 1 or more |
| `api` | `mlbpp-api` | REST API, under `/api` | `--scale api=N` |
| `worker` | `mlbpp-worker` | Background jobs (no public route) | `--scale worker=N` |
| `mcp` | `mlbpp-mcp` | MCP server, under `/mcp` (tools come with L16) | 1 or more |
| `migrate` | `mlbpp-api` | Applies the database migrations, then exits; `api` and `worker` wait for it | one-shot |
| `postgres` | `mlbpp-postgres` | PostgreSQL 18 with pgBackRest; archives every WAL segment | 1 |
| `backup` | `mlbpp-postgres` | Scheduled pgBackRest backups | 1 |
| `valkey` | `valkey/valkey:9.2-alpine` | Cache, sessions, pub/sub (used from L02) | 1 |
| `seaweedfs` | `chrislusf/seaweedfs:4.05` | S3 storage for attachments (used from L05) | 1 |
| `clamav` | `clamav/clamav:1.5` | Antivirus of attachments (used from L05); downloads its signatures on first start | 1 |

Profile `sso`: `keycloak` (under `/auth`) and `keycloak-db-init`, which creates its database in the shared PostgreSQL.
Profile `observability`: `alloy` (ships every container's logs to Loki; receives the services' traces and metrics), `loki` (logs), `tempo` (traces), `prometheus` (metrics of the services and of Traefik), `grafana` (under `/grafana`, data sources and the "Services" dashboard provisioned). See "Observability" below.

Our images (`api`, `worker`, `mcp`: distroless; `web`: nginx-unprivileged) run as non-root, hold no secret, and check their own health with a Docker `HEALTHCHECK`. Each one gets an SBOM (`just images-test` writes them to `deploy/build/sbom/`; the Rust ones list every crate thanks to `cargo auditable`).

## Install on a server

Requirements: Docker Engine with Compose v2.24 or later, ports 80 and 443 open, a DNS record for the domain.

1. **Variables.** `cp deploy/.env.example deploy/.env`, then set at least `MLBPP_DOMAIN`, `ACME_EMAIL` and `MAIL_FROM` (sender of the emails). In multi-tenant mode, also set `PUBLIC_URL=https://{tenant}.<domain>`: email links point to it. Pin `MLBPP_VERSION` to a release (`v0.1.0`) rather than `latest`.
2. **Secrets.** `deploy/scripts/generate-secrets.sh` creates every file of `deploy/secrets/` with random values (it never overwrites an existing one). To use your own values, write the files yourself first:

   | File | Content |
   | --- | --- |
   | `postgres_password` | PostgreSQL superuser password |
   | `mlbpp_owner_password`, `mlbpp_app_password` | Passwords of the owner role (migrations) and of the runtime role |
   | `database_url`, `database_migration_url` | `postgres://mlbpp_app:<password>@postgres:5432/mlbpp`, and the same with `mlbpp_owner` |
   | `pgbackrest_cipher_pass` | Encryption key of the backups. **Keep a copy off the server**: without it, no restore is possible |
   | `valkey_password`, `valkey_url` | Valkey password, and `redis://:<password>@valkey:6379` for the services |
   | `seaweedfs_s3_config` | SeaweedFS S3 identities (JSON) |
   | `keycloak_db_password`, `keycloak_admin_password` | Profile `sso` |
   | `grafana_admin_password` | Profile `observability` (user `admin`) |
   | `smtp_url` | SMTP server of the emails (password resets): `smtp://host:587?tls=required` (STARTTLS) or `smtps://host:465`, with the account in the URL if needed. **The script writes a placeholder: replace it.** Until then, emails stay in the job queue and the worker logs the failures |

   Secrets are mounted as files (`/run/secrets/…`) and read through the `*_FILE` variables; none appears in the compose file, in the images, or in `docker inspect`.
3. **Start.** `docker compose -f deploy/compose.yaml up -d --wait`. The first start takes a few minutes (ClamAV signatures).
4. **First tenant.** Tenant administration comes with L02-US01. Until then, create it in SQL:

   ```sh
   docker compose -f deploy/compose.yaml exec postgres psql -U postgres -d mlbpp \
     -c "INSERT INTO tenant (id, slug, name) VALUES (uuidv7(), 'default', 'My organization')"
   ```
5. **First administrator.** `api add-member` adds an internal member to a tenant; it reads the password (12 characters or more) from standard input, so it never appears in the shell history or the process list:

   ```sh
   docker compose -f deploy/compose.yaml exec -T api /usr/local/bin/api \
     add-member default admin@example.org "Alice Martin" --role tenant_admin
   ```

   Type the password, then Enter. An email that already has an account in another tenant is linked to it and keeps its password.

Optional profiles: add `--profile sso` and/or `--profile observability` to every command.

## Sign-in and sessions

Members sign in with their email and password (Argon2id hashes). Sessions live in Valkey: any api instance serves them, and they survive a restart. A session ends after `SESSION_IDLE_MINUTES` without use (8 hours by default) and `SESSION_MAX_AGE_HOURS` after sign-in at the latest (7 days); signing out or resetting the password ends it at once. Its cookie is `HttpOnly`, `Secure` and `SameSite=Lax`, scoped to the tenant's host name.

After 5 failed sign-ins on an email, or 30 from one client address, sign-in is refused for 15 minutes. The client address is the last entry of `X-Forwarded-For`, which Traefik sets: do not expose the api without it.

## Scaling

```sh
docker compose -f deploy/compose.yaml up -d --scale api=3 --scale worker=2
```

Traefik discovers the new instances, checks their health (`/health/live`) and spreads the requests across them. The services are stateless; the worker queue relies on `SKIP LOCKED` (L02), so any number of workers can run.

## TLS

Traefik gets a Let's Encrypt certificate for `MLBPP_DOMAIN` (TLS-ALPN challenge on port 443). HTTP redirects to HTTPS.

In multi-tenant mode (`TENANCY_MODE=multi`, one subdomain per tenant), the routes also answer on `*.MLBPP_DOMAIN`, but the TLS challenge cannot issue a wildcard certificate: configure a DNS challenge for your DNS provider in Traefik, or provide your own wildcard certificate. `MLBPP_HOST_RULE` replaces the host rule of every route if needed.

## Backups (pgBackRest)

- **WAL archiving**: PostgreSQL archives each WAL segment to the pgBackRest repository, at least every `POSTGRES_ARCHIVE_TIMEOUT` seconds (default 300): the data loss is bounded to minutes (target: 15 minutes, specification §19).
- **Scheduled backups**: the `backup` service takes a full backup on `BACKUP_FULL_DAY` (default 7, Sunday) and an incremental backup the other days, at `BACKUP_TIME` (UTC, default 02:00). A first full backup is taken as soon as the stack starts.
- **Retention**: `BACKUP_RETENTION_FULL` full backups (default 4, so about a month of point-in-time recovery); older backups and their WAL are expired after each backup.
- **Repository**: encrypted (AES-256, key `pgbackrest_cipher_pass`), compressed (zstd), on the `pgbackrest-repo` volume. To keep backups off the machine, use an S3 repository: set `PGBACKREST_REPO1_TYPE=s3`, `PGBACKREST_REPO1_S3_BUCKET`, `PGBACKREST_REPO1_S3_ENDPOINT`, `PGBACKREST_REPO1_S3_REGION`, `PGBACKREST_REPO1_PATH` on both `postgres` and `backup`, and provide the keys as secret files through `PGBACKREST_REPO1_S3_KEY_FILE` and `PGBACKREST_REPO1_S3_KEY_SECRET_FILE`.
- **Checking**: `docker compose -f deploy/compose.yaml exec backup with-backup-secrets.sh pgbackrest --stanza=main info`. The `backup` service turns unhealthy when a scheduled backup fails.

The restore procedure and its timed drill come with L17. Back up the files of SeaweedFS separately (replication or volume snapshots).

## Persistent data

| Volume | Content |
| --- | --- |
| `postgres-data` | Database |
| `pgbackrest-repo` | Backups and archived WAL |
| `seaweedfs-data` | Files |
| `valkey-data` | Valkey append-only file |
| `clamav-db` | Antivirus signatures |
| `letsencrypt` | Certificates |
| `prometheus-data`, `loki-data`, `tempo-data`, `alloy-data`, `grafana-data` | Profile `observability` |

`docker compose down` keeps them; only `down --volumes` deletes them.

## Observability

### Health

Every service answers two endpoints, never exposed through Traefik:

| Endpoint | Answers 200 when | Used by |
| --- | --- | --- |
| `/health/live` | the process runs | the image's own `HEALTHCHECK` (`api healthcheck`), later Kubernetes liveness probes |
| `/health/ready` | its dependencies are reachable (api and worker: PostgreSQL, Valkey, S3; mcp: the API); 503 otherwise, with the state of each one | the compose health checks (`healthcheck --ready`), Traefik's choice of instances, later Kubernetes readiness probes |

`docker compose ps` therefore shows `api` unhealthy while PostgreSQL is down, and Traefik sends no request to an instance that cannot serve it.

### Logs

The services write one JSON object per line on standard output. Each request yields one INFO line with its status and duration, and every line of a request carries its span: `request_id` (the `x-request-id` correlation ID, also returned in the response), `method`, `route`, `tenant` and `client_ip` (from Traefik's `X-Forwarded-For`).

These technical logs hold client IP addresses (GDPR, specification §14):

- on the host, Docker rotates each container's logs by size (`LOG_MAX_SIZE`, `LOG_MAX_FILES`);
- with the `observability` profile, Loki keeps them `LOG_RETENTION` (default 6 months, in line with the CNIL recommendation), then deletes them;
- Traefik's access log also holds them and follows the same path.

### Traces and metrics

The services export OpenTelemetry traces and metrics when `OTEL_EXPORTER_OTLP_ENDPOINT` is set. To run the profile, add `OTEL_EXPORTER_OTLP_ENDPOINT=http://alloy:4318` to the stack variables, then start it with `--profile observability`.

- **Traces** (Tempo, kept `TRACE_RETENTION`, 14 days by default): one server span per request, named after its route (`GET /api/v1/reference-items/{id}`). A W3C `traceparent` header from the caller continues its trace.
- **Metrics** (Prometheus): `http_server_request_duration_seconds` (histogram by method, route and status: rate, latency percentiles, errors) and `db_client_connection_count` (PostgreSQL pool, used and idle), with the service as `job`; Traefik's own metrics too.
- **Dashboard**: Grafana › Dashboards › My Little Big Project Planner › Services (`deploy/observability/dashboards/mlbpp-services.json`), with the latency targets of the specification.

To send telemetry to another backend, point Alloy at it (`deploy/observability/config.alloy`): the services only talk to the collector, in plain HTTP inside the network.

## Running the stack locally

```sh
just compose-up        # builds the images, starts the stack: https://localhost:18443
just compose-test      # smoke, backup, scaling and profile tests
just images-test       # image tests and SBOMs
just compose-down      # stops it (data kept); just compose-reset deletes the data
```

Emails are caught by Mailpit instead of being sent: read them on http://localhost:18026.

The browser shows a certificate warning: Traefik's default certificate is self-signed.
