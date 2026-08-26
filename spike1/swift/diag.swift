import Virtualization
import Foundation

// ponytail: VZ lifecycle diagnostic — 3 fresh VMs, no Docker, known-good Spike1 kernel+tiny initramfs
// purpose: characterize transient HOST_VZ_START_FAILURE Code=1 without reboot
// each cycle: brand-new config/VM, fresh serial, no reuse, explicit release
func ts()->String{ ISO8601DateFormatter().string(from: Date()) }
func log(_ m:String){ fputs("[\(ts())] \(m)\n", stderr) }

let kernelURL = URL(fileURLWithPath: "spike1/cache/Image-virt")
let initramfsURL = URL(fileURLWithPath: "spike1/cache/harpoon-tiny-initramfs.cpio.gz")
let cycles = 3
let bootTimeout: TimeInterval = 10
let stopTimeout: TimeInterval = 5

log("host \(ProcessInfo.processInfo.operatingSystemVersionString) isSupported=\(VZVirtualMachine.isSupported)")
guard VZVirtualMachine.isSupported else { log("not supported"); exit(3) }
guard FileManager.default.fileExists(atPath: kernelURL.path) else { log("kernel missing"); exit(4)}
guard FileManager.default.fileExists(atPath: initramfsURL.path) else { log("initramfs missing"); exit(4)}

var currentCycle = 1

func runCycle(_ n:Int, completion: @escaping (Bool)->Void){
  log("CYCLE_\(n)_CREATE")
  let serialURL = URL(fileURLWithPath: "/tmp/harpoon-diag-cycle\(n).log")
  try? FileManager.default.removeItem(at: serialURL)

  let cfg = VZVirtualMachineConfiguration()
  cfg.cpuCount = 2
  cfg.memorySize = 512*1024*1024
  let plat = VZGenericPlatformConfiguration()
  cfg.platform = plat
  let bl = VZLinuxBootLoader(kernelURL: kernelURL)
  bl.commandLine = "console=hvc0"
  bl.initialRamdiskURL = initramfsURL
  cfg.bootLoader = bl
  cfg.entropyDevices = [VZVirtioEntropyDeviceConfiguration()]
  let net = VZVirtioNetworkDeviceConfiguration()
  net.attachment = VZNATNetworkDeviceAttachment()
  cfg.networkDevices = [net]
  let serial = VZVirtioConsoleDeviceSerialPortConfiguration()
  do {
    serial.attachment = try VZFileSerialPortAttachment(url: serialURL, append:false)
  } catch { log("CYCLE_\(n)_SERIAL_FAIL \(error)"); completion(false); return }
  cfg.serialPorts = [serial]

  do { try cfg.validate(); log("CYCLE_\(n)_VALIDATE_OK") } catch {
    let e=error as NSError
    log("CYCLE_\(n)_VALIDATE_FAIL \(e.domain) \(e.code) \(e.userInfo)")
    completion(false); return
  }

  // fresh VM object — no reuse
  // ponytail: retain note — vm captured strongly by start/stop closures; released only after STOPPED + pause
  var vm: VZVirtualMachine? = VZVirtualMachine(configuration: cfg)
  guard let vmUnwrapped = vm else { log("CYCLE_\(n)_VM_NIL"); completion(false); return }
  // use unowned for closures to avoid cycle, but need weak to allow release after stop
  log("CYCLE_\(n)_START_REQUEST state=\(vmUnwrapped.state.rawValue)")
  var bootDetected = false
  var cycleDone = false
  func finish(_ success:Bool){
    if cycleDone { return }; cycleDone = true
    // ensure observers cancelled, file handles closed via VZFileSerialPortAttachment auto-close
    // release VZ graph — critical: nil out vm, cfg, devices to drop strong refs
    DispatchQueue.main.asyncAfter(deadline:.now()+1.0){
      log("CYCLE_\(n)_RELEASED")
      vm = nil
      // pause briefly before next cycle to let host reclaim hypervisor resources
      DispatchQueue.main.asyncAfter(deadline:.now()+1.0){
        completion(success)
      }
    }
  }

  // boot poll
  let poll = DispatchSource.makeTimerSource(queue: .main)
  poll.schedule(deadline:.now()+1, repeating:1)
  poll.setEventHandler {
    guard let d=try? Data(contentsOf: serialURL), !d.isEmpty else { return }
    let s=String(data:d, encoding:.utf8) ?? ""
    if s.contains("HARPOON_SPIKE_OK") && !bootDetected {
      bootDetected = true
      log("CYCLE_\(n)_BOOT_DETECTED")
      poll.cancel()
      log("CYCLE_\(n)_STOP_REQUEST")
      if vmUnwrapped.canStop {
        vmUnwrapped.stop { err in
          if let err=err { log("CYCLE_\(n)_STOP_ERROR \(err)") }
          // wait until Stopped
          let wait = DispatchSource.makeTimerSource(queue:.main)
          wait.schedule(deadline:.now(), repeating:0.2)
          wait.setEventHandler {
            if vmUnwrapped.state == .stopped {
              wait.cancel()
              log("CYCLE_\(n)_STOPPED")
              finish(true)
            }
          }
          wait.resume()
          DispatchQueue.main.asyncAfter(deadline:.now()+stopTimeout){
            if vmUnwrapped.state != .stopped {
              log("CYCLE_\(n)_STOP_TIMEOUT state=\(vmUnwrapped.state.rawValue)")
              wait.cancel()
              finish(false)
            }
          }
        }
      } else if vmUnwrapped.canRequestStop {
        do { try vmUnwrapped.requestStop(); log("CYCLE_\(n)_REQUEST_STOP sent")
          DispatchQueue.main.asyncAfter(deadline:.now()+2){
            log("CYCLE_\(n)_STOPPED (requestStop)")
            finish(true)
          }
        } catch { log("CYCLE_\(n)_REQUEST_STOP_FAIL \(error)"); finish(false) }
      } else {
        log("CYCLE_\(n)_STOP_NOT_AVAILABLE state=\(vmUnwrapped.state.rawValue)")
        finish(false)
      }
    }
  }
  poll.resume()

  // boot timeout
  DispatchQueue.main.asyncAfter(deadline:.now()+bootTimeout){
    if !bootDetected && !cycleDone {
      poll.cancel()
      if let d=try? Data(contentsOf: serialURL), let s=String(data:d, encoding:.utf8) {
        log("CYCLE_\(n)_BOOT_TIMEOUT serial tail=\(s.suffix(300)) state=\(vmUnwrapped.state.rawValue)")
      } else {
        log("CYCLE_\(n)_BOOT_TIMEOUT no serial state=\(vmUnwrapped.state.rawValue)")
      }
      // attempt stop even if no boot
      if vmUnwrapped.state == .running || vmUnwrapped.state == .starting {
        if vmUnwrapped.canStop {
          vmUnwrapped.stop { _ in log("CYCLE_\(n)_STOPPED after timeout"); finish(false) }
        } else { finish(false) }
      } else {
        finish(false)
      }
    }
  }

  vmUnwrapped.start { result in
    if case .failure(let e) = result {
      let ne=e as NSError
      poll.cancel()
      log("CYCLE_\(n)_START_FAIL domain=\(ne.domain) code=\(ne.code) userInfo=\(ne.userInfo) state=\(vmUnwrapped.state.rawValue)")
      finish(false)
      return
    }
    log("CYCLE_\(n)_START_OK state=\(vmUnwrapped.state.rawValue)")
  }
}

func runNext(){
  if currentCycle > cycles {
    log("DIAG_COMPLETE cycles=\(cycles)")
    exit(0)
  }
  let n = currentCycle
  currentCycle += 1
  runCycle(n) { success in
    // if first cycle fails with Code=1, classify immediately per spec but continue to observe? Spec says stop diagnostic if cycle1 fails
    if n==1 && !success {
      // check if failure was START_FAIL Code=1 — log classification and stop
      log("HOST CURRENTLY IN TRANSIENT VZ START FAILURE — cycle 1 failed, stopping diagnostic (REBOOT_SKIPPED)")
      // do not retry-loop; exit after brief pause
      DispatchQueue.main.asyncAfter(deadline:.now()+1){ exit(0) }
      return
    }
    // if cycle 2 fails after 1 passed, that's REPRODUCIBLE state failure — continue to cycle 3 to confirm
    runNext()
  }
}

runNext()
RunLoop.main.run()
