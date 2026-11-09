#!/usr/bin/env bash
# Scaling test (L01-US06): with 3 api and 2 worker instances, Traefik
# discovers every api instance and spreads requests across them.
#
# Needs the local stack running: `just compose-up`. Scales back to one
# instance of each at the end.
source "$(dirname "$0")/lib.sh"

restore() { compose up -d --wait --scale api=1 --scale worker=1 > /dev/null 2>&1 || true; }
trap restore EXIT

section "scale to 3 api and 2 worker instances"
compose up -d --wait --wait-timeout 300 --scale api=3 --scale worker=2
[ "$(service_states api | grep -c '|running|healthy|')" = "3" ] || fail "api: $(service_states api)"
pass "3 api instances healthy"
[ "$(service_states worker | grep -c '|running|healthy|')" = "2" ] || fail "worker: $(service_states worker)"
pass "2 worker instances healthy"

section "Traefik balances requests"
since="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
# Traefik checks the health of new instances before sending them traffic.
sleep 12
for _ in $(seq 1 30); do
    [ "$(status_of https://localhost/api/v1/ping)" = "200" ] || fail "a request failed while scaled"
done
pass "30 requests answered"

# The JSON access log names the instance that served each request.
instances="$(compose logs --no-log-prefix --since "$since" traefik \
    | grep '"ServiceName":"mlbpp-api@docker"' \
    | grep -o '"ServiceAddr":"[^"]*"' | sort -u || true)"
count="$(grep -c . <<< "$instances" || true)"
[ "$count" = "3" ] || fail "requests reached $count api instance(s): $instances"
pass "requests reached the 3 api instances"

echo "[$TEST_NAME] all passed"
