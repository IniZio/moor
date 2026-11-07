//! Script that brings a work tree a previous Box left back to a fresh clone plus its ignored
//! build outputs, before the clone script runs.

use super::clone::{Checkout, SUBMODULE_GIT};
use super::quote;

/// What the refresh prints on stdout before `refreshed`, `cold` or `cold after: <reason>`.
pub const LOG_PREFIX: &str = "moor: work tree ";

/// What the refresh prints when this Box's tree fell back to a fresh clone, on every attempt of
/// that Box, since a later attempt's log is all a restarted controller has.
const WENT_COLD: &str = "moor.tree.went_cold";

/// Whether `output` says the tree of this Box fell back to a fresh clone.
#[must_use]
pub fn went_cold(output: &str) -> bool {
    output.lines().any(|l| l.trim_end() == WENT_COLD)
}

/// The outcome the refresh printed in `output`: `refreshed`, `cold` or `cold after: <reason>`.
#[must_use]
pub fn outcome(output: &str) -> Option<&str> {
    output
        .lines()
        .find_map(|line| line.trim_end().strip_prefix(LOG_PREFIX))
}

/// POSIX sh, run in the Box workdir before [`super::clone::clone_script`], for Box `key`.
///
/// It runs once per Box: `.moor/tree` names the Box that last refreshed the tree, and a rerun
/// on that Box changes nothing, so the session's commits stay. Otherwise it leaves the workdir
/// either on the Box branch, as `origin/<branch>` or else the base, with every tracked file
/// reset, every untracked file and nested repo removed and ignored files kept, no remote but
/// `origin`, no ref but the Box branch, `origin`'s branches and tags, no pseudo-ref such as
/// `ORIG_HEAD` naming an old commit, and no reflog; or, when any step fails, empty, so the
/// clone script clones cold. A base of `HEAD` means the remote's default branch, never the old local HEAD. Lazy
/// fetch stays on, so a partial clone's checkout fetches the blobs it needs. The refresh never
/// fails the script. A URL, base or branch that starts with `-` is left to the clone script,
/// which refuses it. Only submodules already initialised are updated; the clone script fetches
/// the rest, so one it left out never sends a later Box cold.
#[must_use]
pub fn refresh_script(c: &Checkout<'_>, key: &str) -> String {
    refresh_with(c, key, Work::Discard)
}

/// What a refresh does with what the tree holds beyond its commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Work {
    /// Resets tracked files and removes untracked ones.
    Discard,
    /// For a fork of a checkpoint of the Box's own base, whose skipped setup lines made what its
    /// tree holds. Untracked files stay, in submodules too, and so do tracked changes while the tree
    /// stays on its commit. A tree moved to the Box's pushed branch resets tracked files, since it is
    /// off the base and its setup lines run again.
    Keep,
}

impl Work {
    /// The line that picks checkout's force, the line that cleans, and the submodule update.
    fn steps(self) -> (String, String, String) {
        match self {
        Self::Discard => (
            "force=-f".to_owned(),
            "git clean -ffdq || { moor_tree_cold 'git clean failed'; return 0; }".to_owned(),
            format!(
                "{SUBMODULE_GIT} submodule update -q --recursive --force &&\n      \
                 git submodule foreach -q --recursive 'git clean -ffdq'"
            ),
        ),
        Self::Keep => (
            r#"force=-f; [ "$(git rev-parse -q --verify "$from^{commit}")" = "$(git rev-parse -q --verify HEAD)" ] && force="#.to_owned(),
            ":".to_owned(),
            format!("{SUBMODULE_GIT} submodule update -q --recursive"),
        ),
    }
    }
}

/// [`refresh_script`], doing `work` with what the tree holds.
#[must_use]
pub fn refresh_with(c: &Checkout<'_>, key: &str, work: Work) -> String {
    let (url, base, branch, key) = (quote(c.url), quote(c.base), quote(c.branch), quote(key));
    let local = quote(&format!("refs/heads/{}", c.branch));
    let tracking = quote(&format!("refs/remotes/origin/{}", c.branch));
    let base_commit = quote(&format!("{}^{{commit}}", c.base));
    let log = quote(LOG_PREFIX);
    let went_cold = quote(WENT_COLD);
    let (force, clean, submodules) = work.steps();
    format!(
        r#"moor_tree_cold() {{
  mkdir -p .moor && printf '%s\n' {key} > .moor/tree-cold
  find . -mindepth 1 -maxdepth 1 ! -name .moor -exec rm -rf -- {{}} +
  find .moor -mindepth 1 -maxdepth 1 ! -name tree-cold -exec rm -rf -- {{}} +
  printf '%s\n' {key} > .moor/tree
  if [ -n "$1" ]; then printf '%scold after: %s\n' {log} "$1"; else printf '%scold\n' {log}; fi
  echo {went_cold}
}}
moor_tree_refresh() {{
  export GIT_TERMINAL_PROMPT=0
  if [ "$(cat .moor/tree 2>/dev/null)" = {key} ]; then
    [ "$(cat .moor/tree-cold 2>/dev/null)" = {key} ] && echo {went_cold}
    return 0
  fi
  for v in {url} {base} {branch}; do case $v in -*) return 0;; esac; done
  [ "$(cat .moor/tree-cold 2>/dev/null)" = {key} ] && {{ moor_tree_cold 'a restart cut off the last wipe'; return 0; }}
  [ -e .git ] || {{ moor_tree_cold; return 0; }}
  [ "$(git rev-parse --git-dir 2>/dev/null)" = .git ] || {{ moor_tree_cold 'not a git repo'; return 0; }}
  [ "$(git config --get remote.origin.url)" = {url} ] || {{ moor_tree_cold 'origin is another repo'; return 0; }}
  rm -rf -- .moor .crabbox .crabbox-sealed .git/index.lock .git/HEAD.lock
  for op in rebase merge cherry-pick revert am; do git "$op" --abort >/dev/null 2>&1 || :; done
  git bisect reset >/dev/null 2>&1 || :
  rm -rf -- .git/rebase-merge .git/rebase-apply
  git fetch -q --force --no-recurse-submodules --prune --prune-tags origin || {{ moor_tree_cold 'git fetch failed'; return 0; }}
  git cat-file -e -- {base_commit} 2>/dev/null || git fetch -q --no-recurse-submodules -- origin {base} || {{ moor_tree_cold 'the base could not be fetched'; return 0; }}
  if [ {base} = HEAD ]; then
    git remote set-head origin -a >/dev/null || {{ moor_tree_cold "the remote's default branch is unknown"; return 0; }}
    from=refs/remotes/origin/HEAD
  else
    from={base}
  fi
  git rev-parse -q --verify {tracking} >/dev/null && from={tracking}
  git update-index -q --refresh >/dev/null 2>&1
  {force}
  git checkout -q $force --no-track -B {branch} "$from" -- || {{ moor_tree_cold 'git checkout failed'; return 0; }}
  {clean}
  git remote | while read -r r; do [ "$r" = origin ] || git remote remove "$r"; done
  git for-each-ref --format='%(refname)' | grep -vxF -- {local} | grep -v -e '^refs/remotes/origin/' -e '^refs/tags/' |
    while read -r r; do git update-ref --no-deref -d "$r"; done
  rm -f -- .git/ORIG_HEAD .git/REBASE_HEAD .git/BISECT_EXPECTED_REV
  git reflog expire --expire=now --all && git worktree prune || {{ moor_tree_cold 'git could not drop old refs'; return 0; }}
  if [ -f .gitmodules ] && git submodule status --recursive 2>/dev/null | grep -q '^[ +U]'; then
    {{ {SUBMODULE_GIT} submodule sync -q --recursive && {submodules}; }} || {{ moor_tree_cold 'a submodule could not be updated'; return 0; }}
  fi
  mkdir -p .moor && printf '%s\n' {key} > .moor/tree
  printf '%srefreshed\n' {log}
}}
moor_tree_refresh
"#
    )
}

#[cfg(test)]
mod tests;
