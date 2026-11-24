//! `show` lists the Boxes of tasks: an idle place is none, and a claimed place is one.

use std::sync::Arc;

use moor_core::domain::decision::{Decision, NewBox, SpecFields};
use moor_core::domain::place::{BoxKind, Claim, PlaceState};
use moor_core::progress::Class;
use moor_core::seam::forge::ForgeRepo;

use crate::store::db::DbExt as _;
use moor_schema::tables::Events;
use sea_query::Query;

use super::super::run;
use super::{add, cx, req};
use crate::testing::cx::{new_idle_place, REQUESTER_USER};
use crate::testing::driver::FakeDriver;

/// The names `show` lists, in order.
fn listed(cx: &crate::cx::Cx) -> Vec<String> {
    let shown = run(cx, &req(None, None)).unwrap();
    shown
        .boxes
        .iter()
        .flatten()
        .map(|b| b.name.clone())
        .collect()
}

#[test]
fn an_idle_place_is_not_listed_and_a_work_box_is() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    add(&cx, "work");
    let store = cx.store().unwrap();
    store
        .apply(new_idle_place(crate::testing::box_name("place")))
        .unwrap();
    assert_eq!(listed(&cx), ["work"]);
}

#[test]
fn a_claimed_place_is_listed_like_any_task() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let store = cx.store().unwrap();
    let claim = Claim {
        repo: ForgeRepo::new("o", "n"),
        also: Vec::new(),
        prompt: "where".to_owned(),
        at: 5,
        served: None,
        steered: None,
    };
    let new = NewBox {
        name: crate::testing::box_name("taken"),
        repo: None,
        spec: SpecFields {
            kind: BoxKind::Place(PlaceState::Claimed(claim)),
            ..SpecFields::default()
        },
    };
    store
        .apply(Decision::insert(new).requester(REQUESTER_USER))
        .unwrap();
    assert_eq!(listed(&cx), ["taken"]);
}

/// Claimed place `name` of the task that asked `where` of `o/n` and `o/m` at 2023-11-14 22:13:20.
pub(super) fn claimed(cx: &crate::cx::Cx, name: &str) -> moor_core::BoxId {
    let claim = Claim {
        repo: ForgeRepo::new("o", "n"),
        also: vec![ForgeRepo::new("o", "m")],
        prompt: "where".to_owned(),
        at: 1_700_000_000_000,
        served: None,
        steered: None,
    };
    let new = NewBox {
        name: crate::testing::box_name(name),
        repo: None,
        spec: SpecFields {
            kind: BoxKind::Place(PlaceState::Claimed(claim)),
            ..SpecFields::default()
        },
    };
    cx.store()
        .unwrap()
        .apply(Decision::insert(new).requester(REQUESTER_USER))
        .unwrap()
        .box_id
}

/// Records that the person steered Box `id` with `message`, a second after the claim.
fn steered(cx: &crate::cx::Cx, id: moor_core::BoxId, message: &str) {
    event(
        cx,
        id,
        ("steered", serde_json::json!({ "message": message })),
        1_700_000_001_000,
    );
}

/// Records that `kind` happened to Box `id` at `ts`.
pub(super) fn event(
    cx: &crate::cx::Cx,
    id: moor_core::BoxId,
    (kind, payload): (&str, serde_json::Value),
    ts: i64,
) {
    let raw = crate::store::db::Conn::open_file(&cx.state_dir().unwrap().join("moor.db")).unwrap();
    raw.run(
        Query::insert()
            .into_table(Events::Table)
            .columns([Events::Box, Events::Kind, Events::Payload, Events::Ts])
            .values_panic([
                id.get().into(),
                kind.into(),
                payload.to_string().into(),
                ts.into(),
            ]),
    )
    .unwrap();
}

/// The text of the entries of `name`'s timeline that a person wrote.
fn said(cx: &crate::cx::Cx, name: &str) -> Vec<String> {
    let shown = run(cx, &req(Some(name), Some(100))).unwrap();
    shown
        .timeline
        .unwrap()
        .into_iter()
        .filter(|e| e.class == Class::You)
        .map(|e| e.text)
        .collect()
}

#[test]
fn a_claimed_place_shows_the_repos_the_words_and_the_time_of_its_claim() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    claimed(&cx, "taken");
    let shown = run(&cx, &req(None, None)).unwrap();
    let task = shown.boxes.unwrap().remove(0);
    assert_eq!(task.repo, Some(ForgeRepo::new("o", "n")));
    assert_eq!(task.other_repos, [ForgeRepo::new("o", "m")]);
    assert_eq!(task.prompt.as_deref(), Some("where"));
    assert_eq!(task.task_at.as_deref(), Some("2023-11-14T22:13:20Z"));
    assert_eq!(task.progress.unwrap().title, "where");
}

#[test]
fn a_work_box_has_no_task_time() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    add(&cx, "work");
    let shown = run(&cx, &req(None, None)).unwrap();
    assert_eq!(shown.boxes.unwrap().remove(0).task_at, None);
}

#[test]
fn a_claimed_place_shows_the_person_their_own_words_without_the_repos_line() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let id = claimed(&cx, "taken");
    steered(&cx, id, "Repos: o/n, o/m\n\nwhere");
    assert_eq!(said(&cx, "taken"), ["where"]);
}

#[test]
fn a_work_box_keeps_a_message_that_starts_like_the_repos_line() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let id = add(&cx, "work");
    steered(&cx, id, "Repos: o/n\n\nwhere");
    assert_eq!(said(&cx, "work"), ["Repos: o/n\n\nwhere"]);
}

#[test]
fn a_claimed_place_shows_nothing_of_its_life_before_the_claim() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let id = claimed(&cx, "taken");
    event(
        &cx,
        id,
        ("box_ready", serde_json::json!({})),
        1_699_999_000_000,
    );
    event(
        &cx,
        id,
        ("setup_failed", serde_json::json!({"reason": "late"})),
        1_700_000_002_000,
    );
    let shown = run(&cx, &req(Some("taken"), Some(100))).unwrap();
    let kinds: Vec<String> = shown
        .timeline
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .collect();
    assert_eq!(kinds, ["setup_failed"]);
}
