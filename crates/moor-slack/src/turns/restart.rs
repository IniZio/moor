//! What the front does with turns an earlier run left behind, before it takes new ones: it
//! kills their processes, posts each stored line not yet posted, and runs a cut-off turn once
//! more when it is recent, or says it stopped.

use std::fs;
use std::path::Path;

use rustix::process::{getpgid, getpgrp, kill_process_group, Pid, Signal};

use super::{lines, Turns};
use crate::db::{Db, End, TurnRow};

/// Oldest a cut-off turn may be, in seconds, and still run again.
const RERUN_WITHIN: i64 = 60 * 60;

impl Turns {
    /// Runs the restart steps; returns the turns that run again.
    pub(super) fn recover(&self, db: &Db) -> Vec<TurnRow> {
        kill_strays(&self.state_dir.join("threads"), &*self.log);
        let now = (self.clock)();
        let unposted = db.unposted().unwrap_or_else(|e| {
            (self.log)(&format!("turns: slack.db: {e}"));
            Vec::new()
        });
        for (row, line) in unposted {
            self.post(&row, row.placeholder.as_deref(), &line);
            self.written(db.mark_posted(row.id, now));
        }
        let unended = db.unended().unwrap_or_else(|e| {
            (self.log)(&format!("turns: slack.db: {e}"));
            Vec::new()
        });
        let mut reruns = Vec::new();
        for cut in unended {
            let row = &cut.row;
            if row.rerun_of.is_none() && now - cut.started_at < RERUN_WITHIN {
                match db.rerun(row, now) {
                    Ok(rerun) => reruns.push(rerun),
                    Err(e) => (self.log)(&format!("turns: slack.db: {e}")),
                }
                continue;
            }
            let line = lines::RESTARTED;
            self.written(db.store_reply(row.id, End::Lost, line, None, None));
            self.post(row, row.placeholder.as_deref(), line);
            self.written(db.mark_posted(row.id, now));
        }
        reruns
    }
}

/// Kills the process group of every process that runs in a thread's workdir under `threads`.
/// No turn runs while the front is down, so each one is left from an earlier run. The scan
/// reads each process's working directory, so it needs no stored pid.
fn kill_strays(threads: &Path, log: &dyn Fn(&str)) {
    let (Ok(threads), Ok(procs)) = (fs::canonicalize(threads), fs::read_dir("/proc")) else {
        return;
    };
    let own = getpgrp();
    for entry in procs.flatten() {
        let pid = entry.file_name().to_str().and_then(|s| s.parse().ok());
        let Some(pid) = pid.and_then(Pid::from_raw) else {
            continue;
        };
        let cwd = fs::read_link(entry.path().join("cwd"));
        if !cwd.is_ok_and(|cwd| cwd.starts_with(&threads)) {
            continue;
        }
        if let Some(group) = getpgid(Some(pid)).ok().filter(|g| *g != own) {
            let shown = group.as_raw_nonzero();
            log(&format!(
                "turns: killed process group {shown} left from a turn"
            ));
            let _ = kill_process_group(group, Signal::KILL);
        }
    }
}
