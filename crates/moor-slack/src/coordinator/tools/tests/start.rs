//! Tests for `start_box`: the repo it picks, the open-Box limit, the requester gate, the
//! rerun's refusal and the Box names.
#![allow(clippy::panic, clippy::indexing_slicing, reason = "test code")]

use std::path::Path;

use moor_client::ClientError;
use moor_core::name::{BoxId, BoxName};
use moor_core::request::Request;
use moor_core::response::Response;

use super::turn_of;
use crate::coordinator::tools::{run, Call, Reply, Scope};
use crate::db::{BoxState, Db, SlackThread};

/// A database whose thread `1.0` of channel `C1` was started by `U1` and has host Session
/// 100, with no Box yet; channel `C1` has repos `acme/api` and `acme/web`, and `default` is
/// marked default when given.
fn fresh_thread(dir: &Path, default: Option<&str>) -> Db {
    let db = Db::open(&dir.join("slack.db")).unwrap();
    let thread = SlackThread {
        channel: "C1".into(),
        thread_ts: "1.0".into(),
        owner: "U1".into(),
    };
    db.insert_thread(&thread, 1).unwrap();
    db.set_root_session("C1", "1.0", "/w", 100).unwrap();
    for repo in ["acme/api", "acme/web"] {
        db.add_channel_repo("C1", repo, "U1", 1).unwrap();
    }
    if let Some(repo) = default {
        db.set_default_channel_repo("C1", repo).unwrap();
    }
    db
}

/// Runs `call` in thread `1.0` with a limit of `max` open Boxes; nothing reaches the
/// controller.
fn start_with(db: &Db, call: &Call, max: u32) -> Reply {
    run_with(db, call, max, &mut |r| {
        panic!("start_box sends nothing: {r:?}")
    })
}

/// Runs `call` in thread `1.0` with a limit of `max` open Boxes and `send` answering the
/// controller.
fn run_with(
    db: &Db,
    call: &Call,
    max: u32,
    send: &mut dyn FnMut(Request) -> Result<Response, ClientError>,
) -> Reply {
    let mut scope = Scope {
        db,
        channel: "C1",
        thread_ts: "1.0",
        call: send,
        clock: || 50,
        max_open_boxes: max,
    };
    run(&mut scope, call)
}

fn start(prompt: &str, repo: Option<&str>) -> Call {
    Call::StartBox {
        prompt: prompt.into(),
        repo: repo.map(str::to_owned),
        base: None,
        node: None,
    }
}

fn start_from(prompt: &str, base: &str) -> Call {
    Call::StartBox {
        prompt: prompt.into(),
        repo: Some("acme/api".into()),
        base: Some(base.to_owned()),
        node: None,
    }
}

#[test]
fn start_box_records_the_box_on_the_channel_default_with_the_host_session_as_parent() {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_thread(dir.path(), Some("acme/web"));
    let turn = turn_of(&db, "U1", "Ev1");
    let reply = start_with(&db, &start("bump the version", None), 1);
    let said = "Box `slack-1-0-1` is starting on acme/web. It has not started yet and can still \
                fail, so do not tell the thread it started; the thread will say whether it did.";
    assert_eq!(reply, Reply::done(said));
    let [row] = db.thread_boxes("C1", "1.0").unwrap().try_into().unwrap();
    assert_eq!(row.box_name.as_str(), "slack-1-0-1");
    assert_eq!(
        (row.repo.as_str(), row.prompt.as_str()),
        ("acme/web", "bump the version")
    );
    assert_eq!(row.requested_by, "U1");
    assert_eq!(row.parent, Some(moor_core::name::SessionId::new(100)));
    assert_eq!(row.state, BoxState::Starting);
    let journal = db.turn_actions(turn).unwrap();
    let outcome = Some("Box `slack-1-0-1` is starting on acme/web".to_owned());
    assert_eq!(
        (journal[0].tool.as_str(), &journal[0].outcome),
        ("start_box", &outcome)
    );
}

#[test]
fn start_box_stores_the_base_it_is_given_and_none_without_one() {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_thread(dir.path(), Some("acme/web"));
    turn_of(&db, "U1", "Ev1");
    let based = start_with(&db, &start_from("set up", "moor-setup"), 2);
    assert!(!based.is_error, "{based:?}");
    let plain = start_with(&db, &start("set up", None), 2);
    assert!(!plain.is_error, "{plain:?}");
    let [based, plain] = db.thread_boxes("C1", "1.0").unwrap().try_into().unwrap();
    assert_eq!(based.base.as_deref(), Some("moor-setup"));
    assert_eq!(plain.base, None);
}

#[test]
fn start_box_reads_base_from_the_tool_arguments() {
    let args = serde_json::json!({"prompt": "g", "base": " moor-setup "});
    let with = Call::StartBox {
        prompt: "g".into(),
        repo: None,
        base: Some("moor-setup".into()),
        node: None,
    };
    assert_eq!(Call::parse("start_box", &args), Ok(with));
    let args = serde_json::json!({"prompt": "g"});
    assert_eq!(Call::parse("start_box", &args), Ok(start("g", None)));
}

#[test]
fn start_box_takes_a_named_channel_repo_and_refuses_any_other_or_a_missing_default() {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_thread(dir.path(), None);
    turn_of(&db, "U1", "Ev1");
    let no_default = start_with(&db, &start("g", None), 1);
    let want =
        "Name a repo: this channel has no default. This channel's repos: acme/api, acme/web.";
    assert_eq!(no_default, Reply::refused(want));
    let unknown = start_with(&db, &start("g", Some("evil/repo")), 1);
    let want = "This channel has no repo evil/repo. This channel's repos: acme/api, acme/web.";
    assert_eq!(unknown, Reply::refused(want));
    assert_eq!(db.thread_boxes("C1", "1.0").unwrap(), []);
    let named = start_with(&db, &start("g", Some("acme/api")), 1);
    assert!(!named.is_error, "{named:?}");
    assert_eq!(db.thread_boxes("C1", "1.0").unwrap()[0].repo, "acme/api");
}

#[test]
fn a_repo_outside_the_channels_repos_is_refused_even_with_a_default() {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_thread(dir.path(), Some("acme/web"));
    turn_of(&db, "U1", "Ev1");
    let reply = start_with(&db, &start("g", Some("evil/repo")), 1);
    let want = "This channel has no repo evil/repo. This channel's repos: acme/api, acme/web.";
    assert_eq!(reply, Reply::refused(want));
    assert_eq!(db.thread_boxes("C1", "1.0").unwrap(), []);
}

#[test]
fn box_numbers_count_only_starts_so_a_steer_between_two_starts_takes_none() {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_thread(dir.path(), Some("acme/api"));
    turn_of(&db, "U1", "Ev1");
    assert!(!start_with(&db, &start("one", None), 2).is_error);
    let first = BoxName::parse_allow_bare("slack-1-0-1").unwrap();
    db.bind(&first, BoxId::new(1), Some("moor/b")).unwrap();
    let steer = Call::SteerBox {
        box_name: first.as_str().into(),
        text: "use tokio".into(),
    };
    let steered = run_with(&db, &steer, 2, &mut |r| Ok(Response::ok(r.cmd(), None)));
    assert!(!steered.is_error, "{steered:?}");
    assert!(!start_with(&db, &start("two", None), 2).is_error);
    let journal = db.thread_actions("C1", "1.0").unwrap();
    let tools: Vec<_> = journal.iter().map(|a| a.tool.as_str()).collect();
    assert_eq!(tools, ["start_box", "steer_box", "start_box"]);
    let names: Vec<_> = db
        .thread_boxes("C1", "1.0")
        .unwrap()
        .iter()
        .map(|b| b.box_name.as_str().to_owned())
        .collect();
    assert_eq!(names, ["slack-1-0-1", "slack-1-0-2"]);
}

#[test]
fn with_the_limit_at_2_a_second_start_works_and_a_third_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_thread(dir.path(), Some("acme/api"));
    turn_of(&db, "U1", "Ev1");
    assert!(!start_with(&db, &start("one", None), 2).is_error);
    assert!(!start_with(&db, &start("two", None), 2).is_error);
    let third = start_with(&db, &start("three", None), 2);
    let want = "This thread already has 2 open Box(es), its limit. Close one first.";
    assert_eq!(third, Reply::refused(want));
    let names: Vec<_> = db
        .thread_boxes("C1", "1.0")
        .unwrap()
        .iter()
        .map(|b| b.box_name.as_str().to_owned())
        .collect();
    assert_eq!(names, ["slack-1-0-1", "slack-1-0-2"]);
}

#[test]
fn box_names_count_the_threads_starts_so_a_refused_start_never_lends_its_name() {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_thread(dir.path(), Some("acme/api"));
    turn_of(&db, "U1", "Ev1");
    assert!(!start_with(&db, &start("one", None), 1).is_error);
    let first = BoxName::parse_allow_bare("slack-1-0-1").unwrap();
    db.forget_starting(&first).unwrap();
    assert!(!start_with(&db, &start("again", None), 1).is_error);
    let [row] = db.thread_boxes("C1", "1.0").unwrap().try_into().unwrap();
    assert_eq!(row.box_name.as_str(), "slack-1-0-2");
}

#[test]
fn start_box_in_a_turn_without_the_starters_message_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_thread(dir.path(), Some("acme/api"));
    turn_of(&db, "U2", "Ev1");
    let reply = start_with(&db, &start("g", None), 1);
    let want = "Only <@U1> can start a Box in this thread, and <@U1> wrote nothing in this turn.";
    assert_eq!(reply, Reply::refused(want));
}

#[test]
fn a_rerun_after_a_start_refuses_a_second_start_and_names_the_box() {
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_thread(dir.path(), Some("acme/api"));
    turn_of(&db, "U1", "Ev1");
    assert!(!start_with(&db, &start("bump", None), 2).is_error);
    let lost = db.unended().unwrap().remove(0).row;
    db.rerun(&lost, 60).unwrap();
    let again = start_with(&db, &start("bump", None), 2);
    assert!(again.is_error);
    assert!(
        again.text.starts_with(
            "Your last turn already started Box `slack-1-0-1`, so no second one starts."
        ),
        "{again:?}"
    );
    assert_eq!(db.thread_boxes("C1", "1.0").unwrap().len(), 1);
}

#[test]
fn start_box_reads_the_node_and_records_it_on_the_row() {
    let args = serde_json::json!({"prompt": "g", "node": "hv"});
    let Ok(Call::StartBox { node, .. }) = Call::parse("start_box", &args) else {
        panic!("not a start_box");
    };
    assert_eq!(node.as_deref(), Some("hv"));
    let dir = tempfile::tempdir().unwrap();
    let db = fresh_thread(dir.path(), Some("acme/web"));
    turn_of(&db, "U1", "Ev1");
    let call = Call::StartBox {
        prompt: "g".into(),
        repo: None,
        base: None,
        node: node.clone(),
    };
    assert!(!start_with(&db, &call, 1).is_error);
    let [row] = db.thread_boxes("C1", "1.0").unwrap().try_into().unwrap();
    assert_eq!(row.node, node);
}
