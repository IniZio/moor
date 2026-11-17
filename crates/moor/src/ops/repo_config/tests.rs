//! `repo-config-get` and `repo-config-set`: who may read and write a repo's stored config.

use std::path::Path;
use std::sync::Arc;

use moor_core::request::{RepoConfigGetReq, RepoConfigSetReq};
use moor_core::seam::forge::{ForgeError, ForgeRepo, RepoAt};
use moor_core::user::Viewer;
use moor_core::ErrorKind;

use super::{get, set};
use crate::cx::Cx;
use crate::testing::cx::REQUESTER_USER;
use crate::testing::driver::FakeDriver;
use crate::testing::forge::{repo_at, FakeForge};
use crate::testing::proc::ScriptedProc;

const BASE: &str = "abc123";

fn repo() -> ForgeRepo {
    ForgeRepo::new("o", "n")
}

/// The operator, with a forge that answers for `repo` as `answer` says.
fn operator(dir: &Path, forge: &Arc<FakeForge>) -> Cx {
    Cx::fake(
        dir,
        Arc::new(FakeDriver::new()),
        Arc::new(ScriptedProc::new()),
    )
    .with_forge(forge.clone())
}

fn user(dir: &Path, forge: &Arc<FakeForge>) -> Cx {
    operator(dir, forge).with_viewer(Viewer::User(REQUESTER_USER))
}

fn get_req() -> RepoConfigGetReq {
    RepoConfigGetReq {
        identity: None,
        repo: repo(),
    }
}

fn set_req(text: &str, expected_revision: Option<u64>) -> RepoConfigSetReq {
    RepoConfigSetReq {
        identity: None,
        repo: repo(),
        text: text.to_owned(),
        expected_revision,
    }
}

fn forge(answer: Result<RepoAt, ForgeError>) -> Arc<FakeForge> {
    let forge = Arc::new(FakeForge::new());
    forge.answer(repo(), answer);
    forge
}

#[test]
fn the_operator_sets_reads_and_updates_a_config_without_asking_the_forge() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(FakeForge::new());
    let cx = operator(dir.path(), &forge);
    let none = get(&cx, &get_req()).unwrap();
    assert_eq!((none.revision, none.text), (0, None));
    let first = set(&cx, &set_req("env:\n  A: '1'\n", Some(0))).unwrap();
    assert_eq!(first.revision, 1);
    assert_eq!(first.edited_by.as_deref(), Some("local:1000"));
    let second = set(&cx, &set_req("env:\n  A: '2'\n", Some(1))).unwrap();
    assert_eq!(second.revision, 2);
    let read = get(&cx, &get_req()).unwrap();
    assert_eq!(read.text.as_deref(), Some("env:\n  A: '2'\n"));
    assert!(forge.reads().is_empty());
}

#[test]
fn text_that_does_not_parse_is_refused_with_the_parsers_error_and_stores_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let cx = operator(dir.path(), &Arc::new(FakeForge::new()));
    let err = set(&cx, &set_req("bogus: 1\n", None)).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
    assert!(err.error.to_string().contains("bogus"), "{}", err.error);
    let err = set(&cx, &set_req("setup: ['']\n", None)).unwrap_err();
    assert!(err.error.to_string().contains("setup[0]"), "{}", err.error);
    assert_eq!(get(&cx, &get_req()).unwrap().revision, 0);
}

#[test]
fn a_set_at_a_stale_revision_is_refused_naming_the_current_one() {
    let dir = tempfile::tempdir().unwrap();
    let cx = operator(dir.path(), &Arc::new(FakeForge::new()));
    set(&cx, &set_req("setup: [a]\n", Some(0))).unwrap();
    set(&cx, &set_req("setup: [b]\n", Some(1))).unwrap();
    let err = set(&cx, &set_req("setup: [c]\n", Some(1))).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(
        err.error.to_string().contains("revision 2"),
        "{}",
        err.error
    );
    let read = get(&cx, &get_req()).unwrap();
    assert_eq!(read.text.as_deref(), Some("setup: [b]\n"));
}

#[test]
fn a_user_who_can_push_may_set_and_a_user_who_cannot_may_only_read() {
    let dir = tempfile::tempdir().unwrap();
    let pusher = user(dir.path(), &forge(Ok(repo_at(BASE, None))));
    let set_ok = set(&pusher, &set_req("setup: [a]\n", Some(0))).unwrap();
    assert_eq!(set_ok.edited_by.as_deref(), Some("local:1000"));

    let read_only = RepoAt {
        can_push: false,
        ..repo_at(BASE, None)
    };
    let reader = user(dir.path(), &forge(Ok(read_only)));
    let err = set(&reader, &set_req("setup: [b]\n", Some(1))).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(
        err.error.to_string().contains("cannot push"),
        "{}",
        err.error
    );
    let read = get(&reader, &get_req()).unwrap();
    assert_eq!(read.text.as_deref(), Some("setup: [a]\n"));
}

#[test]
fn a_user_whose_account_cannot_see_the_repo_reads_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let operator_cx = operator(dir.path(), &Arc::new(FakeForge::new()));
    set(&operator_cx, &set_req("setup: [a]\n", Some(0))).unwrap();
    let blind = user(dir.path(), &forge(Err(ForgeError::NotVisible)));
    assert_eq!(
        get(&blind, &get_req()).unwrap_err().kind,
        ErrorKind::Refused
    );
    assert_eq!(
        set(&blind, &set_req("setup: [b]\n", None))
            .unwrap_err()
            .kind,
        ErrorKind::Refused
    );
    let read = get(&operator_cx, &get_req()).unwrap();
    assert_eq!(read.text.as_deref(), Some("setup: [a]\n"));
}

#[test]
fn a_user_with_no_github_token_is_told_to_log_in() {
    let dir = tempfile::tempdir().unwrap();
    let cx = user(dir.path(), &forge(Ok(repo_at(BASE, None))));
    let store = cx.store().unwrap();
    let name = moor_core::user::SecretName::parse("github").unwrap();
    store.delete_user_secret(REQUESTER_USER, &name).unwrap();
    let err = get(&cx, &get_req()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused);
    assert!(err.error.to_string().contains("log in"), "{}", err.error);
}

#[test]
fn text_over_the_cap_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let cx = operator(dir.path(), &Arc::new(FakeForge::new()));
    let big = format!("brief: {}\n", "x".repeat(super::TEXT_CAP));
    assert_eq!(
        set(&cx, &set_req(&big, None)).unwrap_err().kind,
        ErrorKind::Usage
    );
}
