#!/bin/bash
# A hard stop of the host must not stop the next boot's Docker from starting. /var/run is a real directory on the persistent root, so
# docker's exec-root survives; libcontainerd refuses to start containerd while containerd.pid names a live pid, and boot pids repeat
# almost exactly. Runs the init's real cleanup line against a sandboxed copy of that state.
#   bash tools/guest-builder/regression-guest-stale-runtime.sh
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
INIT="$HERE/src/init"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/harpoon-guest-stale.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT
fail() { echo "guest-stale-runtime FAIL: $*" >&2; exit 1; }

line=$(grep -n '^rm -f /var/run/docker/containerd/containerd.pid' "$INIT" | head -1 | cut -d: -f1)
start=$(grep -n '^dockerd --host=' "$INIT" | head -1 | cut -d: -f1)
[ -n "$line" ] && [ -n "$start" ] && [ "$line" -lt "$start" ] || fail "init does not clear the stale containerd pidfile before dockerd starts"

# The real command, with the root moved into the sandbox and the guest-console tee (/dev/hvc0 exists only in the guest) dropped.
mkdir -p "$TMP/var/run/docker/containerd" "$TMP/var/lib/docker/containers/keep"
echo 14771 > "$TMP/var/run/docker/containerd/containerd.pid"
echo data > "$TMP/var/lib/docker/containers/keep/state"
sed -n "${line}p" "$INIT" | sed "s# 2>&1 | tee.*##; s#/var/run#$TMP/var/run#g" | sh
[ ! -e "$TMP/var/run/docker/containerd/containerd.pid" ] || fail "the stale pidfile is still there"
[ -e "$TMP/var/lib/docker/containers/keep/state" ] || fail "container data under /var/lib/docker must never be touched"
# and a clean boot (no pidfile) is not an error
sed -n "${line}p" "$INIT" | sed "s# 2>&1 | tee.*##; s#/var/run#$TMP/var/run#g" | sh || fail "a boot with no stale pidfile must not fail"
sh -n "$INIT" || fail "init has a syntax error"
echo "guest stale runtime regression: PASS"
