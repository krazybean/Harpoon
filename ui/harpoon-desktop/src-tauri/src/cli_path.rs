//! Puts the bundled `docker` and `harpoon` commands on the Terminal PATH.
//! An installed Harpoon.app writes its bin folder to /etc/paths.d/harpoon once,
//! behind a single administrator prompt. path_helper appends paths.d entries
//! after /usr/local/bin and Homebrew, so a docker the user installed still wins.
//! A declined prompt is remembered and not shown again.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const PATHS_D_ENTRY: &str = "/etc/paths.d/harpoon";
const DECLINED_MARKER: &str = "cli-path-declined";

pub fn ensure_on_launch(app_data_dir: PathBuf) {
    if cfg!(debug_assertions) { return; }
    let Some(bin_dir) = installed_bin_dir() else { return; };
    let entry = format!("{}\n", bin_dir.display());
    if fs::read_to_string(PATHS_D_ENTRY).ok().as_deref() == Some(entry.as_str()) { return; }
    let declined = app_data_dir.join(DECLINED_MARKER);
    if declined.exists() { return; }
    std::thread::spawn(move || {
        if !install_paths_d_entry(&bin_dir) {
            let _ = fs::create_dir_all(&app_data_dir);
            let _ = fs::write(&declined, "");
        }
    });
}

/// The bundle's bin folder, only for an app installed under /Applications
/// (never a copy running from the mounted DMG or a build directory).
fn installed_bin_dir() -> Option<PathBuf> {
    let harpoon = fs::canonicalize(crate::bundled_harpoon_path()?).ok()?;
    let bin_dir = harpoon.parent()?.to_path_buf();
    let installed = bin_dir.starts_with("/Applications") && bin_dir.ends_with("Contents/Resources/harpoon/bin");
    (installed && bin_dir.join("docker").is_file()).then_some(bin_dir)
}

fn install_paths_d_entry(bin_dir: &Path) -> bool {
    Command::new("/usr/bin/osascript")
        .args(["-e", &paths_d_applescript(bin_dir)])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn paths_d_applescript(bin_dir: &Path) -> String {
    let shell = format!("printf '%s\\n' {} > {}", shell_quote(&bin_dir.display().to_string()), PATHS_D_ENTRY);
    let prompt = "Harpoon wants to add its docker and harpoon commands to your Terminal PATH.";
    format!("do shell script {} with administrator privileges with prompt {}", applescript_quote(&shell), applescript_quote(prompt))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn applescript_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applescript_writes_quoted_bin_dir() {
        let script = paths_d_applescript(Path::new("/Applications/Harpoon's \"Copy\".app/Contents/Resources/harpoon/bin"));
        assert_eq!(
            script,
            "do shell script \"printf '%s\\\\n' '/Applications/Harpoon'\\\\''s \\\"Copy\\\".app/Contents/Resources/harpoon/bin' > /etc/paths.d/harpoon\" with administrator privileges with prompt \"Harpoon wants to add its docker and harpoon commands to your Terminal PATH.\""
        );
    }
}
