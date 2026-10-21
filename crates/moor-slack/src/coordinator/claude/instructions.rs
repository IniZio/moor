//! The instruction sources a turn would still load: the machine's managed file and managed
//! settings, and the files and rules in its working directory. The turn runs in `/` with
//! `CLAUDE_CODE_DISABLE_CLAUDE_MDS=1`, so no other directory is read.

use std::io::ErrorKind::{NotADirectory, NotFound};
use std::path::Path;

/// The managed instruction file Claude Code loads on Linux for every user on the machine.
pub(super) const MANAGED: &str = "/etc/claude-code/CLAUDE.md";

/// Files Claude Code loads as instructions from its working directory. It reads the two
/// `AGENTS.md` names when it finds no `CLAUDE.md` file, which is the default.
const FILES: [&str; 5] = [
    "CLAUDE.md",
    "CLAUDE.local.md",
    ".claude/CLAUDE.md",
    "AGENTS.md",
    ".claude/AGENTS.md",
];

/// The managed settings file Claude Code reads on Linux. Its `claudeMd` key becomes managed
/// instructions.
pub(super) const MANAGED_SETTINGS: &str = "/etc/claude-code/managed-settings.json";

/// The first instruction source a turn in `cwd` would load, or the first one that cannot be
/// checked; `None` when there is none. `managed` is the machine's managed instruction file and
/// `settings` its managed settings file. A `.claude/rules/` directory in `cwd` counts when it
/// holds any `.md` file, however deep and scoped or not.
pub(super) fn loaded(cwd: &Path, managed: &Path, settings: &Path) -> Option<String> {
    std::iter::once(file(managed))
        .chain(std::iter::once_with(|| claude_md_setting(settings)))
        .chain(FILES.iter().map(|name| file(&cwd.join(name))))
        .chain(std::iter::once_with(|| rules(&cwd.join(".claude/rules"))))
        .find_map(|found| found)
}

/// Whether the instruction file `path` exists.
fn file(path: &Path) -> Option<String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Some(format!("{} would load as instructions", path.display())),
        Err(e) => unreadable(path, &e),
    }
}

/// A "cannot check" line for a failure other than a missing path.
fn unreadable(path: &Path, e: &std::io::Error) -> Option<String> {
    (!matches!(e.kind(), NotFound | NotADirectory))
        .then(|| format!("cannot check {}: {e}", path.display()))
}

/// Whether the managed settings file at `path` carries a `claudeMd` key.
fn claude_md_setting(path: &Path) -> Option<String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) => return unreadable(path, &e),
    };
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(json) if json.get("claudeMd").is_some() => Some(format!(
            "{} sets claudeMd, which would load as instructions",
            path.display()
        )),
        Ok(_) => None,
        Err(e) => Some(format!("cannot check {}: {e}", path.display())),
    }
}

/// A `.md` file anywhere under `dir`. A symlink counts too: it is not followed, so a loop cannot
/// hang the walk.
fn rules(dir: &Path) -> Option<String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => return unreadable(dir, &e),
    };
    for entry in entries {
        let found = entry.map_err(|e| e.to_string()).and_then(|entry| {
            let path = entry.path();
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            let md = path
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("md"));
            if kind.is_symlink() || (md && !kind.is_dir()) {
                Ok(Some(format!(
                    "{} would load as instructions",
                    path.display()
                )))
            } else if kind.is_dir() {
                Ok(rules(&path))
            } else {
                Ok(None)
            }
        });
        match found {
            Ok(None) => {}
            Ok(Some(why)) => return Some(why),
            Err(e) => return Some(format!("cannot check {}: {e}", dir.display())),
        }
    }
    None
}
