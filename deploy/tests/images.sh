#!/usr/bin/env bash
# Tests of the production images (L01-US06-T01): each one starts alone and
# reports healthy through its /health/live endpoint, runs as non-root,
# carries no secret, and gets an SBOM listing its dependencies.
#
# Needs the images: `just images-build`. SBOMs are written to deploy/build/sbom/.
source "$(dirname "$0")/lib.sh"

SYFT_IMAGE=anchore/syft:v1.40.0
GITLEAKS_IMAGE=zricethezav/gitleaks:v8.30.0
BUILD_DIR=deploy/build
image() { echo "$MLBPP_REGISTRY/mlbpp-$1:$MLBPP_VERSION"; }

cleanup() { docker rm -f mlbpp-image-test > /dev/null 2>&1 || true; }
trap cleanup EXIT

# wait_healthy CONTAINER: waits for the HEALTHCHECK of the image to pass.
wait_healthy() {
    local deadline=$((SECONDS + 60)) state
    while true; do
        state="$(docker inspect --format '{{.State.Health.Status}}' "$1")"
        [ "$state" = "healthy" ] && return 0
        [ "$SECONDS" -lt "$deadline" ] || return 1
        sleep 1
    done
}

section "each image starts alone and reports healthy"
# The required settings, pointing nowhere: services connect lazily.
declare -A RUN_ARGS=(
    [api]="-e DATABASE_URL=postgres://nobody@127.0.0.1:1/none -e VALKEY_URL=redis://127.0.0.1:1 -e PUBLIC_URL=https://planner.example.test"
    [worker]="-e DATABASE_URL=postgres://nobody@127.0.0.1:1/none -e SMTP_URL=smtp://127.0.0.1:1 -e MAIL_FROM=noreply@example.test"
    [mcp]="-e MCP_API_BASE_URL=http://127.0.0.1:1"
    [web]=""
)
for name in api worker mcp web; do
    cleanup
    # shellcheck disable=SC2086
    docker run -d --name mlbpp-image-test ${RUN_ARGS[$name]} "$(image "$name")" > /dev/null
    wait_healthy mlbpp-image-test || fail "$name: not healthy: $(docker logs mlbpp-image-test 2>&1 | tail -5)"
    pass "$name: healthy (its HEALTHCHECK probes /health/live)"
done
cleanup

section "application images run as non-root"
for name in api worker mcp web; do
    user="$(docker image inspect --format '{{.Config.User}}' "$(image "$name")")"
    case "$user" in
        "" | root | 0 | root:* | 0:*) fail "$name runs as '${user:-root}'" ;;
    esac
    pass "$name runs as $user"
done

section "images carry no secret"
mkdir -p "$BUILD_DIR/fs"
for name in api worker mcp web postgres; do
    config="$(docker image inspect --format '{{range .Config.Env}}{{println .}}{{end}}' "$(image "$name")")"
    if grep -Eiq '^[A-Z_]*(PASSWORD|SECRET|TOKEN|CIPHER_PASS)[A-Z_]*=' <<< "$config"; then
        fail "$name declares a secret in its environment"
    fi
    # Scan the whole file system of the image with gitleaks.
    dir="$BUILD_DIR/fs/$name"
    rm -rf "$dir" && mkdir -p "$dir"
    container="$(docker create "$(image "$name")")"
    docker export "$container" | tar -x -C "$dir" --exclude='dev/*' 2> /dev/null || true
    docker rm "$container" > /dev/null
    docker run --rm -v "$PWD/$dir:/scan:ro" -v "$PWD/deploy/tests/gitleaks-images.toml:/config.toml:ro" \
        "$GITLEAKS_IMAGE" dir /scan --config /config.toml --no-banner --redact --log-level error \
        || fail "$name: gitleaks found secrets"
    rm -rf "$dir"
    pass "$name: no secret in its environment nor its files"
done

section "SBOM of each image"
mkdir -p "$BUILD_DIR/sbom"
declare -A EXPECTED=(
    [api]='"name": *"axum"'
    [worker]='"name": *"tokio"'
    [mcp]='"name": *"axum"'
    [web]='"name": *"@angular/core"'
    [postgres]='"name": *"pgbackrest"'
)
for name in api worker mcp web postgres; do
    sbom="$BUILD_DIR/sbom/mlbpp-$name.spdx.json"
    # The lock-file cataloger reads web's package-lock.json (production
    # dependencies only); syft leaves it out of image scans by default.
    docker run --rm -v /var/run/docker.sock:/var/run/docker.sock:ro "$SYFT_IMAGE" \
        "docker:$(image "$name")" --quiet --output spdx-json \
        --select-catalogers +javascript-lock-cataloger > "$sbom"
    grep -Eq "${EXPECTED[$name]}" "$sbom" || fail "$name: the SBOM does not list ${EXPECTED[$name]}"
    packages="$(grep -o '"SPDXID": *"SPDXRef-Package-' "$sbom" | wc -l)"
    pass "$name: $sbom ($packages packages)"
done

echo "[$TEST_NAME] all passed"
