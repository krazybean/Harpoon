# Resource sparklines — bounded prototype

**Status:** prototype implemented; live host/VM acceptance not claimed

The desktop now has a Rust-owned sampler used only by the visible Overview.
It samples once per second, retains at most 180 samples (three minutes), and
publishes the bounded snapshot every two samples. The WebView subscribes to
native events; it does not poll for metrics. The worker stops on Overview
unmount or document hide and is joined during shutdown.

## Truth boundary

| Surface | Signal | Source / unit | UI status |
| --- | --- | --- | --- |
| HOST | CPU utilization | macOS `host_statistics` tick deltas / percent | available after first sample |
| HOST | Physical memory used | macOS `host_statistics64` + `hw.memsize` / bytes | available when native call succeeds |
| IMAGE | Harpoon disk logical size | `metadata.len()` / bytes | available when `status.diskPath` resolves |
| IMAGE | Harpoon disk allocated bytes | macOS filesystem block count / bytes | available when metadata is readable |
| HOST/IMAGE | Disk activity | no stable low-cost native counter selected | unavailable |
| VM / guest | Guest CPU, memory, filesystem usage | no guest agent or Virtualization.framework metric interface exposed here | unavailable |
| CONTAINER | Docker CPU/memory/block I/O | Docker stats not sampled by this prototype | unavailable |

The disk graph is explicitly image allocation, not filesystem usage and not
read/write activity. Configured VM memory, process peak RSS, host memory, and
Docker/container values are not substituted for VM or container metrics.

## Acceptance procedure

Offline acceptance:

- frontend tests: `npm test`
- frontend production bundle: `npm run build`
- Rust tests: `cargo test --manifest-path src-tauri/Cargo.toml`
- verify the sampler history remains bounded, missing values stay missing,
  counter reset does not create a spike, and listener cleanup handles a late
  native listener resolution.

Live acceptance is intentionally deferred until a real macOS/Tauri run can
confirm native values, visibility stop/restart, and responsive behavior. It
must not start or mutate Docker, the VM, guest filesystems, containers,
images, volumes, or networks. Guest/container metrics require a separately
audited read-only source, likely a guest agent and bounded Docker stats call;
they should not be added by inference.
