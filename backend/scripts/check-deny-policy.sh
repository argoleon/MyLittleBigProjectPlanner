#!/usr/bin/env bash
# Checks that the license policy of deny.toml rejects GPL code: runs
# cargo-deny on deny-fixture/, an MIT crate depending on a GPL one, and
# expects a rejection. Runs inside the backend dev container (`just deny`).
set -euo pipefail
cd "$(dirname "$0")/.."

if output=$(cargo deny --manifest-path deny-fixture/Cargo.toml --config deny.toml check licenses 2>&1); then
    echo "deny.toml accepts a GPL dependency: the license policy is broken" >&2
    exit 1
fi
if ! grep -q 'GPL-3.0' <<< "$output"; then
    echo "cargo-deny failed on the fixture, but not on its GPL license:" >&2
    echo "$output" >&2
    exit 1
fi
echo "License policy OK: a GPL dependency is rejected"
