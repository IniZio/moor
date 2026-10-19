//! Close copies the Box's published artifacts into the local store before teardown.

use sha2::{Digest, Sha256};

use moor_core::domain::box_state::BoxLifecycle;

use super::teardown::{base_has, with_home, YAML};
use super::{fx_with, req, run, Fx};
use crate::testing::driver::FakeDriver;
use crate::testing::surface::FakeSurface;

const BODY: &[u8] = b"hello";

fn fx(driver: FakeDriver) -> Fx {
    fx_with(driver, FakeSurface::new("/s"), 0)
}

fn listing() -> String {
    format!("notes.txt\t{}\t{:x}\n", BODY.len(), Sha256::digest(BODY))
}

fn published(f: &Fx) -> usize {
    let events = f.cx.store().unwrap().events(f.id, 100).unwrap();
    events
        .iter()
        .filter(|e| e.kind == "artifact_published")
        .count()
}

#[test]
fn a_ready_box_has_its_artifacts_collected_on_close() {
    let f = fx(FakeDriver::new()
        .artifact_index([listing()])
        .copy_text(BODY));
    let v = run(&f.cx, &req(false)).unwrap();
    assert_eq!(v.task.lifecycle, BoxLifecycle::Closed);
    assert_eq!(published(&f), 1);
    let rows = f.cx.store().unwrap().artifacts(f.id).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows.first().map(|r| r.name.as_str()), Some("notes.txt"));
}

#[test]
fn force_close_collects_artifacts_too() {
    let f = fx(FakeDriver::new()
        .artifact_index([listing()])
        .copy_text(BODY));
    run(&f.cx, &req(true)).unwrap();
    assert_eq!(published(&f), 1);
}

#[test]
fn collection_runs_before_teardown_and_release() {
    let f = fx(with_home(
        FakeDriver::new()
            .artifact_index([listing()])
            .copy_text(BODY),
    ));
    std::fs::write(f.dir.path().join(".moor.yaml"), YAML).unwrap();
    base_has(&f, YAML);
    run(&f.cx, &req(false)).unwrap();
    let calls = f.driver.calls();
    let copy = calls
        .iter()
        .position(|c| c.starts_with("copy_out"))
        .unwrap();
    let release = calls.iter().position(|c| c == "release box-1").unwrap();
    let last_exec = calls.iter().rposition(|c| c.starts_with("exec ")).unwrap();
    assert!(copy < last_exec, "{calls:?}");
    assert!(last_exec < release, "{calls:?}");
    assert_eq!(published(&f), 1);
}
