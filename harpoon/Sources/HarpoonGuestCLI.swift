import Foundation
import Darwin

func handleExec(args: [String]) -> Int32 {
    // parse -- separator
    var argv: [String] = []
    if let dashIdx = args.firstIndex(of: "--") {
        argv = Array(args[(dashIdx+1)...])
    } else {
        // allow without -- if args provided (support both)
        argv = args
        // but if args empty or starts with -, treat as missing
        if argv.isEmpty {
            cliError("usage: harpoon exec -- <command> [args...]")
            return 2
        }
        if argv.first?.hasPrefix("-") == true {
            cliError("usage: harpoon exec -- <command> [args...]")
            cliError("hint: use '--' to separate harpoon options from guest command")
            return 2
        }
    }
    if argv.isEmpty {
        cliError("usage: harpoon exec -- <command> [args...]")
        return 2
    }
    return mgmtExec(argv: argv)
}

func handleShell(args: [String]) -> Int32 {
    // any args after shell are ignored? spec says no args
    if args.contains("--help") || args.contains("-h") {
        cliPrint("usage: harpoon shell")
        return 0
    }
    let snap = statusSnapshot()
    if snap.state == "stopped" || snap.state == "stale" {
        cliError("Harpoon VM is not running")
        return 1
    }
    if !waitForManagementReady() {
        cliError("Guest management service is not ready: \(managementFailureReason())")
        return 1
    }
    guard let fd = connectMgmtSocket() else {
        cliError("Connection to guest management service failed")
        return 1
    }
    // send shell request
    let req: [String: Any] = ["op": "shell"]
    guard let jsonData = try? JSONSerialization.data(withJSONObject: req, options: []),
          let jsonStr = String(data: jsonData, encoding: .utf8) else {
        close(fd); return 1
    }
    let line = jsonStr + "\n"
    let bytes = Array(line.utf8)
    var off = 0
    while off < bytes.count {
        let n = bytes.withUnsafeBytes { ptr in write(fd, ptr.baseAddress!.advanced(by: off), bytes.count - off) }
        if n <= 0 { if errno == EINTR { continue }; close(fd); cliError("Connection to guest management service failed"); return 1 }
        off += Int(n)
    }
    // terminal raw mode if tty
    let isTTY = isatty(STDIN_FILENO) != 0
    var origTerm = termios()
    var rawTerm = termios()
    var didSetRaw = false
    if isTTY {
        if tcgetattr(STDIN_FILENO, &origTerm) == 0 {
            rawTerm = origTerm
            cfmakeraw(&rawTerm)
            // keep SIG handling? raw disables, we want Ctrl-C to go to guest, so keep raw
            if tcsetattr(STDIN_FILENO, TCSANOW, &rawTerm) == 0 {
                didSetRaw = true
            }
        }
    }
    defer {
        if didSetRaw { tcsetattr(STDIN_FILENO, TCSANOW, &origTerm) }
        close(fd)
    }
    // proxy loop: stdin <-> socket, socket <-> stdout
    signal(SIGPIPE, SIG_IGN)
    var shouldExit = false
    var receivedGuestData = false
    var connectionFailed = false
    // set non-blocking for stdin and fd
    let f1 = fcntl(STDIN_FILENO, F_GETFL, 0); if f1 >= 0 { _ = fcntl(STDIN_FILENO, F_SETFL, f1 | O_NONBLOCK) }
    let f2 = fcntl(fd, F_GETFL, 0); if f2 >= 0 { _ = fcntl(fd, F_SETFL, f2 | O_NONBLOCK) }
    while !shouldExit {
        var pfds = [pollfd(fd: STDIN_FILENO, events: Int16(POLLIN), revents: 0), pollfd(fd: fd, events: Int16(POLLIN), revents: 0)]
        let ret = poll(&pfds, 2, 1000)
        if ret < 0 { if errno == EINTR { continue }; connectionFailed = true; break }
        if ret == 0 { continue }
        if (pfds[0].revents & Int16(POLLIN)) != 0 {
            var buf = [UInt8](repeating: 0, count: 8192)
            let n = read(STDIN_FILENO, &buf, buf.count)
            if n > 0 {
                var off2 = 0
                while off2 < n {
                    let w = buf.withUnsafeBytes { ptr in write(fd, ptr.baseAddress!.advanced(by: off2), n-off2) }
                    if w > 0 { off2 += Int(w); continue }
                    if w < 0 && errno == EINTR { continue }
                    if w < 0 && (errno == EAGAIN || errno == EWOULDBLOCK) { usleep(1000); continue }
                    connectionFailed = true; shouldExit = true; break
                }
            } else if n == 0 {
                // EOF
                shutdown(fd, SHUT_WR)
                shouldExit = true
            }
        }
        if (pfds[1].revents & Int16(POLLIN)) != 0 {
            var buf = [UInt8](repeating: 0, count: 8192)
            let n = buf.withUnsafeMutableBytes { ptr in read(fd, ptr.baseAddress!, ptr.count) }
            if n > 0 {
                receivedGuestData = true
                var off2 = 0
                while off2 < n {
                    let w = buf.withUnsafeBytes { ptr in write(STDOUT_FILENO, ptr.baseAddress!.advanced(by: off2), n-off2) }
                    if w > 0 { off2 += Int(w); continue }
                    if w < 0 && errno == EINTR { continue }
                    if w < 0 && (errno == EAGAIN || errno == EWOULDBLOCK) { usleep(1000); continue }
                    break
                }
            } else if n == 0 {
                if !receivedGuestData { connectionFailed = true }
                break
            } else {
                if errno != EAGAIN && errno != EWOULDBLOCK && errno != EINTR {
                    connectionFailed = true
                    break
                }
            }
        }
    }
    if connectionFailed {
        cliError("Guest management connection closed before shell started")
        return 1
    }
    return 0
}
