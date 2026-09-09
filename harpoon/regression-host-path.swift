import Foundation

struct SharedRoot {
    let hostPath: String
    let guestPath: String
    let tag: String
}

let root = URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent("harpoon-host-path-\(UUID().uuidString)")
let directory = root.appendingPathComponent("directory")
let file = root.appendingPathComponent("file.conf")
try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
try Data("file-bind".utf8).write(to: file)
defer { try? FileManager.default.removeItem(at: root) }

var logs: [String] = []
let translator = HostPathTranslator(roots: [SharedRoot(hostPath: root.path, guestPath: "/mnt/harpoon-host/test", tag: "test")]) { logs.append($0) }
guard translator.translateHostPath(directory.path) == "/mnt/harpoon-host/test/directory",
      translator.translateHostPath(file.path) == "/mnt/harpoon-host/test/file.conf",
      FileManager.default.fileExists(atPath: file.path),
      (try? file.resourceValues(forKeys: [.isRegularFileKey]).isRegularFile) == true,
      logs.contains(where: { $0.contains("type=dir") }),
      logs.contains(where: { $0.contains("type=file") }) else {
    exit(1)
}
print("host path file+directory translation PASS")
