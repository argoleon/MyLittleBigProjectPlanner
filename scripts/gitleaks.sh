#!/usr/bin/env bash
# Secret scan of the repository (`just gitleaks`), in the gitleaks container:
#
# 1. checks that gitleaks still catches a committed secret: a throwaway
#    repository gets a commit holding a private key block made of random
#    bytes (generated on the fly, so nothing secret-looking is ever
#    committed here), and the scan must fail on it;
# 2. scans the whole git history of this repository, then the changes not
#    committed yet. False positives go to .gitleaksignore or get an inline
#    `gitleaks:allow` comment with the reason.
set -euo pipefail

IMAGE=zricethezav/gitleaks:v8.30.0
cd "$(dirname "$0")/.."

run() {
    docker run --rm --user "$(id -u):$(id -g)" -e HOME=/tmp -v "$PWD:/repo:ro" -w /repo "$@"
}

echo "gitleaks: self-test on a committed private key"
# The script runs in the gitleaks container: nothing expands on the host.
# shellcheck disable=SC2016
run --entrypoint sh "$IMAGE" -c '
set -eu
cd "$(mktemp -d)"
git init -q
git config user.email ci@example.org
git config user.name ci
{
    echo "-----BEGIN PRIVATE KEY-----" # gitleaks:allow (self-test: random bytes, generated below)
    head -c 600 /dev/urandom | base64 -w 64
    echo "-----END PRIVATE KEY-----" # gitleaks:allow
} > server.pem
git add server.pem
git commit -qm "add server certificate"
if gitleaks git . --no-banner --log-level error > /dev/null; then
    echo "gitleaks did not catch a committed private key" >&2
    exit 1
fi
'
echo "gitleaks: a committed private key fails the scan, as expected"

echo "gitleaks: git history"
run "$IMAGE" git /repo --redact --no-banner --log-level warn

# Changes not committed yet (git-ignored files, such as deploy/secrets/,
# are left out): catches a secret before it reaches the history.
echo "gitleaks: staged and unstaged changes"
run "$IMAGE" git /repo --staged --redact --no-banner --log-level warn
run "$IMAGE" git /repo --pre-commit --redact --no-banner --log-level warn
