//! Probe for unsaved work in a Box checkout: commits not on any remote-tracking ref or saved sha,
//! and dirty files outside `.moor/`.

use super::quote;

/// Box-relative path of the probe's result file.
pub const BOXWORK_FILE: &str = ".moor/boxwork";

/// Prefix of the marker line the probe prints on stdout.
pub const BOXWORK_MARKER: &str = "MOOR_BOXWORK";

/// Unsaved work found in a Box checkout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoxworkCounts {
    /// Commits reachable from HEAD but from no remote-tracking ref and no saved sha.
    pub ahead: u32,
    /// Changed or untracked paths outside `.moor/`.
    pub dirty: u32,
}

/// Directories of regenerable build output; paths under them never count as unsaved work.
///
/// Each entry matches as whole path components at any depth, so `.turbo` skips `a/.turbo/x` but
/// not `src/.turbo.rs`.
const CACHE_DIRS: [&str; 4] = [
    ".next/cache",
    "node_modules/.cache",
    ".turbo",
    ".pnpm-store",
];

/// Extended regex for status lines whose path does not count: `.moor/` and the cache dirs.
fn ignored_status_pattern() -> String {
    let dirs = CACHE_DIRS.map(|d| d.replace('.', "\\.")).join("|");
    format!(r#"^.. "?(\.moor/|(.*/)?({dirs})/)"#)
}

/// True when `s` is exactly 40 ASCII hex chars.
#[must_use]
pub fn is_full_sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Script run with `sh -c` in the Box workdir.
///
/// `saved` are commit shas whose history counts as saved (task base, host HEAD). Returns `None`
/// when any entry is not a full 40-hex sha. Any git failure makes the script exit non-zero.
#[must_use]
pub fn boxwork_script(saved: &[&str]) -> Option<String> {
    if !saved.iter().all(|s| is_full_sha(s)) {
        return None;
    }
    let shas: String = saved.iter().fold(String::new(), |mut acc, s| {
        acc.push(' ');
        acc.push_str(&quote(s));
        acc
    });
    // A result file from an earlier probe is removed first so it is never read as fresh.
    // Lazy fetch is off because a partial clone with an unreachable origin would hang the probe.
    // Saved shas absent from the Box are dropped so rev-list never looks for a missing object.
    // `--no-renames` splits a rename into delete and add, so a move out of `.moor/` still counts.
    // A status line is `XY path`; paths under `.moor/` are moor's own and never count, and
    // neither do cache dirs, which a package install rewrites even when the repo tracks them.
    let ignored = quote(&ignored_status_pattern());
    Some(format!(
        r#"export GIT_NO_LAZY_FETCH=1
rm -f {BOXWORK_FILE}
mkdir -p .moor || exit 1
keep=
for s in{shas}; do
  git cat-file -e "$s^{{commit}}" 2>/dev/null && keep="$keep $s"
done
ahead=$(git rev-list --count HEAD --not --remotes $keep) || exit 1
st=$(git status --porcelain --no-renames) || exit 1
if [ -z "$st" ]; then
  dirty=0
else
  dirty=$(printf '%s\n' "$st" | grep -vcE {ignored} || true)
fi
ahead=$((ahead + 0))
dirty=$((dirty + 0))
printf 'ahead=%s dirty=%s\n' "$ahead" "$dirty" > {BOXWORK_FILE} || exit 1
echo "{BOXWORK_MARKER} ahead=$ahead dirty=$dirty"
"#
    ))
}

/// Parses the result file text (or a marker line).
///
/// Takes the first line of the form `ahead=N dirty=M`, optionally prefixed by the marker. `N` and
/// `M` are unsigned decimal. Returns `None` when no line matches; never defaults to zeros.
#[must_use]
pub fn parse_boxwork(text: &str) -> Option<BoxworkCounts> {
    text.lines().find_map(parse_line)
}

/// One `ahead=N dirty=M` line, with the optional marker prefix; exactly those two tokens.
fn parse_line(line: &str) -> Option<BoxworkCounts> {
    let line = line.trim();
    let line = match line.strip_prefix(BOXWORK_MARKER) {
        Some(rest) if rest.starts_with(char::is_whitespace) => rest,
        Some(_) => return None,
        None => line,
    };
    let mut parts = line.split_whitespace();
    let ahead = number(parts.next()?.strip_prefix("ahead=")?)?;
    let dirty = number(parts.next()?.strip_prefix("dirty=")?)?;
    parts
        .next()
        .is_none()
        .then_some(BoxworkCounts { ahead, dirty })
}

/// Unsigned decimal digits only (no sign), fitting `u32`.
fn number(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

#[cfg(test)]
mod tests;
