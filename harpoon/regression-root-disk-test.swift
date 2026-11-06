import Foundation

// Unit test for the root-disk boundary in RuntimeConfig: resolving a path never creates a disk, provisioning happens only through
// provisionedRootDisk(for:), only for the default destination, and never touches a disk that exists. Runs in a private
// HARPOON_TEST_TMPDIR namespace with a fake template (HARPOON_TEST_ROOT); no VM, no engine. `log` is main.swift's, stubbed here.
func log(_ m: String) {}
func require(_ condition: @autoclosure () -> Bool, _ message: String) {
    guard condition() else { fputs("FAIL: \(message)\n", stderr); exit(1) }
}
let fm = FileManager.default
let root = ProcessInfo.processInfo.environment["HARPOON_TEST_TMPDIR"]!
let dest = root + "/data/harpoon-root.img"
func size(_ p: String) -> UInt64 { ((try? fm.attributesOfItem(atPath: p))?[.size] as? UInt64) ?? 0 }
func bytes(_ p: String) -> Data? { try? Data(contentsOf: URL(fileURLWithPath: p)) }

// missing disk: a path, no file
require(RuntimeConfig.resolveRootDiskPath().path == dest, "the missing disk resolves to the default destination")
require(RuntimeConfig().diskURL.path == dest, "constructing a RuntimeConfig resolves the path")
require(RuntimeConfig.fromEnvironment().diskURL.path == dest, "fromEnvironment resolves the path")
require(!fm.fileExists(atPath: dest) && !fm.fileExists(atPath: root + "/data"), "resolving created nothing")
require(RuntimeConfig.wouldProvisionRootDisk(URL(fileURLWithPath: dest)), "the default destination would be provisioned")

// an explicit path is never the default: nothing is created, for it or for the default
let other = URL(fileURLWithPath: root + "/elsewhere.img")
require(!RuntimeConfig.wouldProvisionRootDisk(other), "an explicit path is not provisioned")
require(RuntimeConfig.provisionedRootDisk(for: other) == other, "an explicit path comes back unchanged")
require(!fm.fileExists(atPath: other.path) && !fm.fileExists(atPath: dest), "nothing was created for an explicit path")

// first run: the default disk is provisioned from the template, at the default size
let made = RuntimeConfig.provisionedRootDisk(for: URL(fileURLWithPath: dest))
require(made.path == dest && fm.fileExists(atPath: dest), "first run provisions the default disk")
require(size(dest) == RuntimeConfig.defaultProvisionBytes, "the provisioned disk has the default logical size (got \(size(dest)))")

// an existing disk is preserved: contents, size and path, however often it is asked for
let marker = Data("keep-me".utf8)
let fh = FileHandle(forWritingAtPath: dest)!
try! fh.seek(toOffset: 4096); try! fh.write(contentsOf: marker); try! fh.close()
let before = bytes(dest)!
for _ in 0..<2 {
    require(!RuntimeConfig.wouldProvisionRootDisk(URL(fileURLWithPath: dest)), "an existing disk is not provisioned again")
    require(RuntimeConfig.provisionedRootDisk(for: URL(fileURLWithPath: dest)).path == dest, "an existing disk comes back unchanged")
    require(RuntimeConfig.resolveRootDiskPath().path == dest, "an existing disk is what resolves")
}
require(bytes(dest)! == before && size(dest) == RuntimeConfig.defaultProvisionBytes, "an existing disk was not rewritten, truncated or resized")

// a smaller existing disk is not grown either
try! fm.removeItem(atPath: dest)
fm.createFile(atPath: dest, contents: Data(count: 5 * 1024 * 1024))
_ = RuntimeConfig.provisionedRootDisk(for: URL(fileURLWithPath: dest))
require(size(dest) == 5 * 1024 * 1024, "a smaller existing disk keeps its size")

// no template: nothing to provision from, the missing path comes back unchanged
try! fm.removeItem(atPath: dest)
setenv("HARPOON_TEST_ROOT", root + "/no-such-template", 1)
require(!RuntimeConfig.wouldProvisionRootDisk(URL(fileURLWithPath: dest)), "without a template there is nothing to provision")
require(RuntimeConfig.provisionedRootDisk(for: URL(fileURLWithPath: dest)).path == dest && !fm.fileExists(atPath: dest), "no template: nothing is created")
print("root disk unit test PASS")
