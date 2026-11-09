#!/usr/bin/env bash
# Creates the application database and its two roles on first start of the
# PostgreSQL container (docker-entrypoint-initdb.d). See backend/migrations/README.md.
#
#   mlbpp_owner  owns the database and runs migrations
#   mlbpp_app    runtime role: not owner, no BYPASSRLS, so row-level security always applies
#
# Required environment: MLBPP_DB_NAME, MLBPP_OWNER_PASSWORD, MLBPP_APP_PASSWORD.
set -euo pipefail

: "${MLBPP_DB_NAME:?}" "${MLBPP_OWNER_PASSWORD:?}" "${MLBPP_APP_PASSWORD:?}"

psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" \
    -v db="$MLBPP_DB_NAME" -v owner_pw="$MLBPP_OWNER_PASSWORD" -v app_pw="$MLBPP_APP_PASSWORD" <<'SQL'
CREATE ROLE mlbpp_owner LOGIN PASSWORD :'owner_pw';
CREATE ROLE mlbpp_app LOGIN PASSWORD :'app_pw' NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS;
CREATE DATABASE :"db" OWNER mlbpp_owner;
REVOKE ALL ON DATABASE :"db" FROM PUBLIC;
GRANT CONNECT, TEMPORARY ON DATABASE :"db" TO mlbpp_app;
SQL
