use std::path::Path;
use std::process::Command;

fn main() {
  // ponytail: prepare Harpoon runtime bundle-resources before tauri build (avoids beforeBuildCommand cwd fragility)
  let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
  let repo_root = manifest_dir.join("../../..");
  let bundle_res = manifest_dir.join("bundle-resources/harpoon");
  let bin_dir = bundle_res.join("bin");
  let lib_dir = bundle_res.join("lib/harpoon");
  let harpoon_bin = repo_root.join("harpoon/build/harpoon");
  let kernel = repo_root.join("spike1/cache/Image-virt");
  let initramfs = repo_root.join("harpoon/cache/harpoon-m4-initramfs.cpio.gz");
  let rootimg = repo_root.join("spike2/cache/harpoon-root.img");

  // ensure harpoon built
  if !harpoon_bin.exists() {
    let _ = Command::new("bash").arg(repo_root.join("harpoon/build.sh")).status();
  }

  if harpoon_bin.exists() && kernel.exists() && initramfs.exists() && rootimg.exists() {
    let _ = std::fs::create_dir_all(&bin_dir);
    let _ = std::fs::create_dir_all(&lib_dir);
    let _ = std::fs::copy(&harpoon_bin, bin_dir.join("harpoon"));
    let _ = std::fs::copy(&kernel, lib_dir.join("Image-virt"));
    let _ = std::fs::copy(&initramfs, lib_dir.join("harpoon-initramfs.cpio.gz"));
    // clone-aware for sparse root: try cp -c via Command, fallback to copy
    let dest = lib_dir.join("harpoon-root.img");
    let cp_status = Command::new("cp").args(["-c", rootimg.to_str().unwrap(), dest.to_str().unwrap()]).status();
    if cp_status.map(|s| !s.success()).unwrap_or(true) {
      let ditto_status = Command::new("ditto").args([rootimg.to_str().unwrap(), dest.to_str().unwrap()]).status();
      if ditto_status.map(|s| !s.success()).unwrap_or(true) {
        let _ = std::fs::copy(&rootimg, &dest);
      }
    }
    // ensure executable
    let _ = Command::new("chmod").args(["+x", bin_dir.join("harpoon").to_str().unwrap()]).status();
  }

  tauri_build::build()
}
