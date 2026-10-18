use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moor_core::seam::agent_profile::NOTHING_TO_ARCHIVE;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, ExecOutcome};
use moor_core::seam::engine::{Engine, EngineError, SessionRef};
use moor_core::seam::proc::Deadline;
use moor_core::{BoxHandle, BoxPath, ProviderId, TaskId, TaskName};

use super::{decode, stream_script, ARCHIVE, STREAM_CAP, TOO_LARGE};
use crate::adapters::box_agent::claude::Claude;
use crate::adapters::box_agent::gh_release::fake::offline_release;
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
        warnings: Vec::new(),
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
fn archives_in_the_workdir_then_streams_the_archive_over_the_batch() {
    let d = live(FakeDriver::new().run_stdouts(["", "YWJj"]));
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out");
    run(&d, &dest).unwrap();
    let execs = d.execs();
    assert_eq!(execs.len(), 2);
    let (_, spec, _) = execs.first().unwrap();
    let script = spec.argv.last().unwrap();
    assert!(script.starts_with("cd '/work/box-1' && "), "{script}");
    assert!(script.contains(".claude/projects"));
    assert!(script.contains(".moor/transcripts.tgz"));
    assert_eq!(spec.host_cwd, Some(PathBuf::from("/host/wt")));
    assert_eq!(std::fs::read(&dest).unwrap(), b"abc");
    assert!(d.copies().is_empty());
}

#[test]
fn an_archive_over_the_cap_is_copied_out_instead() {
    let d = live(FakeDriver::new().run_stdouts(["", &format!("{TOO_LARGE}\n")]));
    let dest = tempfile::tempdir().unwrap().path().join("out");
    run(&d, &dest).unwrap();
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
fn a_refused_stream_step_fails_without_a_copy() {
    let d = live(FakeDriver::new().exec_results([
        Ok(ExecOutcome::Exited { code: 0 }),
        Ok(ExecOutcome::Exited { code: 4 }),
    ]));
    let e = run(&d, Path::new("/x")).unwrap_err();
    assert_eq!(e.to_string(), "transcript: stream: exit 4");
    assert!(d.copies().is_empty());
}

#[test]
fn an_existing_destination_is_not_overwritten() {
    let d = live(FakeDriver::new().run_stdouts(["", "YWJj"]));
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out");
    std::fs::write(&dest, "old").unwrap();
    assert!(run(&d, &dest).is_err());
    assert_eq!(std::fs::read(&dest).unwrap(), b"old");
}

/// Runs the stream step in `dir` with `cap`.
fn stream(dir: &Path, cap: u64) -> std::process::Output {
    let script = stream_script(dir.to_str().unwrap(), cap);
    std::process::Command::new("sh")
        .args(["-c", &script])
        .output()
        .unwrap()
}

fn archive_dir(bytes: &[u8]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".moor")).unwrap();
    std::fs::write(dir.path().join(ARCHIVE), bytes).unwrap();
    dir
}

#[test]
fn binary_bytes_with_nul_round_trip_through_the_stream_step() {
    let bytes: Vec<u8> = (0..=255u8).chain([0, 0, 1, 0]).collect();
    let dir = archive_dir(&bytes);
    let out = stream(dir.path(), STREAM_CAP);
    assert!(out.status.success());
    assert_eq!(decode(&String::from_utf8(out.stdout).unwrap()), Some(bytes));
}

#[test]
fn a_symlinked_archive_is_refused() {
    let dir = archive_dir(b"x");
    std::fs::write(dir.path().join("secret"), "s").unwrap();
    std::fs::remove_file(dir.path().join(ARCHIVE)).unwrap();
    std::os::unix::fs::symlink(dir.path().join("secret"), dir.path().join(ARCHIVE)).unwrap();
    let out = stream(dir.path(), STREAM_CAP);
    assert_eq!(out.status.code(), Some(4));
    assert!(out.stdout.is_empty());
}

#[test]
fn an_archive_over_the_cap_prints_the_marker_not_the_bytes() {
    let dir = archive_dir(&[7u8; 100]);
    let out = stream(dir.path(), 99);
    assert!(out.status.success());
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), TOO_LARGE);
    let at = stream(dir.path(), 100);
    assert_ne!(String::from_utf8(at.stdout).unwrap().trim(), TOO_LARGE);
}

#[test]
fn decode_rejects_malformed_text() {
    assert_eq!(decode("YWJj"), Some(b"abc".to_vec()));
    assert_eq!(decode("YW Jj\n"), Some(b"abc".to_vec()));
    assert_eq!(decode("YQ=="), Some(b"a".to_vec()));
    assert_eq!(decode("Y!=="), None);
    assert_eq!(decode("YQ==YQ"), None);
}

#[test]
fn the_archive_runs_in_one_batch_not_a_separate_exec() {
    let d = live(FakeDriver::new().run_stdouts(["", "YWJj"]));
    let dir = tempfile::tempdir().unwrap();
    run(&d, &dir.path().join("out")).unwrap();
    assert_eq!(d.box_calls(), ["acquire", "connect_argv", "batch"]);
}

#[test]
fn the_archive_deadline_is_fifteen_seconds() {
    let d = live(FakeDriver::new().run_stdouts(["", "YWJj"]));
    let dir = tempfile::tempdir().unwrap();
    run(&d, &dir.path().join("out")).unwrap();
    let execs = d.execs();
    assert_eq!(execs.first().unwrap().2, Deadline(Duration::from_secs(15)));
}

#[test]
fn a_failed_archive_copies_nothing() {
    let d = live(FakeDriver::new().exec_results([Ok(ExecOutcome::Exited { code: 2 })]));
    let e = run(&d, Path::new("/x")).unwrap_err();
    assert_eq!(e.to_string(), "transcript: archive: exit 2");
    assert!(d.copies().is_empty());
}

#[test]
fn nothing_to_archive_is_not_a_failure_and_copies_nothing() {
    let d = live(FakeDriver::new().run_stdout(&format!("{NOTHING_TO_ARCHIVE}\n")));
    let e = run(&d, Path::new("/x")).unwrap_err();
    assert!(matches!(e, EngineError::NothingToArchive), "{e}");
    assert!(d.copies().is_empty());
}

#[test]
fn a_failed_copy_is_reported() {
    let d = live(
        FakeDriver::new()
            .run_stdouts(["", &format!("{TOO_LARGE}\n")])
            .fail_copy_out(DriverError::Failed {
                summary: "boom".into(),
            }),
    );
    let e = run(&d, Path::new("/x")).unwrap_err();
    assert!(e.to_string().starts_with("transcript: copy: "), "{e}");
}
