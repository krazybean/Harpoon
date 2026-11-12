import Foundation

// Disk capacity policy: when to warn, which sizes to offer, and whether an expansion is allowed. Pure functions over numbers, so the
// thresholds and the refusal rules are unit-tested without a VM. Nothing here stops, starts or resizes anything.

enum DiskPressure: String {
    case unknown, ok, warning, critical
}

enum DiskPolicy {
    static let gib: UInt64 = 1024 * 1024 * 1024
    static let mib: UInt64 = 1024 * 1024

    // Thresholds are on the guest's df numbers. df "available" excludes ext4's 5% root reserve, but dockerd runs as root and can
    // spend that reserve, so waiting for available == 0 leaves no room to act. A percentage alone is wrong at both ends: 80% of
    // 512 GiB is 100 GiB of headroom, 80% of 4 GiB is under 1 GiB. Either condition warns, so small disks warn early on absolute space
    // and large ones on percentage.
    static let warnUsedPercent = 80.0
    static let criticalUsedPercent = 92.0
    static let warnAvailableBytes = 1 * gib
    static let criticalAvailableBytes = 384 * mib

    /// Ladder of sizes offered, in GiB. Only sizes above the current capacity are suggested.
    static let ladderGiB: [UInt64] = [4, 8, 16, 32, 64, 128, 256, 512]
    static let defaultMaxBytes = 512 * gib
    static let defaultHostReserveBytes = 5 * gib
    static let maxSuggestions = 3

    static func pressure(used: UInt64, available: UInt64) -> DiskPressure {
        let total = used + available
        guard total > 0 else { return .unknown }
        let usedPercent = Double(used) / Double(total) * 100
        if usedPercent >= criticalUsedPercent || available <= criticalAvailableBytes { return .critical }
        if usedPercent >= warnUsedPercent || available <= warnAvailableBytes { return .warning }
        return .ok
    }

    /// `HARPOON_DISK_MAX`, else 512 GiB. An unparseable value falls back to the default rather than lifting the cap.
    static func maxBytes(environment: String? = ProcessInfo.processInfo.environment["HARPOON_DISK_MAX"]) -> UInt64 {
        if let raw = environment, let bytes = RuntimeConfig.parseDiskSize(raw), bytes >= RuntimeConfig.minProvisionBytes { return bytes }
        return defaultMaxBytes
    }

    /// `HARPOON_DISK_HOST_RESERVE`, else 5 GiB: host space that must stay free after the disk's new capacity is counted.
    static func hostReserveBytes(environment: String? = ProcessInfo.processInfo.environment["HARPOON_DISK_HOST_RESERVE"]) -> UInt64 {
        if let raw = environment, let bytes = RuntimeConfig.parseDiskSize(raw) { return bytes }
        return defaultHostReserveBytes
    }

    // Host space policy. The disk image is a sparse APFS file, so growing it allocates (almost) nothing now; what a larger disk costs is
    // spread over three different quantities, and only the first two can be known when the user asks:
    //   immediate  - physical space the grow itself takes: ext4 metadata for the new block groups (inode tables, bitmaps), estimated
    //                at 2% of the added capacity (16 KiB per inode, 256-byte inodes is about 1.6%). Must fit with the reserve: HARD rule.
    //   worst case - what the host would need if the guest filled the whole disk: new logical size minus what is already allocated.
    //                Usually never needed, so it is not required to fit; it is limited to `oversubscriptionFactor` times the headroom
    //                (free space minus the reserve) so a Mac cannot be promised a disk it could never hold. Beyond the headroom the
    //                size is allowed but flagged `oversubscribed`, and the confirmation says so.
    //   reserve    - space that must stay free for macOS itself (default 5 GiB), applied to the immediate need and to the headroom.
    // Nothing in Harpoon stops a guest that fills an oversubscribed disk, which is why the flag is shown instead of hidden.
    static let metadataFraction = 0.02
    static let oversubscriptionFactor: UInt64 = 4

    struct Assessment: Equatable {
        var error: String?
        var immediateBytes: UInt64
        var worstCaseBytes: UInt64
        var oversubscribed: Bool
    }

    static func assess(current: UInt64, physical: UInt64, requested: UInt64, hostAvailable: UInt64?, maxBytes: UInt64, reserve: UInt64) -> Assessment {
        let added = requested > current ? requested - current : 0
        let immediate = UInt64((Double(added) * metadataFraction).rounded(.up))
        let worst = requested > physical ? requested - physical : 0
        func refuse(_ why: String) -> Assessment { Assessment(error: why, immediateBytes: immediate, worstCaseBytes: worst, oversubscribed: false) }
        if requested < RuntimeConfig.minProvisionBytes {
            return refuse("requested \(RuntimeConfig.formatBytes(requested)) is below the 2 GiB minimum")
        }
        if requested == current {
            return refuse("already \(RuntimeConfig.formatBytes(current)); nothing to grow (grow-only)")
        }
        if requested < current {
            return refuse("shrinking is not supported: requested \(RuntimeConfig.formatBytes(requested)) is below the current \(RuntimeConfig.formatBytes(current))")
        }
        if requested > maxBytes {
            return refuse("requested \(RuntimeConfig.formatBytes(requested)) exceeds the growth cap of \(RuntimeConfig.formatBytes(maxBytes)) (HARPOON_DISK_MAX)")
        }
        guard let host = hostAvailable else {
            return refuse("host free space could not be read; refusing to grow without checking the \(RuntimeConfig.formatBytes(reserve)) reserve")
        }
        if immediate > host || host - immediate < reserve {
            return refuse("not enough host space: the grow itself needs about \(RuntimeConfig.formatBytes(immediate)) for filesystem metadata plus a \(RuntimeConfig.formatBytes(reserve)) reserve, host has \(RuntimeConfig.formatBytes(host)) free (HARPOON_DISK_HOST_RESERVE)")
        }
        let headroom = host - reserve
        let (limit, overflow) = headroom.multipliedReportingOverflow(by: oversubscriptionFactor)
        if !overflow && worst > limit {
            return refuse("a \(RuntimeConfig.formatBytes(requested)) disk could need \(RuntimeConfig.formatBytes(worst)) of host space if it filled, more than \(oversubscriptionFactor) times the \(RuntimeConfig.formatBytes(headroom)) this Mac can spare after the \(RuntimeConfig.formatBytes(reserve)) reserve; choose a smaller size or free space")
        }
        return Assessment(error: nil, immediateBytes: immediate, worstCaseBytes: worst, oversubscribed: worst > headroom)
    }

    /// Why growing is refused, or nil.
    static func expansionError(current: UInt64, physical: UInt64, requested: UInt64, hostAvailable: UInt64?, maxBytes: UInt64, reserve: UInt64) -> String? {
        assess(current: current, physical: physical, requested: requested, hostAvailable: hostAvailable, maxBytes: maxBytes, reserve: reserve).error
    }

    /// Up to three ladder sizes above `current` that `assess` accepts, each with its oversubscription flag.
    static func suggestions(current: UInt64, physical: UInt64, hostAvailable: UInt64?, maxBytes: UInt64, reserve: UInt64) -> [(bytes: UInt64, oversubscribed: Bool)] {
        var out: [(bytes: UInt64, oversubscribed: Bool)] = []
        for g in ladderGiB {
            let size = g * gib
            if size <= current { continue }
            let a = assess(current: current, physical: physical, requested: size, hostAvailable: hostAvailable, maxBytes: maxBytes, reserve: reserve)
            if a.error != nil { continue }
            out.append((size, a.oversubscribed))
            if out.count == maxSuggestions { break }
        }
        return out
    }

    /// `df` reports usable ext4 capacity: the journal, inode tables and reserved GDT blocks (about 3 to 5% on the shipped layout) are not in it.
    /// A backing file larger than that by more than 6% means the filesystem was not finished growing; anything smaller is just ext4 geometry.
    static func filesystemMatchesBacking(capacity: UInt64, logical: UInt64) -> Bool {
        capacity >= logical || logical - capacity <= logical / 100 * 6
    }

    /// A grown filesystem is "expanded" when it reports most of the added capacity, judged against the old LOGICAL size so an unknown
    /// earlier reading cannot make a filesystem that did not grow look grown. ext4 metadata takes a little, hence 75%.
    static func filesystemExpanded(after: UInt64, from: UInt64, to: UInt64) -> Bool {
        guard to > from else { return false }
        return Double(after) >= Double(from) + Double(to - from) * 0.75
    }
}

/// Written (and read back) before an expansion stops anything, and removed once it is verified, so an interrupted run is visible and
/// explained. Growing the backing file is idempotent and the guest finishes the filesystem on boot; nothing is rolled back.
struct DiskExpansionRecord: Codable, Equatable {
    var phase: String // stopping | growing | starting | verifying
    var fromBytes: UInt64
    var toBytes: UInt64
    var diskPath: String
    var startedAt: String
    var pid: Int32? = nil // the process performing the expansion; a dead pid means the run was interrupted

    static var path: String { HarpoonPaths.appSupportDir.appendingPathComponent("disk-expansion.json").path }

    static func load(at path: String = DiskExpansionRecord.path) -> DiskExpansionRecord? {
        guard let data = try? Data(contentsOf: URL(fileURLWithPath: path)) else { return nil }
        return try? JSONDecoder().decode(DiskExpansionRecord.self, from: data)
    }

    /// Durably replaces the record (temp file, fsync, rename, directory fsync) and reads it back. Returns why it could not, or nil.
    /// The caller must not stop or resize anything for a record that was not saved.
    func save(at path: String = DiskExpansionRecord.path) -> String? {
        let dir = (path as NSString).deletingLastPathComponent
        do { try FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true) }
        catch { return "cannot create \(dir): \(error.localizedDescription)" }
        guard let data = try? JSONEncoder().encode(self) else { return "cannot encode the record" }
        let tmp = path + ".tmp"
        let fd = open(tmp, O_WRONLY | O_CREAT | O_TRUNC, 0o600)
        if fd < 0 { return "cannot create \(tmp): \(String(cString: strerror(errno)))" }
        var written = 0
        while written < data.count {
            let n = data.withUnsafeBytes { write(fd, $0.baseAddress!.advanced(by: written), data.count - written) }
            if n < 0 { if errno == EINTR { continue }; let why = String(cString: strerror(errno)); close(fd); unlink(tmp); return "cannot write \(tmp): \(why)" }
            written += n
        }
        if fcntl(fd, F_FULLFSYNC) != 0 && fsync(fd) != 0 { let why = String(cString: strerror(errno)); close(fd); unlink(tmp); return "cannot sync \(tmp): \(why)" }
        close(fd)
        if rename(tmp, path) != 0 { let why = String(cString: strerror(errno)); unlink(tmp); return "cannot replace \(path): \(why)" }
        let dfd = open(dir, O_RDONLY)
        if dfd >= 0 { fsync(dfd); close(dfd) }
        return DiskExpansionRecord.load(at: path) == self ? nil : "the record did not read back from \(path)"
    }

    static func clear(at path: String = DiskExpansionRecord.path) { try? FileManager.default.removeItem(atPath: path) }

    /// True while the process that wrote it is still a running Harpoon: an expansion is in progress, not interrupted.
    var isActive: Bool {
        guard let pid, isProcessAlive(pid: pid) else { return false }
        return isHarpoonProcess(pid: pid)
    }

    /// What the user should do. Judged by what the disk file is NOW, not by the last phase written, because a phase update that could
    /// not be saved leaves an older phase behind.
    func recovery(currentLogical: UInt64?) -> String {
        let size = RuntimeConfig.formatBytes(toBytes).replacingOccurrences(of: " GiB", with: "G")
        if (currentLogical ?? 0) >= toBytes {
            return "The disk file is already \(RuntimeConfig.formatBytes(currentLogical ?? toBytes)). Run `harpoon start` if Harpoon is stopped (or `harpoon restart` if it is running) so the guest expands the filesystem on boot, then check `harpoon disk status`. Nothing was lost."
        }
        return "The disk was not grown (still \(RuntimeConfig.formatBytes(currentLogical ?? fromBytes))); nothing was lost. Run `harpoon status`, start Harpoon if it is stopped, and retry `harpoon disk expand \(size) --yes`."
    }
}

/// What a restart would change compared with the VM that is running. An expansion restarts the VM, so any difference is refused
/// before anything is stopped: it must come back with exactly the CPUs, memory, disk, kernel and initramfs it had.
func expansionConfigMismatches(meta: RuntimeMetadata, resolved cfg: RuntimeConfig) -> [String] {
    func same(_ a: String, _ b: String) -> Bool {
        URL(fileURLWithPath: a).standardized.resolvingSymlinksInPath().path == URL(fileURLWithPath: b).standardized.resolvingSymlinksInPath().path
    }
    var out: [String] = []
    if meta.cpus != cfg.cpuCount { out.append("cpus: running with \(meta.cpus), a restart would use \(cfg.cpuCount)") }
    if meta.memoryMiB != cfg.memoryMIB { out.append("memory: running with \(meta.memoryMiB) MiB, a restart would use \(cfg.memoryMIB) MiB") }
    let a = diskIdentity(at: meta.diskPath), b = diskIdentity(at: cfg.diskURL.path)
    if a == nil || b == nil ? !same(meta.diskPath, cfg.diskURL.path) : a != b {
        out.append("disk: running with \(meta.diskPath), a restart would use \(cfg.diskURL.path)")
    }
    if let k = meta.kernelPath { if !same(k, cfg.kernelURL.path) { out.append("kernel: running with \(k), a restart would use \(cfg.kernelURL.path)") } }
    else { out.append("kernel: not recorded by the Harpoon that started this VM (an older build), so the same kernel cannot be guaranteed") }
    if let i = meta.initramfsPath { if !same(i, cfg.initramfsURL.path) { out.append("initramfs: running with \(i), a restart would use \(cfg.initramfsURL.path)") } }
    else { out.append("initramfs: not recorded by the Harpoon that started this VM (an older build), so the same initramfs cannot be guaranteed") }
    return out
}

/// The message when another lifecycle operation holds the lock, naming a disk expansion when that is what it is.
func controlBusyMessage() -> String {
    if let r = DiskExpansionRecord.load(), r.isActive {
        return "HARPOON_CONTROL_BUSY: a disk expansion is in progress (\(r.phase), \(RuntimeConfig.formatBytes(r.fromBytes)) -> \(RuntimeConfig.formatBytes(r.toBytes))); wait for it to finish"
    }
    return "HARPOON_CONTROL_BUSY: another lifecycle operation is in progress"
}

/// User-confirmed space reclamation: guest `fstrim /` punches holes in the sparse image for blocks ext4 no longer uses. Nothing is
/// promised beforehand; the result is the measured change in the file's APFS allocation, which may be zero (or even a small growth,
/// because the guest keeps writing). The logical size never changes and no Docker object is touched.
enum DiskReclaim {
    enum TrimResult: Equatable { case ok, unsupported, failed }

    /// BusyBox/util-linux report "not supported" when the device or filesystem takes no discards, and 127 / "not found" without fstrim.
    static func classifyTrim(code: Int32, stderr: String) -> TrimResult {
        if code == 0 { return .ok }
        let e = stderr.lowercased()
        if code == 127 || e.contains("not supported") || e.contains("not found") || e.contains("no such file") || e.contains("applet") { return .unsupported }
        return .failed
    }

    struct Report: Equatable {
        var logicalBefore: UInt64, logicalAfter: UInt64
        var physicalBefore: UInt64, physicalAfter: UInt64
        var logicalPreserved: Bool { logicalBefore == logicalAfter }
        /// Signed: negative means the allocation shrank.
        var changeBytes: Int64 { Int64(clamping: physicalAfter) - Int64(clamping: physicalBefore) }
        var reclaimedBytes: UInt64 { physicalBefore > physicalAfter ? physicalBefore - physicalAfter : 0 }
        var message: String {
            if !logicalPreserved { return "The disk's logical size changed from \(RuntimeConfig.formatBytes(logicalBefore)) to \(RuntimeConfig.formatBytes(logicalAfter)); this should never happen, run `harpoon disk status`." }
            if reclaimedBytes > 0 { return "Reclaimed \(RuntimeConfig.formatBytes(reclaimedBytes)) of host disk space (\(RuntimeConfig.formatBytes(physicalBefore)) -> \(RuntimeConfig.formatBytes(physicalAfter)))." }
            if changeBytes > 0 { return "Nothing was reclaimed: the allocation grew by \(RuntimeConfig.formatBytes(UInt64(changeBytes))) while trimming because the guest kept writing." }
            return "Nothing was reclaimed: the allocation is unchanged at \(RuntimeConfig.formatBytes(physicalAfter)). The guest had no unused blocks to give back."
        }
    }
}
