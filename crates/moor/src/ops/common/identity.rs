//! Gives a Box the user's git author name and email.

use std::path::Path;
use std::time::Duration;

use moor_core::seam::driver::{ExecOutcome, ExecSpec, SyncMode};
use moor_core::seam::proc::Deadline;
use moor_core::{BoxHandle, Warning};

use crate::cx::Cx;

/// Wall-clock allowance for one `git config` call.
pub(crate) const IDENTITY_DEADLINE: Duration = Duration::from_secs(30);

/// Sets the Box's global git `user.name` and `user.email` from the author of `worktree`.
///
/// Only those two values leave the host. A missing author or a failing call
/// is a warning at most, never an error.
pub(crate) fn set_identity(cx: &Cx, handle: &BoxHandle, worktree: &Path) {
    let Ok(author) = cx.git().author(worktree) else {
        return;
    };
    for (key, value) in [("user.name", author.name), ("user.email", author.email)] {
        let Some(value) = value else { continue };
        let spec = ExecSpec {
            argv: ["git", "config", "--global", key, value.as_str()]
                .map(str::to_owned)
                .to_vec(),
            env: Vec::new(),
            cwd: None,
            sync: SyncMode::None,
            output: None,
            host_cwd: None,
            class: None,
        };
        let reason = match cx.driver().exec(handle, &spec, Deadline(IDENTITY_DEADLINE)) {
            Ok(ExecOutcome::Exited { code: 0 }) => continue,
            Ok(ExecOutcome::Exited { code }) => format!("exit {code}"),
            Ok(ExecOutcome::Interrupted) => "interrupted".to_owned(),
            Err(e) => e.to_string(),
        };
        cx.warn(Warning {
            code: "identity_failed".to_owned(),
            message: format!("could not set git {key} in the box: {reason}"),
        });
    }
}

#[cfg(test)]
mod tests;
