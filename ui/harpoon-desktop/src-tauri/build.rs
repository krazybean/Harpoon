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
  let asset_manifest = repo_root.join("assets/guest/guest-assets.manifest");
  let docker_cli = repo_root.join("harpoon/build/docker");
  let docker_buildx = repo_root.join("harpoon/build/docker-buildx");
  let buildx_license_dir = repo_root.join("harpoon/third-party/docker-buildx");

  for asset in [&harpoon_bin, &docker_cli, &docker_buildx, &kernel, &initramfs, &rootimg, &asset_manifest] {
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

  // Docker's official CLI ships beside the runtime for Macs without their own.
  if !docker_cli.exists() {
    let _ = Command::new("sh")
      .arg(repo_root.join("harpoon/fetch-docker-cli.sh"))
      .status();
  }

  // Pinned upstream Docker Buildx (verified by fetch-buildx.sh). The only builder Harpoon-managed
  // builds may use, so release builds require it.
  if !docker_buildx.exists() {
    let _ = Command::new("sh")
      .arg(repo_root.join("harpoon/fetch-buildx.sh"))
      .status();
  }

  let required_assets = [&harpoon_bin, &docker_cli, &docker_buildx, &kernel, &initramfs, &rootimg, &asset_manifest];
  let missing_assets: Vec<_> = required_assets
    .iter()
    .filter(|asset| !asset.exists())
    .collect();

  if missing_assets.is_empty() {
    std::fs::create_dir_all(&bin_dir).expect("failed to create bundle bin directory");
    std::fs::create_dir_all(&lib_dir).expect("failed to create bundle lib directory");
    std::fs::copy(&harpoon_bin, bin_dir.join("harpoon"))
      .expect("failed to stage Harpoon runtime binary");
    std::fs::copy(&docker_cli, bin_dir.join("docker"))
      .expect("failed to stage bundled Docker CLI");
    std::fs::copy(&docker_buildx, bin_dir.join("docker-buildx"))
      .expect("failed to stage bundled Docker Buildx");
    let license_dir = bundle_res.join("share/licenses/docker-buildx");
    std::fs::create_dir_all(&license_dir).expect("failed to create Buildx license directory");
    for name in ["LICENSE", "NOTICE.md"] {
      std::fs::copy(buildx_license_dir.join(name), license_dir.join(name))
        .expect("failed to stage Docker Buildx license notice");
    }
    std::fs::copy(&kernel, lib_dir.join("Image-virt"))
      .expect("failed to stage guest kernel");
    std::fs::copy(&initramfs, lib_dir.join("harpoon-initramfs.cpio.gz"))
      .expect("failed to stage guest initramfs");
    std::fs::copy(&asset_manifest, lib_dir.join("guest-assets.manifest"))
      .expect("failed to stage guest asset manifest");

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
      .args(["+x", bin_dir.join("harpoon").to_str().unwrap(), bin_dir.join("docker").to_str().unwrap(), bin_dir.join("docker-buildx").to_str().unwrap()])
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

  // Universal bundles also ship the x86_64 guest, staged by prepare-bundle.sh.
  println!("cargo:rerun-if-env-changed=HARPOON_ARCH");
  if env::var("HARPOON_ARCH").as_deref() == Ok("universal") && env::var("PROFILE").as_deref() == Ok("release") {
    let x86_dir = lib_dir.join("x86_64");
    let missing = ["Image-virt", "harpoon-initramfs.cpio.gz", "guest-assets.manifest", "harpoon-root.img"]
      .iter()
      .filter(|name| !x86_dir.join(name).exists())
      .map(|name| x86_dir.join(name).display().to_string())
      .collect::<Vec<_>>();
    if !missing.is_empty() {
      panic!("universal release bundle is missing x86_64 guest assets: {}", missing.join(", "));
    }
  }

  tauri_build::build()
}
