//! What a Box changed in git: the files and lines it added and removed against its base, the
//! scripts that read them from a work tree, and the receipt moor records at each turn end.
//!
//! A script snapshots the work tree, untracked files included, as a git tree in a scratch index,
//! so the agent's own index is never touched. A receipt diffs that snapshot against the one the
//! last receipt took, which the Box keeps in `<git dir>/moor/turn`.

use serde::{Deserialize, Serialize};

use crate::boxscript::quote;
pub use crate::domain::changes::{FileChange, RepoReceipt, Tally, RECEIPT_FILES};

/// The most bytes of patch a view carries for one repo.
pub const PATCH_BYTES: usize = 512 * 1024;

/// One repo of what `changes` answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoChanges {
    /// The repo, as `OWNER/NAME`.
    pub repo: String,
    /// Everything against the base.
    pub total: Tally,
    /// Each file changed; a receipt keeps only the first [`RECEIPT_FILES`].
    pub files: Vec<FileChange>,
    /// The unified diff against the base, cut at [`PATCH_BYTES`]; none unless read from the Box now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
    /// True when the patch was cut.
    #[serde(default)]
    pub cut: bool,
    /// Why the repo could not be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

/// What `changes` answers: a Box's changes in git, each repo first one first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangesView {
    /// True when read from the Box now; false for what the last turn's receipt saw.
    pub live: bool,
    /// Why the Box was not read now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// When the receipt shown was taken, for a view that is not live; none when there is none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// Each repo.
    pub repos: Vec<RepoChanges>,
}

/// The files `git diff --numstat -z -M` printed.
#[must_use]
pub fn parse_numstat(out: &str) -> Vec<FileChange> {
    let count = |n: &str| n.parse().ok();
    let mut parts = out.split('\0');
    let mut files = Vec::new();
    while let Some(head) = parts.next() {
        let mut cols = head.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) = (cols.next(), cols.next(), cols.next())
        else {
            continue;
        };
        let (path, from) = if path.is_empty() {
            let (Some(from), Some(to)) = (parts.next(), parts.next()) else {
                break;
            };
            (to.to_owned(), Some(from.to_owned()))
        } else {
            (path.to_owned(), None)
        };
        files.push(FileChange {
            path,
            from,
            added: count(added),
            removed: count(removed),
        });
    }
    files
}

/// A repo's work tree in the Box and the branch it started from; none for the remote's default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tree {
    /// The repo, as `OWNER/NAME`.
    pub repo: String,
    /// Its work tree.
    pub dir: String,
    /// Its base branch.
    pub base: Option<String>,
}

/// Enters `dir` and sets `g` to its git dir; when it cannot, prints `why` and exits 0.
fn enter(dir: &str, why: bool) -> String {
    let say = |s: &str| {
        if why {
            format!("echo '{s}'; ")
        } else {
            String::new()
        }
    };
    format!(
        "cd -- {dir} 2>/dev/null || {{ {}exit 0; }}\n\
         g=$(git rev-parse --absolute-git-dir 2>/dev/null) || {{ {}exit 0; }}\n\
         export GIT_NO_LAZY_FETCH=1 GIT_OPTIONAL_LOCKS=0\n",
        say("no checkout"),
        say("not a git repo"),
        dir = quote(dir),
    )
}

/// Snapshots `tree` as `<git dir>/moor/<tag>.tree`, with its merge base with the base as
/// `<tag>.base`, and prints `ok`; otherwise prints why not.
#[must_use]
pub fn snapshot(tree: &Tree, tag: &str) -> String {
    let refs: Vec<String> = tree
        .base
        .iter()
        .flat_map(|b| [format!("origin/{b}"), b.clone()])
        .chain(["origin/HEAD".to_owned()])
        .map(|r| quote(&r))
        .collect();
    format!(
        r#"{enter}mkdir -p "$g/moor" || exit 0
b=''
for r in {refs}; do b=$(git merge-base HEAD "$r" 2>/dev/null) && break; b=''; done
[ -n "$b" ] || b=$(git rev-parse -q --verify HEAD) || b=$(git hash-object -t tree /dev/null)
i="$g/moor/{tag}.index"
rm -f "$i"; cp "$g/index" "$i" 2>/dev/null
if GIT_INDEX_FILE="$i" git add -A . >/dev/null 2>&1 && t=$(GIT_INDEX_FILE="$i" git write-tree); then
  rm -f "$i"
  echo "$b" >"$g/moor/{tag}.base" && echo "$t" >"$g/moor/{tag}.tree" && echo ok
else
  rm -f "$i"; echo 'git could not read the work tree'
fi
"#,
        enter = enter(&tree.dir, true),
        refs = refs.join(" "),
    )
}

/// Reads the snapshot `tag` into `b` and `t`, or exits 0 having printed nothing.
fn read(tree: &Tree, tag: &str) -> String {
    format!(
        "{}t=$(cat \"$g/moor/{tag}.tree\" 2>/dev/null) && b=$(cat \"$g/moor/{tag}.base\") || exit 0\n",
        enter(&tree.dir, false)
    )
}

/// The diff flags every script uses: renames found, and no helper the repo configures runs.
const FLAGS: &str = "-M --no-color --no-ext-diff --no-textconv";

/// Prints the numstat of snapshot `tag` against the one the last receipt kept, or the base.
#[must_use]
pub fn turn_numstat(tree: &Tree, tag: &str) -> String {
    format!(
        "{}p=$(cat \"$g/moor/turn\" 2>/dev/null) && git cat-file -e \"$p^{{tree}}\" 2>/dev/null && b=$p\n\
         git diff --numstat -z {FLAGS} \"$b\" \"$t\"\n:\n",
        read(tree, tag)
    )
}

/// Prints the numstat of snapshot `tag` against the base; `keep` makes it the one the next
/// receipt diffs against.
#[must_use]
pub fn total_numstat(tree: &Tree, tag: &str, keep: bool) -> String {
    let keep = if keep {
        format!("mv -f \"$g/moor/{tag}.tree\" \"$g/moor/turn\"; rm -f \"$g/moor/{tag}.base\"\n")
    } else {
        String::new()
    };
    format!(
        "{}git diff --numstat -z {FLAGS} \"$b\" \"$t\"\n{keep}:\n",
        read(tree, tag)
    )
}

/// Prints the patch of snapshot `tag` against the base, at most one byte past `cap`, then
/// removes the snapshot.
#[must_use]
pub fn patch(tree: &Tree, tag: &str, cap: usize) -> String {
    format!(
        "{}git diff {FLAGS} \"$b\" \"$t\" | head -c {}\nrm -f \"$g/moor/{tag}\".*\n:\n",
        read(tree, tag),
        cap + 1
    )
}

/// The patch a script printed, cut at the last whole line within `cap` bytes, and whether it
/// was cut.
#[must_use]
pub fn capped(out: &str, cap: usize) -> (String, bool) {
    if out.len() <= cap {
        return (out.to_owned(), false);
    }
    let end = out
        .as_bytes()
        .get(..cap)
        .and_then(|head| head.iter().rposition(|b| *b == b'\n'))
        .map_or(0, |at| at + 1);
    (out.get(..end).unwrap_or_default().to_owned(), true)
}

#[cfg(test)]
mod tests;
