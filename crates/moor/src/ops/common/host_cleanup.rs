//! Removes a Task's leftovers from a static host that outlives its Box.

use std::path::Path;
use std::time::Duration;

use moor_core::domain::run::SyncMode;
use moor_core::seam::driver::{Driver, ExecOutcome, ExecSpec};
use moor_core::seam::proc::Deadline;
use moor_core::{BoxHandle, Warning};

use crate::adapters::box_agent::device::{self, DeviceKind};
use crate::cx::Cx;
use crate::store::{BoxRow, TaskRow};

/// Wall-clock allowance for the cleanup script.
const DEADLINE: Duration = Duration::from_secs(60);

/// A name safe to splice into a path: non-empty, `[A-Za-z0-9._-]`, not `.` or `..`.
fn safe_name(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// A Task id safe to splice into a glob: non-empty ASCII digits only.
///
/// A hyphen or dot would let one Task's glob (`1-*`) match another's (`1-5-…`).
fn safe_task_id(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Single-quotes `s` for `sh`.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The Box name inside a driver handle (`<provider>/<name>`).
pub(crate) fn box_name(handle: &BoxHandle) -> &str {
    handle
        .as_str()
        .split_once('/')
        .map_or(handle.as_str(), |(_, n)| n)
}

/// The lease directory: the parent of `workdir`, when its name is `box_name`.
///
/// `box_name` must be `moor-` plus a suffix, so a handle such as `ssh/newman`
/// can never point the removal at a user's home. Anything else is left alone, so a recorded path that does not match this
/// Task's lease is never removed.
fn lease_dir<'a>(workdir: &'a str, box_name: &str) -> Option<&'a str> {
    let (lease, repo) = workdir.rsplit_once('/')?;
    let (root, name) = lease.rsplit_once('/')?;
    let owned = box_name
        .strip_prefix("moor-")
        .is_some_and(|rest| !rest.is_empty());
    let plain = |p: &str| !matches!(p, "" | "." | "..");
    let dirs_ok = workdir.strip_prefix('/')?.split('/').all(plain);
    (owned && dirs_ok && plain(repo) && !root.is_empty() && name == box_name).then_some(lease)
}

/// The `sh -c` script that removes the Task's credential and temp directories
/// and per-Task home and, when `workdir` sits in the Box's own lease directory, that directory.
/// It also runs each enabled device kind's release line, which deletes the Task's
/// device by exact name (the iOS simulator, on a host with `xcrun`, named `moor-<task>`).
///
/// Returns `None` when `task` is not all ASCII digits, `box_name` is not a plain name or `workdir` is
/// not an absolute path, because the paths are built from them.
pub(crate) fn script(task: &str, box_name: &str, workdir: &str) -> Option<String> {
    script_with(task, box_name, workdir, device::ENABLED)
}

/// [`script`] for an explicit set of device kinds.
pub(crate) fn script_with(
    task: &str,
    box_name: &str,
    workdir: &str,
    kinds: &[&'static dyn DeviceKind],
) -> Option<String> {
    if !safe_task_id(task) || !safe_name(box_name) || !workdir.starts_with('/') {
        return None;
    }
    let mut lines = vec![
        r#"[ -n "$HOME" ] && [ "$HOME" != / ] || exit 1"#.to_owned(),
        format!(r#"rm -rf -- "$HOME"/.moor-cred-{task}-* "$HOME"/.moor-tmp-{task}-*"#),
    ];
    // The agent's per-Task HOME sits beside the workdir, so it goes even when
    // the lease directory below is kept.
    let home = quote(&format!("{workdir}.moor-home"));
    lines.push(format!(
        r#"case "$HOME/" in {home}/*) ;; *) rm -rf -- {home} ;; esac"#
    ));
    if let Some(lease) = lease_dir(workdir, box_name) {
        let lease = quote(lease);
        // Never remove `$HOME` or a directory that contains it.
        lines.push(format!(
            r#"case "$HOME/" in {lease}/*) ;; *) rm -rf -- {lease} ;; esac"#
        ));
    }
    lines.extend(kinds.iter().map(|k| k.release_line(task)));
    Some(lines.join("\n"))
}

/// Runs the cleanup on the host behind `handle`.
///
/// # Errors
/// Returns why it did not finish: unsafe names, a failed exec, or a non-zero exit.
pub(crate) fn run(
    driver: &dyn Driver,
    handle: &BoxHandle,
    task: &str,
    workdir: &str,
    host_cwd: Option<&Path>,
) -> Result<(), String> {
    let Some(script) = script(task, box_name(handle), workdir) else {
        return Err("the task or Box name is not a plain name".to_owned());
    };
    let spec = ExecSpec {
        argv: vec!["sh".to_owned(), "-c".to_owned(), script],
        env: Vec::new(),
        cwd: None,
        sync: SyncMode::None,
        output: None,
        host_cwd: host_cwd.map(Path::to_path_buf),
        class: None,
    };
    match driver.exec(handle, &spec, Deadline(DEADLINE)) {
        Ok(ExecOutcome::Exited { code: 0 }) => Ok(()),
        Ok(ExecOutcome::Exited { code }) => Err(format!("cleanup exited {code}")),
        Ok(ExecOutcome::Interrupted) => Err("cleanup was interrupted".to_owned()),
        Err(e) => Err(e.to_string()),
    }
}

/// Cleans a static host before its Box is released, because the host keeps the
/// Task's directories otherwise; a failure warns and the release goes ahead.
pub(crate) fn on_release(cx: &Cx, row: &TaskRow, b: &BoxRow, handle: &BoxHandle) {
    if !cx.driver().caps(&b.provider).is_ok_and(|c| c.static_host) {
        return;
    }
    let reason = match b.workdir.as_deref() {
        Some(workdir) => {
            let host_cwd = row.worktree.as_ref().map(Path::new);
            let task = row.id.get().to_string();
            match run(cx.driver(), handle, &task, workdir, host_cwd) {
                Ok(()) => return,
                Err(e) => e,
            }
        }
        None => "the Box workdir is unknown".to_owned(),
    };
    cx.warn(Warning {
        code: "host_cleanup_failed".to_owned(),
        message: format!(
            "host cleanup did not finish: {reason}; the Task's lease directory and \
             `.moor-cred-*` / `.moor-tmp-*` directories may remain on the host"
        ),
    });
}

#[cfg(test)]
mod tests;
