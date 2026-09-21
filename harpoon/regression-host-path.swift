import Foundation

struct SharedRoot {
    let hostPath: String
    let guestPath: String
    let tag: String
}

func require(_ condition: @autoclosure () -> Bool, _ message: String) {
    guard condition() else {
        fputs("FAIL: \(message)\n", stderr)
        exit(1)
    }
}

let root = URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent("harpoon-host-path-\(UUID().uuidString)")
let directory = root.appendingPathComponent("directory")
let file = root.appendingPathComponent("file.conf")
try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
try Data("file-bind".utf8).write(to: file)
defer { try? FileManager.default.removeItem(at: root) }

var logs: [String] = []
let guestRoot = "/mnt/harpoon-host/test"
let translator = HostPathTranslator(roots: [SharedRoot(hostPath: root.path, guestPath: guestRoot, tag: "test")]) { logs.append($0) }

require(translator.translateHostPath(directory.path) == "\(guestRoot)/directory", "directory translation")
require(translator.translateHostPath(file.path) == "\(guestRoot)/file.conf", "file translation")
require(translator.translateHostPath(directory.appendingPathComponent("../file.conf").path) == "\(guestRoot)/file.conf", "standardized path translation")
require(translator.translateHostPath("relative/path") == nil, "relative path must not translate")
require(translator.translateHostPath("/definitely/outside/harpoon/shared/root") == nil, "unshared absolute path must not translate")
require(FileManager.default.fileExists(atPath: file.path), "fixture file exists")
require((try? file.resourceValues(forKeys: [.isRegularFileKey]).isRegularFile) == true, "fixture is a regular file")
require(logs.contains(where: { $0.contains("type=dir") }), "directory translation is logged")
require(logs.contains(where: { $0.contains("type=file") }), "file translation is logged")

let bind = "\(file.path):/etc/harpoon.conf:ro"
require(translator.translateBindsEntry(bind) == "\(guestRoot)/file.conf:/etc/harpoon.conf:ro", "bind translation preserves target and mode")
require(translator.translateBindsEntry("not-a-bind") == "not-a-bind", "malformed bind remains unchanged")
require(translator.translateBindsEntry("relative:/data:rw") == "relative:/data:rw", "relative bind source remains unchanged")

let createBody: [String: Any] = [
    "HostConfig": [
        "Binds": [bind],
        "Mounts": [
            ["Type": "bind", "Source": directory.path, "Target": "/bind"],
            ["Type": "volume", "Source": file.path, "Target": "/volume"],
            ["Source": file.path, "Target": "/legacy-bind"]
        ]
    ],
    "Mounts": [
        ["Type": "bind", "Source": file.path, "Target": "/top-bind"],
        ["Type": "volume", "Source": file.path, "Target": "/top-volume"]
    ]
]
let createData = try JSONSerialization.data(withJSONObject: createBody)
guard let translatedData = translator.translateCreateBody(createData),
      let translated = try JSONSerialization.jsonObject(with: translatedData) as? [String: Any],
      let hostConfig = translated["HostConfig"] as? [String: Any],
      let binds = hostConfig["Binds"] as? [String],
      let hostMounts = hostConfig["Mounts"] as? [[String: Any]],
      let topMounts = translated["Mounts"] as? [[String: Any]] else {
    fputs("FAIL: translated create body shape\n", stderr)
    exit(1)
}

require(binds == ["\(guestRoot)/file.conf:/etc/harpoon.conf:ro"], "HostConfig.Binds translation")
require(hostMounts[0]["Source"] as? String == "\(guestRoot)/directory", "explicit bind mount translation")
require(hostMounts[1]["Source"] as? String == file.path, "explicit volume mount must remain untouched")
require(hostMounts[2]["Source"] as? String == "\(guestRoot)/file.conf", "legacy mount without Type translates")
require(topMounts[0]["Source"] as? String == "\(guestRoot)/file.conf", "top-level bind mount translation")
require(topMounts[1]["Source"] as? String == file.path, "top-level volume mount must remain untouched")
require(logs.contains(where: { $0.contains("HARPOON_HOST_PATH_TRANSLATION_APPLIED") }), "body translation is logged")

require(translator.translateCreateBody(Data("not-json".utf8)) == nil, "invalid JSON must not translate")
let unchanged = try JSONSerialization.data(withJSONObject: ["Image": "alpine:latest"])
require(translator.translateCreateBody(unchanged) == nil, "unchanged create body returns nil")

print("host path translation regression PASS")
