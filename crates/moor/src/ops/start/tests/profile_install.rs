//! Profile installs the launch script reports as failed reach the Box's events and the thread.

use super::profile::{set_profile, world_answering, HEAD};
use super::relaunch::{gone, session};
use super::setup::warning_codes;
use super::*;
use crate::testing::engine::FakeEngine;

/// Starts a Box with a profile, then relaunches it once the Box's failures file holds `failures`;
/// returns the `detail` of each failed install and the warning codes of the relaunch.
fn relaunch_with(failures: Option<&str>) -> (Vec<String>, Vec<String>) {
    let engine = FakeEngine::new()
        .launch_result(Ok(session("p-1")))
        .launch_result(Ok(session("p-2")));
    let mut w = world_answering(None, HEAD, None);
    w.cx = w.cx.with_engine(Arc::new(engine));
    set_profile(&w, None);
    let id = BoxId::new(run(&w.cx, &req("demo")).unwrap().boxed.id);
    let row = w.cx.store().unwrap().box_row(id).unwrap().unwrap();
    let boxed = row.boxed.unwrap();
    if let Some(text) = failures {
        let workdir = boxed.workdir.unwrap();
        let path = BoxPath::under(&workdir, ".moor/profile-failures").unwrap();
        w.driver
            .set_peek_text(boxed.handle.unwrap().as_str(), &path, text);
    }
    gone(&w);
    w.cx.take_warnings();
    run(&w.cx, &req("demo")).unwrap();
    let events = w.cx.store().unwrap().events(id, 50).unwrap();
    let details = events
        .into_iter()
        .filter(|e| e.kind == "profile_failed")
        .filter(|e| e.payload.get("step") == Some(&serde_json::json!("install")))
        .filter_map(|e| Some(e.payload.get("detail")?.as_str()?.to_owned()))
        .collect();
    (details, warning_codes(&w))
}

#[test]
fn each_failed_install_line_is_an_event_and_a_warning() {
    let (details, warnings) = relaunch_with(Some("skill lint exit 1\nplugin tools exit 2\n"));
    assert_eq!(details, ["skill lint exit 1", "plugin tools exit 2"]);
    let profile = warnings.iter().filter(|c| *c == "profile_failed").count();
    assert_eq!(profile, 2, "{warnings:?}");
}

#[test]
fn no_failures_file_records_nothing() {
    let (details, warnings) = relaunch_with(None);
    assert!(details.is_empty());
    assert!(
        !warnings.contains(&"profile_failed".to_owned()),
        "{warnings:?}"
    );
}

#[test]
fn an_empty_failures_file_records_nothing() {
    let (details, _) = relaunch_with(Some(""));
    assert!(details.is_empty());
}
