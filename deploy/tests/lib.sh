#!/usr/bin/env bash
# Helpers of the deployment tests (deploy/tests/*.sh), run from the
# repository root by `just compose-test` and `just images-test`.
#
# They drive the local production stack (compose.yaml + compose.local.yaml)
# with the docker CLI only: HTTP requests go through Traefik from a curl
# container, as a browser on the network would.
set -euo pipefail

export MLBPP_DOMAIN=localhost
export MLBPP_REGISTRY=mlbpp-local
export MLBPP_VERSION=local
export MLBPP_PUBLIC_PORT=:18443
# First platform super admin, bootstrapped by the migrate service; its
# password is in deploy/secrets/bootstrap_admin_password.
export BOOTSTRAP_ADMIN_EMAIL=root@example.test

CURL_IMAGE=curlimages/curl:8.17.0
TEST_NAME="$(basename "$0" .sh)"

compose() {
    docker compose -f deploy/compose.yaml -f deploy/compose.local.yaml "$@"
}

pass() { echo "  ok    $*"; }
fail() {
    echo "  FAIL  $*" >&2
    exit 1
}
section() { echo "[$TEST_NAME] $*"; }

# curl on the edge network: `https://localhost/…` reaches Traefik, with its
# self-signed certificate in the local stack.
curl_edge() {
    docker run --rm --network mlbpp_edge "$CURL_IMAGE" \
        --silent --show-error --insecure --max-time 20 \
        --connect-to localhost:443:traefik:443 --connect-to localhost:80:traefik:80 "$@"
}

# curl on the backend network (data stores, never exposed).
curl_backend() {
    docker run --rm --network mlbpp_backend "$CURL_IMAGE" --silent --show-error --max-time 20 "$@"
}

# HTTP status of a request through Traefik.
status_of() { curl_edge --output /dev/null --write-out '%{http_code}' "$@"; }

# expect_status EXPECTED DESCRIPTION CURL_ARGS…
expect_status() {
    local expected="$1" description="$2"
    shift 2
    local actual
    actual="$(status_of "$@")"
    [ "$actual" = "$expected" ] || fail "$description: expected HTTP $expected, got $actual"
    pass "$description ($actual)"
}

# One line per container of the service: "service|state|health|exit code".
service_states() {
    compose ps --all --format '{{.Service}}|{{.State}}|{{.Health}}|{{.ExitCode}}' "$@"
}

# expect_healthy SERVICE…: every container of each service is healthy.
expect_healthy() {
    local service
    for service in "$@"; do
        local lines
        lines="$(service_states "$service")"
        [ -n "$lines" ] || fail "$service: no container"
        local state health code
        while IFS='|' read -r _ state health code; do
            [ "$state" = "running" ] && [ "$health" = "healthy" ] || fail "$service: $state $health (exit $code)"
        done <<< "$lines"
        pass "$service healthy ($(wc -l <<< "$lines") container(s))"
    done
}

# wait_until DESCRIPTION SECONDS COMMAND…: retries COMMAND every 2 s.
wait_until() {
    local description="$1" timeout="$2"
    shift 2
    local deadline=$((SECONDS + timeout))
    until "$@" > /dev/null 2>&1; do
        [ "$SECONDS" -lt "$deadline" ] || fail "$description: not after ${timeout}s"
        sleep 2
    done
    pass "$description"
}

# SQL as the PostgreSQL superuser, in the application database.
psql_admin() {
    compose exec -T postgres psql -U postgres -d mlbpp -v ON_ERROR_STOP=1 -At "$@"
}
