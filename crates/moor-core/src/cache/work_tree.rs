//! The repo's work tree as a cache volume, at one path per repo, so a later Box of the same user
//! and repo starts from the previous tree and its ignored build outputs.

use std::num::NonZeroU32;

use serde::Deserialize;

use super::{CacheName, CachePath, OnFail, Volume};
use crate::box_key::WORK_ROOT;

/// The volume's name.
pub const NAME: &str = "worktree";

/// The directory below the work root that holds one work tree per repo.
const REPOS: &str = "repos";

/// Size hint in GB when no layer sets one.
const DEFAULT_SIZE_GB: NonZeroU32 = NonZeroU32::new(40).unwrap();

/// `cache.work_tree`: `true` (the default), `false`, or `{size_gb: N}`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum WorkTree {
    /// Kept with the default size, or not kept.
    On(bool),
    /// Kept with this size.
    Sized(WorkTreeSize),
}

/// `cache.work_tree: {size_gb: N}`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkTreeSize {
    /// Size hint in GB.
    pub size_gb: NonZeroU32,
}

impl Default for WorkTree {
    fn default() -> Self {
        Self::On(true)
    }
}

impl WorkTree {
    /// Whether this layer keeps the work tree.
    pub(super) const fn keeps(&self) -> bool {
        !matches!(self, Self::On(false))
    }

    /// The size this layer sets, if any.
    pub(super) const fn size_gb(&self) -> Option<NonZeroU32> {
        match self {
            Self::Sized(s) => Some(s.size_gb),
            Self::On(_) => None,
        }
    }
}

impl CachePath {
    /// `~/moor/repos/<name>`; none for a name that is empty, `.`, `..`, or holds a character
    /// outside letters, digits and `._-+@`. The only cache path below the work root.
    pub(super) fn work_tree(name: &str) -> Option<Self> {
        let ok = !name.is_empty()
            && name != "."
            && name != ".."
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-+@".contains(&b));
        ok.then(|| Self(format!("~/{}", dir(name))))
    }
}

/// The work tree's directory below the login home's work root, `repos/<name>`.
fn dir(name: &str) -> String {
    format!("{WORK_ROOT}/{REPOS}/{name}")
}

/// The work tree's directory below the work root, as the plan reports it.
pub(super) fn below_root(path: &CachePath) -> Option<&str> {
    path.as_str()
        .strip_prefix("~/")?
        .strip_prefix(WORK_ROOT)?
        .strip_prefix('/')
}

/// The work tree volume for the repo `name`, sized `size_gb` or the default.
pub(super) fn volume(name: &str, size_gb: Option<NonZeroU32>) -> Option<Volume> {
    let path = CachePath::work_tree(name)?;
    let seal = vec![seal_step(&path)];
    Some(Volume {
        name: CacheName::parse(NAME).ok()?,
        paths: vec![path],
        size_gb: Some(size_gb.unwrap_or(DEFAULT_SIZE_GB)),
        seal,
        on_fail: OnFail::Keep,
    })
}

/// The seal step: it passes only when the tree holds nothing a remote lacks, and otherwise prints
/// why on its last line. Modified and untracked files do not block it, as they never entered
/// the object store and the next Box's refresh removes them. `MOOR_SEAL_BASE`, when set, is the
/// Box's base, which counts as pushed. Lazy fetch is off so a check never reaches the network.
fn seal_step(path: &CachePath) -> String {
    format!(
        r#"cd {dir} 2>/dev/null && [ "$(git rev-parse --git-dir 2>/dev/null)" = .git ] || {{ echo 'the work tree is not a git checkout'; exit 1; }}
export GIT_NO_LAZY_FETCH=1
for f in rebase-merge rebase-apply MERGE_HEAD CHERRY_PICK_HEAD REVERT_HEAD BISECT_LOG; do
  [ -e ".git/$f" ] && {{ echo "a git operation is in progress (.git/$f)"; exit 1; }}
done
git rev-parse -q --verify refs/stash >/dev/null && {{ echo 'it holds a git stash'; exit 1; }}
git diff --cached --quiet 2>/dev/null || {{ echo 'it has staged changes'; exit 1; }}
set -- HEAD --branches --not --remotes
if [ -n "$MOOR_SEAL_BASE" ] && b=$(git rev-parse -q --verify "$MOOR_SEAL_BASE^{{commit}}" 2>/dev/null); then set -- "$@" "$b"; fi
n=$(git rev-list --count "$@" 2>/dev/null) || {{ echo 'git could not count unpushed commits'; exit 1; }}
[ "$n" -eq 0 ] || {{ [ "$n" -eq 1 ] && s= || s=s; echo "$n unpushed commit$s"; exit 1; }}"#,
        dir = path.shell(),
    )
}
