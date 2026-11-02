// Compiled together with Sources/GuestDNS.swift by regression-guest-dns-doctor.sh.
import Foundation

var failures = 0
func expect(_ cond: Bool, _ what: String) { if !cond { failures += 1; print("FAIL \(what)") } }

// ---- resolv.conf parsing (real guest contents) ------------------------------------------------------------------------------
expect(guestNameservers("search lan\nnameserver 192.168.64.1\n") == ["192.168.64.1"], "dhcp resolv.conf")
expect(guestNameservers("nameserver 8.8.8.8\nnameserver 8.8.4.4\n") == ["8.8.8.8", "8.8.4.4"], "override resolv.conf")
expect(guestNameservers("nameserver 8.8.8.8\nnameserver 8.8.8.8\n# nameserver 1.1.1.1\nnameserver\nnameserver evil;rm\n") == ["8.8.8.8"], "duplicates, comments, junk")
expect(guestNameservers("nameserver 2001:4860:4860::8888\n") == ["2001:4860:4860::8888"], "ipv6")
expect(guestNameservers("").isEmpty, "empty file")

// ---- what the guest's busybox nslookup really printed ------------------------------------------------------------------------
let refused = classifyResolverProbe(exit: 1, stdout: ";; connection timed out; no servers could be reached\n", stderr: "nslookup: read: Connection refused\nnslookup: read: Connection refused\nGuest command exited with status 1\n")
let nxdomain = classifyResolverProbe(exit: 1, stdout: "Server:\t\t8.8.8.8\nAddress:\t8.8.8.8:53\n\n** server can't find localhost: NXDOMAIN\n\n** server can't find localhost: NXDOMAIN\n\n", stderr: "Guest command exited with status 1\n")
let address = classifyResolverProbe(exit: 0, stdout: "Server:\t\t192.168.64.53\nAddress:\t192.168.64.53:53\n\nName:\tlocalhost\nAddress: 127.0.0.1\n", stderr: "")
expect(refused == .unreachable, "refused connection is unreachable")
expect(nxdomain == .answers, "NXDOMAIN proves the resolver is alive (and needs no public name)")
expect(address == .answers, "an address answer")
expect(classifyResolverProbe(exit: 1, stdout: ";; connection timed out; no servers could be reached\n", stderr: "") == .unreachable, "timeout")
expect(classifyResolverProbe(exit: 1, stdout: "", stderr: "") == .unreachable, "no output")
expect(classifyResolverProbe(exit: 1, stdout: "", stderr: "invalid management response") == .unreachable, "no management answer")

// ---- the verdicts ---------------------------------------------------------------------------------------------------------------
// the defect: resolv.conf rewritten to the gateway that runs no DNS
let stale = assessGuestDNS(nameservers: ["192.168.64.1"], probes: ["192.168.64.1": refused], hostServers: ["8.8.8.8", "8.8.4.4"])
expect(!stale.ok && stale.message.contains("192.168.64.1") && stale.message.contains("Docker daemon") && stale.message.contains("not affected"), "stale gateway DNS is a failure that names the resolver and what is affected")
expect(stale.notes.joined().contains("restart Harpoon"), "and says how to recover")
// healthy: the Mac's resolvers held in the guest, answering
let held = assessGuestDNS(nameservers: ["8.8.8.8", "8.8.4.4"], probes: ["8.8.8.8": nxdomain, "8.8.4.4": nxdomain], hostServers: ["8.8.8.8", "8.8.4.4"])
expect(held.ok && held.notes.isEmpty, "held host DNS passes quietly")
// healthy: DHCP DNS works and differs from the Mac's: informational only
let dhcp = assessGuestDNS(nameservers: ["192.168.64.53"], probes: ["192.168.64.53": address], hostServers: ["8.8.8.8"])
expect(dhcp.ok && dhcp.notes.count == 1 && dhcp.notes[0].contains("differs from the Mac"), "working DHCP DNS passes with a note")
// one of two dead: still usable, the dead one is named
let partial = assessGuestDNS(nameservers: ["192.168.64.1", "8.8.8.8"], probes: ["192.168.64.1": refused, "8.8.8.8": nxdomain], hostServers: ["8.8.8.8"])
expect(partial.ok && partial.notes.contains("not answering: 192.168.64.1"), "a dead secondary is reported")
// nothing configured
expect(!assessGuestDNS(nameservers: [], probes: [:], hostServers: ["8.8.8.8"]).ok, "empty resolv.conf fails")
// the Mac offline must not turn a working local resolver into a failure: a LAN resolver that answers NXDOMAIN passes without any public lookup
expect(assessGuestDNS(nameservers: ["192.168.1.1"], probes: ["192.168.1.1": nxdomain], hostServers: []).ok, "no dependency on public DNS or on the host's list")

if failures == 0 { print("guest dns doctor regression: PASS") } else { print("guest dns doctor regression: \(failures) failure(s)"); exit(1) }
