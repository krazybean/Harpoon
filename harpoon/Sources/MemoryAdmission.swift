import Foundation

/// Host-wide guest-memory admission. Every engine, whatever its HARPOON_TEST_TMPDIR namespace, registers one reservation file in a
/// per-user directory under the REAL /tmp and holds an exclusive flock on it for its whole life. The kernel drops that lock when the
/// process dies, so "live" is never a guess from a pid: a file whose lock can be taken has no owner and is reclaimed. Deciding and
/// registering happen under one admission lock, so concurrent starts are serialized and cannot both fit into the same headroom.
enum MemoryAdmission {
    static let overheadMiB = 512 // per guest: VZ helper, vsock/bridge buffers, page cache
    static let budgetPercent = 60 // of physical memory; the rest is for macOS and every other app
    static let unreadableReservationMiB = 4096 // a live reservation that cannot be read counts as a default-size guest (fail closed)

    /// nil when `requestedMiB` fits next to `activeMiB`; otherwise why not.
    static func failure(requestedMiB: Int, activeMiB: [Int], hostMiB: Int) -> String? {
        let budget = hostMiB * budgetPercent / 100
        let need = requestedMiB + activeMiB.reduce(0, +) + overheadMiB * (activeMiB.count + 1)
        if need <= budget { return nil }
        return "guest memory admission denied: \(requestedMiB) MiB requested, \(activeMiB.count) other guest(s) hold \(activeMiB.reduce(0, +)) MiB; with \(overheadMiB) MiB overhead each that needs \(need) MiB but the budget is \(budget) MiB (\(budgetPercent)% of \(hostMiB) MiB). Stop another guest or lower --memory."
    }

    final class Reservation {
        let path: String
        private var fd: Int32
        fileprivate init(path: String, fd: Int32) { self.path = path; self.fd = fd }
        /// Normal exit is unnecessary (the kernel drops the lock); this is for tests and explicit stops.
        func release() { if fd >= 0 { unlink(path); close(fd); fd = -1 } }
        deinit { release() }
    }

    enum Outcome {
        case admitted(Reservation)
        case denied(String)
        case unavailable(String)
    }

    static func defaultDirectory() -> String { "/tmp/harpoon-admission-\(getuid())" }

    /// `afterDecision` runs between the decision and the registration; tests widen that window to prove the admission lock closes it.
    static func reserve(requestedMiB: Int, hostMiB: Int, namespace: String, directory: String = defaultDirectory(), afterDecision: (() -> Void)? = nil) -> Outcome {
        if mkdir(directory, 0o700) != 0 && errno != EEXIST { return .unavailable("cannot create \(directory): \(String(cString: strerror(errno)))") }
        let dirFd = open(directory, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
        if dirFd < 0 { return .unavailable("cannot open \(directory): \(String(cString: strerror(errno)))") }
        defer { close(dirFd) }
        var st = stat()
        guard fstat(dirFd, &st) == 0, st.st_uid == getuid(), st.st_mode & 0o022 == 0 else {
            return .unavailable("\(directory) is not a private directory owned by this user")
        }
        let lockFd = open(directory + "/admission.lock", O_CREAT | O_RDWR | O_CLOEXEC | O_NOFOLLOW, 0o600)
        if lockFd < 0 { return .unavailable("cannot open admission lock: \(String(cString: strerror(errno)))") }
        defer { close(lockFd) }
        while flock(lockFd, LOCK_EX) != 0 { // ADMISSION_LOCK: serializes decide + register across every process
            if errno != EINTR { return .unavailable("cannot take admission lock: \(String(cString: strerror(errno)))") }
        }

        var active: [Int] = []
        for name in ((try? FileManager.default.contentsOfDirectory(atPath: directory)) ?? []) where name.hasSuffix(".res") {
            let path = directory + "/" + name
            let fd = open(path, O_RDWR | O_CLOEXEC | O_NOFOLLOW)
            if fd < 0 { continue }
            defer { close(fd) }
            if flock(fd, LOCK_EX | LOCK_NB) == 0 { unlink(path); continue } // STALE: nobody holds it, so its owner is gone
            var buf = [UInt8](repeating: 0, count: 256)
            let n = read(fd, &buf, buf.count - 1)
            let text = n > 0 ? String(decoding: buf[0..<n], as: UTF8.self) : ""
            active.append(text.split(separator: " ").first(where: { $0.hasPrefix("mib=") }).flatMap { Int($0.dropFirst(4)) } ?? unreadableReservationMiB)
        }
        if let why = failure(requestedMiB: requestedMiB, activeMiB: active, hostMiB: hostMiB) { return .denied(why) }
        afterDecision?()

        let path = "\(directory)/\(getpid())-\(UUID().uuidString).res"
        let fd = open(path, O_CREAT | O_EXCL | O_RDWR | O_CLOEXEC, 0o600)
        if fd < 0 { return .unavailable("cannot create reservation: \(String(cString: strerror(errno)))") }
        let line = Array("mib=\(requestedMiB) pid=\(getpid()) ns=\(namespace.replacingOccurrences(of: " ", with: "_"))\n".utf8)
        guard flock(fd, LOCK_EX | LOCK_NB) == 0, write(fd, line, line.count) == line.count else {
            let e = String(cString: strerror(errno)); unlink(path); close(fd)
            return .unavailable("cannot register reservation: \(e)")
        }
        return .admitted(Reservation(path: path, fd: fd))
    }
}
