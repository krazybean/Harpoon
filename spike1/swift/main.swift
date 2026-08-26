import Virtualization
import Foundation

// ponytail: spike1 minimal VM proof — no abstractions, blocking run loop with timeout, single VZVirtualMachine
// proves Rust-centric arch can boot ARM64 Linux via Virtualization.framework (Swift bridge if Rust bindings fragile)

func log(_ msg: String) {
    let ts = ISO8601DateFormatter().string(from: Date())
    fputs("[\(ts)] \(msg)\n", stderr)
}

guard CommandLine.arguments.count >= 3 else {
    fputs("usage: harpoon-spike1 <kernel> <initramfs> [timeoutSec]\n", stderr)
    exit(2)
}
let kernelURL = URL(fileURLWithPath: CommandLine.arguments[1])
let initramfsURL = URL(fileURLWithPath: CommandLine.arguments[2])
let timeout: TimeInterval = CommandLine.arguments.count >= 4 ? Double(CommandLine.arguments[3]) ?? 30 : 30

log("host: \(ProcessInfo.processInfo.operatingSystemVersionString) arch=\(ProcessInfo.processInfo.environment["HOST_ARCH"] ?? "arm64")")
log("isSupported=\(VZVirtualMachine.isSupported)")
if !VZVirtualMachine.isSupported { fputs("Virtualization not supported on this host\n", stderr); exit(3) }
guard FileManager.default.fileExists(atPath: kernelURL.path) else { fputs("kernel not found: \(kernelURL.path)\n", stderr); exit(4) }
guard FileManager.default.fileExists(atPath: initramfsURL.path) else { fputs("initramfs not found: \(initramfsURL.path)\n", stderr); exit(4) }

let config = VZVirtualMachineConfiguration()
config.cpuCount = 2
config.memorySize = 512 * 1024 * 1024
let platform = VZGenericPlatformConfiguration()
config.platform = platform
let bootLoader = VZLinuxBootLoader(kernelURL: kernelURL)
bootLoader.commandLine = "console=hvc0"
bootLoader.initialRamdiskURL = initramfsURL
config.bootLoader = bootLoader
config.entropyDevices = [VZVirtioEntropyDeviceConfiguration()]

// serial console -> file via VZVirtioConsoleDeviceSerialPortConfiguration (maps to guest console)
let serialLogURL = URL(fileURLWithPath: "/tmp/harpoon-spike1-serial.log")
try? FileManager.default.removeItem(at: serialLogURL)
let serialConfig = VZVirtioConsoleDeviceSerialPortConfiguration()
serialConfig.attachment = try! VZFileSerialPortAttachment(url: serialLogURL, append: false)
config.serialPorts = [serialConfig]


// NAT network so Alpine can get outbound if needed (not required for boot proof)
let netConfig = VZVirtioNetworkDeviceConfiguration()
netConfig.attachment = VZNATNetworkDeviceAttachment()
config.networkDevices = [netConfig]

do {
    try config.validate()
    log("config validated")
} catch {
    let e = error as NSError
    fputs("validate failed: \(e.localizedDescription) domain=\(e.domain) code=\(e.code) info=\(e.userInfo)\n", stderr)
    exit(5)
}

let vm = VZVirtualMachine(configuration: config)

// capture guest output by polling file
var bootDetected = false
func checkSerialLog() -> String {
    guard let data = try? Data(contentsOf: serialLogURL), !data.isEmpty else { return "" }
    let str = String(data: data, encoding: .utf8) ?? ""
    if str.contains("HARPOON_SPIKE_OK") {
        if !bootDetected {
            bootDetected = true
            log("BOOT_DETECTED HARPOON_SPIKE_OK")
        }
    }
    return str
}

let timeoutWork = DispatchWorkItem {
    // re-check serial before deciding timeout — fixes race where boot was detected but poll hadn't cancelled yet
    let serialStr = checkSerialLog()
    if bootDetected {
        // boot was observed, do not emit TIMEOUT; proceed to clean shutdown via timeout path if poll missed
        log("boot detected before timeout, proceeding to shutdown")
        if vm.canStop {
            vm.stop { err in
                if let err = err { log("stop after timeout error: \(err)"); exit(6) }
                log("SHUTDOWN_OK")
                exit(0)
            }
        } else if vm.canRequestStop {
            do { try vm.requestStop(); log("requestStop sent") } catch { log("requestStop error: \(error)") }
            DispatchQueue.main.asyncAfter(deadline: .now()+2) { exit(0) }
        } else {
            exit(0)
        }
        return
    }
    log("TIMEOUT: no boot marker after \(timeout)s, serial size=\(serialStr.count) tail=\(String(serialStr.suffix(500)))")
    log("captured tail: \(String(serialStr.suffix(2000)))")
    // attempt stop even if not booted
    if vm.canStop {
        vm.stop { err in
            if let err = err { log("stop after timeout error: \(err)") }
            else { log("stop after timeout ok") }
            exit(bootDetected ? 0 : 6)
        }
    } else if vm.canRequestStop {
        do { try vm.requestStop(); log("requestStop sent") } catch { log("requestStop error: \(error)") }
        DispatchQueue.main.asyncAfter(deadline: .now()+2) { exit(bootDetected ? 0 : 6) }
    } else {
        exit(bootDetected ? 0 : 6)
    }
}
DispatchQueue.main.asyncAfter(deadline: .now() + timeout, execute: timeoutWork)

// when boot detected, schedule graceful shutdown shortly after (allow some stable time)
var shutdownScheduled = false
func scheduleShutdown() {
    guard !shutdownScheduled else { return }
    shutdownScheduled = true
    DispatchQueue.main.asyncAfter(deadline: .now() + 3) {
        log("attempting graceful shutdown...")
        if vm.canRequestStop {
            do {
                try vm.requestStop()
                log("requestStop succeeded, waiting 5s then force stop if needed")
                DispatchQueue.main.asyncAfter(deadline: .now() + 5) {
                    if vm.state == .running || vm.state == .starting {
                        if vm.canStop {
                            vm.stop { err in
                                if let err = err { log("force stop error: \(err)"); exit(6) }
                                log("force stop ok")
                                log("SHUTDOWN_OK")
                                exit(0)
                            }
                        } else {
                            log("still running but canStop false, exiting")
                            exit(0)
                        }
                    } else {
                        log("SHUTDOWN_OK")
                        exit(0)
                    }
                }
            } catch {
                log("requestStop failed: \(error), trying stop")
                if vm.canStop {
                    vm.stop { err in
                        if let err = err { log("stop error: \(err)"); exit(6) }
                        log("SHUTDOWN_OK")
                        exit(0)
                    }
                } else { exit(6) }
            }
        } else if vm.canStop {
            vm.stop { err in
                if let err = err { log("stop error: \(err)"); exit(6) }
                log("SHUTDOWN_OK")
                exit(0)
            }
        } else {
            log("cannot stop nor requestStop, state=\(vm.state.rawValue)")
            exit(6)
        }
    }
}

// poll for bootDetected to trigger shutdown — calls checkSerialLog each tick
let poll = DispatchSource.makeTimerSource(queue: .main)
poll.schedule(deadline: .now()+1, repeating: 1)
poll.setEventHandler {
    _ = checkSerialLog()
    if bootDetected && !shutdownScheduled {
        timeoutWork.cancel()
        log("boot confirmed, scheduling shutdown")
        scheduleShutdown()
    }
}
poll.resume()

log("starting VM kernel=\(kernelURL.lastPathComponent) initramfs=\(initramfsURL.lastPathComponent)")
vm.start { result in
    if case .failure(let err) = result {
        let e = err as NSError
        fputs("start failed: \(e.localizedDescription) domain=\(e.domain) code=\(e.code) info=\(e.userInfo)\n", stderr)
        // dump captured output if any
        let s = checkSerialLog()
        if !s.isEmpty {
            log("partial output: \(s.prefix(2000))")
        }
        exit(7)
    }
    log("VM start returned ok, state=\(vm.state.rawValue)")
}

RunLoop.main.run()
