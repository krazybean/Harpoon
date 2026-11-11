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

    /// Why growing a disk from `current` to `requested` is refused, or nil. The host rule is deliberately conservative: the added
    /// capacity may all be written, so it plus the reserve must fit in the host's free space. Unknown host space refuses.
    static func expansionError(current: UInt64, requested: UInt64, hostAvailable: UInt64?, maxBytes: UInt64, reserve: UInt64) -> String? {
        if requested < RuntimeConfig.minProvisionBytes {
            return "requested \(RuntimeConfig.formatBytes(requested)) is below the 2 GiB minimum"
        }
        if requested == current {
            return "already \(RuntimeConfig.formatBytes(current)); nothing to grow (grow-only)"
        }
        if requested < current {
            return "shrinking is not supported: requested \(RuntimeConfig.formatBytes(requested)) is below the current \(RuntimeConfig.formatBytes(current))"
        }
        if requested > maxBytes {
            return "requested \(RuntimeConfig.formatBytes(requested)) exceeds the growth cap of \(RuntimeConfig.formatBytes(maxBytes)) (HARPOON_DISK_MAX)"
        }
        guard let host = hostAvailable else {
            return "host free space could not be read; refusing to grow without checking the \(RuntimeConfig.formatBytes(reserve)) reserve"
        }
        let added = requested - current
        if added > host || host - added < reserve {
            return "not enough host space: growing by \(RuntimeConfig.formatBytes(added)) needs it plus a \(RuntimeConfig.formatBytes(reserve)) reserve, host has \(RuntimeConfig.formatBytes(host)) free (HARPOON_DISK_HOST_RESERVE)"
        }
        return nil
    }

    /// Up to three ladder sizes above `current` that `expansionError` would accept.
    static func suggestedSizes(current: UInt64, hostAvailable: UInt64?, maxBytes: UInt64, reserve: UInt64) -> [UInt64] {
        var out: [UInt64] = []
        for g in ladderGiB {
            let size = g * gib
            if size <= current { continue }
            if expansionError(current: current, requested: size, hostAvailable: hostAvailable, maxBytes: maxBytes, reserve: reserve) != nil { continue }
            out.append(size)
            if out.count == maxSuggestions { break }
        }
        return out
    }

    /// The guest's ext4 loses a little to metadata, so a grown filesystem is "expanded" when it gained most of the added capacity.
    static func filesystemExpanded(before: UInt64, after: UInt64, from: UInt64, to: UInt64) -> Bool {
        guard to > from else { return false }
        let gained = after > before ? after - before : 0
        return Double(gained) >= Double(to - from) * 0.75
    }
}

/// Written before an expansion starts and removed when it is verified, so an interrupted run is visible and explained. Growing the
/// backing file is idempotent and the guest finishes the filesystem on boot; nothing is rolled back.
struct DiskExpansionRecord: Codable, Equatable {
    var phase: String // stopping | growing | starting | verifying
    var fromBytes: UInt64
    var toBytes: UInt64
    var diskPath: String
    var startedAt: String

    static var path: String { HarpoonPaths.appSupportDir.appendingPathComponent("disk-expansion.json").path }

    static func load(at path: String = DiskExpansionRecord.path) -> DiskExpansionRecord? {
        guard let data = try? Data(contentsOf: URL(fileURLWithPath: path)) else { return nil }
        return try? JSONDecoder().decode(DiskExpansionRecord.self, from: data)
    }

    func save(at path: String = DiskExpansionRecord.path) {
        if let data = try? JSONEncoder().encode(self) { try? data.write(to: URL(fileURLWithPath: path), options: .atomic) }
    }

    static func clear(at path: String = DiskExpansionRecord.path) { try? FileManager.default.removeItem(atPath: path) }

    /// What the user should do, given how far the interrupted expansion got and what the disk looks like now.
    func recovery(currentLogical: UInt64?) -> String {
        let grown = (currentLogical ?? 0) >= toBytes
        switch phase {
        case "stopping":
            return "Harpoon may still be running or may have stopped; the disk was not changed. Run `harpoon status`, then retry `harpoon disk expand \(RuntimeConfig.formatBytes(toBytes).replacingOccurrences(of: " GiB", with: "G")) --yes`."
        case "growing":
            return grown
                ? "The disk file was grown to \(RuntimeConfig.formatBytes(toBytes)). Run `harpoon start`; the guest expands the filesystem on boot."
                : "The disk was not grown (still \(RuntimeConfig.formatBytes(currentLogical ?? fromBytes))). Retry `harpoon disk expand \(RuntimeConfig.formatBytes(toBytes).replacingOccurrences(of: " GiB", with: "G")) --yes`."
        default:
            return "The disk is \(RuntimeConfig.formatBytes(currentLogical ?? toBytes)). Run `harpoon start` if Harpoon is stopped, then `harpoon disk status`; the guest expands the filesystem on boot."
        }
    }
}
