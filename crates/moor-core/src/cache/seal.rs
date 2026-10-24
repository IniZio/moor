//! The close-time script that seals each volume before the provider keeps it.

use super::{CacheName, CachePath, PlannedVolume};
use crate::boxscript::quote;

/// What the seal script prints on stdout, before the volume name, for a seal that failed.
pub const SEAL_FAILED: &str = "moor: cache seal failed: ";

/// Empties each directory given; a file the login user cannot delete is retried through
/// `sudo -n`, as Docker's data root is owned by root.
const CLEAR: &str = "moor_cache_clear() { for d in \"$@\"; do [ -d \"$d\" ] || continue; find \"$d\" -mindepth 1 -delete 2>/dev/null || sudo -n find \"$d\" -mindepth 1 -delete; done; }";

/// POSIX sh text that runs each volume's seal steps in order; a volume whose step fails is
/// named on stdout and emptied, so the provider never keeps it half sealed. Exits 0.
pub(super) fn script(volumes: &[PlannedVolume]) -> Option<String> {
    let sealed: Vec<&PlannedVolume> = volumes.iter().filter(|v| !v.seal.is_empty()).collect();
    if sealed.is_empty() {
        return None;
    }
    let mut lines = vec![CLEAR.to_owned()];
    lines.extend(sealed.into_iter().map(|v| {
        let steps: Vec<String> = v
            .seal
            .iter()
            .map(|s| format!("sh -c {}", quote(s)))
            .collect();
        let dirs: Vec<String> = v.paths.iter().map(CachePath::shell).collect();
        let failed = quote(&format!("{SEAL_FAILED}{}", v.name.as_str()));
        format!(
            "if ! {{ {}; }}; then printf '%s\\n' {failed}; moor_cache_clear {}; fi",
            steps.join(" && "),
            dirs.join(" ")
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
