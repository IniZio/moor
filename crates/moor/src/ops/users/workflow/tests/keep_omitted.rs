//! `workflow-add` over a stored workflow keeps every field the request leaves out; a new
//! workflow gets the defaults.

use moor_core::workflow::WorkflowState;

use super::{add, add_req, me, ok, stored};
use crate::ops::users::tests::fixture;

/// A request that leaves out every optional field.
fn bare(cron: &str) -> moor_core::request::WorkflowAddReq {
    let mut req = add_req("daily", cron, "UTC");
    req.cron = None;
    req.timezone = None;
    req.enabled = None;
    req.on_pull_request = None;
    req
}

#[test]
fn a_replace_that_leaves_fields_out_keeps_the_stored_ones() {
    let f = fixture();
    let who = me(&f, "local:2000");
    let mut first = add_req("daily", "0 9 * * *", "Europe/London");
    first.on_pull_request = Some(true);
    first.timeout_minutes = Some(45);
    first.config = Some("env:\n  A: b\n".to_owned());
    first.enabled = Some(false);
    ok(&f, &who, add(first, None));
    let before = stored(&f, &who);

    let mut again = bare("");
    again.prompt = "edited".to_owned();
    ok(&f, &who, add(again, None));
    let got = stored(&f, &who);
    assert_eq!(got.prompt, "edited");
    assert!(got.on_pull_request, "on_pull_request kept");
    assert_eq!(got.state, WorkflowState::Disabled, "enabled kept");
    assert_eq!(got.timeout.minutes(), 45);
    assert_eq!(got.config, before.config, "config kept");
    assert_eq!(got.schedule, before.schedule, "cron and zone kept");
}

#[test]
fn a_replace_that_sends_the_fields_changes_them() {
    let f = fixture();
    let who = me(&f, "local:2000");
    let mut first = add_req("daily", "0 9 * * *", "Europe/London");
    first.on_pull_request = Some(true);
    first.config = Some("env:\n  A: b\n".to_owned());
    first.enabled = Some(false);
    ok(&f, &who, add(first, None));

    let mut explicit = bare("");
    explicit.on_pull_request = Some(false);
    explicit.enabled = Some(true);
    explicit.timeout_minutes = Some(10);
    explicit.config = Some(String::new());
    ok(&f, &who, add(explicit, None));
    let got = stored(&f, &who);
    assert!(!got.on_pull_request);
    assert!(matches!(got.state, WorkflowState::Enabled { .. }));
    assert_eq!(got.timeout.minutes(), 10);
    assert_eq!(got.config, None, "a blank config clears it");
}

#[test]
fn a_new_workflow_that_leaves_fields_out_gets_the_defaults() {
    let f = fixture();
    let who = me(&f, "local:2000");
    let mut req = bare("");
    req.cron = Some("0 9 * * *".to_owned());
    req.timezone = Some("UTC".to_owned());
    ok(&f, &who, add(req, None));
    let got = stored(&f, &who);
    assert!(!got.on_pull_request);
    assert!(matches!(got.state, WorkflowState::Enabled { .. }));
    assert_eq!(got.timeout.minutes(), 180);
    assert_eq!(got.config, None);
}

#[test]
fn a_blank_cron_clears_the_stored_one_and_a_new_cron_keeps_the_stored_zone() {
    let f = fixture();
    let who = me(&f, "local:2000");
    ok(
        &f,
        &who,
        add(add_req("daily", "0 9 * * *", "Europe/London"), None),
    );

    let mut clear = bare("");
    clear.cron = Some(String::new());
    ok(&f, &who, add(clear, None));
    assert_eq!(stored(&f, &who).schedule, None, "a blank cron clears it");

    let mut set = bare("");
    set.cron = Some("0 10 * * *".to_owned());
    set.timezone = Some("Asia/Tokyo".to_owned());
    ok(&f, &who, add(set, None));
    let mut recron = bare("");
    recron.cron = Some("0 11 * * *".to_owned());
    ok(&f, &who, add(recron, None));
    let schedule = stored(&f, &who).schedule.unwrap();
    assert_eq!(schedule.expr(), "0 11 * * *");
    assert_eq!(schedule.zone(), "Asia/Tokyo");
}
