//! Script that brings a work tree a previous Box left back to a fresh clone plus its ignored
//! build outputs, before the clone script runs.

use super::clone::Checkout;
use super::quote;

/// What the refresh prints on stdout before `refreshed`, `cold` or `cold after: <reason>`.
pub const LOG_PREFIX: &str = "moor: work tree ";

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
/// which refuses it.
#[must_use]
pub fn refresh_script(c: &Checkout<'_>, key: &str) -> String {
    let (url, base, branch, key) = (quote(c.url), quote(c.base), quote(c.branch), quote(key));
    let local = quote(&format!("refs/heads/{}", c.branch));
    let tracking = quote(&format!("refs/remotes/origin/{}", c.branch));
    let base_commit = quote(&format!("{}^{{commit}}", c.base));
    let log = quote(LOG_PREFIX);
    format!(
        r#"moor_tree_cold() {{
  find . -mindepth 1 -maxdepth 1 -exec rm -rf -- {{}} +
  mkdir -p .moor && printf '%s\n' {key} > .moor/tree
  if [ -n "$1" ]; then printf '%scold after: %s\n' {log} "$1"; else printf '%scold\n' {log}; fi
}}
moor_tree_refresh() {{
  export GIT_TERMINAL_PROMPT=0
  [ "$(cat .moor/tree 2>/dev/null)" = {key} ] && return 0
  for v in {url} {base} {branch}; do case $v in -*) return 0;; esac; done
  [ -e .git ] || {{ moor_tree_cold; return 0; }}
  [ "$(git rev-parse --git-dir 2>/dev/null)" = .git ] || {{ moor_tree_cold 'not a git repo'; return 0; }}
  [ "$(git config --get remote.origin.url)" = {url} ] || {{ moor_tree_cold 'origin is another repo'; return 0; }}
  rm -rf -- .moor .crabbox .crabbox-sealed .git/index.lock .git/HEAD.lock
  for op in rebase merge cherry-pick revert am; do git "$op" --abort >/dev/null 2>&1 || :; done
  git bisect reset >/dev/null 2>&1 || :
  rm -rf -- .git/rebase-merge .git/rebase-apply
  git fetch -q --prune --prune-tags origin || {{ moor_tree_cold 'git fetch failed'; return 0; }}
  git cat-file -e -- {base_commit} 2>/dev/null || git fetch -q -- origin {base} || {{ moor_tree_cold 'the base could not be fetched'; return 0; }}
  if [ {base} = HEAD ]; then
    git remote set-head origin -a >/dev/null || {{ moor_tree_cold "the remote's default branch is unknown"; return 0; }}
    from=refs/remotes/origin/HEAD
  else
    from={base}
  fi
  git rev-parse -q --verify {tracking} >/dev/null && from={tracking}
  git update-index -q --refresh >/dev/null 2>&1
  git checkout -q -f --no-track -B {branch} "$from" -- || {{ moor_tree_cold 'git checkout failed'; return 0; }}
  git clean -ffdq || {{ moor_tree_cold 'git clean failed'; return 0; }}
  git remote | while read -r r; do [ "$r" = origin ] || git remote remove "$r"; done
  git for-each-ref --format='%(refname)' | grep -vxF -- {local} | grep -v -e '^refs/remotes/origin/' -e '^refs/tags/' |
    while read -r r; do git update-ref --no-deref -d "$r"; done
  rm -f -- .git/ORIG_HEAD .git/REBASE_HEAD
  git reflog expire --expire=now --all && git worktree prune || {{ moor_tree_cold 'git could not drop old refs'; return 0; }}
  if [ -f .gitmodules ] && git submodule status --recursive 2>/dev/null | grep -q '^[ +U]'; then
    {{ git submodule sync -q --recursive && git submodule update -q --init --recursive --force &&
      git submodule foreach -q --recursive 'git clean -ffdq'; }} || {{ moor_tree_cold 'a submodule could not be updated'; return 0; }}
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
