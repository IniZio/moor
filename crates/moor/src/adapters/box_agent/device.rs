//! Per-Task devices: simulators, emulators or hardware an agent builds and tests against.
//!
//! Each [`DeviceKind`] is independent. A Task may hold several devices at once (an iOS
//! simulator and an Android emulator on one Mac), and a kind that fails to provision is
//! skipped without stopping the others.
//!
//! Kinds that fit this contract later:
//! - An Android emulator on a Mac: a per-Task AVD named `moor-<task>`, created and booted
//!   outside the sandbox. Its identity reaches the agent as `ANDROID_SERIAL` through
//!   [`DeviceKind::env`]. Grants cover the AVD directory under `~/.android/avd` and the SDK.
//!   The console and adb ports come from a per-Task port lease. The AVD is deleted when the
//!   agent exits, when the launch fails, and at release.
//! - An Android emulator on a Linux Box that has KVM, and Android builds on Linux with no
//!   device. The second is a kind whose `provision` yields an identity that carries only
//!   grants and env, and whose `teardown` is `None`.
//! - Physical devices are not built, but nothing assumes moor creates every device:
//!   `teardown` may be `None`.

use moor_core::name::BoxPath;
use moor_core::seam::driver::Driver;
use moor_core::seam::engine::LaunchReq;

pub(crate) mod ios;
#[cfg(test)]
mod tests;

/// Proof files go here, relative to the workdir; `.moor/` is already git-excluded.
pub(crate) const PROOF_DIR: &str = ".moor/proof";

/// One kind of device a Task can hold.
pub(crate) trait DeviceKind: Sync {
    /// Whether a Box whose `uname -s` is `os` can host this kind.
    fn available(&self, os: &str) -> bool;
    /// Creates and starts the device outside the sandbox; returns its identity, `None` on any failure.
    fn provision(&self, driver: &dyn Driver, req: &LaunchReq, cred: &BoxPath) -> Option<String>;
    /// Variables exported to the agent for device `id`; empty when `id` is invalid.
    fn env(&self, id: &str) -> Vec<(String, String)>;
    /// Absolute paths granted read-write to the sandboxed agent; `login_home` is the Box login user's home.
    fn grants(&self, id: &str, login_home: &str) -> Vec<String>;
    /// The brief paragraph naming the device (no trailing space/newline).
    fn brief(&self, id: &str) -> String;
    /// The brief sentences telling the agent how to capture proof into [`PROOF_DIR`] (no trailing space/newline).
    fn proof(&self, id: &str) -> String;
    /// A `sh` line deleting device `id`; `None` when `id` is invalid.
    fn teardown(&self, id: &str) -> Option<String>;
    /// A `sh` line run at release that deletes this kind's device for `task` (ASCII digits) by exact name, never failing the cleanup.
    fn release_line(&self, task: &str) -> String;
}

/// One provisioned device of a Task.
#[derive(Clone)]
pub(crate) struct Device {
    /// The kind that created it.
    pub(crate) kind: &'static dyn DeviceKind,
    /// The identity [`DeviceKind::provision`] returned.
    pub(crate) id: String,
}

/// The device kinds moor provisions for every Task, in order.
pub(crate) const ENABLED: &[&'static dyn DeviceKind] = &[&ios::Ios];

/// Provisions every kind in `kinds` that `os` can host, in order; a kind that fails is skipped
/// (the Task runs without it) and does not stop the others. `os == None` provisions nothing.
pub(crate) fn provision(
    kinds: &[&'static dyn DeviceKind],
    driver: &dyn Driver,
    req: &LaunchReq,
    cred: &BoxPath,
    os: Option<&str>,
) -> Vec<Device> {
    let Some(os) = os else {
        return Vec::new();
    };
    kinds
        .iter()
        .filter(|kind| kind.available(os))
        .filter_map(|&kind| {
            let id = kind.provision(driver, req, cred)?;
            Some(Device { kind, id })
        })
        .collect()
}
