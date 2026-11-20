//! `workflow-add` with no cron: stored with no schedule, never due, and a cron needs its zone.

use moor_core::request::WorkflowCmd;

use super::{add, add_req, cmd, me, ok, refusal};
use crate::ops::users::tests::fixture;

#[test]
fn a_workflow_with_no_cron_is_stored_and_listed_with_no_schedule_or_next_due() {
    let f = fixture();
    let who = me(&f, "local:2000");
    for (name, cron, zone) in [
        ("none", None, None),
        ("blank", Some("  "), Some("UTC")),
        ("zone-only", None, Some("UTC")),
    ] {
        let mut req = add_req(name, "", "");
        req.cron = cron.map(str::to_owned);
        req.timezone = zone.map(str::to_owned);
        req.on_pull_request = true;
        let added = ok(&f, &who, add(req, None));
        assert_eq!(added.get("next_due"), None, "{name}");
    }
    let listed = ok(&f, &who, cmd(WorkflowCmd::List));
    for w in listed["workflows"].as_array().unwrap() {
        assert_eq!(w.get("cron"), None, "{w}");
        assert_eq!(w.get("timezone"), None, "{w}");
        assert_eq!(w.get("next_due"), None, "{w}");
        assert_eq!(w["enabled"], true);
    }
    let on = ok(
        &f,
        &who,
        cmd(WorkflowCmd::SetEnabled {
            name: "none".to_owned(),
            enabled: true,
        }),
    );
    assert_eq!(on.get("next_due"), None);
    ok(&f, &who, add(add_req("none", "0 9 * * *", "UTC"), None));
    let listed = ok(&f, &who, cmd(WorkflowCmd::List));
    let w = listed["workflows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["name"] == "none")
        .unwrap();
    assert_eq!(w["cron"], "0 9 * * *");
}

#[test]
fn a_cron_without_a_timezone_is_refused() {
    let f = fixture();
    let who = me(&f, "local:2000");
    let mut req = add_req("a", "0 9 * * *", "");
    req.timezone = None;
    assert!(refusal(&f, &who, add(req, None)).contains("timezone"));
}
