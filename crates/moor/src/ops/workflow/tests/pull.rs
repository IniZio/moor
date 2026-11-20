//! A run for a pull request fired by hand: its head read, its Box named for it, and the
//! pull request in the Box's env.

use std::sync::Arc;

use moor_core::request::CloseReq;
use moor_core::view::workflow::RunNowValue;
use moor_core::workflow::{CommitSha, PullRef, PullTarget, RunCause};
use moor_core::ErrorKind;

use super::{repo, rig, Rig};
use crate::error::CommandError;
use crate::ops::workflow::now::run_with;
use crate::ops::workflow::{run_now, run_start};
use crate::testing::cx::REQUESTER_USER;
use crate::testing::engine::FakeEngine;

/// The head the fake read gives.
const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

fn target(text: &str) -> PullTarget {
    PullTarget::parse(text).unwrap()
}

/// `run_workflow` for `pull`, its head read as [`HEAD`] and its Box started on the spot.
fn fire(r: &Rig, pull: Option<&PullTarget>) -> Result<RunNowValue, CommandError> {
    let read = |_: &PullTarget| Ok(CommitSha::parse(HEAD).unwrap());
    let start = |job| {
        run_start(&r.cx, &job);
        Ok(())
    };
    run_with(
        &r.cx,
        r.cx.store().unwrap(),
        REQUESTER_USER,
        ("daily-fix", None, pull),
        (&read, &start),
    )
}

/// The rig over an engine that records each agent launch.
fn rig_seeing_launches() -> (Rig, Arc<FakeEngine>) {
    let engine = Arc::new(FakeEngine::new());
    let mut r = rig();
    r.cx = r.cx.with_engine(engine.clone());
    r.answer_repo();
    (r, engine)
}

/// The `MOOR_PULL_*` variables of the Box the last launch started, sorted by name.
fn pull_env(engine: &FakeEngine) -> Vec<(String, String)> {
    let launch = engine.launches().pop().expect("an agent was launched");
    let mut env: Vec<(String, String)> = launch
        .proxy
        .map(|p| p.box_env.clone())
        .unwrap_or_default()
        .into_iter()
        .filter(|(k, _)| k.starts_with("MOOR_PULL_"))
        .collect();
    env.sort();
    env
}

#[test]
fn a_run_for_a_pull_request_names_its_box_for_it_and_gives_it_the_pull_request() {
    let (r, engine) = rig_seeing_launches();
    let id = r.put(180);
    r.advance_to(5_000);
    let line = fire(&r, Some(&target("o/n#65"))).unwrap();
    assert_eq!(line.status, "started", "{line:?}");
    assert_eq!(
        line.box_name.as_deref(),
        Some("daily-fix/w1-pr65-0123456-m19700101000005")
    );
    let pull = PullRef {
        repo: repo(),
        number: 65,
        head_sha: CommitSha::parse(HEAD).unwrap(),
    };
    assert_eq!(r.latest(id).cause, RunCause::Manual { pull: Some(pull) });
    let want = [
        ("MOOR_PULL_HEAD_SHA", HEAD),
        ("MOOR_PULL_NUMBER", "65"),
        ("MOOR_PULL_REPO", "o/n"),
        ("MOOR_PULL_URL", "https://github.com/o/n/pull/65"),
    ]
    .map(|(k, v)| (k.to_owned(), v.to_owned()));
    assert_eq!(pull_env(&engine), want);
    let launch = engine.launches().pop().unwrap();
    let prompt = launch.prompt.text().unwrap_or_default();
    assert!(
        prompt.starts_with("This run is for pull request o/n#65 at head 0123456"),
        "{prompt}"
    );
}

#[test]
fn a_run_without_a_pull_request_gets_no_pull_request_variables() {
    let (r, engine) = rig_seeing_launches();
    let id = r.put(180);
    let line = fire(&r, None).unwrap();
    assert_eq!(line.status, "started", "{line:?}");
    assert_eq!(r.latest(id).cause, RunCause::Manual { pull: None });
    assert!(pull_env(&engine).is_empty());
}

#[test]
fn run_again_at_an_unchanged_head_starts_a_box_of_its_own() {
    let (r, _engine) = rig_seeing_launches();
    let id = r.put(180);
    r.advance_to(5_000);
    let first = fire(&r, Some(&target("o/n#65"))).unwrap();
    let close = CloseReq {
        target: r
            .cx
            .store()
            .unwrap()
            .box_row(r.latest_box(id))
            .unwrap()
            .unwrap()
            .name
            .into(),
        force: true,
    };
    crate::ops::close::run(&r.cx, &close).unwrap();
    r.pass();
    r.cx.clock().sleep(std::time::Duration::from_secs(5));
    let again = fire(&r, Some(&target("o/n#65"))).unwrap();
    assert_eq!(again.status, "started", "{again:?}");
    assert_ne!(again.box_name, first.box_name);
    assert_eq!(r.runs(id).len(), 2);
}

#[test]
fn a_pull_request_on_another_repo_is_refused_and_records_nothing() {
    let (r, _engine) = rig_seeing_launches();
    let id = r.put(180);
    let refused = fire(&r, Some(&target("o/other#65"))).unwrap_err();
    assert_eq!(refused.kind, ErrorKind::Refused, "{refused:?}");
    assert!(
        refused.error.to_string().contains("first repo"),
        "{refused:?}"
    );
    assert!(r.runs(id).is_empty());
    assert_eq!(r.open_boxes(), 0);
}

#[test]
fn without_a_github_app_a_run_for_a_pull_request_is_refused_and_records_nothing() {
    let (r, _engine) = rig_seeing_launches();
    let id = r.put(180);
    let pull = target("o/n#65");
    let refused = run_now(
        &r.cx,
        r.cx.store().unwrap(),
        REQUESTER_USER,
        ("daily-fix", None, Some(&pull)),
    )
    .unwrap_err();
    assert_eq!(refused.kind, ErrorKind::Refused, "{refused:?}");
    assert!(
        refused
            .error
            .to_string()
            .starts_with("no GitHub App configured"),
        "{refused:?}"
    );
    assert!(r.runs(id).is_empty());
    assert_eq!(r.open_boxes(), 0);
}
