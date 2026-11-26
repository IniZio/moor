//! A run Box's agent reports what it made outside moor; the run read lists it.

use moor_core::request::OutputReportReq;

use super::rig;
use crate::ops::output_report;
use crate::ops::workflow::now::run_with;
use crate::ops::workflow::run_start;
use crate::testing::cx::REQUESTER_USER;

fn report(box_id: moor_core::BoxId, kind: &str, title: &str) -> OutputReportReq {
    OutputReportReq {
        box_id,
        kind: kind.to_owned(),
        url: "https://example.com/acme/web/pull/5#c1".to_owned(),
        title: title.to_owned(),
    }
}

#[test]
fn a_reported_output_is_kept_once_per_url_and_a_bad_one_is_refused_with_why() {
    let r = rig();
    r.answer_repo();
    let id = r.put(180);
    r.advance_to(5_000);
    let start = |job| {
        run_start(&r.cx, &job);
        Ok(())
    };
    let read = |_: &_| unreachable!("no pull request");
    run_with(
        &r.cx,
        r.cx.store().unwrap(),
        REQUESTER_USER,
        ("daily-fix", None, None),
        (&read, &start),
    )
    .unwrap();
    let box_id = r.latest_box(id);
    let first = output_report::run(&r.cx, &report(box_id, "github.comment", "Evidence")).unwrap();
    assert!(
        first.reply.starts_with("kept github.comment"),
        "{}",
        first.reply
    );
    let again =
        output_report::run(&r.cx, &report(box_id, "github.comment", "Evidence, edited")).unwrap();
    assert!(again.reply.starts_with("updated"), "{}", again.reply);
    let kept = r.cx.store().unwrap().outputs_of(box_id).unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].title, "Evidence, edited");
    assert_eq!(kept[0].kind, "github.comment");
    let bad = output_report::run(&r.cx, &report(box_id, "GitHub", "t")).unwrap_err();
    assert!(bad.error.to_string().contains("kind"), "{}", bad.error);
}
