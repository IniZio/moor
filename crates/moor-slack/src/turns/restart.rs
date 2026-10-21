//! What the front does with turns an earlier run left behind, before it takes new ones: it
//! kills their processes, posts each stored line not yet posted, and runs a cut-off turn once
//! more when it is recent, or says it stopped and ends its thread's host Session.

use std::fs;
use std::os::unix::fs::MetadataExt as _;

use rustix::process::{getpgid, kill_process_group, Pid, Signal};

use super::{host, lines, workdir_path, Turns};
use crate::credentials::daemon_uid;
use crate::db::{Db, End, TurnRow};

/// Oldest a cut-off turn may be, in seconds, and still run again.
const RERUN_WITHIN: i64 = 60 * 60;

impl Turns {
    /// Runs the restart steps; returns the turns that run again.
    pub(super) fn recover(&self, db: &Db) -> Vec<TurnRow> {
        let now = (self.clock)();
        let unended = db.unended().unwrap_or_else(|e| {
            (self.log)(&format!("turns: slack.db: {e}"));
            Vec::new()
        });
        for (pid, started) in unended.iter().filter_map(|cut| cut.process) {
            kill_stray(pid, started, &*self.log);
        }
        let unposted = db.unposted().unwrap_or_else(|e| {
            (self.log)(&format!("turns: slack.db: {e}"));
            Vec::new()
        });
        for (row, line) in unposted {
            if self.post(&row, row.placeholder.as_deref(), &line) {
                self.written(db.mark_posted(row.id, now));
            }
        }
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
            if self.post(row, row.placeholder.as_deref(), line) {
                self.written(db.mark_posted(row.id, now));
            }
            self.end_left(db, row);
        }
        reruns
    }

    /// Ends each live host Session whose workdir is that of `row`'s thread, whose turn does
    /// not run again. A rerun adopts its thread's Session instead.
    fn end_left(&self, db: &Db, row: &TurnRow) {
        let live = workdir_path(&self.state_dir, row)
            .and_then(|dir| host::live_with(&self.client, &dir.display().to_string()));
        match live {
            Ok(live) => {
                for session in live {
                    self.end_host(db, &row.channel, &row.thread_ts, session);
                }
            }
            Err(e) => (self.log)(&format!(
                "host: thread {}/{}: {e}",
                row.channel, row.thread_ts
            )),
        }
    }
}

/// When process `pid` started, in clock ticks after boot, read from `/proc/<pid>/stat`.
pub(super) fn start_time(pid: u32) -> Option<i64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    start_time_in(&stat)
}

/// Field 22 of `/proc/<pid>/stat` text. The command name in field 2 sits in parentheses and may
/// hold spaces and `)`, so the fields count from the last `)`.
fn start_time_in(stat: &str) -> Option<i64> {
    let (_, fields) = stat.rsplit_once(')')?;
    fields.split_whitespace().nth(19)?.parse().ok()
}

/// Kills the process group of a turn's process an earlier run left behind. Only the process
/// that turn started is killed: the same pid, the same start time, this user's, and still its
/// group's leader. A process that only runs in a thread's workdir is left alone.
fn kill_stray(pid: u32, started: i64, log: &dyn Fn(&str)) {
    let ours = fs::metadata(format!("/proc/{pid}")).is_ok_and(|m| m.uid() == daemon_uid());
    if !ours || start_time(pid) != Some(started) {
        return;
    }
    let Some(group) = i32::try_from(pid).ok().and_then(Pid::from_raw) else {
        return;
    };
    if getpgid(Some(group)).ok() == Some(group) {
        log(&format!(
            "turns: killed process group {pid} left from a turn"
        ));
        let _ = kill_process_group(group, Signal::KILL);
    }
}

#[cfg(test)]
mod tests {
    use super::start_time_in;

    #[test]
    fn the_start_time_is_field_22_even_when_the_command_name_holds_a_paren() {
        let stat = "1234 (a) b) S 1 1234 1234 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 987654 \
                    12345678 100 18446744073709551615 1 1 0 0 0 0 0 0 0 0 0 0 17 3 0 0 0 0 0";
        assert_eq!(start_time_in(stat), Some(987_654));
        assert_eq!(start_time_in("no parenthesis"), None);
    }
}
