//! A failed setup line says why: its exit code, the end of its output and what the Proxy withheld.

use moor_core::domain::event::Event;
use moor_core::view::progress::{failure, Happened};
use moor_core::BoxId;

use super::setup::{exit, start_with};
use crate::testing::driver::FakeDriver;

fn withhold(store: &crate::store::Store) {
    store
        .record_proxy_event(
            BoxId::new(1),
            &Event::CredentialWithheld {
                host: "github.com".to_owned(),
                rule: "github-git".to_owned(),
                method: "GET".to_owned(),
                shape: None,
            },
        )
        .unwrap();
}

fn failed_with(events: &[Happened]) -> (String, String) {
    let failed = events.iter().find(|e| e.kind == "setup_failed").unwrap();
    let text = |key: &str| failed.payload.get(key).and_then(|v| v.as_str());
    (
        text("line").unwrap().to_owned(),
        text("why").unwrap().to_owned(),
    )
}

fn happened(w: &super::World, id: BoxId) -> Vec<Happened> {
    w.cx.store()
        .unwrap()
        .events(id, 100)
        .unwrap()
        .into_iter()
        .map(|e| Happened {
            kind: e.kind,
            payload: e.payload,
            ts: String::new(),
        })
        .collect()
}

#[test]
fn a_failed_line_records_its_exit_code_and_output_tail() {
    let log = "moor setup: $ a\n\u{1b}[31mfatal\u{1b}[0m: repository not found\n";
    let driver = FakeDriver::new()
        .exec_results([Ok(exit(0)), Ok(exit(128))])
        .copy_text(log);
    let (w, v) = start_with("setup: [a]\n", driver);
    let id = BoxId::new(v.boxed.id);
    let events = happened(&w, id);
    let (line, why) = failed_with(&events);
    assert_eq!(line, "a");
    assert!(why.contains("exit 128"), "{why}");
    assert!(why.contains("fatal: repository not found"), "{why}");
    assert!(!why.contains('\u{1b}'), "{why:?}");
    let shown = failure(&events).unwrap();
    assert!(shown.contains("exit 128"), "{shown}");
    assert!(shown.contains("repository not found"), "{shown}");
}

#[test]
fn the_tail_keeps_only_the_last_lines() {
    let log: String = (1..=60).fold(String::new(), |acc, n| acc + &format!("out {n}\n"));
    let driver = FakeDriver::new()
        .exec_results([Ok(exit(0)), Ok(exit(1))])
        .copy_text(format!("moor setup: $ a\n{log}"));
    let (w, v) = start_with("setup: [a]\n", driver);
    let events = happened(&w, BoxId::new(v.boxed.id));
    let (_, why) = failed_with(&events);
    assert!(why.contains("out 60"), "{why}");
    assert!(!why.contains("out 30\n"), "{why}");
}

#[test]
fn a_credential_withheld_while_the_line_ran_is_named() {
    let driver = FakeDriver::new()
        .exec_results([Ok(exit(0)), Ok(exit(1))])
        .exec_stdout(["", "moor setup: $ a\n"])
        .copy_text("moor setup: $ a\nfatal: could not read Username\n");
    let w = super::world(driver);
    super::script_base_yaml(&w, Some("setup: [a]\n"));
    let store = w.cx.store().unwrap().clone();
    w.driver.on_copy_out(move || withhold(&store));
    let v = crate::ops::start::run(&w.cx, &super::setup::start_req()).unwrap();
    let events = happened(&w, BoxId::new(v.boxed.id));
    let shown = failure(&events).unwrap();
    assert!(
        shown.contains("while it ran, the Proxy withheld github.com (rule github-git)"),
        "{shown}"
    );
}

#[test]
fn a_withhold_before_the_line_ran_is_not_blamed() {
    let driver = FakeDriver::new()
        .exec_results([Ok(exit(0)), Ok(exit(1))])
        .exec_stdout(["", "moor setup: $ a\n"])
        .copy_text("moor setup: $ a\nboom\n");
    let w = super::world(driver);
    super::script_base_yaml(&w, Some("setup: [a]\n"));
    let store = w.cx.store().unwrap().clone();
    w.driver.on_exec_of("git clone", move || withhold(&store));
    let v = crate::ops::start::run(&w.cx, &super::setup::start_req()).unwrap();
    let shown = failure(&happened(&w, BoxId::new(v.boxed.id))).unwrap();
    assert!(!shown.contains("withheld"), "{shown}");
}

#[test]
fn a_passing_setup_stores_nothing_extra() {
    let driver = FakeDriver::new().exec_results([Ok(exit(0)), Ok(exit(0))]);
    let (w, v) = start_with("setup: [a]\n", driver);
    let events = happened(&w, BoxId::new(v.boxed.id));
    assert!(events.iter().all(|e| e.kind != "setup_failed"));
    assert!(failure(&events).is_none());
}
