//! Opt-in check against a real Mac over crabbox's static `ssh` provider.
//!
//! Runs only when `MOOR_REAL_MAC_HOST` names the Mac (`user@host`); otherwise
//! it returns at once, so default runs never touch a host.
#![expect(
    clippy::print_stderr,
    reason = "an opt-in run reports its evidence on stderr"
)]

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;

use moor_core::name::{BoxHandle, ProviderId, TaskId, TaskName};
use moor_core::seam::driver::{AcquireReq, Acquired, Driver};

use super::Crabbox;
use crate::adapters::proc::{SystemProc, ToolPaths};
use crate::env::Env;
use crate::ops::common::host_cleanup;

/// Variable that names the Mac and turns the check on.
pub(super) const HOST_VAR: &str = "MOOR_REAL_MAC_HOST";
/// The only directory on the Mac the check may create or remove things in.
pub(super) const WORK_ROOT: &str = "/Users/newman/.moor-boxes";

/// Runs `test -e path` on the Mac over plain ssh.
pub(super) fn exists_on_mac(target: &str, path: &str) -> bool {
    Command::new("ssh")
        .args(["-o", "BatchMode=yes", target, "test", "-e", path])
        .status()
        .is_ok_and(|s| s.success())
}

/// The address ssh would dial for `target`; crabbox does not read `~/.ssh/config`.
pub(super) fn dial_host(target: &str) -> String {
    let out = Command::new("ssh").args(["-G", target]).output().unwrap();
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .find_map(|l| l.strip_prefix("hostname ").map(str::to_owned))
        .unwrap()
}

/// Frees every lease the check made, also when an assertion fails.
pub(super) struct Leases<'a> {
    /// Driver that made the leases.
    pub(super) driver: &'a Crabbox,
    /// Mac as `user@host`.
    pub(super) target: &'a str,
    /// Handles to release.
    pub(super) handles: Vec<BoxHandle>,
    /// Lease directory names under [`WORK_ROOT`] to remove.
    pub(super) dirs: Vec<String>,
}

impl Drop for Leases<'_> {
    fn drop(&mut self) {
        for h in &self.handles {
            let _ = self.driver.release(h);
        }
        for dir in self.dirs.iter().filter(|d| d.starts_with("moor-")) {
            let path = format!("{WORK_ROOT}/{dir}");
            let _ = Command::new("ssh")
                .args(["-o", "BatchMode=yes", self.target, "rm", "-rf", "--", &path])
                .stdout(Stdio::null())
                .status();
        }
    }
}

/// Acquires a Box for task `n` named `name`.
pub(super) fn acquire(
    driver: &Crabbox,
    leases: &mut Leases<'_>,
    n: i64,
    name: &str,
    repo: &Path,
) -> Acquired {
    let req = AcquireReq {
        provider: ProviderId::new("ssh"),
        task: TaskId::new(n),
        name: TaskName::parse(name).unwrap(),
        class: None,
        nonce: format!("{:x}", std::process::id()),
        worktree: Some(repo.to_path_buf()),
    };
    let planned = driver.planned_handle(&req);
    leases
        .dirs
        .push(host_cleanup::box_name(&planned).to_owned());
    leases.handles.push(planned);
    driver.acquire(&req).unwrap()
}

/// A one-commit git repo that crabbox can take as the worktree.
pub(super) fn fixture_repo(dir: &Path) {
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?} failed");
    };
    git(&["init", "-q"]);
    git(&["commit", "-q", "--allow-empty", "-m", "fixture"]);
}

/// The environment crabbox runs in: the Mac as a macOS static host under [`WORK_ROOT`].
pub(super) fn crabbox_env(target: &str) -> Env {
    let user = target.split_once('@').unwrap().0;
    Env::from_pairs(
        std::env::vars()
            .filter(|(k, _)| matches!(k.as_str(), "PATH" | "HOME" | "SSH_AUTH_SOCK"))
            .chain([
                ("CRABBOX_TARGET".to_owned(), "macos".to_owned()),
                ("CRABBOX_STATIC_HOST".to_owned(), dial_host(target)),
                ("CRABBOX_STATIC_USER".to_owned(), user.to_owned()),
                ("CRABBOX_STATIC_WORK_ROOT".to_owned(), WORK_ROOT.to_owned()),
            ]),
    )
}

/// The lease directory above a Box's workdir.
pub(super) fn lease_of(acq: &Acquired) -> String {
    let (lease, _) = acq.workdir.as_str().rsplit_once('/').unwrap();
    lease.to_owned()
}

/// Cleans up for `acq` as the Task named `name` with numeric id `id`, and reports whether its lease is gone.
fn clean(
    driver: &Crabbox,
    target: &str,
    acq: &Acquired,
    name: &str,
    id: &str,
    repo: &Path,
) -> bool {
    host_cleanup::run(
        driver,
        &acq.handle,
        name,
        id,
        acq.workdir.as_str(),
        Some(repo),
    )
    .unwrap();
    !exists_on_mac(target, &lease_of(acq))
}

#[test]
fn macos_box_over_static_ssh_gets_per_task_workdirs_and_cleanup() {
    let Ok(target) = std::env::var(HOST_VAR) else {
        eprintln!("skip: {HOST_VAR} is not set");
        return;
    };
    let env = crabbox_env(&target);
    let driver = Crabbox::fake_unchecked(Arc::new(SystemProc::new(&env, &ToolPaths::default())));
    let repo = tempfile::tempdir().unwrap();
    fixture_repo(repo.path());
    let mut leases = Leases {
        driver: &driver,
        target: &target,
        handles: Vec::new(),
        dirs: Vec::new(),
    };

    let a = acquire(&driver, &mut leases, 987_654_321, "realmac-a", repo.path());
    let b = acquire(&driver, &mut leases, 987_654_322, "realmac-b", repo.path());
    for (n, acq) in [("a", &a), ("b", &b)] {
        eprintln!("workdir {n}: {}", acq.workdir.as_str());
        eprintln!("platform {n}: {:?}", acq.platform);
        assert_eq!(acq.platform.os, "Darwin");
        assert_eq!(acq.platform.arch, "arm64");
        assert!(acq.workdir.as_str().starts_with(&format!("{WORK_ROOT}/")));
        assert!(exists_on_mac(&target, &lease_of(acq)));
    }
    assert_ne!(a.workdir, b.workdir);

    let a_gone = clean(&driver, &target, &a, "realmac-a", "987654321", repo.path());
    let b_kept = exists_on_mac(&target, &lease_of(&b));
    eprintln!("a gone after its cleanup: {a_gone}");
    eprintln!("b kept after a's cleanup: {b_kept}");
    assert!(a_gone && b_kept);

    let b_gone = clean(&driver, &target, &b, "realmac-b", "987654322", repo.path());
    eprintln!("b gone after its cleanup: {b_gone}");
    assert!(b_gone);
}
