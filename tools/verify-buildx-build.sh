#!/bin/bash
set -uo pipefail
# Hermetic build test for the bundled Docker Buildx against a RUNNING Harpoon VM (A7 #5/#6 in
# docs/results/COMPAT_BUILDX_AND_PORT_FORWARDING.md; evidence in docs/results/DEPLOY_BUILDX_VALIDATION.md).
#
# LIVE: this script builds real images in the Harpoon engine. It refuses to start unless
# HARPOON_LIVE_VM_TESTS=1 is set. `--plan` prints what it would do and exits without contacting
# any engine. It is not run by CI and not part of any release pipeline.
#
#   HARPOON_LIVE_VM_TESTS=1 bash tools/verify-buildx-build.sh [--bundle-bin DIR] [--socket PATH]
#   bash tools/verify-buildx-build.sh --plan
#
#   --bundle-bin DIR  directory holding the bundled `docker-buildx` and `docker`
#                     (default: <Harpoon.app>/Contents/Resources/harpoon/bin, built app path)
#   --socket PATH     Harpoon's Docker socket (default: /tmp/harpoon-docker.sock)
#
# Safety: every command runs under `env -i PATH=/usr/bin:/bin HOME=<tmp> DOCKER_CONFIG=<tmp>
# DOCKER_HOST=unix://<socket>`; only the bundled binaries are used (never Docker Desktop, PATH
# or the user's ~/.docker); no image is pulled (the base image must already be in the guest); it
# only ever removes the uniquely tagged images it built itself; it never prunes anything; the
# generated secret is never printed.
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUNDLE_BIN="$REPO_ROOT/ui/harpoon-desktop/src-tauri/target/release/bundle/macos/Harpoon.app/Contents/Resources/harpoon/bin"
SOCKET="/tmp/harpoon-docker.sock"
BASE_IMAGE="alpine:3.22"
PLAN=0
while [ $# -gt 0 ]; do
  case "$1" in
    --plan) PLAN=1 ;;
    --bundle-bin) BUNDLE_BIN="${2:?--bundle-bin needs a directory}"; shift ;;
    --socket) SOCKET="${2:?--socket needs a path}"; shift ;;
    *) echo "[buildx-build] unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done

if [ "$PLAN" -eq 1 ]; then
  cat <<PLANEOF
[buildx-build] plan (nothing is executed in --plan mode)
  tools: $BUNDLE_BIN/docker-buildx and $BUNDLE_BIN/docker; engine: unix://$SOCKET; base image: $BASE_IMAGE (must already exist; never pulled)
  P0  preflight: bundled binaries present+executable; engine reachable; base image present; record image/container/volume counts
  T1  plain Dockerfile (no '# syntax=' line): build --load; image exists
  T2  RUN --mount=type=secret: correct secret builds; image exists; secret absent from docker history, image config and build logs
  T3  RUN --mount=type=cache + heredoc: build --load; image exists
  N1  negative control: --no-cache with a WRONG secret must FAIL
  N2  negative control: legacy 'docker build --secret' must FAIL (the legacy builder does not support it)
  N3  negative control: buildx without a reachable engine (bogus socket) must FAIL, never fall back
  A1  accounting: containers and volumes unchanged; images back to the starting count after removing only this run's tags; nothing pruned
PLANEOF
  exit 0
fi

if [ "${HARPOON_LIVE_VM_TESTS:-0}" != "1" ]; then
  echo "[buildx-build] REFUSING: this test builds images in the live Harpoon engine. Set HARPOON_LIVE_VM_TESTS=1 to run it (or use --plan)." >&2
  exit 2
fi

BUILDX="$BUNDLE_BIN/docker-buildx"
DOCKER="$BUNDLE_BIN/docker"
FAIL=0
pass() { echo "[buildx-build] PASS: $1" >&2; }
fail() { echo "[buildx-build] FAIL: $1" >&2; FAIL=1; }

for b in "$BUILDX" "$DOCKER"; do
  [ -f "$b" ] && [ -x "$b" ] || { echo "[buildx-build] FAIL: bundled binary missing or not executable: $b (no fallback to Docker Desktop or PATH)" >&2; exit 1; }
done
[ -S "$SOCKET" ] || { echo "[buildx-build] FAIL: no Docker socket at $SOCKET (is Harpoon running?)" >&2; exit 1; }

WORK="$(mktemp -d)"
HOME_DIR="$WORK/home"; mkdir -p "$HOME_DIR" "$WORK/ctx"
RUN_ID="$(date +%s)-$$"
TAGS=""
cleanup() {
  # Remove only the images this run built. No prune of any kind.
  for t in $TAGS; do hx "$DOCKER" rmi "$t" >/dev/null 2>&1 || true; done
  rm -rf "$WORK"
}
trap cleanup EXIT
hx() { local sock="$SOCKET"; env -i PATH=/usr/bin:/bin HOME="$HOME_DIR" DOCKER_CONFIG="$HOME_DIR" DOCKER_HOST="unix://$sock" "$@"; }
hx_nosock() { env -i PATH=/usr/bin:/bin HOME="$HOME_DIR" DOCKER_CONFIG="$HOME_DIR" DOCKER_HOST="unix://$WORK/no-such.sock" "$@"; }
count() { hx "$DOCKER" "$1" ls -q 2>/dev/null | wc -l | tr -d ' '; }
tag() { TAGS="$TAGS harpoon-buildx-verify/$RUN_ID:$1"; echo "harpoon-buildx-verify/$RUN_ID:$1"; }

# P0 preflight
hx "$BUILDX" version >/dev/null 2>&1 && pass "bundled buildx runs hermetically" || { fail "bundled buildx does not run"; exit 1; }
hx "$DOCKER" version --format '{{.Server.Version}}' >/dev/null 2>&1 && pass "engine reachable" || { fail "engine not reachable at $SOCKET"; exit 1; }
hx "$DOCKER" image inspect "$BASE_IMAGE" >/dev/null 2>&1 && pass "base image $BASE_IMAGE already present (no pulls)" || { fail "base image $BASE_IMAGE is not in the guest; this test never pulls"; exit 1; }
IMG0="$(count image)"; CTR0="$(hx "$DOCKER" ps -aq 2>/dev/null | wc -l | tr -d ' ')"; VOL0="$(count volume)"
echo "[buildx-build] before: images=$IMG0 containers=$CTR0 volumes=$VOL0" >&2

SECRET="$(od -An -N24 -tx1 /dev/urandom | tr -d ' \n')"
SECRET_SHA="$(printf '%s' "$SECRET" | shasum -a 256 | cut -d' ' -f1)"
printf '%s' "$SECRET" > "$WORK/secret.txt"
printf '%s' "not-the-secret-$RUN_ID" > "$WORK/wrong.txt"

# T1 plain
cat > "$WORK/ctx/plain.Dockerfile" <<DF
FROM scratch
LABEL io.harpoon.test="plain"
DF
T1="$(tag plain)"
if hx "$BUILDX" build --load -t "$T1" -f "$WORK/ctx/plain.Dockerfile" "$WORK/ctx" >"$WORK/t1.log" 2>&1 && hx "$DOCKER" image inspect "$T1" >/dev/null 2>&1; then pass "T1 plain build"; else fail "T1 plain build"; tail -n 5 "$WORK/t1.log" >&2; fi

# T2 secret. The expected value is passed as its SHA-256 in a build arg (part of the cache key), never the secret itself.
cat > "$WORK/ctx/secret.Dockerfile" <<DF
FROM $BASE_IMAGE
ARG EXPECT_SHA
RUN --mount=type=secret,id=tok test "\$(sha256sum /run/secrets/tok | cut -d' ' -f1)" = "\$EXPECT_SHA"
DF
T2="$(tag secret)"
if hx "$BUILDX" build --load --no-cache --progress=plain --build-arg "EXPECT_SHA=$SECRET_SHA" --secret "id=tok,src=$WORK/secret.txt" -t "$T2" -f "$WORK/ctx/secret.Dockerfile" "$WORK/ctx" >"$WORK/t2.log" 2>&1 && hx "$DOCKER" image inspect "$T2" >/dev/null 2>&1; then
  pass "T2 secret build"
  { hx "$DOCKER" history --no-trunc "$T2"; hx "$DOCKER" image inspect "$T2"; cat "$WORK/t2.log"; } > "$WORK/t2.all" 2>&1
  if grep -qF "$SECRET" "$WORK/t2.all"; then fail "T2 secret value leaked into history, image config or build log"; else pass "T2 secret absent from history, image config and build log"; fi
else fail "T2 secret build"; tail -n 5 "$WORK/t2.log" >&2; fi

# T3 cache mount + heredoc
cat > "$WORK/ctx/cache.Dockerfile" <<DF
FROM $BASE_IMAGE
RUN --mount=type=cache,target=/cache <<'EOS'
set -e
echo ok > /cache/probe
test "\$(cat /cache/probe)" = ok
echo heredoc-ran > /out.txt
EOS
RUN test "\$(cat /out.txt)" = heredoc-ran
DF
T3="$(tag cache)"
if hx "$BUILDX" build --load -t "$T3" -f "$WORK/ctx/cache.Dockerfile" "$WORK/ctx" >"$WORK/t3.log" 2>&1 && hx "$DOCKER" image inspect "$T3" >/dev/null 2>&1; then pass "T3 cache mount + heredoc"; else fail "T3 cache mount + heredoc"; tail -n 5 "$WORK/t3.log" >&2; fi

# N1 wrong secret must fail (the assertion has teeth)
if hx "$BUILDX" build --no-cache --output type=cacheonly --build-arg "EXPECT_SHA=$SECRET_SHA" --secret "id=tok,src=$WORK/wrong.txt" -f "$WORK/ctx/secret.Dockerfile" "$WORK/ctx" >"$WORK/n1.log" 2>&1; then fail "N1 wrong secret was ACCEPTED (test has no teeth)"; else pass "N1 wrong secret rejected"; fi

# N2 legacy builder must not accept --secret
if hx env DOCKER_BUILDKIT=0 "$DOCKER" build --secret "id=tok,src=$WORK/secret.txt" -f "$WORK/ctx/secret.Dockerfile" "$WORK/ctx" >"$WORK/n2.log" 2>&1; then fail "N2 legacy docker build accepted --secret"; else pass "N2 legacy docker build rejects --secret"; fi

# N3 no engine: must fail, must not fall back to anything else
if hx_nosock "$BUILDX" build --output type=cacheonly -f "$WORK/ctx/plain.Dockerfile" "$WORK/ctx" >"$WORK/n3.log" 2>&1; then fail "N3 buildx succeeded without a reachable engine (a fallback exists)"; else pass "N3 buildx fails closed without an engine"; fi

# A1 accounting
for t in $TAGS; do hx "$DOCKER" rmi "$t" >/dev/null 2>&1 || true; done
TAGS=""
IMG1="$(count image)"; CTR1="$(hx "$DOCKER" ps -aq 2>/dev/null | wc -l | tr -d ' ')"; VOL1="$(count volume)"
echo "[buildx-build] after:  images=$IMG1 containers=$CTR1 volumes=$VOL1" >&2
[ "$CTR1" = "$CTR0" ] && [ "$VOL1" = "$VOL0" ] && pass "A1 containers and volumes unchanged" || fail "A1 containers/volumes changed ($CTR0->$CTR1, $VOL0->$VOL1)"
[ "$IMG1" = "$IMG0" ] && pass "A1 images back to the starting count" || fail "A1 image count differs ($IMG0->$IMG1); inspect manually, nothing was pruned"

if [ "$FAIL" -ne 0 ]; then echo "[buildx-build] FAIL" >&2; exit 1; fi
echo "[buildx-build] PASS: all scenarios" >&2
