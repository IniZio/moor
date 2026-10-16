//! Gives a Box the user's git author name and email.

use std::fmt::Write;
use std::path::Path;
use std::time::Duration;

use moor_core::boxscript::quote;
use moor_core::seam::driver::{BoxStep, ExecOutcome, StepDone};
use moor_core::seam::proc::Deadline;
use moor_core::{BoxHandle, Warning};

use crate::cx::Cx;

/// Wall-clock allowance for one `git config` call.
pub(crate) const IDENTITY_DEADLINE: Duration = Duration::from_secs(30);

/// Sets the Box's global git `user.name` and `user.email` from the author of `worktree`.
///
/// Only those two values leave the host. A missing author or a failing call
/// is a warning at most, never an error. Both keys go in one batch so the
/// Box is reached once.
pub(crate) fn set_identity(cx: &Cx, handle: &BoxHandle, worktree: &Path) {
    let Ok(author) = cx.git().author(worktree) else {
        return;
    };
    let keys: Vec<(&str, String)> = [("user.name", author.name), ("user.email", author.email)]
        .into_iter()
        .filter_map(|(key, value)| value.map(|v| (key, v)))
        .collect();
    if keys.is_empty() {
        return;
    }
    let step = BoxStep::Run(script(&keys));
    let count = u32::try_from(keys.len()).unwrap_or(1);
    let deadline = Deadline(IDENTITY_DEADLINE * count);
    let done = cx
        .driver()
        .batch(handle, &[], &[step], worktree, deadline)
        .into_iter()
        .next();
    for (key, _) in &keys {
        let reason = match &done {
            Some(Ok(StepDone::Ran { outcome, stdout })) => match key_code(*outcome, stdout, key) {
                Ok(0) => continue,
                Ok(code) => format!("exit {code}"),
                Err(reason) => reason,
            },
            Some(Ok(_)) => "unexpected batch result".to_owned(),
            Some(Err(e)) => e.to_string(),
            None => "no batch result".to_owned(),
        };
        cx.warn(Warning {
            code: "identity_failed".to_owned(),
            message: format!("could not set git {key} in the box: {reason}"),
        });
    }
}

/// One script that attempts every key whatever the earlier ones did.
///
/// A failing step would stop the batch, so each exit code goes to stdout as
/// `key=code` and the script exits with the first non-zero one.
fn script(keys: &[(&str, String)]) -> String {
    let mut out = String::new();
    for (i, (key, value)) in keys.iter().enumerate() {
        let _ = write!(
            out,
            "git config --global {key} {}; c{i}=$?; echo {key}=$c{i}; ",
            quote(value)
        );
    }
    for i in 0..keys.len() {
        let _ = write!(out, "[ $c{i} -eq 0 ] || exit $c{i}; ");
    }
    out.push_str("exit 0");
    out
}

/// The exit code of `key`, read from stdout when present and else from the script's own exit.
///
/// A driver that does not return stdout still reports the script's exit code.
fn key_code(outcome: ExecOutcome, stdout: &str, key: &str) -> Result<i32, String> {
    let ExecOutcome::Exited { code } = outcome else {
        return Err("interrupted".to_owned());
    };
    let reported = stdout.lines().find_map(|line| {
        let (k, v) = line.split_once('=')?;
        (k == key).then(|| v.trim().parse::<i32>().ok())?
    });
    Ok(reported.unwrap_or(code))
}

#[cfg(test)]
mod tests;
