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
use moor_core::seam::engine::{DeviceFailure, LaunchReq};

pub(crate) mod ios;
#[cfg(test)]
mod tests;

/// Proof files go here, relative to the workdir; `.moor/` is already git-excluded.
pub(crate) const PROOF_DIR: &str = ".moor/proof";

/// One kind of device a Task can hold.
pub(crate) trait DeviceKind: Sync {
    /// Short name used in failure reports, such as `ios`.
    fn name(&self) -> &'static str {
        "device"
    }
    /// Whether a Box whose `uname -s` is `os` can host this kind.
    fn available(&self, os: &str) -> bool;
    /// Creates and starts the device outside the sandbox; returns its identity, or a short reason it failed.
    ///
    /// # Errors
    /// Returns a short human reason when the device could not be created.
    fn provision(
        &self,
        driver: &dyn Driver,
        req: &LaunchReq,
        cred: &BoxPath,
    ) -> Result<String, String>;
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
/// (the Task runs without it) and does not stop the others. A kind that `os` cannot host is
/// not a failure. `os == None` provisions nothing. Returns the devices and the failures.
pub(crate) fn provision(
    kinds: &[&'static dyn DeviceKind],
    driver: &dyn Driver,
    req: &LaunchReq,
    cred: &BoxPath,
    os: Option<&str>,
) -> (Vec<Device>, Vec<DeviceFailure>) {
    let mut devices = Vec::new();
    let mut failures = Vec::new();
    let Some(os) = os else {
        return (devices, failures);
    };
    for &kind in kinds.iter().filter(|kind| kind.available(os)) {
        match kind.provision(driver, req, cred) {
            Ok(id) => devices.push(Device { kind, id }),
            Err(reason) => failures.push(DeviceFailure {
                kind: kind.name().to_owned(),
                reason,
            }),
        }
    }
    (devices, failures)
}
