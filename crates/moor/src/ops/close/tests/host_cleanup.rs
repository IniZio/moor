use moor_core::seam::driver::ExecOutcome;

use super::{fx_with, req, run, Fx};
use crate::testing::driver::FakeDriver;
use crate::testing::surface::FakeSurface;

fn fx(driver: FakeDriver) -> Fx {
    fx_with(driver, FakeSurface::new("/s"), 0)
}

fn cleanup_scripts(f: &Fx) -> Vec<String> {
    f.driver
        .execs()
        .into_iter()
        .filter_map(|(_, spec, _)| spec.argv.last().cloned())
        .filter(|s| s.contains(".moor-cred-"))
        .collect()
}

#[test]
fn a_static_host_is_cleaned_before_release() {
    let f = fx(FakeDriver::new().static_host());
    let v = run(&f.cx, &req(false)).unwrap();
    assert!(v.box_deleted);
    let id = f.id.get();
    let want = format!(".moor-cred-{id}-*");
    assert!(matches!(cleanup_scripts(&f).as_slice(), [s] if s.contains(&want)));
    let calls = f.driver.calls();
    let last_exec = calls.iter().rposition(|c| c.starts_with("exec")).unwrap();
    let release = calls.iter().position(|c| c.starts_with("release")).unwrap();
    assert!(last_exec < release);
    assert!(f.cx.take_warnings().is_empty());
}

#[test]
fn another_host_is_not_cleaned() {
    let f = fx(FakeDriver::new());
    run(&f.cx, &req(false)).unwrap();
    assert!(cleanup_scripts(&f).is_empty());
}

#[test]
fn failed_cleanup_warns_and_still_releases() {
    let f = fx(FakeDriver::new().static_host().exec_results([
        Ok(ExecOutcome::Exited { code: 0 }),
        Ok(ExecOutcome::Exited { code: 0 }),
        Ok(ExecOutcome::Exited { code: 0 }),
        Ok(ExecOutcome::Exited { code: 3 }),
    ]));
    let v = run(&f.cx, &req(false)).unwrap();
    assert!(v.box_deleted);
    let w = f.cx.take_warnings();
    assert!(w.iter().any(|w| w.code == "host_cleanup_failed"
        && w.message.contains("cleanup exited 3")
        && w.message.contains(".moor-cred-")));
}
