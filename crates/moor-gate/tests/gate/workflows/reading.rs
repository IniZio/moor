//! A workflow's reading reaches the page as the controller sent it, so the page draws marks from
//! it; a workflow with none sends none.

use serde_json::{json, Value};

use super::{as_user, json_of, world, HELD};
use crate::login::get;

const PATH: &str = "/account/api/workflows/weekly-deps";

fn held(reading: Value, who: &str) {
    HELD.lock()
        .unwrap()
        .iter_mut()
        .find(|(k, _)| k.contains(who))
        .unwrap()
        .1
        .reading = reading;
}

#[test]
fn the_page_gets_the_reading_the_controller_sent_unchanged() {
    let w = world(true, &[]);
    let cookie = as_user(&w, "alice-reading");
    get(&w.addr, PATH, Some(&cookie));
    let reading = json!({
        "id": null, "state": "read", "stale": ["prompt"], "source": "Post one comment.",
        "reader": null, "runs": [{"text": "Post one comment.", "phrase": "p1"}],
        "phrases": [{"id": "p1", "mark": "held", "effects": ["e1"]}],
        "effects": [{"id": "e1", "kind": "allow", "line": "POST /repos/{repo}/issues/*/comments",
                     "key": "allow:github-api:POST /repos/{repo}/issues/*/comments",
                     "phrases": ["p1"]}],
        "gates": null,
        "may": {"github-api": ["POST /repos/{repo}/issues/*/comments"]}
    });
    held(reading.clone(), "alice-reading");

    let flow = json_of(&get(&w.addr, PATH, Some(&cookie)));

    assert_eq!(flow["reading"], reading);
}

#[test]
fn a_workflow_with_no_reading_sends_none() {
    let w = world(true, &[]);
    let cookie = as_user(&w, "alice-no-reading");

    let flow = json_of(&get(&w.addr, PATH, Some(&cookie)));

    assert!(flow["reading"].is_null(), "{flow}");
}
