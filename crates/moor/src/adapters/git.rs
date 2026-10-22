//! Git over `Proc`: the two reads the CLI makes in its caller's checkout.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_core::seam::checkout::Checkout;
use moor_core::seam::proc::{Deadline, Proc, ProcSpec, Tool};

/// Time limit for each git call.
const DEADLINE: Deadline = Deadline(Duration::from_secs(30));

/// Concrete git client; real git runs through the `Proc` seam.
pub(crate) struct Git {
    /// Spawner for every git call.
    proc: Arc<dyn Proc>,
}

impl Git {
    /// Builds a client over `proc`.
    pub(crate) fn new(proc: Arc<dyn Proc>) -> Self {
        Self { proc }
    }

    /// Runs git in `dir` and returns stdout; `None` when git fails.
    fn run(&self, dir: &Path, args: &[&str]) -> Option<String> {
        let spec = ProcSpec::new(Tool::Git, DEADLINE)
            .args(args.iter().copied())
            .cwd(dir);
        self.proc.run(&spec).ok().map(|out| out.stdout)
    }
}

impl Checkout for Git {
    fn origin_url(&self, dir: &Path) -> Option<String> {
        let out = self.run(dir, &["remote", "get-url", "origin"])?;
        let url = out.trim();
        (!url.is_empty()).then(|| url.to_owned())
    }

    /// A branch with no upstream counts as unpushed; a failed read counts as none.
    fn local_changes(&self, dir: &Path) -> bool {
        let Some(out) = self.run(dir, &["status", "--porcelain=v1", "--branch"]) else {
            return false;
        };
        let mut lines = out.lines();
        let unpushed = lines
            .next()
            .is_some_and(|branch| !branch.contains("...") || branch.contains("[ahead "));
        unpushed || lines.any(|l| !l.trim().is_empty())
    }
}

#[cfg(test)]
mod tests;
