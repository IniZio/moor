//! Git over `Proc`: repo root and the facts the close gate needs.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moor_core::seam::proc::{Deadline, Proc, ProcError, ProcSpec, Tool};

/// Time limit for each git call.
const DEADLINE: Deadline = Deadline(Duration::from_secs(30));

/// Why a git query failed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum GitError {
    /// The directory is not inside a git repository.
    #[error("not a git repository")]
    NotARepo,
    /// Git could not answer; the text is moor-worded.
    #[error("{0}")]
    Failed(String),
}

/// Concrete git client; real git runs through the `Proc` seam.
#[cfg_attr(not(test), expect(dead_code, reason = "used by ops close/start"))]
pub(crate) struct Git {
    /// Spawner for every git call.
    proc: Arc<dyn Proc>,
}

#[cfg_attr(not(test), expect(dead_code, reason = "used by ops close/start"))]
impl Git {
    /// Builds a client over `proc`.
    pub(crate) fn new(proc: Arc<dyn Proc>) -> Self {
        Self { proc }
    }

    /// Repo root of `cwd`.
    ///
    /// # Errors
    /// [`GitError::NotARepo`] outside a repository, [`GitError::Failed`] otherwise.
    pub(crate) fn toplevel(&self, cwd: &Path) -> Result<PathBuf, GitError> {
        let out = self.run(cwd, &["rev-parse", "--show-toplevel"])?;
        let line = out.trim();
        if line.is_empty() {
            return Err(GitError::Failed(
                "git printed no repository root".to_owned(),
            ));
        }
        Ok(PathBuf::from(line))
    }

    /// Commits reachable from HEAD that neither `base` nor any remote-tracking
    /// ref contains.
    ///
    /// A missing worktree counts as zero. Without a `base` and without remote
    /// refs, every commit on HEAD is unpushed.
    ///
    /// # Errors
    /// [`GitError::Failed`] when git cannot answer.
    pub(crate) fn unpushed(&self, worktree: &Path, base: Option<&str>) -> Result<u32, GitError> {
        if !worktree.is_dir() {
            return Ok(0);
        }
        let mut args = vec!["rev-list", "--count", "HEAD", "--not"];
        args.extend(base);
        args.push("--remotes");
        let out = self.run(worktree, &args)?;
        out.trim().parse().map_err(|_| {
            GitError::Failed(format!("git printed an unexpected count: {}", out.trim()))
        })
    }

    /// Whether the worktree has uncommitted changes (untracked files included).
    ///
    /// A missing worktree is clean.
    ///
    /// # Errors
    /// [`GitError::Failed`] when git cannot answer.
    pub(crate) fn dirty(&self, worktree: &Path) -> Result<bool, GitError> {
        if !worktree.is_dir() {
            return Ok(false);
        }
        let out = self.run(worktree, &["status", "--porcelain"])?;
        Ok(out.lines().any(|l| !l.trim().is_empty()))
    }

    /// Runs git in `cwd` and returns stdout.
    fn run(&self, cwd: &Path, args: &[&str]) -> Result<String, GitError> {
        let spec = ProcSpec::new(Tool::Git, DEADLINE)
            .args(args.iter().copied())
            .cwd(cwd);
        match self.proc.run(&spec) {
            Ok(out) => Ok(out.stdout),
            Err(ProcError::NonZero { stderr_tail, .. })
                if stderr_tail.contains("not a git repository") =>
            {
                Err(GitError::NotARepo)
            }
            Err(ProcError::NonZero {
                code, stderr_tail, ..
            }) => Err(GitError::Failed(format!(
                "git {} failed (exit {code}): {}",
                args.first().copied().unwrap_or(""),
                stderr_tail.trim()
            ))),
            Err(e) => Err(GitError::Failed(format!(
                "git {} could not run: {e}",
                args.first().copied().unwrap_or("")
            ))),
        }
    }
}

#[cfg(test)]
mod tests;
