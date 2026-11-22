//! Script that clones a Box's repo into its workdir and checks out the Box branch.

use super::quote;

/// What the clone script needs: where the repo is, the commit the Box starts from and its branch.
#[derive(Debug, Clone, Copy)]
pub struct Checkout<'a> {
    /// The URL `origin` gets, with no credentials in it.
    pub url: &'a str,
    /// The commit a new Box branch starts from when the remote has no such branch.
    pub base: &'a str,
    /// The Box branch.
    pub branch: &'a str,
}

/// What the clone script prints before why it cloned without the repo's git mirror.
pub const MIRROR_SKIPPED: &str = "moor: git mirror skipped: ";

/// Why the clone script's `output` says it cloned without the git mirror, if it did.
#[must_use]
pub fn mirror_skipped(output: &str) -> Option<&str> {
    output
        .lines()
        .find_map(|l| l.trim_end().strip_prefix(MIRROR_SKIPPED))
}

/// Seconds a clone waits for another Box's turn at the git mirror before it clones without it.
const MIRROR_WAIT_SECS: u64 = 10 * 60;

/// The exit code of the mirror step that means "clone without the mirror"; no git command
/// exits with it.
const MIRROR_SKIP: i32 = 75;

/// What the clone script prints before `<path>: <reason>` for a submodule it left out.
pub const SUBMODULE_LEFT_OUT: &str = "moor.clone.submodule=";

/// The `<path>: <reason>` of each submodule the clone script's `output` says it left out.
#[must_use]
pub fn left_out_submodules(output: &str) -> Vec<&str> {
    output
        .lines()
        .filter_map(|l| l.trim_end().strip_prefix(SUBMODULE_LEFT_OUT))
        .collect()
}

/// Seconds from the clone script's start until it stops fetching submodules, well inside the
/// clone exec's deadline, so a large submodule costs a warning and not the Box.
const SUBMODULE_BUDGET_SECS: u64 = 20 * 60;

/// git for submodule commands. A Box has no ssh key and only https reaches the Proxy, so an ssh
/// GitHub URL in `.gitmodules` goes over https for this command and its children.
pub(super) const SUBMODULE_GIT: &str = "git -c url.https://github.com/.insteadOf=git@github.com: \
     -c url.https://github.com/.insteadOf=ssh://git@github.com/";

/// The step that stopped the clone script; the discriminant is the script's exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloneStep {
    /// `git clone` failed.
    Clone = 70,
    /// The base commit is not on the remote.
    Base = 71,
    /// The Box branch could not be checked out.
    Checkout = 72,
}

impl CloneStep {
    /// The step that exits with `code`, if any.
    #[must_use]
    pub const fn of_exit(code: i32) -> Option<Self> {
        match code {
            70 => Some(Self::Clone),
            71 => Some(Self::Base),
            72 => Some(Self::Checkout),
            _ => None,
        }
    }

    /// What went wrong, worded for the user.
    #[must_use]
    pub fn reason(self, c: &Checkout<'_>) -> String {
        match self {
            Self::Clone => format!("git clone {} failed", c.url),
            Self::Base => format!("base {} is not on {}; push it first", c.base, c.url),
            Self::Checkout => format!("could not check out branch {}", c.branch),
        }
    }
}

/// POSIX sh, run in the Box workdir, that leaves it a partial clone of `c.url` on `c.branch`,
/// taking the objects it already has from the bare git mirror at shell path `mirror`, when given.
///
/// A rerun converges: a workdir already on the branch is left as it is, a clone that stopped
/// before its checkout carries on, and a branch that already exists is checked out where it is, so
/// commits made on it are never lost. A new branch starts from `origin/<branch>` when the remote
/// has it, else from the base. A URL, base or branch that starts with `-` is refused so git never
/// reads it as an option.
///
/// `git clone` needs an empty directory, so a workdir with no `.git` is cloned into a temp dir
/// under `.moor/`, after the partial clones of runs that were cut off are removed, whose
/// `.git` then moves into the workdir; the rename stays on one filesystem. When another run's
/// `.git` appeared first, `mv` moves this clone's into it instead, so that nested copy is removed
/// and the script stops at the clone step. `.moor/` is excluded so moor's files never show as
/// changes.
///
/// With a mirror, the clone holds an exclusive `flock` on `<mirror>.lock` while it fetches
/// `c.url`'s branches and tags into the mirror and then clones with `--reference` and
/// `--dissociate`, so two Boxes never write the mirror at once, a fetch never prunes objects a clone
/// is reading, and the Box keeps no link to the mirror. The URL is passed on the command line, so
/// the mirror's config holds no remote. A missing `flock`, a lock held past [`MIRROR_WAIT_SECS`]
/// or a mirror git cannot fetch into is printed after [`MIRROR_SKIPPED`], and the clone goes to
/// `c.url` alone, as without a mirror.
///
/// Each submodule with no checkout is then fetched on its own, nested ones too. One that fails is
/// printed after [`SUBMODULE_LEFT_OUT`] and left uninitialised for a rerun to retry, and never stops
/// the script, since the agent may not need it.
#[must_use]
pub fn clone_script(c: &Checkout<'_>, mirror: Option<&str>) -> String {
    script(c, mirror, SUBMODULE_BUDGET_SECS)
}

/// The shell function `moor_clone_into DIR` that clones `url` into `DIR`, through `mirror` when
/// given.
fn clone_function(url: &str, mirror: Option<&str>) -> String {
    let direct = format!("git clone -q --filter=blob:none --no-checkout -- {url} \"$1\"");
    let Some(mirror) = mirror else {
        return format!("moor_clone_into() {{\n  {direct}\n}}\n");
    };
    let (skipped, wait, skip) = (quote(MIRROR_SKIPPED), MIRROR_WAIT_SECS, MIRROR_SKIP);
    format!(
        r#"moor_mirror={mirror}
moor_clone_into() {{
  if ! command -v flock >/dev/null 2>&1; then
    printf '%s%s\n' {skipped} 'flock is missing'
  elif ! mkdir -p -- "${{moor_mirror%/*}}" || ! : >>"$moor_mirror.lock"; then
    printf '%s%s\n' {skipped} 'its directory is not writable'
  else
    (
      flock -w {wait} 9 || {{ printf '%s%s\n' {skipped} 'another Box held it for {wait} s'; exit {skip}; }}
      [ -d "$moor_mirror/objects" ] || git init -q --bare -- "$moor_mirror" || {{ printf '%s%s\n' {skipped} 'git init failed'; exit {skip}; }}
      find "$moor_mirror" -name '*.lock' -type f -exec rm -f -- {{}} +
      git -C "$moor_mirror" -c gc.autoDetach=false fetch -q --prune --no-write-fetch-head -- {url} '+refs/heads/*:refs/heads/*' '+refs/tags/*:refs/tags/*' ||
        {{ printf '%s%s\n' {skipped} 'git fetch failed'; exit {skip}; }}
      git clone -q --filter=blob:none --no-checkout --reference "$moor_mirror" --dissociate -- {url} "$1"
    ) 9>>"$moor_mirror.lock"
    code=$?
    [ "$code" -eq {skip} ] || return "$code"
  fi
  {direct}
}}
"#
    )
}

/// [`clone_script`] whose submodule fetches stop `budget` seconds after the script starts.
fn script(c: &Checkout<'_>, mirror: Option<&str>, budget: u64) -> String {
    let (url, base, branch) = (quote(c.url), quote(c.base), quote(c.branch));
    let local = quote(&format!("refs/heads/{}", c.branch));
    let tracking = quote(&format!("refs/remotes/origin/{}", c.branch));
    let from = quote(&format!("origin/{}", c.branch));
    let base_commit = quote(&format!("{}^{{commit}}", c.base));
    let submodules = submodule_functions(budget);
    let cloner = clone_function(&url, mirror);
    let [clone, fetch, checkout] =
        [CloneStep::Clone, CloneStep::Base, CloneStep::Checkout].map(|s| s as i32);
    format!(
        r#"export GIT_TERMINAL_PROMPT=0
{submodules}{cloner}case {url} in -*) echo 'the repo URL starts with -' >&2; exit {clone};; esac
case {base} in -*) echo 'the base starts with -' >&2; exit {fetch};; esac
case {branch} in -*) echo 'the branch starts with -' >&2; exit {checkout};; esac
if [ "$(git symbolic-ref -q --short HEAD 2>/dev/null)" != {branch} ]; then
  if [ ! -d .git ]; then
    tmp=.moor/clone.$$
    rm -rf -- .moor/clone.* && mkdir -p -- "$tmp" || exit {clone}
    moor_clone_into "$tmp" && mv -- "$tmp/.git" .git &&
      [ ! -e .git/.git ]
    cloned=$?
    rm -rf -- "$tmp" .git/.git
    [ "$cloned" -eq 0 ] || exit {clone}
  fi
  if git rev-parse -q --verify {local} >/dev/null; then
    git checkout -q {branch} -- || exit {checkout}
  else
    git cat-file -e -- {base_commit} 2>/dev/null || git fetch -q --no-recurse-submodules -- origin {base} || exit {fetch}
    if git rev-parse -q --verify {tracking} >/dev/null; then
      from={from}
    else
      from={base}
    fi
    git checkout -q --no-track -b {branch} "$from" -- || exit {checkout}
  fi
fi
moor_submodules ''
for p in .moor/; do
  grep -qxF -- "$p" .git/info/exclude 2>/dev/null || printf '%s\n' "$p" >> .git/info/exclude
done
:
"#
    )
}

/// The shell functions that fetch submodules, `moor_submodules` the one to call.
fn submodule_functions(budget: u64) -> String {
    let left_out = quote(SUBMODULE_LEFT_OUT);
    format!(
        r#"moor_started=$(date +%s)
moor_submodule_fetch() {{
  left=$((moor_started + {budget} - $(date +%s)))
  [ "$left" -gt 0 ] || {{ echo 'out of time'; return 1; }}
  limit=
  command -v timeout >/dev/null && limit="timeout -k 10 $left"
  err=$($limit {SUBMODULE_GIT} submodule update -q --init $filter -- "$1" 2>&1 </dev/null) && return 0
  code=$?
  case $code in 124|137) echo 'out of time'; return 1;; esac
  printf '%s\n' "$err" | grep -m1 '^fatal:' || echo "git exited $code"
  return 1
}}
moor_submodules() {{
  [ -f .gitmodules ] || return 0
  filter=
  git submodule -h 2>&1 | grep -q -e --filter && filter=--filter=blob:none
  git -c core.quotepath=off ls-files -s | grep '^160000 ' | cut -f2 | while IFS= read -r p; do
    if [ ! -e "$p/.git" ] && ! why=$(moor_submodule_fetch "$p"); then
      git submodule deinit -f -- "$p" >/dev/null 2>&1
      printf '%s%s%s: %s\n' {left_out} "$1" "$p" "$why"
    else
      (cd -- "$p" && moor_submodules "$1$p/")
    fi
  done
}}
"#
    )
}

#[cfg(test)]
mod tests;
