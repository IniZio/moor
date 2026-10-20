//! Close copies the Box's `.moor/reports/*.md` into the local reports store.

use std::fmt::Write;
use std::fs;
use std::path::PathBuf;

use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::Decision;
use moor_core::domain::run::SyncMode;
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::driver::{DriverError, ExecOutcome};
use moor_core::BoxPath;

use super::teardown::{base_has, YAML};
use super::{box_state, fx_with, req, row, run, Fx};
use crate::testing::driver::FakeDriver;
use crate::testing::surface::FakeSurface;

const WORKDIR: &str = "/work/box-1";

fn index_path() -> BoxPath {
    BoxPath::under(WORKDIR, ".moor/reports.index").unwrap()
}

fn report_path(name: &str) -> BoxPath {
    BoxPath::under(WORKDIR, &format!(".moor/reports/{name}")).unwrap()
}

/// A driver whose Box lists `files` in its report index and serves their text.
fn driver_with(files: &[(&str, &str)]) -> FakeDriver {
    let mut index = String::new();
    for (name, text) in files {
        writeln!(index, "{name}\t{}", text.len()).unwrap();
    }
    files
        .iter()
        .filter(|(name, _)| !name.contains('/') && !name.contains(' '))
        .fold(
            FakeDriver::new().peek_text("box-1", &index_path(), &index),
            |d, (name, text)| d.peek_text("box-1", &report_path(name), text),
        )
}

fn fx(driver: FakeDriver) -> Fx {
    fx_with(driver, FakeSurface::new("/s"), 0)
}

fn kinds(f: &Fx) -> Vec<(String, i64)> {
    let mut rows: Vec<_> =
        f.cx.store()
            .unwrap()
            .reports(f.id)
            .unwrap()
            .into_iter()
            .map(|r| (r.kind, r.version))
            .collect();
    rows.sort();
    rows
}

fn stored(f: &Fx, kind: &str, version: u32) -> String {
    let path: PathBuf = f
        .dir
        .path()
        .join("reports")
        .join(f.id.get().to_string())
        .join(format!("{kind}.v{version}.md"));
    fs::read_to_string(path).unwrap()
}

fn reports_dir(f: &Fx) -> PathBuf {
    f.dir.path().join("reports")
}

/// Indices into the driver's call log of every `exec`.
fn exec_at(f: &Fx) -> Vec<usize> {
    f.driver
        .calls()
        .iter()
        .enumerate()
        .filter(|(_, c)| c.starts_with("exec "))
        .map(|(i, _)| i)
        .collect()
}

/// Unsynced execs that write the report index.
fn collect_execs(f: &Fx) -> usize {
    f.driver
        .execs()
        .iter()
        .filter(|(_, spec, _)| {
            spec.sync == SyncMode::None && spec.argv.iter().any(|a| a.contains("reports.index"))
        })
        .count()
}

fn peeked(f: &Fx) -> Vec<String> {
    f.driver
        .peeks()
        .into_iter()
        .map(|(_, p, _)| p.as_str().to_owned())
        .collect()
}

#[test]
fn two_reports_are_collected() {
    let f = fx(driver_with(&[
        ("result.md", "R body"),
        ("spec.md", "S body"),
    ]));
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
    assert_eq!(collect_execs(&f), 1);
    let peeks = peeked(&f);
    assert!(peeks.contains(&index_path().as_str().to_owned()));
    assert!(peeks.contains(&report_path("result.md").as_str().to_owned()));
    assert!(peeks.contains(&report_path("spec.md").as_str().to_owned()));
    assert_eq!(
        kinds(&f),
        [("result".to_owned(), 1), ("spec".to_owned(), 1)]
    );
    assert_eq!(stored(&f, "result", 1), "R body");
    assert_eq!(stored(&f, "spec", 1), "S body");
    assert!(f.cx.take_warnings().is_empty());
}

#[test]
fn duplicate_is_not_readded() {
    let f = fx(driver_with(&[("result.md", "same")]));
    let store = f.cx.store().unwrap();
    store
        .add_report(f.id, "result", "same", &reports_dir(&f))
        .unwrap();
    run(&f.cx, &req(false)).unwrap();
    assert_eq!(kinds(&f), [("result".to_owned(), 1)]);
}

#[test]
fn changed_content_adds_a_new_version() {
    let f = fx(driver_with(&[("result.md", "new text")]));
    let store = f.cx.store().unwrap();
    store
        .add_report(f.id, "result", "old text", &reports_dir(&f))
        .unwrap();
    run(&f.cx, &req(false)).unwrap();
    assert_eq!(
        kinds(&f),
        [("result".to_owned(), 1), ("result".to_owned(), 2)]
    );
    assert_eq!(stored(&f, "result", 2), "new text");
}

#[test]
fn failure_warns_and_close_proceeds() {
    let f = fx(driver_with(&[("result.md", "SECRET-BODY")]).exec_results([
        Ok(ExecOutcome::Exited { code: 0 }),
        Ok(ExecOutcome::Exited { code: 0 }),
        Err(DriverError::Failed {
            summary: "boom".into(),
        }),
    ]));
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
    assert!(v.box_deleted);
    assert!(f.driver.calls().iter().any(|c| c == "release box-1"));
    assert!(kinds(&f).is_empty());
    let warnings = f.cx.take_warnings();
    assert!(warnings.iter().any(|w| w.code == "reports_collect_failed"));
    assert!(warnings.iter().all(|w| !w.message.contains("SECRET-BODY")));
}

#[test]
fn nonzero_exit_warns_and_close_proceeds() {
    let f = fx(driver_with(&[("result.md", "x")]).exec_results([
        Ok(ExecOutcome::Exited { code: 0 }),
        Ok(ExecOutcome::Exited { code: 0 }),
        Ok(ExecOutcome::Exited { code: 2 }),
    ]));
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
    assert!(kinds(&f).is_empty());
    assert!(f
        .cx
        .take_warnings()
        .iter()
        .any(|w| w.code == "reports_collect_failed"));
}

/// `--force` skips the unsaved-work probe, so the index peek is the first peek.
#[test]
fn peek_failure_warns_and_close_proceeds() {
    let f = fx(
        driver_with(&[("result.md", "x")]).fail_peek(DriverError::Failed {
            summary: "boom".into(),
        }),
    );
    let v = run(&f.cx, &req(true)).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
    assert!(v.box_deleted);
    assert!(kinds(&f).is_empty());
    assert!(f
        .cx
        .take_warnings()
        .iter()
        .any(|w| w.code == "reports_collect_failed"));
}

#[test]
fn bad_name_is_skipped() {
    let f = fx(driver_with(&[
        ("../evil.md", "evil"),
        ("a b.md", "spaced"),
        ("ok.md", "fine"),
    ]));
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
    assert_eq!(kinds(&f), [("ok".to_owned(), 1)]);
    let warnings = f.cx.take_warnings();
    assert!(warnings.iter().any(|w| w.code == "report_skipped"));
    assert!(warnings
        .iter()
        .all(|w| !w.message.contains("evil\n") && !w.message.contains("spaced")));
    let peeks = peeked(&f);
    assert!(peeks
        .iter()
        .all(|p| !p.contains("evil") && !p.contains("a b")));
    assert!(peeks.contains(&report_path("ok.md").as_str().to_owned()));
}

#[test]
fn collection_runs_before_teardown_and_release() {
    let f = fx(driver_with(&[("result.md", "R")]));
    fs::write(f.dir.path().join(".moor.yaml"), YAML).unwrap();
    base_has(&f, YAML);
    run(&f.cx, &req(false)).unwrap();
    let calls = f.driver.calls();
    let execs = exec_at(&f);
    assert_eq!(
        execs.len(),
        5,
        "probe, recheck, collection, artifacts, teardown: {calls:?}"
    );
    let (collect, teardown) = (execs.get(2).unwrap(), execs.get(4).unwrap());
    let specs = f.driver.execs();
    let (c, t) = (&specs.get(2).unwrap().1, &specs.get(4).unwrap().1);
    assert_eq!(c.sync, SyncMode::None);
    assert!(c.argv.iter().any(|a| a.contains("reports.index")));
    assert_eq!(t.sync, SyncMode::Full);
    let last_peek = calls
        .iter()
        .rposition(|c| c.starts_with("peek_file"))
        .unwrap();
    let release = calls.iter().position(|c| c == "release box-1").unwrap();
    assert!(*collect < last_peek);
    assert!(last_peek < *teardown);
    assert!(*teardown < release);
    assert_eq!(kinds(&f), [("result".to_owned(), 1)]);
}

#[test]
fn gone_box_collects_nothing() {
    let f = fx(driver_with(&[("result.md", "R")]));
    f.cx.store()
        .unwrap()
        .apply(Decision::new(f.id).box_input(BoxInput::DriverMissing))
        .unwrap();
    run(&f.cx, &req(false)).unwrap();
    assert_eq!(collect_execs(&f), 0);
    assert!(f.driver.execs().is_empty());
    assert!(kinds(&f).is_empty());
}

#[test]
fn asleep_box_under_force_is_not_woken_to_collect() {
    let f = fx(driver_with(&[("result.md", "R")]));
    let store = f.cx.store().unwrap();
    store
        .apply(Decision::new(f.id).session(SessionInput::AgentQuiet))
        .unwrap();
    store
        .apply(
            Decision::new(f.id)
                .lifecycle(BoxLifecycleInput::Sleep { auto: false })
                .box_input(BoxInput::Sleep),
        )
        .unwrap();
    assert_eq!(box_state(&f), BoxState::Asleep);
    let v = run(&f.cx, &req(true)).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
    assert_eq!(row(&f).lifecycle, BoxLifecycle::Closed);
    assert_eq!(collect_execs(&f), 0);
    assert!(!f.driver.calls().iter().any(|c| c.starts_with("wake")));
    assert!(kinds(&f).is_empty());
}

#[test]
fn dot_led_names_are_skipped_and_later_reports_still_stored() {
    let f = fx(driver_with(&[
        ("..md", "d"),
        ("...md", "d"),
        (".md.md", "d"),
        ("result.md", "R"),
    ]));
    run(&f.cx, &req(false)).unwrap();
    assert_eq!(kinds(&f), [("result".to_owned(), 1)]);
    let skips =
        f.cx.take_warnings()
            .iter()
            .filter(|w| w.code == "report_skipped")
            .count();
    assert_eq!(skips, 3);
}

#[test]
fn double_suffix_is_its_own_kind() {
    let f = fx(driver_with(&[("x.md", "one"), ("x.md.md", "two")]));
    run(&f.cx, &req(false)).unwrap();
    assert_eq!(kinds(&f), [("x".to_owned(), 1), ("x.md".to_owned(), 1)]);
    assert_eq!(stored(&f, "x", 1), "one");
    assert_eq!(stored(&f, "x.md", 1), "two");
}

#[test]
fn one_failed_store_does_not_stop_the_rest() {
    let f = fx(driver_with(&[
        ("a.md", "A"),
        ("b.md", "SECRET-BODY"),
        ("c.md", "C"),
    ]));
    let blocker = reports_dir(&f).join(f.id.get().to_string()).join("b.v1.md");
    fs::create_dir_all(&blocker).unwrap();
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
    assert_eq!(kinds(&f), [("a".to_owned(), 1), ("c".to_owned(), 1)]);
    let warnings = f.cx.take_warnings();
    let skips = warnings
        .iter()
        .filter(|w| w.code == "report_skipped")
        .count();
    assert_eq!(skips, 1);
    assert!(warnings
        .iter()
        .all(|w| !w.message.contains("SECRET-BODY") && !w.message.contains("b.md")));
}

#[test]
fn recheck_probe_and_report_index_share_one_batch() {
    let f = fx(driver_with(&[("result.md", "R")]));
    run(&f.cx, &req(false)).unwrap();
    assert_eq!(kinds(&f), [("result".to_owned(), 1)]);
    let batches = f
        .driver
        .box_calls()
        .iter()
        .filter(|c| *c == "batch")
        .count();
    assert_eq!(batches, 3, "{:?}", f.driver.box_calls());
    let calls = f.driver.box_calls();
    let after_acquire: Vec<&str> = calls.iter().skip(1).map(String::as_str).collect();
    assert_eq!(
        after_acquire,
        ["batch", "batch", "peek_file", "batch", "release"],
        "{calls:?}"
    );
}

#[test]
fn reports_are_collected_alone_when_the_batch_stops_at_the_recheck() {
    let f = fx(driver_with(&[("result.md", "R")]).exec_results([
        Ok(ExecOutcome::Exited { code: 0 }),
        Err(DriverError::NotFound),
    ]));
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
    assert_eq!(kinds(&f), [("result".to_owned(), 1)]);
    assert_eq!(collect_execs(&f), 1);
}

#[test]
fn a_refused_recheck_stores_no_report() {
    let f = fx(driver_with(&[("result.md", "R")]));
    let driver = f.driver.clone();
    let runs = std::sync::atomic::AtomicU32::new(0);
    f.driver.on_exec(move || {
        // The first probe sees a clean Box; the recheck sees unsaved work.
        if runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1 {
            let path = BoxPath::under(WORKDIR, ".moor/boxwork").unwrap();
            driver.set_peek_text("box-1", &path, "ahead=1 dirty=2");
        }
    });
    let e = run(&f.cx, &req(false)).unwrap_err();
    assert_eq!(e.kind, moor_core::ErrorKind::Refused);
    assert!(kinds(&f).is_empty());
}
