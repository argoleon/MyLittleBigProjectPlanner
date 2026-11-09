#!/usr/bin/env bash
# Optional profiles (L01-US06): "sso" adds Keycloak, "observability" adds
# Prometheus, Loki (fed by Alloy) and Grafana.
#
# Needs the local stack running: `just compose-up`. Stops the profile
# services at the end.
source "$(dirname "$0")/lib.sh"

PROFILES=(--profile sso --profile observability)
stop_profiles() {
    compose "${PROFILES[@]}" rm --stop --force keycloak keycloak-db-init prometheus loki alloy grafana > /dev/null 2>&1 || true
}
trap stop_profiles EXIT

section "start the sso and observability profiles"
compose "${PROFILES[@]}" up -d --wait --wait-timeout 600
expect_healthy keycloak prometheus
# Loki and Grafana images have no tool for a Docker health check: ask them.
wait_until "Loki is ready" 120 curl_edge --fail http://loki:3100/ready
wait_until "Grafana is ready" 120 curl_edge --fail http://grafana:3000/api/health

section "sso"
realm="$(curl_edge https://localhost/auth/realms/master)"
grep -q '"realm":"master"' <<< "$realm" || fail "Keycloak is not reachable at /auth: $realm"
pass "Keycloak answers at /auth through Traefik"

section "observability"
expect_status 200 "Grafana answers at /grafana through Traefik" https://localhost/grafana/api/health
targets() { curl_edge -G http://prometheus:9090/api/v1/query --data-urlencode 'query=up{job="traefik"}'; }
prometheus_scrapes_traefik() { targets | grep -q '"value":\[[0-9.]*,"1"\]'; }
wait_until "Prometheus scrapes Traefik" 60 prometheus_scrapes_traefik
loki_has_api_logs() {
    curl_edge http://loki:3100/loki/api/v1/label/service/values | grep -q '"api"'
}
wait_until "Loki receives the logs of the api service" 90 loki_has_api_logs

echo "[$TEST_NAME] all passed"
