#!/usr/bin/env bash
set -u
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp="$(mktemp -d /tmp/harpoon-stage6-regression.XXXXXX)"
trap 'rm -rf "$tmp"' EXIT

cat >"$tmp/docker" <<'SH'
#!/usr/bin/env bash
set -u
log="$MOCK_LOG"
printf 'docker %s\n' "$*" >>"$log"
if [ "$1" = --context ] && [ "$2" = harpoon ]; then shift 2; fi
case "$1" in
  version) [ -e "$MOCK_STARTED" ] || [ "$MOCK_PRESERVE" = 1 ] || exit 1 ;;
  create)
    printf '%s\n' "$MOCK_NAME" >"$MOCK_CONTAINER"
    printf '%s\n' "$*" >"$MOCK_CREATE_ARGS"
    printf 'mock-id\n'
    ;;
  start|inspect|logs|exec|stop)
    [ -e "$MOCK_CONTAINER" ] || exit 1
    if [ "$1" = logs ]; then printf 'stage6-log\n'; fi
    ;;
  rm)
    [ "$MOCK_RM_FAIL" = 1 ] && exit 1
    rm -f "$MOCK_CONTAINER"
    ;;
esac
SH
chmod +x "$tmp/docker"

cat >"$tmp/harpoon" <<'SH'
#!/usr/bin/env bash
set -u
printf 'harpoon %s\n' "$*" >>"$MOCK_LOG"
[ "$1" = start ] && { : >"$MOCK_STARTED"; exit 0; }
[ "$1" = stop ] && { [ -e "$MOCK_CONTAINER" ] && exit 9; : >"$MOCK_STOPPED"; exit 0; }
exit 0
SH
chmod +x "$tmp/harpoon"

run_case() {
  local label="$1" start="$2" rm_fail="$3" expected="$4"
  : >"$tmp/log"; rm -f "$tmp/container" "$tmp/started" "$tmp/stopped"
  MOCK_LOG="$tmp/log" MOCK_CONTAINER="$tmp/container" MOCK_CREATE_ARGS="$tmp/create" MOCK_NAME="harpoon-stage6-boundary-$$" MOCK_STARTED="$tmp/started" MOCK_STOPPED="$tmp/stopped" MOCK_RM_FAIL="$rm_fail" \
    PATH="$tmp:$PATH" HARPOON_STAGE6_BIN="$tmp/harpoon" HARPOON_STAGE6_SKIP_PRIOR_GATES=1 HARPOON_STAGE6_LIVE=1 MOCK_PRESERVE="$start" \
    bash "$ROOT/tools/stage-6-docker-boundary-gate.sh" >"$tmp/out" 2>&1
  rc=$?
  [ "$rc" -eq "$expected" ] || { cat "$tmp/out"; echo "FAIL $label exit=$rc expected=$expected"; exit 1; }
  if [ "$label" = ownership ]; then
    rg -q -- '--label com.krazybean.harpoon.validation=stage6 .*--label com.krazybean.harpoon.stage=6' "$tmp/create" || { echo 'FAIL Stage 6 ownership labels'; exit 1; }
    [ ! -e "$tmp/container" ] || { echo 'FAIL successful cleanup left container'; exit 1; }
    ! rg -q '^harpoon stop' "$tmp/log" || { echo 'FAIL already-running Harpoon was stopped'; exit 1; }
  elif [ "$label" = cleanup-failure ]; then
    rg -q 'FAIL Docker cleanup' "$tmp/out" || { echo 'FAIL cleanup failure was not reported'; exit 1; }
  elif [ "$label" = started ]; then
    rg -q 'Harpoon stop after probe cleanup' "$tmp/out" || { echo 'FAIL started Harpoon was not stopped'; exit 1; }
    rm_line="$(rg -n 'docker .* rm -f' "$tmp/log" | head -1 | cut -d: -f1)"
    stop_line="$(rg -n '^harpoon stop' "$tmp/log" | head -1 | cut -d: -f1)"
    [ -n "$rm_line" ] && [ -n "$stop_line" ] && [ "$rm_line" -lt "$stop_line" ] || { echo 'FAIL Harpoon stopped before cleanup'; exit 1; }
  fi
}

run_case ownership 1 0 0
run_case cleanup-failure 1 1 1
run_case started 0 0 0
echo STAGE_6_CLEANUP_REGRESSION_PASS
