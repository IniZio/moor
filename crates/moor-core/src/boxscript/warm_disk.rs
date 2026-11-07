//! Script that says how a Box's tree differs from the checkpoint disk it forked: how many tracked
//! paths changed since the disk's commit, and which ignored paths came with the disk.

use super::quote;

/// What the script prints before the changed path count.
const CHANGED: &str = "moor.warm.changed=";

/// What the script prints before each ignored path.
const IGNORED: &str = "moor.warm.ignored=";

/// POSIX sh, run in the work tree after the clone script.
///
/// It always ends with exit 0, since a count it cannot make costs a line of the note and not
/// the Box. `git diff --name-only` reads only trees, so a partial clone fetches nothing for it. It
/// prints at most `limit` ignored paths, leaving out moor's and crabbox's own excluded directories.
#[must_use]
pub fn script(commit: &str, limit: usize) -> String {
    let (commit, changed, ignored) = (quote(commit), quote(CHANGED), IGNORED);
    format!(
        r#"if moor_names=$(git diff --no-renames --name-only {commit} HEAD -- 2>/dev/null); then
  printf '%s%s\n' {changed} "$(printf '%s' "$moor_names" | grep -c '')"
fi
git ls-files --others --ignored --exclude-standard --directory 2>/dev/null |
  grep -vxF -e .moor/ -e .crabbox/ -e .crabbox-sealed | head -n {limit} | sed 's|^|{ignored}|'
:
"#
    )
}

/// POSIX sh, run in the work tree, that exits 0 only while it can read HEAD and HEAD is `base`.
#[must_use]
pub fn head_at(base: &str) -> String {
    let base = quote(&format!("{base}^{{commit}}"));
    format!(
        r#"moor_head=$(git rev-parse -q --verify HEAD) && [ -n "$moor_head" ] && [ "$moor_head" = "$(git rev-parse -q --verify {base})" ]"#
    )
}

/// The changed path count and the ignored paths the script printed in `output`.
#[must_use]
pub fn parse(output: &str) -> (Option<u64>, Vec<String>) {
    let changed = output
        .lines()
        .find_map(|l| l.trim_end().strip_prefix(CHANGED)?.parse().ok());
    let ignored = output
        .lines()
        .filter_map(|l| l.trim_end().strip_prefix(IGNORED))
        .map(str::to_owned)
        .collect();
    (changed, ignored)
}

#[cfg(test)]
mod tests;
