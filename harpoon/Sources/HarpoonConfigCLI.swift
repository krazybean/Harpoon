import Foundation
import Virtualization
import Darwin

func handleConfig(args: [String]) -> Int32 {
    if args.isEmpty || args[0]=="show" {
        let path = configFilePath()
        let (cfg, err) = loadUserConfig()
        if let e = err {
            cliError("Harpoon configuration is invalid:")
            cliError(e)
            cliError("Config: \(path)")
            cliError("Hint: run harpoon config reset cpus/memory or fix JSON")
            return 1
        }
        cliPrint("Config: \(path)")
        if let c = cfg {
            if let v = c.cpus { cliPrint("cpus: \(v)") } else { cliPrint("cpus: (default 2)") }
            if let v = c.memory { cliPrint("memory: \(v)") } else { cliPrint("memory: (default 4096)") }
            if let v = c.diskSize { cliPrint("disk-size: \(v)") } else { cliPrint("disk-size: (default 32G)") }
            if c.cpus==nil && c.memory==nil && c.diskSize==nil { cliPrint("(no user config, using defaults)") }
        } else {
            cliPrint("(no user config, using defaults)")
            cliPrint("cpus: (default 2)")
            cliPrint("memory: (default 4096)")
            cliPrint("disk-size: (default 32G)")
        }
        return 0
    } else if args[0]=="get" && args.count>=2 {
        let key = args[1]
        let (cfg, err) = loadUserConfig()
        if let e = err { cliError("config invalid: \(e)"); return 1 }
        if key=="cpus" || key=="cpu" {
            if let v = cfg?.cpus { cliPrint("\(v)") } else { cliPrint("2") }
            return 0
        } else if key=="memory" {
            if let v = cfg?.memory { cliPrint("\(v)") } else { cliPrint("4096") }
            return 0
        } else if key=="disk-size" || key=="diskSize" || key=="disk_size" {
            if let v = cfg?.diskSize { cliPrint(v) } else { cliPrint("32G") }
            return 0
        } else {
            cliError("unknown config key: \(key)"); return 1
        }
    } else if args[0]=="path" {
        cliPrint(configFilePath())
        return 0
    } else if args[0]=="set" && args.count>=3 {
        let key = args[1]
        let valStr = args[2]
        var (cfg, err) = loadUserConfig()
        if let e = err {
            cliError("Harpoon configuration is invalid: \(e)")
            cliError("Config: \(configFilePath())")
            return 1
        }
        var cur = cfg ?? HarpoonUserConfig()
        if key=="cpus" || key=="cpu" {
            guard let v = Int(valStr) else { cliError("invalid cpus: \(valStr)"); return 1 }
            if v<1 || v>8 { cliError("cpus must be 1...8, got \(v)"); return 1 }
            cur.cpus = v
        } else if key=="memory" {
            guard let v = Int(valStr) else { cliError("invalid memory: \(valStr)"); return 1 }
            if let error = RuntimeConfig.memoryValidationError(v) { cliError(error); return 1 }
            cur.memory = v
        } else if key=="disk-size" || key=="diskSize" || key=="disk_size" {
            guard let bytes = RuntimeConfig.parseDiskSize(valStr) else { cliError("invalid disk-size: \(valStr) — use e.g. 8G, 16G, 1024M (G/GiB/M/MiB)"); return 1 }
            if bytes < RuntimeConfig.minProvisionBytes { cliError("disk-size must be at least 2G, got \(valStr) (\(bytes) bytes)"); return 1 }
            cur.diskSize = valStr
        } else {
            cliError("unknown config key: \(key) (expected cpus, memory, disk-size)")
            return 1
        }
        if let e = saveUserConfig(cur) { cliError("failed to save: \(e)"); return 1 }
        cliPrint("set \(key)=\(valStr)")
        return 0
    } else if args[0]=="reset" && args.count>=2 {
        let key = args[1]
        var (cfg, err) = loadUserConfig()
        if let e = err {
            // if malformed, reset by removing file
            try? FileManager.default.removeItem(atPath: configFilePath())
            cliPrint("removed malformed config")
            return 0
        }
        var cur = cfg ?? HarpoonUserConfig()
        if key=="cpus" || key=="cpu" { cur.cpus = nil }
        else if key=="memory" { cur.memory = nil }
        else if key=="disk-size" || key=="diskSize" || key=="disk_size" { cur.diskSize = nil }
        else if key=="all" { cur = HarpoonUserConfig() }
        else { cliError("unknown key: \(key)"); return 1 }
        // if all nil, remove file
        if cur.cpus==nil && cur.memory==nil && cur.diskSize==nil {
            try? FileManager.default.removeItem(atPath: configFilePath())
            cliPrint("reset \(key) (now default)")
        } else {
            if let e = saveUserConfig(cur) { cliError("failed: \(e)"); return 1 }
            cliPrint("reset \(key)")
        }
        return 0
    } else if args[0]=="--help" || args[0]=="-h" || args[0]=="help" {
        cliPrint("usage: harpoon config <show|set|get|reset|path> [args]")
        cliPrint("  show              Show current config and path")
        cliPrint("  set cpus 2        Set cpus 1...8")
        cliPrint("  set memory 4096   Set memory in MiB (minimum 512)")
        cliPrint("  set disk-size 16G Set disk size (e.g. 8G, 16G, 32G) — first provision or resize via harpoon disk resize")
        cliPrint("  get disk-size     Get disk-size")
        cliPrint("  reset cpus        Reset to default")
        cliPrint("  reset memory      Reset to default")
        cliPrint("  path              Print config file path")
        return 0
    } else {
        cliError("unknown config command: \(args.joined(separator: " "))")
        cliPrint("usage: harpoon config <show|set|reset|path>")
        return 1
    }
}
