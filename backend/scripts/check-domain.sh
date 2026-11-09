#!/usr/bin/env bash
# Fails if crates/domain depends on any crate: business rules stay free of I/O.
# Runs inside the backend dev container (`just check-domain`).
set -euo pipefail
cd "$(dirname "$0")/.."

deps=$(cargo tree -p domain --edges normal,build --prefix none --depth 1 --locked | tail -n +2 | sort -u)
if [ -n "$deps" ]; then
    echo "crates/domain must not depend on other crates; found:"
    echo "$deps"
    exit 1
fi
echo "crates/domain has no dependency"
