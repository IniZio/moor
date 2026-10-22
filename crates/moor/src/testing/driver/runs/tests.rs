//! The fake's detached runs, played by the Box's run scripts under the local `/bin/sh`.

use std::fs;
use std::time::Duration;

use moor_core::boxscript::record::MOVED_FILE;
use moor_core::seam::driver::{
    Cancelled, Driver, DriverError, ExecSpec, FollowEnd, Followed, Launched, RunEnd, RunOutcome,
};
use moor_core::seam::proc::Deadline;
use moor_core::{BoxHandle, BoxPath};
use tempfile::TempDir;

use super::super::FakeDriver;
use super::{eventually, live_in_group};

struct Local {
    dir: TempDir,
    driver: FakeDriver,
}

fn local() -> Local {
    let dir = tempfile::tempdir().unwrap();
    let (home, work) = (dir.path().join("home"), dir.path().join("work"));
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&work).unwrap();
    let driver = FakeDriver::new().local_runs(&home, &work);
    Local { dir, driver }
}

fn h() -> BoxHandle {
    BoxHandle::new("box-1")
}

impl Local {
    fn launch(&self, run: i64, script: &str) -> Result<Launched, DriverError> {
        let spec = ExecSpec {
            argv: vec!["sh".to_owned(), "-c".to_owned(), script.to_owned()],
            env: vec![("WHO".to_owned(), "the run".to_owned())],
            cwd: None,
            output: None,
        };
        self.driver.launch_run(&h(), run, &spec)
    }

    fn follow(&self, run: i64) -> (Result<Followed, DriverError>, String) {
        let mut out = Vec::new();
        let deadline = Deadline(Duration::from_secs(20));
        let r = self.driver.follow_run(&h(), run, 0, &mut out, deadline);
        (r, String::from_utf8(out).unwrap())
    }

    fn end(&self, run: i64) -> Result<RunEnd, DriverError> {
        let work = BoxPath::restore("/work/cbx_1/repo").unwrap();
        self.driver.end_run(&h(), run, &work)
    }
}

#[test]
fn launch_then_follow_prints_the_whole_log_and_the_exit() {
    let l = local();
    let script = r#"printf 'hello %s\n' "$WHO"; sleep 1; printf 'from %s\n' "$PWD"; exit 3"#;
    assert!(matches!(l.launch(1, script), Ok(Launched::Started { .. })));
    assert_eq!(l.launch(1, script), Ok(Launched::AlreadyLaunched));
    let (followed, text) = l.follow(1);
    let want = format!(
        "hello the run\nfrom {}\n",
        l.dir.path().join("work").display()
    );
    assert_eq!(text, want);
    let offset = u64::try_from(want.len()).unwrap();
    assert_eq!(
        followed,
        Ok(Followed {
            offset,
            end: FollowEnd::Exited
        })
    );
    assert_eq!(
        l.end(1),
        Ok(RunEnd {
            outcome: RunOutcome::Exited(3),
            cancelled: false,
            moved: false,
        })
    );
    assert_eq!(
        l.driver.calls(),
        [
            "launch_run box-1 1",
            "launch_run box-1 1",
            "follow_run box-1 1 0",
            "end_run box-1 1"
        ]
    );
}

#[test]
fn cancel_ends_the_run_group_and_end_reads_it_cancelled() {
    let l = local();
    let Launched::Started { pid } = l.launch(2, "sleep 30 & sleep 30").unwrap() else {
        unreachable!("a fresh run id starts");
    };
    assert!(
        eventually(|| live_in_group(pid) > 2),
        "leader and two sleeps"
    );
    assert_eq!(l.driver.cancel_run(&h(), 2, 2), Ok(Cancelled::Signalled));
    assert!(
        eventually(|| live_in_group(pid) == 0),
        "the group outlived cancel"
    );
    assert!(l.end(2).unwrap().cancelled);
    assert_eq!(l.driver.cancel_run(&h(), 9, 2), Ok(Cancelled::NoProcess));
}

#[test]
fn end_reads_the_moved_marker_in_the_workdir() {
    let l = local();
    l.launch(3, "exit 0").unwrap();
    assert_eq!(l.follow(3).0.map(|f| f.end), Ok(FollowEnd::Exited));
    let marker = l.dir.path().join("work").join(MOVED_FILE);
    fs::create_dir_all(marker.parent().unwrap()).unwrap();
    fs::write(marker, "").unwrap();
    assert_eq!(
        l.end(3),
        Ok(RunEnd {
            outcome: RunOutcome::Exited(0),
            cancelled: false,
            moved: true,
        })
    );
}

#[test]
fn without_local_runs_the_fake_does_not_support_them() {
    let d = FakeDriver::new();
    assert_eq!(
        d.cancel_run(&h(), 1, 1),
        Err(DriverError::Unsupported("detached runs"))
    );
}
