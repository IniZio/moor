//! Git over `Proc`: repo root, base commit, repo files and author reads on the host.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moor_core::seam::proc::{Deadline, Proc, ProcError, ProcSpec, Tool};

/// Time limit for each git call.
const DEADLINE: Deadline = Deadline(Duration::from_secs(30));

/// Time limit for fetching the remote default branch.
const FETCH_DEADLINE: Deadline = Deadline(Duration::from_secs(20));

/// Time limit for cloning a repository.
const CLONE_DEADLINE: Deadline = Deadline(Duration::from_secs(600));

/// The remote moor bases new Boxes on.
const REMOTE: &str = "origin";

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

/// Effective git author for a worktree; `None` where git has no value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GitAuthor {
    /// `user.name`.
    pub(crate) name: Option<String>,
    /// `user.email`.
    pub(crate) email: Option<String>,
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

    /// Commit SHA that `rev` names in the repository at `dir`.
    ///
    /// # Errors
    /// [`GitError::NotARepo`] outside a repository, [`GitError::Failed`] when `rev` names no commit.
    pub(crate) fn commit_of(&self, dir: &Path, rev: &str) -> Result<String, GitError> {
        let out = self.run(
            dir,
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{rev}^{{commit}}"),
            ],
        )?;
        Ok(out.trim().to_owned())
    }

    /// Text of `path` at revision `rev` of the repository at `repo`; `None` when `rev` has no such file.
    ///
    /// # Errors
    /// [`GitError::NotARepo`] outside a repository, [`GitError::Failed`] otherwise.
    pub(crate) fn file_at(
        &self,
        repo: &Path,
        rev: &str,
        path: &str,
    ) -> Result<Option<String>, GitError> {
        let listed = self.run(repo, &["ls-tree", "--name-only", rev, "--", path])?;
        if listed.trim().is_empty() {
            return Ok(None);
        }
        self.run(repo, &["show", &format!("{rev}:{path}")])
            .map(Some)
    }

    /// Fetches the remote's default branch and returns the remote-tracking ref and its commit.
    ///
    /// `None` when the repository has no remote default branch to follow.
    ///
    /// # Errors
    /// [`GitError::Failed`] when the fetch fails or times out.
    pub(crate) fn fetch_default(&self, repo: &Path) -> Result<Option<(String, String)>, GitError> {
        let head = format!("refs/remotes/{REMOTE}/HEAD");
        let Ok(out) = self.exec(repo, &["symbolic-ref", "--short", &head], DEADLINE) else {
            return Ok(None);
        };
        let tracking = out.trim();
        let Some(branch) = tracking
            .strip_prefix(REMOTE)
            .and_then(|b| b.strip_prefix('/'))
            .filter(|b| !b.is_empty())
        else {
            return Ok(None);
        };
        let args = ["fetch", REMOTE, branch];
        self.exec(repo, &args, FETCH_DEADLINE)
            .map_err(|e| Self::failure(&args, e))?;
        let sha = self.run(repo, &["rev-parse", tracking])?;
        Ok(Some((tracking.to_owned(), sha.trim().to_owned())))
    }

    /// Clones `url` into `dest` with the host's own git credentials.
    ///
    /// # Errors
    /// [`GitError::Failed`] when the clone fails or times out.
    pub(crate) fn clone_repo(&self, url: &str, dest: &Path) -> Result<(), GitError> {
        let dest = dest.to_string_lossy();
        let args = ["clone", "--", url, &dest];
        let cwd = Path::new(dest.as_ref())
            .parent()
            .unwrap_or_else(|| Path::new("."));
        self.exec(cwd, &args, CLONE_DEADLINE)
            .map(drop)
            .map_err(|e| Self::failure(&args, e))
    }

    /// Fetches `branch` from `origin` and returns the commit of its remote-tracking ref.
    ///
    /// # Errors
    /// [`GitError::Failed`] when the fetch fails or times out, or the branch is absent.
    pub(crate) fn fetch_branch(&self, repo: &Path, branch: &str) -> Result<String, GitError> {
        let tracking = format!("refs/remotes/{REMOTE}/{branch}");
        let refspec = format!("+refs/heads/{branch}:{tracking}");
        let args = ["fetch", REMOTE, &refspec];
        self.exec(repo, &args, FETCH_DEADLINE)
            .map_err(|e| Self::failure(&args, e))?;
        Ok(self.run(repo, &["rev-parse", &tracking])?.trim().to_owned())
    }

    /// Effective git author of `worktree`, as git resolves it.
    ///
    /// A key git has no value for is `None`.
    ///
    /// # Errors
    /// [`GitError::NotARepo`] outside a repository, [`GitError::Failed`] otherwise.
    pub(crate) fn author(&self, worktree: &Path) -> Result<GitAuthor, GitError> {
        Ok(GitAuthor {
            name: self.config_value(worktree, "user.name")?,
            email: self.config_value(worktree, "user.email")?,
        })
    }

    /// One config value; git exits 1 for a missing key, which is `None`.
    fn config_value(&self, cwd: &Path, key: &str) -> Result<Option<String>, GitError> {
        let args = ["config", "--get", key];
        match self.exec(cwd, &args, DEADLINE) {
            Ok(out) => Ok(Some(out.trim().to_owned()).filter(|v| !v.is_empty())),
            Err(ProcError::NonZero { code: 1, .. }) => Ok(None),
            Err(e) => Err(Self::failure(&args, e)),
        }
    }

    /// Runs git in `cwd` and returns stdout.
    fn run(&self, cwd: &Path, args: &[&str]) -> Result<String, GitError> {
        self.exec(cwd, args, DEADLINE)
            .map_err(|e| Self::failure(args, e))
    }

    /// Runs git in `cwd` within `deadline`, keeping the raw process error.
    fn exec(&self, cwd: &Path, args: &[&str], deadline: Deadline) -> Result<String, ProcError> {
        let spec = ProcSpec::new(Tool::Git, deadline)
            .args(args.iter().copied())
            .cwd(cwd);
        self.proc.run(&spec).map(|out| out.stdout)
    }

    /// Words a process error as a [`GitError`].
    fn failure(args: &[&str], err: ProcError) -> GitError {
        match err {
            ProcError::NonZero { stderr_tail, .. }
                if stderr_tail.contains("not a git repository") =>
            {
                GitError::NotARepo
            }
            ProcError::NonZero {
                code, stderr_tail, ..
            } => GitError::Failed(format!(
                "git {} failed (exit {code}): {}",
                args.first().copied().unwrap_or(""),
                stderr_tail.trim()
            )),
            e => GitError::Failed(format!(
                "git {} could not run: {e}",
                args.first().copied().unwrap_or("")
            )),
        }
    }
}

#[cfg(test)]
mod tests;
