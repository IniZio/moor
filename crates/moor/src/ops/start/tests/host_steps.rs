//! Tests for the backend host step during a start on a macOS Box.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use moor_core::seam::driver::{ExecSpec, Platform};
use moor_core::{BoxHandle, ErrorKind, StartReq, TaskId};

use super::moor_yaml::script_base_yaml;
use super::setup::{exit, is_setup_line, start_req, start_with};
use super::{world_at, World};
use crate::ops::start::run;
use crate::store::lease::{LeaseKind, LeaseState};
use crate::testing::driver::FakeDriver;
use moor_core::seam::driver::DriverError;

const BACKEND: &str = "setup: [a]\nbox: {host_steps: [backend]}\n";

const PORT_ERR: &str = "Bind for 0.0.0.0:8080 failed: port is already allocated\n";

fn mac_driver() -> FakeDriver {
    let driver = FakeDriver::new();
    driver.set_platform(Platform::Macos);
    driver
}

fn is_compose(argv: &[String]) -> bool {
    argv.first().is_some_and(|a| a == "sh")
        && argv.get(2).is_some_and(|s| s.contains("docker compose -p"))
}

/// A setup line, which is also an `sh` exec but not the compose one.
fn is_setup(argv: &[String]) -> bool {
    is_setup_line(argv) && !is_compose(argv)
}

fn all_execs(w: &World) -> Vec<(BoxHandle, ExecSpec, moor_core::seam::proc::Deadline)> {
    w.driver.execs()
}

#[test]
fn a_mac_box_brings_the_backend_up_before_setup_runs() {
    let (w, v) = start_with(BACKEND, mac_driver());
    let execs = all_execs(&w);
    let up = execs
        .iter()
        .position(|e| is_compose(&e.1.argv))
        .expect("a compose exec");
    let setup = execs
        .iter()
        .position(|e| is_setup(&e.1.argv))
        .expect("a setup exec");
    assert!(up < setup, "{up} {setup}");
    assert!(execs[up].1.argv[2].contains("docker compose -p moor-demo up -d"));
    assert!(execs[up].1.argv.iter().all(|a| !a.contains("nono")));
    let rows =
        w.cx.store()
            .unwrap()
            .leases(TaskId::new(v.task.id))
            .unwrap();
    let compose: Vec<_> = rows
        .iter()
        .filter(|r| r.kind == LeaseKind::Compose)
        .collect();
    assert_eq!(compose.len(), 1, "{rows:?}");
    assert_eq!(compose[0].key, "moor-demo");
    assert_eq!(compose[0].state, LeaseState::Held);
}

#[test]
fn a_repo_without_host_steps_runs_no_docker_and_holds_no_compose_lease() {
    let (w, v) = start_with("setup: [a]\n", mac_driver());
    assert!(all_execs(&w)
        .iter()
        .all(|e| e.1.argv.first().map(String::as_str) != Some("docker")));
    let rows =
        w.cx.store()
            .unwrap()
            .leases(TaskId::new(v.task.id))
            .unwrap();
    assert!(
        rows.iter().all(|r| r.kind != LeaseKind::Compose),
        "{rows:?}"
    );
}

#[test]
fn an_unknown_host_step_fails_before_a_replacement_box_is_acquired() {
    let repo = tempfile::tempdir().unwrap();
    let base = "setup: [a]\n";
    std::fs::write(repo.path().join(".moor.yaml"), base).unwrap();
    let driver = FakeDriver::new().fail_acquire(DriverError::Failed {
        summary: "boom".into(),
    });
    let w = world_at(driver, repo.path());
    script_base_yaml(&w, Some(base));
    run(&w.cx, &start_req()).unwrap_err();
    let name = moor_core::TaskName::parse("demo").unwrap();
    let row = w.cx.store().unwrap().open_task_by_name(&name).unwrap();
    let wt = row.unwrap().worktree.unwrap();
    std::fs::create_dir_all(&wt).unwrap();
    std::fs::write(
        std::path::Path::new(&wt).join(".moor.yaml"),
        "setup: [a]\nbox: {host_steps: [nope]}\n",
    )
    .unwrap();
    script_base_yaml(&w, Some(base));
    let acquires = || w.driver.calls().iter().filter(|c| *c == "acquire").count();
    let before = acquires();
    let trusted = StartReq {
        trust_moor_yaml: true,
        ..start_req()
    };
    let err = run(&w.cx, &trusted).unwrap_err();
    let msg = err.error.to_string();
    assert!(msg.contains(".moor.yaml:box.host_steps[0]:"), "{msg}");
    assert_eq!(acquires(), before);
}

#[test]
fn a_port_clash_is_the_start_failure_and_setup_does_not_run() {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), BACKEND).unwrap();
    let w = world_at(mac_driver(), repo.path());
    script_base_yaml(&w, Some(BACKEND));
    // The execs before the compose up each exit 0; the compose up exits 1 with the clash.
    let before = {
        let probe = start_with(BACKEND, mac_driver()).0;
        all_execs(&probe)
            .iter()
            .position(|e| is_compose(&e.1.argv))
            .unwrap()
    };
    let mut results: Vec<_> = (0..before).map(|_| Ok(exit(0))).collect();
    results.extend([Ok(exit(1)), Ok(exit(0))]);
    let driver = mac_driver()
        .exec_results(results)
        .exec_stdout([&format!("moor-up-begin\n{PORT_ERR}moor-up-end 1\n"), ""]);
    let w = world_at(driver, repo.path());
    script_base_yaml(&w, Some(BACKEND));
    let err = run(&w.cx, &start_req()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    assert_eq!(
        err.error.to_string(),
        "host step backend: port 8080 is held by another process or container; \
         stop it or change the port"
    );
    assert!(all_execs(&w).iter().all(|e| !is_setup(&e.1.argv)));
}
