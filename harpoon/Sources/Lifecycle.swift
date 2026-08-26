import Foundation

enum LifecycleState: String {
    case stopped = "STOPPED"
    case starting = "STARTING"
    case booting = "BOOTING"
    case dockerReady = "DOCKER_READY"
    case running = "RUNNING"
    case stopping = "STOPPING"
    case failed = "FAILED"
}

final class Lifecycle {
    private(set) var state: LifecycleState = .stopped
    private let lock = NSLock()

    func transition(to next: LifecycleState, reason: String? = nil) {
        lock.lock()
        let prev = state
        state = next
        lock.unlock()
        if let r = reason {
            fputs("[\(ISO8601DateFormatter().string(from: Date()))] HARPOON_STATE \(prev.rawValue) -> \(next.rawValue) reason=\(r)\n", stderr)
        } else {
            fputs("[\(ISO8601DateFormatter().string(from: Date()))] HARPOON_STATE \(prev.rawValue) -> \(next.rawValue)\n", stderr)
        }
    }

    func fail(_ reason: String) {
        transition(to: .failed, reason: reason)
    }
}
