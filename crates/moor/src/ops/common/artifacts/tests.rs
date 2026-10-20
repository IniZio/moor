//! Tests for collecting artifacts out of a Box.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::fs;
use std::path::Path;
use std::sync::Arc;

use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, ExecOutcome};
use moor_core::{BoxId, BoxName, ProviderId, Warning};
use sha2::{Digest, Sha256};

mod stream;

use super::collect;
use crate::cx::Cx;
use crate::store::BoxRow;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

struct Fx {
    dir: tempfile::TempDir,
    cx: Cx,
    driver: Arc<FakeDriver>,
    id: BoxId,
}

fn fx() -> Fx {
    fx_with(FakeDriver::new())
}

fn fx_with(driver: FakeDriver) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(driver);
    let cx = Cx::fake(
        dir.path(),
        dir.path(),
        driver.clone(),
        Arc::new(FakeSurface::new("/s")),
        Arc::new(ScriptedProc::new()),
    );
    let id = admit(&cx, &driver, dir.path());
    Fx {
        dir,
        cx,
        driver,
        id,
    }
}

/// Admits Box `t` and makes it Ready with recorded workdir `/work/box-1`.
fn admit(cx: &Cx, driver: &FakeDriver, dir: &Path) -> BoxId {
    let store = cx.store().unwrap();
    let name = BoxName::parse_allow_bare("t").unwrap();
    let id = store
        .apply(Decision::insert(NewBox {
            name: name.clone(),
            repo: "r".into(),
        }))
        .unwrap()
        .box_id;
    let provider = ProviderId::new("p");
    let fields = Fields {
        box_provider: Field::Set(provider.clone()),
        worktree: Field::Set(dir.display().to_string()),
        workspace_id: Field::Set("ws-1".into()),
        box_workdir: Field::Set("/work/box-1".into()),
        ..Fields::default()
    };
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(fields);
    store.apply(d).unwrap();
    let got = driver
        .acquire(&AcquireReq {
            provider,
            box_id: id,
            name,
            class: None,
            nonce: "n".to_owned(),
            worktree: None,
        })
        .unwrap();
    let d = Decision::new(id)
        .lifecycle(BoxLifecycleInput::Ready {
            setup: SetupOutcome::Ok,
        })
        .session(SessionInput::Started)
        .box_input(BoxInput::Acquired { handle: got.handle });
    store.apply(d).unwrap();
    id
}

fn row(f: &Fx) -> BoxRow {
    f.cx.store().unwrap().box_row(f.id).unwrap().unwrap()
}

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// An index line for `name` holding `bytes`.
fn line(name: &str, bytes: &[u8]) -> String {
    format!("{name}\t{}\t{}\n", bytes.len(), sha(bytes))
}

fn codes(f: &Fx) -> Vec<String> {
    f.cx.take_warnings().into_iter().map(|w| w.code).collect()
}

fn warnings(f: &Fx) -> Vec<Warning> {
    f.cx.take_warnings()
}

fn versions(f: &Fx) -> Vec<(String, i64)> {
    f.cx.store()
        .unwrap()
        .artifacts(f.id)
        .unwrap()
        .into_iter()
        .map(|a| (a.name, a.version))
        .collect()
}

fn run(f: &Fx) -> usize {
    collect(&f.cx, f.cx.store().unwrap(), &row(f))
}

// Driver calls made after `before` calls, so setup's Box acquire is excluded.
fn collect_calls(f: &Fx, before: usize) -> Vec<String> {
    f.driver
        .box_calls()
        .get(before..)
        .unwrap_or_default()
        .to_vec()
}

#[test]
fn fresh_file_is_stored_with_an_event() {
    let f = fx();
    f.driver.push_artifact_index(line("a.md", b"hello"));
    f.driver.set_copy_text("hello");
    assert_eq!(run(&f), 1);
    assert!(codes(&f).is_empty());
    assert_eq!(versions(&f), vec![("a.md".to_owned(), 1)]);
    let events = f.cx.store().unwrap().events(f.id, usize::MAX).unwrap();
    let e = events
        .iter()
        .find(|e| e.kind == "artifact_published")
        .unwrap();
    assert_eq!(e.payload["version"], 1);
    let stored = f.dir.path().join("artifacts").join(f.id.get().to_string());
    assert_eq!(fs::read(stored.join("v1/a.md")).unwrap(), b"hello");
    assert!(!f
        .dir
        .path()
        .join("artifacts/.staging")
        .join(f.id.get().to_string())
        .exists());
}

#[test]
fn unchanged_file_is_not_copied_again() {
    let f = fx();
    f.driver.set_copy_text("hello");
    f.driver.push_artifact_index(line("a.md", b"hello"));
    assert_eq!(run(&f), 1);
    f.driver.push_artifact_index(line("a.md", b"hello"));
    assert_eq!(run(&f), 0);
    assert_eq!(f.driver.copies().len(), 1);
    assert_eq!(versions(&f).len(), 1);
}

#[test]
fn changed_file_becomes_the_next_version() {
    let f = fx();
    f.driver.set_copy_text("one");
    f.driver.push_artifact_index(line("a.md", b"one"));
    assert_eq!(run(&f), 1);
    f.driver.set_copy_text("two!");
    f.driver.push_artifact_index(line("a.md", b"two!"));
    assert_eq!(run(&f), 1);
    assert_eq!(versions(&f), vec![("a.md".into(), 1), ("a.md".into(), 2)]);
}

#[test]
fn mismatched_copy_stores_nothing_then_good_bytes_are_stored() {
    let f = fx();
    f.driver.set_copy_text("other");
    f.driver.push_artifact_index(line("a.md", b"hello"));
    assert_eq!(run(&f), 0);
    let w = warnings(&f);
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].code, "artifact_skipped");
    assert!(w[0].message.contains("a.md") && w[0].message.contains("changed"));
    assert!(!w[0].message.contains("other"));
    assert!(versions(&f).is_empty());
    f.driver.set_copy_text("hello");
    f.driver.push_artifact_index(line("a.md", b"hello"));
    assert_eq!(run(&f), 1);
    assert_eq!(versions(&f), vec![("a.md".to_owned(), 1)]);
}

#[test]
fn size_mismatch_is_refused() {
    let f = fx();
    f.driver.set_copy_text("hello");
    f.driver
        .push_artifact_index(format!("a.md\t4\t{}\n", sha(b"hello")));
    assert_eq!(run(&f), 0);
    assert_eq!(codes(&f), vec!["artifact_skipped"]);
}

#[test]
fn copy_failure_warns_and_a_later_collect_stores() {
    let f = fx_with(FakeDriver::new().fail_copy_out(DriverError::Failed {
        summary: "boom".into(),
    }));
    f.driver.set_copy_text("hello");
    f.driver.push_artifact_index(line("a.md", b"hello"));
    assert_eq!(run(&f), 0);
    let w = warnings(&f);
    assert_eq!(w[0].code, "artifact_skipped");
    assert!(w[0].message.contains("a.md") && w[0].message.contains("boom"));
    f.driver.push_artifact_index(line("a.md", b"hello"));
    assert_eq!(run(&f), 1);
}

#[test]
fn symlink_copy_is_refused_and_nothing_escapes_staging() {
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("secret.txt");
    fs::write(&secret, "keep").unwrap();
    let f = fx_with(FakeDriver::new().copy_symlink(&secret));
    f.driver.push_artifact_index(line("a.md", b"keep"));
    assert_eq!(run(&f), 0);
    assert_eq!(codes(&f), vec!["artifact_skipped"]);
    assert!(versions(&f).is_empty());
    assert_eq!(fs::read(&secret).unwrap(), b"keep");
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 1);
}

#[test]
fn index_failure_warns_and_stores_nothing() {
    let f = fx_with(FakeDriver::new().exec_results([Ok(ExecOutcome::Exited { code: 3 })]));
    assert_eq!(run(&f), 0);
    let w = warnings(&f);
    assert_eq!(w[0].code, "artifacts_collect_failed");
    assert!(w[0].message.contains('3'));
    assert!(f.driver.copies().is_empty());
}

#[test]
fn invalid_names_and_bad_lines_are_skipped() {
    let f = fx();
    f.driver.set_copy_text("hello");
    let mut out = line("a.md", b"hello");
    out.push_str(&line("../x", b"hello"));
    out.push_str("garbage\n");
    f.driver.push_artifact_index(out);
    assert_eq!(run(&f), 1);
    assert_eq!(codes(&f), vec!["artifact_skipped"]);
    assert_eq!(f.driver.copies().len(), 1);
}

#[test]
fn the_index_script_checks_the_directory_and_has_a_shasum_fallback() {
    let f = fx();
    let before = f.driver.box_calls().len();
    run(&f);
    assert_eq!(collect_calls(&f, before), vec!["batch"]);
    let execs = f.driver.execs();
    assert_eq!(execs.len(), 1);
    let script = &execs[0].1.argv[2];
    assert!(script.starts_with("cd -- '/work/box-1'"));
    assert!(script.contains(".moor/artifacts"));
    assert!(script.contains("shasum -a 256"));
    assert!(script.contains("-maxdepth 1 -type f"));
}

#[test]
fn a_row_without_a_box_collects_nothing() {
    let f = fx();
    let mut r = row(&f);
    r.boxed = None;
    assert_eq!(collect(&f.cx, f.cx.store().unwrap(), &r), 0);
    assert!(f.driver.execs().is_empty());
    assert!(codes(&f).is_empty());
}
