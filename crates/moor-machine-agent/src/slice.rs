//! The user slice every Box VM runs under, so the kernel holds the Boxes to the host's memory
//! less the reserve while the agent, outside it, keeps reporting.

use std::path::Path;

use moor_core::decide::schedule::default_reserve_mib;

use crate::pool::host_mem_mib;
use crate::service::Manager;
use crate::settings::{self, Ensured};

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

/// Brings the Boxes slice and the provider's setting for it up to date: writes the unit, reloads
/// the user manager only when the unit changed, then tells the provider. `up` and `run` share it.
///
/// A launchd host, or one that cannot say its memory, has nothing to do.
///
/// # Errors
/// The unit cannot be written or the manager cannot reload.
pub fn ensure(
    manager: &Manager,
    home: &Path,
    provider: &str,
    reserve_mib: Option<u64>,
    say: &mut dyn FnMut(&str),
) -> Result<(), String> {
    let Some(total) = host_mem_mib() else {
        return Ok(());
    };
    let reserve = reserve_mib.unwrap_or_else(|| default_reserve_mib(total));
    if install(manager, total, reserve)? {
        manager.reload()?;
        say(&format!(
            "wrote {SLICE} with the Boxes' memory ceiling, {reserve} MiB less than the host"
        ));
    }
    tell_provider(manager, &settings::config_path(home), provider, say);
    Ok(())
}

/// Tells the provider to put VM scopes in [`SLICE`], saying what changed.
///
/// The setting goes in the crabbox config at `config`. Call it after [`install`], so the provider
/// never names a slice with no unit. A launchd host has no slice to name.
pub fn tell_provider(manager: &Manager, config: &Path, provider: &str, say: &mut dyn FnMut(&str)) {
    if !matches!(manager, Manager::Systemd { .. }) {
        return;
    }
    match settings::ensure_slice(config, provider, SLICE) {
        Ok(Ensured::Added) => say(&format!(
            "set cgroupSlice to {SLICE} in {}; change it there to use another slice",
            config.display()
        )),
        Ok(Ensured::NoCeiling) => say(&format!(
            "cgroup is off in {}, so Boxes on this machine run with no memory ceiling; remove it to turn the ceiling on",
            config.display()
        )),
        Ok(Ensured::Kept) => {}
        Err(e) => say(&format!("cannot set the Boxes slice: {e}")),
    }
}

#[cfg(test)]
mod tests;
