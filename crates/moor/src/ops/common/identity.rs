//! Gives a Box the user's git author name and email.

use std::fmt::Write;
use std::path::Path;

use moor_core::boxscript::quote;
use moor_core::Warning;

use crate::cx::Cx;

/// The global git `user.name` and `user.email` a Box gets from the author of the host worktree.
///
/// Only those two values leave the host. The keys are set inside the clone exec, so
/// setting them costs no round trip of its own.
pub(crate) struct Identity {
    /// Each key with its value, `user.name` first.
    keys: Vec<(&'static str, String)>,
}

impl Identity {
    /// The author of `worktree`; `None` when it has neither a name nor an email.
    pub(crate) fn of(cx: &Cx, worktree: &Path) -> Option<Self> {
        let author = cx.git().author(worktree).ok()?;
        let keys: Vec<_> = [("user.name", author.name), ("user.email", author.email)]
            .into_iter()
            .filter_map(|(key, value)| value.map(|v| (key, v)))
            .collect();
        (!keys.is_empty()).then_some(Self { keys })
    }

    /// Shell that sets every key whatever the earlier ones did, printing each exit as `key=code`.
    ///
    /// It never exits, so the command after it decides the exec's exit code.
    pub(crate) fn script(&self) -> String {
        let mut out = String::new();
        for (key, value) in &self.keys {
            let _ = write!(
                out,
                "git config --global {key} {}; echo {key}=$?; ",
                quote(value)
            );
        }
        out
    }

    /// Warns for each key that `output` of [`Self::script`] reports as failed.
    ///
    /// A key with no line is taken as set: a driver that does not return output still
    /// reports a failing exec through its exit code.
    pub(crate) fn report(&self, cx: &Cx, output: &str) {
        for (key, _) in &self.keys {
            let code = output.lines().find_map(|line| {
                let (k, v) = line.trim_end().split_once('=')?;
                (k == *key).then(|| v.parse::<i32>().ok())?
            });
            if let Some(code) = code.filter(|c| *c != 0) {
                cx.warn(Warning {
                    code: "identity_failed".to_owned(),
                    message: format!("could not set git {key} in the box: exit {code}"),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests;
