#!/usr/bin/env bash
set -u

# Stage 8: controlled image deletion vs guest free space and host APFS allocation.
# This probe never prunes images, containers, volumes, or build cache broadly.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
ARTIFACT_DIR="${HARPOON_STAGE8_ARTIFACT_DIR:-/tmp/harpoon-stage8-disk-reclamation}"
mkdir -p "$ARTIFACT_DIR"
LOG="$ARTIFACT_DIR/run.log"
RESULT="$ARTIFACT_DIR/reclamation.csv"
WORK="$ARTIFACT_DIR/work"
mkdir -p "$WORK"
exec > >(tee "$LOG") 2>&1

fail() { printf 'FAIL %s\n' "$*" >&2; exit 1; }
note() { printf 'INFO %s\n' "$*"; }
cleanup() {
  if [ -n "${IMAGE:-}" ]; then docker --context harpoon image rm "$IMAGE" >/dev/null 2>&1 || true; fi
  rm -f "$WORK/payload" "$WORK/image.tar"
  rmdir "$WORK" 2>/dev/null || true
}
trap cleanup EXIT

command -v docker >/dev/null 2>&1 || fail "docker CLI not found"
command -v stat >/dev/null 2>&1 || fail "stat not found"
command -v du >/dev/null 2>&1 || fail "du not found"
[ -x harpoon/build/harpoon ] || fail "harpoon/build/harpoon is missing; build the runtime first"

if ! docker --context harpoon version >/dev/null 2>&1; then
  harpoon/build/harpoon start >"$ARTIFACT_DIR/start.log" 2>&1 || fail "Harpoon start"
fi
docker --context harpoon version >/dev/null 2>&1 || fail "Harpoon Docker endpoint is not ready"

DISK="$(harpoon/build/harpoon status --json 2>/dev/null | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d.get("diskPath", d.get("disk", "")))' 2>/dev/null || true)"
[ -f "$DISK" ] || fail "persistent disk path is unavailable"

disk_bytes() { stat -f '%z %b %k' "$DISK" 2>/dev/null || stat -c '%s %b %B' "$DISK"; }
guest_df() {
  harpoon/build/harpoon exec -- df -B1 / 2>/dev/null |
    awk 'NR > 1 && $NF == "/" { print $(NF-4), $(NF-3), $(NF-2); exit }'
}

printf 'timestamp,phase,logical_bytes,allocated_blocks,block_size,guest_used_bytes,guest_free_bytes\n' >"$RESULT"
record() {
  phase="$1"
  set -- $(disk_bytes)
  guest="$(guest_df || true)"
  set -- "$@" $guest
  printf '%s,%s,%s,%s,%s,%s,%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$phase" "$1" "$2" "$3" "${4:-}" "${5:-}" >>"$RESULT"
}

record baseline
note "baseline recorded; no existing Docker objects will be removed"

IMAGE="stage8-reclamation-$(date +%s)-$$"
mkdir -p "$WORK/root"
dd if=/dev/urandom of="$WORK/payload" bs=1m count=64 >/dev/null 2>&1 || fail "payload creation"
cp "$WORK/payload" "$WORK/root/payload"
tar -C "$WORK/root" -cf "$WORK/image.tar" . || fail "image archive creation"
docker --context harpoon import "$WORK/image.tar" "$IMAGE" >/dev/null || fail "temporary image import"
record image-created

docker --context harpoon image rm "$IMAGE" >/dev/null || fail "temporary image removal"
IMAGE=""
record image-removed

DISCARD_CAPABILITY="$ARTIFACT_DIR/discard-capability.txt"
if harpoon/build/harpoon exec -- sh -c '
  printf "fstrim="
  command -v fstrim || true
  printf "root="
  awk '\''$2 == "/" { print $3; exit }'\'' /proc/mounts
  for name in discard_granularity discard_max_bytes discard_zeroes_data; do
    printf "%s=" "$name"
    cat "/sys/class/block/vda/queue/$name" 2>/dev/null || printf "unavailable"
  done
' >"$DISCARD_CAPABILITY" 2>&1; then
  note "guest discard capability recorded"
else
  note "guest discard capability query failed; see discard-capability.txt"
fi

if harpoon/build/harpoon exec -- sh -c 'command -v fstrim >/dev/null 2>&1 && fstrim /' >"$ARTIFACT_DIR/fstrim.log" 2>&1; then
  TRIM_RESULT="completed"
  note "guest root fstrim completed"
else
  TRIM_RESULT="unsupported-or-rejected"
  note "guest root fstrim unavailable or rejected; see fstrim.log"
fi
record after-fstrim

cat >"$ARTIFACT_DIR/verdict.txt" <<'EOF'
Stage 8 image reclamation probe completed without broad cleanup.

Interpretation:
- image-created vs image-removed measures guest deletion and host backing allocation;
- discard-capability.txt records whether the guest exposes fstrim and virtio-blk discard;
- fstrim.log records an explicit root-filesystem trim attempt (BusyBox uses `fstrim /`);
- a missing or rejected trim is unsupported-or-rejected evidence, never a trim pass;
- after-fstrim tests whether guest discard reaches the sparse/APFS backing file;
- unchanged host allocation is evidence that deletion alone did not reclaim APFS blocks;
- build-cache pruning was not executed because Docker's broad builder prune is not
  scoped to this probe's temporary image and could remove user cache.
EOF
printf 'trim_result=%s\n' "$TRIM_RESULT" >>"$ARTIFACT_DIR/verdict.txt"
note "controlled image lifecycle completed; build cache untouched; trim=$TRIM_RESULT"
