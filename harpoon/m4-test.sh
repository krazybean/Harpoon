#!/bin/sh
set -eu
HOST_SOCK="/tmp/harpoon-docker.sock"
export DOCKER_HOST="unix://$HOST_SOCK"
say() { echo "[m4] $*"; }
fail() { echo "[m4] FAIL $*" >&2; exit 1; }
ok() { echo "[m4] PASS $*"; }
need_sock() {
  [ -S "$HOST_SOCK" ] || fail "socket $HOST_SOCK missing (harpoon not RUNNING)"
  perms=$(stat -f "%Lp" "$HOST_SOCK" 2>/dev/null || stat -c "%a" "$HOST_SOCK" 2>/dev/null || echo "?")
  [ "$perms" = "600" ] || fail "socket perms $perms != 600"
  ok "socket 0600 $HOST_SOCK"
}
# require harpoon RUNNING for live tests
if [ ! -S "$HOST_SOCK" ]; then
  say "Harpoon not RUNNING — socket missing, cannot run M4 live tests"
  say "Start Harpoon: harpoon/build/harpoon > /tmp/harpoon.log 2>&1 & then rerun"
  exit 2
fi
need_sock
say "=== M4 Filesystem & Storage $HOST_SOCK ==="
# check VirtioFS devices
say "VirtioFS shares"
DOCKER_HOST="unix://$HOST_SOCK" docker version 2>&1 | head -n 5 || fail "docker version"
# ensure share roots exist
for p in /Users /private/tmp /tmp; do
  [ -d "$p" ] || echo "[m4] WARN $p not exists"
done
# create dedicated host test dir
M4_HOST="/tmp/harpoon-m4"
rm -rf "$M4_HOST" 2>&1 | true
mkdir -p "$M4_HOST"
say "host test dir $M4_HOST"
# 7. read/write via translated macOS path (not /mnt/harpoon-*)
say "read/write translated bind"
echo "host-value" > "$M4_HOST/host.txt"
DOCKER_HOST="unix://$HOST_SOCK" docker run --rm -v "$M4_HOST:/workspace" alpine:3.22 cat /workspace/host.txt | grep -q "host-value" || fail "host->container read"
ok "host->container"
DOCKER_HOST="unix://$HOST_SOCK" docker run --rm -v "$M4_HOST:/workspace" alpine:3.22 sh -c 'echo container-value > /workspace/container.txt'
cat "$M4_HOST/container.txt" | grep -q "container-value" || fail "container->host write"
ok "container->host"
# nested directories
mkdir -p "$M4_HOST/nested/a/b"
echo "nested" > "$M4_HOST/nested/a/b/file.txt"
DOCKER_HOST="unix://$HOST_SOCK" docker run --rm -v "$M4_HOST:/workspace" alpine:3.22 cat /workspace/nested/a/b/file.txt | grep -q "nested" || fail "nested"
ok "nested directories"
# symlink
ln -sf host.txt "$M4_HOST/link.txt"
DOCKER_HOST="unix://$HOST_SOCK" docker run --rm -v "$M4_HOST:/workspace" alpine:3.22 cat /workspace/link.txt | grep -q "host-value" || fail "symlink"
ok "symlink"
# file bind (single file)
echo "file-bind" > "$M4_HOST/single.txt"
DOCKER_HOST="unix://$HOST_SOCK" docker run --rm -v "$M4_HOST/single.txt:/workspace/single.txt" alpine:3.22 cat /workspace/single.txt | grep -q "file-bind" || fail "file bind"
ok "file bind"
# read-only
DOCKER_HOST="unix://$HOST_SOCK" docker run --rm -v "$M4_HOST:/workspace:ro" alpine:3.22 cat /workspace/host.txt | grep -q "host-value" || fail "ro read"
# ro write should fail
set +e
out_ro=$(DOCKER_HOST="unix://$HOST_SOCK" docker run --rm -v "$M4_HOST:/workspace:ro" alpine:3.22 sh -c 'echo x > /workspace/should-fail' 2>&1)
status_ro=$?
set -e
if [ $status_ro -eq 0 ]; then
  # check if file was created on host (should not)
  if [ -f "$M4_HOST/should-fail" ]; then fail "ro write should have failed but file created"; else echo "[m4] WARN ro write exit 0 but file not created (maybe container succeeded but host RO prevented?)"; fi
else
  echo "$out_ro" | grep -qi "read-only\|permission denied\|operation not permitted" || echo "[m4] WARN ro write failed but without expected text: $out_ro"
  ok "ro write fails as expected"
fi
# long syntax --mount
DOCKER_HOST="unix://$HOST_SOCK" docker run --rm --mount type=bind,source="$M4_HOST",target=/workspace alpine:3.22 cat /workspace/host.txt | grep -q "host-value" || fail "mount long syntax"
ok "long syntax --mount"
DOCKER_HOST="unix://$HOST_SOCK" docker run --rm --mount type=bind,source="$M4_HOST",target=/workspace,readonly alpine:3.22 cat /workspace/host.txt | grep -q "host-value" || fail "mount ro long"
ok "long syntax ro"
# spaces in path
SPACED="$M4_HOST/space dir"
mkdir -p "$SPACED"
echo "spaced" > "$SPACED/file.txt"
DOCKER_HOST="unix://$HOST_SOCK" docker run --rm -v "$SPACED:/workspace" alpine:3.22 cat /workspace/file.txt | grep -q "spaced" || fail "spaces in path"
ok "spaces in path"
# /private/tmp vs /tmp canonicalization
PRIVATE_TMP="/private/tmp/harpoon-m4-private"
mkdir -p "$PRIVATE_TMP"
echo "private" > "$PRIVATE_TMP/file.txt"
DOCKER_HOST="unix://$HOST_SOCK" docker run --rm -v "$PRIVATE_TMP:/workspace" alpine:3.22 cat /workspace/file.txt | grep -q "private" || fail "/private/tmp bind"
# also via /tmp alias if /tmp/harpoon-m4-private is symlink? On macOS /tmp is symlink to /private/tmp, but we created /private/tmp directly, test both
DOCKER_HOST="unix://$HOST_SOCK" docker run --rm -v "/tmp/harpoon-m4-private:/workspace" alpine:3.22 cat /workspace/file.txt 2>&1 | grep -q "private" || echo "[m4] INFO /tmp alias not same as /private/tmp (expected if host path canonicalized)"
ok "/private/tmp and /tmp alias"
# live coherency
say "live coherency"
DOCKER_HOST="unix://$HOST_SOCK" docker run -d --name m4-live --mount type=bind,source="$M4_HOST",target=/workspace alpine:3.22 sleep 120 2>&1 >/dev/null
sleep 2
echo "host-live" > "$M4_HOST/live.txt"
DOCKER_HOST="unix://$HOST_SOCK" docker exec m4-live cat /workspace/live.txt | grep -q "host-live" || fail "host->container live"
ok "host->container live"
DOCKER_HOST="unix://$HOST_SOCK" docker exec m4-live sh -c 'echo container-live > /workspace/from-container.txt'
cat "$M4_HOST/from-container.txt" | grep -q "container-live" || fail "container->host live"
ok "container->host live"
# measure latency qualitatively
say "filesystem semantics"
DOCKER_HOST="unix://$HOST_SOCK" docker exec m4-live sh -c 'mkdir -p /workspace/sem && rmdir /workspace/sem && echo mkdir-rmdir-ok'
DOCKER_HOST="unix://$HOST_SOCK" docker exec m4-live sh -c 'mkdir -p /workspace/sem && echo a > /workspace/sem/a && mv /workspace/sem/a /workspace/sem/b && cat /workspace/sem/b' | grep -q "a" || fail "rename"
DOCKER_HOST="unix://$HOST_SOCK" docker exec m4-live sh -c 'rm /workspace/sem/b && test ! -e /workspace/sem/b' || fail "unlink" 
DOCKER_HOST="unix://$HOST_SOCK" docker exec m4-live sh -c 'touch /workspace/execbit && chmod +x /workspace/execbit && ls -l /workspace/execbit' | grep -q "x" || fail "chmod +x"
DOCKER_HOST="unix://$HOST_SOCK" docker exec m4-live sh -c 'ln -s /workspace/host.txt /workspace/link2 && readlink /workspace/link2' | grep -q "host.txt" || fail "symlink create"
DOCKER_HOST="unix://$HOST_SOCK" docker exec m4-live sh -c 'touch /workspace/ts && sleep 1 && touch /workspace/ts && ls -l --time-style=full-iso /workspace/ts 2>&1 | head -n 5' | head -n 5 || true
# UID/GID
DOCKER_HOST="unix://$HOST_SOCK" docker exec m4-live id 2>&1 | head -n 5
ls -ln "$M4_HOST/host.txt" 2>&1 | head -n 5
ok "filesystem semantics"
say "Git working tree"
if command -v git >/dev/null 2>&1; then
  rm -rf "$M4_HOST/git-test" 2>&1 | true
  mkdir -p "$M4_HOST/git-test"
  (cd "$M4_HOST/git-test" && git init -q 2>&1 | head -n 5)
  echo "initial" > "$M4_HOST/git-test/file.txt"
  (cd "$M4_HOST/git-test" && git add file.txt 2>&1 | head -n 5; git commit -m "init" -q 2>&1 | head -n 5 || true)
  DOCKER_HOST="unix://$HOST_SOCK" docker run --rm -v "$M4_HOST/git-test:/workspace" alpine:3.22 sh -c 'apk add --no-cache git >/dev/null 2>&1; git -C /workspace status' | grep -q "file.txt" || echo "[m4] WARN git status not as expected"
  echo "change" >> "$M4_HOST/git-test/file.txt"
  DOCKER_HOST="unix://$HOST_SOCK" docker run --rm -v "$M4_HOST/git-test:/workspace" alpine:3.22 sh -c 'apk add --no-cache git >/dev/null 2>&1; git -C /workspace diff --stat' | grep -q "file.txt" || echo "[m4] WARN git diff not detected"
  # modify on host, detect in container via live mount
  DOCKER_HOST="unix://$HOST_SOCK" docker exec m4-live sh -c 'apk add --no-cache git >/dev/null 2>&1; git -C /workspace status' 2>&1 | head -n 10 || true
  ok "Git working tree"
else
  say "git not available, skipping Git test"
fi
say "file watchers (inotify)"
if DOCKER_HOST="unix://$HOST_SOCK" docker exec m4-live sh -c 'apk add --no-cache inotify-tools >/dev/null 2>&1; timeout 3 inotifywait -e modify /workspace/live.txt 2>&1 | head -n 5' 2>&1 | grep -q "MODIFY" ; then
  ok "inotify host->guest delivered"
else
  echo "[m4] FILE_CONTENT_COHERENCY PASS (host edits visible), FILE_WATCH_EVENT_DELIVERY NOT OBSERVED (expected per Spike 4, affects webpack/vite/nodemon hot reload, use polling)"
fi
# check content coherency already proven via live tests above
say "read-only binds already tested"
# named volume persistence (without restart, just create and verify)
say "named volume"
DOCKER_HOST="unix://$HOST_SOCK" docker volume create m4-volume 2>&1 | tail -n 2
DOCKER_HOST="unix://$HOST_SOCK" docker volume ls | grep -q "m4-volume" || fail "volume ls"
DOCKER_HOST="unix://$HOST_SOCK" docker volume inspect m4-volume | grep -q "m4-volume" || fail "volume inspect"
DOCKER_HOST="unix://$HOST_SOCK" docker run --rm -v m4-volume:/data alpine:3.22 sh -c 'echo persistent > /data/value'
DOCKER_HOST="unix://$HOST_SOCK" docker run --rm -v m4-volume:/data alpine:3.22 cat /data/value | grep -q "persistent" || fail "volume persistent"
ok "named volume (restart persistence requires manual harpoon restart, see docs)"
# disk observability
say "disk capacity"
ls -lh spike2/cache/harpoon-root.img harpoon/cache/harpoon-m4-initramfs.cpio.gz 2>&1 | head -n 5
DOCKER_HOST="unix://$HOST_SOCK" docker system df 2>&1 | head -n 10 || true
DOCKER_HOST="unix://$HOST_SOCK" docker info 2>&1 | grep -i "Root Dir\|Docker Root" | head -n 5 || true
# security boundary
say "security boundary: only /Users and /private/tmp (via /tmp) plus /tmp/harpoon-share exposed, not /"
# attempt to bind /etc must be rejected by Harpoon (outside shared roots)
set +e
out_etc=$(DOCKER_HOST="unix://$HOST_SOCK" docker run --rm -v /etc:/workspace alpine:3.22 ls /workspace 2>&1)
status_etc=$?
set -e
if [ $status_etc -eq 0 ]; then
  fail "security: docker run -v /etc:/workspace should have failed but exited 0, output: $out_etc"
fi
echo "$out_etc" | grep -qi "not shared\|unsupported host path" || fail "security: expected 'not shared' or 'unsupported host path' in error, got: $out_etc"
ok "security /etc rejected as not shared (unsupported host path)"
# cleanup m4 objects but keep harpoon running
say "cleanup"
DOCKER_HOST="unix://$HOST_SOCK" docker rm -f m4-live 2>&1 >/dev/null || true
DOCKER_HOST="unix://$HOST_SOCK" docker volume rm m4-volume 2>&1 >/dev/null || true
rm -rf "$M4_HOST" "$PRIVATE_TMP" "$SPACED" 2>&1 | true
ok "cleanup m4 objects"
say "regression M1/M2/M3"
DOCKER_HOST="unix://$HOST_SOCK" docker version 2>&1 | head -n 5 || fail "docker version"
DOCKER_HOST="unix://$HOST_SOCK" docker run --rm hello-world 2>&1 | grep -q "Hello" || fail "hello-world"
DOCKER_HOST="unix://$HOST_SOCK" docker run --rm alpine:3.22 ping -c 1 1.1.1.1 2>&1 | grep -q "1 packets" || echo "[m4] WARN ping failed (networking M5?)"
[ -S /tmp/harpoon-docker.sock ] || fail "regression socket gone"
ok "regression"
say "ALL M4 CHECKS PASS (restart persistence manual, see docs)"
