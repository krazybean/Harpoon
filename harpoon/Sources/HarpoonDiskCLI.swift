import Foundation
import Virtualization
import Darwin

// MARK: - Disk (Stage 3C)
func diskBackingPathForStatus() -> String {
    if let p = RuntimeConfig.existingUserDiskPath() { return p }
    // not yet provisioned — show where it will be
    let primary = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Harpoon/data/harpoon-root.img").path
    if FileManager.default.fileExists(atPath: primary) { return primary }
    return primary
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

func handleDiskStatus() -> Int32 {
    let backing = diskBackingPathForStatus()
    let exists = FileManager.default.fileExists(atPath: backing)
    if !exists {
        let desired = RuntimeConfig.desiredProvisionBytes()
        cliPrint("Backing file:      \(backing) (not yet provisioned)")
        cliPrint("Logical capacity:  \(RuntimeConfig.formatBytes(desired)) (\(desired) bytes) — default first provision")
        cliPrint("Host allocation:   0 bytes (sparse, not yet allocated)")
        cliPrint("Filesystem:        not yet provisioned — will be \(RuntimeConfig.formatBytes(desired)) on first start")
        let cfg = loadUserConfig().0
        if let ds = cfg?.diskSize { cliPrint("Configured:        \(ds)") } else { cliPrint("Configured:        (default 32G)") }
        return 0
    }
    let (logical, physical) = RuntimeConfig.backingFileInfo(at: backing)
    let cfg = loadUserConfig().0
    let configured = cfg?.diskSize ?? "(default 32G)"
    cliPrint("Backing file:      \(backing)")
    cliPrint("Logical capacity:  \(RuntimeConfig.formatBytes(logical)) (\(logical) bytes)")
    cliPrint("Host allocation:   \(RuntimeConfig.formatBytes(physical)) (\(physical) bytes) — sparse")
    // inode
    var st = stat()
    if stat(backing, &st) == 0 { cliPrint("Inode:             \(st.st_ino) dev=\(st.st_dev)") }
    if let tmpl = RuntimeConfig.installedLibDir()?.appendingPathComponent("harpoon-root.img").path, FileManager.default.fileExists(atPath: tmpl) {
        let (tLog, _) = RuntimeConfig.backingFileInfo(at: tmpl)
        cliPrint("Template:          \(tmpl) (\(RuntimeConfig.formatBytes(tLog))) — immutable")
    }
    cliPrint("Configured:        \(configured)")
    // guest filesystem
    if let info = guestFilesystemInfoViaMgmt(), let cap = info.capacity, let used = info.used, let free = info.free {
        cliPrint("Filesystem:        \(RuntimeConfig.formatBytes(cap))")
        cliPrint("Used:              \(RuntimeConfig.formatBytes(used))")
        cliPrint("Free:              \(RuntimeConfig.formatBytes(free))")
        if logical > cap {
            cliPrint("WARNING: backing \(RuntimeConfig.formatBytes(logical)) > filesystem \(RuntimeConfig.formatBytes(cap)) — interrupted resize pending, next boot will expand")
        }
        // host free space warning
        if let attrs = try? FileManager.default.attributesOfFileSystem(forPath: backing), let freeHost = attrs[.systemFreeSize] as? UInt64, freeHost < 2 * 1024 * 1024 * 1024 {
            cliPrint("WARNING: host filesystem low free space \(RuntimeConfig.formatBytes(freeHost)) — sparse growth may fail")
        }
    } else {
        cliPrint("Filesystem:        unavailable (VM stopped) — backing facts above are authoritative; start VM for live FS stats")
        // pending resize warning even without FS
        // we can still warn if logical is larger than expected template? Not needed
    }
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
        cliError("no provisioned disk found — run harpoon start first (first provision default 32G)")
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
    cliPrint("Next: harpoon start (guest will auto-expand ext4 if backing > filesystem)")
    // filesystem UUID check will happen on next boot; old FS remains valid
    return 0
}

func handleDisk(args: [String]) -> Int32 {
    if args.isEmpty || args[0]=="help" || args[0]=="--help" || args[0]=="-h" {
        cliPrint("usage: harpoon disk <status|resize> [args]")
        cliPrint("  status              Show backing file, logical, physical, filesystem, used/free")
        cliPrint("  resize <new-size>   Grow sparse disk (e.g. harpoon disk resize 16G) — VM must be stopped, grow-only")
        return 0
    }
    switch args[0] {
    case "status": return handleDiskStatus()
    case "resize":
        if args.count < 2 { cliError("usage: harpoon disk resize <new-size>"); return 2 }
        return handleDiskResize(args: Array(args.dropFirst(1)))
    default:
        cliError("unknown disk subcommand: \(args[0])")
        return 2
    }
}
