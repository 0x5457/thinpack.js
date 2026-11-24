//! Cargo build locations and fingerprints shared by the xtask commands.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

/// Cargo's target directory for this workspace. `CARGO_TARGET_DIR` wins because it is
/// the documented way to build without the checked-in `target` symlink.
pub(crate) fn target_root() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map_or_else(|| crate::repo_root().join("target"), PathBuf::from)
}

pub(crate) fn host_triple() -> Result<String, String> {
    let output = Command::new("rustc")
        .arg("-vV")
        .output()
        .map_err(|error| format!("rustc: {error}"))?;
    if !output.status.success() {
        return Err(format!("rustc -vV failed ({})", output.status));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(str::to_owned)
        .ok_or_else(|| "rustc -vV printed no host line".to_owned())
}

pub(crate) fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let digest = Sha256::digest(&bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    Ok(hex)
}
