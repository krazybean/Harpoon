import Foundation
import Virtualization

extension BridgeSet {
// MARK: - Port forward (M5 dynamic)

func startPortForwarding() {
    let mgr = PortForwardManager(log: log)
    mgr.setVsockDevice(vsockDevice)
    self.portManager = mgr
    mgr.startPolling()
    // guest IP discovery — poll serial log for HARPOON_GUEST_IP, fallback to 192.168.64.3
    let poll = DispatchSource.makeTimerSource(queue: .main)
    self.guestIPPoll = poll
    poll.schedule(deadline: .now()+1, repeating: 1)
    var attempts = 0
    poll.setEventHandler { [weak self] in
        guard let self = self else { poll.cancel(); return }
        attempts += 1
        if let ip = self.parseGuestIP() {
            poll.cancel()
            self.guestIP = ip
            self.log("HARPOON_GUEST_IP_DISCOVERED \(ip)")
            mgr.setGuestIP(ip)
            // trigger initial sync after DOCKER_READY
            mgr.scheduleSync(delayMs: 1000)
            mgr.scheduleSync(delayMs: 3000)
        } else if attempts > 15 {
            self.log("HOST_FORWARD_DISCOVERY_FAILED no HARPOON_GUEST_IP after 15s")
            poll.cancel()
            let fallback = "192.168.64.3"
            self.log("HOST_FORWARD_TRY_FALLBACK \(fallback)")
            self.guestIP = fallback
            mgr.setGuestIP(fallback)
            mgr.scheduleSync(delayMs: 1000)
        }
    }
    poll.resume()
    // also update vsock device if it becomes available later (already set)
}

func parseGuestIP() -> String? {
    guard let d = try? Data(contentsOf: URL(fileURLWithPath: config.serialLogPath)), let s = String(data: d, encoding: .utf8) else { return nil }
    var ip: String?
    for line in s.components(separatedBy: "\n") where line.contains("HARPOON_GUEST_IP") {
        let parts = line.components(separatedBy: "HARPOON_GUEST_IP")
        if let last = parts.last {
            let cand = last.trimmingCharacters(in: .whitespacesAndNewlines).components(separatedBy: " ").first ?? ""
            if cand.hasPrefix("192.") || cand.hasPrefix("10.") { ip = cand }
            else if cand.range(of: #"^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$"#, options: .regularExpression) != nil { ip = cand }
        }
    }
    return ip
}

func _legacy_startHostPortForward(guestIP: String) { log("legacy hardcoded 8080 forward disabled"); return
// legacy body retained below but unreachable — kept for reference, not used
}
}
