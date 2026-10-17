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
        let dockerSocketIdentity = ownedSocketIdentity(fd: listenerFd, path: config.dockerSocketPath, owned: ownsDockerSocket)
        let balloonSocketIdentity = ownedSocketIdentity(fd: balloonControlFd, path: config.balloonControlPath, owned: ownsBalloonControlSocket)
        let mgmtSocketIdentity = ownedSocketIdentity(fd: mgmtListenerFd, path: config.mgmtSocketPath, owned: ownsMgmtSocket)
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
            removeOwnedSocket(config.dockerSocketPath, identity: dockerSocketIdentity)
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
            removeOwnedSocket(config.balloonControlPath, identity: balloonSocketIdentity)
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
            removeOwnedSocket(config.mgmtSocketPath, identity: mgmtSocketIdentity)
            ownsMgmtSocket = false
        } else {
            log("HARPOON_BRIDGES_STOP_ALL skip remove \(config.mgmtSocketPath) (not owned) end")
        }
    }

    private func ownedSocketIdentity(fd: Int32, path: String, owned: Bool) -> (dev: Int64, ino: Int64)? {
        guard owned, fd >= 0 else { return nil }
        var fdStat = stat()
        var pathStat = stat()
        guard fstat(fd, &fdStat) == 0, stat(path, &pathStat) == 0,
              (fdStat.st_mode & S_IFMT) == S_IFSOCK,
              (pathStat.st_mode & S_IFMT) == S_IFSOCK else { return nil }
        guard fdStat.st_dev == pathStat.st_dev && fdStat.st_ino == pathStat.st_ino else { return nil }
        return (Int64(fdStat.st_dev), Int64(fdStat.st_ino))
    }

    private func removeOwnedSocket(_ path: String, identity: (dev: Int64, ino: Int64)?) {
        guard let identity else {
            log("HARPOON_SOCKET_REMOVE_SKIP path=\(path) reason=owner-mismatch")
            return
        }
        var current = stat()
        guard stat(path, &current) == 0,
              Int64(current.st_dev) == identity.dev,
              Int64(current.st_ino) == identity.ino else {
            log("HARPOON_SOCKET_REMOVE_SKIP path=\(path) reason=path-replaced")
            return
        }
        try? FileManager.default.removeItem(atPath: path)
        log("HARPOON_SOCKET_REMOVED path=\(path) owner=inode")
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

}
