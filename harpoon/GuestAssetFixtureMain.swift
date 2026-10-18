import Foundation

struct HarpoonUserConfig: Codable {
    var cpus: Int?
    var memory: Int?
    var diskSize: String?
}

@main
struct GuestAssetFixtureMain {
    static func main() {
        let args = CommandLine.arguments
        guard args.count == 3 else { exit(2) }
        let error = RuntimeConfig.guestAssetValidationError(
            kernelURL: URL(fileURLWithPath: args[1]),
            initramfsURL: URL(fileURLWithPath: args[2]))
        if let error { print(error); exit(1) }
        print("GUEST_ASSET_MATCH")
    }
}
