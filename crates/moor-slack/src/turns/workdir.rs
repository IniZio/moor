//! The thread's workdir, which holds what the agent is started with.

use std::path::{Path, PathBuf};

use crate::credentials::{daemon_uid, private_dir};
use crate::db::TurnRow;

/// The thread's workdir, `<state dir>/threads/<channel>-<thread ts>`, created mode 0700 in a
/// private parent. A channel or ts that is not safe as a path component is refused.
pub(super) fn workdir(state_dir: &Path, row: &TurnRow) -> Result<PathBuf, String> {
    let dir = workdir_path(state_dir, row)?;
    let parent = dir.parent().unwrap_or(&dir);
    for d in [parent, &dir] {
        private_dir(d, daemon_uid()).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    Ok(dir)
}

/// The path of the thread's workdir, without creating it.
fn workdir_path(state_dir: &Path, row: &TurnRow) -> Result<PathBuf, String> {
    let safe = |s: &str, extra: char| {
        !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == extra)
    };
    if !safe(&row.channel, '_') || !safe(&row.thread_ts, '.') {
        return Err("the thread id is not safe as a directory name".to_owned());
    }
    let parent = std::path::absolute(state_dir.join("threads")).map_err(|e| e.to_string())?;
    Ok(parent.join(format!("{}-{}", row.channel, row.thread_ts)))
}
