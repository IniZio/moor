//! Tests for proof file recording and copying.

use std::fs;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_core::config::DEFAULT_PROOF_MAX_BYTES;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, Intent, IntentChange, NewTask};
use moor_core::domain::task::{SetupOutcome, TaskInput};
use moor_core::seam::clock::Clock;
use moor_core::seam::driver::{AcquireReq, Driver, DriverError, ExecOutcome};
use moor_core::{BoxId, BoxName, ProviderId};

use super::{sync_proofs, ProofBackoff};
use crate::adapters::clock::SystemClock;
use crate::cx::Cx;
use crate::ops::common::boxlock::BoxLock;
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
    backoff: ProofBackoff,
    dir: tempfile::TempDir,
    cx: Cx,
    driver: Arc<FakeDriver>,
    id: BoxId,
}

/// Adds a Task `name` with a Ready Box and returns its id.
fn ready_task(cx: &Cx, driver: &FakeDriver, dir: &std::path::Path, name: &str) -> BoxId {
    let name = BoxName::parse_allow_bare(name).unwrap();
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
            worktree: Field::Set(dir.display().to_string()),
            ..Fields::default()
        });
    store.apply(d).unwrap();
    id
}

/// A Task `t` with a Ready Box; each batched listing prints the next listing in `listings`.
fn fx(driver: FakeDriver, listings: &[&str]) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let driver = Arc::new(
        driver
            .run_stdouts(listings.iter().copied())
            .copy_text("proof"),
    );
    let cx = Cx::fake(
        dir.path(),
        dir.path(),
        driver.clone(),
        Arc::new(FakeSurface::new(dir.path())),
        Arc::new(ScriptedProc::new()),
    );
    let id = ready_task(&cx, &driver, dir.path(), "t");
    Fx {
        backoff: ProofBackoff::default(),
        dir,
        cx,
        driver,
        id,
    }
}

fn pass(f: &Fx, cap: u64) -> Vec<String> {
    sync_proofs(&f.cx, f.cx.store().unwrap(), cap, &f.backoff);
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
    let staging = f
        .dir
        .path()
        .join("proof-staging")
        .join(f.id.get().to_string());
    assert_eq!(to, [&staging.join("a.png"), &staging.join("b/c.txt")]);
    assert!(!staging.exists());
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

/// A driver whose first listing exits non-zero.
fn failing_listing() -> FakeDriver {
    FakeDriver::new().exec_results([Ok(ExecOutcome::Exited { code: 1 })])
}

/// Records the stored workdir on the fixture's Box.
fn store_workdir(f: &Fx) {
    let d = Decision::new(f.id).set(Fields {
        box_workdir: Field::Set("/work/stored".into()),
        ..Fields::default()
    });
    f.cx.store().unwrap().apply(d).unwrap();
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
fn a_sync_whose_listing_fails_warns_and_records_nothing() {
    let f = fx(failing_listing(), &[]);
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

#[test]
fn a_task_with_an_operation_in_flight_is_not_probed() {
    let f = fx(FakeDriver::new(), &[&line("a.png", 10, A)]);
    let set = Decision::new(f.id).intent(IntentChange::Set(Intent::Provisioning { pid: 1 }));
    f.cx.store().unwrap().apply(set).unwrap();
    assert!(pass(&f, DEFAULT_PROOF_MAX_BYTES).is_empty());
    assert!(f.driver.execs().is_empty());
}

#[test]
fn a_held_box_lock_skips_the_probe_without_a_warning() {
    let f = fx(FakeDriver::new(), &[&line("a.png", 10, A)]);
    let state = f.cx.state_dir().unwrap();
    let _held = BoxLock::acquire(f.cx.clock(), state, f.id, "setup", Duration::ZERO).unwrap();
    assert!(pass(&f, DEFAULT_PROOF_MAX_BYTES).is_empty());
    assert!(f.driver.execs().is_empty());
}

#[test]
fn a_stored_workdir_is_used_without_asking_the_provider() {
    let f = fx(FakeDriver::new(), &[&line("a.png", 10, A)]);
    store_workdir(&f);
    assert!(pass(&f, DEFAULT_PROOF_MAX_BYTES).is_empty());
    assert_eq!(proofs(&f).len(), 1);
    assert!(f.driver.calls().iter().all(|c| !c.starts_with("workdir ")));
    let execs = f.driver.execs();
    let script = execs.first().and_then(|e| e.1.argv.get(2)).unwrap();
    assert!(script.starts_with("cd '/work/stored'"), "{script}");
}

/// Releases the blocked listing on drop, so a failed assertion cannot hang the pass.
struct Unblock(std::sync::mpsc::Sender<()>);

impl Drop for Unblock {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

#[test]
fn the_box_lock_is_free_while_the_listing_runs() {
    let f = fx(FakeDriver::new(), &[&line("a.png", 10, A)]);
    let (started_tx, started) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel::<()>();
    let (started_tx, release_rx) = (Mutex::new(started_tx), Mutex::new(release_rx));
    f.driver.on_exec(move || {
        started_tx.lock().unwrap().send(()).unwrap();
        let _ = release_rx
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5));
    });
    let state = f.cx.state_dir().unwrap().to_path_buf();
    let unblock = Unblock(release);
    std::thread::scope(|s| {
        let pass = s.spawn(|| {
            let cx = Cx::fake(
                &state,
                &state,
                f.driver.clone(),
                Arc::new(FakeSurface::new(&state)),
                Arc::new(ScriptedProc::new()),
            );
            sync_proofs(
                &cx,
                cx.store().unwrap(),
                DEFAULT_PROOF_MAX_BYTES,
                &ProofBackoff::default(),
            );
            cx.take_warnings()
        });
        started.recv().unwrap();
        let clock = SystemClock::real();
        let begun = clock.instant();
        let got = BoxLock::acquire(
            &SystemClock::real(),
            &state,
            f.id,
            "end_session",
            crate::ops::common::boxlock::WAIT,
        );
        assert!(got.is_ok());
        assert!(clock.instant() - begun < Duration::from_secs(5));
        drop(got);
        unblock.0.send(()).unwrap();
        assert!(pass.join().unwrap().is_empty());
    });
    assert_eq!(proofs(&f).len(), 1);
}

#[test]
fn a_failed_pass_backs_off_for_five_minutes() {
    let f = fx(failing_listing(), &["", &line("a.png", 10, A)]);
    assert_eq!(pass(&f, DEFAULT_PROOF_MAX_BYTES).len(), 1);
    let calls = f.driver.calls().len();
    assert!(calls > 0);
    f.cx.clock().sleep(Duration::from_secs(299));
    assert!(pass(&f, DEFAULT_PROOF_MAX_BYTES).is_empty());
    assert_eq!(f.driver.calls().len(), calls);
    f.cx.clock().sleep(Duration::from_secs(2));
    assert!(pass(&f, DEFAULT_PROOF_MAX_BYTES).is_empty());
    assert!(f.driver.calls().len() > calls);
    assert_eq!(proofs(&f).len(), 1);
}

#[test]
fn a_success_after_the_window_clears_the_box_entry() {
    let f = fx(failing_listing(), &["", &line("a.png", 10, A)]);
    assert_eq!(pass(&f, DEFAULT_PROOF_MAX_BYTES).len(), 1);
    assert!(f.backoff.until.lock().unwrap().contains_key(&f.id));
    f.cx.clock().sleep(Duration::from_secs(301));
    assert!(pass(&f, DEFAULT_PROOF_MAX_BYTES).is_empty());
    assert!(!f.backoff.until.lock().unwrap().contains_key(&f.id));
}

#[test]
fn a_failing_box_does_not_delay_another_box() {
    let f = fx(failing_listing(), &["", ""]);
    ready_task(&f.cx, &f.driver, f.dir.path(), "u");
    let execs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = execs.clone();
    f.driver.on_exec(move || {
        seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    });
    assert_eq!(pass(&f, DEFAULT_PROOF_MAX_BYTES).len(), 1);
    assert_eq!(execs.load(std::sync::atomic::Ordering::SeqCst), 2);
    f.cx.clock().sleep(Duration::from_secs(10));
    assert!(pass(&f, DEFAULT_PROOF_MAX_BYTES).is_empty());
    assert_eq!(execs.load(std::sync::atomic::Ordering::SeqCst), 3);
}

#[test]
fn the_box_lock_covers_only_the_local_writes() {
    let listing = line("a.png", 10, A);
    let f = fx(FakeDriver::new(), &[&listing, &listing]);
    let held = Arc::new(Mutex::new(None));
    let (slot, state, id) = (held.clone(), f.cx.state_dir().unwrap().to_path_buf(), f.id);
    let once = std::sync::atomic::AtomicBool::new(true);
    f.driver.on_exec(move || {
        let mut slot = slot.lock().unwrap();
        if once.swap(false, std::sync::atomic::Ordering::SeqCst) {
            *slot = Some(
                BoxLock::acquire(
                    &SystemClock::real(),
                    &state,
                    id,
                    "end_session",
                    Duration::ZERO,
                )
                .unwrap(),
            );
        }
    });
    assert!(pass(&f, DEFAULT_PROOF_MAX_BYTES).is_empty());
    assert!(proofs(&f).is_empty());
    assert!(!f
        .dir
        .path()
        .join("proof-staging")
        .join(f.id.get().to_string())
        .exists());
    assert!(!pdir(&f).join("a.png").exists());
    held.lock().unwrap().take();
    assert!(pass(&f, DEFAULT_PROOF_MAX_BYTES).is_empty());
    assert_eq!(proofs(&f).len(), 1);
    assert!(pdir(&f).join("a.png").is_file());
}
