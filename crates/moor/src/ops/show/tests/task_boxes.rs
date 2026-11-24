//! `show` on a claimed place lists the work Boxes its agent started, with what each line needs.

use std::sync::Arc;

use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::decision::{Decision, NewBox, SpecFields};
use moor_core::view::TaskBoxState;
use moor_core::BoxId;
use moor_schema::tables::Events;
use sea_query::{Expr, ExprTrait as _, Query};

use super::super::run;
use super::place::{claimed, event};
use super::{add, cx, req};
use crate::cx::Cx;
use crate::store::db::{Conn, DbExt as _};
use crate::testing::cx::{box_repo, REQUESTER_USER};
use crate::testing::driver::FakeDriver;

/// A work Box named `name` that the agent of place `place` started on `o/n`.
fn started_by(cx: &Cx, name: &str, place: BoxId) -> BoxId {
    let new = NewBox {
        name: crate::testing::box_name(name),
        repo: Some(box_repo()),
        spec: SpecFields {
            task: Some(place),
            ..SpecFields::default()
        },
    };
    cx.store()
        .unwrap()
        .apply(Decision::insert(new).requester(REQUESTER_USER))
        .unwrap()
        .box_id
}

/// When Box `id` was started, in epoch milliseconds.
fn started_at(cx: &Cx, id: BoxId) -> i64 {
    cx.store()
        .unwrap()
        .box_row(id)
        .unwrap()
        .unwrap()
        .created_at
        .as_millis()
}

/// Closes Box `id` and dates its `closed` event `after_ms` after the Box started.
fn close_after(cx: &Cx, id: BoxId, after_ms: i64) {
    let at = started_at(cx, id) + after_ms;
    cx.store()
        .unwrap()
        .apply(Decision::new(id).lifecycle(BoxLifecycleInput::Close { reason: None }))
        .unwrap();
    let raw = Conn::open_file(&cx.state_dir().unwrap().join("moor.db")).unwrap();
    let closed = Query::update()
        .table(Events::Table)
        .value(Events::Ts, at)
        .and_where(Expr::col(Events::Box).eq(id.get()))
        .and_where(Expr::col(Events::Kind).eq("closed"))
        .take();
    raw.run(closed).unwrap();
}

fn task_boxes(cx: &Cx, place: &str) -> Vec<moor_core::view::TaskBoxView> {
    run(cx, &req(Some(place), None)).unwrap().task_boxes
}

#[test]
fn a_claimed_place_lists_its_task_boxes_oldest_first() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let place = claimed(&cx, "taken");
    let other = claimed(&cx, "other");
    started_by(&cx, "first", place);
    started_by(&cx, "elsewhere", other);
    started_by(&cx, "second", place);
    add(&cx, "by-hand");

    let listed = task_boxes(&cx, "taken");
    let names: Vec<&str> = listed.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, ["first", "second"]);
    assert_eq!(listed.first().unwrap().repo, "o/n");
    assert_eq!(listed.first().unwrap().state, TaskBoxState::Starting);
    assert_eq!(task_boxes(&cx, "other").len(), 1);
}

#[test]
fn ready_ms_is_the_first_working_minus_created() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let place = claimed(&cx, "taken");
    let id = started_by(&cx, "w", place);
    let at = started_at(&cx, id);
    assert_eq!(task_boxes(&cx, "taken").first().unwrap().ready_ms, None);

    event(&cx, id, ("box_ready", serde_json::json!({})), at + 5_200);
    assert_eq!(task_boxes(&cx, "taken").first().unwrap().ready_ms, None);
    event(&cx, id, ("working", serde_json::json!({})), at + 10_100);
    event(&cx, id, ("working", serde_json::json!({})), at + 90_000);

    assert_eq!(
        task_boxes(&cx, "taken").first().unwrap().ready_ms,
        Some(10_100)
    );
}

#[test]
fn a_closed_task_box_reads_closed_with_its_time() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let place = claimed(&cx, "taken");
    let id = started_by(&cx, "w", place);
    let at = started_at(&cx, id);
    let open = task_boxes(&cx, "taken");
    assert_eq!(open.first().unwrap().closed_at, None);
    event(&cx, id, ("working", serde_json::json!({})), at + 4_000);
    close_after(&cx, id, 360_000);

    let shown = task_boxes(&cx, "taken");
    let closed = shown.first().unwrap();
    assert_eq!(closed.state, TaskBoxState::Closed);
    assert_eq!(closed.closed_at, Some(rfc3339_of(at + 360_000)));
    assert_eq!(closed.started_at, rfc3339_of(at));
}

/// RFC 3339 UTC time of `ms`, as a view writes it.
fn rfc3339_of(ms: i64) -> String {
    crate::ops::common::view::rfc3339(moor_core::seam::clock::Ts::from_millis(ms))
}

#[test]
fn a_task_box_s_artifacts_carry_its_own_name() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let place = claimed(&cx, "taken");
    let (a, b) = (started_by(&cx, "a", place), started_by(&cx, "b", place));
    let store = cx.store().unwrap();
    store
        .add_artifact(a, "note.txt", b"hi", cx.blobs().unwrap())
        .unwrap();
    store
        .add_artifact(b, "shot.png", b"png", cx.blobs().unwrap())
        .unwrap();
    store
        .add_artifact(b, "shot.png", b"png2", cx.blobs().unwrap())
        .unwrap();
    store
        .add_artifact(place, "mine.txt", b"x", cx.blobs().unwrap())
        .unwrap();
    close_after(&cx, a, 1_000);

    let shown = task_boxes(&cx, "taken");
    let files = |name: &str| -> Vec<(String, u32)> {
        let found = shown.iter().find(|t| t.name == name).unwrap();
        let art = found.artifacts.iter();
        art.map(|f| (f.name.clone(), f.version)).collect()
    };
    assert_eq!(files("a"), [("note.txt".to_owned(), 1)]);
    assert_eq!(files("b"), [("shot.png".to_owned(), 2)]);
}

#[test]
fn a_work_box_lists_no_task_boxes() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let place = claimed(&cx, "taken");
    started_by(&cx, "w", place);
    add(&cx, "by-hand");
    assert!(task_boxes(&cx, "w").is_empty());
    assert!(task_boxes(&cx, "by-hand").is_empty());
    assert!(run(&cx, &req(None, None)).unwrap().task_boxes.is_empty());
}
