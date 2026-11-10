#!/usr/bin/env bash
# Optional profiles (L01-US06, L01-US08): "sso" adds Keycloak;
# "observability" adds Prometheus, Loki, Tempo, Alloy and Grafana, and the
# services then export their traces and metrics.
#
# Needs the local stack running: `just compose-up`. Stops the profile
# services and turns the export off again at the end.
source "$(dirname "$0")/lib.sh"

PROFILES=(--profile sso --profile observability)
restore() {
    unset OTEL_EXPORTER_OTLP_ENDPOINT
    compose "${PROFILES[@]}" rm --stop --force keycloak keycloak-db-init prometheus loki tempo alloy grafana > /dev/null 2>&1 || true
    compose up -d --wait > /dev/null 2>&1 || true
}
trap restore EXIT

section "start the sso and observability profiles, services exporting telemetry"
export OTEL_EXPORTER_OTLP_ENDPOINT=http://alloy:4318
compose "${PROFILES[@]}" up -d --wait --wait-timeout 600
expect_healthy keycloak prometheus api worker mcp
# Loki, Tempo and Grafana images have no tool for a Docker health check: ask them.
wait_until "Loki is ready" 120 curl_edge --fail http://loki:3100/ready
wait_until "Tempo is ready" 120 curl_edge --fail http://tempo:3200/ready
wait_until "Grafana is ready" 120 curl_edge --fail http://grafana:3000/api/health

section "sso"
realm="$(curl_edge https://localhost/auth/realms/master)"
grep -q '"realm":"master"' <<< "$realm" || fail "Keycloak is not reachable at /auth: $realm"
pass "Keycloak answers at /auth through Traefik"

section "metrics"
expect_status 200 "Grafana answers at /grafana through Traefik" https://localhost/grafana/api/health
query() { curl_edge -G http://prometheus:9090/api/v1/query --data-urlencode "query=$1"; }
prometheus_scrapes_traefik() { query 'up{job="traefik"}' | grep -q '"value":\[[0-9.]*,"1"\]'; }
wait_until "Prometheus scrapes Traefik" 60 prometheus_scrapes_traefik
for _ in $(seq 1 10); do curl_edge --output /dev/null https://localhost/api/v1/ping; done
api_request_metric() {
    query 'http_server_request_duration_seconds_count{job="api",http_route="/api/v1/ping"}' | grep -q '"http_response_status_code":"200"'
}
wait_until "the api's request duration metric reaches Prometheus" 120 api_request_metric
pool_metric() { query 'db_client_connection_count{job="api"}' | grep -q '"db_client_connection_state"'; }
wait_until "the api's database pool metric reaches Prometheus" 120 pool_metric

section "traces"
trace_id="$(head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \n')"
curl_edge --output /dev/null -H "traceparent: 00-$trace_id-00f067aa0ba902b7-01" https://localhost/api/v1/ping
trace_in_tempo() { curl_edge --fail "http://tempo:3200/api/traces/$trace_id" | grep -q 'GET /api/v1/ping'; }
wait_until "the api's span of a request reaches Tempo, in the caller's trace" 120 trace_in_tempo

section "logs"
loki_has_api_logs() {
    curl_edge http://loki:3100/loki/api/v1/label/service/values | grep -q '"api"'
}
wait_until "Loki receives the logs of the api service" 90 loki_has_api_logs
loki_config="$(curl_edge http://loki:3100/config)"
retention="$(grep -E '^\s*retention_period:' <<< "$loki_config" | head -n 1 | tr -d ' ')"
[ "$retention" = "retention_period:6mo" ] || [ "$retention" = "retention_period:180d" ] || [ "$retention" = "retention_period:4320h" ] \
    || fail "Loki's log retention is not the configured 6 months: $retention"
pass "Loki keeps logs (client IPs included) for the configured retention ($retention)"

section "dashboard"
grafana_password="$(cat deploy/secrets/grafana_admin_password)"
dashboard="$(curl_edge -u "admin:$grafana_password" http://grafana:3000/api/dashboards/uid/mlbpp-services)"
grep -q '"title":"Services"' <<< "$dashboard" || fail "the Services dashboard is not provisioned"
pass "the Services dashboard is provisioned in Grafana"

echo "[$TEST_NAME] all passed"
