import Foundation

// Unit test for the size of a NEW root disk (#184): HARPOON_DISK_SIZE > config disk-size > 32 GiB, and a bad value falls through.
// Runs in private HARPOON_TEST_TMPDIR namespaces with a fake template (HARPOON_TEST_ROOT); no VM, no engine. `log` is main.swift's.
func log(_ m: String) {}
func require(_ condition: @autoclosure () -> Bool, _ message: String) {
    guard condition() else { fputs("FAIL: \(message)\n", stderr); exit(1) }
}
let fm = FileManager.default
let GiB: UInt64 = 1024 * 1024 * 1024
let base = ProcessInfo.processInfo.environment["HARPOON_TEST_TMPDIR"]!
var n = 0
func size(_ p: String) -> UInt64 { ((try? fm.attributesOfItem(atPath: p))?[.size] as? UInt64) ?? 0 }

/// A fresh namespace with an optional config.json and HARPOON_DISK_SIZE; returns the size a first provision gives.
func provisioned(config: String?, env: String?) -> UInt64 {
    n += 1
    let root = base + "/case\(n)"
    try! fm.createDirectory(atPath: root, withIntermediateDirectories: true)
    setenv("HARPOON_TEST_TMPDIR", root, 1)
    if let config { try! config.write(toFile: root + "/config.json", atomically: true, encoding: .utf8) }
    if let env { setenv("HARPOON_DISK_SIZE", env, 1) } else { unsetenv("HARPOON_DISK_SIZE") }
    let desired = RuntimeConfig.desiredProvisionBytes()
    let dest = URL(fileURLWithPath: root + "/data/harpoon-root.img")
    require(RuntimeConfig.provisionedRootDisk(for: dest).path == dest.path, "case \(n): the default disk is provisioned")
    require(size(dest.path) == desired, "case \(n): the disk (\(size(dest.path))) has the desired size (\(desired))")
    return desired
}

// the layers
require(provisioned(config: nil, env: nil) == 32 * GiB, "default only: 32 GiB")
require(provisioned(config: #"{"diskSize":"16G"}"#, env: nil) == 16 * GiB, "config only: 16 GiB")
require(provisioned(config: nil, env: "8G") == 8 * GiB, "environment only: 8 GiB")
require(provisioned(config: #"{"diskSize":"16G"}"#, env: "8G") == 8 * GiB, "the environment beats the config file")
require(provisioned(config: #"{"diskSize":"16G"}"#, env: "64G") == 64 * GiB, "the environment beats the config file (larger)")
require(provisioned(config: #"{"cpus":2}"#, env: nil) == 32 * GiB, "a config file without disk-size leaves the default")

// invalid and below-minimum values are skipped, layer by layer
require(provisioned(config: #"{"diskSize":"16G"}"#, env: "banana") == 16 * GiB, "an unparseable environment value falls through to the config file")
require(provisioned(config: #"{"diskSize":"16G"}"#, env: "1G") == 16 * GiB, "an environment value below 2G falls through to the config file")
require(provisioned(config: #"{"diskSize":"16G"}"#, env: "") == 16 * GiB, "an empty environment value falls through to the config file")
require(provisioned(config: #"{"diskSize":"banana"}"#, env: nil) == 32 * GiB, "an unparseable config value falls through to the default")
require(provisioned(config: #"{"diskSize":"1G"}"#, env: nil) == 32 * GiB, "a config value below 2G falls through to the default")
require(provisioned(config: #"{"diskSize":"banana"}"#, env: "banana") == 32 * GiB, "two bad values leave the default")
require(provisioned(config: #"{"diskSize":"1G"}"#, env: "8G") == 8 * GiB, "a bad config value does not hide a good environment value")
require(provisioned(config: "{ not json", env: "8G") == 8 * GiB, "a malformed config file does not hide a good environment value")
require(RuntimeConfig.provisionBytes(environment: "2G", config: "16G") == 2 * GiB, "the minimum itself is accepted")
require(RuntimeConfig.defaultProvisionBytes == 32 * GiB, "the default is 32 GiB")

// an existing disk is never resized or rewritten, whatever the layers say
n += 1
let root = base + "/case\(n)"
try! fm.createDirectory(atPath: root + "/data", withIntermediateDirectories: true)
setenv("HARPOON_TEST_TMPDIR", root, 1); setenv("HARPOON_DISK_SIZE", "64G", 1)
try! #"{"diskSize":"16G"}"#.write(toFile: root + "/config.json", atomically: true, encoding: .utf8)
let existing = root + "/data/harpoon-root.img"
fm.createFile(atPath: existing, contents: Data(count: 5 * 1024 * 1024))
let stamp = (try! fm.attributesOfItem(atPath: existing))[.modificationDate] as! Date
require(RuntimeConfig.provisionedRootDisk(for: URL(fileURLWithPath: existing)).path == existing, "an existing disk comes back unchanged")
require(size(existing) == 5 * 1024 * 1024 && ((try! fm.attributesOfItem(atPath: existing))[.modificationDate] as! Date) == stamp, "an existing disk keeps its size and was not touched")
print("disk size unit test PASS")
