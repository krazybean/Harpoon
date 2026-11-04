import Foundation

/// Where the host port forwarder dials the guest, published in `runtime.json` so that a client can check a deployment's
/// health through that path. It is published ONLY when the address was discovered from this VM's own serial log
/// (the serial log is recreated at every boot, so it cannot be a previous boot's line); a fallback address is never
/// published. The address alone proves nothing about which machine answers there: a client must still prove, over the
/// engine's vsock-bound management channel, that the machine at this address is the VM the engine owns.
struct ForwardTarget: Codable, Equatable {
    let address: String
    /// Always "discovered". Any other value (or no value) means the address is unproven and clients must refuse it.
    let source: String
    let recordedAt: String
}

/// An address the forwarder may dial and clients may rely on: a dotted IPv4 quad in an RFC 1918 range. Not loopback,
/// link-local, multicast, broadcast or unspecified, and no leading zeros (so two spellings cannot name one address).
func isUsableForwardAddress(_ s: String) -> Bool {
    let parts = s.split(separator: ".", omittingEmptySubsequences: false)
    guard parts.count == 4 else { return false }
    var o: [Int] = []
    for p in parts {
        guard !p.isEmpty, p.count <= 3, p.allSatisfy({ $0 >= "0" && $0 <= "9" }), let n = Int(p), n <= 255 else { return false }
        if p.count > 1 && p.hasPrefix("0") { return false }
        o.append(n)
    }
    if o[0] == 10 { return true }
    if o[0] == 172 && (16...31).contains(o[1]) { return true }
    if o[0] == 192 && o[1] == 168 { return true }
    return false
}
