#!/usr/bin/env bash
# End-to-end tests (Playwright, web/e2e/) against the local production stack:
# nginx and its Content-Security-Policy behind Traefik, from the e2e
# container on the edge network. Run by `just compose-e2e`; extra arguments
# go to `playwright test`.
#
# Creates a member of its own for the flow that mocks nothing
# (web/e2e/auth-stack.spec.ts), which reads the reset email from Mailpit.
source "$(dirname "$0")/lib.sh"

section "member for the end-to-end flow"
member="e2e-$(date +%s)-$RANDOM@example.test"
passphrase="e2e passphrase $RANDOM$RANDOM"
printf '%s\n' "$passphrase" | compose exec -T api /usr/local/bin/api add-member default "$member" "E2E Member" > /dev/null \
    || fail "api add-member failed"
pass "created $member"
# Another one for two-step verification (web/e2e/mfa-stack.spec.ts).
mfa_member="e2e-mfa-$(date +%s)-$RANDOM@example.test"
mfa_passphrase="e2e mfa passphrase $RANDOM$RANDOM"
printf '%s\n' "$mfa_passphrase" | compose exec -T api /usr/local/bin/api add-member default "$mfa_member" "E2E Mfa" > /dev/null \
    || fail "api add-member failed"
pass "created $mfa_member"

section "playwright"
docker run --rm --network mlbpp_edge --ipc host --user "${DEV_UID:-$(id -u)}:${DEV_GID:-$(id -g)}" \
    -v "$PWD:/workspace" -w /workspace/web \
    -e E2E_BASE_URL=https://localhost -e "E2E_HOST_RULES=MAP localhost traefik" \
    -e "E2E_MEMBER_EMAIL=$member" -e "E2E_MEMBER_SECRET=$passphrase" -e E2E_MAILPIT_URL=http://mailpit:8025 \
    -e "E2E_MFA_EMAIL=$mfa_member" -e "E2E_MFA_SECRET=$mfa_passphrase" \
    mlbpp-dev-e2e npx playwright test "$@"
