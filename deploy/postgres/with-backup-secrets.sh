#!/usr/bin/env bash
# Entrypoint wrapper: reads the pgBackRest secrets from their files, then runs
# the given command (the PostgreSQL entrypoint, or the backup scheduler).
#
#   PGBACKREST_REPO1_CIPHER_PASS_FILE  repository encryption key (required)
#   PGBACKREST_REPO1_S3_KEY_FILE       S3 repository access key (optional)
#   PGBACKREST_REPO1_S3_KEY_SECRET_FILE
#
# pgBackRest reads PGBACKREST_* variables, not files: archive-push, run by
# PostgreSQL for every WAL segment, inherits them from this process.
set -euo pipefail

for name in PGBACKREST_REPO1_CIPHER_PASS PGBACKREST_REPO1_S3_KEY PGBACKREST_REPO1_S3_KEY_SECRET; do
    file_var="${name}_FILE"
    if [ -n "${!file_var:-}" ]; then
        export "$name"="$(< "${!file_var}")"
        unset "$file_var"
    fi
done

if [ -z "${PGBACKREST_REPO1_CIPHER_PASS:-}" ]; then
    echo "with-backup-secrets: PGBACKREST_REPO1_CIPHER_PASS_FILE is required" >&2
    exit 1
fi

exec "$@"
