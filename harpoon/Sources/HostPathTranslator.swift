import Foundation

// ponytail: minimal host->guest path translation for M4 — only rewrites absolute macOS bind sources under shared roots
final class HostPathTranslator {
    let roots: [SharedRoot]
    let log: (String)->Void

    init(roots: [SharedRoot], log: @escaping (String)->Void) {
        self.roots = roots
        self.log = log
    }

    // canonicalize /tmp vs /private/tmp: both map to guest /mnt/harpoon-host/tmp
    func translateHostPath(_ host: String) -> String? {
        // must be absolute
        guard host.hasPrefix("/") else { return nil }
        // normalize: remove trailing slash, resolve . and .. via URL standardized
        let url = URL(fileURLWithPath: host)
        let std = url.standardized.path // resolves /tmp symlink? On macOS, URL standardizing does NOT resolve symlink, but we handle both
        // also handle /private/tmp vs /tmp: treat both as /tmp root
        // check longest prefix first
        var best: SharedRoot?
        var bestLen = -1
        for r in roots {
            let hp = r.hostPath
            // match exact or prefix with /
            if std == hp || std.hasPrefix(hp + "/") {
                if hp.count > bestLen {
                    best = r
                    bestLen = hp.count
                }
            }
            // special: /tmp should also match /private/tmp and vice versa if we share /private/tmp
            // if root is /private/tmp, also match /tmp prefix
            if hp == "/private/tmp" && (std == "/tmp" || std.hasPrefix("/tmp/")) {
                // map /tmp/... to same guest as /private/tmp
                if hp.count > bestLen {
                    best = r
                    bestLen = hp.count
                }
            }
            if hp == "/tmp" && (std == "/private/tmp" || std.hasPrefix("/private/tmp/")) {
                if hp.count > bestLen {
                    best = r
                    bestLen = hp.count
                }
            }
        }
        guard let root = best else { return nil }
        // compute remainder
        let remainder: String
        if std == root.hostPath {
            remainder = ""
        } else if std.hasPrefix(root.hostPath + "/") {
            remainder = String(std.dropFirst(root.hostPath.count))
        } else if root.hostPath == "/private/tmp" && std.hasPrefix("/tmp") {
            // /tmp -> /private/tmp mapping
            if std == "/tmp" {
                remainder = ""
            } else {
                remainder = String(std.dropFirst("/tmp".count))
            }
        } else if root.hostPath == "/tmp" && std.hasPrefix("/private/tmp") {
            if std == "/private/tmp" {
                remainder = ""
            } else {
                remainder = String(std.dropFirst("/private/tmp".count))
            }
        } else {
            return nil
        }
        let guest = root.guestPath + remainder
        log("HARPOON_HOST_PATH_TRANSLATE \(host) -> \(guest) via \(root.hostPath)->\(root.guestPath)")
        return guest
    }

    // translate Binds entry "source:target[:ro|rw]" where source is absolute host path
    func translateBindsEntry(_ entry: String) -> String {
        // Binds may be "source:target:mode" where source may contain spaces? Docker handles via JSON string, so entry is single string
        // Split on ":" but need to handle Windows paths not relevant for macOS
        let parts = entry.split(separator: ":", omittingEmptySubsequences: false).map(String.init)
        guard parts.count >= 2 else { return entry }
        let source = parts[0]
        // only translate absolute host paths
        guard let translated = translateHostPath(source) else { return entry }
        var newParts = parts
        newParts[0] = translated
        return newParts.joined(separator: ":")
    }

    // narrow Docker API JSON transformation for POST /containers/create
    func translateCreateBody(_ data: Data) -> Data? {
        guard let json = try? JSONSerialization.jsonObject(with: data, options: []) as? [String: Any] else { return nil }
        var mutable = json
        var changed = false

        // HostConfig.Binds: [String]
        if var hostConfig = mutable["HostConfig"] as? [String: Any], let binds = hostConfig["Binds"] as? [String] {
            var newBinds: [String] = []
            for b in binds {
                let nb = translateBindsEntry(b)
                if nb != b { changed = true }
                newBinds.append(nb)
            }
            if changed {
                hostConfig["Binds"] = newBinds
                mutable["HostConfig"] = hostConfig
            }
        }

        // HostConfig.Mounts: array of dict with Type bind and Source
        if var hostConfig = mutable["HostConfig"] as? [String: Any], let mounts = hostConfig["Mounts"] as? [[String: Any]] {
            var newMounts: [[String: Any]] = []
            var mountChanged = false
            for m in mounts {
                var nm = m
                if let type = m["Type"] as? String, type == "bind", let src = m["Source"] as? String, let trans = translateHostPath(src) {
                    nm["Source"] = trans
                    mountChanged = true
                } else if let src = m["Source"] as? String, let trans = translateHostPath(src) {
                    // be permissive: if Source looks like host absolute and type missing, still translate
                    nm["Source"] = trans
                    mountChanged = true
                }
                newMounts.append(nm)
            }
            if mountChanged {
                hostConfig["Mounts"] = newMounts
                mutable["HostConfig"] = hostConfig
                changed = true
            }
        }

        // Top-level Mounts (long syntax --mount)
        if let mounts = mutable["Mounts"] as? [[String: Any]] {
            var newMounts: [[String: Any]] = []
            var mountChanged = false
            for m in mounts {
                var nm = m
                if let src = m["Source"] as? String, let trans = translateHostPath(src) {
                    // only for bind type, but check Type field if present
                    if let t = m["Type"] as? String, t == "bind" {
                        nm["Source"] = trans
                        mountChanged = true
                    } else if m["Type"] == nil {
                        // Docker may omit Type for bind? be conservative
                        nm["Source"] = trans
                        mountChanged = true
                    }
                }
                newMounts.append(nm)
            }
            if mountChanged {
                mutable["Mounts"] = newMounts
                changed = true
            }
        }

        // Also handle HostConfig.Binds may be at top-level? Docker API has it only inside HostConfig, but handle directly
        if let binds = mutable["Binds"] as? [String] {
            var newBinds: [String] = []
            var bChanged = false
            for b in binds {
                let nb = translateBindsEntry(b)
                if nb != b { bChanged = true }
                newBinds.append(nb)
            }
            if bChanged {
                mutable["Binds"] = newBinds
                changed = true
            }
        }

        guard changed else { return nil }
        guard let out = try? JSONSerialization.data(withJSONObject: mutable, options: []) else { return nil }
        log("HARPOON_HOST_PATH_TRANSLATION_APPLIED original=\(data.count) translated=\(out.count)")
        return out
    }
}
