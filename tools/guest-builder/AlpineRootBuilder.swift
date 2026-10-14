import Foundation
import Virtualization

final class BuilderDelegate: NSObject, VZVirtualMachineDelegate {
    var vm: VZVirtualMachine?
    var finished = false
    var failure = false
    var serialPath = ""

    func virtualMachine(_ virtualMachine: VZVirtualMachine, didStopWithError error: Error) {
        fputs("[alpine-root-builder] VM stopped: \(error)\n", stderr)
        failure = true
        finished = true
    }

    func virtualMachine(_ virtualMachine: VZVirtualMachine, networkDevice: VZNetworkDevice, attachmentWasDisconnectedWithError error: Error) {
        fputs("[alpine-root-builder] network disconnected: \(error)\n", stderr)
    }

    func poll() {
        guard let data = try? Data(contentsOf: URL(fileURLWithPath: serialPath)),
              let serial = String(data: data, encoding: .utf8) else { return }
        if serial.contains("HARPOON_ROOT_BUILDER_DONE") {
            fputs(serial, stderr)
            finished = true
            vm?.stop { error in
                if let error { fputs("[alpine-root-builder] stop failed: \(error)\n", stderr); self.failure = true }
                self.finished = true
            }
        } else if serial.contains("HARPOON_ROOT_BUILDER_FAIL") {
            fputs(serial, stderr)
            failure = true
            finished = true
            vm?.stop { _ in }
        }
    }
}

func usage() -> Never {
    fputs("usage: AlpineRootBuilder <kernel> <initramfs> <disk> <serial>\n", stderr)
    exit(2)
}

let args = Array(CommandLine.arguments.dropFirst())
guard args.count == 4 else { usage() }
guard VZVirtualMachine.isSupported else { fputs("Virtualization.framework unavailable\n", stderr); exit(2) }

let kernel = URL(fileURLWithPath: args[0])
let initramfs = URL(fileURLWithPath: args[1])
let disk = URL(fileURLWithPath: args[2])
let serialURL = URL(fileURLWithPath: args[3])
try? FileManager.default.removeItem(at: serialURL)

let config = VZVirtualMachineConfiguration()
config.cpuCount = 2
config.memorySize = 1024 * 1024 * 1024
let boot = VZLinuxBootLoader(kernelURL: kernel)
boot.initialRamdiskURL = initramfs
boot.commandLine = "console=hvc0"
config.bootLoader = boot
config.entropyDevices = [VZVirtioEntropyDeviceConfiguration()]
let network = VZVirtioNetworkDeviceConfiguration()
network.attachment = VZNATNetworkDeviceAttachment()
config.networkDevices = [network]
let serial = VZVirtioConsoleDeviceSerialPortConfiguration()
serial.attachment = try VZFileSerialPortAttachment(url: serialURL, append: false)
config.serialPorts = [serial]
let storage = try VZDiskImageStorageDeviceAttachment(url: disk, readOnly: false)
config.storageDevices = [VZVirtioBlockDeviceConfiguration(attachment: storage)]
try config.validate()

let delegate = BuilderDelegate()
delegate.serialPath = serialURL.path
let vm = VZVirtualMachine(configuration: config)
delegate.vm = vm
vm.delegate = delegate
vm.start { error in
    if case .failure(let error) = error {
        fputs("[alpine-root-builder] start failed: \(error)\n", stderr)
        delegate.failure = true
        delegate.finished = true
    }
}

let timer = DispatchSource.makeTimerSource(queue: .main)
timer.schedule(deadline: .now(), repeating: .milliseconds(250))
timer.setEventHandler { delegate.poll() }
timer.resume()
while !delegate.finished { RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.25)) }
timer.cancel()
exit(delegate.failure ? 1 : 0)
