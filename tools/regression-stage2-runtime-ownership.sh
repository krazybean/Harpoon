#!/usr/bin/env bash
set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp="$(mktemp -d /tmp/harpoon-stage2-regression.XXXXXX)"
trap 'rm -rf "$tmp"' EXIT
export HARPOON_STAGE2_LIB_ONLY=1 HARPOON_STAGE2_READY_ATTEMPTS=1 HARPOON_STAGE2_READY_DELAY=0 HARPOON_BIN="$tmp/harpoon"
cat >"$HARPOON_BIN" <<'SH'
#!/usr/bin/env bash
case "$1 ${2:-}" in
  "status --json") printf '%s\n' "${MOCK_STATUS:-{\"state\":\"stopped\",\"dockerReady\":false}}" ;;
  start) [ "${MOCK_START_FAIL:-0}" = 1 ] && exit 1; touch "$MOCK_STARTED_FILE" ;;
esac
SH
chmod +x "$HARPOON_BIN"
source "$ROOT/tools/stage-2-resource-gate.sh"

docker() {
  MOCK_DOCKER_CALLS=$((MOCK_DOCKER_CALLS + 1))
  [ "$MOCK_CASE" = stopped ] && [ "$MOCK_DOCKER_CALLS" -eq 1 ] && return 1
  case "$MOCK_CASE:$*" in
    stopped:*ps\ -a\ --no-trunc) [ "$MOCK_DOCKER_CALLS" -eq 1 ] && return 1 ;;
    running:*ps\ -a\ --no-trunc) return 0 ;;
    failure:*ps\ -a\ --no-trunc) return 1 ;;
    foreign:*ps\ -a\ --no-trunc) echo foreign; return 0 ;;
    foreign:*ps\ -aq) echo foreign; return 0 ;;
    foreign:*inspect*) [ "$MOCK_INSPECTS" -eq 0 ] && echo foreign-name || echo '<none>'; MOCK_INSPECTS=$((MOCK_INSPECTS + 1)); return 0 ;;
  esac
  return 0
}

export MOCK_DOCKER_CALLS=0 MOCK_CASE=stopped MOCK_STATUS='{"state":"stopped","dockerReady":false}' MOCK_STARTED_FILE="$tmp/started"
ensure_runtime || exit 1
[ "$STARTED_BY_STAGE2" -eq 1 ] || { echo 'FAIL stopped runtime ownership'; exit 1; }

STARTED_BY_STAGE2=0 MOCK_DOCKER_CALLS=0 MOCK_CASE=running MOCK_STATUS='{"state":"running","dockerReady":true}'
ensure_runtime || exit 1
[ "$STARTED_BY_STAGE2" -eq 0 ] || { echo 'FAIL already-running runtime was restarted'; exit 1; }

STARTED_BY_STAGE2=0 MOCK_DOCKER_CALLS=0 MOCK_CASE=failure MOCK_START_FAIL=1 MOCK_STATUS='{"state":"stopped","dockerReady":false}'
ensure_runtime && { echo 'FAIL start failure was accepted'; exit 1; } || true

MOCK_CASE=foreign MOCK_INSPECTS=0
name="$(docker --context harpoon inspect -f '{{.Name}}' foreign)"
label="$(docker --context harpoon inspect -f '{{index .Config.Labels \"com.krazybean.harpoon.validation\"}}' foreign)"
[ "$label" != stage2 ] && [ "$name" = foreign-name ] || { echo 'FAIL foreign container was not preserved/blocked'; exit 1; }
echo STAGE_2_RUNTIME_OWNERSHIP_REGRESSION_PASS
