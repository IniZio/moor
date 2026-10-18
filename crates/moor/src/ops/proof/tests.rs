//! Tests for proof file recording and copying.

use std::fs;
use std::sync::Arc;

use moor_core::config::DEFAULT_PROOF_MAX_BYTES;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewTask};
use moor_core::domain::task::{SetupOutcome, TaskInput};
use moor_core::seam::driver::{AcquireReq, Driver, DriverError};
use moor_core::{BoxId, BoxName, ProviderId};

use super::sync_proofs;
use crate::cx::Cx;
use crate::testing::driver::FakeDriver;
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// Host proof copy dir for the fixture's Box.
fn pdir(f: &Fx) -> std::path::PathBuf {
    f.dir.path().join("proof").join(f.id.get().to_string())
}

struct Fx {
    dir: tempfile::TempDir,
    cx: Cx,
    driver: Arc<FakeDriver>,
    id: BoxId,
}

/// A Task `t` with a Ready Box; each `exec` pass prints the next listing in `listings`.
fn fx(driver: FakeDriver, listings: &[&str]) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(
        driver
            .exec_stdout(listings.iter().copied())
            .copy_text("proof"),
    );
    let cx = Cx::fake(
        dir.path(),
        dir.path(),
        driver.clone(),
        Arc::new(FakeSurface::new(dir.path())),
        Arc::new(ScriptedProc::new()),
    );
    let name = BoxName::parse_allow_bare("t").unwrap();
    let store = cx.store().unwrap();
    let id = store
        .apply(Decision::insert(NewTask {
            name: name.clone(),
            repo: "r".into(),
        }))
        .unwrap()
        .task;
    let got = driver
        .acquire(&AcquireReq {
            provider: ProviderId::new("p"),
            task: id,
            name,
            class: None,
            nonce: "abc123".into(),
            worktree: None,
        })
        .unwrap();
    let d = Decision::new(id)
        .task_input(TaskInput::Admitted)
        .box_input(BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(ProviderId::new("p")),
            ..Fields::default()
        });
    store.apply(d).unwrap();
    let d = Decision::new(id)
        .task_input(TaskInput::BoxReady {
            setup: SetupOutcome::Ok,
        })
        .box_input(BoxInput::Acquired { handle: got.handle })
        .set(Fields {
            worktree: Field::Set(dir.path().display().to_string()),
            ..Fields::default()
        });
    store.apply(d).unwrap();
    Fx {
        dir,
        cx,
        driver,
        id,
    }
}

fn pass(f: &Fx, cap: u64) -> Vec<String> {
    sync_proofs(&f.cx, f.cx.store().unwrap(), cap);
    f.cx.take_warnings()
        .into_iter()
        .map(|w| w.message)
        .collect()
}

/// The `proof` events recorded so far: name and refusal text.
fn proofs(f: &Fx) -> Vec<(String, Option<String>)> {
    let events = f.cx.store().unwrap().events(f.id, 1000).unwrap();
    events
        .into_iter()
        .filter(|e| e.kind == "proof")
        .map(|e| {
            (
                e.payload
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap()
                    .to_owned(),
                e.payload
                    .get("refused")
                    .and_then(|n| n.as_str())
                    .map(str::to_owned),
            )
        })
        .collect()
}

fn line(name: &str, size: u64, sha: &str) -> String {
    format!("{name}\t{size}\t{sha}\n")
}

#[test]
fn two_new_files_record_two_events_and_two_copies() {
    let listing = line("a.png", 10, A) + &line("b/c.txt", 20, B);
    let f = fx(FakeDriver::new(), &[&listing]);
    assert!(pass(&f, DEFAULT_PROOF_MAX_BYTES).is_empty());
    let names: Vec<String> = proofs(&f).into_iter().map(|p| p.0).collect();
    assert_eq!(names, ["a.png", "b/c.txt"]);
    let copies = f.driver.copies();
    assert_eq!(copies.len(), 2);
    let (from, to): (Vec<_>, Vec<_>) = copies.iter().map(|c| (c.1.as_str(), &c.2)).unzip();
    assert!(from.iter().all(|p| p.contains("/.moor/proof/")), "{from:?}");
    assert_eq!(to, [&pdir(&f).join("a.png"), &pdir(&f).join("b/c.txt")]);
    assert!(copies.iter().all(|c| c.3 == f.dir.path()));
    assert!(pdir(&f).join("b/c.txt").is_file());
}

#[test]
fn a_second_pass_with_the_same_listing_does_nothing() {
    let listing = line("a.png", 10, A);
    let f = fx(FakeDriver::new(), &[&listing, &listing]);
    pass(&f, DEFAULT_PROOF_MAX_BYTES);
    pass(&f, DEFAULT_PROOF_MAX_BYTES);
    assert_eq!(proofs(&f).len(), 1);
    assert_eq!(f.driver.copies().len(), 1);
}

#[test]
fn a_changed_file_records_one_more_event_and_copies_again() {
    let first = line("a.png", 10, A);
    let second = line("a.png", 11, B);
    let f = fx(FakeDriver::new(), &[&first, &second, &second]);
    for _ in 0..3 {
        pass(&f, DEFAULT_PROOF_MAX_BYTES);
    }
    assert_eq!(proofs(&f).len(), 2);
    assert_eq!(f.driver.copies().len(), 2);
}

#[test]
fn a_file_above_the_cap_is_refused_once_and_never_copied() {
    let listing = line("big.bin", 101, A);
    let f = fx(FakeDriver::new(), &[&listing, &listing]);
    pass(&f, 100);
    pass(&f, 100);
    let got = proofs(&f);
    assert_eq!(got.len(), 1);
    let why = got.iter().find_map(|p| p.1.as_deref()).unwrap();
    assert!(why.contains("100-byte"), "{why}");
    assert!(f.driver.copies().is_empty());
}

#[test]
fn a_custom_cap_decides_what_is_copied() {
    let listing = line("a.txt", 150, A);
    let f = fx(FakeDriver::new(), &[&listing]);
    pass(&f, 200);
    assert_eq!(proofs(&f), [("a.txt".to_owned(), None)]);
    assert_eq!(f.driver.copies().len(), 1);
}

#[test]
fn a_failed_copy_records_nothing_and_warns() {
    let listing = line("a.png", 10, A);
    let f = fx(
        FakeDriver::new().fail_copy_out(DriverError::Failed {
            summary: "boom".into(),
        }),
        &[&listing],
    );
    let warnings = pass(&f, DEFAULT_PROOF_MAX_BYTES);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings.iter().all(|w| w.contains("a.png")), "{warnings:?}");
    assert!(proofs(&f).is_empty());
}

#[test]
fn no_proof_directory_means_no_events() {
    let f = fx(FakeDriver::new(), &[""]);
    assert!(pass(&f, DEFAULT_PROOF_MAX_BYTES).is_empty());
    assert!(proofs(&f).is_empty());
    assert!(fs::read_dir(f.dir.path()).unwrap().all(|e| !e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with("proof-listing")));
}

/// The Box handle of the Task.
fn handle(f: &Fx) -> moor_core::BoxHandle {
    let rows = f.cx.store().unwrap().open_tasks().unwrap();
    rows.into_iter()
        .find_map(|r| r.boxed.and_then(|b| b.handle))
        .unwrap()
}

/// Where the planted symlinks point; nothing may appear here.
fn outside(f: &Fx) -> std::path::PathBuf {
    let out = f.dir.path().join("outside");
    fs::create_dir_all(&out).unwrap();
    out
}

#[test]
fn a_symlinked_task_directory_is_refused_without_copying() {
    let listing = line("a.png", 10, A);
    let f = fx(FakeDriver::new(), &[&listing]);
    let out = outside(&f);
    fs::create_dir_all(f.dir.path().join("proof")).unwrap();
    std::os::unix::fs::symlink(&out, pdir(&f)).unwrap();
    let warnings = pass(&f, DEFAULT_PROOF_MAX_BYTES);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings.iter().all(|w| w.contains("symlink")),
        "{warnings:?}"
    );
    assert!(f.driver.copies().is_empty());
    assert!(proofs(&f).is_empty());
    assert_eq!(fs::read_dir(&out).unwrap().count(), 0);
}

#[test]
fn a_symlink_at_the_destination_is_refused_without_copying() {
    let listing = line("a.png", 10, A);
    let f = fx(FakeDriver::new(), &[&listing]);
    let out = outside(&f);
    let secret = out.join("secret");
    fs::write(&secret, "keep").unwrap();
    fs::create_dir_all(pdir(&f)).unwrap();
    std::os::unix::fs::symlink(&secret, pdir(&f).join("a.png")).unwrap();
    let warnings = pass(&f, DEFAULT_PROOF_MAX_BYTES);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(f.driver.copies().is_empty());
    assert!(proofs(&f).is_empty());
    assert_eq!(fs::read_to_string(&secret).unwrap(), "keep");
}

#[test]
fn a_symlink_left_by_the_copy_is_removed_and_nothing_is_recorded() {
    let listing = line("a.png", 10, A);
    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("target");
    let f = fx(FakeDriver::new().copy_symlink(&target), &[&listing]);
    let warnings = pass(&f, DEFAULT_PROOF_MAX_BYTES);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings.iter().all(|w| w.contains("a.png")), "{warnings:?}");
    let dest = pdir(&f).join("a.png");
    assert!(fs::symlink_metadata(&dest).is_err());
    assert!(proofs(&f).is_empty());
    assert!(!target.exists());
}

#[test]
fn consecutive_listings_use_distinct_log_files() {
    let f = fx(FakeDriver::new(), &["", ""]);
    let handle = handle(&f);
    for _ in 0..2 {
        super::list(&f.cx, &handle, f.id, None).unwrap();
    }
    let logs: Vec<_> = f
        .driver
        .execs()
        .iter()
        .filter_map(|e| match &e.1.output {
            Some(moor_core::seam::proc::Output::Log(p)) => Some(p.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(logs.len(), 2);
    assert_ne!(logs.first(), logs.get(1));
}

#[test]
fn a_listing_that_wrote_no_log_is_an_error() {
    let f = fx(FakeDriver::new(), &[]);
    let handle = handle(&f);
    let err = super::list(&f.cx, &handle, f.id, None).unwrap_err();
    assert!(err.contains("listing"), "{err}");
}

#[test]
fn a_sync_with_no_listing_log_warns_and_records_nothing() {
    let f = fx(FakeDriver::new(), &[]);
    let warnings = pass(&f, DEFAULT_PROOF_MAX_BYTES);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(proofs(&f).is_empty());
    assert!(f.driver.copies().is_empty());
}

#[test]
fn a_file_listed_twice_records_one_event_and_one_copy() {
    let listing = line("a.png", 10, A) + &line("a.png", 10, A);
    let f = fx(FakeDriver::new(), &[&listing]);
    assert!(pass(&f, DEFAULT_PROOF_MAX_BYTES).is_empty());
    assert_eq!(proofs(&f).len(), 1);
    assert_eq!(f.driver.copies().len(), 1);
}
