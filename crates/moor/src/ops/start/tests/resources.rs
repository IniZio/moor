use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::Decision;
use moor_core::shape::Shape;
use moor_core::{BoxId, ShowReq};

use super::setup::{start_req, start_with};
use crate::ops::start::run;
use crate::testing::driver::{is_clone_exec, FakeDriver};

const fn shape(cpus: u32, mem_mib: u64) -> Shape {
    Shape { cpus, mem_mib }
}

fn probe(cpus: u32, mem_mib: u64) -> String {
    format!("moor.cpus={cpus}\nmoor.mem_mib={mem_mib}\n")
}

fn show(w: &super::World) -> serde_json::Value {
    let req = ShowReq {
        target: Some(moor_core::BoxName::parse_allow_bare("demo").unwrap().into()),
        events: None,
        agent: false,
        reports: false,
        artifacts: false,
        pane: false,
    };
    let shown = crate::ops::show::run(&w.cx, &req).unwrap();
    let t = shown.boxed.as_ref().unwrap();
    serde_json::to_value(t.driver.as_ref().unwrap()).unwrap()
}

#[test]
fn the_delivered_cpus_and_memory_are_recorded_on_the_box() {
    let driver = FakeDriver::new().exec_stdout([probe(4, 7900)]);
    let (w, v) = start_with("setup: [a]\n", driver);
    let row =
        w.cx.store()
            .unwrap()
            .box_row(BoxId::new(v.boxed.id))
            .unwrap()
            .unwrap();
    assert_eq!(row.boxed.unwrap().delivered, Some(shape(4, 7900)));
}

#[test]
fn the_start_reply_and_show_carry_the_delivered_shape() {
    let driver = FakeDriver::new().exec_stdout([probe(4, 7900)]);
    let (w, v) = start_with("setup: [a]\n", driver);
    let started = v.boxed.driver.unwrap();
    assert_eq!((started.cpus, started.mem_mib), (Some(4), Some(7900)));
    let shown = show(&w);
    assert_eq!(shown.get("cpus"), Some(&serde_json::json!(4)));
    assert_eq!(shown.get("mem_mib"), Some(&serde_json::json!(7900)));
}

#[test]
fn a_box_whose_probe_printed_nothing_shows_no_shape() {
    let (w, v) = start_with("setup: [a]\n", FakeDriver::new());
    let started = v.boxed.driver.unwrap();
    assert_eq!((started.cpus, started.mem_mib), (None, None));
    assert_eq!(show(&w).get("cpus"), Some(&serde_json::Value::Null));
}

#[test]
fn the_probe_rides_the_clone_exec_and_adds_no_box_call() {
    let driver = FakeDriver::new().exec_stdout([probe(4, 7900)]);
    let (w, _) = start_with("setup: [a]\n", driver);
    let execs = w.driver.execs();
    let probing: Vec<_> = execs
        .iter()
        .filter(|e| e.1.argv.iter().any(|a| a.contains("nproc")))
        .collect();
    assert_eq!(probing.len(), 1);
    assert!(probing.iter().all(|e| is_clone_exec(&e.1)));
}

#[test]
fn a_box_with_a_probe_records_its_shape() {
    let driver = FakeDriver::new().exec_stdout([probe(1, 512)]);
    let (_, v) = start_with("setup: [a]\n", driver);
    assert_eq!(v.boxed.driver.unwrap().cpus, Some(1));
}

#[test]
fn a_re_provisioned_box_records_the_new_machine() {
    let driver = FakeDriver::new().exec_stdout([probe(4, 7900), probe(8, 15000)]);
    let (w, v) = start_with("setup: []\n", driver);
    let id = BoxId::new(v.boxed.id);
    let store = w.cx.store().unwrap();
    store
        .apply(Decision::new(id).box_input(BoxInput::DriverMissing))
        .unwrap();
    run(&w.cx, &start_req()).unwrap();
    let row = store.box_row(id).unwrap().unwrap();
    assert_eq!(row.boxed.unwrap().delivered, Some(shape(8, 15000)));
}

#[test]
fn the_launch_request_carries_the_repo_notes_and_the_delivered_shape() {
    let driver = FakeDriver::new().exec_stdout([probe(4, 7900)]);
    let (w, v) = start_with("setup: [a]\nbrief: run make emu\n", driver);
    let row =
        w.cx.store()
            .unwrap()
            .box_row(BoxId::new(v.boxed.id))
            .unwrap()
            .unwrap();
    let req = crate::ops::start::launch::launch_req(&w.cx, &row, None).unwrap();
    assert_eq!(req.repo_notes.as_deref(), Some("run make emu"));
    assert_eq!(req.shape, Some(shape(4, 7900)));
}
