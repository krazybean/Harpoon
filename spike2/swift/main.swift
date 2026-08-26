import Virtualization
import Foundation

// ponytail: minimal vsock bridge — sequential clients sufficient for spike, concurrent works but not required
// host bridge: Unix 0600 -> VZVirtioSocketDevice.connect(toPort:2375) -> vsock fd, full-duplex byte proxy, no Docker parsing
signal(SIGPIPE, SIG_IGN)

func log(_ m:String){ let ts=ISO8601DateFormatter().string(from: Date()); fputs("[\(ts)] \(m)\n", stderr) }

let kernelURL = URL(fileURLWithPath: "spike1/cache/Image-virt")
let initramfsURL = URL(fileURLWithPath: "spike2/cache/harpoon-docker-initramfs.cpio.gz")
let timeout:TimeInterval = 120 // bounded readiness for apk+docker at boot
let socketPath = "/tmp/harpoon-docker.sock"
let vsockPort: UInt32 = 2375
let hostForwardPort: UInt16 = 8080
let guestForwardPort: UInt16 = 8080

log("host \(ProcessInfo.processInfo.operatingSystemVersionString) isSupported=\(VZVirtualMachine.isSupported)")
guard VZVirtualMachine.isSupported else { exit(3) }
guard FileManager.default.fileExists(atPath: kernelURL.path) else { fputs("kernel missing\n", stderr); exit(4)}
guard FileManager.default.fileExists(atPath: initramfsURL.path) else { fputs("initramfs missing\n", stderr); exit(4)}

let cfg = VZVirtualMachineConfiguration()
cfg.cpuCount = 2
// Spike 5 — parameterized configured memory via HARPOON_MEMORY_MIB (512/768/1024, default 1024)
let rawMIB = ProcessInfo.processInfo.environment["HARPOON_MEMORY_MIB"] ?? "1024"
let parsedMIB = Int(rawMIB) ?? 1024
let allowedMIB: Set<Int> = [512, 768, 1024]
let memoryMIB = allowedMIB.contains(parsedMIB) ? parsedMIB : 1024
if !allowedMIB.contains(parsedMIB) && rawMIB != "1024" {
  log("HARPOON_MEMORY_CONFIG_WARN raw=\(rawMIB) parsed=\(parsedMIB) clamped to \(memoryMIB) (allowed 512/768/1024)")
}
cfg.memorySize = UInt64(memoryMIB) * 1024 * 1024
log("HARPOON_MEMORY_CONFIG_MIB \(memoryMIB)")
log("HARPOON_MEMORY_CONFIG_BYTES \(cfg.memorySize)")
let platform = VZGenericPlatformConfiguration()
cfg.platform = platform
let bl = VZLinuxBootLoader(kernelURL: kernelURL)
bl.commandLine = "console=hvc0"
bl.initialRamdiskURL = initramfsURL
cfg.bootLoader = bl
cfg.entropyDevices = [VZVirtioEntropyDeviceConfiguration()]
// serial
let serialURL = URL(fileURLWithPath: "/tmp/harpoon-spike2-serial.log")
try? FileManager.default.removeItem(at: serialURL)
let serial = VZVirtioConsoleDeviceSerialPortConfiguration()
serial.attachment = try! VZFileSerialPortAttachment(url: serialURL, append:false)
cfg.serialPorts = [serial]
// network for apk
let net = VZVirtioNetworkDeviceConfiguration()
net.attachment = VZNATNetworkDeviceAttachment()
cfg.networkDevices = [net]
// block-backed root for Docker (2G sparse raw ext4) — pivot_root suitable, not ramdisk
let diskURL = URL(fileURLWithPath: "spike2/cache/harpoon-root.img")
if FileManager.default.fileExists(atPath: diskURL.path) {
    do {
        let attachment = try VZDiskImageStorageDeviceAttachment(url: diskURL, readOnly: false)
        let blockDev = VZVirtioBlockDeviceConfiguration(attachment: attachment)
        cfg.storageDevices = [blockDev]
        log("block device attached \(diskURL.path) 2G ext4 harpoon-root")
    } catch {
        log("block attach failed \(error)")
    }
} else {
    log("block image missing at \(diskURL.path) — guest will remain ramdisk (pivot will fail)")
}
// harpoon-share VirtioFS — host /tmp/harpoon-share -> guest tag harpoon-share (readWrite, not exposing /Users)
let shareHostPath = "/tmp/harpoon-share"
do {
  try FileManager.default.createDirectory(atPath: shareHostPath, withIntermediateDirectories: true, attributes: nil)
  log("HARPOON_SHARE_HOST \(shareHostPath)")
} catch { log("HARPOON_SHARE_HOST_CREATE_FAILED \(error)") }
let sharedDirURL = URL(fileURLWithPath: shareHostPath)
let sharedDir = VZSharedDirectory(url: sharedDirURL, readOnly: false)
let singleShare = VZSingleDirectoryShare(directory: sharedDir)
let virtiofsConfig = VZVirtioFileSystemDeviceConfiguration(tag: "harpoon-share")
virtiofsConfig.share = singleShare
cfg.directorySharingDevices = [virtiofsConfig]
log("HARPOON_VIRTIOFS_CONFIGURED harpoon-share -> \(shareHostPath) readOnly=false")
// vsock

let vsockConfig = VZVirtioSocketDeviceConfiguration()
cfg.socketDevices = [vsockConfig]
// Spike 5 — memory balloon (modular, verify with 6.12.94-0-virt virtio_balloon.ko)
let balloonConfig = VZVirtioTraditionalMemoryBalloonDeviceConfiguration()
cfg.memoryBalloonDevices = [balloonConfig]
log("HARPOON_BALLOON_CONFIGURED \(memoryMIB)MiB cpu=2 balloon=virtio-traditional memorySize=\(cfg.memorySize) balloonInitial=\(cfg.memorySize)")

log("HARPOON_HOST_NETDEV_COUNT \(cfg.networkDevices.count) attachment=\(String(describing: cfg.networkDevices.first?.attachment)) VZNAT=\(cfg.networkDevices.first?.attachment is VZNATNetworkDeviceAttachment)")
do{ try cfg.validate(); log("validate OK HARPOON_HOST_NETDEV_COUNT \(cfg.networkDevices.count)")}catch{ let e=error as NSError; fputs("validate \(e.domain) \(e.code) \(e.userInfo)\n", stderr); exit(5)}

let vm = VZVirtualMachine(configuration: cfg)
var bootReady = false
var bootFailedReason: String? = nil
func checkSerial()->String{
  guard let d=try? Data(contentsOf: serialURL), !d.isEmpty else {return ""}
  let s=String(data:d, encoding:.utf8) ?? ""
  if s.contains("HARPOON_DOCKER_READY") && !bootReady {
    bootReady=true; log("HARPOON_DOCKER_READY observed")
  }
  if s.contains("HARPOON_DOCKER_FAILED") && bootFailedReason==nil {
    bootFailedReason = String(s.components(separatedBy: "HARPOON_DOCKER_FAILED").last?.prefix(300) ?? "")
    log("HARPOON_DOCKER_FAILED observed \(bootFailedReason ?? "")")
  }
  return s
}
let poll = DispatchSource.makeTimerSource(queue:.main)
poll.schedule(deadline:.now()+1, repeating:1)
poll.setEventHandler{ _=checkSerial() }
poll.resume()

// vsock device after VM start
var vsockDevice: VZVirtioSocketDevice?
var listenerFd: Int32 = -1
var listenerSource: DispatchSourceRead?
var activeProxies: [(clientFd:Int32, vsockFd:Int32)] = []
// host port forward state (127.0.0.1:8080 -> guestIP:8080)
var hostForwardFd: Int32 = -1
var hostForwardSource: DispatchSourceRead?
var hostForwardGuestIP: String? = nil
var hostForwardStarted: Bool = false
// spike 5 balloon control socket /tmp/harpoon-control
var balloonControlFd: Int32 = -1
var balloonControlSource: DispatchSourceRead?
var balloonControlClients: [Int32: DispatchSourceRead] = [:]
var balloonControlBuffers: [Int32: Data] = [:]

func cleanupUnixSocket(){
  if listenerFd>=0 { close(listenerFd); listenerFd = -1 }
  listenerSource?.cancel(); listenerSource=nil
  try? FileManager.default.removeItem(atPath: socketPath)
  log("cleaned /tmp/harpoon-docker.sock")
}
func parseGuestIP()->String? {
  guard let d=try? Data(contentsOf: serialURL), let s=String(data:d, encoding:.utf8) else { return nil }
  // find last HARPOON_GUEST_IP line
  var ip: String? = nil
  for line in s.components(separatedBy:"\n") {
    if line.contains("HARPOON_GUEST_IP") {
      let parts = line.components(separatedBy:"HARPOON_GUEST_IP")
      if let last = parts.last {
        let candidate = last.trimmingCharacters(in:.whitespacesAndNewlines).components(separatedBy:" ").first ?? ""
        // candidate may be "192.168.64.3" or "unknown"
        if candidate.hasPrefix("192.") || candidate.hasPrefix("10.") {
          ip = candidate
        } else if candidate.range(of:#"^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$"#, options:.regularExpression) != nil {
          ip = candidate
        }
      }
    }
  }
  return ip
}
func cleanupHostForward(){
  if hostForwardFd>=0 { close(hostForwardFd); hostForwardFd = -1 }
  hostForwardSource?.cancel(); hostForwardSource=nil
  log("HOST_FORWARD_CLEANED")
}
func startHostPortForward(guestIP: String){
  if hostForwardStarted { log("HOST_FORWARD_ALREADY \(guestIP)"); return }
  hostForwardStarted = true
  hostForwardGuestIP = guestIP
  let fd = socket(AF_INET, SOCK_STREAM, 0)
  guard fd>=0 else { log("HOST_FORWARD_SOCKET_FAILED \(String(cString:strerror(errno)))"); return }
  hostForwardFd = fd
  var one: Int32 = 1
  setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &one, socklen_t(MemoryLayout<Int32>.size))
  var addr = sockaddr_in()
  addr.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
  addr.sin_family = sa_family_t(AF_INET)
  addr.sin_port = in_port_t(hostForwardPort.bigEndian)
  addr.sin_addr.s_addr = inet_addr("127.0.0.1")
  let bindResult = withUnsafePointer(to: &addr) { ptr in
    ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockPtr in Darwin.bind(fd, sockPtr, socklen_t(MemoryLayout<sockaddr_in>.size)) }
  }
  guard bindResult==0 else { perror("bind hostForward"); log("HOST_FORWARD_BIND_FAILED 127.0.0.1:\(hostForwardPort) \(String(cString:strerror(errno)))"); close(fd); hostForwardFd = -1; return }
  guard listen(fd, 16)==0 else { perror("listen hostForward"); log("HOST_FORWARD_LISTEN_FAILED"); close(fd); return }
  log("HOST_FORWARD_LISTENING 127.0.0.1:\(hostForwardPort) -> \(guestIP):\(guestForwardPort) (loopback-only)")
  let flags = fcntl(fd, F_GETFL, 0)
  _ = fcntl(fd, F_SETFL, flags | O_NONBLOCK)
  var nextForwardId: Int = 0
  let source = DispatchSource.makeReadSource(fileDescriptor: fd, queue: .main)
  hostForwardSource = source
  source.setEventHandler {
    while true {
      var clientAddr = sockaddr_in()
      var clientLen = socklen_t(MemoryLayout<sockaddr_in>.size)
      let clientFd = withUnsafeMutablePointer(to: &clientAddr) { ptr in
        ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockPtr in accept(fd, sockPtr, &clientLen) }
      }
      if clientFd<0 {
        if errno==EAGAIN || errno==EWOULDBLOCK { break }
        log("HOST_FORWARD_ACCEPT_ERROR \(String(cString:strerror(errno)))")
        break
      }
      let fid = nextForwardId
      nextForwardId += 1
      log("HOST_FORWARD_ACCEPT \(fid) clientFd=\(clientFd) -> \(guestIP):\(guestForwardPort)")
      // connect to guest in background (blocking connect with timeout)
      DispatchQueue.global().async {
        let guestFd = socket(AF_INET, SOCK_STREAM, 0)
        if guestFd<0 {
          log("HOST_FORWARD_GUEST_SOCKET_FAILED \(fid) \(String(cString:strerror(errno)))")
          close(clientFd)
          return
        }
        // timeout 5s via SO_RCVTIMEO etc? use blocking connect with 5s via dispatch after?
        var gaddr = sockaddr_in()
        gaddr.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        gaddr.sin_family = sa_family_t(AF_INET)
        gaddr.sin_port = in_port_t(guestForwardPort.bigEndian)
        // inet_pton
        guestIP.withCString { cstr in inet_pton(AF_INET, cstr, &gaddr.sin_addr) }
        let conn = withUnsafePointer(to: &gaddr) { ptr in
          ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockPtr in connect(guestFd, sockPtr, socklen_t(MemoryLayout<sockaddr_in>.size)) }
        }
        if conn != 0 {
          log("HOST_FORWARD_CONNECT_FAILED \(fid) -> \(guestIP):\(guestForwardPort) \(String(cString:strerror(errno)))")
          close(clientFd); close(guestFd)
          return
        }
        log("HOST_FORWARD_CONNECTED \(fid) clientFd=\(clientFd) guestFd=\(guestFd) -> \(guestIP):\(guestForwardPort)")
        // full-duplex proxy (no HTTP parsing, TCP stream)
        let c2g = DispatchSource.makeReadSource(fileDescriptor: clientFd, queue: .global())
        let g2c = DispatchSource.makeReadSource(fileDescriptor: guestFd, queue: .global())
        var closedClientRead = false
        var closedGuestRead = false
        var closed = false
        func closeBoth(_ reason:String){
          if closed { return }; closed=true
          c2g.cancel(); g2c.cancel()
          close(clientFd); close(guestFd)
          log("HOST_FORWARD_CLOSE \(fid) \(reason)")
        }
        c2g.setEventHandler {
          var buf = [UInt8](repeating:0, count:8192)
          let n = buf.withUnsafeMutableBytes { ptr in read(clientFd, ptr.baseAddress!, ptr.count) }
          if n==0 {
            closedClientRead = true
            log("HOST_FORWARD_CLIENT_EOF \(fid)")
            c2g.cancel()
            shutdown(guestFd, SHUT_WR)
            if closedGuestRead { closeBoth("both EOF") }
            return
          }
          if n<0 {
            if errno==EAGAIN || errno==EWOULDBLOCK { return }
            if errno==EINTR { return }
            log("HOST_FORWARD_CLOSE \(fid) client read error \(String(cString:strerror(errno)))")
            closeBoth("client read"); return
          }
          var off=0
          while off<n {
            let w = buf.withUnsafeBytes { ptr in write(guestFd, ptr.baseAddress!.advanced(by: off), n-off) }
            if w>0 { off+=Int(w); continue }
            if w<0 && errno==EINTR { continue }
            if w<0 && (errno==EAGAIN || errno==EWOULDBLOCK) { usleep(1000); continue }
            if w<0 && errno==EPIPE { closeBoth("guest EPIPE"); return }
            if w<=0 { closeBoth("guest write"); return }
          }
        }
        g2c.setEventHandler {
          var buf = [UInt8](repeating:0, count:8192)
          let n = buf.withUnsafeMutableBytes { ptr in read(guestFd, ptr.baseAddress!, ptr.count) }
          if n==0 {
            closedGuestRead = true
            log("HOST_FORWARD_GUEST_EOF \(fid)")
            g2c.cancel()
            shutdown(clientFd, SHUT_WR)
            if closedClientRead { closeBoth("both EOF") }
            return
          }
          if n<0 {
            if errno==EAGAIN || errno==EWOULDBLOCK { return }
            if errno==EINTR { return }
            log("HOST_FORWARD_CLOSE \(fid) guest read error \(String(cString:strerror(errno)))")
            closeBoth("guest read"); return
          }
          var off=0
          while off<n {
            let w = buf.withUnsafeBytes { ptr in write(clientFd, ptr.baseAddress!.advanced(by: off), n-off) }
            if w>0 { off+=Int(w); continue }
            if w<0 && errno==EINTR { continue }
            if w<0 && (errno==EAGAIN || errno==EWOULDBLOCK) { usleep(1000); continue }
            if w<0 && errno==EPIPE { closeBoth("client EPIPE"); return }
            if w<=0 { closeBoth("client write"); return }
          }
        }
        c2g.setCancelHandler {}
        g2c.setCancelHandler {}
        c2g.resume(); g2c.resume()
      }
    }
  }
  source.setCancelHandler { close(fd) }
  source.resume()
  // atexit host forward closed via cleanupHostForward (cannot capture fd)
  _ = fd
}
func startUnixBridge(){
  // stale cleanup — minimal spike: unlink stale, do not attempt live-probe
  try? FileManager.default.removeItem(atPath: socketPath)
  let fd = socket(AF_UNIX, SOCK_STREAM, 0)
  guard fd>=0 else { log("socket failed \(String(cString:strerror(errno)))"); exit(6) }
  listenerFd = fd
  var addr = sockaddr_un()
  addr.sun_family = sa_family_t(AF_UNIX)
  memset(&addr.sun_path, 0, MemoryLayout.size(ofValue: addr.sun_path))
  _ = socketPath.withCString { src in withUnsafeMutablePointer(to: &addr.sun_path) { dst in strncpy(UnsafeMutableRawPointer(dst).assumingMemoryBound(to: CChar.self), src, MemoryLayout.size(ofValue: dst.pointee)-1) } }
  let len = socklen_t(MemoryLayout<sockaddr_un>.size)
  let bindResult = withUnsafePointer(to: addr) { ptr in
    ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockPtr in Darwin.bind(fd, sockPtr, len) }
  }
  guard bindResult==0 else { perror("bind"); log("bind failed"); exit(6) }
  chmod(socketPath, 0o600)
  guard listen(fd, 16)==0 else { perror("listen"); exit(6) }
  log("UNIX socket listening at \(socketPath) 0600")
  let flags = fcntl(fd, F_GETFL, 0)
  _ = fcntl(fd, F_SETFL, flags | O_NONBLOCK)

  var nextBridgeId: Int = 0
  let source = DispatchSource.makeReadSource(fileDescriptor: fd, queue: .main)
  listenerSource = source
  // ponytail: retain cycle note — source captures vm/vsockDevice; must cancel on VM stop to release graph
  source.setEventHandler {
    while true {
      var clientAddr = sockaddr_un()
      var clientLen: socklen_t = socklen_t(MemoryLayout<sockaddr_un>.size)
      let clientFd = withUnsafeMutablePointer(to: &clientAddr) { ptr in
        ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockPtr in accept(fd, sockPtr, &clientLen) }
      }
      if clientFd<0 {
        if errno==EAGAIN || errno==EWOULDBLOCK { break }
        log("accept error \(String(cString:strerror(errno)))")
        break
      }
      let bridgeId = nextBridgeId
      nextBridgeId += 1
      log("BRIDGE_ACCEPT \(bridgeId) fd=\(clientFd)")
      guard let device = vsockDevice else { log("BRIDGE_CLOSE \(bridgeId) vsockDevice not ready"); close(clientFd); continue }
      device.connect(toPort: vsockPort) { result in
        switch result {
        case .failure(let error):
          log("VSOCK_CONNECT_FAILURE port \(vsockPort) error \(error)")
          log("BRIDGE_CLOSE \(bridgeId) vsock connect failed")
          close(clientFd)
        case .success(let vsockConn):
          let vsockFd = vsockConn.fileDescriptor
          log("BRIDGE_VSOCK_CONNECTED \(bridgeId) vsockFd=\(vsockFd) clientFd=\(clientFd)")
          // transparent byte-stream proxy — no Docker HTTP parsing
          // handles: partial writes, EAGAIN, EINTR, EPIPE, half-close (shutdown SHUT_WR), keep-alive (multiple requests on one connection), concurrent clients (each id independent)
          let clientRead = DispatchSource.makeReadSource(fileDescriptor: clientFd, queue: .global())
          let vsockRead = DispatchSource.makeReadSource(fileDescriptor: vsockFd, queue: .global())
          var closedClientRead = false
          var closedVsockRead = false
          var closed = false
          func closeBoth(_ reason: String){
            if closed { return }; closed=true
            clientRead.cancel(); vsockRead.cancel()
            close(clientFd)
            vsockConn.close()
            log("BRIDGE_CLOSE \(bridgeId) \(reason) clientFd=\(clientFd) vsockFd=\(vsockFd)")
          }
          func shutdownVsockWrite(){
            if !closedVsockRead {
              shutdown(vsockFd, SHUT_WR)
              log("BRIDGE_VSOCK_SHUT_WR \(bridgeId)")
            }
          }
          func shutdownClientWrite(){
            if !closedClientRead {
              shutdown(clientFd, SHUT_WR)
              log("BRIDGE_CLIENT_SHUT_WR \(bridgeId)")
            }
          }
          clientRead.setEventHandler {
            var buf = [UInt8](repeating:0, count:8192)
            let n = buf.withUnsafeMutableBytes { ptr in read(clientFd, ptr.baseAddress!, ptr.count) }
            if n==0 {
              closedClientRead = true
              log("BRIDGE_CLIENT_EOF \(bridgeId)")
              clientRead.cancel()
              shutdownVsockWrite()
              if closedVsockRead { closeBoth("both EOF") }
              return
            }
            if n<0 {
              if errno==EAGAIN || errno==EWOULDBLOCK { return }
              if errno==EINTR { return }
              log("BRIDGE_CLOSE \(bridgeId) client read error \(String(cString:strerror(errno)))")
              closeBoth("client read error"); return
            }
            var off=0
            while off<n {
              let w = buf.withUnsafeBytes { ptr in write(vsockFd, ptr.baseAddress!.advanced(by: off), n-off) }
              if w>0 { off+=Int(w); continue }
              if w<0 && errno==EINTR { continue }
              if w<0 && (errno==EAGAIN || errno==EWOULDBLOCK) {
                // vsock not ready for write — wait briefly then retry (vsock is blocking, should not happen)
                usleep(1000); continue
              }
              if w<0 && errno==EPIPE {
                log("BRIDGE_CLOSE \(bridgeId) vsock EPIPE on write")
                closeBoth("vsock EPIPE"); return
              }
              log("BRIDGE_CLOSE \(bridgeId) vsock write error \(String(cString:strerror(errno)))")
              closeBoth("vsock write error"); return
            }
          }
          vsockRead.setEventHandler {
            var buf = [UInt8](repeating:0, count:8192)
            let n = buf.withUnsafeMutableBytes { ptr in read(vsockFd, ptr.baseAddress!, ptr.count) }
            if n==0 {
              closedVsockRead = true
              log("BRIDGE_VSOCK_EOF \(bridgeId)")
              vsockRead.cancel()
              shutdownClientWrite()
              if closedClientRead { closeBoth("both EOF") }
              return
            }
            if n<0 {
              if errno==EAGAIN || errno==EWOULDBLOCK { return }
              if errno==EINTR { return }
              log("BRIDGE_CLOSE \(bridgeId) vsock read error \(String(cString:strerror(errno)))")
              closeBoth("vsock read error"); return
            }
            var off=0
            while off<n {
              let w = buf.withUnsafeBytes { ptr in write(clientFd, ptr.baseAddress!.advanced(by: off), n-off) }
              if w>0 { off+=Int(w); continue }
              if w<0 && errno==EINTR { continue }
              if w<0 && (errno==EAGAIN || errno==EWOULDBLOCK) { usleep(1000); continue }
              if w<0 && errno==EPIPE {
                log("BRIDGE_CLOSE \(bridgeId) client EPIPE on write")
                closeBoth("client EPIPE"); return
              }
              log("BRIDGE_CLOSE \(bridgeId) client write error \(String(cString:strerror(errno)))")
              closeBoth("client write error"); return
            }
          }
          clientRead.setCancelHandler {}
          vsockRead.setCancelHandler {}
          log("BRIDGE_VSOCK_CONNECTED \(bridgeId) proxy start — keep-alive, half-close, concurrent safe")
          clientRead.resume(); vsockRead.resume()
        }
      }
    }
  }
  source.setCancelHandler { close(fd) }
  source.resume()
  // cleanup on normal exit
  atexit { try? FileManager.default.removeItem(atPath: socketPath) }
}

log("starting VM kernel=Image-virt initramfs=harpoon-docker")
vm.start { result in
  if case .failure(let e)=result {
    let ne=e as NSError
    fputs("start FAIL \(ne.domain) \(ne.code) \(ne.userInfo)\n", stderr)
    log("HOST_VZ_START_FAILURE")
    exit(7)
  }
  log("VM start SUCCESS state=\(vm.state.rawValue)")
  if let dev = vm.socketDevices.first as? VZVirtioSocketDevice {
    vsockDevice = dev
    log("VSOCK device ready")
  } else {
    log("VSOCK device not found")
  }
  // Spike 5 — balloon runtime device (verify exactly one, expose diagnostic control via Unix socket)
  if let balloon = vm.memoryBalloonDevices.first as? VZVirtioTraditionalMemoryBalloonDevice {
    log("HARPOON_BALLOON_RUNTIME_FOUND count=\(vm.memoryBalloonDevices.count) type=VZVirtioTraditionalMemoryBalloonDevice")
    let initial = balloon.targetVirtualMachineMemorySize
    log("HARPOON_BALLOON_TARGET_INITIAL \(initial)")
    log("HARPOON_BALLOON_GRANULARITY 1048576 minimum=\(VZVirtualMachineConfiguration.minimumAllowedMemorySize) maximum=\(VZVirtualMachineConfiguration.maximumAllowedMemorySize)")
    // Unix control socket /tmp/harpoon-control (0600) — preferred diagnostic control, not stdin
    let controlPath = "/tmp/harpoon-control"
    try? FileManager.default.removeItem(atPath: controlPath)
    let cfd = socket(AF_UNIX, SOCK_STREAM, 0)
    balloonControlFd = cfd
    if cfd < 0 {
      log("HARPOON_BALLOON_CONTROL_FAILED socket \(String(cString:strerror(errno)))")
    } else {
      var caddr = sockaddr_un()
      caddr.sun_family = sa_family_t(AF_UNIX)
      memset(&caddr.sun_path, 0, MemoryLayout.size(ofValue: caddr.sun_path))
      _ = controlPath.withCString { src in withUnsafeMutablePointer(to: &caddr.sun_path) { dst in strncpy(UnsafeMutableRawPointer(dst).assumingMemoryBound(to: CChar.self), src, MemoryLayout.size(ofValue: dst.pointee)-1) } }
      let clen = socklen_t(MemoryLayout<sockaddr_un>.size)
      let cbind = withUnsafePointer(to: caddr) { ptr in ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sp in Darwin.bind(cfd, sp, clen) } }
      if cbind != 0 {
        log("HARPOON_BALLOON_CONTROL_FAILED bind \(String(cString:strerror(errno)))")
        close(cfd); balloonControlFd = -1
      } else {
        chmod(controlPath, 0o600)
        if listen(cfd, 8) != 0 {
          log("HARPOON_BALLOON_CONTROL_FAILED listen \(String(cString:strerror(errno)))")
          close(cfd); balloonControlFd = -1
        } else {
          let cflags = fcntl(cfd, F_GETFL, 0)
          _ = fcntl(cfd, F_SETFL, cflags | O_NONBLOCK)
          log("HARPOON_BALLOON_CONTROL_LISTENING \(controlPath)")
          balloonControlSource = DispatchSource.makeReadSource(fileDescriptor: cfd, queue: .main)
          guard let csource = balloonControlSource else { log("HARPOON_BALLOON_CONTROL_FAILED source"); close(cfd); balloonControlFd = -1; return }
          csource.setEventHandler {
            while true {
              var caddr2 = sockaddr_un()
              var clen2 = socklen_t(MemoryLayout<sockaddr_un>.size)
              let clientFd = withUnsafeMutablePointer(to: &caddr2) { ptr in ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sp in accept(cfd, sp, &clen2) } }
              if clientFd < 0 {
                if errno == EAGAIN || errno == EWOULDBLOCK { break }
                log("HARPOON_BALLOON_CONTROL_ACCEPT_FAILED \(String(cString:strerror(errno)))")
                break
              }
              log("HARPOON_BALLOON_CONTROL_ACCEPT fd=\(clientFd)")
              // Make client nonblocking and create retained DispatchSourceRead for robust async read
              let cflags2 = fcntl(clientFd, F_GETFL, 0)
              _ = fcntl(clientFd, F_SETFL, cflags2 | O_NONBLOCK)
              balloonControlBuffers[clientFd] = Data()
              let clientSource = DispatchSource.makeReadSource(fileDescriptor: clientFd, queue: .main)
              balloonControlClients[clientFd] = clientSource
              clientSource.setEventHandler {
                var buf = [UInt8](repeating: 0, count: 8192)
                let n = read(clientFd, &buf, buf.count)
                if n > 0 {
                  log("HARPOON_BALLOON_CONTROL_READ fd=\(clientFd) bytes=\(n)")
                  var data = balloonControlBuffers[clientFd] ?? Data()
                  data.append(contentsOf: buf[0..<n])
                  balloonControlBuffers[clientFd] = data
                  // Process complete lines immediately, buffer across reads
                  while let nlRange = data.range(of: Data([UInt8(ascii: "\n")])) {
                    let lineData = data.subdata(in: 0..<nlRange.lowerBound)
                    let line = String(data: lineData, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
                    data.removeSubrange(0..<nlRange.upperBound)
                    balloonControlBuffers[clientFd] = data
                    if line.isEmpty { continue }
                    log("HARPOON_BALLOON_CONTROL_LINE \(line)")
                    let lower = line.lowercased()
                    var token = lower
                    if lower.hasPrefix("balloon") { token = String(lower.dropFirst(7)).trimmingCharacters(in: .whitespaces) }
                    else if lower.hasPrefix("target") { token = String(lower.dropFirst(6)).trimmingCharacters(in: .whitespaces) }
                    token = token.replacingOccurrences(of: "mib", with: "").replacingOccurrences(of: "mb", with: "").replacingOccurrences(of: "m", with: "")
                    token = token.trimmingCharacters(in: .whitespaces)
                    var requested: UInt64? = nil
                    if let v = UInt64(token) {
                      if v < 8192 { requested = v * 1024 * 1024 }
                      else { requested = v }
                    }
                    if let req = requested {
                      log("HARPOON_BALLOON_TARGET_REQUEST \(req)")
                      balloon.targetVirtualMachineMemorySize = req
                      let set = balloon.targetVirtualMachineMemorySize
                      log("HARPOON_BALLOON_TARGET_SET \(set)")
                      log("HARPOON_BALLOON_TARGET_APPLIED requested=\(req) actual=\(set) MiB=\(set/1024/1024)")
                    } else {
                      log("HARPOON_BALLOON_TARGET_PARSE_FAILED \(line)")
                    }
                  }
                  balloonControlBuffers[clientFd] = data
                } else if n == 0 {
                  // EOF — process final buffered line if non-empty (client sent without newline)
                  log("HARPOON_BALLOON_CONTROL_EOF fd=\(clientFd)")
                  if let data = balloonControlBuffers[clientFd], !data.isEmpty {
                    let line = String(data: data, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
                    if !line.isEmpty {
                      log("HARPOON_BALLOON_CONTROL_LINE \(line)")
                      let lower = line.lowercased()
                      var token = lower
                      if lower.hasPrefix("balloon") { token = String(lower.dropFirst(7)).trimmingCharacters(in: .whitespaces) }
                      else if lower.hasPrefix("target") { token = String(lower.dropFirst(6)).trimmingCharacters(in: .whitespaces) }
                      token = token.replacingOccurrences(of: "mib", with: "").replacingOccurrences(of: "mb", with: "").replacingOccurrences(of: "m", with: "")
                      token = token.trimmingCharacters(in: .whitespaces)
                      var requested: UInt64? = nil
                      if let v = UInt64(token) {
                        if v < 8192 { requested = v * 1024 * 1024 }
                        else { requested = v }
                      }
                      if let req = requested {
                        log("HARPOON_BALLOON_TARGET_REQUEST \(req)")
                        balloon.targetVirtualMachineMemorySize = req
                        let set = balloon.targetVirtualMachineMemorySize
                        log("HARPOON_BALLOON_TARGET_SET \(set)")
                        log("HARPOON_BALLOON_TARGET_APPLIED requested=\(req) actual=\(set) MiB=\(set/1024/1024)")
                      } else {
                        log("HARPOON_BALLOON_TARGET_PARSE_FAILED \(line)")
                      }
                    }
                  }
                  log("HARPOON_BALLOON_CONTROL_CLOSE fd=\(clientFd)")
                  clientSource.cancel()
                  balloonControlClients.removeValue(forKey: clientFd)
                  balloonControlBuffers.removeValue(forKey: clientFd)
                  close(clientFd)
                } else {
                  if errno != EAGAIN && errno != EWOULDBLOCK && errno != EINTR {
                    log("HARPOON_BALLOON_CONTROL_READ_FAILED fd=\(clientFd) err=\(String(cString:strerror(errno)))")
                    clientSource.cancel()
                    balloonControlClients.removeValue(forKey: clientFd)
                    balloonControlBuffers.removeValue(forKey: clientFd)
                    close(clientFd)
                  }
                }
              }
              clientSource.setCancelHandler {
                // retained until cancel, cleanup already done in EOF/error
              }
              clientSource.resume()
            }
          }
          csource.setCancelHandler { close(cfd); balloonControlFd = -1; try? FileManager.default.removeItem(atPath: controlPath) }
          csource.resume()
          // Cleanup via cancel handler, not atexit capture
          // control socket cleaned via setCancelHandler, not atexit with capture
        }
      }
    }
  } else {
    log("HARPOON_BALLOON_RUNTIME_NOT_FOUND count=\(vm.memoryBalloonDevices.count)")
  }
  let readyPoll = DispatchSource.makeTimerSource(queue:.main)
  readyPoll.schedule(deadline:.now()+1, repeating:1)
  readyPoll.setEventHandler {
    _=checkSerial()
    if bootReady {
      readyPoll.cancel()
      log("HARPOON_DOCKER_READY observed, starting Unix bridge")
      startUnixBridge()
      log("VM Running, vsock bridge active, socket \(socketPath)")
      // Spike 3: host port forward 127.0.0.1:8080 -> guestIP:8080 (poll serial for HARPOON_GUEST_IP)
      let forwardPoll = DispatchSource.makeTimerSource(queue:.main)
      forwardPoll.schedule(deadline:.now()+1, repeating:1)
      var forwardAttempts = 0
      forwardPoll.setEventHandler {
        forwardAttempts += 1
        if let ip = parseGuestIP() {
          forwardPoll.cancel()
          log("HARPOON_GUEST_IP_DISCOVERED \(ip) -> starting host forward")
          startHostPortForward(guestIP: ip)
        } else if forwardAttempts > 15 {
          log("HOST_FORWARD_DISCOVERY_FAILED no HARPOON_GUEST_IP after 15s serial tail=\(checkSerial().suffix(400))")
          forwardPoll.cancel()
          // still start with fallback 192.168.64.2 to allow diagnostics (will log CONNECT_FAILED if NAT blocked)
          log("HOST_FORWARD_TRY_FALLBACK 192.168.64.3")
          startHostPortForward(guestIP: "192.168.64.3")
        }
      }
      forwardPoll.resume()
    }
    if let r = bootFailedReason {
      readyPoll.cancel()
      log("GUEST_INIT_FAILURE HARPOON_DOCKER_FAILED \(r)")
      // do not start bridge, keep VM for diagnostics then exit
      DispatchQueue.main.asyncAfter(deadline:.now()+2) { exit(6) }
    }
  }
  readyPoll.resume()
  DispatchQueue.main.asyncAfter(deadline:.now()+timeout){
    if !bootReady && bootFailedReason==nil {
      let s=checkSerial()
      log("GUEST_INIT_FAILURE no HARPOON_DOCKER_READY after \(timeout)s tail=\(s.suffix(800))")
      exit(6)
    }
  }
}

RunLoop.main.run()
