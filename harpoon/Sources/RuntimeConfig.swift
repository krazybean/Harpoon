import Foundation
import Virtualization
import CryptoKit

/// The lifecycle namespace: lock files, sockets, stop file, serial log, state directory and disk.
/// Production names are fixed (/tmp/harpoon-*, ~/Library/Application Support/Harpoon).
/// HARPOON_TEST_TMPDIR moves ALL of it under one directory and switches off every production
/// fallback, so a test can never read, lock, signal or stop a real engine's files.
enum HarpoonRoots {
    static let socketNameBudget = 100 // sun_path is 104 bytes, longest name is /harpoon-docker.sock

    static var testRoot: String? {
        guard let v = ProcessInfo.processInfo.environment["HARPOON_TEST_TMPDIR"], !v.isEmpty else { return nil }
        return v
    }
    static var runDir: String { testRoot ?? "/tmp" }
    static var productionSupport: String {
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Harpoon").path
    }
    static var productionDisk: String { productionSupport + "/data/harpoon-root.img" }

    private static func resolved(_ p: String) -> String { URL(fileURLWithPath: p).resolvingSymlinksInPath().path }

    /// nil when the namespace is usable; otherwise why it is not. Evaluated before any command runs.
    static func isolationError() -> String? {
        guard let root = testRoot else { return nil }
        let env = ProcessInfo.processInfo.environment
        guard root.hasPrefix("/") else { return "HARPOON_TEST_TMPDIR must be an absolute path, got \(root)" }
        do { try FileManager.default.createDirectory(atPath: root, withIntermediateDirectories: true, attributes: nil) }
        catch { return "HARPOON_TEST_TMPDIR \(root) cannot be created: \(error.localizedDescription)" }
        var isDir: ObjCBool = false
        guard FileManager.default.fileExists(atPath: root, isDirectory: &isDir), isDir.boolValue,
              FileManager.default.isWritableFile(atPath: root) else { return "HARPOON_TEST_TMPDIR \(root) is not a writable directory" }
        let real = resolved(root)
        if real == resolved("/tmp") { return "HARPOON_TEST_TMPDIR resolves to /tmp: its lock and socket names would be the production names" }
        if real == resolved(productionSupport) { return "HARPOON_TEST_TMPDIR resolves to the production state directory" }
        if (root + "/harpoon-docker.sock").utf8.count > socketNameBudget { return "HARPOON_TEST_TMPDIR is too long for a unix socket path (\(root))" }
        if let sock = env["HARPOON_DOCKER_SOCK"], !sock.isEmpty, sock != root + "/harpoon-docker.sock" {
            return "HARPOON_DOCKER_SOCK overrides the isolated Docker socket (\(sock))"
        }
        if let disk = env["HARPOON_DISK"], !disk.isEmpty, resolved(disk) == resolved(productionDisk) {
            return "HARPOON_DISK is the production persistent disk"
        }
        return nil
    }

    /// Disk lookup order. In isolation only the test root's own disk is ever considered.
    static func userDiskCandidates(testFlags: Bool) -> [String] {
        if let root = testRoot { return [root + "/data/harpoon-root.img"] }
        var paths = [productionDisk]
        if testFlags { paths.append("/tmp/harpoon-runtime/data/harpoon-root.img") }
        return paths
    }
    static func configCandidates() -> [String] {
        if let root = testRoot { return [root + "/config.json"] }
        return [productionSupport + "/config.json", "/tmp/harpoon-runtime/config.json"]
    }
}

struct SharedRoot {
    let hostPath: String
    let guestPath: String
    let tag: String
}

// ponytail: minimal production config — no public config system yet (M8), defaults reproduce Spike 5
struct RuntimeConfig {
    var cpuCount: Int = 2
    var memoryMIB: Int = 4096
    var memorySizeBytes: UInt64 { UInt64(memoryMIB) * 1024 * 1024 }

    var kernelURL: URL = RuntimeConfig.resolveResource(named: "Image-virt", fallback: "assets/guest/Image-virt")
    var initramfsURL: URL = RuntimeConfig.resolveResource(named: "harpoon-initramfs.cpio.gz", fallback: "assets/guest/harpoon-initramfs.cpio.gz")
    // A path only: resolving a config never creates a disk. Creation is provisionedRootDisk(for:), called where a VM is about to start.
    var diskURL: URL = RuntimeConfig.resolveRootDiskPath()

    var shareHostPath: String = HarpoonRoots.runDir + "/harpoon-share"
    var virtioFSTag: String = "harpoon-share"
    // M4 shared roots: /Users and /private/tmp (covers /tmp symlink via translator alias, no separate /tmp device)
    var sharedRoots: [SharedRoot] {
        return [
            SharedRoot(hostPath: "/Users", guestPath: "/mnt/harpoon-host/Users", tag: "harpoon-host-users"),
            SharedRoot(hostPath: "/private/tmp", guestPath: "/mnt/harpoon-host/tmp", tag: "harpoon-host-tmp"),
        ]
    }
    // legacy single share kept for backward compat via first roots entry? Actually we keep explicit
    var allVirtioShares: [(host: String, tag: String)] {
        var shares: [(String, String)] = []
        shares.append((shareHostPath, virtioFSTag))
        // deduplicate tags: harpoon-host-tmp appears twice (tmp alias) — only add once per tag
        var seen = Set<String>()
        seen.insert(virtioFSTag)
        for r in sharedRoots {
            if !seen.contains(r.tag) {
                // only add if host path exists
                if FileManager.default.fileExists(atPath: r.hostPath) || r.hostPath == "/tmp" || r.hostPath == "/private/tmp" {
                    shares.append((r.hostPath, r.tag))
                    seen.insert(r.tag)
                }
            } else if r.tag == "harpoon-host-tmp-alias" {
                // alias shares same underlying host dir as /private/tmp, skip duplicate device
                continue
            }
        }
        // Ensure /Users share always added even if /Users not exists? It exists on macOS
        return shares
    }

    var dockerSocketPath: String = HarpoonRoots.runDir + "/harpoon-docker.sock"
    var balloonControlPath: String = HarpoonRoots.runDir + "/harpoon-control"
    var mgmtSocketPath: String = HarpoonRoots.runDir + "/harpoon-mgmt.sock"
    var serialLogPath: String = HarpoonRoots.runDir + "/harpoon-serial.log"

    var vsockPort: UInt32 = 2375
    var mgmtVsockPort: UInt32 = 2377 // Stage 3A management channel — vsock-only, no TCP, no SSH
    var hostForwardPort: UInt16 = 8080
    var guestForwardPort: UInt16 = 8080

    var bootTimeout: TimeInterval = 120
    var dockerReadyTimeout: TimeInterval = 120

    static func executableURLs() -> [URL] {
        var urls: [URL] = []
        let arg0 = CommandLine.arguments.first ?? ""
        if !arg0.isEmpty {
            let url = arg0.hasPrefix("/")
                ? URL(fileURLWithPath: arg0)
                : URL(fileURLWithPath: FileManager.default.currentDirectoryPath).appendingPathComponent(arg0)
            urls.append(url.standardized.resolvingSymlinksInPath())
        }
        if let bundle = Bundle.main.executableURL {
            urls.append(bundle.standardized.resolvingSymlinksInPath())
        }
        return urls
    }

    static func isDirectory(_ url: URL) -> Bool {
        var isDirectory: ObjCBool = false
        return FileManager.default.fileExists(atPath: url.path, isDirectory: &isDirectory) && isDirectory.boolValue
    }

    static func installedLibDir() -> URL? {
        // Harpoon.app bundle: Resources/harpoon/lib/harpoon (when harpoon is at Resources/harpoon/bin/harpoon)
        // Tauri flat resources: Resources/harpoon or Resources/
        for exec in executableURLs() {
            let binDir = exec.deletingLastPathComponent()
            // Bundle: <Harpoon.app>/Contents/MacOS/harpoon-desktop -> ../Resources/harpoon/lib/harpoon
            // For harpoon binary itself at Resources/harpoon/bin/harpoon, binDir is .../Resources/harpoon/bin
            let bundleCandidates: [URL] = [
                binDir.deletingLastPathComponent().appendingPathComponent("lib/harpoon"), // Resources/harpoon/lib/harpoon
                binDir.appendingPathComponent("../lib/harpoon").standardized,
                binDir.appendingPathComponent("../../Resources/harpoon/lib/harpoon").standardized, // from MacOS/harpoon-desktop
                URL(fileURLWithPath: binDir.path).deletingLastPathComponent().appendingPathComponent("../Resources/harpoon/lib/harpoon").standardized,
                binDir.appendingPathComponent("../Resources/harpoon").standardized,
                // Current Tauri bundle-resources layout (Resources/bundle-resources/harpoon)
                binDir.appendingPathComponent("../Resources/bundle-resources/harpoon/lib/harpoon").standardized,
                binDir.appendingPathComponent("../../Resources/bundle-resources/harpoon/lib/harpoon").standardized,
                URL(fileURLWithPath: binDir.path).deletingLastPathComponent().appendingPathComponent("../Resources/bundle-resources/harpoon/lib/harpoon").standardized,
                binDir.appendingPathComponent("../Resources/bundle-resources/harpoon").standardized,
            ]
            for c in bundleCandidates {
                if isDirectory(c) { return c }
            }
        }
        if let res = Bundle.main.resourceURL {
            for path in ["harpoon/lib/harpoon", "bundle-resources/harpoon/lib/harpoon", "harpoon", "bundle-resources/harpoon"] {
                let candidate = res.appendingPathComponent(path)
                if isDirectory(candidate) { return candidate }
            }
        }
        // Relative to executable: <prefix>/bin/harpoon -> <prefix>/lib/harpoon
        for exec in executableURLs() {
            let binDir = exec.deletingLastPathComponent()
            let lib1 = binDir.deletingLastPathComponent().appendingPathComponent("lib/harpoon")
            if isDirectory(lib1) { return lib1 }
            let lib2 = binDir.appendingPathComponent("../lib/harpoon").standardized
            if isDirectory(lib2) { return lib2 }
        }
        return nil
    }

    static func sourceAssetRoot() -> URL? {
        let fm = FileManager.default
        for exec in executableURLs() {
            let root = exec.deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            let assets = root.appendingPathComponent("assets/guest")
            if fm.fileExists(atPath: guestAssetDir(assets).appendingPathComponent("Image-virt").path),
               fm.fileExists(atPath: guestAssetDir(assets).appendingPathComponent("harpoon-initramfs.cpio.gz").path) { return assets }
        }
        let assets = URL(fileURLWithPath: FileManager.default.currentDirectoryPath).appendingPathComponent("assets/guest")
        if fm.fileExists(atPath: guestAssetDir(assets).appendingPathComponent("Image-virt").path),
           fm.fileExists(atPath: guestAssetDir(assets).appendingPathComponent("harpoon-initramfs.cpio.gz").path) { return assets }
        return nil
    }

    // Guest assets for the host architecture: the arm64 guest sits at the asset
    // root and the x86_64 guest in an x86_64/ subdirectory (universal builds ship both).
    static func guestAssetDir(_ base: URL) -> URL {
        #if arch(x86_64)
        return base.appendingPathComponent("x86_64")
        #else
        return base
        #endif
    }

    static func resolveResource(named: String, fallback: String) -> URL {
        // Explicit overrides are applied in fromEnvironment. An installed resource root wins over CWD.
        if let lib = installedLibDir() {
            return guestAssetDir(lib).appendingPathComponent(named)
        }
        if let assets = sourceAssetRoot() { return guestAssetDir(assets).appendingPathComponent(named) }
        return guestAssetDir(URL(fileURLWithPath: fallback).deletingLastPathComponent()).appendingPathComponent(named)
    }

    static func rootTemplateURL() -> URL {
        if let path = ProcessInfo.processInfo.environment["HARPOON_TEST_ROOT"], !path.isEmpty { return URL(fileURLWithPath: path) }
        if let lib = installedLibDir() { return guestAssetDir(lib).appendingPathComponent("harpoon-root.img") }
        if let assets = sourceAssetRoot() { return guestAssetDir(assets).appendingPathComponent("harpoon-root.img") }
        return guestAssetDir(URL(fileURLWithPath: "assets/guest")).appendingPathComponent("harpoon-root.img")
    }

    /// Where the persistent root disk is, or would be created: the first existing user disk, else the default destination. Creates nothing.
    static func resolveRootDiskPath() -> URL {
        if let existing = existingUserDiskPath() { return URL(fileURLWithPath: existing) }
        return URL(fileURLWithPath: defaultRootDiskDestination())
    }

    static func defaultRootDiskDestination() -> String {
        let env = ProcessInfo.processInfo.environment
        let isTest = env["HARPOON_ALLOW_TMP_FALLBACK"] == "1" || env["HARPOON_TEST_MODE"] == "1" || env["HARPOON_TEST_TMPDIR"] != nil
        return HarpoonRoots.userDiskCandidates(testFlags: isTest)[0]
    }

    /// True when `url` is the default destination, nothing exists there, and a template is available to provision it from.
    static func wouldProvisionRootDisk(_ url: URL) -> Bool {
        url.path == defaultRootDiskDestination()
            && !FileManager.default.fileExists(atPath: url.path)
            && existingUserDiskPath() == nil
            && FileManager.default.fileExists(atPath: rootTemplateURL().path)
    }

    /// The disk a VM start should use. Provisions the default disk from the template on first run; an explicit path
    /// (`--disk`, `HARPOON_DISK`) or an existing disk is returned unchanged and nothing is created.
    static func provisionedRootDisk(for url: URL) -> URL {
        guard wouldProvisionRootDisk(url) else { return url }
        let made = provisionDefaultRootDisk()
        return made.path == rootTemplateURL().path ? url : made
    }

    private static func provisionDefaultRootDisk() -> URL {
        // User-writable disk takes precedence if exists — NEVER silently delete/replace
        // because size differs from immutable template; template is read-only, user disk is mutable.
        // ponytail: no template-size-equality invariant for already-provisioned disk (resize deferred)
        // Production: only ~/Library/...; /tmp fallback only when explicitly enabled for tests (HARPOON_ALLOW_TMP_FALLBACK=1)
        let isTest = ProcessInfo.processInfo.environment["HARPOON_ALLOW_TMP_FALLBACK"] == "1" || ProcessInfo.processInfo.environment["HARPOON_TEST_MODE"] == "1" || ProcessInfo.processInfo.environment["HARPOON_TEST_TMPDIR"] != nil
        let userPaths = HarpoonRoots.userDiskCandidates(testFlags: isTest)
        for p in userPaths {
            if FileManager.default.fileExists(atPath: p) { return URL(fileURLWithPath: p) }
        }
        // Otherwise use template from installed lib or fallback
        let tmpl = rootTemplateURL()
        if FileManager.default.fileExists(atPath: tmpl.path) {
                // provision on first run: copy to user location — production MUST be ~/Library, fail if not writable
                let userDest = URL(fileURLWithPath: userPaths[0])
                let dest: URL
                do {
                    try FileManager.default.createDirectory(at: userDest.deletingLastPathComponent(), withIntermediateDirectories: true, attributes: nil)
                    dest = userDest
                } catch {
                    if isTest && HarpoonRoots.testRoot == nil {
                        let fallback = URL(fileURLWithPath: "/tmp/harpoon-runtime/data/harpoon-root.img")
                        try? FileManager.default.createDirectory(at: fallback.deletingLastPathComponent(), withIntermediateDirectories: true, attributes: nil)
                        dest = fallback
                    } else {
                        dest = userDest
                    }
                }
                if !FileManager.default.fileExists(atPath: dest.path) {
                    // clone-aware copy: APFS clone via cp -c preserves sparseness; FileManager.copyItem truncates holes on some OS versions (36M vs 962M)
                    let tmplPath = tmpl.path
                    let destPath = dest.path
                    var copied = false
                    let cp = Process()
                    cp.executableURL = URL(fileURLWithPath: "/bin/cp")
                    cp.arguments = ["-c", "-p", tmplPath, destPath]
                    do {
                        try cp.run()
                        cp.waitUntilExit()
                        copied = cp.terminationStatus == 0 && FileManager.default.fileExists(atPath: destPath)
                    } catch { copied = false }
                    if !copied {
                        // fallback: ditto clone, then FileManager
                        let ditto = Process()
                        ditto.executableURL = URL(fileURLWithPath: "/usr/bin/ditto")
                        ditto.arguments = [tmplPath, destPath]
                        do { try ditto.run(); ditto.waitUntilExit(); copied = ditto.terminationStatus == 0 } catch { copied = false }
                    }
                    if !copied {
                        try? FileManager.default.copyItem(at: tmpl, to: dest)
                    }
                    // verify logical size matches source; if mismatch, retry with FileManager
                    if let s = try? FileManager.default.attributesOfItem(atPath: tmplPath), let ss = s[.size] as? UInt64,
                       let d = try? FileManager.default.attributesOfItem(atPath: destPath), let ds = d[.size] as? UInt64,
                       ss != ds {
                        try? FileManager.default.removeItem(atPath: destPath)
                        try? FileManager.default.copyItem(at: tmpl, to: dest)
                    }
                    // Stage 3C: first-provision sparse grow to default 8G (or config) — template is small (2G) for distribution
                    let desired = desiredProvisionBytes()
                    if let attrs = try? FileManager.default.attributesOfItem(atPath: destPath), let cur = attrs[.size] as? UInt64, cur < desired {
                        // sparse truncate — does not allocate physical blocks
                        if let fh = FileHandle(forWritingAtPath: destPath) {
                            try? fh.truncate(atOffset: desired)
                            try? fh.close()
                        } else {
                            // fallback via truncate
                            let t = Process(); t.executableURL = URL(fileURLWithPath: "/usr/bin/truncate"); t.arguments = ["-s", "\(desired)", destPath]; try? t.run(); t.waitUntilExit()
                        }
                    }
                }
                return dest
        }
        return tmpl
    }

    static func fromEnvironment() -> RuntimeConfig {
        var c = RuntimeConfig()
        // user config (file) overrides defaults, env overrides config, CLI overrides all
        // load user config if present
        let configCandidates = HarpoonRoots.configCandidates()
        var fileCfg: HarpoonUserConfig? = nil
        for cand in configCandidates {
            if let data = try? Data(contentsOf: URL(fileURLWithPath: cand)),
               let decoded = try? JSONDecoder().decode(HarpoonUserConfig.self, from: data) {
                fileCfg = decoded
                break
            }
        }
        if let fc = fileCfg {
            if let v = fc.cpus { c.cpuCount = v }
            if let v = fc.memory { c.memoryMIB = v }
        }
        if let raw = ProcessInfo.processInfo.environment["HARPOON_CPUS"],
           let parsed = Int(raw) {
            c.cpuCount = parsed
        }
        if let raw = ProcessInfo.processInfo.environment["HARPOON_MEMORY_MIB"],
           let parsed = Int(raw) {
            c.memoryMIB = parsed
        }
        if let p = ProcessInfo.processInfo.environment["HARPOON_KERNEL"] { c.kernelURL = URL(fileURLWithPath: p) }
        if let p = ProcessInfo.processInfo.environment["HARPOON_INITRAMFS"] { c.initramfsURL = URL(fileURLWithPath: p) }
        if let p = ProcessInfo.processInfo.environment["HARPOON_DISK"] { c.diskURL = URL(fileURLWithPath: p) }
        if let p = ProcessInfo.processInfo.environment["HARPOON_SHARE"] { c.shareHostPath = p }
        if let p = ProcessInfo.processInfo.environment["HARPOON_DOCKER_SOCK"], HarpoonRoots.testRoot == nil { c.dockerSocketPath = p }
        return c
    }

    func validate() -> String? {
        // CPU: validate against VZ limits (minimum 1, maximum from framework)
        if cpuCount < 1 { return "cpuCount must be >=1, got \(cpuCount)" }
        #if canImport(Virtualization)
        let minCPU = 1 // VZVirtualMachineConfiguration.minimumAllowedCPUCount is 1 on macOS 13+, but import for check
        let maxCPU = 8 // conservative Phase1; framework maximumAllowedCPUCount may be larger (e.g. 32) but 8 is safe for M6
        if cpuCount < minCPU || cpuCount > maxCPU { return "cpuCount must be \(minCPU)...\(maxCPU), got \(cpuCount)" }
        #endif
        if let error = Self.memoryValidationError(memoryMIB) { return error }
        if !FileManager.default.fileExists(atPath: kernelURL.path) { return "kernel not found: \(kernelURL.path)" }
        if !FileManager.default.fileExists(atPath: initramfsURL.path) { return "initramfs not found: \(initramfsURL.path)" }
        if let mismatch = Self.guestAssetValidationError(kernelURL: kernelURL, initramfsURL: initramfsURL) { return mismatch }
        if FileManager.default.fileExists(atPath: diskURL.path),
           let attrs = try? FileManager.default.attributesOfItem(atPath: diskURL.path),
           let type = attrs[.type] as? FileAttributeType,
           type != .typeRegular {
            return "persistent disk identity invalid: not a regular file \(diskURL.path)"
        }
        // disk is optional for validation — ramdisk fallback exists but production expects block
        return nil
    }

    static func guestAssetValidationError(kernelURL: URL, initramfsURL: URL) -> String? {
        let manifestURL = kernelURL.deletingLastPathComponent().appendingPathComponent("guest-assets.manifest")
        guard let data = try? Data(contentsOf: manifestURL),
              let text = String(data: data, encoding: .utf8) else {
            return "GUEST_ASSET_MISMATCH manifest missing: \(manifestURL.path)"
        }
        guard text.split(separator: "\n").first == "harpoon-guest-assets-v1" else {
            return "GUEST_ASSET_MISMATCH invalid manifest: \(manifestURL.path)"
        }
        let fields = Dictionary(text.split(separator: "\n").dropFirst().compactMap { line -> (String, String)? in
            let parts = line.split(separator: " ", maxSplits: 1).map(String.init)
            return parts.count == 2 ? (parts[0], parts[1]) : nil
        }, uniquingKeysWith: { _, last in last })
        guard let expectedKernel = fields["kernel_sha256"],
              let expectedInitramfs = fields["initramfs_sha256"] else {
            return "GUEST_ASSET_MISMATCH invalid manifest: \(manifestURL.path)"
        }
        func sha256(_ url: URL) -> String? {
            guard let data = try? Data(contentsOf: url) else { return nil }
            return SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
        }
        let actualKernel = sha256(kernelURL) ?? "missing"
        let actualInitramfs = sha256(initramfsURL) ?? "missing"
        guard actualKernel == expectedKernel, actualInitramfs == expectedInitramfs else {
            return "GUEST_ASSET_MISMATCH kernel=\(kernelURL.path) expected=\(expectedKernel) actual=\(actualKernel); initramfs=\(initramfsURL.path) expected=\(expectedInitramfs) actual=\(actualInitramfs); rebuild the matching guest pair"
        }
        return nil
    }

    static func memoryValidationError(_ mib: Int) -> String? {
        let minMiB = max(512, Int(VZVirtualMachineConfiguration.minimumAllowedMemorySize / 1024 / 1024))
        let maxMiB = Int(VZVirtualMachineConfiguration.maximumAllowedMemorySize / 1024 / 1024)
        if mib < minMiB || mib > maxMiB { return "memoryMIB must be \(minMiB)...\(maxMiB), got \(mib)" }
        return nil
    }

    var diskLogicalBytes: UInt64 {
        guard let attrs = try? FileManager.default.attributesOfItem(atPath: diskURL.path),
              let size = attrs[.size] as? UInt64 else { return 32 * 1024 * 1024 * 1024 } // default 32GiB (sparse)
        return size
    }

    // Stage 3C: disk size parsing and defaults (sparse)
    static let defaultProvisionBytes: UInt64 = 32 * 1024 * 1024 * 1024 // 32 GiB logical sparse default (was 8G)
    static let minProvisionBytes: UInt64 = 2 * 1024 * 1024 * 1024 // must fit template contents (~500M) but enforce 8G for new
    static func parseDiskSize(_ s: String) -> UInt64? {
        let t = s.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        if t.isEmpty { return nil }
        var numPart = ""
        var unitPart = ""
        for ch in t {
            if ch.isNumber || ch == "." { if !unitPart.isEmpty { return nil }; numPart.append(ch) } else if ch.isLetter { unitPart.append(ch) } else if ch.isWhitespace { continue } else { return nil }
        }
        // ponytail: integer only for disk sizes (no 1.5G ambiguity); reject decimal
        if numPart.contains(".") { return nil }
        guard let num = UInt64(numPart), num > 0 else { return nil }
        let unit = unitPart.trimmingCharacters(in: .whitespaces)
        let mult: UInt64
        switch unit {
        case "", "b": mult = 1
        case "m", "mb", "mib": mult = 1024 * 1024
        case "g", "gb", "gib": mult = 1024 * 1024 * 1024
        case "k", "kb", "kib": mult = 1024
        case "t", "tb", "tib": mult = 1024 * 1024 * 1024 * 1024
        default: return nil
        }
        let (res, overflow) = num.multipliedReportingOverflow(by: mult)
        if overflow { return nil }
        if res == 0 { return nil }
        // reject > 2TiB artificial max? allow up to 1024G, but check overflow already
        if res > 5 * 1024 * 1024 * 1024 * 1024 { return nil }
        return res
    }

    static func formatBytes(_ b: UInt64) -> String {
        if b % (1024*1024*1024) == 0 { return "\(b/(1024*1024*1024)) GiB" }
        if b % (1024*1024) == 0 { return "\(b/(1024*1024)) MiB" }
        return "\(b) bytes"
    }

    static func existingUserDiskPath() -> String? {
        let isTest = ProcessInfo.processInfo.environment["HARPOON_ALLOW_TMP_FALLBACK"] == "1" || ProcessInfo.processInfo.environment["HARPOON_TEST_MODE"] == "1" || ProcessInfo.processInfo.environment["HARPOON_TEST_TMPDIR"] != nil
        let userPaths = HarpoonRoots.userDiskCandidates(testFlags: isTest)
        for p in userPaths { if FileManager.default.fileExists(atPath: p) { return p } }
        return nil
    }

    static func desiredProvisionBytes() -> UInt64 {
        // unlike cpus/memory, the config file wins over the environment here: HARPOON_DISK_SIZE applies only without a valid config disk-size
        let cfgCandidates = HarpoonRoots.configCandidates()
        for cand in cfgCandidates {
            if let data = try? Data(contentsOf: URL(fileURLWithPath: cand)),
               let obj = try? JSONDecoder().decode(HarpoonUserConfig.self, from: data),
               let ds = obj.diskSize, let parsed = parseDiskSize(ds) {
                if parsed >= defaultProvisionBytes { return parsed }
                if parsed >= minProvisionBytes { return parsed }
                // if parsed < min, fall back to default (avoid tiny)
            }
        }
        if let raw = ProcessInfo.processInfo.environment["HARPOON_DISK_SIZE"], let p = parseDiskSize(raw) { return p }
        return defaultProvisionBytes
    }

    // host helpers for disk ops
    static func backingFileInfo(at path: String) -> (logical: UInt64, physical: UInt64) {
        var logical: UInt64 = 0
        var physical: UInt64 = 0
        if let attrs = try? FileManager.default.attributesOfItem(atPath: path), let sz = attrs[.size] as? UInt64 { logical = sz }
        // physical via stat blocks
        var st = stat()
        if stat(path, &st) == 0 {
            // st_blocks is 512-byte blocks allocated
            physical = UInt64(st.st_blocks) * 512
        }
        return (logical, physical)
    }
}
