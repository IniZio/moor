use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, EgressPlan, ExecOutcome};
use moor_core::seam::engine::{Engine, EngineError, SessionRef};
use moor_core::{BoxHandle, BoxPath, ProviderId, TaskId, TaskName};

use crate::adapters::box_agent::claude::Claude;
use crate::adapters::box_agent::nono_release::fake::offline_release;
use crate::adapters::box_agent::BoxAgent;
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;
use crate::testing::surface::FakeSurface;

fn live(driver: FakeDriver) -> Arc<FakeDriver> {
    let d = Arc::new(driver);
    d.acquire(&AcquireReq {
        provider: ProviderId::new("fake"),
        task: TaskId::new(1),
        name: TaskName::parse("t1").unwrap(),
        egress: EgressPlan::Open,
        class: None,
        nonce: "n0".to_owned(),
        worktree: None,
    })
    .unwrap();
    d
}

fn session() -> SessionRef {
    SessionRef {
        id: "s1".to_owned(),
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::under("/work", "box-1").unwrap(),
        worktree: PathBuf::from("/host/wt"),
        pane: None,
        sandbox: None,
    }
}

fn run(d: &Arc<FakeDriver>, dest: &Path) -> Result<(), EngineError> {
    BoxAgent::new(
        d.clone(),
        Arc::new(FakeSurface::new("/r")),
        Arc::new(Claude),
        Arc::new(FakeClock::new(Ts::from_millis(0))),
        offline_release(),
    )
    .transcript(&session(), dest)
}

#[test]
fn archives_in_the_workdir_then_copies_the_archive_out() {
    let d = live(FakeDriver::new());
    let dest = tempfile::tempdir().unwrap().path().join("out");
    run(&d, &dest).unwrap();
    let execs = d.execs();
    assert_eq!(execs.len(), 1);
    let spec = &execs.first().unwrap().1;
    let script = spec.argv.last().unwrap();
    assert!(script.contains(".claude/projects"));
    assert!(script.contains(".moor/transcripts.tgz"));
    assert_eq!(spec.cwd, None);
    assert_eq!(spec.host_cwd, Some(PathBuf::from("/host/wt")));
    assert_eq!(
        d.copies(),
        [(
            BoxHandle::new("box-1"),
            BoxPath::under("/work/box-1", ".moor/transcripts.tgz").unwrap(),
            dest,
            PathBuf::from("/host/wt")
        )]
    );
}

#[test]
fn a_failed_archive_copies_nothing() {
    let d = live(FakeDriver::new().exec_results([Ok(ExecOutcome::Exited { code: 3 })]));
    let e = run(&d, Path::new("/x")).unwrap_err();
    assert_eq!(e.to_string(), "transcript: archive: exit 3");
    assert!(d.copies().is_empty());
}

#[test]
fn a_failed_copy_is_reported() {
    let d = live(FakeDriver::new().fail_copy_out(DriverError::Failed {
        summary: "boom".into(),
    }));
    let e = run(&d, Path::new("/x")).unwrap_err();
    assert!(e.to_string().starts_with("transcript: copy: "), "{e}");
}
