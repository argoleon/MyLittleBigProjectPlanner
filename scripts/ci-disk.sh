#!/usr/bin/env bash
# Disk space of a GitHub-hosted runner (`just ci-disk-free`, `just ci-disk-wait`).
#
# The debug test binaries of the backend, and the images and stack of the
# deployment job, outgrow the ~14 GB a runner has free: the linker then dies
# with SIGBUS, or Docker runs out of space. The preinstalled toolchains this
# project never uses take about 25 GB, but deleting them takes one to two
# minutes. So:
#
#   free  starts deleting them in the background and returns at once; the
#         checkout, the caches and the images are fetched meanwhile;
#   wait  waits until it is done (at most 10 minutes), before the steps that
#         need the space.
#
# Only for CI runners: it needs sudo and deletes system directories.
set -euo pipefail

state=${RUNNER_TEMP:-/tmp}/ci-disk
unused=(/usr/share/dotnet /usr/local/lib/android /opt/ghc /opt/hostedtoolcache/CodeQL /usr/local/share/boost)

case "${1:-}" in
    free | delete)
        [ "${GITHUB_ACTIONS:-}" = true ] || { echo "ci-disk.sh deletes system directories: GitHub runners only" >&2; exit 1; }
        ;;
esac

case "${1:-}" in
    free)
        mkdir -p "$state"
        rm -f "$state/done"
        df -h /
        # Detached, its output in a file: the step does not wait for it.
        nohup setsid "$0" delete > "$state/log" 2>&1 < /dev/null &
        echo "deleting ${unused[*]} in the background"
        ;;
    delete)
        # (internal: run by `free`) the exit status tells `wait` how it went.
        status=0
        sudo rm -rf "${unused[@]}" || status=$?
        echo "$status" > "$state/done"
        ;;
    wait)
        for _ in $(seq 600); do
            if [ -f "$state/done" ]; then
                cat "$state/log"
                df -h /
                exit "$(cat "$state/done")"
            fi
            sleep 1
        done
        echo "the disk was still being freed after 10 minutes" >&2
        exit 1
        ;;
    *)
        echo "usage: $0 free|wait" >&2
        exit 2
        ;;
esac
