//! Detached-run half of the Driver fake: runs the Box's run scripts with the local `/bin/sh`.
//!
//! A test that signals a run's process group runs inside a pid namespace, whose pid 1 does not
//! reap orphans: a killed run leader stays a zombie there, which the scripts read as gone.

use std::io::{Read as _, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;

use moor_core::boxscript::detached::{
    cancel_script, end_script, follow_script, launch_script, parse_end, LOST, REFUSED,
};
use moor_core::boxscript::quote;
use moor_core::seam::driver::{
    Cancelled, DriverError, ExecSpec, FollowEnd, Followed, Launched, RunEnd,
};
use moor_core::BoxHandle;

use super::{ExecHook, FakeDriver};

/// What `launch_run` returns.
pub(super) type Launch = Result<Launched, DriverError>;

/// The detached-run half of the fake's state.
#[derive(Default)]
pub(super) struct RunState {
    /// Where runs play out; `None` leaves detached runs unsupported.
    local: Option<LocalBox>,
    /// Bytes after which the next follow is cut.
    cut_follow: Option<u64>,
    /// Bytes after which every follow is cut.
    cut_every_follow: Option<u64>,
    /// Runs in each follow once it ended.
    after_follow: Option<Arc<ExecHook>>,
    /// Runs in each cancel before it reaches the run.
    before_cancel: Option<Arc<ExecHook>>,
    /// Every launch: handle and spec.
    launches: Vec<(BoxHandle, ExecSpec)>,
    /// The error the next launch fails with.
    fail_launch: Option<DriverError>,
}

/// A local stand-in for a Box: its `HOME` and its workdir.
#[derive(Clone)]
pub(super) struct LocalBox {
    /// Where the run dirs go, under `.moor/runs`.
    home: PathBuf,
    /// Where every run starts and `end_run` looks for the moved marker.
    workdir: PathBuf,
}

impl LocalBox {
    /// `sh -c script` in the workdir, with `HOME` the Box's, `$HOME/bin` first on `PATH`, and
    /// nothing else from this host.
    fn sh(&self, script: &str) -> Command {
        let mut path = self.home.join("bin").into_os_string();
        path.push(":/usr/bin:/bin");
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg(script)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", path)
            .current_dir(&self.workdir)
            .stdin(Stdio::null());
        cmd
    }

    fn output(&self, script: &str) -> Result<Output, DriverError> {
        self.sh(script).output().map_err(|e| failed(&e.to_string()))
    }
}

impl FakeDriver {
    /// Runs detached runs with the local `/bin/sh`: `home` stands for the Box's `HOME`, and
    /// every run starts in `workdir` whatever its spec's cwd. Without this they are unsupported.
    pub(crate) fn local_runs(self, home: &Path, workdir: &Path) -> Self {
        self.st().runs.local = Some(LocalBox {
            home: home.to_path_buf(),
            workdir: workdir.to_path_buf(),
        });
        self
    }

    /// [`Self::local_runs`] with the Box's `home` and `work` under `dir`, and guards in
    /// `$HOME/bin` that refuse `sudo` and `apt-get`.
    pub(crate) fn local_box(self, dir: &Path) -> Self {
        let (home, work) = (dir.join("home"), dir.join("work"));
        moor_testkit::write_host_guards(&home.join("bin")).expect("the Box home is writable");
        std::fs::create_dir_all(&work).expect("the Box workdir is writable");
        self.local_runs(&home, &work)
    }

    /// Every `launch_run` so far: handle and spec, in call order.
    pub(crate) fn launches(&self) -> Vec<(BoxHandle, ExecSpec)> {
        self.st().runs.launches.clone()
    }

    /// The next `launch_run` fails once with `e`, starting nothing.
    pub(crate) fn fail_launch(self, e: DriverError) -> Self {
        self.st().runs.fail_launch = Some(e);
        self
    }

    /// The next `follow_run` stops once it has written `after` bytes and reports the channel cut.
    pub(crate) fn cut_next_follow(&self, after: u64) {
        self.st().runs.cut_follow = Some(after);
    }

    /// Every `follow_run` from now on stops once it has written `after` bytes, as
    /// [`Self::cut_next_follow`].
    pub(crate) fn cut_every_follow(&self, after: u64) {
        self.st().runs.cut_every_follow = Some(after);
    }

    /// Runs `f` in each `follow_run` once its follow has ended, before it returns.
    pub(crate) fn after_follow(&self, f: impl Fn() + Send + Sync + 'static) {
        self.st().runs.after_follow = Some(Arc::new(f));
    }

    /// Runs `f` in each `cancel_run` before the cancel reaches the run.
    pub(crate) fn before_cancel(&self, f: impl Fn() + Send + Sync + 'static) {
        self.st().runs.before_cancel = Some(Arc::new(f));
    }

    fn local(&self, what: String) -> Result<LocalBox, DriverError> {
        let mut s = self.st();
        s.note(what);
        s.runs
            .local
            .clone()
            .ok_or(DriverError::Unsupported("detached runs"))
    }

    pub(super) fn local_launch(
        &self,
        h: &BoxHandle,
        run: i64,
        spec: &ExecSpec,
    ) -> Result<Launched, DriverError> {
        self.st().runs.launches.push((h.clone(), spec.clone()));
        let b = self.local(format!("launch_run {} {run}", h.as_str()))?;
        let fail = self.st().runs.fail_launch.take();
        if let Some(e) = fail {
            return Err(e);
        }
        let command: Vec<String> = spec.argv.iter().map(|a| quote(a)).collect();
        let out = b
            .sh(&launch_script(run, &command.join(" ")))
            .envs(spec.env.iter().map(|(k, v)| (k, v)))
            .output()
            .map_err(|e| failed(&e.to_string()))?;
        match out.status.code() {
            Some(0) => String::from_utf8_lossy(&out.stdout)
                .trim()
                .parse()
                .map(|pid| Launched::Started { pid })
                .map_err(|_| failed("the launch printed no pid")),
            Some(REFUSED) => Ok(Launched::AlreadyLaunched),
            _ => Err(failed(&String::from_utf8_lossy(&out.stderr))),
        }
    }

    pub(super) fn local_follow(
        &self,
        h: &BoxHandle,
        run: i64,
        from: u64,
        out: &mut dyn Write,
    ) -> Result<Followed, DriverError> {
        let b = self.local(format!("follow_run {} {run} {from}", h.as_str()))?;
        let cut = {
            let mut s = self.st();
            s.runs.cut_follow.take().or(s.runs.cut_every_follow)
        };
        let mut child = b
            .sh(&follow_script(run, from))
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| failed(&e.to_string()))?;
        let mut offset = from;
        let mut buf = [0u8; 8192];
        if let Some(mut r) = child.stdout.take() {
            while let Ok(n @ 1..) = r.read(&mut buf) {
                let room = cut.map_or(n as u64, |c| (from + c).saturating_sub(offset));
                let n = usize::try_from(room.min(n as u64)).unwrap_or(n);
                out.write_all(buf.get(..n).unwrap_or_default())
                    .map_err(|e| failed(&e.to_string()))?;
                offset += n as u64;
                if cut.is_some_and(|c| offset >= from + c) {
                    child.kill().map_err(|e| failed(&e.to_string()))?;
                    child.wait().map_err(|e| failed(&e.to_string()))?;
                    return Ok(Followed {
                        offset,
                        end: FollowEnd::Cut,
                    });
                }
            }
        }
        let end = match child.wait().map_err(|e| failed(&e.to_string()))?.code() {
            Some(0) => FollowEnd::Exited,
            Some(LOST) => FollowEnd::Lost,
            code => return Err(failed(&format!("follow exited {code:?}"))),
        };
        let hook = self.st().runs.after_follow.clone();
        if let Some(hook) = hook {
            hook();
        }
        Ok(Followed { offset, end })
    }

    pub(super) fn local_end(&self, h: &BoxHandle, run: i64) -> Result<RunEnd, DriverError> {
        let b = self.local(format!("end_run {} {run}", h.as_str()))?;
        let out = b.output(&end_script(run))?;
        parse_end(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| failed("unreadable end"))
    }

    pub(super) fn local_cancel(
        &self,
        h: &BoxHandle,
        run: i64,
        grace: u32,
    ) -> Result<Cancelled, DriverError> {
        let b = self.local(format!("cancel_run {} {run}", h.as_str()))?;
        let hook = self.st().runs.before_cancel.clone();
        if let Some(hook) = hook {
            hook();
        }
        let out = b.output(&cancel_script(run, grace))?;
        match out.status.code() {
            Some(0) => Ok(Cancelled::Signalled),
            Some(REFUSED) => Ok(Cancelled::NoProcess),
            _ => Err(failed(&String::from_utf8_lossy(&out.stderr))),
        }
    }
}

fn failed(summary: &str) -> DriverError {
    DriverError::Failed {
        summary: summary.trim().to_owned(),
    }
}

/// Polls `ok` for up to 20 s.
pub(crate) fn eventually(mut ok: impl FnMut() -> bool) -> bool {
    for _ in 0..400 {
        if ok() {
            return true;
        }
        sleep(Duration::from_millis(50));
    }
    false
}

/// Processes in group `pgid` that have not exited; zombies nobody reaped do not count.
pub(crate) fn live_in_group(pgid: u32) -> usize {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return 0;
    };
    dir.filter_map(|e| e.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| {
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
                return false;
            };
            let Some((_, rest)) = stat.rsplit_once(')') else {
                return false;
            };
            let mut fields = rest.split_whitespace();
            let state = fields.next();
            let pgrp = fields.nth(1).and_then(|g| g.parse::<u32>().ok());
            state != Some("Z") && pgrp == Some(pgid)
        })
        .count()
}

/// A pid whose process has exited and been reaped.
pub(crate) fn reaped_pid() -> u32 {
    let mut child = Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

#[cfg(test)]
mod tests;
