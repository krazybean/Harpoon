import Foundation
import Virtualization

// ponytail: bridges are explicit — each owns one FD + DispatchSource, cleaned centrally on STOPPING, no global state
final class BridgeSet {
    let config: RuntimeConfig
    let vsockDevice: VZVirtioSocketDevice?
    let balloonDevice: VZVirtioTraditionalMemoryBalloonDevice?
    let log: (String)->Void
    lazy var translator: HostPathTranslator = {
        let roots = config.sharedRoots + [SharedRoot(hostPath: config.shareHostPath, guestPath: "/mnt/harpoon-share", tag: config.virtioFSTag)]
        return HostPathTranslator(roots: roots, log: log)
    }()

    // docker sock bridge
    var listenerFd: Int32 = -1
    var listenerSource: DispatchSourceRead?
    var ownsDockerSocket = false
    // M5 dynamic port publishing
    var portManager: PortForwardManager?
    var guestIPPoll: DispatchSourceTimer?
    var guestIP: String?
    // legacy single-forward stubs (kept to keep _legacy compiling, not used)
    var hostForwardFd: Int32 = -1
    var hostForwardSource: DispatchSourceRead?
    var hostForwardGuestIP: String?
    var hostForwardStarted = false
    // balloon control
    var balloonControlFd: Int32 = -1
    var balloonControlSource: DispatchSourceRead?
    var ownsBalloonControlSocket = false
    var balloonClients: [Int32: DispatchSourceRead] = [:]
    var balloonBuffers: [Int32: Data] = [:]
    // management channel (Stage 3A) — vsock 2377, unix 0600, no TCP
    var mgmtListenerFd: Int32 = -1
    var mgmtListenerSource: DispatchSourceRead?
    var ownsMgmtSocket = false

    init(config: RuntimeConfig, vsockDevice: VZVirtioSocketDevice?, balloonDevice: VZVirtioTraditionalMemoryBalloonDevice?, log: @escaping (String)->Void) {
        self.config = config
        self.vsockDevice = vsockDevice
        self.balloonDevice = balloonDevice
        self.log = log
    }

    func startAll() {
        startUnixBridge()
        startBalloonControl()
        startMgmtBridge()
        startPortForwarding()
    }

    func stopAll() {
        log("HARPOON_BRIDGES_STOP_ALL begin dockerSock=\(config.dockerSocketPath) balloonControl=\(config.balloonControlPath) listenerFd=\(listenerFd) balloonFd=\(balloonControlFd) ownsDocker=\(ownsDockerSocket) ownsBalloon=\(ownsBalloonControlSocket)")
        // FD ownership: DispatchSource cancelHandler owns close; stopAll only cancels and nils.
        if listenerSource != nil {
            listenerSource?.cancel(); listenerSource = nil
            listenerFd = -1
        } else if listenerFd >= 0 {
            // fallback if source missing but fd leaked
            close(listenerFd); listenerFd = -1
        }
        // socket pathname removed only if this BridgeSet owns it
        if ownsDockerSocket {
            try? FileManager.default.removeItem(atPath: config.dockerSocketPath)
            log("HARPOON_BRIDGES_STOP_ALL removed \(config.dockerSocketPath) (owned)")
            ownsDockerSocket = false
        } else {
            log("HARPOON_BRIDGES_STOP_ALL skip remove \(config.dockerSocketPath) (not owned)")
        }
        portManager?.stopAll()
        portManager = nil
        guestIPPoll?.cancel(); guestIPPoll = nil
        guestIP = nil
        log("HOST_FORWARD_CLEANED")
        if balloonControlSource != nil {
            balloonControlSource?.cancel(); balloonControlSource = nil
            balloonControlFd = -1
        } else if balloonControlFd >= 0 {
            close(balloonControlFd); balloonControlFd = -1
        }
        for (fd, src) in balloonClients { src.cancel(); close(fd) }
        balloonClients.removeAll(); balloonBuffers.removeAll()
        if ownsBalloonControlSocket {
            try? FileManager.default.removeItem(atPath: config.balloonControlPath)
            log("HARPOON_BRIDGES_STOP_ALL removed \(config.balloonControlPath) (owned) end")
            ownsBalloonControlSocket = false
        } else {
            log("HARPOON_BRIDGES_STOP_ALL skip remove \(config.balloonControlPath) (not owned) end")
        }
        if mgmtListenerSource != nil {
            mgmtListenerSource?.cancel(); mgmtListenerSource = nil
            mgmtListenerFd = -1
        } else if mgmtListenerFd >= 0 {
            close(mgmtListenerFd); mgmtListenerFd = -1
        }
        if ownsMgmtSocket {
            try? FileManager.default.removeItem(atPath: config.mgmtSocketPath)
            log("HARPOON_BRIDGES_STOP_ALL removed \(config.mgmtSocketPath) (owned) end")
            ownsMgmtSocket = false
        } else {
            log("HARPOON_BRIDGES_STOP_ALL skip remove \(config.mgmtSocketPath) (not owned) end")
        }
    }

    func isSocketLive(_ path: String) -> Bool {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        if fd < 0 { return false }
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        memset(&addr.sun_path, 0, MemoryLayout.size(ofValue: addr.sun_path))
        _ = path.withCString { src in withUnsafeMutablePointer(to: &addr.sun_path) { dst in strncpy(UnsafeMutableRawPointer(dst).assumingMemoryBound(to: CChar.self), src, MemoryLayout.size(ofValue: dst.pointee)-1) } }
        let len = socklen_t(MemoryLayout<sockaddr_un>.size)
        let ret = withUnsafePointer(to: addr) { ptr in ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sp in connect(fd, sp, len) } }
        close(fd)
        return ret == 0
    }

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
