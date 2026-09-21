import Foundation
import Darwin

// MARK: - Docker context integration (M8)

let harpoonContextName = "harpoon"
let harpoonSocketEndpoint = "unix:///tmp/harpoon-docker.sock"

func findDocker() -> String? {
    let candidates = ["/usr/local/bin/docker", "/opt/homebrew/bin/docker", "/usr/bin/docker", "/run/current-system/sw/bin/docker"]
    for c in candidates {
        if FileManager.default.isExecutableFile(atPath: c) { return c }
    }
    let proc = Process()
    proc.executableURL = URL(fileURLWithPath: "/usr/bin/which")
    proc.arguments = ["docker"]
    let pipe = Pipe()
    proc.standardOutput = pipe
    proc.standardError = FileHandle.nullDevice
    do {
        try proc.run()
        proc.waitUntilExit()
        if proc.terminationStatus == 0 {
            let data = pipe.fileHandleForReading.readDataToEndOfFile()
            if let out = String(data: data, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines), !out.isEmpty {
                if FileManager.default.isExecutableFile(atPath: out) { return out }
            }
        }
    } catch {}
    // try whereis via env
    if let pathEnv = ProcessInfo.processInfo.environment["PATH"] {
        for dir in pathEnv.split(separator: ":") {
            let cand = URL(fileURLWithPath: String(dir)).appendingPathComponent("docker").path
            if FileManager.default.isExecutableFile(atPath: cand) { return cand }
        }
    }
    return nil
}

func runDocker(_ args: [String], env: [String:String]? = nil) -> (Int32, String, String) {
    guard let docker = findDocker() else { return (127, "", "docker not found") }
    let proc = Process()
    proc.executableURL = URL(fileURLWithPath: docker)
    proc.arguments = args
    if let e = env {
        var merged = ProcessInfo.processInfo.environment
        for (k,v) in e { merged[k]=v }
        proc.environment = merged
    }
    let outPipe = Pipe()
    let errPipe = Pipe()
    proc.standardOutput = outPipe
    proc.standardError = errPipe
    do {
        try proc.run()
        proc.waitUntilExit()
        let out = String(data: outPipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
        let err = String(data: errPipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
        return (proc.terminationStatus, out, err)
    } catch {
        return (1, "", "\(error)")
    }
}

func dockerContextEndpoint(_ name: String) -> String? {
    let (code, out, _) = runDocker(["context", "inspect", name])
    if code != 0 { return nil }
    // parse JSON
    guard let data = out.data(using: .utf8) else { return nil }
    if let arr = try? JSONSerialization.jsonObject(with: data) as? [[String:Any]],
       let first = arr.first,
       let endpoints = first["Endpoints"] as? [String:Any],
       let dockerEp = endpoints["docker"] as? [String:Any],
       let host = dockerEp["Host"] as? String {
        return host
    }
    return nil
}

func dockerContextExists(_ name: String) -> Bool {
    return dockerContextEndpoint(name) != nil
}

func currentDockerContext() -> String? {
    let (code, out, _) = runDocker(["context", "show"])
    if code != 0 { return nil }
    let s = out.trimmingCharacters(in: .whitespacesAndNewlines)
    return s.isEmpty ? nil : s
}

func dockerComposeVersion() -> (installed: Bool, version: String) {
    guard findDocker() != nil else { return (false, "") }
    let (code, out, err) = runDocker(["compose", "version"])
    let combined = (out + err).trimmingCharacters(in: .whitespacesAndNewlines)
    if code == 0 && (combined.contains("v2") || combined.lowercased().contains("compose")) {
        return (true, combined)
    }
    return (false, combined)
}

func dockerConfigPath() -> String {
    if let cfg = ProcessInfo.processInfo.environment["DOCKER_CONFIG"], !cfg.trimmingCharacters(in: .whitespaces).isEmpty {
        return (cfg as NSString).appendingPathComponent("config.json")
    }
    if let home = ProcessInfo.processInfo.environment["HOME"] {
        return (home as NSString).appendingPathComponent(".docker/config.json")
    }
    return FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".docker/config.json").path
}

func isHelperInPath(_ name: String) -> Bool {
    if let pathEnv = ProcessInfo.processInfo.environment["PATH"] {
        for dir in pathEnv.split(separator: ":") {
            let cand = URL(fileURLWithPath: String(dir)).appendingPathComponent(name).path
            if FileManager.default.isExecutableFile(atPath: cand) { return true }
        }
    }
    for cand in ["/opt/homebrew/bin/\(name)", "/usr/local/bin/\(name)", "/usr/bin/\(name)"] {
        if FileManager.default.isExecutableFile(atPath: cand) { return true }
    }
    return false
}

func credsStoreStatus() -> (store: String?, status: String, warning: String?) {
    let path = dockerConfigPath()
    guard FileManager.default.fileExists(atPath: path),
          let data = try? Data(contentsOf: URL(fileURLWithPath: path)),
          let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
        return (nil, "PASS", nil)
    }
    guard let store = obj["credsStore"] as? String, !store.isEmpty else { return (nil, "PASS", nil) }
    if store == "desktop" {
        let found = isHelperInPath("docker-credential-desktop")
        if !found {
            return (store, "WARN", "config references docker-credential-desktop but the helper is not installed. Docker Desktop credsStore will fail — remove or change credsStore in ~/.docker/config.json, or install the helper. Harpoon does not require Docker Desktop.")
        }
    }
    return (store, "PASS", nil)
}

func handleDockerSetup() -> Int32 {
    guard let docker = findDocker() else {
        cliError("Docker CLI: FAIL — Docker CLI not installed/found (checked PATH, /opt/homebrew/bin/docker, /usr/local/bin/docker, /usr/bin/docker)")
        cliError("Install Docker CLI (https://docs.docker.com/engine/install/) — Docker Desktop NOT required")
        return 1
    }
    cliPrint("Docker CLI: PASS \(docker)")
    let exists = dockerContextExists(harpoonContextName)
    if exists {
        if let ep = dockerContextEndpoint(harpoonContextName) {
            if ep == harpoonSocketEndpoint {
                cliPrint("Harpoon context already exists and is valid")
                cliPrint("Endpoint: \(ep)")
                if let cur = currentDockerContext() { cliPrint("Current context: \(cur) (unchanged)") }
                cliPrint("Docker socket: \(HarpoonPaths.dockerSocketPath)")
                return 0
            } else {
                cliPrint("Harpoon context exists but endpoint wrong — repairing")
                cliPrint("  existing: \(ep)")
                cliPrint("  expected: \(harpoonSocketEndpoint)")
                // Try update first (preserves context), then fallback rm+create
                let (updCode, _, updErr) = runDocker(["context", "update", harpoonContextName, "--docker", "host=\(harpoonSocketEndpoint)"])
                if updCode == 0, let newEp = dockerContextEndpoint(harpoonContextName), newEp == harpoonSocketEndpoint {
                    cliPrint("Repaired Harpoon context \(ep) -> \(newEp)")
                    return 0
                }
                if updCode != 0 && !updErr.isEmpty { cliPrint("update failed: \(updErr.trimmingCharacters(in: .whitespacesAndNewlines)) — trying remove+create") }
                let (rmCode, _, _) = runDocker(["context", "rm", "-f", harpoonContextName])
                if rmCode != 0 {
                    cliError("Failed to remove wrong context for repair")
                    return 1
                }
                // fall through to create
            }
        }
    }
    // create
    cliPrint("Creating Docker context 'harpoon' -> \(harpoonSocketEndpoint)")
    let (code, out, err) = runDocker(["context", "create", harpoonContextName, "--docker", "host=\(harpoonSocketEndpoint)", "--description", "Harpoon"])
    if code != 0 {
        // maybe already exists race, try inspect again
        if dockerContextExists(harpoonContextName), let ep = dockerContextEndpoint(harpoonContextName), ep == harpoonSocketEndpoint {
            cliPrint("Context now exists (race)")
            return 0
        }
        cliError("docker context create failed (\(code))")
        if !out.isEmpty { cliError(out) }
        if !err.isEmpty { cliError(err) }
        return 1
    }
    cliPrint(out.trimmingCharacters(in: .whitespacesAndNewlines))
    if let ep = dockerContextEndpoint(harpoonContextName) {
        cliPrint("Endpoint: \(ep)")
    }
    cliPrint("Done. Try: docker --context harpoon version")
    return 0
}

func handleDockerStatus() -> Int32 {
    if let docker = findDocker() {
        cliPrint("Docker CLI: \(docker)")
        let (vCode, vOut, _) = runDocker(["--version"])
        if vCode==0 { cliPrint(vOut.trimmingCharacters(in: .whitespacesAndNewlines)) }
    } else {
        cliPrint("Docker CLI: not found")
    }
    if dockerContextExists(harpoonContextName) {
        if let ep = dockerContextEndpoint(harpoonContextName) {
            if ep == harpoonSocketEndpoint {
                cliPrint("Harpoon context: installed")
                cliPrint("Endpoint: \(ep)")
            } else {
                cliPrint("Harpoon context: conflict")
                cliPrint("Endpoint: \(ep) (expected \(harpoonSocketEndpoint))")
            }
        }
    } else {
        cliPrint("Harpoon context: not installed (run harpoon docker setup)")
    }
    if let cur = currentDockerContext() { cliPrint("Current context: \(cur)") } else { cliPrint("Current context: unknown") }
    let snap = statusSnapshot()
    cliPrint("Harpoon runtime: \(snap.state)")
    if let pid = snap.pid { cliPrint("PID: \(pid)") }
    cliPrint("Socket: \(HarpoonPaths.dockerSocketPath) exists=\(snap.sockExists) ready=\(snap.dockerReady)")
    var st = stat()
    let perms = stat(HarpoonPaths.dockerSocketPath, &st)==0 ? String(format:"%o", st.st_mode & 0o777) : "missing"
    cliPrint("Socket perms: \(perms) (expected 600)")
    return 0
}

func handleDockerRemove() -> Int32 {
    guard findDocker() != nil else { cliError("docker CLI not found"); return 1 }
    guard dockerContextExists(harpoonContextName) else { cliPrint("Harpoon context not installed"); return 0 }
    guard let ep = dockerContextEndpoint(harpoonContextName) else { cliError("cannot inspect harpoon context"); return 1 }
    if ep != harpoonSocketEndpoint {
        cliError("Refusing to remove conflicting context 'harpoon' (endpoint \(ep) != \(harpoonSocketEndpoint))")
        cliError("Not a Harpoon-owned context. Remove manually if intended: docker context rm harpoon")
        return 1
    }
    // if current context is harpoon, switch to default first
    if let cur = currentDockerContext(), cur == harpoonContextName {
        let (code, _, err) = runDocker(["context", "use", "default"])
        if code != 0 {
            // try desktop-linux or just force
            _ = runDocker(["context", "use", "desktop-linux"])
        }
    }
    let (code, out, err) = runDocker(["context", "rm", harpoonContextName])
    if code != 0 {
        // try force
        let (code2, out2, err2) = runDocker(["context", "rm", "-f", harpoonContextName])
        if code2 != 0 {
            cliError("docker context rm failed")
            if !err.isEmpty { cliError(err) }
            if !err2.isEmpty { cliError(err2) }
            return 1
        }
        cliPrint(out2)
    } else {
        cliPrint(out)
    }
    cliPrint("Harpoon context removed")
    return 0
}

func handleDockerUse() -> Int32 {
    guard findDocker() != nil else { cliError("docker CLI not found"); return 1 }
    guard dockerContextExists(harpoonContextName) else {
        cliError("Harpoon context not installed (run harpoon docker setup)")
        return 1
    }
    if let ep = dockerContextEndpoint(harpoonContextName), ep != harpoonSocketEndpoint {
        cliError("Conflict: harpoon context exists but endpoint \(ep) != \(harpoonSocketEndpoint)")
        return 1
    }
    let (code, out, err) = runDocker(["context", "use", harpoonContextName])
    if code != 0 {
        cliError("docker context use failed")
        if !err.isEmpty { cliError(err) }
        return 1
    }
    cliPrint(out.trimmingCharacters(in: .whitespacesAndNewlines))
    cliPrint("Now using context: harpoon")
    return 0
}

func handleDocker(args: [String]) -> Int32 {
    if args.isEmpty || args[0]=="help" || args[0]=="--help" || args[0]=="-h" {
        cliPrint("usage: harpoon docker <setup|status|remove|use|env> [options]")
        cliPrint("  setup   create/verify harpoon context (unix:///tmp/harpoon-docker.sock)")
        cliPrint("  status  show Docker CLI, context, runtime, socket")
        cliPrint("  remove  remove harpoon context if owned")
        cliPrint("  use     activate harpoon context (docker context use harpoon)")
        cliPrint("  env     print DOCKER_HOST for Harpoon")
        return 0
    }
    switch args[0] {
    case "setup": return handleDockerSetup()
    case "status": return handleDockerStatus()
    case "remove": return handleDockerRemove()
    case "use": return handleDockerUse()
    case "env": return handleDockerEnv()
    default:
        cliError("unknown docker subcommand: \(args[0])")
        cliPrint("usage: harpoon docker <setup|status|remove|use|env>")
        return 2
    }
}


