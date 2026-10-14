//! The worktree's `origin` remote, read for the proxy's per-Task upstream.

use std::path::Path;
use std::time::Duration;

use moor_core::seam::proc::{Deadline, ProcSpec, Tool};

use crate::cx::Cx;

/// Log lines kept when a start fails.
pub(crate) const TAIL_LINES: usize = 20;

/// Allowance for reading the origin URL.
const ORIGIN_DEADLINE: Duration = Duration::from_secs(5);

/// The `origin` remote URL of the worktree, when it has one.
pub(crate) fn origin(cx: &Cx, worktree: &Path) -> Option<String> {
    let spec = ProcSpec::new(Tool::Git, Deadline(ORIGIN_DEADLINE))
        .args(["remote", "get-url", "origin"])
        .cwd(worktree);
    let out = cx.proc().run(&spec).ok()?;
    let url = out.stdout.trim();
    (!url.is_empty()).then(|| url.to_owned())
}
