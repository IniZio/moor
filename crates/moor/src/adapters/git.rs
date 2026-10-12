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
pub(crate) struct Git {
    /// Spawner for every git call.
    proc: Arc<dyn Proc>,
}

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

    /// Main checkout of the repository `dir` belongs to (parent of the common git dir).
    ///
    /// # Errors
    /// [`GitError::NotARepo`] outside a repository, [`GitError::Failed`] otherwise.
    pub(crate) fn main_checkout(&self, dir: &Path) -> Result<PathBuf, GitError> {
        let out = self.run(
            dir,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?;
        Path::new(out.trim())
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| GitError::Failed("git printed no common directory".to_owned()))
    }

    /// Commit SHA of HEAD in `dir`.
    ///
    /// # Errors
    /// [`GitError::NotARepo`] outside a repository, [`GitError::Failed`] otherwise.
    pub(crate) fn head(&self, dir: &Path) -> Result<String, GitError> {
        let out = self.run(dir, &["rev-parse", "HEAD"])?;
        Ok(out.trim().to_owned())
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

    /// Sets the author name and email for a linked `worktree` only.
    ///
    /// It is a no-op on the main checkout. On a linked worktree it sets
    /// `extensions.worktreeConfig=true` in the repository's shared config (a side
    /// effect visible to every worktree of that repo, enabling per-worktree
    /// config) and writes `user.name`/`user.email` to that worktree's own config
    /// only. `None` leaves a field alone.
    ///
    /// # Errors
    /// [`GitError::Failed`] when git cannot write the config.
    pub(crate) fn set_identity(
        &self,
        worktree: &Path,
        name: Option<&str>,
        email: Option<&str>,
    ) -> Result<(), GitError> {
        if name.is_none() && email.is_none() {
            return Ok(());
        }
        let main = self.main_checkout(worktree)?;
        if main.canonicalize().ok() == worktree.canonicalize().ok() {
            return Ok(());
        }
        self.run(worktree, &["config", "extensions.worktreeConfig", "true"])?;
        for (key, value) in [("user.name", name), ("user.email", email)] {
            if let Some(v) = value {
                self.run(worktree, &["config", "--worktree", key, v])?;
            }
        }
        Ok(())
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
