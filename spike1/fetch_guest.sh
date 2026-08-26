#!/bin/bash
set -euo pipefail
# ponytail: minimal fetch, documented provenance
# Source: Alpine Linux netboot — authoritative Alpine CDN
# Alpine 3.22 virt kernel + initramfs for ARM64
# Licensing: GPL-2.0 / Alpine
BASE="https://dl-cdn.alpinelinux.org/alpine/v3.22/releases/aarch64/netboot"
CACHE="spike1/cache"
mkdir -p "$CACHE"
KERNEL="$CACHE/vmlinuz-virt"
INITRAMFS="$CACHE/initramfs-virt"
if [ ! -f "$KERNEL" ]; then
  echo "fetching vmlinuz-virt..." >&2
  curl -L --fail -o "$KERNEL" "$BASE/vmlinuz-virt"
fi
if [ ! -f "$INITRAMFS" ]; then
  echo "fetching initramfs-virt..." >&2
  curl -L --fail -o "$INITRAMFS" "$BASE/initramfs-virt"
fi
# Also fetch checksums for provenance verification if available
# Alpine provides .sha256? Try fetch apkindex checksum sidecar
echo "kernel: $(stat -f%z "$KERNEL" 2>/dev/null || stat -c%s "$KERNEL") bytes"
echo "initramfs: $(stat -f%z "$INITRAMFS" 2>/dev/null || stat -c%s "$INITRAMFS") bytes"
# sha256
if command -v shasum >/dev/null; then
  echo "sha256 vmlinuz: $(shasum -a 256 "$KERNEL" | cut -d' ' -f1)"
  echo "sha256 initramfs: $(shasum -a 256 "$INITRAMFS" | cut -d' ' -f1)"
fi
# version provenance
echo "source: $BASE"
echo "version: Alpine 3.22 netboot (virt) — kernel 6.12.x"
echo "fetched: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
# create minimal initramfs override for deterministic HARPOON_SPIKE_OK marker
# We build a secondary tiny initramfs that appends to Alpine's: contains /init-harpoon that prints marker
# But simplest: use Alpine's ramfs as-is and detect its own boot string; also create our own tiny ramfs for stricter test
mkdir -p "$CACHE/init-harpoon"
cat > "$CACHE/init-harpoon/init" <<'INIT'
#!/bin/sh
echo "HARPOON_SPIKE_OK"
echo "HARPOON_SPIKE_OK" > /dev/hvc0 2>/dev/null || true
echo "HARPOON_SPIKE_OK" > /dev/console 2>/dev/null || true
# keep alive briefly so host can detect then shutdown via poweroff
sleep 5
poweroff -f 2>/dev/null || halt -f 2>/dev/null || sleep 30
INIT
chmod +x "$CACHE/init-harpoon/init"
# pack as cpio.gz (requires cpio)
if command -v cpio >/dev/null; then
  (cd "$CACHE/init-harpoon" && find . | cpio -o -H newc 2>/dev/null | gzip > "$CACHE/harpoon-initramfs.cpio.gz")
  echo "harpoon initramfs: $(stat -f%z "$CACHE/harpoon-initramfs.cpio.gz" 2>/dev/null || stat -c%s "$CACHE/harpoon-initramfs.cpio.gz") bytes"
  echo "sha256 harpoon initramfs: $(shasum -a 256 "$CACHE/harpoon-initramfs.cpio.gz" | cut -d' ' -f1)"
else
  echo "cpio not found, skipping harpoon initramfs" >&2
fi

# --- Harpoon spike note: Virtualization.framework requires uncompressed Image on Apple Silicon ---
# Alpine's vmlinuz-virt is a PE+gz wrapper. Decompress inner gzip to get uncompressed Image (33M).
# See vfkit docs: kernel must be uncompressed or VM will hang / fail with VZErrorDomain Code=1.
if [ -f "$CACHE/vmlinuz-virt" ] && [ ! -f "$CACHE/Image-virt" ]; then
  echo "decompressing vmlinuz-virt -> Image-virt (uncompressed)..." >&2
  # Find gzip offset 52152 and decompress rest
  python3 -c "
import pathlib, subprocess, os
data=pathlib.Path('$CACHE/vmlinuz-virt').read_bytes()
pos=data.find(b'\x1f\x8b\x08')
open('/tmp/_harpoon_chunk.gz','wb').write(data[pos:])
import subprocess
r=subprocess.run(['gunzip','-c','/tmp/_harpoon_chunk.gz'], capture_output=True)
open('$CACHE/Image-virt','wb').write(r.stdout)
print(f'decompressed {len(r.stdout)} bytes')
"
  echo "Image-virt: $(stat -f%z "$CACHE/Image-virt" 2>/dev/null || stat -c%s "$CACHE/Image-virt") bytes $(file "$CACHE/Image-virt" | head -c 100)" >&2
  echo "sha256 Image-virt: $(shasum -a 256 "$CACHE/Image-virt" | cut -d' ' -f1)" >&2
fi

echo "done" >&2
