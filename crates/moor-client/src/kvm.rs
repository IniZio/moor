//! Every Box gets KVM, so a host whose Boxes cannot open the KVM device is refused.

use std::os::unix::fs::MetadataExt as _;
use std::path::Path;

use crate::rootless::Failure;

/// The host's KVM device.
pub const DEVICE: &str = "/dev/kvm";
/// Names another device for the checks to read, as tests do; Boxes always get [`DEVICE`].
pub const DEVICE_VAR: &str = "MOOR_KVM_DEVICE";
/// The runbook section that sets KVM up.
const RUNBOOK: &str = "https://github.com/IniZio/moor/blob/develop/docs/rootless-docker.md#kvm";

/// The device the checks read: `set`, the value of [`DEVICE_VAR`], else [`DEVICE`].
#[must_use]
pub fn device(set: Option<&str>) -> &str {
    set.filter(|d| !d.is_empty()).unwrap_or(DEVICE)
}

/// The permission bits of `device`, when it exists.
#[must_use]
pub fn mode_of(device: &str) -> Option<u32> {
    std::fs::metadata(Path::new(device))
        .ok()
        .map(|m| m.mode() & 0o777)
}

/// Why Boxes cannot open `device`, with permission bits `mode`; none when they can.
///
/// Rootless Docker drops the device's group, so only bits open to everyone count there. Rootful
/// Boxes join its group. `mode` is none when the device is missing.
#[must_use]
pub fn refusal(rootless: bool, device: &str, mode: Option<u32>) -> Option<Failure> {
    let udev = format!(
        "add a udev rule with MODE=\"0666\" for KERNEL==\"kvm\", such as in \
         /etc/udev/rules.d/65-kvm.rules; see {RUNBOOK}"
    );
    let Some(mode) = mode else {
        return Some(Failure {
            check: "kvm",
            detail: format!("{device} is missing, and every Box gets KVM"),
            fix: format!(
                "turn on virtualization in the host's firmware and load kvm_intel or kvm_amd; see {RUNBOOK}"
            ),
        });
    };
    let open = |bits: u32| mode & bits == bits;
    let detail = if rootless && !open(0o006) {
        format!(
            "{device} is mode {mode:04o}, and rootless Docker drops its group, so Boxes cannot open it"
        )
    } else if !rootless && !open(0o060) && !open(0o006) {
        format!("{device} is mode {mode:04o}, so Boxes cannot open it even in its group")
    } else {
        return None;
    };
    Some(Failure {
        check: "kvm",
        detail,
        fix: udev,
    })
}

#[cfg(test)]
mod tests;
