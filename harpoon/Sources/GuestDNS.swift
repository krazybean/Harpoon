import Foundation

// Guest DNS assessment for `harpoon doctor`. Pure functions: the doctor gathers the guest's resolv.conf and asks each configured
// resolver one question about "localhost", then these decide. Nothing here names a public resolver or a public host, so the verdict
// never depends on the Internet being up: a resolver that is itself unreachable is reported as such, whoever runs it.

enum ResolverProbe: Equatable { case answers, unreachable }

/// Nameserver addresses in file order, duplicates removed. Only address literals are accepted (a stray word is not a resolver).
func guestNameservers(_ resolvConf: String) -> [String] {
    var out: [String] = []
    for raw in resolvConf.split(whereSeparator: \.isNewline) {
        let parts = raw.split(whereSeparator: { $0 == " " || $0 == "\t" })
        guard parts.count >= 2, parts[0] == "nameserver" else { continue }
        let ip = String(parts[1])
        guard ip.allSatisfy({ $0.isHexDigit || $0 == "." || $0 == ":"}), !out.contains(ip) else { continue }
        out.append(ip)
    }
    return out
}

/// Reads busybox `nslookup localhost <resolver>`: any DNS reply (an address or NXDOMAIN) proves the resolver is alive; a refused
/// connection, a timeout or no output at all proves it is not.
func classifyResolverProbe(exit: Int32, stdout: String, stderr: String) -> ResolverProbe {
    let text = (stdout + "\n" + stderr).lowercased()
    for dead in ["connection refused", "no servers could be reached", "timed out", "network is unreachable", "guest management service is not ready", "invalid management response"] where text.contains(dead) {
        return .unreachable
    }
    return text.contains("server:") ? .answers : .unreachable
}

struct GuestDNSVerdict: Equatable {
    var ok: Bool
    var message: String
    var notes: [String]
}

/// `probes` maps each guest nameserver to what it did; `hostServers` is what the Mac uses now (the guest copies it at boot).
func assessGuestDNS(nameservers: [String], probes: [String: ResolverProbe], hostServers: [String]) -> GuestDNSVerdict {
    guard !nameservers.isEmpty else {
        return GuestDNSVerdict(ok: false, message: "FAIL — the guest's /etc/resolv.conf lists no nameserver; names cannot be resolved by the guest or the Docker daemon (pull, login)", notes: [])
    }
    let alive = nameservers.filter { probes[$0] == .answers }
    let dead = nameservers.filter { probes[$0] != .answers }
    if alive.isEmpty {
        return GuestDNSVerdict(ok: false,
            message: "FAIL — none of the guest's nameservers (\(dead.joined(separator: ", "))) answers; the guest and the Docker daemon cannot resolve names (pull, login). Containers use dockerd's own --dns and are not affected by this",
            notes: ["restart Harpoon to re-apply the Mac's resolvers (a guest that predates the DHCP-renewal fix loses them again within 30 minutes of boot)"])
    }
    var notes: [String] = []
    if !dead.isEmpty { notes.append("not answering: \(dead.joined(separator: ", "))") }
    let message = "PASS — \(alive.joined(separator: ", ")) answers"
    if !hostServers.isEmpty, Set(nameservers).isDisjoint(with: hostServers) {
        notes.append("differs from the Mac's current resolvers (\(hostServers.joined(separator: ", "))); the guest took its resolvers when it booted or from DHCP")
    }
    return GuestDNSVerdict(ok: true, message: message, notes: notes)
}
