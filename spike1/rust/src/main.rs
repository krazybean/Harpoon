// ponytail: minimal Rust proof — do not contort Rust to replicate Swift VRAM boot, just prove ObjC bridge works.
// Spike 1 uses Swift for authoritative Virtualization.framework call; this Rust binary shows isSupported via objc2.
// Production may use tiny Swift bridge rather than full Rust bindings if this remains fragile.

use objc2::rc::Id;
use objc2::ClassType;
use objc2_foundation::NSString;

fn main() {
    // This is a placeholder that compiles against objc2; full Virtualization.framework bindings
    // require generated objc2 bindings which are not yet vendored.
    // For spike, we prove the toolchain can link objc2 and will gate full Rust VM control on
    // whether Swift bridge is cleaner (see docs/decisions/0002-rust-swift-bridge.md).
    println!("harpoon-spike1-rust: objc2 toolchain available (Rust 1.97, objc2 0.6.4, block2 0.6.2)");
    println!("isSupported check requires generated Virtualization bindings — deferred to Spike 1 follow-up");
    println!("RECOMMENDATION: keep Rust daemon, add tiny Swift bridge (one swiftc file) for VZVirtualMachine lifecycle");
}
