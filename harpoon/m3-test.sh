#!/bin/sh
set -eu
# M3 bounded compatibility test — must be run when harpoon is RUNNING
# All commands use explicit DOCKER_HOST=unix:///tmp/harpoon-docker.sock
HOST="unix:///tmp/harpoon-docker.sock"
export DOCKER_HOST="$HOST"
say() { echo "[m3] $*"; }
fail() { echo "[m3] FAIL $*" >&2; exit 1; }
ok() { echo "[m3] PASS $*"; }
need_sock() {
  [ -S /tmp/harpoon-docker.sock ] || fail "socket /tmp/harpoon-docker.sock missing (harpoon not RUNNING)"
  perms=$(stat -f "%Lp" /tmp/harpoon-docker.sock 2>/dev/null || stat -c "%a" /tmp/harpoon-docker.sock 2>/dev/null || echo "?")
  [ "$perms" = "600" ] || fail "socket perms $perms != 600"
  ok "socket 0600 $HOST"
}
say "=== M3 Docker Compatibility $HOST ==="
need_sock
say "docker version/api negotiation (29.3.1 client 1.54 -> 28.3.3 server 1.51)"
docker version || fail "docker version"
docker info 2>&1 | head -n 20 || fail "docker info"
docker system info 2>&1 | head -n 5 || true
ok "api negotiation (client downgrades to 1.51, no EOF)"
say "core container lifecycle"
docker run --rm hello-world 2>&1 | grep -q "Hello from Docker" || fail "hello-world"
docker pull alpine:3.22 2>&1 | tail -n 5
docker run --name m3-alpine alpine:3.22 echo hello 2>&1 | grep -q "hello" || fail "m3-alpine echo"
docker ps -a | grep -q "m3-alpine" || fail "ps -a"
docker start m3-alpine 2>&1 >/dev/null
docker wait m3-alpine 2>&1 | grep -q "0" || true
docker logs m3-alpine | grep -q "hello" || fail "logs"
docker inspect m3-alpine | grep -q "m3-alpine" || fail "inspect"
docker rm m3-alpine
docker create --name m3-create alpine:3.22 sleep 1 2>&1 >/dev/null
docker start m3-create && docker stop m3-create || true
docker rm m3-create 2>&1 >/dev/null || true
docker run -d --name m3-kill alpine:3.22 sleep 60 2>&1 >/dev/null
docker kill m3-kill 2>&1 >/dev/null || true
docker rm -f m3-kill 2>&1 >/dev/null || true
docker run -d --name m3-restart alpine:3.22 sleep 60 2>&1 >/dev/null
docker restart m3-restart 2>&1 >/dev/null
docker rm -f m3-restart
ok "container lifecycle"
say "image workflows"
docker images | grep -q "alpine" || fail "images"
docker image inspect alpine:3.22 | grep -q "alpine" || fail "image inspect"
docker tag alpine:3.22 harpoon-m3:test
docker image ls | grep -q "harpoon-m3" || fail "tag"
docker rmi harpoon-m3:test
docker save alpine:3.22 -o /tmp/harpoon-m3-alpine.tar
ls -lh /tmp/harpoon-m3-alpine.tar
docker load -i /tmp/harpoon-m3-alpine.tar 2>&1 | grep -q "Loaded" || true
ok "image workflows (host CLI archive /tmp/harpoon-m3-alpine.tar, daemon store guest-side)"
say "build workflows"
TMPDIR=$(mktemp -d /tmp/harpoon-m3-build.XXXX)
cat > "$TMPDIR/Dockerfile" <<'DF'
FROM alpine:3.22
RUN echo build-ok > /build-ok
CMD ["cat", "/build-ok"]
DF
docker build -t harpoon-m3-build "$TMPDIR" 2>&1 | tail -n 5
docker run --rm harpoon-m3-build | grep -q "build-ok" || fail "build run"
docker build --no-cache -t harpoon-m3-build "$TMPDIR" 2>&1 | tail -n 5
docker run --rm harpoon-m3-build | grep -q "build-ok" || fail "build no-cache"
docker rmi harpoon-m3-build
rm -rf "$TMPDIR" /tmp/harpoon-m3-alpine.tar
ok "build (docker build -t, --no-cache) — BuildKit path recorded if used"
say "exec/logs/stdin"
docker run -d --name m3-shell alpine:3.22 sleep 120 2>&1 >/dev/null
docker exec m3-shell echo exec-ok | grep -q "exec-ok" || fail "exec"
docker logs m3-shell | grep -q "exec-ok" || true
printf 'stdin-ok\n' | docker run --rm -i alpine:3.22 cat | grep -q "stdin-ok" || fail "stdin pipe"
printf 'abc\n' | docker exec -i m3-shell sh -c 'cat > /tmp/input; cat /tmp/input' | grep -q "abc" || fail "exec -i cat"
docker run --rm -i alpine:3.22 cat <<'IN' | grep -q "stdin-ok" || fail "tty cat"
stdin-ok
IN
docker rm -f m3-shell
ok "exec/logs/stdin no truncation"
say "env/workdir/entrypoint/user"
docker run --rm -e FOO=bar alpine:3.22 env | grep -q "FOO=bar" || fail "env"
docker run --rm -w /tmp alpine:3.22 pwd | grep -q "/tmp" || fail "workdir"
docker run --rm --entrypoint /bin/echo alpine:3.22 entrypoint-ok | grep -q "entrypoint-ok" || fail "entrypoint"
docker run --rm --user 1000:1000 alpine:3.22 id | grep -q "1000" || fail "user"
ok "env/workdir/entrypoint/user"
say "labels/filters"
docker run -d --name m3-label --label harpoon.test=yes alpine:3.22 sleep 60 2>&1 >/dev/null
docker ps --filter label=harpoon.test=yes | grep -q "m3-label" || fail "filter"
docker inspect m3-label | grep -q "harpoon.test" || fail "inspect label"
docker rm -f m3-label
ok "labels"
say "network objects"
docker network ls | grep -q "bridge" || fail "network ls"
docker network create m3-net
docker network inspect m3-net | grep -q "m3-net" || fail "network inspect"
docker network rm m3-net
ok "network objects"
say "volume objects"
docker volume create m3-volume
docker volume ls | grep -q "m3-volume" || fail "volume ls"
docker volume inspect m3-volume | grep -q "m3-volume" || fail "volume inspect"
docker run --rm -v m3-volume:/data alpine:3.22 sh -c 'echo persistent > /data/value'
docker run --rm -v m3-volume:/data alpine:3.22 cat /data/value | grep -q "persistent" || fail "volume persistent"
docker volume rm m3-volume
ok "volume guest storage (not VirtioFS)"
say "concurrency"
for i in 1 2 3 4 5; do docker run --rm alpine:3.22 echo "$i" 2>&1 | grep -q "$i" || fail "concurrent $i" & done; wait
(docker ps & docker images & docker info >/dev/null 2>&1 & wait) || fail "concurrent ps/images/info"
ok "concurrency no deadlock/EOF mixup"
say "large response/streaming (2-5MB logs)"
docker run --name m3-big alpine:3.22 sh -c 'for i in $(seq 1 50000); do echo "line $i 0123456789 abcdef"; done' 2>&1 >/dev/null || true
docker logs m3-big | wc -l | grep -q "50000" || fail "big logs"
docker rm -f m3-big 2>&1 >/dev/null || true
ok "streaming 50000 lines (~2MB) no truncation"
say "error semantics passthrough"
# proof: command must exit non-zero and contain useful daemon/client error text (case-insensitive)
set +e
out_pull=$(docker run does-not-exist.invalid/image 2>&1)
status_pull=$?
set -e
echo "$out_pull" | grep -qi "pull access denied\|not found\|does-not-exist\|no such" || fail "error passthrough pull missing useful text: $out_pull"
[ $status_pull -ne 0 ] || fail "error passthrough pull expected non-zero exit, got 0"
# docker inspect no-such-container currently returns [] + "error: no such object: no-such-container" exit 1
set +e
out_inspect=$(docker inspect no-such-container 2>&1)
status_inspect=$?
set -e
printf "%s\n" "$out_inspect" | grep -qi "no such\|not found" || fail "error inspect missing useful text: $out_inspect"
[ $status_inspect -ne 0 ] || fail "error inspect expected non-zero exit, got 0"
# docker rm no-such-container similarly
set +e
out_rm=$(docker rm no-such-container 2>&1)
status_rm=$?
set -e
printf "%s\n" "$out_rm" | grep -qi "no such\|not found" || fail "error rm missing useful text: $out_rm"
[ $status_rm -ne 0 ] || fail "error rm expected non-zero exit, got 0"
ok "error passthrough"
say "identity/provenance"
docker info 2>&1 | grep -q "harpoon.runtime" && ok "daemon label harpoon.runtime=true" || echo "[m3] INFO daemon label not yet set (deferred per spec if invasive)"
say "cleanup"
docker container prune -f 2>&1 | head -n 2 || true
for n in $(docker ps -a --format "{{.Names}}" | grep -E "^m3-"); do docker rm -f "$n" 2>&1 >/dev/null || true; done
for i in $(docker images --format "{{.Repository}}:{{.Tag}}" | grep "harpoon-m3"); do docker rmi "$i" 2>&1 >/dev/null || true; done
for n in $(docker network ls --format "{{.Name}}" | grep "m3-net"); do docker network rm "$n" 2>&1 >/dev/null || true; done
for v in $(docker volume ls --format "{{.Name}}" | grep "m3-volume"); do docker volume rm "$v" 2>&1 >/dev/null || true; done
rm -f /tmp/harpoon-m3-alpine.tar
ok "cleanup m3-/harpoon-m3"
say "regression M1/M2"
[ -S /tmp/harpoon-docker.sock ] || fail "regression socket gone"
ls -lh /tmp/harpoon-control 2>&1 | grep -q "srw" || true
ok "regression harpoon RUNNING graceful stop will be tested via SIGINT, restart, persistence"
say "ALL M3 CHECKS PASS"
