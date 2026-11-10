//! The user slice every Box VM runs under, so the kernel holds the Boxes to the host's memory
//! less the reserve while the agent, outside it, keeps reporting.

use crate::service::Manager;

/// The slice's unit name, which the provider is told to put each VM's scope in. It has no hyphen:
/// systemd would nest `a-b.slice` under `a.slice`, where `CPUWeight` has no sibling to weigh against.
pub const SLICE: &str = "moorboxes.slice";

/// The slice unit for a host of `total_mib` that keeps `reserve_mib` for itself.
#[must_use]
pub fn unit(total_mib: u64, reserve_mib: u64) -> String {
    let high = total_mib.saturating_sub(reserve_mib).max(1);
    let max = total_mib.saturating_sub(reserve_mib / 2).max(1);
    format!(
        "[Unit]
Description=moor Boxes: the VMs on this machine, held below its memory less the reserve

[Slice]
MemoryHigh={high}M
MemoryMax={max}M
CPUWeight=50
"
    )
}

/// Writes the slice into `manager`'s unit directory when it differs, so the reload that follows
/// picks it up; whether the file changed. A launchd host has no slices.
///
/// # Errors
/// The file cannot be written.
pub fn install(manager: &Manager, total_mib: u64, reserve_mib: u64) -> Result<bool, String> {
    let Manager::Systemd { dir } = manager else {
        return Ok(false);
    };
    let (file, text) = (dir.join(SLICE), unit(total_mib, reserve_mib));
    if std::fs::read_to_string(&file).ok().as_deref() == Some(text.as_str()) {
        return Ok(false);
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    std::fs::write(&file, text).map_err(|e| format!("cannot write {}: {e}", file.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests;
