//! The per-Task simulator is created and recorded before setup runs.

use std::sync::{Arc, Mutex};

use moor_core::config::{BoxCapability, Flags, TaskSettings, UserConfig};
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{Acquired, Platform};
use moor_core::{BoxHandle, BoxPath, ErrorKind, TaskId};

use crate::ops::common::simulator;
use crate::ops::start::run;
use crate::ops::start::tests::setup::{script_argv, start_req};
use crate::ops::start::tests::{world_at, World};
use crate::store::lease::{LeaseKind, LeaseState};
use crate::store::{Store, STORE_FILE};
use crate::testing::clock::FakeClock;
use crate::testing::driver::FakeDriver;

/// The `simctl list` output of a Mac with iOS 26.0 and 26.4.
const INVENTORY: &str = r#"{
  "devicetypes": [
    {"productFamily":"iPhone","identifier":"com.apple.CoreSimulator.SimDeviceType.iPhone-17","name":"iPhone 17","minRuntimeVersion":1703936}
  ],
  "runtimes": [
    {"identifier":"com.apple.CoreSimulator.SimRuntime.iOS-26-0","version":"26.0","name":"iOS 26.0","isAvailable":true,"platform":"iOS",
     "supportedDeviceTypes":[{"identifier":"com.apple.CoreSimulator.SimDeviceType.iPhone-17","name":"iPhone 17","productFamily":"iPhone"}]},
    {"identifier":"com.apple.CoreSimulator.SimRuntime.iOS-26-4","version":"26.4.1","name":"iOS 26.4","isAvailable":true,"platform":"iOS",
     "supportedDeviceTypes":[{"identifier":"com.apple.CoreSimulator.SimDeviceType.iPhone-17","name":"iPhone 17","productFamily":"iPhone"}]}
  ]
}"#;

/// The UDID the fake `simctl create` prints.
const UDID: &str = "0A1B2C3D-0000-4000-8000-0123456789AB";

/// What crabbox's log looks like when step `i` prints `text`: a banner, the framed text, a summary.
fn logged(i: u32, text: &str) -> String {
    let n = crate::ops::common::release::testkit::nonce(i);
    format!(
        "{}MOOR_SIM_BEGIN {n}\n{text}\nMOOR_SIM_END {n}\nrun summary\n",
        crate::ops::common::release::testkit::BANNER
    )
}

/// A world on a Mac layout whose repo has `yaml`; the host steps (the first rng draw is spent earlier) print `""`, the inventory, then the UDID.
fn mac_world(yaml: &str) -> World {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(repo.path().join(".moor.yaml"), yaml).unwrap();
    let driver = FakeDriver::new()
        .work_root("/macwork/demo/work")
        .exec_stdout(["", &logged(1, INVENTORY), &logged(2, UDID)]);
    driver.set_platform(Platform::Macos);
    let w = world_at(driver, repo.path());
    // The repo dir must outlive the run.
    std::mem::forget(repo);
    w
}

/// The `(kind, key, state)` of each simulator lease row of Task 1.
fn sim_rows(store: &Store) -> Vec<(LeaseKind, String, LeaseState)> {
    store
        .leases(TaskId::new(1))
        .unwrap()
        .into_iter()
        .filter(|r| matches!(r.kind, LeaseKind::SimSet | LeaseKind::SimDevice))
        .map(|r| (r.kind, r.key, r.state))
        .collect()
}

#[test]
fn a_selected_simulator_is_created_and_both_rows_are_held() {
    let w = mac_world("box:\n  capabilities: [ios-simulator]\n");
    run(&w.cx, &start_req()).unwrap();
    let rows = sim_rows(w.cx.store().unwrap());
    assert_eq!(
        rows,
        vec![
            (
                LeaseKind::SimSet,
                "/macwork/demo/sim".to_owned(),
                LeaseState::Held
            ),
            (LeaseKind::SimDevice, UDID.to_owned(), LeaseState::Held),
        ]
    );
    let store = w.cx.store().unwrap();
    assert_eq!(
        simulator::recorded(store, TaskId::new(1))
            .unwrap()
            .as_deref(),
        Some(UDID)
    );
    let creates = w
        .driver
        .execs()
        .iter()
        .filter(|(_, s, _)| s.argv.iter().any(|a| a.contains("simctl --set")))
        .count();
    assert_eq!(creates, 1);
}

#[test]
fn the_set_row_is_planned_before_anything_is_created() {
    let w = mac_world("box:\n  capabilities: [ios-simulator]\n");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (sink, path) = (Arc::clone(&seen), w.dir.path().join(STORE_FILE));
    w.driver.on_exec(move || {
        let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000_000)));
        let store = Store::open(&path, clock).unwrap();
        let set = sim_rows(&store)
            .into_iter()
            .find(|r| r.0 == LeaseKind::SimSet)
            .map(|r| r.2);
        sink.lock().unwrap().push(set);
    });
    run(&w.cx, &start_req()).unwrap();
    let seen = seen.lock().unwrap();
    // The directory, the listing and the create run while the set is only planned.
    assert_eq!(seen.get(..3), Some(&[Some(LeaseState::Planned); 3][..]));
}

#[test]
fn nothing_is_run_or_recorded_when_the_simulator_is_not_selected() {
    let w = mac_world("setup: []\n");
    run(&w.cx, &start_req()).unwrap();
    assert!(sim_rows(w.cx.store().unwrap()).is_empty());
    assert!(w
        .driver
        .execs()
        .iter()
        .all(|(_, s, _)| !s.argv.iter().any(|a| a.contains("simctl"))));
}

#[test]
fn an_unknown_runtime_pin_fails_as_a_config_error_and_setup_does_not_run() {
    let w = mac_world(
        "setup: [a]\nbox:\n  capabilities: [ios-simulator]\n  simulator:\n    runtime: iOS 99\n",
    );
    let err = run(&w.cx, &start_req()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Failed);
    let text = err.error.to_string();
    assert!(
        text.starts_with(".moor.yaml:box.simulator.runtime:"),
        "{text}"
    );
    assert!(text.contains("iOS 26.0, iOS 26.4"), "{text}");
    assert!(w
        .driver
        .execs()
        .iter()
        .all(|(_, s, _)| s.argv != script_argv(&["a"])));
    let rows = sim_rows(w.cx.store().unwrap());
    let planned = vec![(
        LeaseKind::SimSet,
        "/macwork/demo/sim".to_owned(),
        LeaseState::Planned,
    )];
    assert_eq!(rows, planned);
}

#[test]
fn a_recorded_device_is_not_created_again() {
    let w = mac_world("box:\n  capabilities: [ios-simulator]\n");
    run(&w.cx, &start_req()).unwrap();
    let before = w.driver.execs().len();
    let mut cfg = TaskSettings::resolve(&UserConfig::default(), None, &Flags::default());
    cfg.repo_box.capabilities.push(BoxCapability::IosSimulator);
    let got = Acquired {
        handle: BoxHandle::new("box-1"),
        workdir: BoxPath::restore("/macwork/demo/work/cbx_1/repo").unwrap(),
        platform: Platform::Macos,
    };
    let store = w.cx.store().unwrap();
    simulator::ensure(
        &w.cx,
        store,
        TaskId::new(1),
        &got,
        std::path::Path::new("/repo"),
        &cfg,
    )
    .unwrap();
    assert_eq!(w.driver.execs().len(), before);
}

#[test]
fn output_without_a_framed_result_names_the_step() {
    let repo = tempfile::tempdir().unwrap();
    std::fs::write(
        repo.path().join(".moor.yaml"),
        "box:\n  capabilities: [ios-simulator]\n",
    )
    .unwrap();
    let driver = FakeDriver::new()
        .work_root("/macwork/demo/work")
        .exec_stdout(["", INVENTORY]);
    driver.set_platform(Platform::Macos);
    let w = world_at(driver, repo.path());
    let err = run(&w.cx, &start_req()).unwrap_err();
    let text = err.error.to_string();
    assert!(text.contains("list the simulators"), "{text}");
    assert!(text.contains("no framed result"), "{text}");
}
