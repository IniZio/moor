//! The script that makes restored volumes safe to use, as the first step of a Box's provisioning.

use super::{CacheName, CachePath, PlannedVolume};
use crate::boxscript::quote;

/// What the script prints on stdout, before `<name>: <why>`, for a volume it emptied.
pub const EMPTIED: &str = "moor: cache emptied ";

/// Present once the script has finished. It lives on the root disk, which a new Box gets fresh and
/// a checkpoint's fork inherits along with its volumes, so live data is never cleaned twice.
const STAMP: &str = "$HOME/.moor/cache-restored";

/// Empties each directory given, through `sudo -n` where the login user cannot.
const CLEAR: &str = "moor_cache_clear() { for d in \"$@\"; do [ -d \"$d\" ] || continue; find \"$d\" -mindepth 1 -delete 2>/dev/null || sudo -n find \"$d\" -mindepth 1 -delete || return 1; done; }";

/// Exits 0 when a directory given holds anything; one the login user cannot read counts.
const HOLDS: &str = "moor_cache_holds() { for d in \"$@\"; do [ -d \"$d\" ] || continue; { [ -r \"$d\" ] && [ -x \"$d\" ]; } || return 0; [ -z \"$(find \"$d\" -mindepth 1 -print -quit 2>/dev/null)\" ] || return 0; done; return 1; }";

/// POSIX sh that runs each volume's steps, unless a run on this root disk finished, skipping one
/// that holds nothing. A failed step empties its volume, named as [`emptied`] reads it.
pub(super) fn script(volumes: &[PlannedVolume]) -> Option<String> {
    let lines: Vec<String> = volumes
        .iter()
        .filter(|v| !v.restore.is_empty())
        .map(volume)
        .collect();
    if lines.is_empty() {
        return None;
    }
    let lines = lines.join("\n");
    Some(format!(
        "{CLEAR}\n{HOLDS}\nmoor_cache_restore() {{\n[ -e \"{STAMP}\" ] && return 0\n{lines}\nmkdir -p \"$HOME/.moor\" && : >\"{STAMP}\"\n}}\nmoor_cache_restore\n"
    ))
}

/// The lines for one volume.
fn volume(v: &PlannedVolume) -> String {
    let dirs: Vec<String> = v.paths.iter().map(CachePath::shell).collect();
    let dirs = dirs.join(" ");
    let run = |s: &str| format!("sh -c {} </dev/null", quote(s));
    let steps: Vec<String> = v.restore.iter().map(|s| run(s)).collect();
    let steps = steps.join(" && ");
    let emptied = quote(&format!("{EMPTIED}{}: ", v.name.as_str()));
    let after = v
        .after
        .map_or_else(String::new, |a| format!("\n  {} || :", run(a)));
    format!(
        "if moor_cache_holds {dirs}; then\n  \
           if ! w=$({steps}); then\n    \
             w=$(printf '%s\\n' \"$w\" | tail -n 1); w=${{w:-a restore step failed}}\n    \
             moor_cache_clear {dirs} || w=\"$w; partial contents may remain\"\n    \
             printf '%s%s\\n' {emptied} \"$w\"\n  \
           fi{after}\n\
         fi"
    )
}

/// A volume the script emptied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Emptied {
    /// The volume.
    pub volume: String,
    /// Why, as the failed step printed it.
    pub reason: String,
}

/// The volumes `stdout` of the script names as emptied.
#[must_use]
pub fn emptied(stdout: &str) -> Vec<Emptied> {
    stdout
        .lines()
        .filter_map(|l| l.trim().strip_prefix(EMPTIED)?.split_once(": "))
        .filter(|(n, _)| CacheName::parse(n).is_ok())
        .map(|(n, why)| Emptied {
            volume: n.to_owned(),
            reason: why.to_owned(),
        })
        .collect()
}
