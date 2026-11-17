//! The agent's status: what it is, where it runs, and the last ops it served, kept in memory.

use std::collections::VecDeque;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use moor_client::link::status::{clip, Served, Status, MAX_ERROR, MAX_RECENT};
use tokio::process::Command;

use crate::serve::Machine;

/// How long `df` may take.
const PROBE: Duration = Duration::from_secs(10);

/// The last [`MAX_RECENT`] ops served, oldest first.
#[derive(Debug, Default)]
pub(crate) struct Recent(Mutex<VecDeque<Served>>);

impl Recent {
    /// Notes that `op` naming `target` ended, failing with `error` when given.
    pub(crate) fn record(&self, op: &str, target: Option<String>, error: Option<&str>) {
        let served = Served {
            op: op.to_owned(),
            target,
            error: error.map(|e| clip(e, MAX_ERROR)),
            at: crate::unix_now(),
        };
        let mut recent = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if recent.len() >= MAX_RECENT {
            recent.pop_front();
        }
        recent.push_back(served);
    }

    /// Every op noted, oldest first.
    fn list(&self) -> Vec<Served> {
        let recent = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        recent.iter().cloned().collect()
    }
}

/// `machine`'s status, with the ops in `recent`.
pub(crate) async fn status(machine: &Machine, recent: &Recent) -> Status {
    Status {
        version: crate::VERSION.to_owned(),
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        free_disk_gb: free_gb(&machine.home).await,
        recent: recent.list(),
    }
}

/// Whole GB free on `home`'s filesystem, from `df -Pk`.
async fn free_gb(home: &Path) -> Option<u64> {
    let mut df = Command::new("df");
    df.arg("-Pk").arg(home);
    let out = probe(df).await.ok()?;
    let kb: u64 = out
        .lines()
        .nth(1)?
        .split_whitespace()
        .nth(3)?
        .parse()
        .ok()?;
    Some(kb / (1024 * 1024))
}

/// What `cmd` printed, when it exits 0 within [`PROBE`].
async fn probe(mut cmd: Command) -> Result<String, String> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let out = tokio::time::timeout(PROBE, cmd.output())
        .await
        .map_err(|_| "it took too long".to_owned())?
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, reason = "tests")]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn machine() -> Machine {
        Machine {
            home: std::env::temp_dir(),
            shell: PathBuf::from("/bin/sh"),
            hooks: PathBuf::from("/nonexistent"),
            allow_exec: false,
            sweep_grace: std::time::Duration::ZERO,
        }
    }

    #[test]
    fn recent_keeps_the_last_ops_and_clips_errors() {
        let recent = Recent::default();
        for n in 0..MAX_RECENT + 5 {
            recent.record("exec", Some(n.to_string()), None);
        }
        recent.record("dial", Some("moor-7".to_owned()), Some(&"x".repeat(500)));
        let list = recent.list();
        assert_eq!(list.len(), MAX_RECENT);
        assert_eq!(list[0].target.as_deref(), Some("6"));
        let newest = list.last().unwrap();
        assert_eq!(newest.op, "dial");
        assert_eq!(newest.error.as_deref().unwrap().len(), MAX_ERROR);
    }

    #[tokio::test]
    async fn an_ordinary_machine_reports_no_pool() {
        let recent = Recent::default();
        recent.record("hook", Some("7".to_owned()), None);
        let status = status(&machine(), &recent).await;
        assert_eq!(status.version, crate::VERSION);
        assert_eq!(status.os, std::env::consts::OS);
        assert!(status.free_disk_gb.is_some());
        assert_eq!(status.recent.len(), 1);
    }
}
