#!/bin/sh
set -eu
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BUILD="$SCRIPT_DIR/build"
mkdir -p "$BUILD"
OUT="$BUILD/harpoon"
# Single source of truth for minimum macOS — must match tauri.conf.json bundle.macOS.minimumSystemVersion
HARPOON_MIN_MACOS="$(cat "$SCRIPT_DIR/MIN_MACOS" 2>/dev/null || echo "15.1")"
export MACOSX_DEPLOYMENT_TARGET="$HARPOON_MIN_MACOS"
# HARPOON_ARCH=arm64 (default) or universal (arm64 + x86_64 slices joined with lipo)
HARPOON_ARCH="${HARPOON_ARCH:-arm64}"
case "$HARPOON_ARCH" in
  arm64) SLICES="arm64" ;;
  universal) SLICES="arm64 x86_64" ;;
  *) echo "[harpoon] FAIL: unsupported HARPOON_ARCH $HARPOON_ARCH (arm64|universal)" >&2; exit 1 ;;
esac
SLICE_OUTS=""
for SLICE in $SLICES; do
  SLICE_OUT="$BUILD/harpoon-$SLICE"
  echo "[harpoon] building production runtime for macOS $HARPOON_MIN_MACOS (swift target $SLICE-apple-macosx$HARPOON_MIN_MACOS)..." >&2
  xcrun swiftc -target "$SLICE-apple-macosx$HARPOON_MIN_MACOS" \
    "$SCRIPT_DIR/Sources/RuntimeConfig.swift" "$SCRIPT_DIR/Sources/HostPathTranslator.swift" "$SCRIPT_DIR/Sources/Lifecycle.swift" "$SCRIPT_DIR/Sources/VMManager.swift" "$SCRIPT_DIR/Sources/Bridges.swift" "$SCRIPT_DIR/Sources/PortForwardManager.swift" "$SCRIPT_DIR/Sources/HarpoonRuntimeState.swift" "$SCRIPT_DIR/Sources/HarpoonStatusCLI.swift" "$SCRIPT_DIR/Sources/HarpoonDoctorCLI.swift" "$SCRIPT_DIR/Sources/HarpoonGuestCLI.swift" "$SCRIPT_DIR/Sources/HarpoonDockerCLI.swift" "$SCRIPT_DIR/Sources/HarpoonConfigCLI.swift" "$SCRIPT_DIR/Sources/HarpoonDiskCLI.swift" "$SCRIPT_DIR/Sources/HarpoonMgmtCLI.swift" "$SCRIPT_DIR/Sources/HarpoonDiagnosticsCLI.swift" "$SCRIPT_DIR/Sources/HarpoonLifecycleCLI.swift" "$SCRIPT_DIR/Sources/UnixBridge.swift" "$SCRIPT_DIR/Sources/BalloonBridge.swift" "$SCRIPT_DIR/Sources/MgmtBridge.swift" "$SCRIPT_DIR/Sources/PortForwardBridge.swift" "$SCRIPT_DIR/Sources/HarpoonCLI.swift" "$SCRIPT_DIR/Sources/ContainerCLI.swift" "$SCRIPT_DIR/Sources/main.swift" \
    -framework Virtualization -o "$SLICE_OUT" -module-cache-path "/tmp/harpoon-mcache-$SLICE" \
    -Xlinker -rpath -Xlinker "@executable_path/../../../Frameworks" \
    -Xlinker -rpath -Xlinker "@loader_path/../../../Frameworks" 2>&1
  SLICE_OUTS="$SLICE_OUTS $SLICE_OUT"
done
# shellcheck disable=SC2086
lipo -create $SLICE_OUTS -output "$OUT"
rm -f $SLICE_OUTS
echo "[harpoon] architectures: $(lipo -archs "$OUT")" >&2
echo "[harpoon] signing with entitlement..." >&2
codesign --entitlements "$SCRIPT_DIR/entitlements.plist" --force -s - "$OUT" 2>&1
codesign -d --entitlements - "$OUT" 2>&1 | grep -q "com.apple.security.virtualization" && echo "[harpoon] entitlement OK" >&2 || { echo "[harpoon] entitlement missing!" >&2; exit 1; }
ls -lh "$OUT" >&2
echo "[harpoon] build complete: $OUT" >&2
