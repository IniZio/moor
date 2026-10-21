//! The instruction files Claude Code loads on its own: the machine's managed one, and those in
//! its working directory and every ancestor.

use std::io::ErrorKind::{NotADirectory, NotFound};
use std::path::{Path, PathBuf};

/// The managed instruction file Claude Code loads on Linux for every user on the machine.
pub(super) const MANAGED: &str = "/etc/claude-code/CLAUDE.md";

/// Files Claude Code loads as instructions from its working directory and every ancestor. It
/// reads the two `AGENTS.md` names when it finds no `CLAUDE.md` file, which is the default.
const FILES: [&str; 5] = [
    "CLAUDE.md",
    "CLAUDE.local.md",
    ".claude/CLAUDE.md",
    "AGENTS.md",
    ".claude/AGENTS.md",
];

/// The first instruction file a turn in `workdir` would load, with `managed` as the machine's
/// managed file, or the first one that cannot be checked; `None` when there is none.
pub(super) fn loaded(workdir: &Path, managed: &Path) -> Option<String> {
    let ancestors = workdir
        .ancestors()
        .flat_map(|dir| FILES.iter().map(move |name| dir.join(name)));
    std::iter::once(managed.to_owned())
        .chain(ancestors)
        .find_map(|path: PathBuf| match std::fs::symlink_metadata(&path) {
            Ok(_) => Some(format!("{} would load as instructions", path.display())),
            Err(e) if matches!(e.kind(), NotFound | NotADirectory) => None,
            Err(e) => Some(format!("cannot check {}: {e}", path.display())),
        })
}
