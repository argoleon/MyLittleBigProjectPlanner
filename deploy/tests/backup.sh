#!/usr/bin/env bash
# Backup test (L01-US06): WAL archiving works, the scheduler took its first
# full backup, an incremental backup runs on demand, and both are listed.
# The full restore drill comes with L17.
#
# Needs the local stack running: `just compose-up`.
source "$(dirname "$0")/lib.sh"

# pgBackRest in the backup service, with its secrets read from their files.
pgbackrest() { compose exec -T backup with-backup-secrets.sh pgbackrest --stanza=main "$@"; }

section "configuration"
expect_healthy backup
pgbackrest check > /dev/null || fail "pgbackrest check failed (WAL archiving)"
pass "WAL archiving and the repository are working"
env_dump="$(docker inspect --format '{{range .Config.Env}}{{println .}}{{end}}' "$(compose ps -q backup)")"
grep -q '^PGBACKREST_REPO1_CIPHER_PASS_FILE=/run/secrets/' <<< "$env_dump" || fail "the cipher key is not read from a secret file"
! grep -q '^PGBACKREST_REPO1_CIPHER_PASS=' <<< "$env_dump" || fail "the cipher key is in the container configuration"
pass "the repository key comes from a secret file only"

section "backups"
info="$(pgbackrest info --output=json)"
grep -q '"type":"full"' <<< "$info" || fail "no full backup: $info"
pass "the scheduler took a full backup"
pgbackrest --type=incr backup > /dev/null || fail "incremental backup failed"
info="$(pgbackrest info --output=json)"
grep -q '"type":"incr"' <<< "$info" || fail "no incremental backup listed: $info"
pass "an incremental backup runs and is listed"
grep -q '"cipher":"aes-256-cbc"' <<< "$info" || fail "the repository is not encrypted: $info"
pass "the repository is encrypted"
pgbackrest info | sed 's/^/        /'

echo "[$TEST_NAME] all passed"
