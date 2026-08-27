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

    var kernelURL: URL = RuntimeConfig.resolveResource(named: "Image-virt", fallback: "spike1/cache/Image-virt")
    var initramfsURL: URL = RuntimeConfig.resolveResource(named: "harpoon-initramfs.cpio.gz", fallback: "harpoon/cache/harpoon-m4-initramfs.cpio.gz")
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
    var serialLogPath: String = "/tmp/harpoon-serial.log"

    var vsockPort: UInt32 = 2375
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
        // User-writable disk takes precedence if exists
        let userPaths = [
            FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Harpoon/data/harpoon-root.img").path,
            "/tmp/harpoon-runtime/data/harpoon-root.img"
        ]
        for p in userPaths {
            if FileManager.default.fileExists(atPath: p) { return URL(fileURLWithPath: p) }
        }
        // Otherwise use template from installed lib or fallback
        if let lib = installedLibDir() {
            let tmpl = lib.appendingPathComponent("harpoon-root.img")
            if FileManager.default.fileExists(atPath: tmpl.path) {
                // provision on first run: copy to user location
                let userDest = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Harpoon/data/harpoon-root.img")
                // try primary, fallback to /tmp if not writable
                let dest: URL
                do {
                    try FileManager.default.createDirectory(at: userDest.deletingLastPathComponent(), withIntermediateDirectories: true, attributes: nil)
                    dest = userDest
                } catch {
                    let fallback = URL(fileURLWithPath: "/tmp/harpoon-runtime/data/harpoon-root.img")
                    try? FileManager.default.createDirectory(at: fallback.deletingLastPathComponent(), withIntermediateDirectories: true, attributes: nil)
                    dest = fallback
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
                }
                return dest
            }
        }
        // development fallback
        return URL(fileURLWithPath: "spike2/cache/harpoon-root.img")
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
              let size = attrs[.size] as? UInt64 else { return 2 * 1024 * 1024 * 1024 } // default 2GiB
        return size
    }
}
