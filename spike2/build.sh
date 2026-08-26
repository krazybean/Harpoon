#!/bin/sh
set -eu
ROOT=/tmp/harpoon-docker-rootfs
INITRAMFS=spike2/cache/harpoon-docker-initramfs.cpio.gz
rm -rf $ROOT
mkdir -p $ROOT
echo "[spike2] extracting alpine-minirootfs..."
tar -xzf /tmp/alpine-minirootfs.tar.gz -C $ROOT
# Keep tar for block population (first boot populates ext4)
cp /tmp/alpine-minirootfs.tar.gz "$ROOT/alpine-minirootfs.tar.gz" 2>&1 | head -n 2
echo "[spike2] injecting virtio-net modules from Alpine initramfs-virt (CONFIG_VIRTIO_NET=m)..."
rm -rf /tmp/spike2_modprep
mkdir -p /tmp/spike2_modprep
gunzip -c spike1/cache/initramfs-virt | (cd /tmp/spike2_modprep && cpio -idm --quiet)
mkdir -p $ROOT/lib/modules
cp -a /tmp/spike2_modprep/lib/modules "$ROOT/lib/"
# ensure modprobe deps available
ls -lh "$ROOT/lib/modules/6.12.94-0-virt/kernel/drivers/net/virtio_net.ko" 2>&1 | head -n 2
echo "[spike2] injecting vsock modules (CONFIG_VSOCKETS=m, VIRTIO_VSOCKETS=m, VIRTIO_VSOCKETS_COMMON=m) from modloop-virt..."
# modloop-virt is SquashFS, use 7z to extract matching 6.12.94 modules (vsock not in initramfs-virt)
rm -rf /tmp/vsock_modprep
mkdir -p /tmp/vsock_modprep
if [ -f /tmp/modloop-virt ]; then
  7z x /tmp/modloop-virt -o/tmp/vsock_modprep -y >/dev/null 2>&1 || true
  mkdir -p "$ROOT/lib/modules/6.12.94-0-virt/kernel/net/vmw_vsock"
  cp -f /tmp/vsock_modprep/modules/6.12.94-0-virt/kernel/net/vmw_vsock/vsock.ko "$ROOT/lib/modules/6.12.94-0-virt/kernel/net/vmw_vsock/" 2>&1 || true
  cp -f /tmp/vsock_modprep/modules/6.12.94-0-virt/kernel/net/vmw_vsock/vmw_vsock_virtio_transport_common.ko "$ROOT/lib/modules/6.12.94-0-virt/kernel/net/vmw_vsock/" 2>&1 || true
  cp -f /tmp/vsock_modprep/modules/6.12.94-0-virt/kernel/net/vmw_vsock/vmw_vsock_virtio_transport.ko "$ROOT/lib/modules/6.12.94-0-virt/kernel/net/vmw_vsock/" 2>&1 || true
  cp -f /tmp/vsock_modprep/modules/6.12.94-0-virt/kernel/net/vmw_vsock/vsock_loopback.ko "$ROOT/lib/modules/6.12.94-0-virt/kernel/net/vmw_vsock/" 2>&1 || true
  cp -f /tmp/vsock_modprep/modules/6.12.94-0-virt/kernel/net/vmw_vsock/vsock_diag.ko "$ROOT/lib/modules/6.12.94-0-virt/kernel/net/vmw_vsock/" 2>&1 || true
  ls -lh "$ROOT/lib/modules/6.12.94-0-virt/kernel/net/vmw_vsock/" 2>&1 | head -n 10
  # Ensure module metadata coherent for BusyBox modprobe — depmod preferred, else copy from same modloop
  if command -v depmod >/dev/null 2>&1; then
    depmod -b "$ROOT" 6.12.94-0-virt 2>&1 | head -n 20
    echo "depmod used for 6.12.94-0-virt" 2>&1 | head -n 2
  else
    echo "depmod not available, copying coherent metadata from same 6.12.94-0-virt modloop" 2>&1 | head -n 2
    for f in modules.dep modules.dep.bin modules.alias modules.alias.bin modules.symbols modules.symbols.bin modules.softdep modules.order modules.builtin.bin modules.builtin.alias.bin; do
      cp -f "/tmp/vsock_modprep/modules/6.12.94-0-virt/$f" "$ROOT/lib/modules/6.12.94-0-virt/$f" 2>&1 || true
    done
    # verify vsock entries exist
    grep -q "vsock.ko" "$ROOT/lib/modules/6.12.94-0-virt/modules.dep" 2>&1 && echo "modules.dep now has vsock" 2>&1 | head -n 2 || echo "modules.dep missing vsock" 2>&1 | head -n 2
    grep "vsock.ko" "$ROOT/lib/modules/6.12.94-0-virt/modules.dep" 2>&1 | head -n 5
  # ext4 filesystem driver — CONFIG_EXT4_FS=m, JBD2=m, FS_MBCACHE=m, CRC16=m (modular, verified via boot/config-6.12.103-0-virt)
  echo "[spike2] injecting ext4 modules from same modloop..." 2>&1 | head -n 2
  mkdir -p "$ROOT/lib/modules/6.12.94-0-virt/kernel/fs/ext4"
  mkdir -p "$ROOT/lib/modules/6.12.94-0-virt/kernel/fs/jbd2"
  mkdir -p "$ROOT/lib/modules/6.12.94-0-virt/kernel/fs"
  mkdir -p "$ROOT/lib/modules/6.12.94-0-virt/kernel/lib"
  cp -f /tmp/vsock_modprep/modules/6.12.94-0-virt/kernel/fs/ext4/ext4.ko "$ROOT/lib/modules/6.12.94-0-virt/kernel/fs/ext4/" 2>&1 || true
  cp -f /tmp/vsock_modprep/modules/6.12.94-0-virt/kernel/fs/jbd2/jbd2.ko "$ROOT/lib/modules/6.12.94-0-virt/kernel/fs/jbd2/" 2>&1 || true
  cp -f /tmp/vsock_modprep/modules/6.12.94-0-virt/kernel/fs/mbcache.ko "$ROOT/lib/modules/6.12.94-0-virt/kernel/fs/" 2>&1 || true
  cp -f /tmp/vsock_modprep/modules/6.12.94-0-virt/kernel/lib/crc16.ko "$ROOT/lib/modules/6.12.94-0-virt/kernel/lib/" 2>&1 || true
  ls -lh "$ROOT/lib/modules/6.12.94-0-virt/kernel/fs/ext4/ext4.ko" "$ROOT/lib/modules/6.12.94-0-virt/kernel/fs/jbd2/jbd2.ko" "$ROOT/lib/modules/6.12.94-0-virt/kernel/fs/mbcache.ko" "$ROOT/lib/modules/6.12.94-0-virt/kernel/lib/crc16.ko" 2>&1 | head -n 10
  grep -q "ext4.ko" "$ROOT/lib/modules/6.12.94-0-virt/modules.dep" 2>&1 && echo "modules.dep now has ext4" 2>&1 | head -n 2 || echo "modules.dep missing ext4" 2>&1 | head -n 2
  grep "ext4.ko" "$ROOT/lib/modules/6.12.94-0-virt/modules.dep" 2>&1 | head -n 5
  # --- Spike 3: Docker bridge networking modules (all 6.12.94-0-virt, verified modular not builtin) ---
  echo "[spike3] injecting Docker bridge/NAT modules from modloop-virt..." 2>&1 | head -n 2
  # stp, llc (bridge deps), bridge, br_netfilter
  mkdir -p "$ROOT/lib/modules/6.12.94-0-virt/kernel/net/802" "$ROOT/lib/modules/6.12.94-0-virt/kernel/net/llc" "$ROOT/lib/modules/6.12.94-0-virt/kernel/net/bridge" "$ROOT/lib/modules/6.12.94-0-virt/kernel/net/bridge/netfilter"
  cp -f /tmp/vsock_modprep/modules/6.12.94-0-virt/kernel/net/802/stp.ko "$ROOT/lib/modules/6.12.94-0-virt/kernel/net/802/" 2>&1 | head -n 2 || true
  cp -f /tmp/vsock_modprep/modules/6.12.94-0-virt/kernel/net/llc/llc.ko "$ROOT/lib/modules/6.12.94-0-virt/kernel/net/llc/" 2>&1 | head -n 2 || true
  cp -f /tmp/vsock_modprep/modules/6.12.94-0-virt/kernel/net/bridge/bridge.ko "$ROOT/lib/modules/6.12.94-0-virt/kernel/net/bridge/" 2>&1 | head -n 2 || true
  cp -f /tmp/vsock_modprep/modules/6.12.94-0-virt/kernel/net/bridge/br_netfilter.ko "$ROOT/lib/modules/6.12.94-0-virt/kernel/net/bridge/" 2>&1 | head -n 2 || true
  # veth, overlay, tun (veth for container veth pairs, overlay for storage, tun optional)
  mkdir -p "$ROOT/lib/modules/6.12.94-0-virt/kernel/drivers/net" "$ROOT/lib/modules/6.12.94-0-virt/kernel/fs/overlayfs"
  cp -f /tmp/vsock_modprep/modules/6.12.94-0-virt/kernel/drivers/net/veth.ko "$ROOT/lib/modules/6.12.94-0-virt/kernel/drivers/net/" 2>&1 | head -n 2 || true
  cp -f /tmp/vsock_modprep/modules/6.12.94-0-virt/kernel/fs/overlayfs/overlay.ko "$ROOT/lib/modules/6.12.94-0-virt/kernel/fs/overlayfs/" 2>&1 | head -n 2 || true
  # iptables/nftables dependencies — copy full net tree minimal required (ensures modprobe finds deps)
  # copy net/netfilter, net/ipv4/netfilter, net/ipv6/netfilter, net/netfilter/ipset, lib/libcrc32c
  mkdir -p "$ROOT/lib/modules/6.12.94-0-virt/kernel/net/netfilter" "$ROOT/lib/modules/6.12.94-0-virt/kernel/net/ipv4/netfilter" "$ROOT/lib/modules/6.12.94-0-virt/kernel/net/ipv6/netfilter" "$ROOT/lib/modules/6.12.94-0-virt/kernel/lib"
  # libcrc32c and defrag helpers (prereq for conntrack)
  cp -f /tmp/vsock_modprep/modules/6.12.94-0-virt/kernel/lib/libcrc32c.ko "$ROOT/lib/modules/6.12.94-0-virt/kernel/lib/" 2>&1 | head -n 2 || true
  cp -f /tmp/vsock_modprep/modules/6.12.94-0-virt/kernel/net/ipv4/netfilter/nf_defrag_ipv4.ko "$ROOT/lib/modules/6.12.94-0-virt/kernel/net/ipv4/netfilter/" 2>&1 | head -n 2 || true
  cp -f /tmp/vsock_modprep/modules/6.12.94-0-virt/kernel/net/ipv6/netfilter/nf_defrag_ipv6.ko "$ROOT/lib/modules/6.12.94-0-virt/kernel/net/ipv6/netfilter/" 2>&1 | head -n 2 || true
  # copy all netfilter relevant kmods (small minimal set covering Docker iptables rules: nat, filter, conntrack, xt_* )
  cp -a /tmp/vsock_modprep/modules/6.12.94-0-virt/kernel/net/netfilter/*.ko "$ROOT/lib/modules/6.12.94-0-virt/kernel/net/netfilter/" 2>&1 | head -n 2 || true
  cp -a /tmp/vsock_modprep/modules/6.12.94-0-virt/kernel/net/ipv4/netfilter/*.ko "$ROOT/lib/modules/6.12.94-0-virt/kernel/net/ipv4/netfilter/" 2>&1 | head -n 2 || true
  cp -a /tmp/vsock_modprep/modules/6.12.94-0-virt/kernel/net/ipv6/netfilter/*.ko "$ROOT/lib/modules/6.12.94-0-virt/kernel/net/ipv6/netfilter/" 2>&1 | head -n 2 || true
  # ipset dep for xt_set (used by Docker in some rules)
  mkdir -p "$ROOT/lib/modules/6.12.94-0-virt/kernel/net/netfilter/ipset"
  cp -a /tmp/vsock_modprep/modules/6.12.94-0-virt/kernel/net/netfilter/ipset/*.ko "$ROOT/lib/modules/6.12.94-0-virt/kernel/net/netfilter/ipset/" 2>&1 | head -n 2 || true
  ls -lh "$ROOT/lib/modules/6.12.94-0-virt/kernel/net/bridge/bridge.ko" "$ROOT/lib/modules/6.12.94-0-virt/kernel/drivers/net/veth.ko" "$ROOT/lib/modules/6.12.94-0-virt/kernel/fs/overlayfs/overlay.ko" "$ROOT/lib/modules/6.12.94-0-virt/kernel/net/netfilter/nf_conntrack.ko" "$ROOT/lib/modules/6.12.94-0-virt/kernel/net/netfilter/nf_nat.ko" 2>&1 | head -n 10
  grep -q "bridge.ko" "$ROOT/lib/modules/6.12.94-0-virt/modules.dep" 2>&1 && echo "modules.dep now has bridge" 2>&1 | head -n 2 || echo "modules.dep missing bridge" 2>&1 | head -n 2
  grep "veth.ko\|bridge.ko\|overlay.ko" "$ROOT/lib/modules/6.12.94-0-virt/modules.dep" 2>&1 | head -n 10
  # Spike 4: inject virtiofs/fuse (both modular, virtiofs depends on fuse)
  echo "[spike4] injecting virtiofs/fuse from same modloop..." 2>&1 | head -n 2
  mkdir -p "$ROOT/lib/modules/6.12.94-0-virt/kernel/fs/fuse"
  cp -f /tmp/vsock_modprep/modules/6.12.94-0-virt/kernel/fs/fuse/fuse.ko "$ROOT/lib/modules/6.12.94-0-virt/kernel/fs/fuse/" 2>&1 | head -n 2 || true
  cp -f /tmp/vsock_modprep/modules/6.12.94-0-virt/kernel/fs/fuse/virtiofs.ko "$ROOT/lib/modules/6.12.94-0-virt/kernel/fs/fuse/" 2>&1 | head -n 2 || true
  ls -lh "$ROOT/lib/modules/6.12.94-0-virt/kernel/fs/fuse/fuse.ko" "$ROOT/lib/modules/6.12.94-0-virt/kernel/fs/fuse/virtiofs.ko" 2>&1 | head -n 10
  grep -q "virtiofs.ko" "$ROOT/lib/modules/6.12.94-0-virt/modules.dep" 2>&1 && echo "modules.dep now has virtiofs" 2>&1 | head -n 2 || echo "modules.dep missing virtiofs" 2>&1 | head -n 2
  # Spike 5: inject virtio_balloon (modular, 36K, dep none — for memory reclamation)
  echo "[spike5] injecting virtio_balloon from same modloop..." 2>&1 | head -n 2
  mkdir -p "$ROOT/lib/modules/6.12.94-0-virt/kernel/drivers/virtio"
  cp -f /tmp/vsock_modprep/modules/6.12.94-0-virt/kernel/drivers/virtio/virtio_balloon.ko "$ROOT/lib/modules/6.12.94-0-virt/kernel/drivers/virtio/" 2>&1 | head -n 2 || true
  ls -lh "$ROOT/lib/modules/6.12.94-0-virt/kernel/drivers/virtio/virtio_balloon.ko" 2>&1 | head -n 10
  grep -q "virtio_balloon.ko" "$ROOT/lib/modules/6.12.94-0-virt/modules.dep" 2>&1 && echo "modules.dep now has virtio_balloon" 2>&1 | head -n 2 || echo "modules.dep missing virtio_balloon" 2>&1 | head -n 2
  fi
else
  echo "modloop-virt not found, skipping vsock injection" 2>&1 | head -n 2
fi
echo "[spike2] preparing apk..."
# Alpine minirootfs already has apk, we need to run apk inside rootfs via chroot or via apk --root
# Use apk --root with --arch aarch64 if needed, but host is arm64 so apk should work natively? Host is macOS, not Linux, so apk binary is for aarch64 musl, won't run on macOS.
# Instead we need to use qemu or docker to run apk, or just create a minimal init that will install docker at boot via apk in guest.
# For spike, create initramfs that at boot will apk add docker.
# So we just add an /init that does apk add
cat > $ROOT/init <<'INIT'
#!/bin/sh
# Harpoon Spike 2 guest init — Alpine 3.22 explicit repos, fail-fast APK, vsock, no TCP
set -u
mount -t proc none /proc
mount -t sysfs none /sys
mount -t devtmpfs none /dev
mkdir -p /var/run /tmp /sys/fs/cgroup
mount -t cgroup2 none /sys/fs/cgroup 2>/dev/null || true
# Block-backed root — pivot from initramfs tmpfs to ext4 virtio-blk for normal pivot_root
# Production prefers block-backed writable root for Docker; spike ramdisk used pivot_root workaround before
ROOTFS_TYPE=$(stat -f -c '%T' / 2>/dev/null || stat -f '%T' / 2>/dev/null || stat -c '%T' / 2>/dev/null || echo "unknown")
echo "HARPOON_ROOTFS_TYPE $ROOTFS_TYPE" > /dev/hvc0
if [ "$ROOTFS_TYPE" = "tmpfs" ] || [ "$ROOTFS_TYPE" = "rootfs" ] || echo "$ROOTFS_TYPE" | grep -q "tmpfs"; then
  echo "HARPOON_BLOCK_TRY_PIVOT" > /dev/hvc0
  # Load virtio-blk driver if needed (built-in on Alpine virt, but ensure)
  modprobe virtio_blk 2>&1 | tee /dev/hvc0 >/dev/null || insmod /lib/modules/6.12.94-0-virt/kernel/drivers/block/virtio_blk.ko 2>&1 | tee /dev/hvc0 >/dev/null || true
  sleep 1
  # Wait for virtio block device
  for i in $(seq 1 10); do
    [ -b /dev/vda ] && break
    [ -b /dev/vda1 ] && break
    sleep 1
  done
  ls -l /dev/vda* 2>&1 | tee /dev/hvc0 >/dev/null || echo "no /dev/vda" > /dev/hvc0
  if [ -b /dev/vda ]; then
    # ext4 filesystem driver — CONFIG_EXT4_FS=m, JBD2=m, FS_MBCACHE=m, CRC16=m — modular, not builtin
    echo "HARPOON_EXT4_MODPROBE_START" > /dev/hvc0
    ls -lh /lib/modules/6.12.94-0-virt/kernel/fs/ext4/ext4.ko /lib/modules/6.12.94-0-virt/kernel/fs/jbd2/jbd2.ko /lib/modules/6.12.94-0-virt/kernel/fs/mbcache.ko /lib/modules/6.12.94-0-virt/kernel/lib/crc16.ko 2>&1 | tee /dev/hvc0 >/dev/null || true
    EXT4_MODPROBE_OK=1
    if command -v modprobe >/dev/null 2>&1; then
      if ! modprobe crc16 > /tmp/modprobe_crc16.log 2>&1; then cat /tmp/modprobe_crc16.log 2>&1 | tee /dev/hvc0 >/dev/null || true; EXT4_MODPROBE_OK=0; fi
      if ! modprobe mbcache > /tmp/modprobe_mbcache.log 2>&1; then cat /tmp/modprobe_mbcache.log 2>&1 | tee /dev/hvc0 >/dev/null || true; EXT4_MODPROBE_OK=0; fi
      if ! modprobe jbd2 > /tmp/modprobe_jbd2.log 2>&1; then cat /tmp/modprobe_jbd2.log 2>&1 | tee /dev/hvc0 >/dev/null || true; EXT4_MODPROBE_OK=0; fi
      if ! modprobe ext4 > /tmp/modprobe_ext4.log 2>&1; then cat /tmp/modprobe_ext4.log 2>&1 | tee /dev/hvc0 >/dev/null || true; EXT4_MODPROBE_OK=0; fi
    else
      EXT4_MODPROBE_OK=0
    fi
    if [ $EXT4_MODPROBE_OK -eq 0 ]; then
      echo "modprobe ext4 failed or unavailable, trying direct insmod in dep order" > /dev/hvc0
      if insmod /lib/modules/6.12.94-0-virt/kernel/lib/crc16.ko > /tmp/insmod_crc16.log 2>&1; then
        echo "HARPOON_EXT4_INSMOD_CRC16_OK" > /dev/hvc0
      else
        ERR=$(cat /tmp/insmod_crc16.log 2>&1 | head -n 5 | tr '\n' ' ')
        echo "HARPOON_EXT4_INSMOD_FAILED crc16 $ERR" > /dev/hvc0
        cat /tmp/insmod_crc16.log 2>&1 | tee /dev/hvc0 >/dev/null || true
      fi
      if insmod /lib/modules/6.12.94-0-virt/kernel/fs/mbcache.ko > /tmp/insmod_mbcache.log 2>&1; then
        echo "HARPOON_EXT4_INSMOD_MBCACHE_OK" > /dev/hvc0
      else
        ERR=$(cat /tmp/insmod_mbcache.log 2>&1 | head -n 5 | tr '\n' ' ')
        echo "HARPOON_EXT4_INSMOD_FAILED mbcache $ERR" > /dev/hvc0
        cat /tmp/insmod_mbcache.log 2>&1 | tee /dev/hvc0 >/dev/null || true
      fi
      if insmod /lib/modules/6.12.94-0-virt/kernel/fs/jbd2/jbd2.ko > /tmp/insmod_jbd2.log 2>&1; then
        echo "HARPOON_EXT4_INSMOD_JBD2_OK" > /dev/hvc0
      else
        ERR=$(cat /tmp/insmod_jbd2.log 2>&1 | head -n 5 | tr '\n' ' ')
        echo "HARPOON_EXT4_INSMOD_FAILED jbd2 $ERR" > /dev/hvc0
        cat /tmp/insmod_jbd2.log 2>&1 | tee /dev/hvc0 >/dev/null || true
      fi
      if insmod /lib/modules/6.12.94-0-virt/kernel/fs/ext4/ext4.ko > /tmp/insmod_ext4.log 2>&1; then
        echo "HARPOON_EXT4_INSMOD_EXT4_OK" > /dev/hvc0
      else
        ERR=$(cat /tmp/insmod_ext4.log 2>&1 | head -n 5 | tr '\n' ' ')
        echo "HARPOON_EXT4_INSMOD_FAILED ext4 $ERR" > /dev/hvc0
        cat /tmp/insmod_ext4.log 2>&1 | tee /dev/hvc0 >/dev/null || true
      fi
    else
      echo "HARPOON_EXT4_INSMOD_CRC16_OK" > /dev/hvc0
      echo "HARPOON_EXT4_INSMOD_MBCACHE_OK" > /dev/hvc0
      echo "HARPOON_EXT4_INSMOD_JBD2_OK" > /dev/hvc0
      echo "HARPOON_EXT4_INSMOD_EXT4_OK" > /dev/hvc0
    fi
    echo "HARPOON_EXT4_MODPROBE_DONE" > /dev/hvc0
    grep -w ext4 /proc/filesystems 2>&1 | tee /dev/hvc0 >/dev/null || echo "no ext4 in /proc/filesystems" > /dev/hvc0
    lsmod 2>&1 | grep -E 'ext4|jbd2|mbcache' | tee /dev/hvc0 >/dev/null || echo "lsmod no ext4" > /dev/hvc0
    if ! grep -qw ext4 /proc/filesystems 2>/dev/null; then
      echo "HARPOON_BLOCK_MOUNT_FAILED ext4 unavailable" > /dev/hvc0
      echo "HARPOON_DOCKER_FAILED ext4 unavailable lsmod=$(lsmod 2>&1 | grep -E 'ext4|jbd2|mbcache' | head -n 5)" > /dev/hvc0
      while true; do sleep 5; echo "HARPOON_BLOCK_MOUNT_FAILED ext4 unavailable" > /dev/hvc0; done
    fi
    mkdir -p /newroot
    # ext4 may need fsck if unclean, but try mount
    mount -t ext4 /dev/vda /newroot 2>&1 | tee /dev/hvc0 >/dev/null || mount -t ext4 -o ro /dev/vda /newroot 2>&1 | tee /dev/hvc0 >/dev/null || echo "mount vda failed" > /dev/hvc0
    if grep -q " /newroot " /proc/mounts 2>/dev/null || mount | grep -q " on /newroot "; then
      echo "HARPOON_BLOCK_MOUNTED /dev/vda -> /newroot" > /dev/hvc0
      # Populate if empty (first boot)
      if [ ! -f /newroot/bin/sh ] || [ ! -f /newroot/sbin/init ]; then
        echo "HARPOON_BLOCK_POPULATE" > /dev/hvc0
        tar -xzf /alpine-minirootfs.tar.gz -C /newroot 2>&1 | tee /dev/hvc0 >/dev/null || echo "populate tar failed" > /dev/hvc0
        # copy kernel modules tree for vsock/virtio
        mkdir -p /newroot/lib/modules
        cp -a /lib/modules/6.12.94-0-virt /newroot/lib/modules/ 2>&1 | tee /dev/hvc0 >/dev/null || true
        # also copy init script itself to newroot for second stage (so switch_root has Docker logic)
        cp /init /newroot/init 2>&1 | tee /dev/hvc0 >/dev/null || true
        chmod +x /newroot/init 2>&1 | tee /dev/hvc0 >/dev/null || true
        # ensure apk repos
        mkdir -p /newroot/etc/apk
        cat > /newroot/etc/apk/repositories <<'REPOS2'
https://dl-cdn.alpinelinux.org/alpine/v3.22/main
https://dl-cdn.alpinelinux.org/alpine/v3.22/community
REPOS2
        ls -l /newroot/bin/sh 2>&1 | tee /dev/hvc0 >/dev/null || true
      fi
      # Harpoon: always refresh final ext4 init to current (fixes stale block image without wipe)
      cp /init /newroot/init 2>&1 | tee /dev/hvc0 >/dev/null || true
      chmod +x /newroot/init 2>&1 | tee /dev/hvc0 >/dev/null || true
      echo "HARPOON_INIT_REFRESHED" > /dev/hvc0
      # Harpoon: always refresh final ext4 module tree to current initramfs (fixes stale harpoon-root.img incomplete module fileset)
      # Do not wipe disk; copy the smallest coherent subtrees needed for Docker networking from same 6.12.94-0-virt modloop
      # Exact paths verified from modloop (Phase 1): llc, stp, bridge, br_netfilter, veth, libcrc32c, etc. plus deps nf_defrag, etc.
      echo "HARPOON_MODULE_REFRESH_START" > /dev/hvc0
      mkdir -p /newroot/lib/modules
      # Remove stale incomplete tree and copy current coherent tree (initramfs has correct files + modules.dep)
      rm -rf /newroot/lib/modules/6.12.94-0-virt
      cp -a /lib/modules/6.12.94-0-virt /newroot/lib/modules/ 2>&1 | tee /dev/hvc0 >/dev/null || echo "HARPOON_MODULE_REFRESH_FAILED" > /dev/hvc0
      ls -lh /newroot/lib/modules/6.12.94-0-virt/kernel/net/llc/llc.ko /newroot/lib/modules/6.12.94-0-virt/kernel/lib/libcrc32c.ko /newroot/lib/modules/6.12.94-0-virt/kernel/drivers/net/veth.ko /newroot/lib/modules/6.12.94-0-virt/kernel/net/netfilter/x_tables.ko 2>&1 | tee /dev/hvc0 >/dev/null || true
      echo "HARPOON_MODULE_REFRESH_DONE" > /dev/hvc0
      # Verify FINAL ext4 root contains required files before switch_root (do not guess, fail if absent)
      for modpath in kernel/net/llc/llc.ko kernel/net/802/stp.ko kernel/net/bridge/bridge.ko kernel/net/bridge/br_netfilter.ko kernel/drivers/net/veth.ko kernel/lib/libcrc32c.ko kernel/net/netfilter/nf_conntrack.ko kernel/net/netfilter/nf_nat.ko kernel/net/netfilter/x_tables.ko kernel/net/ipv4/netfilter/ip_tables.ko kernel/net/ipv4/netfilter/iptable_nat.ko kernel/net/ipv4/netfilter/iptable_filter.ko kernel/net/netfilter/xt_MASQUERADE.ko kernel/net/netfilter/xt_addrtype.ko kernel/net/netfilter/xt_conntrack.ko kernel/net/netfilter/xt_comment.ko kernel/net/netfilter/xt_tcpudp.ko kernel/net/ipv4/netfilter/nf_defrag_ipv4.ko kernel/net/ipv6/netfilter/nf_defrag_ipv6.ko kernel/lib/crc16.ko kernel/fs/mbcache.ko kernel/fs/jbd2/jbd2.ko kernel/fs/ext4/ext4.ko kernel/net/vmw_vsock/vsock.ko kernel/fs/fuse/fuse.ko kernel/fs/fuse/virtiofs.ko kernel/drivers/virtio/virtio_balloon.ko; do
        if [ -f "/newroot/lib/modules/6.12.94-0-virt/$modpath" ]; then
          echo "HARPOON_MODULE_FILE $modpath OK" > /dev/hvc0
        else
          echo "HARPOON_MODULE_FILE $modpath MISSING" > /dev/hvc0
        fi
      done
      # Mandatory gates (spec Phase 3): llc, veth, libcrc32c, x_tables
      for req in llc veth libcrc32c x_tables; do
        case "$req" in
          llc) f="kernel/net/llc/llc.ko" ;;
          veth) f="kernel/drivers/net/veth.ko" ;;
          libcrc32c) f="kernel/lib/libcrc32c.ko" ;;
          x_tables) f="kernel/net/netfilter/x_tables.ko" ;;
        esac
        if [ ! -f "/newroot/lib/modules/6.12.94-0-virt/$f" ]; then
          echo "HARPOON_DOCKER_FAILED module_file_missing $f" > /dev/hvc0
          while true; do sleep 5; echo "HARPOON_DOCKER_FAILED module_file_missing $f" > /dev/hvc0; done
        fi
      done
      # Prepare newroot mounts for switch_root
      mkdir -p /newroot/proc /newroot/sys /newroot/dev /newroot/tmp /newroot/run
      mount --move /proc /newroot/proc 2>&1 | tee /dev/hvc0 >/dev/null || mount -t proc none /newroot/proc 2>&1 | tee /dev/hvc0 >/dev/null || true
      mount --move /sys /newroot/sys 2>&1 | tee /dev/hvc0 >/dev/null || mount -t sysfs none /newroot/sys 2>&1 | tee /dev/hvc0 >/dev/null || true
      mount --move /dev /newroot/dev 2>&1 | tee /dev/hvc0 >/dev/null || mount -t devtmpfs none /newroot/dev 2>&1 | tee /dev/hvc0 >/dev/null || true
      mkdir -p /newroot/sys/fs/cgroup
      mount -t cgroup2 none /newroot/sys/fs/cgroup 2>/dev/null || true
      echo "HARPOON_PIVOT" > /dev/hvc0
      # Use switch_root if available (BusyBox), else pivot
      if command -v switch_root >/dev/null 2>&1; then
        exec switch_root /newroot /init
      else
        # fallback: pivot_root
        mkdir -p /newroot/oldroot
        pivot_root /newroot /newroot/oldroot 2>&1 | tee /dev/hvc0 >/dev/null || echo "pivot_root failed" > /dev/hvc0
        exec chroot /newroot /init
      fi
    else
      echo "HARPOON_BLOCK_MOUNT_FAILED" > /dev/hvc0
    fi
  else
    echo "HARPOON_BLOCK_NO_DEVICE" > /dev/hvc0
  fi
fi
# If we reach here, we are on block-backed root (ext4) — confirm
# Phase 1 — Confirm root filesystem (now should be ext4 after pivot)
echo "HARPOON_ROOTFS" > /dev/hvc0
mount 2>&1 | grep ' on / ' | tee /dev/hvc0 >/dev/null || mount | tee /dev/hvc0 >/dev/null || true
# stat format varies (BusyBox coreutils): try multiple
ROOTFS_T2=$(stat -f -c '%T' / 2>&1 | head -n 1)
echo "$ROOTFS_T2" | tee /dev/hvc0 >/dev/null || true
stat -f '%T' / 2>&1 | tee /dev/hvc0 >/dev/null || stat -c '%T' / 2>&1 | tee /dev/hvc0 >/dev/null || echo "stat not available" > /dev/hvc0
# Verify ext4 after pivot — fail fast if still tmpfs/rootfs
if echo "$ROOTFS_T2" | grep -qE "tmpfs|rootfs"; then
  echo "HARPOON_DOCKER_FAILED rootfs_not_ext4 type=$ROOTFS_T2" > /dev/hvc0
  while true; do sleep 5; echo "HARPOON_DOCKER_FAILED rootfs_not_ext4" > /dev/hvc0; done
fi
echo "HARPOON_DOCKER_INIT_START" > /dev/hvc0
# --- Spike 3 FINAL ext4 runtime networking (must run in final ext4 root before dockerd) ---
echo "HARPOON_NET_RUNTIME_START" > /dev/hvc0
# load already-injected bridge/NAT modules (capture errors, do not silently ignore)
ERR_NET=0
for mod in bridge br_netfilter veth overlay nf_conntrack nf_nat x_tables ip_tables iptable_nat iptable_filter xt_MASQUERADE xt_addrtype xt_conntrack xt_comment xt_tcpudp; do
  if ! modprobe "$mod" > /tmp/modprobe_$mod.log 2>&1; then
    cat /tmp/modprobe_$mod.log 2>&1 | tee /dev/hvc0 >/dev/null || true
    echo "HARPOON_MODPROBE_FAILED $mod $(cat /tmp/modprobe_$mod.log 2>&1 | head -n 5)" > /dev/hvc0
    ERR_NET=1
  else
    echo "HARPOON_MODPROBE_OK $mod" > /dev/hvc0
  fi
done
if [ $ERR_NET -ne 0 ]; then
  echo "HARPOON_DOCKER_FAILED net_modprobe" > /dev/hvc0
fi
# enable IPv4 forwarding (smallest guest-runtime mechanism)
if [ -f /proc/sys/net/ipv4/ip_forward ]; then
  echo 1 > /proc/sys/net/ipv4/ip_forward 2>&1 | tee /dev/hvc0 >/dev/null || true
fi
IPF=$(cat /proc/sys/net/ipv4/ip_forward 2>&1 | head -n 1 | tr -d " \n")
echo "HARPOON_IP_FORWARD $IPF" > /dev/hvc0
if [ "$IPF" != "1" ]; then
  echo "HARPOON_DOCKER_FAILED ip_forward got=$IPF" > /dev/hvc0
  while true; do sleep 5; echo "HARPOON_DOCKER_FAILED ip_forward" > /dev/hvc0; done
fi
# bridge-nf-call sysctls if present (do not fail solely if absent)
if [ -f /proc/sys/net/bridge/bridge-nf-call-iptables ]; then
  echo 1 > /proc/sys/net/bridge/bridge-nf-call-iptables 2>&1 | tee /dev/hvc0 >/dev/null || echo "HARPOON_BRIDGE_NF_IPTABLES_FAILED" > /dev/hvc0
fi
if [ -f /proc/sys/net/bridge/bridge-nf-call-ip6tables ]; then
  echo 1 > /proc/sys/net/bridge/bridge-nf-call-ip6tables 2>&1 | tee /dev/hvc0 >/dev/null || echo "HARPOON_BRIDGE_NF_IP6TABLES_FAILED" > /dev/hvc0
fi
ip link set lo up 2>&1 | tee /dev/hvc0 >/dev/null
# --- Spike 4 VirtioFS host bind mount (FINAL ext4 runtime) ---
echo "HARPOON_VIRTIOFS_MODULE_START" > /dev/hvc0
ls -lh /lib/modules/6.12.94-0-virt/kernel/fs/fuse/fuse.ko /lib/modules/6.12.94-0-virt/kernel/fs/fuse/virtiofs.ko 2>&1 | tee /dev/hvc0 >/dev/null || true
VIRTIOFS_OK=1
if ! modprobe fuse > /tmp/modprobe_fuse.log 2>&1; then
  cat /tmp/modprobe_fuse.log 2>&1 | tee /dev/hvc0 >/dev/null || true
  echo "HARPOON_VIRTIOFS_FAILED fuse $(cat /tmp/modprobe_fuse.log 2>&1 | head -n 5)" > /dev/hvc0
  VIRTIOFS_OK=0
else
  echo "HARPOON_VIRTIOFS_MODULE_OK fuse" > /dev/hvc0
fi
if [ $VIRTIOFS_OK -eq 1 ]; then
  if ! modprobe virtiofs > /tmp/modprobe_virtiofs.log 2>&1; then
    cat /tmp/modprobe_virtiofs.log 2>&1 | tee /dev/hvc0 >/dev/null || true
    echo "HARPOON_VIRTIOFS_FAILED virtiofs $(cat /tmp/modprobe_virtiofs.log 2>&1 | head -n 5)" > /dev/hvc0
    VIRTIOFS_OK=0
  else
    echo "HARPOON_VIRTIOFS_MODULE_OK virtiofs" > /dev/hvc0
  fi
fi
if grep -q "virtiofs" /proc/filesystems 2>/dev/null || grep -q "fuse" /proc/filesystems 2>/dev/null; then
  echo "HARPOON_VIRTIOFS_FILESYSTEM_OK" > /dev/hvc0
else
  lsmod 2>&1 | grep -E "fuse|virtiofs" | tee /dev/hvc0 >/dev/null || true
  cat /proc/filesystems 2>&1 | tee /dev/hvc0 >/dev/null || true
  echo "HARPOON_VIRTIOFS_FAILED filesystem not present" > /dev/hvc0
  VIRTIOFS_OK=0
fi
echo "HARPOON_VIRTIOFS_MOUNT_START" > /dev/hvc0
mkdir -p /mnt/harpoon-share 2>&1 | tee /dev/hvc0 >/dev/null || true
if [ $VIRTIOFS_OK -eq 1 ]; then
  if mount -t virtiofs harpoon-share /mnt/harpoon-share 2>&1 | tee /dev/hvc0 >/dev/null; then
    echo "HARPOON_VIRTIOFS_MOUNTED /mnt/harpoon-share" > /dev/hvc0
    mount 2>&1 | grep harpoon-share | tee /dev/hvc0 >/dev/null || true
    cat /proc/mounts 2>&1 | grep harpoon-share | tee /dev/hvc0 >/dev/null || true
    # RW probe
    if touch /mnt/harpoon-share/.harpoon_rw_test 2>&1 && rm /mnt/harpoon-share/.harpoon_rw_test 2>&1; then
      echo "HARPOON_VIRTIOFS_RW_OK" > /dev/hvc0
    else
      echo "HARPOON_VIRTIOFS_FAILED rw $(ls -ld /mnt/harpoon-share 2>&1 | head -n 5)" > /dev/hvc0
    fi
  else
    echo "HARPOON_VIRTIOFS_FAILED mount $(dmesg 2>&1 | tail -n 20 | head -n 20)" > /dev/hvc0
  fi
else
  echo "HARPOON_VIRTIOFS_FAILED module" > /dev/hvc0
fi
# --- Spike 5 memory balloon + observability (FINAL ext4 runtime) ---
echo "HARPOON_BALLOON_DRIVER_START" > /dev/hvc0
ls -lh /lib/modules/6.12.94-0-virt/kernel/drivers/virtio/virtio_balloon.ko 2>&1 | tee /dev/hvc0 >/dev/null || true
if modprobe virtio_balloon > /tmp/modprobe_balloon.log 2>&1; then
  echo "HARPOON_BALLOON_DRIVER_OK virtio_balloon" > /dev/hvc0
  lsmod 2>&1 | grep balloon | tee /dev/hvc0 >/dev/null || true
else
  cat /tmp/modprobe_balloon.log 2>&1 | tee /dev/hvc0 >/dev/null || true
  echo "HARPOON_BALLOON_FAILED virtio_balloon $(cat /tmp/modprobe_balloon.log 2>&1 | head -n 5)" > /dev/hvc0
fi
# Guest memory baseline (do not spam, concise)
echo "HARPOON_MEMORY_GUEST_START" > /dev/hvc0
cat /proc/meminfo 2>&1 | head -n 20 | tee /dev/hvc0 >/dev/null || true
FREE_M=$(free -m 2>&1 | tee /dev/hvc0 >/dev/null || true; free -m 2>&1 | head -n 5)
# Emit concise KV for host to correlate (guest totals)
MEMTOTAL=$(awk '/MemTotal/ {print $2}' /proc/meminfo 2>/dev/null | head -n1)
MEMAVAIL=$(awk '/MemAvailable/ {print $2}' /proc/meminfo 2>/dev/null | head -n1)
echo "HARPOON_MEMORY_GUEST_TOTAL_KB $MEMTOTAL" > /dev/hvc0
echo "HARPOON_MEMORY_GUEST_AVAILABLE_KB $MEMAVAIL" > /dev/hvc0
# cgroup v2 state if present
cat /sys/fs/cgroup/memory.current 2>&1 | head -n 1 | tee /dev/hvc0 >/dev/null || true
cat /sys/fs/cgroup/memory.max 2>&1 | head -n 1 | tee /dev/hvc0 >/dev/null || true

# Explicit Alpine 3.22 repositories — do not use edge, do not mix branches
cat > /etc/apk/repositories <<'REPOS'
https://dl-cdn.alpinelinux.org/alpine/v3.22/main
https://dl-cdn.alpinelinux.org/alpine/v3.22/community
REPOS
echo "HARPOON_APK_REPOSITORIES" > /dev/hvc0
cat /etc/apk/repositories 2>&1 | tee /dev/hvc0 >/dev/null
ls -l /etc/apk/repositories 2>&1 | tee /dev/hvc0 >/dev/null

# Deterministic network diagnostics — required before HARPOON_DOCKER_FAILED
echo "HARPOON_NET_SYSFS" > /dev/hvc0
ls -la /sys/class/net 2>&1 | tee /dev/hvc0 >/dev/null
echo "HARPOON_NET_PCI" > /dev/hvc0
ls /sys/bus/pci/devices 2>&1 | tee /dev/hvc0 >/dev/null || true
lspci 2>&1 | tee /dev/hvc0 >/dev/null || true
# dmesg filtered for virtio/net/pci
if command -v dmesg >/dev/null 2>&1; then
  dmesg 2>&1 | grep -i -E "virtio|net|pci" | head -n 50 | tee /dev/hvc0 >/dev/null || true
fi

# Load virtio-net driver — kernel has CONFIG_VIRTIO=y, VIRTIO_PCI=y built-in, VIRTIO_NET=m
# Custom initramfs from minirootfs lacked modules; now injected from Alpine initramfs-virt
echo "HARPOON_NET_MODPROBE_START" > /dev/hvc0
ls -lh /lib/modules/6.12.94-0-virt/kernel/drivers/net/virtio_net.ko /lib/modules/6.12.94-0-virt/kernel/drivers/net/net_failover.ko /lib/modules/6.12.94-0-virt/kernel/net/core/failover.ko 2>&1 | tee /dev/hvc0 >/dev/null || true
# modprobe handles deps via modules.dep; fallback to insmod ordered
if command -v modprobe >/dev/null 2>&1; then
  modprobe failover 2>&1 | tee /dev/hvc0 >/dev/null || true
  modprobe net_failover 2>&1 | tee /dev/hvc0 >/dev/null || true
  modprobe virtio_net 2>&1 | tee /dev/hvc0 >/dev/null || echo "modprobe virtio_net rc=$?" > /dev/hvc0
else
  insmod /lib/modules/6.12.94-0-virt/kernel/net/core/failover.ko 2>&1 | tee /dev/hvc0 >/dev/null || true
  insmod /lib/modules/6.12.94-0-virt/kernel/drivers/net/net_failover.ko 2>&1 | tee /dev/hvc0 >/dev/null || true
  insmod /lib/modules/6.12.94-0-virt/kernel/drivers/net/virtio_net.ko 2>&1 | tee /dev/hvc0 >/dev/null || echo "insmod virtio_net rc=$?" > /dev/hvc0
fi
sleep 1
ls -la /sys/class/net 2>&1 | tee /dev/hvc0 >/dev/null
echo "HARPOON_NET_MODPROBE_DONE" > /dev/hvc0

# Networking — VZNAT virtio-net may be eth0, enp0s1, ens3, etc. Discover first non-lo.
echo "HARPOON_NET_START" > /dev/hvc0
ip link 2>&1 | tee /dev/hvc0 >/dev/null
ls /sys/class/net 2>&1 | tee /dev/hvc0 >/dev/null
IFACE=""
for i in $(seq 1 10); do
  for cand in $(ls /sys/class/net 2>/dev/null); do
    [ "$cand" = "lo" ] && continue
    if [ -e "/sys/class/net/$cand" ]; then
      IFACE="$cand"
      break
    fi
  done
  [ -n "$IFACE" ] && break
  sleep 1
done
if [ -z "$IFACE" ]; then
  echo "HARPOON_NET_FAILED no interface found" > /dev/hvc0
  echo "HARPOON_DOCKER_FAILED no network interface" > /dev/hvc0
  while true; do sleep 5; echo "HARPOON_NET_FAILED" > /dev/hvc0; done
fi
echo "HARPOON_NET_IFACE $IFACE" > /dev/hvc0
# bring up iface and DHCP — udhcpc from busybox
ip link set "$IFACE" up 2>&1 | tee /dev/hvc0 >/dev/null
udhcpc -i "$IFACE" > /tmp/udhcpc.log 2>&1 &
UDHCPC_PID=$!
echo "udhcpc -i $IFACE pid=$UDHCPC_PID" > /dev/hvc0
# wait up to 10s for inet
for i in $(seq 1 10); do
  ip -4 addr show "$IFACE" 2>/dev/null | grep -q "inet " && break
  sleep 1
done
ip -4 addr show "$IFACE" 2>&1 | tee /dev/hvc0 >/dev/null
cat /tmp/udhcpc.log 2>&1 | tee /dev/hvc0 >/dev/null || true
# verify we have connectivity before apk — fail fast if no inet
if ! ip -4 addr show "$IFACE" 2>/dev/null | grep -q "inet "; then
  echo "HARPOON_NET_FAILED no inet on $IFACE" > /dev/hvc0
  echo "HARPOON_DOCKER_FAILED no inet on $IFACE" > /dev/hvc0
  while true; do sleep 5; echo "HARPOON_NET_FAILED" > /dev/hvc0; done
fi

# Phase 1 — Deterministic network diagnostics before apk
echo "HARPOON_NET_ADDR" > /dev/hvc0
ip addr show 2>&1 | tee /dev/hvc0 >/dev/null
echo "HARPOON_NET_ROUTE" > /dev/hvc0
ip route 2>&1 | tee /dev/hvc0 >/dev/null
echo "HARPOON_NET_RESOLV" > /dev/hvc0
cat /etc/resolv.conf 2>&1 | tee /dev/hvc0 >/dev/null || echo "no /etc/resolv.conf" > /dev/hvc0
# DHCP helper script status
echo "HARPOON_DHCP_HELPER" > /dev/hvc0
ls -l /usr/share/udhcpc/default.script /etc/udhcpc/udhcpc.conf 2>&1 | tee /dev/hvc0 >/dev/null || true
cat /usr/share/udhcpc/default.script 2>&1 | head -n 20 | tee /dev/hvc0 >/dev/null || true

# Gateway probe
if ping -c 1 -W 2 192.168.64.1 > /tmp/ping_gw.log 2>&1; then
  echo "HARPOON_GATEWAY_OK" > /dev/hvc0
else
  echo "HARPOON_GATEWAY_FAILED" > /dev/hvc0
  cat /tmp/ping_gw.log 2>&1 | tee /dev/hvc0 >/dev/null || true
fi
# Raw Internet IPv4
if ping -c 1 -W 2 1.1.1.1 > /tmp/ping_inet.log 2>&1; then
  echo "HARPOON_INET_OK" > /dev/hvc0
else
  echo "HARPOON_INET_FAILED" > /dev/hvc0
  cat /tmp/ping_inet.log 2>&1 | tee /dev/hvc0 >/dev/null || true
fi
# DNS — test DHCP-provided DNS first, fallback to public DNS on failure (guest /etc/resolv.conf only)
if command -v nslookup >/dev/null 2>&1; then
  if nslookup dl-cdn.alpinelinux.org > /tmp/nslookup.log 2>&1; then
    # DHCP DNS succeeded
    echo "HARPOON_DNS_OK" > /dev/hvc0
    cat /tmp/nslookup.log 2>&1 | tee /dev/hvc0 >/dev/null || true
  else
    echo "HARPOON_DNS_DHCP_FAILED" > /dev/hvc0
    cat /tmp/nslookup.log 2>&1 | tee /dev/hvc0 >/dev/null || true
    # Replace GUEST /etc/resolv.conf only — do NOT modify macOS host
    cat > /etc/resolv.conf <<'RESOLV'
nameserver 1.1.1.1
nameserver 8.8.8.8
RESOLV
    echo "HARPOON_DNS_OVERRIDE_APPLIED" > /dev/hvc0
    cat /etc/resolv.conf 2>&1 | tee /dev/hvc0 >/dev/null || true
    # Retry with overridden DNS
    if nslookup dl-cdn.alpinelinux.org > /tmp/nslookup2.log 2>&1; then
      RESOLVED=$(cat /tmp/nslookup2.log 2>&1 | grep -i "Address" | tail -n 1 | awk '{print $2}')
      echo "HARPOON_DNS_OK $RESOLVED" > /dev/hvc0
      cat /tmp/nslookup2.log 2>&1 | tee /dev/hvc0 >/dev/null || true
    else
      echo "HARPOON_DNS_FAILED" > /dev/hvc0
      cat /tmp/nslookup2.log 2>&1 | tee /dev/hvc0 >/dev/null || true
      echo "HARPOON_DNS_FAILED diagnostic resolv.conf=$(cat /etc/resolv.conf 2>&1 | head -n 10)" > /dev/hvc0
      echo "HARPOON_DOCKER_FAILED dns resolution failed" > /dev/hvc0
      while true; do sleep 5; echo "HARPOON_DNS_FAILED" > /dev/hvc0; done
    fi
  fi
else
  echo "HARPOON_DNS_FAILED nslookup not found" > /dev/hvc0
  echo "HARPOON_DOCKER_FAILED dns resolution failed" > /dev/hvc0
  while true; do sleep 5; echo "HARPOON_DNS_FAILED" > /dev/hvc0; done
fi
# HTTPS/TLS probe independent of apk
if command -v wget >/dev/null 2>&1; then
  if wget -S --spider https://dl-cdn.alpinelinux.org/alpine/v3.22/main/aarch64/APKINDEX.tar.gz > /tmp/wget.log 2>&1; then
    echo "HARPOON_HTTPS_OK" > /dev/hvc0
  else
    REASON=$(cat /tmp/wget.log 2>&1 | head -n 20)
    echo "HARPOON_HTTPS_FAILED $REASON" > /dev/hvc0
    cat /tmp/wget.log 2>&1 | tee /dev/hvc0 >/dev/null || true
    ls -l /etc/ssl/certs/ca-certificates.crt /etc/ssl/cert.pem 2>&1 | tee /dev/hvc0 >/dev/null || true
  fi
else
  echo "HARPOON_HTTPS_FAILED wget not found" > /dev/hvc0
fi

# APK — fail fast, explicit markers (rc alone insufficient)
echo "HARPOON_APK_UPDATE_START" > /dev/hvc0
apk update > /tmp/apk.log 2>&1
RC=$?
cat /tmp/apk.log 2>&1 | tee /dev/hvc0 >/dev/null
# Correct success requires usable indexes, not just rc==0
# Check for unavailable repos or stale, and verify policy
if [ $RC -ne 0 ] || grep -q "unavailable" /tmp/apk.log 2>&1 || grep -q "temporary error" /tmp/apk.log 2>&1; then
  echo "HARPOON_APK_UPDATE_FAILED rc=$RC unavailable" > /dev/hvc0
  echo "HARPOON_APK_UPDATE_FAILED $(cat /tmp/apk.log 2>&1 | head -n 20)" > /dev/hvc0
  echo "HARPOON_DOCKER_FAILED apk update failed" > /dev/hvc0
  while true; do sleep 5; echo "HARPOON_APK_UPDATE_FAILED" > /dev/hvc0; done
fi
# Additional verification: apk policy / search
if ! apk policy docker-engine > /tmp/apk_policy.log 2>&1 || ! grep -q "docker-engine" /tmp/apk_policy.log 2>&1; then
  # fallback apk search
  if ! apk search -x docker-engine > /tmp/apk_search.log 2>&1 || ! grep -q "docker-engine" /tmp/apk_search.log 2>&1; then
    echo "HARPOON_APK_UPDATE_FAILED no usable index for docker-engine" > /dev/hvc0
    cat /tmp/apk_policy.log 2>&1 | tee /dev/hvc0 >/dev/null || true
    cat /tmp/apk_search.log 2>&1 | tee /dev/hvc0 >/dev/null || true
    echo "HARPOON_DOCKER_FAILED apk index not usable" > /dev/hvc0
    while true; do sleep 5; echo "HARPOON_APK_UPDATE_FAILED" > /dev/hvc0; done
  fi
fi
echo "HARPOON_APK_UPDATE_OK" > /dev/hvc0

echo "HARPOON_APK_INSTALL_START" > /dev/hvc0
# Prefer explicit packages: docker-engine provides dockerd, docker-cli provides docker, containerd, socat, ca-certificates
# Use docker meta as fallback but explicit dependencies make diagnosis clearer
if ! apk add --no-cache docker-engine docker-cli containerd runc socat ca-certificates > /tmp/apk2.log 2>&1; then
  cat /tmp/apk2.log 2>&1 | tee /dev/hvc0 >/dev/null
  echo "HARPOON_APK_INSTALL_FAILED rc=$?" > /dev/hvc0
  echo "HARPOON_APK_INSTALL_FAILED $(cat /tmp/apk2.log 2>&1 | head -n 30)" > /dev/hvc0
  echo "HARPOON_DOCKER_FAILED apk add failed" > /dev/hvc0
  while true; do sleep 5; echo "HARPOON_APK_INSTALL_FAILED" > /dev/hvc0; done
fi
cat /tmp/apk2.log 2>&1 | tee /dev/hvc0 >/dev/null
echo "HARPOON_APK_INSTALL_OK" > /dev/hvc0

# Verify binaries exist — explicit package that provides containerd is P:containerd (community) at /usr/bin/containerd
# Alpine 3.22: P:containerd provides p:cmd:containerd, file usr/bin/containerd (verified via APKINDEX/community/containerd-2.1.5-r2.apk)
# PATH check: /usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin (busybox default)
export PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:$PATH"
for bin in dockerd docker containerd socat; do
  if ! p=$(command -v "$bin" 2>/dev/null); then
    echo "HARPOON_DOCKER_FAILED missing_binary $bin PATH=$PATH" > /dev/hvc0
    ls -l /usr/bin/$bin /usr/sbin/$bin /bin/$bin 2>&1 | tee /dev/hvc0 >/dev/null || true
    while true; do sleep 5; echo "HARPOON_DOCKER_FAILED missing_binary $bin" > /dev/hvc0; done
  fi
  # emit exact paths
  case "$bin" in
    dockerd) echo "HARPOON_BIN_DOCKERD $p" > /dev/hvc0 ;;
    docker) echo "HARPOON_BIN_DOCKER $p" > /dev/hvc0 ;;
    containerd) echo "HARPOON_BIN_CONTAINERD $p" > /dev/hvc0 ;;
    socat) echo "HARPOON_BIN_SOCAT $p" > /dev/hvc0 ;;
  esac
  ls -l "$p" 2>&1 | tee /dev/hvc0 >/dev/null || true
done
echo "HARPOON_APK_DOCKER_VERSION $(docker --version 2>&1)" > /dev/hvc0
echo "HARPOON_APK_CONTAINERD_VERSION $(containerd --version 2>&1)" > /dev/hvc0
echo "HARPOON_APK_SOCAT_VERSION $(socat -V 2>&1 | head -n 5)" > /dev/hvc0

# Phase 2 — Docker ramdisk mode NOT used on block-backed root (normal pivot_root)
# Previous spike used DOCKER_RAMDISK=true to workaround initramfs pivot_root invalid argument
# Now block-backed ext4 provides normal semantics, so do NOT set DOCKER_RAMDISK
unset DOCKER_RAMDISK 2>/dev/null || true
export DOCKER_RAMDISK
echo "DOCKER_RAMDISK=${DOCKER_RAMDISK:-unset} (block-backed, should be unset)" > /dev/hvc0
# Start dockerd normal bridge/NAT (Docker default) — vsock API still unix-only, no TCP exposed
GUEST_IP=$(ip -4 addr show "$IFACE" 2>/dev/null | grep -oE "inet [0-9.]+/[0-9]+" | head -n1 | cut -d' ' -f2 | cut -d/ -f1)
if [ -n "$GUEST_IP" ]; then
  echo "HARPOON_GUEST_IP $GUEST_IP" > /dev/hvc0
else
  echo "HARPOON_GUEST_IP unknown" > /dev/hvc0
fi
echo "HARPOON_DOCKER_DNS 1.1.1.1 8.8.8.8" > /dev/hvc0
# --- Spike 5 persistent /run fix: make /run ephemeral (tmpfs) before containerd/dockerd ---
# Inspect pre-mount semantics (do not guess)
echo "HARPOON_RUN_FS $(mount 2>&1 | grep ' on /run ' || echo 'no /run mount')" > /dev/hvc0
ls -ld /run 2>&1 | tee /dev/hvc0 >/dev/null || true
ls -ld /var/run 2>&1 | tee /dev/hvc0 >/dev/null || true
readlink /var/run 2>&1 | tee /dev/hvc0 >/dev/null || true
echo "HARPOON_VAR_RUN $(ls -l /var/run 2>&1 | head -n 5)" > /dev/hvc0
# Bounded pre-Docker runtime files (docker/containerd only)
find /run /var/run -maxdepth 2 -type f -o -type s 2>&1 | grep -E "docker|containerd" | head -n 20 | tee /dev/hvc0 >/dev/null || echo "HARPOON_RUN_PRECHECK clean" > /dev/hvc0
# Mount tmpfs over /run to ensure ephemeral runtime (preserves symlink /var/run -> ../run)
if mount -t tmpfs -o mode=0755 tmpfs /run 2>&1 | tee /dev/hvc0 >/dev/null; then
  echo "HARPOON_RUN_TMPFS_MOUNTED /run tmpfs mode=0755" > /dev/hvc0
  mkdir -p /run/docker /run/containerd 2>&1 | tee /dev/hvc0 >/dev/null || true
  chmod 0755 /run 2>&1 | tee /dev/hvc0 >/dev/null || true
  # Verify /var/run still symlink to ../run (now tmpfs)
  ls -ld /run 2>&1 | tee /dev/hvc0 >/dev/null || true
  ls -l /var/run 2>&1 | tee /dev/hvc0 >/dev/null || true
else
  echo "HARPOON_RUN_TMPFS_FAILED" > /dev/hvc0
fi
# Post-mount verification (ephemeral) and stale cleanup
echo "HARPOON_RUN_FS $(mount 2>&1 | grep ' on /run ' || echo 'no /run mount after')" > /dev/hvc0
echo "HARPOON_VAR_RUN $(ls -l /var/run 2>&1 | head -n 5)" > /dev/hvc0
find /run /var/run -maxdepth 2 2>&1 | grep -E "docker|containerd" | head -n 20 | tee /dev/hvc0 >/dev/null || echo "HARPOON_RUN_POSTCHECK clean no docker/containerd stale" > /dev/hvc0
# Ensure no stale sockets/PIDs remain (do not kill arbitrary PIDs)
rm -f /run/docker.sock /var/run/docker.sock /run/docker.pid /var/run/docker.pid /run/containerd/containerd.sock /var/run/docker/containerd.sock 2>&1 | tee /dev/hvc0 >/dev/null || true
rm -f /run/containerd/containerd.sock 2>&1 | tee /dev/hvc0 >/dev/null || true
echo "HARPOON_DOCKERD_START" > /dev/hvc0
rm -f /var/run/docker.sock
# Docker default bridge, iptables true, ip-masq true, userland-proxy true (required for -p) plus explicit DNS (fixes VZNAT DHCP 192.168.64.1)
dockerd --host=unix:///var/run/docker.sock --dns=1.1.1.1 --dns=8.8.8.8 > /tmp/dockerd.log 2>&1 &
DOCKERD_PID=$!
echo "dockerd pid=$DOCKERD_PID" > /dev/hvc0

# bounded readiness: wait for socket + docker info (120s)
echo "waiting for /var/run/docker.sock + docker info (120s)" > /dev/hvc0
READY=0
REASON="timeout"
for i in $(seq 1 120); do
  if [ -S /var/run/docker.sock ]; then
    if docker info > /tmp/docker-info.log 2>&1; then
      READY=1
      cat /tmp/docker-info.log 2>&1 | tee /dev/hvc0 >/dev/null
      break
    else
      REASON="docker info failed i=$i $(cat /tmp/docker-info.log 2>&1 | head -n 3)"
    fi
  else
    REASON="no socket i=$i"
  fi
  sleep 1
  if ! kill -0 $DOCKERD_PID 2>/dev/null; then
    REASON="dockerd exited i=$i $(tail -n 20 /tmp/dockerd.log 2>&1 | head -n 20)"
    break
  fi
done

if [ $READY -eq 1 ]; then
  # vsock driver check — Alpine 6.12.94-0-virt has CONFIG_VSOCKETS=m, VIRTIO_VSOCKETS=m, VIRTIO_VSOCKETS_COMMON=m (modular, not builtin, verified via boot/config-6.12.103-0-virt)
  # modloop-virt provides net/vmw_vsock/vsock.ko, vmw_vsock_virtio_transport_common.ko, vmw_vsock_virtio_transport.ko (not in initramfs-virt, newly injected)
  echo "HARPOON_VSOCK_MODPROBE_START" > /dev/hvc0
  ls -lh /lib/modules/6.12.94-0-virt/kernel/net/vmw_vsock/vsock.ko /lib/modules/6.12.94-0-virt/kernel/net/vmw_vsock/vmw_vsock_virtio_transport_common.ko /lib/modules/6.12.94-0-virt/kernel/net/vmw_vsock/vmw_vsock_virtio_transport.ko 2>&1 | tee /dev/hvc0 >/dev/null || true
  # load in dependency order: vsock -> common -> transport — modprobe preferred, insmod fallback observable
  # Try modprobe first; if it fails, fallback to explicit insmod with per-module markers
  MODPROBE_OK=1
  if command -v modprobe >/dev/null 2>&1; then
    if ! modprobe vsock > /tmp/modprobe_vsock.log 2>&1; then
      cat /tmp/modprobe_vsock.log 2>&1 | tee /dev/hvc0 >/dev/null || true
      MODPROBE_OK=0
    fi
    if ! modprobe vmw_vsock_virtio_transport_common > /tmp/modprobe_common.log 2>&1; then
      cat /tmp/modprobe_common.log 2>&1 | tee /dev/hvc0 >/dev/null || true
      MODPROBE_OK=0
    fi
    if ! modprobe vmw_vsock_virtio_transport > /tmp/modprobe_transport.log 2>&1; then
      cat /tmp/modprobe_transport.log 2>&1 | tee /dev/hvc0 >/dev/null || true
      MODPROBE_OK=0
    fi
  else
    MODPROBE_OK=0
  fi
  # Direct insmod fallback — observable per-module
  if [ $MODPROBE_OK -eq 0 ]; then
    echo "modprobe failed or unavailable, trying direct insmod" > /dev/hvc0
    if insmod /lib/modules/6.12.94-0-virt/kernel/net/vmw_vsock/vsock.ko > /tmp/insmod_vsock.log 2>&1; then
      echo "HARPOON_VSOCK_INSMOD_VSOCK_OK" > /dev/hvc0
    else
      ERR=$(cat /tmp/insmod_vsock.log 2>&1 | head -n 5 | tr '\n' ' ')
      echo "HARPOON_VSOCK_INSMOD_FAILED vsock $ERR" > /dev/hvc0
      cat /tmp/insmod_vsock.log 2>&1 | tee /dev/hvc0 >/dev/null || true
    fi
    if insmod /lib/modules/6.12.94-0-virt/kernel/net/vmw_vsock/vmw_vsock_virtio_transport_common.ko > /tmp/insmod_common.log 2>&1; then
      echo "HARPOON_VSOCK_INSMOD_COMMON_OK" > /dev/hvc0
    else
      ERR=$(cat /tmp/insmod_common.log 2>&1 | head -n 5 | tr '\n' ' ')
      echo "HARPOON_VSOCK_INSMOD_FAILED vmw_vsock_virtio_transport_common $ERR" > /dev/hvc0
      cat /tmp/insmod_common.log 2>&1 | tee /dev/hvc0 >/dev/null || true
    fi
    if insmod /lib/modules/6.12.94-0-virt/kernel/net/vmw_vsock/vmw_vsock_virtio_transport.ko > /tmp/insmod_transport.log 2>&1; then
      echo "HARPOON_VSOCK_INSMOD_TRANSPORT_OK" > /dev/hvc0
    else
      ERR=$(cat /tmp/insmod_transport.log 2>&1 | head -n 5 | tr '\n' ' ')
      echo "HARPOON_VSOCK_INSMOD_FAILED vmw_vsock_virtio_transport $ERR" > /dev/hvc0
      cat /tmp/insmod_transport.log 2>&1 | tee /dev/hvc0 >/dev/null || true
    fi
  else
    # modprobe succeeded for all — still emit OK markers for observability
    echo "HARPOON_VSOCK_INSMOD_VSOCK_OK" > /dev/hvc0
    echo "HARPOON_VSOCK_INSMOD_COMMON_OK" > /dev/hvc0
    echo "HARPOON_VSOCK_INSMOD_TRANSPORT_OK" > /dev/hvc0
  fi
  sleep 1
  echo "HARPOON_VSOCK_MODPROBE_DONE" > /dev/hvc0
  # runtime verification — /dev/vsock may not exist, authoritative is AF_VSOCK listener
  grep -i vsock /proc/net/protocols 2>&1 | tee /dev/hvc0 >/dev/null || echo "no vsock in /proc/net/protocols" > /dev/hvc0
  lsmod 2>&1 | grep -i vsock | tee /dev/hvc0 >/dev/null || echo "lsmod no vsock" > /dev/hvc0
  ls -l /dev/vsock 2>&1 | tee /dev/hvc0 >/dev/null || echo "no /dev/vsock (may be normal)" > /dev/hvc0
  # bounded preflight: try to create AF_VSOCK socket via python or socat dry-run (timeout 2s)
  VSOCK_READY=0
  if command -v python3 >/dev/null 2>&1; then
    python3 -c "import socket; s=socket.socket(40,1,0); s.close(); print('vsock socket ok')" > /tmp/vsock_preflight.log 2>&1 && VSOCK_READY=1 || echo "python vsock socket failed" > /dev/hvc0
    cat /tmp/vsock_preflight.log 2>&1 | tee /dev/hvc0 >/dev/null || true
  fi
  # fallback: try socat to listen briefly and check
  if [ $VSOCK_READY -eq 0 ]; then
    timeout 2 socat VSOCK-LISTEN:2375,fork /dev/null 2> /tmp/vsock_socat_preflight.log &
    sleep 1
    if grep -q "Address family not supported" /tmp/vsock_socat_preflight.log 2>/dev/null; then
      echo "socat vsock not supported" > /dev/hvc0
      VSOCK_READY=0
    else
      # if not error about address family, consider ready (socat started)
      VSOCK_READY=1
      pkill -f "VSOCK-LISTEN:2375" 2>/dev/null || true
    fi
    cat /tmp/vsock_socat_preflight.log 2>&1 | tee /dev/hvc0 >/dev/null || true
  fi
  if [ $VSOCK_READY -eq 0 ]; then
    # also check if vsock module loaded via lsmod as last resort
    if lsmod 2>&1 | grep -q vsock; then
      VSOCK_READY=1
    fi
  fi
  if [ $VSOCK_READY -eq 0 ]; then
    echo "HARPOON_DOCKER_FAILED vsock unavailable" > /dev/hvc0
    echo "HARPOON_DOCKER_FAILED vsock unavailable lsmod=$(lsmod 2>&1 | grep vsock | head -n 5) protocols=$(cat /proc/net/protocols 2>&1 | grep vsock)" > /dev/hvc0
    while true; do sleep 5; echo "HARPOON_DOCKER_FAILED vsock unavailable" > /dev/hvc0; done
  fi
  echo "HARPOON_VSOCK_READY" > /dev/hvc0
  echo "dockerd ready, starting vsock bridge VSOCK-LISTEN:2375 -> /var/run/docker.sock" > /dev/hvc0
  socat VSOCK-LISTEN:2375,fork UNIX-CONNECT:/var/run/docker.sock > /tmp/vsock-bridge.log 2>&1 &
  VSOCK_PID=$!
  sleep 1
  if kill -0 $VSOCK_PID 2>/dev/null && [ -S /var/run/docker.sock ]; then
    echo "HARPOON_DOCKER_READY" > /dev/hvc0
    echo "vsock bridge pid=$VSOCK_PID" > /dev/hvc0
    # Spike 3 verification: bridge existence, ip_forward, iptables
    # Spec 6-7: verify bridge after dockerd readiness
    echo "HARPOON_BRIDGE_CHECK" > /dev/hvc0
    docker network ls 2>&1 | tee /dev/hvc0 >/dev/null || true
    ip link show docker0 2>&1 | tee /dev/hvc0 >/dev/null || echo "no docker0" > /dev/hvc0
    ip addr show docker0 2>&1 | tee /dev/hvc0 >/dev/null || true
    iptables -t nat -S 2>&1 | tee /dev/hvc0 >/dev/null || nft list ruleset 2>&1 | head -n 100 | tee /dev/hvc0 >/dev/null || echo "no iptables/nft nat" > /dev/hvc0
    iptables -S FORWARD 2>&1 | tee /dev/hvc0 >/dev/null || echo "no iptables FORWARD" > /dev/hvc0
    if ip link show docker0 2>/dev/null | grep -q "docker0"; then
      echo "HARPOON_BRIDGE_OK" > /dev/hvc0
    else
      echo "HARPOON_BRIDGE_FAILED no docker0" > /dev/hvc0
    fi
    IPF_FINAL=$(cat /proc/sys/net/ipv4/ip_forward 2>&1 | head -n 1 | tr -d " \n")
    echo "HARPOON_IP_FORWARD_FINAL $IPF_FINAL" > /dev/hvc0
    # overlay check
    lsmod | grep overlay 2>&1 | tee /dev/hvc0 >/dev/null || echo "overlay not yet loaded (lazy)" > /dev/hvc0
    # guest IP re-emit for host port forward (in case DHCP raced)
    GUEST_IP2=$(ip -4 addr show "$IFACE" 2>/dev/null | grep -oE "inet [0-9.]+/[0-9]+" | head -n1 | cut -d' ' -f2 | cut -d/ -f1)
    [ -n "$GUEST_IP2" ] && echo "HARPOON_GUEST_IP $GUEST_IP2" > /dev/hvc0 || true
    echo "HARPOON_NET_READY" > /dev/hvc0
  else
    echo "HARPOON_DOCKER_FAILED vsock bridge failed vsock_pid=$VSOCK_PID log=$(cat /tmp/vsock-bridge.log 2>&1 | head -n 20)" > /dev/hvc0
  fi
else
  echo "HARPOON_DOCKER_FAILED $REASON" > /dev/hvc0
  echo "dockerd log tail $(tail -n 50 /tmp/dockerd.log 2>&1 | tail -n 50)" > /dev/hvc0
fi

while true; do sleep 2; echo "HARPOON_SPIKE_OK" > /dev/hvc0; done
INIT
chmod +x $ROOT/init
# Also ensure apk keys
mkdir -p $ROOT/etc/apk/keys
# Pack
echo "[spike2] packing cpio..."
( cd $ROOT && find . | cpio -o -H newc 2>/dev/null | gzip > /tmp/harpoon-docker-initramfs.cpio.gz )
cp /tmp/harpoon-docker-initramfs.cpio.gz $INITRAMFS
ls -lh $INITRAMFS
shasum -a 256 $INITRAMFS | cut -c1-64
echo "[spike2] done $INITRAMFS"
