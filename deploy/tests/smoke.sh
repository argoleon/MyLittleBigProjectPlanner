#!/usr/bin/env bash
# Smoke test of the production stack (L01-US06, L02-US02): every service
# healthy, routing and security headers through Traefik, sign-in and the
# reference endpoint end to end, a password reset email sent by the worker,
# and data and sessions kept across a full restart.
#
# Needs the local stack running: `just compose-up`.
source "$(dirname "$0")/lib.sh"

SERVICES=(traefik web api worker mcp postgres backup valkey seaweedfs clamav mailpit)

section "services"
expect_healthy "${SERVICES[@]}"
IFS='|' read -r _ state _ code <<< "$(service_states migrate)"
[ "$state" = "exited" ] && [ "$code" = "0" ] || fail "migrate: $state, exit code $code"
pass "migrate ran and exited 0"

section "container logs"
# Traefik's access logs and the api's hold client IP addresses (GDPR,
# specification §14): rotated by size, never kept whole on the host.
for service in traefik api; do
    log_config="$(docker inspect --format '{{.HostConfig.LogConfig.Type}} {{index .HostConfig.LogConfig.Config "max-size"}} {{index .HostConfig.LogConfig.Config "max-file"}}' "$(compose ps -q "$service")")"
    [ "$log_config" = "json-file 20m 5" ] || fail "$service: logs not rotated as configured: $log_config"
done
pass "container logs are rotated (20m, 5 files)"

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
curl_edge -H 'host: 127.0.0.1' https://localhost/ | grep -q '<app-root>' || fail "127.0.0.1 does not serve the application"
tenant="$(curl_edge -H 'host: 127.0.0.1' https://localhost/api/v1/tenant)"
grep -q '"slug":"default"' <<< "$tenant" || fail "127.0.0.1 does not reach the API: $tenant"
pass "the local stack also answers on 127.0.0.1"
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

section "sign-in"
[ "$(psql_admin -c "SELECT count(*) FROM tenant WHERE slug = 'default'")" = "1" ] \
    || fail "the migrate service did not bootstrap the default tenant"
pass "the migrate service bootstrapped the default tenant"
smoke_email="smoke-admin@example.test"
smoke_secret="smoke test passphrase"
printf '%s\n' "$smoke_secret" | compose exec -T api /usr/local/bin/api add-member default "$smoke_email" "Smoke Admin" --role tenant_admin > /dev/null \
    || fail "api add-member failed"
pass "api add-member created the first administrator"
expect_status 401 "the reference endpoint needs a session" https://localhost/api/v1/reference-items
signed_in="$(curl_edge --include -X POST https://localhost/api/v1/auth/sign-in \
    -H 'content-type: application/json' -d "{\"email\": \"$smoke_email\", \"password\": \"$smoke_secret\"}")"
grep -q '^HTTP/[0-9.]* 200' <<< "$signed_in" || fail "sign-in failed: $signed_in"
grep -qi '^set-cookie: mlbpp_session=.*HttpOnly' <<< "$signed_in" || fail "no HttpOnly session cookie: $signed_in"
grep -qi '^set-cookie: mlbpp_session=.*Secure' <<< "$signed_in" || fail "the session cookie is not Secure"
sid="$(grep -i '^set-cookie: mlbpp_session=' <<< "$signed_in" | sed -E 's/^[^=]*=([^;]*);.*/\1/')"
xsrf="$(grep -i '^set-cookie: mlbpp_csrf=' <<< "$signed_in" | sed -E 's/^[^=]*=([^;]*);.*/\1/')"
[ -n "$sid" ] && [ -n "$xsrf" ] || fail "missing cookies: $signed_in"
pass "signed in: HttpOnly, Secure session cookie and CSRF cookie"
# As a browser of the tenant: its session cookie, and its CSRF token on changes.
as_member() { curl_edge -H "cookie: mlbpp_session=$sid" -H "x-csrf-token: $xsrf" "$@"; }

section "platform super admin"
expect_status 403 "a tenant admin cannot manage tenants" -H "cookie: mlbpp_session=$sid" \
    https://localhost/api/v1/platform/tenants
root_secret="$(cat deploy/secrets/bootstrap_admin_password)"
root_in="$(curl_edge --include -X POST https://localhost/api/v1/auth/sign-in \
    -H 'content-type: application/json' -d "{\"email\": \"$BOOTSTRAP_ADMIN_EMAIL\", \"password\": \"$root_secret\"}")"
grep -q '^HTTP/[0-9.]* 200' <<< "$root_in" || fail "the bootstrapped super admin cannot sign in: $root_in"
root_sid="$(grep -i '^set-cookie: mlbpp_session=' <<< "$root_in" | sed -E 's/^[^=]*=([^;]*);.*/\1/')"
tenants="$(curl_edge -H "cookie: mlbpp_session=$root_sid" https://localhost/api/v1/platform/tenants)"
grep -q '"slug":"default"' <<< "$tenants" || fail "the super admin does not see the tenants: $tenants"
grep -q '"url":"https://localhost:18443"' <<< "$tenants" || fail "unexpected tenant address: $tenants"
pass "the bootstrapped super admin signs in and lists the tenants"

section "reference endpoint, end to end"
code="SMOKE-$(date +%s)"
expect_status 403 "a change without the CSRF token is refused" -X POST https://localhost/api/v1/reference-items \
    -H "cookie: mlbpp_session=$sid" -H 'content-type: application/json' \
    -d "{\"code\": \"$code\", \"labelTranslations\": {\"fr\": \"Fumée\", \"en\": \"Smoke\"}, \"quantity\": 1}"
created="$(as_member --include -X POST https://localhost/api/v1/reference-items \
    -H 'content-type: application/json' -H "idempotency-key: smoke-$code" -H 'accept-language: en' \
    -d "{\"code\": \"$code\", \"labelTranslations\": {\"fr\": \"Fumée\", \"en\": \"Smoke\"}, \"quantity\": 1}")"
grep -q '^HTTP/[0-9.]* 201' <<< "$created" || fail "creation failed: $created"
location="$(grep -i '^location:' <<< "$created" | tr -d '\r' | cut -d' ' -f2)"
pass "created $location"
read_back="$(as_member -H 'accept-language: fr' "https://localhost$location")"
if ! grep -q "\"code\":\"$code\"" <<< "$read_back" || ! grep -q '"label":"Fumée"' <<< "$read_back"; then
    fail "read back: $read_back"
fi
pass "read back in French"

section "password reset email, through the worker"
expect_status 202 "a password reset is accepted" -X POST https://localhost/api/v1/auth/password-reset \
    -H 'content-type: application/json' -d "{\"email\": \"$smoke_email\"}"
reset_email() {
    curl_backend "http://mailpit:8025/api/v1/message/latest" | grep -q 'https://localhost:18443/reset-password?token='
}
wait_until "the worker sent the reset link (caught by Mailpit)" 60 reset_email

section "logs and readiness"
request_id="smoke-$(date +%s)"
curl_edge --output /dev/null -H "x-request-id: $request_id" https://localhost/api/v1/ping
log_line() {
    compose logs --no-log-prefix --since 2m api | grep "\"request_id\":\"$request_id\"" | grep -q '"message":"finished processing request"'
}
wait_until "the api logs the request as a JSON line holding its x-request-id" 30 log_line

api_ready() { compose exec -T api /usr/local/bin/api healthcheck --ready; }
api_ready > /dev/null || fail "the api is not ready"
pass "the api is ready"
compose stop postgres > /dev/null 2>&1
if api_ready > /dev/null 2>&1; then
    fail "the api still reports ready without PostgreSQL"
fi
compose exec -T api /usr/local/bin/api healthcheck > /dev/null || fail "the api should stay alive without PostgreSQL"
pass "without PostgreSQL the api is alive but not ready (503)"
compose start postgres > /dev/null 2>&1
wait_until "the api is ready again once PostgreSQL is back" 90 api_ready

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
expect_status 200 "the session and the item survive the restart (Valkey, PostgreSQL)" \
    -H "cookie: mlbpp_session=$sid" "https://localhost$location"
# The tests recreate containers too: the stack must stay as `just compose-up`
# starts it.
curl_edge -H 'host: 127.0.0.1' https://localhost/ | grep -q '<app-root>' \
    || fail "127.0.0.1 no longer serves the application once the tests restart the stack"
pass "the restarted stack still answers on 127.0.0.1"

section "sign-out"
expect_status 204 "sign-out" -X POST -H "cookie: mlbpp_session=$sid" -H "x-csrf-token: $xsrf" \
    https://localhost/api/v1/auth/sign-out
expect_status 401 "the cookie no longer works after sign-out" -H "cookie: mlbpp_session=$sid" \
    https://localhost/api/v1/auth/session

echo "[$TEST_NAME] all passed"
