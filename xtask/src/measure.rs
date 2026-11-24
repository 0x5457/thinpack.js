//! Measurement helpers shared by the size commands: the transfer size of an artifact and
//! the unit its report prints.

use std::path::Path;
use std::process::Command;

/// The artifact's transfer size: `gzip -9` of the same bytes, not a format the runtime reads.
pub(crate) fn gzip_size(binary: &Path) -> Result<u64, String> {
    let output = Command::new("gzip")
        .args(["-9", "-n", "-c"])
        .arg(binary)
        .output()
        .map_err(|error| format!("gzip: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "gzip failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout.len() as u64)
}

#[expect(
    clippy::cast_precision_loss,
    reason = "measured artifacts stay far below 2^53 bytes, where `u64` → `f64` is exact"
)]
pub(crate) fn mib(bytes: u64) -> f64 {
    bytes as f64 / 1_048_576.0
}
