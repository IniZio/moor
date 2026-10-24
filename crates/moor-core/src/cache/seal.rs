//! The close-time script that seals each volume before the provider keeps it.

use super::{CacheName, CachePath, PlannedVolume};
use crate::boxscript::quote;

/// What the seal script prints on stdout, before the volume name, for a seal that failed.
pub const SEAL_FAILED: &str = "moor: cache seal failed: ";

/// Empties each directory given; a file the login user cannot delete is retried through
/// `sudo -n`, as Docker's data root is owned by root.
const CLEAR: &str = "moor_cache_clear() { for d in \"$@\"; do [ -d \"$d\" ] || continue; find \"$d\" -mindepth 1 -delete 2>/dev/null || sudo -n find \"$d\" -mindepth 1 -delete; done; }";

/// Writes `.crabbox-sealed` into each existing directory given, through `sudo -n` where the
/// login user cannot; the provider keeps a volume that has a seal only when every directory of
/// it holds that file.
const MARK: &str = "moor_cache_mark() { for d in \"$@\"; do [ -d \"$d\" ] || continue; touch \"$d/.crabbox-sealed\" 2>/dev/null || sudo -n touch \"$d/.crabbox-sealed\" || return 1; done; }";

/// Removes `.crabbox-sealed` from each directory given, through `sudo -n` where the login user
/// cannot; it fails when a marker stays.
const UNMARK: &str = "moor_cache_unmark() { for d in \"$@\"; do rm -f \"$d/.crabbox-sealed\" 2>/dev/null || sudo -n rm -f \"$d/.crabbox-sealed\" || return 1; done; }";

/// POSIX sh text that first removes every marker of every sealed volume, so one from an earlier
/// seal cannot outlive this one, then runs each volume's seal steps in order and marks the
/// volume sealed. A volume whose marker stays, step or mark fails is named on stdout and
/// emptied, and has no marker, so the provider never keeps it half sealed. Exits 0.
pub(super) fn script(volumes: &[PlannedVolume]) -> Option<String> {
    let sealed: Vec<&PlannedVolume> = volumes.iter().filter(|v| !v.seal.is_empty()).collect();
    if sealed.is_empty() {
        return None;
    }
    let dirs_of = |v: &PlannedVolume| -> String {
        let dirs: Vec<String> = v.paths.iter().map(CachePath::shell).collect();
        dirs.join(" ")
    };
    let mut lines = vec![CLEAR.to_owned(), MARK.to_owned(), UNMARK.to_owned()];
    lines.extend(
        sealed
            .iter()
            .enumerate()
            .map(|(i, v)| format!("stale{i}=; moor_cache_unmark {} || stale{i}=1", dirs_of(v))),
    );
    lines.extend(sealed.into_iter().enumerate().map(|(i, v)| {
        let steps: Vec<String> = v
            .seal
            .iter()
            .map(|s| format!("sh -c {}", quote(s)))
            .collect();
        let failed = quote(&format!("{SEAL_FAILED}{}", v.name.as_str()));
        let dirs = dirs_of(v);
        format!(
            "if [ -n \"$stale{i}\" ] || ! {{ {} && moor_cache_mark {dirs}; }}; then printf '%s\\n' {failed}; moor_cache_clear {dirs}; fi",
            steps.join(" && "),
        )
    }));
    lines.push("exit 0".to_owned());
    Some(lines.join("\n"))
}

/// The volumes `stdout` of a seal script names as failed and emptied.
#[must_use]
pub fn failed(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|l| l.trim().strip_prefix(SEAL_FAILED))
        .filter(|n| CacheName::parse(n).is_ok())
        .map(str::to_owned)
        .collect()
}
