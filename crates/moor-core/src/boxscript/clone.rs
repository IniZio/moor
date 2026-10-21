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

/// POSIX sh, run in the Box workdir, that leaves it a partial clone of `c.url` on `c.branch`.
///
/// A rerun converges: a workdir already on the branch is left as it is, a clone that stopped
/// before its checkout carries on, and a branch that already exists is checked out where it is, so
/// commits made on it are never lost. A new branch starts from `origin/<branch>` when the remote
/// has it, else from the base. A URL, base or branch that starts with `-` is refused so git never
/// reads it as an option. `.moor/` is excluded so moor's own files never show as changes.
#[must_use]
pub fn clone_script(c: &Checkout<'_>) -> String {
    let (url, base, branch) = (quote(c.url), quote(c.base), quote(c.branch));
    let local = quote(&format!("refs/heads/{}", c.branch));
    let tracking = quote(&format!("refs/remotes/origin/{}", c.branch));
    let from = quote(&format!("origin/{}", c.branch));
    let base_commit = quote(&format!("{}^{{commit}}", c.base));
    let [clone, fetch, checkout] =
        [CloneStep::Clone, CloneStep::Base, CloneStep::Checkout].map(|s| s as i32);
    format!(
        r#"export GIT_TERMINAL_PROMPT=0
case {url} in -*) echo 'the repo URL starts with -' >&2; exit {clone};; esac
case {base} in -*) echo 'the base starts with -' >&2; exit {fetch};; esac
case {branch} in -*) echo 'the branch starts with -' >&2; exit {checkout};; esac
if [ "$(git symbolic-ref -q --short HEAD 2>/dev/null)" != {branch} ]; then
  if [ ! -d .git ]; then
    git clone -q --filter=blob:none --no-checkout -- {url} . || exit {clone}
  fi
  if git rev-parse -q --verify {local} >/dev/null; then
    git checkout -q {branch} -- || exit {checkout}
  else
    git cat-file -e -- {base_commit} 2>/dev/null || git fetch -q -- origin {base} || exit {fetch}
    if git rev-parse -q --verify {tracking} >/dev/null; then
      from={from}
    else
      from={base}
    fi
    git checkout -q --no-track -b {branch} "$from" -- || exit {checkout}
  fi
fi
grep -qx '.moor/' .git/info/exclude 2>/dev/null || printf '.moor/\n' >> .git/info/exclude
"#
    )
}

#[cfg(test)]
mod tests;
