//! Git over `Proc`: repo root and the facts the close gate needs.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moor_core::seam::proc::{Deadline, Proc, ProcError, ProcSpec, Tool};

/// Time limit for each git call.
const DEADLINE: Deadline = Deadline(Duration::from_secs(30));

/// Time limit for fetching the remote default branch.
const FETCH_DEADLINE: Deadline = Deadline(Duration::from_secs(20));

/// The remote moor bases new Tasks on.
const REMOTE: &str = "origin";

/// Scratch directories kept out of a Task worktree's status.
const SCRATCH_DIRS: [&str; 2] = [".crabbox/", ".moor/"];

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

/// What [`Git::sync_branch`] did to the local Task branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BranchSync {
    /// The branch and its worktree moved forward to the remote tip.
    FastForwarded,
    /// The local branch already contains the remote tip.
    UpToDate,
    /// The local and remote branches each have commits the other lacks; nothing changed.
    Diverged,
    /// The worktree has uncommitted changes; nothing changed.
    Dirty,
    /// The worktree has another branch checked out; nothing changed.
    OffBranch,
    /// The remote has no such branch, so there is nothing to sync.
    NotPushed,
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

    /// Fetches `branch` from `origin`, if the remote has it, and fast-forwards the worktree's checked-out `branch` to it.
    ///
    /// Only a remote tip that descends from the local tip moves the branch; local commits
    /// and uncommitted changes are never rewritten or discarded.
    ///
    /// # Errors
    /// [`GitError::Failed`] when the fetch or any git query fails.
    pub(crate) fn sync_branch(
        &self,
        worktree: &Path,
        branch: &str,
    ) -> Result<BranchSync, GitError> {
        let head_ref = format!("refs/heads/{branch}");
        let probe = ["ls-remote", "--exit-code", "origin", &head_ref];
        match self.exec(worktree, &probe, DEADLINE) {
            Ok(_) => {}
            Err(ProcError::NonZero { code: 2, .. }) => return Ok(BranchSync::NotPushed),
            Err(e) => return Err(Self::failure(&probe, e)),
        }
        let tracking = format!("refs/remotes/origin/{branch}");
        let refspec = format!("+refs/heads/{branch}:{tracking}");
        self.run(worktree, &["fetch", "origin", &refspec])?;
        let current = self.run(worktree, &["symbolic-ref", "--short", "HEAD"])?;
        if current.trim() != branch {
            return Ok(BranchSync::OffBranch);
        }
        let local = self.head(worktree)?;
        let remote = self.run(worktree, &["rev-parse", &tracking])?;
        let (local, remote) = (local.trim(), remote.trim());
        if self.is_ancestor(worktree, remote, local)? {
            return Ok(BranchSync::UpToDate);
        }
        if !self.is_ancestor(worktree, local, remote)? {
            return Ok(BranchSync::Diverged);
        }
        if self.dirty(worktree)? {
            return Ok(BranchSync::Dirty);
        }
        self.run(worktree, &["merge", "--ff-only", &tracking])?;
        Ok(BranchSync::FastForwarded)
    }

    /// Whether commit `old` is `new` or one of its ancestors; git exits 1 when it is not.
    fn is_ancestor(&self, cwd: &Path, old: &str, new: &str) -> Result<bool, GitError> {
        let args = ["merge-base", "--is-ancestor", old, new];
        match self.exec(cwd, &args, DEADLINE) {
            Ok(_) => Ok(true),
            Err(ProcError::NonZero { code: 1, .. }) => Ok(false),
            Err(e) => Err(Self::failure(&args, e)),
        }
    }

    /// Hides crabbox and moor scratch directories from `git status` in `worktree`.
    ///
    /// Appends `.crabbox/` and `.moor/` to the repository's `info/exclude`,
    /// skipping lines already there. Linked worktrees share that file.
    ///
    /// # Errors
    /// [`GitError::Failed`] when git cannot answer or the file cannot be written.
    pub(crate) fn exclude_scratch(&self, worktree: &Path) -> Result<(), GitError> {
        let out = self.run(worktree, &["rev-parse", "--git-path", "info/exclude"])?;
        let path = worktree.join(out.trim());
        let io =
            |e: std::io::Error| GitError::Failed(format!("cannot update {}: {e}", path.display()));
        let mut text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(io(e)),
        };
        let mut changed = false;
        for line in SCRATCH_DIRS {
            if text.lines().any(|l| l == line) {
                continue;
            }
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(line);
            text.push('\n');
            changed = true;
        }
        if !changed {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        std::fs::write(&path, text).map_err(io)
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
