import Foundation

struct SharedRoot {
    let hostPath: String
    let guestPath: String
    let tag: String
}

// ponytail: minimal production config — no public config system yet (M8), defaults reproduce Spike 5
struct RuntimeConfig {
    var cpuCount: Int = 2
    var memoryMIB: Int = 1024 // allowed 512/768/1024, default 1024 safe
    var memorySizeBytes: UInt64 { UInt64(memoryMIB) * 1024 * 1024 }

    var kernelURL: URL = RuntimeConfig.resolveResource(named: "Image-virt", fallback: "assets/guest/Image-virt")
    var initramfsURL: URL = RuntimeConfig.resolveResource(named: "harpoon-initramfs.cpio.gz", fallback: "assets/guest/harpoon-initramfs.cpio.gz")
    var diskURL: URL = RuntimeConfig.resolveRootDisk()

    var shareHostPath: String = "/tmp/harpoon-share"
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

    var dockerSocketPath: String = "/tmp/harpoon-docker.sock"
    var balloonControlPath: String = "/tmp/harpoon-control"
    var mgmtSocketPath: String = "/tmp/harpoon-mgmt.sock"
    var serialLogPath: String = "/tmp/harpoon-serial.log"

    var vsockPort: UInt32 = 2375
    var mgmtVsockPort: UInt32 = 2377 // Stage 3A management channel — vsock-only, no TCP, no SSH
    var hostForwardPort: UInt16 = 8080
    var guestForwardPort: UInt16 = 8080

    var bootTimeout: TimeInterval = 120
    var dockerReadyTimeout: TimeInterval = 120

    static func installedLibDir() -> URL? {
        // Harpoon.app bundle: Resources/harpoon/lib/harpoon (when harpoon is at Resources/harpoon/bin/harpoon)
        // Tauri flat resources: Resources/harpoon or Resources/
        if let exec = Bundle.main.executableURL {
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
                if FileManager.default.fileExists(atPath: c.path) { return c }
                // also check if parent contains Image-virt directly (flat)
                let flat = c.deletingLastPathComponent()
                if FileManager.default.fileExists(atPath: flat.appendingPathComponent("Image-virt").path) { return flat }
            }
            // Check Bundle resourceURL directly
            if let res = Bundle.main.resourceURL {
                let resHarpoonLib = res.appendingPathComponent("harpoon/lib/harpoon")
                if FileManager.default.fileExists(atPath: resHarpoonLib.path) { return resHarpoonLib }
                let resBundleLib = res.appendingPathComponent("bundle-resources/harpoon/lib/harpoon")
                if FileManager.default.fileExists(atPath: resBundleLib.path) { return resBundleLib }
                let resHarpoon = res.appendingPathComponent("harpoon")
                if FileManager.default.fileExists(atPath: resHarpoon.appendingPathComponent("Image-virt").path) { return resHarpoon }
                let resBundle = res.appendingPathComponent("bundle-resources/harpoon")
                if FileManager.default.fileExists(atPath: resBundle.appendingPathComponent("Image-virt").path) { return resBundle }
                if FileManager.default.fileExists(atPath: res.appendingPathComponent("Image-virt").path) { return res }
            }
        }
        // Try common prefixes relative to executable (installed via package.sh)
        let candidates: [URL] = [
            URL(fileURLWithPath: "/usr/local/lib/harpoon"),
            URL(fileURLWithPath: "/opt/homebrew/lib/harpoon"),
            URL(fileURLWithPath: "/opt/homebrew/libexec/harpoon"),
        ]
        for c in candidates {
            if FileManager.default.fileExists(atPath: c.path) { return c }
        }
        // Relative to executable: <prefix>/bin/harpoon -> <prefix>/lib/harpoon
        if let exec = Bundle.main.executableURL {
            let binDir = exec.deletingLastPathComponent()
            let lib1 = binDir.deletingLastPathComponent().appendingPathComponent("lib/harpoon")
            if FileManager.default.fileExists(atPath: lib1.path) { return lib1 }
            let lib2 = binDir.appendingPathComponent("../lib/harpoon").standardized
            if FileManager.default.fileExists(atPath: lib2.path) { return lib2 }
        }
        return nil
    }

    static func resolveResource(named: String, fallback: String) -> URL {
        // 1. env override already handled in fromEnvironment, but check installed
        if let lib = installedLibDir() {
            let cand = lib.appendingPathComponent(named)
            if FileManager.default.fileExists(atPath: cand.path) { return cand }
        }
        // 2. development fallback relative to cwd
        let cwdCand = URL(fileURLWithPath: FileManager.default.currentDirectoryPath).appendingPathComponent(fallback)
        if FileManager.default.fileExists(atPath: cwdCand.path) { return cwdCand }
        // 3. relative to executable's directory (when running from build)
        if let exec = Bundle.main.executableURL {
            let execDir = exec.deletingLastPathComponent()
            // try harpoon/../../fallback
            let cand2 = execDir.appendingPathComponent("../../").appendingPathComponent(fallback).standardized
            if FileManager.default.fileExists(atPath: cand2.path) { return cand2 }
        }
        // 4. fallback as given (will fail validation later with clear message)
        if let lib = installedLibDir() {
            return lib.appendingPathComponent(named)
        }
        return URL(fileURLWithPath: fallback)
    }

    static func resolveRootDisk() -> URL {
        // User-writable disk takes precedence if exists — NEVER silently delete/replace
        // because size differs from immutable template; template is read-only, user disk is mutable.
        // ponytail: no template-size-equality invariant for already-provisioned disk (resize deferred)
        // Production: only ~/Library/...; /tmp fallback only when explicitly enabled for tests (HARPOON_ALLOW_TMP_FALLBACK=1)
        let isTest = ProcessInfo.processInfo.environment["HARPOON_ALLOW_TMP_FALLBACK"] == "1" || ProcessInfo.processInfo.environment["HARPOON_TEST_MODE"] == "1" || ProcessInfo.processInfo.environment["HARPOON_TEST_TMPDIR"] != nil
        var userPaths = [FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Harpoon/data/harpoon-root.img").path]
        if isTest {
            userPaths.append("/tmp/harpoon-runtime/data/harpoon-root.img")
            if let custom = ProcessInfo.processInfo.environment["HARPOON_TEST_TMPDIR"] { userPaths.append((custom as NSString).appendingPathComponent("data/harpoon-root.img")) }
        }
        for p in userPaths {
            if FileManager.default.fileExists(atPath: p) { return URL(fileURLWithPath: p) }
        }
        // Otherwise use template from installed lib or fallback
        if let lib = installedLibDir() {
            let tmpl = lib.appendingPathComponent("harpoon-root.img")
            if FileManager.default.fileExists(atPath: tmpl.path) {
                // provision on first run: copy to user location — production MUST be ~/Library, fail if not writable
                let userDest = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Harpoon/data/harpoon-root.img")
                let dest: URL
                do {
                    try FileManager.default.createDirectory(at: userDest.deletingLastPathComponent(), withIntermediateDirectories: true, attributes: nil)
                    dest = userDest
                } catch {
                    if isTest {
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
        }
        // development fallback — canonical assets/guest (no spike fallback)
        return URL(fileURLWithPath: "assets/guest/harpoon-root.img")
    }

    static func fromEnvironment() -> RuntimeConfig {
        var c = RuntimeConfig()
        // user config (file) overrides defaults, env overrides config, CLI overrides all
        // load user config if present
        let configCandidates = [
            FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Harpoon/config.json").path,
            "/tmp/harpoon-runtime/config.json"
        ]
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
        if let p = ProcessInfo.processInfo.environment["HARPOON_DOCKER_SOCK"] { c.dockerSocketPath = p }
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
        let allowed: Set<Int> = [512, 768, 1024]
        if !allowed.contains(memoryMIB) { return "memoryMIB must be 512/768/1024, got \(memoryMIB)" }
        if !FileManager.default.fileExists(atPath: kernelURL.path) { return "kernel not found: \(kernelURL.path)" }
        if !FileManager.default.fileExists(atPath: initramfsURL.path) { return "initramfs not found: \(initramfsURL.path)" }
        // disk is optional for validation — ramdisk fallback exists but production expects block
        return nil
    }

    var diskLogicalBytes: UInt64 {
        guard let attrs = try? FileManager.default.attributesOfItem(atPath: diskURL.path),
              let size = attrs[.size] as? UInt64 else { return 8 * 1024 * 1024 * 1024 } // default 8GiB (sparse)
        return size
    }

    // Stage 3C: disk size parsing and defaults (sparse)
    static let defaultProvisionBytes: UInt64 = 8 * 1024 * 1024 * 1024 // 8 GiB logical minimum, sparse
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
        var userPaths = [FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Harpoon/data/harpoon-root.img").path]
        if isTest {
            userPaths.append("/tmp/harpoon-runtime/data/harpoon-root.img")
            if let custom = ProcessInfo.processInfo.environment["HARPOON_TEST_TMPDIR"] { userPaths.append((custom as NSString).appendingPathComponent("data/harpoon-root.img")) }
        }
        for p in userPaths { if FileManager.default.fileExists(atPath: p) { return p } }
        return nil
    }

    static func desiredProvisionBytes() -> UInt64 {
        // config file overrides default, env overrides config
        let cfgCandidates = [
            FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Harpoon/config.json").path,
            "/tmp/harpoon-runtime/config.json"
        ]
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
