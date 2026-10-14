# Stage 3 — Guest Boot Audit

Status: audit complete; no boot or production behavior was changed. Source under audit is `tools/guest-builder/src/init` (working tree), with host ownership in `harpoon/Sources/VMManager.swift` and guest artifact assembly in `tools/guest-builder/build-initramfs.sh`.

## Scope and evidence

This audit covers VM start through the guest steady-state loop. It separates the disposable initramfs from the persistent ext4 root and does not treat Docker, containerd, or runc as Harpoon-owned implementations.

Fresh live evidence was captured on 2026-10-14 UTC from `/tmp/harpoon-serial.log` after the existing Stage 0–1 gate started the VM. The serial attachment is a raw file, so guest markers have deterministic order but no guest-side timestamps. Host log correlation recorded `HARPOON_DOCKER_READY observed` at `2026-10-14T03:16:57Z`, `HARPOON_RUNNING` at `03:16:57Z`, and `HARPOON_MGMT_READY observed` at `03:16:58Z`. The host therefore observes Docker readiness before management readiness; exact per-phase elapsed time is unobservable from this artifact.

The machine-readable phase map is [stage-3-guest-boot-phases.json](stage-3-guest-boot-phases.json). Its line ranges intentionally point to the source, not to a generated initramfs listing.

Ownership is split explicitly: the guest-init PID 1 shell owns sequencing,
mounts, validation, reconciliation, retries, and markers; Docker Engine,
containerd, and runc own the Docker/OCI runtime after their binaries are
started; the host Swift runtime owns VM configuration, serial observation, and
host-side bridges; and the guest builder/release checks own build-time artifact
closure. A phase that invokes Docker or runc is therefore still guest-init
orchestration, not a Harpoon replacement for that stack.

## Authoritative boot timeline

| Order | Phase and source | Boundary / commands | State transition and marker evidence | Classification | Every boot? |
|---:|---|---|---|---|---|
| 1 | Early mounts, `init:1-9` | initramfs; `mount`, `mkdir` | PID 1 attempts proc, sysfs, devtmpfs, `/run`, `/tmp`, cgroup2. | BOOT_CRITICAL | Yes |
| 2 | Block discovery and ext4 module load, `init:10-86` | initramfs; `stat`, `modprobe`, `insmod`, `mount` | `HARPOON_ROOTFS_TYPE tmpfs` → `HARPOON_BLOCK_TRY_PIVOT` → ext4 module markers → `/dev/vda` at `/newroot`. | BOOT_CRITICAL; RECOVERY | Yes |
| 3 | First-boot root population, `init:87-105` | initramfs; `tar`, `cp`, `chmod`, `cat` | Only when `/newroot/bin/sh` or `/newroot/sbin/init` is absent; seeds Alpine, modules, second-stage `/init`, repositories. | MIGRATION; RECOVERY | Conditional |
| 4 | Immutable runtime reconciliation, `init:106-151` | initramfs; `sha256sum`, `stat`, atomic `cp`/`mv` | Refreshes management, Python, resize2fs, and init payloads from the immutable initramfs; emits `HARPOON_RUNTIME_REFRESH_DONE`. | MIGRATION; VALIDATION; RECOVERY | Yes |
| 5 | Module closure reconciliation, `init:152-196` | initramfs; manifest reads, hashes, mode checks, atomic replacement | Validates and repairs the persistent module closure; live marker: `HARPOON_MODULE_REFRESH_SOURCE_VALID entries=56`, then `HARPOON_MODULE_REFRESH_DONE`. | MIGRATION; VALIDATION; RECOVERY | Yes |
| 6 | Root handoff, `init:197-219` | initramfs; mount moves, `switch_root`, fallback `pivot_root`/`chroot` | `HARPOON_PIVOT`; persistent root becomes PID 1’s root and second-stage `/init` continues. | BOOT_CRITICAL | Yes |
| 7 | Post-handoff root validation, `init:220-231` | post-switch_root; `mount`, `stat`, `grep` | Confirms root is not tmpfs/rootfs; live evidence was `HARPOON_ROOTFS_TYPE ext2/ext3` followed by `HARPOON_ROOTFS`. | BOOT_CRITICAL; VALIDATION | Yes |
| 8 | Offline disk/filesystem reconciliation, `init:232-289`, `676-686` | post-switch_root; `blockdev`, `df`, raw ext4 geometry reads, `resize2fs` | Validates device/filesystem geometry and grows ext4 only when needed. Live: 34,359,738,368-byte device and filesystem geometry already equal; `HARPOON_DISK_NO_RESIZE`. | MIGRATION; VALIDATION; RECOVERY | Yes |
| 9 | Network module closure and forwarding, `init:292-326` | post-switch_root; `modprobe`, `ip`, `/proc/sys` | Loads bridge/NAT modules and requires IPv4 forwarding. Live markers included `HARPOON_MODPROBE_OK ...`, `HARPOON_IP_FORWARD 1`. | BOOT_CRITICAL; RUNTIME; VALIDATION | Yes |
| 10 | VirtioFS and balloon setup, `init:328-416` | post-switch_root; `modprobe`, `mount`, `touch`, `rm`, `/proc` | Mounts share, host Users/tmp roots, loads balloon, emits memory baseline. Live: all three shares ready and balloon driver loaded. | RUNTIME; VALIDATION | Yes |
| 11 | Guest network acquisition, `init:417-575` | post-switch_root; virtio-net modules, `ip`, `udhcpc`, `ping`, `nslookup`, `wget` | Discovers `eth0`, acquires `192.168.64.20`, validates gateway/Internet/DNS/HTTPS. Live markers: `HARPOON_GATEWAY_OK`, `HARPOON_INET_OK`, `HARPOON_DNS_OK`, `HARPOON_HTTPS_OK`. | BOOT_CRITICAL; VALIDATION; RECOVERY | Yes |
| 12 | APK/package reconciliation, `init:576-675` | post-switch_root; `apk`, `command -v`, Docker/containerd/socat version probes | If required binaries already exist, offline path emits `HARPOON_APK_SKIPPED`; otherwise network package update/install runs. Live used offline skip, then verified Docker 28.3.3, containerd 2.1.5, Python, socat, resize2fs. | MIGRATION; VALIDATION; RECOVERY | Yes |
| 13 | Second disk observation and `/run` reset, `init:676-728` | post-switch_root; `mount tmpfs`, `find`, `rm`, `chmod` | Rechecks disk, unsets `DOCKER_RAMDISK`, makes `/run` ephemeral, removes selected stale Docker/containerd paths. Live: `DOCKER_RAMDISK=unset`, `HARPOON_RUN_TMPFS_MOUNTED`. | MIGRATION; RECOVERY; RUNTIME; VALIDATION | Yes |
| 14 | Docker Engine start/readiness, `init:729-759` | post-switch_root; `dockerd`, `docker info`, bounded sleep/poll | Starts Docker on guest Unix socket and waits up to 120 seconds for `docker info`. Live: `dockerd pid=9099`, then host observed Docker ready at `03:16:57Z`. | RUNTIME; VALIDATION | Yes |
| 15 | Docker vsock bridge and network validation, `init:760-878` | post-switch_root; vsock module load, Python AF_VSOCK/socat preflight, `socat`, Docker/IP/iptables/nft probes | Loads vsock closure, starts `VSOCK-LISTEN:2375`, confirms Docker bridge/NAT and emits `HARPOON_DOCKER_READY` and `HARPOON_NET_READY`. Live markers: `HARPOON_VSOCK_READY`, `HARPOON_BRIDGE_OK`, `HARPOON_IP_FORWARD_FINAL 1`. | RUNTIME; VALIDATION | Yes |
| 16 | Management service readiness, `init:888-935` | post-switch_root; Python imports/compile, `socat VSOCK-LISTEN:2377`, three-attempt retry | Wrapper validates handler/interpreter, then listener emits `HARPOON_MGMT_LISTENER_READY` and `HARPOON_MGMT_READY`. Live: attempt 1 succeeded; host observed management ready at `03:16:58Z`; subsequent exec and PTY shell probes completed. | RUNTIME; VALIDATION; RECOVERY | Yes |
| 17 | Steady-state marker loop, `init:944` | post-switch_root; `sleep`, `echo` | PID 1 remains alive and emits `HARPOON_SPIKE_OK` every two seconds. This is a historical diagnostic loop, not a readiness transition. | RUNTIME; REDUNDANT_OR_HISTORICAL | Yes |

## Failure and recovery semantics

Boot-critical failures mostly enter an intentional loop that repeats a concise `HARPOON_DOCKER_FAILED` or subsystem failure marker. This preserves serial diagnostics but means the shell does not have a single terminal state. Recovery is local and explicit: module load fallback, read-write to read-only mount fallback, first-boot repopulation, atomic payload repair, DHCP/DNS fallback, offline package reuse, and bounded management listener retries. There is no general rollback transaction for a partially completed boot.

Migration/recovery is distinct from steady state: root population, immutable payload/module repair, ext4 growth, `/run` reset, and package installation are state reconciliation. Module loading, network acquisition, Docker startup, vsock bridging, and management listener setup are runtime boot. Hash/mode manifests and builder closure checks validate immutable inputs; runtime repeats those checks because the persistent root can drift.

## Build-time guarantees versus runtime revalidation

| Concern | Build-time guarantee | Runtime revalidation / duplication |
|---|---|---|
| Kernel/initramfs inputs | `build-initramfs.sh` fingerprints pinned upstream inputs and source files. | Guest validates required files, modules, hashes, modes, and module manifest every boot. |
| BusyBox and commands | `tools/release/verify-runtime-closure.sh` inspects command presence in the artifact. | `init` calls `command -v` for Docker, containerd, socat, Python, resize2fs and fails if missing. |
| Kernel module closure | Builder and release verification inspect required modules and dependencies. | `init` reloads modules and copies a 56-entry persistent module closure every boot. |
| Python/management closure | Builder writes manifests and release verification checks Python loader/import closure. | `init` checks imports and `py_compile` before exposing vsock 2377. |
| ext4 size | Root builder creates a fixed-size image and verification sanitizes it. | `init` reads raw geometry and may resize every boot; the operation is idempotent but the probes are repeated. |
| Docker/package set | Builder artifact may already contain the full package set. | `init` still runs network probes and package decision logic every boot, usually taking the offline branch. |
| `/run` cleanliness | No persistent-root build guarantee can make runtime `/run` clean. | tmpfs mount and selected socket/PID removal are appropriately runtime-only. |

The clearest duplication candidates are module/payload verification (build-time closure plus full boot-time hash/mode reconciliation), command/package checks (artifact validation plus `command -v` plus version output), and disk geometry (root sanitation plus two boot-time geometry passes). Their correctness value differs: `/run` and process readiness cannot be shifted to build time; persistent immutable closure checks can potentially be reduced later only with a measured, trustworthy version identity.

## Shell boundary and future compiled candidates

Shell is appropriate to keep for small, linear, inspectable glue: initial mounts, marker emission, one-shot `mkdir`/`chmod`, simple command discovery, and delegating Docker/containerd/runc work to their native tools. Shell is a poor long-term owner for the persistent state machine already present here: phase transitions, retries, fatal-loop policy, manifest parsing, atomic repair, raw ext4 geometry arithmetic, module dependency fallback, networking fallbacks, readiness polling, and two independent vsock services.

Strong future `harpoon-guest` candidates, without implementing them in Stage 3, are:

- a typed boot-phase coordinator with explicit terminal states and structured failure reasons;
- immutable payload/module reconciliation with one manifest reader and atomic repair policy;
- disk geometry inspection/resize orchestration with idempotent postconditions;
- readiness supervision for Docker, the 2375 bridge, and the 2377 management listener;
- structured event/marker emission with monotonic timestamps and boot ID;
- bounded retry and recovery policy, including the distinction between degraded optional VirtioFS/balloon setup and fatal Docker prerequisites.

Docker Engine, containerd, runc, and their OCI semantics are not candidates for replacement. A compiled guest coordinator would supervise them, not reimplement them.

## Repeated or expensive work to measure later

Do not optimize during this audit. The likely repeated costs are full manifest hash/mode scans and possible copying across the persistent root; repeated module load attempts; raw disk reads and two geometry checks; DHCP plus gateway/Internet/DNS/HTTPS probes; package index/update logic even when offline binaries already satisfy the closure; Docker network/iptables inspection; Python compile validation; and the two-second marker loop. The first optimization target should be measured boot timing per phase, not guessed from source size.

## Stage 3 decisions

- No production behavior, Docker Engine/containerd/runc stack, guest artifact, or UI code was changed.
- The live ordering and readiness markers were observed on the permitting host; guest marker timestamps remain explicitly unobservable.
- The current source supports a complete phase map and all required classification categories.
- Stage 4, host audit, and Harpoon 2 UI work were not started.
