use std::env;
use std::path::Path;
use std::process::Command;

fn main() {
  // Prepare Harpoon runtime bundle-resources before Tauri builds.
  // Debug/check builds may run from a clean source checkout where the large guest
  // assets are intentionally absent; release builds must never silently ship an
  // incomplete bundle.
  let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
  let repo_root = manifest_dir.join("../../..");
  let bundle_res = manifest_dir.join("bundle-resources/harpoon");
  let bin_dir = bundle_res.join("bin");
  let lib_dir = bundle_res.join("lib/harpoon");
  let harpoon_bin = repo_root.join("harpoon/build/harpoon");
  let kernel = repo_root.join("assets/guest/Image-virt");
  let initramfs = repo_root.join("assets/guest/harpoon-initramfs.cpio.gz");
  let rootimg = repo_root.join("assets/guest/harpoon-root.img");

  for asset in [&harpoon_bin, &kernel, &initramfs, &rootimg] {
    println!("cargo:rerun-if-changed={}", asset.display());
  }

  // Tauri validates configured resource paths even during `cargo check`, so keep
  // the resource directory present on clean checkouts. Actual release contents
  // are still enforced below.
  std::fs::create_dir_all(&bundle_res)
    .expect("failed to create Tauri bundle resource directory");

  // Ensure the Harpoon runtime binary exists when possible.
  if !harpoon_bin.exists() {
    let _ = Command::new("bash")
      .arg(repo_root.join("harpoon/build.sh"))
      .status();
  }

  let required_assets = [&harpoon_bin, &kernel, &initramfs, &rootimg];
  let missing_assets: Vec<_> = required_assets
    .iter()
    .filter(|asset| !asset.exists())
    .collect();

  if missing_assets.is_empty() {
    std::fs::create_dir_all(&bin_dir).expect("failed to create bundle bin directory");
    std::fs::create_dir_all(&lib_dir).expect("failed to create bundle lib directory");
    std::fs::copy(&harpoon_bin, bin_dir.join("harpoon"))
      .expect("failed to stage Harpoon runtime binary");
    std::fs::copy(&kernel, lib_dir.join("Image-virt"))
      .expect("failed to stage guest kernel");
    std::fs::copy(&initramfs, lib_dir.join("harpoon-initramfs.cpio.gz"))
      .expect("failed to stage guest initramfs");

    // Clone-aware for sparse root: try cp -c, then ditto, then ordinary copy.
    let dest = lib_dir.join("harpoon-root.img");
    let cp_status = Command::new("cp")
      .args(["-c", rootimg.to_str().unwrap(), dest.to_str().unwrap()])
      .status();
    if cp_status.map(|s| !s.success()).unwrap_or(true) {
      let ditto_status = Command::new("ditto")
        .args([rootimg.to_str().unwrap(), dest.to_str().unwrap()])
        .status();
      if ditto_status.map(|s| !s.success()).unwrap_or(true) {
        std::fs::copy(&rootimg, &dest).expect("failed to stage guest root image");
      }
    }

    let _ = Command::new("chmod")
      .args(["+x", bin_dir.join("harpoon").to_str().unwrap()])
      .status();
  } else if env::var("PROFILE").as_deref() == Ok("release") {
    let missing = missing_assets
      .iter()
      .map(|asset| asset.display().to_string())
      .collect::<Vec<_>>()
      .join(", ");
    panic!("release bundle assets are missing: {missing}");
  } else {
    println!(
      "cargo:warning=guest bundle assets are not fully staged; continuing non-release build/check"
    );
  }

  tauri_build::build()
}
