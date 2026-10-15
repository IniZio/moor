//! A per-Task iOS simulator device on a macOS Box.
//!
//! The device is named `moor-<task>` and is created and booted outside the
//! sandbox. Only names of exactly that shape are ever managed, so the owner's
//! own simulators are never booted, shut down or deleted.

use std::time::Duration;

use moor_core::boxscript::quote;
use moor_core::domain::run::SyncMode;
use moor_core::name::BoxPath;
use moor_core::seam::driver::{Driver, ExecOutcome, ExecSpec};
use moor_core::seam::engine::LaunchReq;
use moor_core::seam::proc::Deadline;

/// Proof files go here, relative to the workdir; `.moor/` is already git-excluded.
pub(super) const PROOF_DIR: &str = ".moor/proof";

/// Time allowed for creating and booting the device.
const CREATE_DEADLINE: Duration = Duration::from_secs(120);

/// File in the credential directory that receives the device UDID.
const UDID_FILE: &str = "simulator-udid";

/// The device name for `task`; `None` unless `task` is non-empty ASCII digits.
pub(super) fn device_name(task: &str) -> Option<String> {
    (!task.is_empty() && task.bytes().all(|b| b.is_ascii_digit())).then(|| format!("moor-{task}"))
}

/// A `sh` script that creates and boots the device and writes its UDID to `out`.
///
/// `simctl` lists iPhone device types newest first and runtimes oldest first, so
/// the script takes the last runtime and tries each iPhone type in list order
/// until `simctl create` accepts one; the newest type can be unsupported by
/// that runtime. It exits 1 when no type is accepted. Exits 0 without writing
/// when `xcrun` is absent. It names the device by UDID only, never by the
/// `booted` alias, which would reach the owner's devices.
pub(super) fn create_script(task: &str, out: &BoxPath) -> Option<String> {
    let name = quote(&device_name(task)?);
    let out = quote(out.as_str());
    Some(format!(
        "command -v xcrun >/dev/null 2>&1 || exit 0\n\
         r=$(xcrun simctl list runtimes available \
         | sed -n 's/^iOS .* - \\(com\\.apple\\.CoreSimulator\\.SimRuntime\\.iOS-[0-9-]*\\)[[:space:]]*$/\\1/p' \
         | tail -n 1)\n\
         [ -n \"$r\" ] || exit 1\n\
         u=\n\
         for t in $(xcrun simctl list devicetypes \
         | sed -n 's/^iPhone .* (\\(com\\.apple\\.CoreSimulator\\.SimDeviceType\\.iPhone[^)]*\\))[[:space:]]*$/\\1/p'); do\n\
         u=$(xcrun simctl create {name} \"$t\" \"$r\" 2>/dev/null) && break\n\
         u=\n\
         done\n\
         [ -n \"$u\" ] || exit 1\n\
         xcrun simctl boot \"$u\" || {{ xcrun simctl delete \"$u\"; exit 1; }}\n\
         printf '%s\\n' \"$u\" > {out}\n"
    ))
}

/// The UDID in `text`: one `8-4-4-4-12` uppercase-hex UUID, whitespace aside.
pub(super) fn parse_udid(text: &str) -> Option<String> {
    let t = text.trim();
    let groups = [8, 4, 4, 4, 12];
    let mut parts = t.split('-');
    for len in groups {
        let p = parts.next()?;
        let hex = |b: u8| b.is_ascii_digit() || (b'A'..=b'F').contains(&b);
        if p.len() != len || !p.bytes().all(hex) {
            return None;
        }
    }
    parts.next().is_none().then(|| t.to_owned())
}

/// The command that deletes the device `udid`; `None` unless `udid` is a valid UDID.
///
/// Never `booted` or `all`, which would reach the owner's devices.
pub(super) fn delete_line(udid: &str) -> Option<String> {
    let udid = parse_udid(udid)?;
    Some(format!("xcrun simctl delete {}", quote(&udid)))
}

/// Creates and boots the Task's device on the Box and returns its UDID.
///
/// Any failure returns `None`; the Task then runs without a simulator.
pub(super) fn create(driver: &dyn Driver, req: &LaunchReq, cred: &BoxPath) -> Option<String> {
    let out = BoxPath::under(cred.as_str(), UDID_FILE).ok()?;
    let script = create_script(req.task.as_str(), &out)?;
    let spec = ExecSpec {
        argv: vec!["sh".to_owned(), "-c".to_owned(), script],
        env: Vec::new(),
        cwd: None,
        sync: SyncMode::None,
        output: None,
        host_cwd: Some(req.worktree.clone()),
        class: None,
    };
    match driver.exec(&req.handle, &spec, Deadline(CREATE_DEADLINE)) {
        Ok(ExecOutcome::Exited { code: 0 }) => {}
        _ => return None,
    }
    let text = driver.peek_file(&req.handle, &out, &req.worktree).ok()??;
    parse_udid(&text)
}

#[cfg(test)]
mod tests;
