#!/usr/bin/env bash
# Smoke test of the production stack (L01-US06): every service healthy,
# routing and security headers through Traefik, the reference endpoint end
# to end, and data kept on the volumes across a full restart.
#
# Needs the local stack running: `just compose-up`.
source "$(dirname "$0")/lib.sh"

SERVICES=(traefik web api worker mcp postgres backup valkey seaweedfs clamav)

section "services"
expect_healthy "${SERVICES[@]}"
IFS='|' read -r _ state _ code <<< "$(service_states migrate)"
[ "$state" = "exited" ] && [ "$code" = "0" ] || fail "migrate: $state, exit code $code"
pass "migrate ran and exited 0"

section "routing through Traefik"
redirect="$(curl_edge --output /dev/null --write-out '%{http_code} %{redirect_url}' http://localhost/workload)"
[[ "$redirect" =~ ^30[178]\ https://localhost/workload$ ]] || fail "http is not redirected to https: $redirect"
pass "http redirects to https ($redirect)"

page="$(curl_edge --include https://localhost/)"
grep -q '<app-root>' <<< "$page" || fail "/ does not serve the internal application"
grep -qi "^content-security-policy: default-src 'self'" <<< "$page" || fail "/ has no Content-Security-Policy"
grep -qi '^x-content-type-options: nosniff' <<< "$page" || fail "/ has no X-Content-Type-Options"
pass "/ serves the internal application with its security headers"
curl_edge https://localhost/workload | grep -q '<app-root>' || fail "deep links do not fall back on index.html"
pass "deep links fall back on the application"
curl_edge https://localhost/portal/ | grep -q '<portal-root>' || fail "/portal/ does not serve the portal"
pass "/portal/ serves the request portal"
expect_status 200 "translations are served" https://localhost/i18n/fr.json

body="$(curl_edge https://localhost/api/v1/ping)"
grep -q '"status":"ok"' <<< "$body" || fail "/api/v1/ping: $body"
pass "/api/v1/ping reaches the api"
expect_status 200 "the OpenAPI contract is published" https://localhost/api/v1/openapi.json
expect_status 200 "the API documentation is published" https://localhost/api/v1/docs/
# The mcp service answers /mcp itself (404 until L16), not the web fallback.
mcp="$(curl_edge --include https://localhost/mcp)"
if ! grep -q '^HTTP/[0-9.]* 404' <<< "$mcp" || grep -q '<app-root>' <<< "$mcp"; then
    fail "/mcp is not routed to the mcp service"
fi
pass "/mcp is routed to the mcp service"
# Health probes are internal: /health/live is just a path of the application.
! curl_edge https://localhost/health/live | grep -q '"status":"ok"' || fail "a health endpoint is exposed publicly"
pass "health endpoints are not exposed publicly"

section "reference endpoint, end to end"
ensure_default_tenant
code="SMOKE-$(date +%s)"
created="$(curl_edge --include -X POST https://localhost/api/v1/reference-items \
    -H 'content-type: application/json' -H "idempotency-key: smoke-$code" -H 'accept-language: en' \
    -d "{\"code\": \"$code\", \"labelTranslations\": {\"fr\": \"Fumée\", \"en\": \"Smoke\"}, \"quantity\": 1}")"
grep -q '^HTTP/[0-9.]* 201' <<< "$created" || fail "creation failed: $created"
location="$(grep -i '^location:' <<< "$created" | tr -d '\r' | cut -d' ' -f2)"
pass "created $location"
read_back="$(curl_edge -H 'accept-language: fr' "https://localhost$location")"
if ! grep -q "\"code\":\"$code\"" <<< "$read_back" || ! grep -q '"label":"Fumée"' <<< "$read_back"; then
    fail "read back: $read_back"
fi
pass "read back in French"

section "data survives a full restart"
curl_backend -F "file=@/etc/hostname" "http://seaweedfs:8888/smoke/$code.txt" > /dev/null
stored="$(curl_backend "http://seaweedfs:8888/smoke/$code.txt")"
[ -n "$stored" ] || fail "nothing stored in SeaweedFS"
pass "file stored in SeaweedFS"

compose down
compose up -d --wait --wait-timeout 600
expect_healthy postgres seaweedfs api
[ "$(psql_admin -c "SELECT count(*) FROM reference_item WHERE code = '$code'")" = "1" ] || fail "the reference item is gone"
pass "PostgreSQL kept the reference item"
[ "$(curl_backend "http://seaweedfs:8888/smoke/$code.txt")" = "$stored" ] || fail "the SeaweedFS file is gone"
pass "SeaweedFS kept the file"
expect_status 200 "the item is still served through Traefik" "https://localhost$location"

echo "[$TEST_NAME] all passed"
