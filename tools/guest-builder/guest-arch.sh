# Guest architecture shared by the guest builder scripts (sourced, POSIX sh).
# HARPOON_GUEST_ARCH=aarch64 (default, Apple Silicon hosts) | x86_64 (Intel hosts).
# aarch64 assets live in assets/guest/, x86_64 assets in assets/guest/x86_64/.
export HARPOON_GUEST_ARCH="${HARPOON_GUEST_ARCH:-aarch64}"
GUEST_ALPINE_BASE="https://dl-cdn.alpinelinux.org/alpine/v3.22/releases/$HARPOON_GUEST_ARCH"
GUEST_MINIROOTFS="alpine-minirootfs-3.22.1-$HARPOON_GUEST_ARCH.tar.gz"
GUEST_KERNEL_VERSION="6.12.110-0-virt"
case "$HARPOON_GUEST_ARCH" in
  aarch64)
    GUEST_ASSET_SUBDIR=""
    GUEST_CONTAINER_PLATFORM="linux/arm64"
    GUEST_MUSL_LOADER="ld-musl-aarch64.so.1"
    GUEST_ELF_PATTERN="aarch64"
    EXPECTED_VMLINUZ_SHA="dd37c4460f933ec909ff022c34944ff7e1a5300aff99750e3bdc02cd4074c332"
    EXPECTED_INITRAMFS_VIRT_SHA="384f2d828bbbf237bc57d03c0b5bb371482f01868b5f5fea151a2022554b889c"
    EXPECTED_MODLOOP_SHA="da142869626ec9d4543ab46482af10b5ff2c8c1c2e01068ddf32360316a06218"
    EXPECTED_MINIROOTFS_SHA="188416d41f9f0c9a6e9427b75149e43ccf3a89587b2d27c9ad506e7ffca78d1c"
    ;;
  x86_64)
    GUEST_ASSET_SUBDIR="x86_64"
    GUEST_CONTAINER_PLATFORM="linux/amd64"
    GUEST_MUSL_LOADER="ld-musl-x86_64.so.1"
    GUEST_ELF_PATTERN="x86-64"
    EXPECTED_VMLINUZ_SHA="__X86_64_VMLINUZ_SHA__"
    EXPECTED_INITRAMFS_VIRT_SHA="__X86_64_INITRAMFS_VIRT_SHA__"
    EXPECTED_MODLOOP_SHA="__X86_64_MODLOOP_SHA__"
    EXPECTED_MINIROOTFS_SHA="__X86_64_MINIROOTFS_SHA__"
    ;;
  *)
    echo "[guest-arch] FAIL: unsupported HARPOON_GUEST_ARCH $HARPOON_GUEST_ARCH (aarch64|x86_64)" >&2
    exit 1
    ;;
esac

# guest_asset_dir <assets/guest root> -> directory holding this architecture's assets
guest_asset_dir() {
  if [ -n "$GUEST_ASSET_SUBDIR" ]; then printf '%s/%s\n' "$1" "$GUEST_ASSET_SUBDIR"; else printf '%s\n' "$1"; fi
}
