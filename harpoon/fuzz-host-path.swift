import Foundation

struct SharedRoot {
    let hostPath: String
    let guestPath: String
    let tag: String
}

struct SeededRNG {
    private var state: UInt64

    init(seed: UInt64) {
        self.state = seed
    }

    mutating func next() -> UInt64 {
        state = state &* 6364136223846793005 &+ 1442695040888963407
        return state
    }

    mutating func int(_ upperBound: Int) -> Int {
        precondition(upperBound > 0)
        return Int(next() % UInt64(upperBound))
    }

    mutating func bool() -> Bool {
        (next() & 1) == 1
    }
}

let seed: UInt64 = 0x484152504f4f4e // "HARPOON"
let iterations = 4_000
var rng = SeededRNG(seed: seed)
var logCount = 0

let roots = [
    SharedRoot(hostPath: "/tmp", guestPath: "/mnt/harpoon-host/tmp", tag: "tmp"),
    SharedRoot(hostPath: "/Users", guestPath: "/mnt/harpoon-host/users", tag: "users")
]

let translator = HostPathTranslator(roots: roots) { _ in logCount += 1 }

func fail(_ message: String, iteration: Int) -> Never {
    fputs("FUZZ FAIL seed=0x\(String(seed, radix: 16)) iteration=\(iteration): \(message)\n", stderr)
    exit(1)
}

let atoms = [
    "", ".", "..", "tmp", "private", "Users", "file", "dir", "with space",
    "colon:name", "💩", "路径", "файл", "~", "-", "_", "...", "%2e%2e"
]
let modes = ["", "ro", "rw", "z", "delegated", "cached"]
let mountTypes = ["bind", "volume", "tmpfs", "npipe", "", "BIND", "unknown"]

func randomSegment(_ rng: inout SeededRNG) -> String {
    var segment = atoms[rng.int(atoms.count)]
    if rng.int(4) == 0 {
        segment += String(rng.next(), radix: 36)
    }
    return segment
}

func randomPath(_ rng: inout SeededRNG) -> String {
    let prefixes = ["/tmp", "/private/tmp", "/Users", "/var", "/", "relative", "../relative", ""]
    var path = prefixes[rng.int(prefixes.count)]
    let count = rng.int(7)
    for _ in 0..<count {
        if !path.isEmpty && !path.hasSuffix("/") { path += "/" }
        path += randomSegment(&rng)
    }
    if rng.int(5) == 0 { path += "/" }
    return path
}

func jsonObject(_ data: Data, iteration: Int) -> Any {
    do {
        return try JSONSerialization.jsonObject(with: data, options: [])
    } catch {
        fail("translator returned invalid JSON: \(error)", iteration: iteration)
    }
}

// Property 1: arbitrary bind/path strings must never crash. Relative/unshared
// sources must remain unchanged; translated outputs must preserve target/mode.
for i in 0..<iterations {
    let source = randomPath(&rng)
    let target = "/container/\(randomSegment(&rng))"
    let mode = modes[rng.int(modes.count)]
    let entry = mode.isEmpty ? "\(source):\(target)" : "\(source):\(target):\(mode)"
    let result = translator.translateBindsEntry(entry)

    let sourceShouldTranslate = source.hasPrefix("/tmp") || source.hasPrefix("/private/tmp") || source == "/Users" || source.hasPrefix("/Users/")
    if !sourceShouldTranslate && result != entry {
        fail("unshared/relative bind was rewritten: \(entry) -> \(result)", iteration: i)
    }
    if !result.contains(":\(target)") {
        fail("bind target was not preserved: \(entry) -> \(result)", iteration: i)
    }
    if !mode.isEmpty && !result.hasSuffix(":\(mode)") {
        fail("bind mode was not preserved: \(entry) -> \(result)", iteration: i)
    }
}

// Property 2: malformed/non-JSON byte streams must fail closed. If a future
// implementation chooses to transform one, any returned payload must still be valid JSON.
for i in 0..<iterations {
    let length = rng.int(257)
    var bytes = [UInt8]()
    bytes.reserveCapacity(length)
    for _ in 0..<length {
        bytes.append(UInt8(truncatingIfNeeded: rng.next()))
    }
    let data = Data(bytes)
    if let output = translator.translateCreateBody(data) {
        _ = jsonObject(output, iteration: i)
    }
}

// Property 3: explicit non-bind mounts must never be rewritten, even when their
// Source looks exactly like a shared host path. Bind mounts should translate.
for i in 0..<iterations {
    let type = mountTypes[rng.int(mountTypes.count)]
    let source = rng.bool() ? "/tmp/fuzz-\(rng.next())" : "/Users/fuzz-\(rng.next())"
    let mount: [String: Any] = [
        "Type": type,
        "Source": source,
        "Target": "/data"
    ]
    let body: [String: Any] = ["HostConfig": ["Mounts": [mount]]]
    let input = try JSONSerialization.data(withJSONObject: body, options: [])
    let output = translator.translateCreateBody(input)

    if type == "bind" {
        guard let output else {
            fail("explicit bind mount was not translated", iteration: i)
        }
        guard let object = jsonObject(output, iteration: i) as? [String: Any],
              let hostConfig = object["HostConfig"] as? [String: Any],
              let mounts = hostConfig["Mounts"] as? [[String: Any]],
              let translated = mounts.first?["Source"] as? String,
              translated != source,
              translated.hasPrefix("/mnt/harpoon-host/") else {
            fail("explicit bind mount translated incorrectly", iteration: i)
        }
    } else if !type.isEmpty {
        if let output {
            guard let object = jsonObject(output, iteration: i) as? [String: Any],
                  let hostConfig = object["HostConfig"] as? [String: Any],
                  let mounts = hostConfig["Mounts"] as? [[String: Any]],
                  let preserved = mounts.first?["Source"] as? String,
                  preserved == source else {
                fail("explicit non-bind mount was rewritten (type=\(type))", iteration: i)
            }
        }
    }
}

// Property 4: mixed payloads may rewrite binds, but must preserve neighboring volumes.
for i in 0..<iterations {
    let volumeSource = "/tmp/volume-\(rng.next())"
    let bindSource = "/tmp/bind-\(rng.next())"
    let body: [String: Any] = [
        "HostConfig": [
            "Mounts": [
                ["Type": "volume", "Source": volumeSource, "Target": "/volume"],
                ["Type": "bind", "Source": bindSource, "Target": "/bind"]
            ]
        ]
    ]
    let input = try JSONSerialization.data(withJSONObject: body, options: [])
    guard let output = translator.translateCreateBody(input),
          let object = jsonObject(output, iteration: i) as? [String: Any],
          let hostConfig = object["HostConfig"] as? [String: Any],
          let mounts = hostConfig["Mounts"] as? [[String: Any]],
          mounts.count == 2 else {
        fail("mixed mount payload was not transformed", iteration: i)
    }

    guard mounts[0]["Source"] as? String == volumeSource else {
        fail("neighboring volume mount changed", iteration: i)
    }
    guard let translatedBind = mounts[1]["Source"] as? String,
          translatedBind != bindSource,
          translatedBind.hasPrefix("/mnt/harpoon-host/tmp/") else {
        fail("neighboring bind mount did not translate", iteration: i)
    }
}

print("HOST_PATH_FUZZ_PASS seed=0x\(String(seed, radix: 16)) iterations=\(iterations) logs=\(logCount)")
