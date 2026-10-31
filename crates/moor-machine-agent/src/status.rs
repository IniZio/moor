//! The agent's status: what it is, where it runs, and the last ops it served, kept in memory.

use std::collections::VecDeque;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use moor_client::link;
use moor_client::link::status::{
    clip, PoolStatus, Served, Status, MAX_CRABBOX, MAX_ERROR, MAX_RECENT,
};
use tokio::process::Command;

use crate::pool::{self, Pool};
use crate::serve::Machine;

/// How long `df` or `crabbox --version` may take.
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
    let pool = match &machine.pool {
        Some(pool) => Some(pool_status(&machine.home, pool).await),
        None => None,
    };
    Status {
        version: crate::VERSION.to_owned(),
        protocol: link::PROTOCOL,
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        free_disk_gb: free_gb(&machine.home).await,
        pool,
        recent: recent.list(),
    }
}

/// A pool host's provider, crabbox release and leases.
async fn pool_status(home: &Path, pool: &Pool) -> PoolStatus {
    let mut version = Command::new(&pool.crabbox);
    version.arg("--version");
    let crabbox = match probe(version).await {
        Ok(out) => clip(out.lines().next().unwrap_or_default(), MAX_CRABBOX),
        Err(e) => clip(&format!("cannot run crabbox --version: {e}"), MAX_CRABBOX),
    };
    let (leases, error) = match pool::list(home, pool).await {
        Ok(leases) => (leases, None),
        Err(e) => (Vec::new(), Some(clip(&e, MAX_ERROR))),
    };
    PoolStatus {
        provider: pool.provider.clone(),
        crabbox,
        leases,
        error,
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

    fn machine(pool: Option<Pool>) -> Machine {
        Machine {
            home: std::env::temp_dir(),
            shell: PathBuf::from("/bin/sh"),
            hooks: PathBuf::from("/nonexistent"),
            pool,
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
        let status = status(&machine(None), &recent).await;
        assert_eq!(status.version, crate::VERSION);
        assert_eq!(status.protocol, link::PROTOCOL);
        assert_eq!(status.os, std::env::consts::OS);
        assert!(status.free_disk_gb.is_some());
        assert_eq!(status.pool, None);
        assert_eq!(status.recent.len(), 1);
    }

    #[tokio::test]
    async fn a_pool_host_reports_its_provider_and_crabbox() {
        let dir = tempfile::tempdir().unwrap();
        let crabbox = dir.path().join("crabbox");
        let long = "v".repeat(300);
        let script = format!(
            "#!/bin/sh\ncase \"$1\" in --version) echo 'crabbox {long}'; echo second;; *) echo '[]';; esac\n"
        );
        std::fs::write(&crabbox, script).unwrap();
        std::fs::set_permissions(
            &crabbox,
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        let pool = Pool {
            provider: "fake".to_owned(),
            max: None,
            cache_gb: None,
            crabbox,
        };
        let status = status(&machine(Some(pool)), &Recent::default()).await;
        let pool = status.pool.unwrap();
        assert_eq!(pool.provider, "fake");
        assert!(pool.crabbox.starts_with("crabbox vvv"), "{}", pool.crabbox);
        assert_eq!(pool.crabbox.chars().count(), MAX_CRABBOX);
        assert_eq!(pool.error, None);
        assert!(pool.leases.is_empty());
    }
}
