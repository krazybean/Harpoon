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
echo "[harpoon] building production runtime for macOS $HARPOON_MIN_MACOS (swift target arm64-apple-macosx$HARPOON_MIN_MACOS)..." >&2
xcrun swiftc -target "arm64-apple-macosx$HARPOON_MIN_MACOS" \
  "$SCRIPT_DIR/Sources/RuntimeConfig.swift" "$SCRIPT_DIR/Sources/HostPathTranslator.swift" "$SCRIPT_DIR/Sources/Lifecycle.swift" "$SCRIPT_DIR/Sources/VMManager.swift" "$SCRIPT_DIR/Sources/Bridges.swift" "$SCRIPT_DIR/Sources/PortForwardManager.swift" "$SCRIPT_DIR/Sources/HarpoonRuntimeState.swift" "$SCRIPT_DIR/Sources/HarpoonStatusCLI.swift" "$SCRIPT_DIR/Sources/HarpoonDoctorCLI.swift" "$SCRIPT_DIR/Sources/HarpoonGuestCLI.swift" "$SCRIPT_DIR/Sources/HarpoonDockerCLI.swift" "$SCRIPT_DIR/Sources/HarpoonConfigCLI.swift" "$SCRIPT_DIR/Sources/HarpoonDiskCLI.swift" "$SCRIPT_DIR/Sources/HarpoonMgmtCLI.swift" "$SCRIPT_DIR/Sources/HarpoonLifecycleCLI.swift" "$SCRIPT_DIR/Sources/UnixBridge.swift" "$SCRIPT_DIR/Sources/BalloonBridge.swift" "$SCRIPT_DIR/Sources/MgmtBridge.swift" "$SCRIPT_DIR/Sources/PortForwardBridge.swift" "$SCRIPT_DIR/Sources/HarpoonCLI.swift" "$SCRIPT_DIR/Sources/ContainerCLI.swift" "$SCRIPT_DIR/Sources/main.swift" \
  -framework Virtualization -o "$OUT" -module-cache-path /tmp/harpoon-mcache \
  -Xlinker -rpath -Xlinker "@executable_path/../../../Frameworks" \
  -Xlinker -rpath -Xlinker "@loader_path/../../../Frameworks" 2>&1
echo "[harpoon] signing with entitlement..." >&2
codesign --entitlements "$SCRIPT_DIR/entitlements.plist" --force -s - "$OUT" 2>&1
codesign -d --entitlements - "$OUT" 2>&1 | grep -q "com.apple.security.virtualization" && echo "[harpoon] entitlement OK" >&2 || { echo "[harpoon] entitlement missing!" >&2; exit 1; }
ls -lh "$OUT" >&2
echo "[harpoon] build complete: $OUT" >&2
