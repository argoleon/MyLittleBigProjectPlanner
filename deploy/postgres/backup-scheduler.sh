#!/usr/bin/env bash
# Scheduled pgBackRest backups, run by the `backup` service.
#
# Every day at BACKUP_TIME (HH:MM, UTC): a full backup on BACKUP_FULL_DAY
# (1 = Monday … 7 = Sunday), an incremental backup the other days.
# Point-in-time recovery between backups relies on WAL archiving, done by
# PostgreSQL itself (archive_command); archive_timeout bounds the data loss.
#
# The stanza is created on start if needed; a first full backup is taken
# right away when the repository holds none.
set -euo pipefail

BACKUP_TIME="${BACKUP_TIME:-02:00}"
BACKUP_FULL_DAY="${BACKUP_FULL_DAY:-7}"
STANZA="${PGBACKREST_STANZA:-main}"
READY_FILE=/tmp/backup-ready

log() { echo "backup-scheduler: $*"; }

if ! [[ "$BACKUP_TIME" =~ ^([01][0-9]|2[0-3]):[0-5][0-9]$ ]]; then
    log "BACKUP_TIME must be HH:MM (UTC), got '$BACKUP_TIME'" >&2
    exit 1
fi
if ! [[ "$BACKUP_FULL_DAY" =~ ^[1-7]$ ]]; then
    log "BACKUP_FULL_DAY must be 1 (Monday) to 7 (Sunday), got '$BACKUP_FULL_DAY'" >&2
    exit 1
fi

until pg_isready -q -h /var/run/postgresql; do
    log "waiting for PostgreSQL"
    sleep 2
done

pgbackrest --stanza="$STANZA" stanza-create
pgbackrest --stanza="$STANZA" check
if ! pgbackrest --stanza="$STANZA" info --output=json | grep -q '"label"'; then
    log "no backup yet: taking a first full backup"
    pgbackrest --stanza="$STANZA" --type=full backup
fi
touch "$READY_FILE"
log "ready; backups every day at $BACKUP_TIME UTC, full on day $BACKUP_FULL_DAY"

# Seconds until the next BACKUP_TIME (UTC).
seconds_until_next_run() {
    local now next
    now=$(date -u +%s)
    next=$(date -u -d "today $BACKUP_TIME" +%s)
    if [ "$next" -le "$now" ]; then
        next=$(date -u -d "tomorrow $BACKUP_TIME" +%s)
    fi
    echo $((next - now))
}

while true; do
    sleep "$(seconds_until_next_run)"
    if [ "$(date -u +%u)" = "$BACKUP_FULL_DAY" ]; then type=full; else type=incr; fi
    log "starting $type backup"
    if pgbackrest --stanza="$STANZA" --type="$type" backup; then
        pgbackrest --stanza="$STANZA" expire || log "expire failed"
        touch "$READY_FILE"
    else
        # The health check turns red: the next attempt is tomorrow.
        rm -f "$READY_FILE"
        log "$type backup FAILED" >&2
    fi
done
