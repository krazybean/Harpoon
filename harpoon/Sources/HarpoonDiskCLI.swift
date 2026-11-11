import Foundation
import Virtualization
import Darwin

// MARK: - Disk (Stage 3C)
func diskBackingPathForStatus() -> String {
    // Not provisioned yet: show where it will be. userDiskCandidates is namespace-aware, so under HARPOON_TEST_TMPDIR this is
    // <root>/data/harpoon-root.img and never the production disk (#186).
    RuntimeConfig.existingUserDiskPath() ?? HarpoonRoots.userDiskCandidates(testFlags: false)[0]
}

func guestFilesystemInfoViaMgmt() -> (capacity: UInt64?, used: UInt64?, free: UInt64?)? {
    // Try to query guest df via management channel if VM running and mgmt reachable
    let snap = statusSnapshot()
    if snap.state != "running" { return nil }
    if !isMgmtServiceReachable() { return nil }
    guard let fd = connectMgmtSocket() else { return nil }
    defer { close(fd) }
    let req: [String: Any] = ["op": "exec", "argv": ["df", "-B1", "/"]]
    guard let jsonData = try? JSONSerialization.data(withJSONObject: req, options: []), let jsonStr = String(data: jsonData, encoding: .utf8) else { return nil }
    let line = jsonStr + "\n"
    let bytes = Array(line.utf8)
    var off = 0
    while off < bytes.count {
        let n = bytes.withUnsafeBytes { ptr in write(fd, ptr.baseAddress!.advanced(by: off), bytes.count - off) }
        if n <= 0 { if errno == EINTR { continue }; return nil }
        off += Int(n)
    }
    var tv = timeval(tv_sec: 5, tv_usec: 0)
    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))
    var respData = Data()
    var buf = [UInt8](repeating: 0, count: 8192)
    while respData.count < 1024*1024 {
        let n = buf.withUnsafeMutableBytes { ptr in read(fd, ptr.baseAddress!, ptr.count) }
        if n > 0 { respData.append(contentsOf: buf[0..<n]); if respData.contains(UInt8(ascii: "\n")) { break } }
        else if n == 0 { break } else { if errno == EINTR { continue }; break }
    }
    guard let nl = respData.firstIndex(of: UInt8(ascii: "\n")) else { return nil }
    let lineData = respData.subdata(in: 0..<nl)
    guard let str = String(data: lineData, encoding: .utf8), let data2 = str.data(using: .utf8), let obj = try? JSONSerialization.jsonObject(with: data2) as? [String: Any], let out = obj["stdout"] as? String else { return nil }
    // df output: header + line: /dev/vda 8589934592 123456 8466478136 2% /
    for l in out.components(separatedBy: "\n") {
        let parts = l.split(separator: " ").filter { !$0.isEmpty }
        if parts.count >= 6 && String(parts[0]).hasPrefix("/dev/") {
            // Filesystem Size Used Avail Use% Mounted
            if let size = UInt64(parts[1]), let used = UInt64(parts[2]), let avail = UInt64(parts[3]) {
                return (size, used, avail)
            }
        }
        // also handle overlay line? root is /dev/vda, but df may show overlay
        if l.contains(" /") && l.contains("/dev/") {
            let p2 = l.split(whereSeparator: { $0 == " " || $0 == "\t" }).filter { !$0.isEmpty }
            if p2.count >= 6, let sz = UInt64(p2[1]), let us = UInt64(p2[2]), let av = UInt64(p2[3]) { return (sz, us, av) }
        }
    }
    return nil
}

/// Everything `disk status` reports, gathered once so the text and JSON forms cannot disagree. The guest numbers come from one
/// `df` over the management channel and only while the VM runs; nothing here reads the image's contents.
struct DiskFacts {
    var backing: String
    var provisioned: Bool
    var state: String
    var logical: UInt64
    var physical: UInt64
    var guest: (capacity: UInt64, used: UInt64, free: UInt64)?
    var hostAvailable: UInt64?
    var desired: UInt64
    var configured: String?
    var pending: DiskExpansionRecord?

    var pressure: DiskPressure {
        guard let g = guest else { return .unknown }
        return DiskPolicy.pressure(used: g.used, available: g.free)
    }
    var suggestionDetails: [(bytes: UInt64, oversubscribed: Bool)] {
        DiskPolicy.suggestions(current: logical, physical: physical, hostAvailable: hostAvailable, maxBytes: DiskPolicy.maxBytes(), reserve: DiskPolicy.hostReserveBytes())
    }
    var suggestions: [UInt64] { suggestionDetails.map { $0.bytes } }
}

func collectDiskFacts() -> DiskFacts {
    let backing = diskBackingPathForStatus()
    let exists = FileManager.default.fileExists(atPath: backing)
    let (logical, physical) = exists ? RuntimeConfig.backingFileInfo(at: backing) : (0, 0)
    let state = statusSnapshot().state
    var guest: (capacity: UInt64, used: UInt64, free: UInt64)? = nil
    if exists, state == "running", let info = guestFilesystemInfoViaMgmt(), let cap = info.capacity, let used = info.used, let free = info.free {
        guest = (cap, used, free)
    }
    let probe = exists ? backing : (backing as NSString).deletingLastPathComponent
    let host = (try? FileManager.default.attributesOfFileSystem(forPath: FileManager.default.fileExists(atPath: probe) ? probe : NSHomeDirectory()))?[.systemFreeSize] as? UInt64
    return DiskFacts(backing: backing, provisioned: exists, state: state, logical: logical, physical: physical, guest: guest,
                     hostAvailable: host, desired: RuntimeConfig.desiredProvisionBytes(), configured: loadUserConfig().0?.diskSize,
                     pending: DiskExpansionRecord.load())
}

func diskSizeArg(_ bytes: UInt64) -> String { RuntimeConfig.formatBytes(bytes).replacingOccurrences(of: " GiB", with: "G").replacingOccurrences(of: " MiB", with: "M") }

/// Actionable lines for someone who is not in the desktop: what is wrong and the exact command, never an automatic action.
func diskAdvisoryLines(_ f: DiskFacts) -> [String] {
    var out: [String] = []
    if let p = f.pending {
        let span = "\(RuntimeConfig.formatBytes(p.fromBytes)) -> \(RuntimeConfig.formatBytes(p.toBytes))"
        if p.isActive {
            out.append("Disk expansion in progress (\(p.phase), \(span)): Harpoon is being stopped, grown and started again; do not start or stop it until it finishes.")
        } else {
            out.append("Interrupted disk expansion (\(p.phase), \(span)): \(p.recovery(currentLogical: f.provisioned ? f.logical : nil))")
        }
    }
    switch f.pressure {
    case .warning, .critical:
        let g = f.guest!
        let word = f.pressure == .critical ? "CRITICAL" : "WARNING"
        out.append("\(word): guest disk is low on space: \(RuntimeConfig.formatBytes(g.free)) free of \(RuntimeConfig.formatBytes(g.capacity)).")
        if let next = f.suggestions.first {
            out.append("  Grow it with: harpoon disk expand \(diskSizeArg(next)) --yes   (stops Harpoon, grows the disk, restarts; running containers are interrupted)")
        } else {
            out.append("  No larger size fits the host free space and growth cap; free space on the host or raise HARPOON_DISK_MAX / lower HARPOON_DISK_HOST_RESERVE.")
        }
    default: break
    }
    return out
}

func diskStatusJSON(_ f: DiskFacts) -> String {
    var obj: [String: Any] = [
        "backingPath": f.backing, "provisioned": f.provisioned, "state": f.state,
        "logicalBytes": f.logical, "physicalBytes": f.physical, "defaultBytes": f.desired,
        "pressure": f.pressure.rawValue, "suggestedSizes": f.suggestions.map { $0 },
        "suggestions": f.suggestionDetails.map { ["bytes": $0.bytes, "oversubscribed": $0.oversubscribed] as [String: Any] },
        "maxBytes": DiskPolicy.maxBytes(), "hostReserveBytes": DiskPolicy.hostReserveBytes(),
        "expansionRequiresRestart": true,
        "thresholds": ["warnUsedPercent": DiskPolicy.warnUsedPercent, "criticalUsedPercent": DiskPolicy.criticalUsedPercent,
                       "warnAvailableBytes": DiskPolicy.warnAvailableBytes, "criticalAvailableBytes": DiskPolicy.criticalAvailableBytes],
    ]
    if let h = f.hostAvailable { obj["hostAvailableBytes"] = h }
    if let c = f.configured { obj["configured"] = c }
    if let g = f.guest { obj["guest"] = ["capacityBytes": g.capacity, "usedBytes": g.used, "availableBytes": g.free] }
    if let p = f.pending {
        obj["pendingExpansion"] = ["phase": p.phase, "fromBytes": p.fromBytes, "toBytes": p.toBytes, "active": p.isActive,
                                   "recovery": p.recovery(currentLogical: f.provisioned ? f.logical : nil)]
    }
    let data = (try? JSONSerialization.data(withJSONObject: obj, options: [.sortedKeys])) ?? Data("{}".utf8)
    return String(data: data, encoding: .utf8) ?? "{}"
}

func handleDiskStatus(json: Bool = false) -> Int32 {
    let f = collectDiskFacts()
    if json { cliPrint(diskStatusJSON(f)); return 0 }
    let backing = f.backing
    if !f.provisioned {
        let desired = f.desired
        cliPrint("Backing file:      \(backing) (not yet provisioned)")
        cliPrint("Logical capacity:  \(RuntimeConfig.formatBytes(desired)) (\(desired) bytes) — default first provision")
        cliPrint("Host allocation:   0 bytes (sparse, not yet allocated)")
        cliPrint("Filesystem:        not yet provisioned — will be \(RuntimeConfig.formatBytes(desired)) on first start")
        if let ds = f.configured { cliPrint("Configured:        \(ds)") } else { cliPrint("Configured:        (default 4G)") }
        diskAdvisoryLines(f).forEach(cliPrint)
        return 0
    }
    let logical = f.logical, physical = f.physical
    let configured = f.configured ?? "(default 4G)"
    cliPrint("Backing file:      \(backing)")
    cliPrint("Logical capacity:  \(RuntimeConfig.formatBytes(logical)) (\(logical) bytes)")
    cliPrint("Host allocation:   \(RuntimeConfig.formatBytes(physical)) (\(physical) bytes) — sparse")
    // inode
    var st = stat()
    if stat(backing, &st) == 0 { cliPrint("Inode:             \(st.st_ino) dev=\(st.st_dev)") }
    if let tmpl = RuntimeConfig.installedLibDir().map(RuntimeConfig.guestAssetDir)?.appendingPathComponent("harpoon-root.img").path, FileManager.default.fileExists(atPath: tmpl) {
        let (tLog, _) = RuntimeConfig.backingFileInfo(at: tmpl)
        cliPrint("Template:          \(tmpl) (\(RuntimeConfig.formatBytes(tLog))) — immutable")
    }
    cliPrint("Configured:        \(configured)")
    if let h = f.hostAvailable { cliPrint("Host free:         \(RuntimeConfig.formatBytes(h))") }
    // guest filesystem
    if let g = f.guest {
        cliPrint("Filesystem:        \(RuntimeConfig.formatBytes(g.capacity))")
        cliPrint("Used:              \(RuntimeConfig.formatBytes(g.used))")
        cliPrint("Free:              \(RuntimeConfig.formatBytes(g.free))")
        if logical > g.capacity {
            cliPrint("WARNING: backing \(RuntimeConfig.formatBytes(logical)) > filesystem \(RuntimeConfig.formatBytes(g.capacity)) — interrupted resize pending, next boot will expand")
        }
        // host free space warning
        if let freeHost = f.hostAvailable, freeHost < 2 * 1024 * 1024 * 1024 {
            cliPrint("WARNING: host filesystem low free space \(RuntimeConfig.formatBytes(freeHost)) — sparse growth may fail")
        }
    } else {
        cliPrint("Filesystem:        unavailable (VM stopped) — backing facts above are authoritative; start VM for live FS stats")
    }
    diskAdvisoryLines(f).forEach(cliPrint)
    return 0
}

/// Grow the sparse backing file to `requested`. The single implementation behind `disk resize` and `disk expand`: it refuses to touch a
/// template, a non-regular file, or anything that is not a strict growth. Returns 0 on success; prints exactly why otherwise.
func growBackingFile(_ backing: String, to requested: UInt64) -> Int32 {
    if let protected = RuntimeConfig.templateProtectionError(for: backing) {
        cliError(protected)
        return 1
    }
    let (curLogical, curPhysical) = RuntimeConfig.backingFileInfo(at: backing)
    if requested <= curLogical {
        if requested == curLogical { cliError("already \(RuntimeConfig.formatBytes(curLogical)) — no-op (grow-only)"); return 1 }
        cliError("shrink not supported — requested \(RuntimeConfig.formatBytes(requested)) <= current \(RuntimeConfig.formatBytes(curLogical)) (grow-only)")
        return 1
    }
    // Validate not corrupt: check file is regular and has ext4 magic? minimal: file exists and size >0
    var st = stat()
    if stat(backing, &st) != 0 || (st.st_mode & S_IFMT) != S_IFREG {
        cliError("backing file invalid or not regular: \(backing)")
        return 1
    }
    // Record identity before
    let inoBefore = st.st_ino
    let devBefore = st.st_dev
    cliPrint("Resizing \(backing)")
    cliPrint("  \(RuntimeConfig.formatBytes(curLogical)) -> \(RuntimeConfig.formatBytes(requested))")
    cliPrint("  host allocation before: \(RuntimeConfig.formatBytes(curPhysical))")
    // Grow sparse file — atomic: truncate, old FS remains valid until guest expands
    if let fh = FileHandle(forWritingAtPath: backing) {
        do { try fh.truncate(atOffset: requested) } catch {
            cliError("truncate failed: \(error)")
            return 1
        }
        try? fh.close()
    } else {
        let t = Process(); t.executableURL = URL(fileURLWithPath: "/usr/bin/truncate"); t.arguments = ["-s", "\(requested)", backing]
        try? t.run(); t.waitUntilExit()
        if t.terminationStatus != 0 {
            cliError("truncate via /usr/bin/truncate failed")
            return 1
        }
    }
    // Verify
    let (newLogical, newPhysical) = RuntimeConfig.backingFileInfo(at: backing)
    var st2 = stat()
    if stat(backing, &st2) != 0 || newLogical != requested {
        cliError("verify failed: logical \(newLogical) != requested \(requested)")
        return 1
    }
    if st2.st_ino != inoBefore || st2.st_dev != devBefore {
        cliPrint("WARNING: inode changed (expected same file, but filesystem may have replaced)")
    }
    cliPrint("Backing grown to \(RuntimeConfig.formatBytes(newLogical)) (\(newLogical) bytes), physical \(RuntimeConfig.formatBytes(newPhysical)) — filesystem will expand on next boot via guest e2fsck+resize2fs")
    return 0
}

func handleDiskResize(args: [String]) -> Int32 {
    guard let newSizeStr = args.first else {
        cliError("usage: harpoon disk resize <new-size>  e.g. harpoon disk resize 16G")
        return 2
    }
    guard let requested = RuntimeConfig.parseDiskSize(newSizeStr) else {
        cliError("invalid size: \(newSizeStr) — use e.g. 8G, 16G, 32G, 1024M (G/GiB/M/MiB)")
        return 2
    }
    if requested < RuntimeConfig.minProvisionBytes {
        cliError("requested \(newSizeStr) (\(requested) bytes) < minimum 2G (\(RuntimeConfig.minProvisionBytes) bytes)")
        return 2
    }
    let snap = statusSnapshot()
    if snap.state == "running" || snap.state == "starting" || snap.state == "booting" {
        cliError("Harpoon VM must be stopped to resize — run: harpoon stop")
        cliError("Current state: \(snap.state)")
        return 1
    }
    guard let backing = RuntimeConfig.existingUserDiskPath() else {
        cliError("no provisioned disk found — run harpoon start first (first provision default 4G)")
        return 1
    }
    let code = growBackingFile(backing, to: requested)
    if code == 0 { cliPrint("Next: harpoon start (guest will auto-expand ext4 if backing > filesystem)") }
    return code
}

/// `harpoon disk expand <size> [--yes]`: grow the disk of a running (or stopped) Harpoon. It stops the VM, so it never acts without
/// `--yes`, never prompts (scripts must not hang), and refuses everything it can refuse BEFORE stopping anything.
func handleDiskExpand(args: [String]) -> Int32 {
    let confirmed = args.contains("--yes")
    let positional = args.filter { $0 != "--yes" }
    guard positional.count == 1 else {
        cliError("usage: harpoon disk expand <new-size> --yes   e.g. harpoon disk expand 8G --yes")
        return 2
    }
    guard let requested = RuntimeConfig.parseDiskSize(positional[0]) else {
        cliError("invalid size: \(positional[0]) — use e.g. 8G, 16G, 32G, 1024M (G/GiB/M/MiB)")
        return 2
    }
    guard let controlLock = LifecycleControlLock() else {
        cliError(controlBusyMessage())
        return 11
    }
    let snap = statusSnapshot()
    guard let configured = RuntimeConfig.existingUserDiskPath() else {
        cliError("no provisioned disk found — nothing to expand (first provision default 4G; use harpoon start --disk-size)")
        return 1
    }
    let running = snap.state == "running"
    if snap.state == "starting" || snap.state == "booting" || snap.state == "stopping" {
        cliError("Harpoon is \(snap.state); wait for it to settle, then retry")
        return 1
    }
    let startArgs = restartStartArgs([])
    // The disk to grow must be the one the running VM actually has open, and the restart must reproduce the VM exactly.
    var backing = configured
    if running {
        guard let meta = snap.meta else {
            cliError("cannot confirm which disk the running Harpoon uses (no runtime metadata); refusing to expand")
            return 1
        }
        guard let activeID = diskIdentity(at: meta.diskPath), let configuredID = diskIdentity(at: configured), activeID == configuredID else {
            cliError("the running Harpoon uses \(meta.diskPath), not \(configured); refusing to expand a disk it does not use")
            cliError("expand that disk manually: harpoon stop; harpoon disk resize \(positional[0]); then start with the same --disk")
            return 1
        }
        backing = meta.diskPath
        let differences = expansionConfigMismatches(meta: meta, resolved: resolveConfigFromArgs(startArgs))
        if !differences.isEmpty {
            cliError("refused: restarting for the expansion would not reproduce the running VM:")
            differences.forEach { cliError("  - \($0)") }
            cliError("nothing was stopped. Expand manually (harpoon stop; harpoon disk resize \(positional[0]); then start with the settings you want), or restart Harpoon once so it records them, then retry.")
            return 1
        }
    }
    if let protected = RuntimeConfig.templateProtectionError(for: backing) { cliError(protected); return 1 }
    let (current, physical) = RuntimeConfig.backingFileInfo(at: backing)
    let host = (try? FileManager.default.attributesOfFileSystem(forPath: backing))?[.systemFreeSize] as? UInt64
    let assessment = DiskPolicy.assess(current: current, physical: physical, requested: requested, hostAvailable: host, maxBytes: DiskPolicy.maxBytes(), reserve: DiskPolicy.hostReserveBytes())
    if let refusal = assessment.error {
        cliError("refused: \(refusal)")
        return 1
    }
    if running, let rejected = startArgsError(startArgs) {
        rejected.lines.forEach(cliError)
        cliError("expansion refused before stopping anything: the running Harpoon was not touched")
        return rejected.code
    }
    cliPrint("Expand \(backing): \(RuntimeConfig.formatBytes(current)) -> \(RuntimeConfig.formatBytes(requested))")
    if assessment.oversubscribed {
        cliPrint("NOTE: if this disk filled completely it would need up to \(RuntimeConfig.formatBytes(assessment.worstCaseBytes)) on this Mac, more than it can spare after the reserve. Nothing stops a guest that fills it; keep an eye on `harpoon disk status`.")
    }
    if running {
        cliPrint("This stops Harpoon, grows the disk, and starts it again. Running containers are interrupted and restart policies apply.")
    } else {
        cliPrint("Harpoon is stopped; the disk is grown now and the filesystem expands the next time Harpoon starts. Nothing is started.")
    }
    guard confirmed else {
        cliError("not confirmed: nothing was changed. Re-run with --yes to proceed.")
        return 2
    }

    // The recovery record must be on disk (and read back) before anything is stopped or resized.
    var record = DiskExpansionRecord(phase: "stopping", fromBytes: current, toBytes: requested, diskPath: backing,
                                     startedAt: ISO8601DateFormatter().string(from: Date()), pid: getpid())
    if let why = record.save() {
        cliError("refused: cannot write the recovery record (\(why)); nothing was stopped or resized")
        return 1
    }
    let plannedID = diskIdentity(at: backing)
    func saveProgress(_ phase: String) {
        record.phase = phase
        // Stopping already happened; a failed update must not abort. The recovery text is judged by the disk's size, not this phase.
        if let why = record.save() { cliError("warning: could not update the recovery record (\(why)); recovery will be judged from the disk size") }
    }
    if running {
        let stopCode = handleStop(controlLock: controlLock)
        if stopCode != 0 {
            cliError("expansion aborted: Harpoon was not stopped cleanly; the disk was not changed (still \(RuntimeConfig.formatBytes(current)))")
            DiskExpansionRecord.clear()
            return stopCode
        }
        // "Stopped" must be real before the file is touched: no lock holder, no live runtime.
        let after = statusSnapshot()
        if isLockHeld() || after.state == "running" || after.state == "starting" || after.state == "booting" || (after.alive && after.isHarpoon) {
            cliError("expansion aborted: Harpoon still looks active (\(after.state)); the disk was not changed")
            DiskExpansionRecord.clear()
            return 1
        }
    }
    if diskIdentity(at: backing) != plannedID {
        cliError("expansion aborted: the disk file changed identity since the plan; it was not grown. \(running ? "Harpoon is stopped; start it with: harpoon start" : "")")
        DiskExpansionRecord.clear()
        return 1
    }
    saveProgress("growing")
    let grow = growBackingFile(backing, to: requested)
    if grow != 0 {
        cliError("expansion failed while growing the disk file; your data is untouched. \(record.recovery(currentLogical: RuntimeConfig.backingFileInfo(at: backing).logical))")
        if running { cliError("Harpoon is stopped. Start it with: harpoon start") }
        DiskExpansionRecord.clear()
        return grow
    }
    if !running {
        DiskExpansionRecord.clear()
        cliPrint("Disk grown to \(RuntimeConfig.formatBytes(requested)). Run `harpoon start`; the guest expands the filesystem on boot.")
        return 0
    }
    saveProgress("starting")
    Thread.sleep(forTimeInterval: 0.5)
    let startCode = handleStart(args: startArgs, controlLock: controlLock)
    if startCode != 0 {
        cliError("The disk was grown to \(RuntimeConfig.formatBytes(requested)) but Harpoon failed to start (exit \(startCode)). Your data is preserved; there is no rollback.")
        cliError("Try: harpoon start   (see `harpoon doctor` and \(HarpoonPaths.logFile.path)). The record stays until a start is verified.")
        return startCode
    }
    saveProgress("verifying")
    // Success needs all of: the file is the requested size, Harpoon is running with Docker ready, and the guest filesystem reports
    // the added capacity. Each failure is reported as what it is; none is reported as success.
    let deadline = Date().addingTimeInterval(120)
    var ready = false, expanded = false
    while Date() < deadline {
        let s = statusSnapshot()
        ready = s.state == "running" && s.dockerReady
        if ready, let cap = guestFilesystemInfoViaMgmt()?.capacity {
            expanded = DiskPolicy.filesystemExpanded(after: cap, from: current, to: requested)
            if expanded { break }
        }
        Thread.sleep(forTimeInterval: 2)
    }
    let finalLogical = RuntimeConfig.backingFileInfo(at: backing).logical
    if finalLogical != requested {
        cliError("Harpoon started but the disk file is \(RuntimeConfig.formatBytes(finalLogical)), not the requested \(RuntimeConfig.formatBytes(requested)). Check `harpoon disk status`.")
        return 1
    }
    if !ready {
        cliError("Harpoon started but Docker did not become ready within 120s. The disk is \(RuntimeConfig.formatBytes(requested)); data is preserved. Check `harpoon doctor`. The record stays until this is verified.")
        return 1
    }
    if !expanded {
        cliError("Docker is ready but the guest filesystem did not report the larger size within 120s. Run `harpoon disk status`; if the filesystem is still smaller than the disk, restart Harpoon (`harpoon restart`) so the guest runs resize2fs. The record stays until this is verified.")
        return 1
    }
    DiskExpansionRecord.clear()
    cliPrint("Disk expanded to \(RuntimeConfig.formatBytes(requested)); filesystem expanded and Docker is ready")
    return 0
}

func handleDisk(args: [String]) -> Int32 {
    if args.isEmpty || args[0]=="help" || args[0]=="--help" || args[0]=="-h" {
        cliPrint("usage: harpoon disk <status|resize|expand> [args]")
        cliPrint("  status [--json]     Show backing file, logical, physical, filesystem, used/free, low-space warning")
        cliPrint("  resize <new-size>   Grow sparse disk (e.g. harpoon disk resize 16G) — VM must be stopped, grow-only")
        cliPrint("  expand <size> --yes Stop Harpoon, grow the disk, start it again and verify (interrupts running containers; without --yes nothing happens)")
        return 0
    }
    switch args[0] {
    case "status":
        let rest = Array(args.dropFirst())
        if rest.contains(where: { $0 != "--json" }) { cliError("usage: harpoon disk status [--json]"); return 2 }
        return handleDiskStatus(json: rest.contains("--json"))
    case "resize":
        if args.count < 2 { cliError("usage: harpoon disk resize <new-size>"); return 2 }
        return handleDiskResize(args: Array(args.dropFirst(1)))
    case "expand":
        return handleDiskExpand(args: Array(args.dropFirst(1)))
    default:
        cliError("unknown disk subcommand: \(args[0])")
        return 2
    }
}
