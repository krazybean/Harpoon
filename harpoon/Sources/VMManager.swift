import Virtualization
import Foundation

// ponytail: minimal VM owner — no abstractions, explicit FD/DispatchSource ownership for bounded cleanup
final class VMManager {
    let config: RuntimeConfig
    let lifecycle: Lifecycle
    var vm: VZVirtualMachine?
    var vsockDevice: VZVirtioSocketDevice?
    var balloonDevice: VZVirtioTraditionalMemoryBalloonDevice?

    // serial poll
    var serialPoll: DispatchSourceTimer?
    var bootReady = false
    var bootFailedReason: String?

    // bridges owned here, cleaned on stop
    var bridge: BridgeSet?

    init(config: RuntimeConfig, lifecycle: Lifecycle) {
        self.config = config
        self.lifecycle = lifecycle
    }

    func log(_ m: String) {
        let ts = ISO8601DateFormatter().string(from: Date())
        fputs("[\(ts)] \(m)\n", stderr)
    }

    func buildVM() throws -> VZVirtualMachine {
        let cfg = VZVirtualMachineConfiguration()
        cfg.cpuCount = config.cpuCount
        cfg.memorySize = config.memorySizeBytes
        log("HARPOON_CPU_CONFIG_COUNT \(config.cpuCount)")
        log("HARPOON_MEMORY_CONFIG_MIB \(config.memoryMIB)")
        log("HARPOON_MEMORY_CONFIG_BYTES \(config.memorySizeBytes)")
        log("HARPOON_DISK_IMAGE \(config.diskURL.path)")
        log("HARPOON_DISK_LOGICAL_BYTES \(config.diskLogicalBytes)")
        let platform = VZGenericPlatformConfiguration()
        cfg.platform = platform
        let bl = VZLinuxBootLoader(kernelURL: config.kernelURL)
        bl.commandLine = "console=hvc0"
        bl.initialRamdiskURL = config.initramfsURL
        cfg.bootLoader = bl
        cfg.entropyDevices = [VZVirtioEntropyDeviceConfiguration()]
        // serial
        let serialURL = URL(fileURLWithPath: config.serialLogPath)
        try? FileManager.default.removeItem(at: serialURL)
        let serial = VZVirtioConsoleDeviceSerialPortConfiguration()
        serial.attachment = try VZFileSerialPortAttachment(url: serialURL, append: false)
        cfg.serialPorts = [serial]
        // network
        let net = VZVirtioNetworkDeviceConfiguration()
        net.attachment = VZNATNetworkDeviceAttachment()
        cfg.networkDevices = [net]
        // block-backed root
        if FileManager.default.fileExists(atPath: config.diskURL.path) {
            do {
                let att = try VZDiskImageStorageDeviceAttachment(url: config.diskURL, readOnly: false)
                cfg.storageDevices = [VZVirtioBlockDeviceConfiguration(attachment: att)]
                log("block device attached \(config.diskURL.path)")
            } catch {
                log("block attach failed \(error)")
            }
        } else {
            log("block image missing at \(config.diskURL.path) — ramdisk fallback")
        }
        // VirtioFS — M4 shared roots: legacy /tmp/harpoon-share plus /Users and /private/tmp
        var fsDevices: [VZVirtioFileSystemDeviceConfiguration] = []
        for (host, tag) in config.allVirtioShares {
            try FileManager.default.createDirectory(atPath: host, withIntermediateDirectories: true, attributes: nil)
            log("HARPOON_SHARE_HOST \(host) tag=\(tag)")
            let sharedDir = VZSharedDirectory(url: URL(fileURLWithPath: host), readOnly: false)
            let singleShare = VZSingleDirectoryShare(directory: sharedDir)
            let fsConfig = VZVirtioFileSystemDeviceConfiguration(tag: tag)
            fsConfig.share = singleShare
            fsDevices.append(fsConfig)
            log("HARPOON_VIRTIOFS_CONFIGURED \(tag) -> \(host) readOnly=false")
        }
        cfg.directorySharingDevices = fsDevices
        log("HARPOON_VIRTIOFS_DEVICE_COUNT \(fsDevices.count)")
        // vsock
        let vsockConfig = VZVirtioSocketDeviceConfiguration()
        cfg.socketDevices = [vsockConfig]
        // balloon
        let balloonConfig = VZVirtioTraditionalMemoryBalloonDeviceConfiguration()
        cfg.memoryBalloonDevices = [balloonConfig]
        log("HARPOON_BALLOON_CONFIGURED \(config.memoryMIB)MiB cpu=\(config.cpuCount) balloon=virtio-traditional memorySize=\(config.memorySizeBytes) balloonInitial=\(config.memorySizeBytes)")
        try cfg.validate()
        log("validate OK HARPOON_HOST_NETDEV_COUNT \(cfg.networkDevices.count)")
        let vm = VZVirtualMachine(configuration: cfg)
        self.vm = vm
        return vm
    }

    func checkSerial() -> String {
        guard let d = try? Data(contentsOf: URL(fileURLWithPath: config.serialLogPath)), !d.isEmpty else { return "" }
        let s = String(data: d, encoding: .utf8) ?? ""
        if s.contains("HARPOON_DOCKER_READY") && !bootReady {
            bootReady = true
            log("HARPOON_DOCKER_READY observed")
        }
        if s.contains("HARPOON_DOCKER_FAILED") && bootFailedReason == nil {
            bootFailedReason = String(s.components(separatedBy: "HARPOON_DOCKER_FAILED").last?.prefix(400) ?? "")
            log("HARPOON_DOCKER_FAILED observed \(bootFailedReason ?? "")")
        }
        return s
    }

    func startBridges() {
        guard let vm = vm else { return }
        vsockDevice = vm.socketDevices.first as? VZVirtioSocketDevice
        balloonDevice = vm.memoryBalloonDevices.first as? VZVirtioTraditionalMemoryBalloonDevice
        if vsockDevice != nil { log("VSOCK device ready") } else { log("VSOCK device not found") }
        if let b = balloonDevice {
            log("HARPOON_BALLOON_RUNTIME_FOUND count=\(vm.memoryBalloonDevices.count) type=VZVirtioTraditionalMemoryBalloonDevice")
            log("HARPOON_BALLOON_TARGET_INITIAL \(b.targetVirtualMachineMemorySize)")
            log("HARPOON_BALLOON_GRANULARITY 1048576 minimum=\(VZVirtualMachineConfiguration.minimumAllowedMemorySize) maximum=\(VZVirtualMachineConfiguration.maximumAllowedMemorySize)")
        } else {
            log("HARPOON_BALLOON_RUNTIME_NOT_FOUND count=\(vm.memoryBalloonDevices.count)")
        }
        bridge = BridgeSet(config: config, vsockDevice: vsockDevice, balloonDevice: balloonDevice, log: log)
        bridge?.startAll()
    }

    func stopBridges() {
        bridge?.stopAll()
        bridge = nil
    }

    func cleanupEphemeral() {
        log("HARPOON_CLEANUP_EPHEMERAL_BEGIN dockerSock=\(config.dockerSocketPath) balloonControl=\(config.balloonControlPath)")
        stopBridges()
        // ownership: BridgeSet.stopAll removes pathnames only if owned; do not blindly unlink global paths here
        log("HARPOON_EPHEMERAL_CLEANED \(config.dockerSocketPath) \(config.balloonControlPath) (owned only)")
    }
}
