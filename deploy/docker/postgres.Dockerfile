# syntax=docker/dockerfile:1
#
# PostgreSQL 18 with pgBackRest (MIT), for the production compose file:
#
# - the `postgres` service archives every WAL segment to the pgBackRest
#   repository (point-in-time recovery);
# - the `backup` service, same image, takes the scheduled full and
#   incremental backups (deploy/postgres/backup-scheduler.sh).
#
# The development environment (compose.dev.yaml) keeps the plain image.

ARG POSTGRES_IMAGE=postgres:18

FROM ${POSTGRES_IMAGE}
LABEL org.opencontainers.image.title="mlbpp-postgres" \
      org.opencontainers.image.description="PostgreSQL 18 with pgBackRest for My Little Big Project Planner" \
      org.opencontainers.image.source="https://github.com/argoleon/MyLittleBigProjectPlanner"
RUN apt-get update \
    && apt-get install -y --no-install-recommends pgbackrest \
    && rm -rf /var/lib/apt/lists/* \
    # Debian's self-signed "snakeoil" key: unused (TLS is off on the internal
    # network), and no private key ships in our images.
    && rm -f /etc/ssl/private/ssl-cert-snakeoil.key /etc/ssl/certs/ssl-cert-snakeoil.pem \
    && install -d -o postgres -g postgres -m 0750 /var/lib/pgbackrest /var/log/pgbackrest /var/spool/pgbackrest /tmp/pgbackrest
COPY deploy/postgres/pgbackrest.conf /etc/pgbackrest/pgbackrest.conf
COPY deploy/postgres/init-roles.sh /docker-entrypoint-initdb.d/10-init-roles.sh
COPY deploy/postgres/with-backup-secrets.sh deploy/postgres/backup-scheduler.sh /usr/local/bin/
ENTRYPOINT ["with-backup-secrets.sh", "docker-entrypoint.sh"]
CMD ["postgres"]
