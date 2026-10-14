//! Detached daemon spawns for `ScriptedProc`.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};

use moor_core::seam::proc::{ProcSpec, Stdin};

use super::{Call, ScriptedProc};

/// A recorded daemon spawn.
#[derive(Clone, Debug)]
pub(crate) struct DaemonCall {
    /// The spawn request.
    pub(crate) call: Call,
    /// Log file the daemon was told to write.
    pub(crate) log: PathBuf,
    /// Pid handed back to the caller.
    pub(crate) pid: u32,
}

/// Callback run on each daemon spawn.
type Hook = Arc<dyn Fn(&DaemonCall) + Send + Sync>;

/// Daemon spawns and the optional spawn callback.
#[derive(Default)]
pub(super) struct Daemons {
    spawned: Mutex<Vec<DaemonCall>>,
    hook: Mutex<Option<Hook>>,
}

/// The `Call` a spawn request records.
pub(super) fn call_of(spec: &ProcSpec) -> Call {
    Call {
        tool: spec.tool,
        args: spec.arg_strings(),
        env: spec.env.extra.clone(),
        cwd: spec.cwd.clone(),
        deadline: spec.deadline,
        stdin: match &spec.stdin {
            Stdin::Bytes(b) => b.clone(),
            _ => Vec::new(),
        },
    }
}

impl ScriptedProc {
    /// Run `hook` after each daemon spawn is recorded, replacing any earlier
    /// hook. No internal lock is held while it runs, so it may call back into
    /// this proc.
    pub(crate) fn on_daemon(&self, hook: impl Fn(&DaemonCall) + Send + Sync + 'static) -> &Self {
        *self
            .daemons
            .hook
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(hook));
        self
    }

    /// Daemon spawns so far; they are not in `calls()`.
    pub(crate) fn daemons(&self) -> Vec<DaemonCall> {
        self.daemons
            .spawned
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(super) fn spawn_daemon_call(&self, spec: &ProcSpec, log: &Path) -> u32 {
        let daemon = DaemonCall {
            call: call_of(spec),
            log: log.to_path_buf(),
            pid: self.next_pid.fetch_add(1, Ordering::Relaxed),
        };
        self.daemons
            .spawned
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(daemon.clone());
        let hook = self
            .daemons
            .hook
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(hook) = hook {
            hook(&daemon);
        }
        daemon.pid
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use moor_core::seam::proc::{Deadline, Proc, Tool};

    use super::*;

    #[test]
    fn daemon_runs_hook_and_is_not_a_call() {
        let p = ScriptedProc::new();
        let seen = Arc::new(AtomicUsize::new(0));
        let hits = Arc::clone(&seen);
        p.on_daemon(move |d| {
            assert_eq!(d.call.args, ["serve"]);
            assert_eq!(d.log, Path::new("/tmp/d.log"));
            hits.fetch_add(1, Ordering::Relaxed);
        });
        let spec = ProcSpec::new(Tool::Git, Deadline(Duration::from_secs(1))).args(["serve"]);
        let a = p.spawn_daemon(&spec, Path::new("/tmp/d.log")).unwrap();
        let b = p.spawn_daemon(&spec, Path::new("/tmp/d.log")).unwrap();
        assert!(b > a);
        assert_eq!(seen.load(Ordering::Relaxed), 2);
        assert_eq!(p.daemons().len(), 2);
        assert!(p.calls().is_empty());
        assert!(p.alive(a));
        p.dead(a);
        assert!(!p.alive(a));
        assert!(p.alive(b));
    }
}
