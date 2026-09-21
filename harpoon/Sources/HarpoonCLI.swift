import Foundation
import Virtualization
import Darwin

// MARK: - CLI handlers

func cliPrint(_ s: String) {
    print(s)
}
func cliError(_ s: String) {
    fputs(s + "\n", stderr)
}

func handleDoctor() -> Int32 {
    var passed=0, warned=0, failed=0
    func check(_ ok: Bool, _ msg: String, warn: Bool=false) {
        if ok { cliPrint("PASS  \(msg)"); passed+=1 }
        else if warn { cliPrint("WARN  \(msg)"); warned+=1 }
        else { cliPrint("FAIL  \(msg)"); failed+=1 }
    }
    cliPrint("Harpoon Doctor")
    cliPrint("")
    // HOST
    let ver = ProcessInfo.processInfo.operatingSystemVersionString
    check(true, "macOS \(ver)")
    #if arch(arm64)
    check(true, "architecture arm64")
    #else
    check(false, "architecture not arm64", warn:true)
    #endif
    check(VZVirtualMachine.isSupported, "Virtualization.framework available")
    // HARPOON
    let bin = Bundle.main.executableURL?.path ?? CommandLine.arguments[0]
    check(FileManager.default.isExecutableFile(atPath: bin), "binary \(bin)")
    let appDir = HarpoonPaths.appSupportDir.path
    check(FileManager.default.isWritableFile(atPath: appDir) || FileManager.default.fileExists(atPath: appDir), "runtime directory writable \(appDir)")
    let runtime = RuntimeConfig.fromEnvironment()
    let kernel = runtime.kernelURL.path
    check(FileManager.default.fileExists(atPath: kernel), "kernel \(kernel)")
    let initramfs = runtime.initramfsURL.path
    check(FileManager.default.fileExists(atPath: initramfs), "initramfs \(initramfs)")
    let disk = runtime.diskURL.path
    let diskExists = FileManager.default.fileExists(atPath: disk)
    check(diskExists, "disk \(disk)")
    if diskExists, let attrs = try? FileManager.default.attributesOfItem(atPath: disk), let sz = attrs[.size] as? UInt64 {
        cliPrint("INFO  disk logical bytes \(sz)")
    }
    let snap = statusSnapshot()
    let lockHeld = isLockHeld()
    check(!lockHeld || snap.state=="running" || snap.state=="starting", "lock state (held=\(lockHeld) state=\(snap.state))", warn: lockHeld && snap.state=="stopped")
    // PID
    if let pid = snap.pid {
        check(snap.alive, "PID \(pid) alive", warn:!snap.alive)
        if snap.alive { check(snap.isHarpoon, "PID is harpoon") }
    } else {
        cliPrint("INFO  no PID file (stopped)")
    }
    // socket
    let sockExists = FileManager.default.fileExists(atPath: HarpoonPaths.dockerSocketPath)
    if snap.state=="running" {
        check(sockExists, "socket \(HarpoonPaths.dockerSocketPath)")
        var st = stat()
        let permsOk = stat(HarpoonPaths.dockerSocketPath, &st)==0 && (st.st_mode & 0o777)==0o600
        check(permsOk, "socket 0600")
    } else {
        cliPrint("INFO  socket exists=\(sockExists) (expected when running)")
    }
    // DOCKER — decomposed per 3B
    if let docker = findDocker() {
        check(true, "Docker CLI ................. PASS \(docker)")
        let (code, out, _) = runDocker(["--version"])
        if code==0 { cliPrint("INFO  \(out.trimmingCharacters(in: .whitespacesAndNewlines))") }
        let (compInstalled, compVer) = dockerComposeVersion()
        if compInstalled {
            check(true, "Docker Compose plugin ...... PASS \(compVer)")
        } else {
            check(false, "Docker Compose plugin ...... FAIL — not installed (checked: docker compose version) — Install Docker Compose v2 plugin", warn:true)
        }
        if dockerContextExists(harpoonContextName) {
            if let ep = dockerContextEndpoint(harpoonContextName) {
                check(ep==harpoonSocketEndpoint, "Harpoon context ............ PASS \(ep)")
                if ep != harpoonSocketEndpoint { cliPrint("INFO  expected \(harpoonSocketEndpoint) — run: harpoon docker setup") }
            }
        } else {
            check(false, "Harpoon context ............ FAIL — not installed (run harpoon docker setup) — endpoint \(harpoonSocketEndpoint)", warn:true)
        }
        if let cur = currentDockerContext() { cliPrint("INFO  active context \(cur) (default unchanged after setup)") }
        if snap.state=="running" && sockExists {
            let (c, vOut, _) = runDocker(["--context", "harpoon", "version"])
            if c==0 {
                let verLine = vOut.components(separatedBy: "\n").first?.trimmingCharacters(in: .whitespaces) ?? ""
                check(true, "Docker Engine .............. PASS \(verLine)")
            } else {
                check(false, "Docker Engine .............. FAIL — not reachable via harpoon context")
            }
        } else if snap.state=="running" {
            check(false, "Harpoon socket ............. FAIL — socket not found (expected \(HarpoonPaths.dockerSocketPath))")
        } else {
            // VM stopped: CLI/context can still PASS, socket/Engine report stopped accurately, don't mark context invalid
            cliPrint("INFO  Harpoon socket ............. not running (expected when VM stopped) exists=\(sockExists) — CLI/context checks above still valid")
            cliPrint("INFO  Docker Engine .............. not running (VM stopped) — start with harpoon start")
        }
        let creds = credsStoreStatus()
        if creds.status == "WARN" {
            check(false, "Docker credential helper ... WARN — config references docker-credential-desktop but helper not installed (credsStore: \(creds.store ?? "")) — Harpoon does not require Docker Desktop", warn:true)
            if let w = creds.warning { cliPrint("WARN  \(w)") }
        } else if let s = creds.store {
            check(true, "Docker credential helper ... PASS (credsStore: \(s))")
        } else {
            check(true, "Docker credential helper ... PASS")
        }
    } else {
        check(false, "Docker CLI ................. FAIL — Docker CLI not installed/found (checked PATH, /opt/homebrew/bin/docker, /usr/local/bin/docker, /usr/bin/docker) — Install Docker CLI; Docker Desktop NOT required", warn:false)
        check(false, "Docker Compose plugin ...... FAIL — Docker CLI not found", warn:true)
        check(false, "Harpoon context ............ FAIL — Docker CLI not found", warn:true)
        cliPrint("INFO  Harpoon socket ............. skipped — Docker CLI not found")
        cliPrint("INFO  Docker Engine .............. skipped — Docker CLI not found")
        let creds = credsStoreStatus()
        if creds.status == "WARN" {
            check(false, "Docker credential helper ... WARN — \(creds.warning ?? "")", warn:true)
        }
    }
    // RUNTIME
    if snap.state=="stale" { check(false, "stale PID metadata", warn:true) }
    if snap.state=="degraded" { check(false, "degraded state", warn:true) }
    // Stage 3A mgmt channel
    let mgmtExists = FileManager.default.fileExists(atPath: HarpoonPaths.mgmtSocketPath)
    if snap.state=="running" {
        check(mgmtExists, "mgmt socket \(HarpoonPaths.mgmtSocketPath)")
        if mgmtExists {
            var st = stat()
            let permsOk = stat(HarpoonPaths.mgmtSocketPath, &st)==0 && (st.st_mode & 0o777)==0o600
            check(permsOk, "mgmt socket 0600")
            check(isMgmtServiceReachable(), "mgmt service reachable (vsock 2377)")
        }
        if let log = try? String(contentsOfFile: HarpoonPaths.logFile.path, encoding: .utf8) {
            check(log.contains("HARPOON_MGMT_READY"), "HARPOON_MGMT_READY in log")
        } else {
            check(false, "HARPOON_MGMT_READY in log")
        }
    } else {
        cliPrint("INFO  mgmt exists=\(mgmtExists) (expected when running)")
    }
    // STORAGE (Stage 3C) — immutable template vs mutable user disk, sparse, grow-only
    let backing = diskBackingPathForStatus()
    let backingExists = FileManager.default.fileExists(atPath: backing)
    if backingExists {
        let (logical, physical) = RuntimeConfig.backingFileInfo(at: backing)
        check(true, "Persistent disk ............ PASS \(backing)")
        cliPrint("INFO  Logical capacity ........... \(RuntimeConfig.formatBytes(logical)) (\(logical) bytes)")
        cliPrint("INFO  Host allocation ............ \(RuntimeConfig.formatBytes(physical)) (\(physical) bytes) sparse")
        if logical < RuntimeConfig.minProvisionBytes {
            check(false, "Persistent disk logical < 2G (\(RuntimeConfig.formatBytes(logical))) — should be >=2G (template size, existing disks <32G remain valid)", warn:true)
        }
        // Config vs actual mismatch (Stage 3C B)
        if let cfgDiskStr = loadUserConfig().0?.diskSize, let cfgBytes = RuntimeConfig.parseDiskSize(cfgDiskStr) {
            if cfgBytes != logical {
                cliPrint("INFO  Configured disk-size ...... \(cfgDiskStr) (\(RuntimeConfig.formatBytes(cfgBytes))) vs actual \(RuntimeConfig.formatBytes(logical)) — not auto-resized; run harpoon disk resize \(cfgDiskStr) (VM stopped, grow-only)")
                if cfgBytes > logical {
                    check(false, "Configured disk-size \(cfgDiskStr) > actual \(RuntimeConfig.formatBytes(logical)) — pending explicit resize", warn:true)
                }
            }
        }
        if let info = guestFilesystemInfoViaMgmt(), let cap = info.capacity {
            cliPrint("INFO  Filesystem capacity ........ \(RuntimeConfig.formatBytes(cap))")
            if let free = info.free { cliPrint("INFO  Filesystem free ............ \(RuntimeConfig.formatBytes(free))") }
            if logical > cap {
                check(false, "Persistent disk backing \(RuntimeConfig.formatBytes(logical)) > filesystem \(RuntimeConfig.formatBytes(cap)) — pending resize (interrupted)", warn:true)
            } else {
                check(true, "Filesystem capacity matches backing")
            }
            if let free = info.free, free < 512 * 1024 * 1024 {
                check(false, "Filesystem low free space \(RuntimeConfig.formatBytes(free))", warn:true)
            }
        } else if snap.state=="running" {
            check(false, "Filesystem capacity ........ FAIL — mgmt not reachable for live stats", warn:true)
        } else {
            cliPrint("INFO  Filesystem capacity ........ unavailable (VM stopped) — backing above is authoritative")
        }
        // host free space
        if let attrs = try? FileManager.default.attributesOfFileSystem(forPath: backing), let freeHost = attrs[.systemFreeSize] as? UInt64, freeHost < 2 * 1024 * 1024 * 1024 {
            check(false, "Host filesystem low free \(RuntimeConfig.formatBytes(freeHost)) — sparse growth may fail", warn:true)
        }
    } else {
        let desired = RuntimeConfig.desiredProvisionBytes()
        check(false, "Persistent disk ............ not yet provisioned — will be \(RuntimeConfig.formatBytes(desired)) at \(backing) on first start", warn:true)
    }
    // config
    let (cfg, err) = loadUserConfig()
    if let e = err { check(false, "config invalid: \(e)") } else if cfg != nil { cliPrint("INFO  user config present") }
    cliPrint("")
    cliPrint("\(passed) passed, \(warned) warnings, \(failed) failures")
    return failed>0 ? 1 : 0
}

func handleLogsPath() -> Int32 {
    cliPrint(HarpoonPaths.logFile.path)
    return 0
}

func handleDockerEnv() -> Int32 {
    cliPrint("export DOCKER_HOST=unix://\(HarpoonPaths.dockerSocketPath)")
    return 0
}

func handleVersion() -> Int32 {
    cliPrint("Harpoon 0.1.0")
    return 0
}

func printUsageFull() {
    let msg = """
    Harpoon — lightweight Docker runtime for macOS

    Usage:
      harpoon <command> [options]

    Commands:
      start       Start Harpoon in background
      stop        Stop Harpoon
      restart     Restart Harpoon
      status      Show runtime status
      logs        Show runtime logs
      config      Manage defaults
      disk        Manage persistent disk (status, resize)
      docker      Manage Docker integration
      exec        Execute command in guest (harpoon exec -- <cmd> [args...])
      shell       Open interactive shell in guest (harpoon shell)
      doctor      Diagnose common problems
      run         Run in foreground (debug)
      version     Show version
      help        Show help

    Start options:
      --cpus N                1...8 (default 2)
      --memory MiB>=512       (default 4096)
      --kernel PATH
      --initramfs PATH
      --disk PATH
      --disk-size 8G|16G|32G  First provision size (e.g. 8G, 16G) — only when no disk exists; use harpoon disk resize to grow

    Precedence: CLI flag > user config > environment > defaults
    Config: ~/Library/Application Support/Harpoon/config.json

    Disk (persistent, grow-only, sparse):
      harpoon disk status           Backing logical/physical, FS used/free, config
      harpoon disk resize 16G       Grow to 16G (VM stopped, never shrink)
      harpoon start --disk-size 16G First provision 16G (no disk), otherwise use resize

    Docker:
      harpoon docker setup   Create harpoon context (unix:///tmp/harpoon-docker.sock)
      harpoon docker status
      docker --context harpoon ps

    Examples:
      harpoon start --disk-size 16G
      harpoon docker setup
      docker --context harpoon run --rm hello-world
      docker --context harpoon compose up -d
      harpoon exec -- uname -a
      harpoon exec -- docker info
      harpoon shell

    Guest management (vsock 2377, no SSH, no TCP):
      harpoon exec -- <cmd> [args...]   Execute in guest, preserve arg boundaries
      harpoon shell                     Interactive PTY shell (/bin/sh)

    Diagnostics:
      harpoon status
      harpoon doctor
      harpoon logs --lines 100
    notes:
      harpoon start  - background managed runtime (log: ~/Library/Application Support/Harpoon/harpoon.log, socket: /tmp/harpoon-docker.sock, lock: /tmp/harpoon.lock)
      harpoon run    - foreground runtime (logs to terminal, Ctrl-C to stop)
      bare 'harpoon --cpus ...' -> alias for 'harpoon run --cpus ...' (preserves Phase 1)
    """
    fputs(msg + "\n", stderr)
}
