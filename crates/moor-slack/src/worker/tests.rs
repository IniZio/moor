//! Tests for the start path: what a top-level mention records before its ack, and how the
//! worker settles a Box row that `start_box` wrote, including after a lost controller
//! connection.
#![allow(
    clippy::panic,
    clippy::indexing_slicing,
    clippy::unnecessary_wraps,
    reason = "test code; helpers build controller answers"
)]

use std::cell::RefCell;
use std::io;
use std::sync::mpsc;
use std::time::Duration;

use moor_client::ClientError;
use moor_core::controller::FrameErrorKind;
use moor_core::name::BoxId;
use moor_core::request::Request;
use moor_core::response::{ErrorKind, ErrorView, Response};
use serde_json::json;

use super::{record, serve, settle, Attempt, Fx, Job};
use crate::command::BoxEvent;
use moor_core::name::{BoxName, SessionId};

use crate::db::{BoxState, Db, SlackBox};
use crate::login::TokenCheck;
use crate::socket::Mention;

fn db(dir: &std::path::Path) -> Db {
    Db::open(&dir.join("slack.db")).unwrap()
}

fn mention(event_id: &str, user: &str, channel: &str, ts: &str, thread: Option<&str>) -> Mention {
    Mention {
        event_id: event_id.into(),
        channel: channel.into(),
        user: user.into(),
        text: "<@UBOT> fix the flaky test".into(),
        ts: ts.into(),
        thread_ts: thread.map(str::to_owned),
        reply: false,
    }
}

/// Records `m`.
fn try_record(db: &Db, m: &Mention) -> rusqlite::Result<Option<Job>> {
    record(m, db, 1_000)
}

/// Records `m`; the write must succeed.
fn record_one(db: &Db, m: &Mention) -> Option<Job> {
    try_record(db, m).unwrap()
}

/// Records the standard mention in thread `10.5` and returns its `starting` row.
fn recorded_start(db: &Db) -> SlackBox {
    started(db, "Ev1", "10.5")
}

/// Records thread `thread`, opened by a mention keyed `event_id`, and the `starting` row that
/// `start_box` writes for its first Box on repo `api` with host Session 100 as parent.
/// Returns the row, which the worker finds by reading `slack.db`.
fn started(db: &Db, event_id: &str, thread: &str) -> SlackBox {
    let job = record_one(db, &mention(event_id, "U1", "C1", thread, None));
    assert_eq!(job, None, "a mention opens a thread and starts nothing");
    let row = SlackBox {
        channel: "C1".into(),
        thread_ts: thread.into(),
        box_name: BoxName::parse_allow_bare(&format!("slack-{}-1", thread.replace('.', "-")))
            .unwrap(),
        repo: "acme/api".into(),
        prompt: "fix the flaky test".into(),
        base: None,
        requested_by: "U1".into(),
        parent: Some(SessionId::new(100)),
        state: BoxState::Starting,
    };
    db.insert_starting(&row, 1_000).unwrap();
    row
}

/// What the effects saw.
#[derive(Default)]
struct Seen {
    calls: Vec<Request>,
    users: Vec<String>,
    replies: Vec<(String, String, String)>,
}

/// A controller answer for one request.
type Answer = Result<Response, ClientError>;

/// The reply of a `start` or `show` that names Box 7 on branch `moor/<name>`.
fn named(cmd: &str, name: &str) -> Answer {
    let value = json!({"box": {"id": 7, "name": name, "branch": format!("moor/{name}")}});
    Ok(Response::ok(cmd, Some(value)))
}

fn refused(cmd: &str, message: &str) -> Answer {
    let error = ErrorView {
        kind: ErrorKind::Failed,
        message: message.to_owned(),
        hints: Vec::new(),
        context: Vec::new(),
        detail: None,
    };
    Ok(Response::err(cmd, error))
}

fn hung_up() -> Answer {
    Err(ClientError::Io(io::Error::from(
        io::ErrorKind::ConnectionReset,
    )))
}

fn refused_hello() -> Answer {
    Err(ClientError::Frame {
        kind: FrameErrorKind::UnsupportedProtocol,
        message: "client speaks protocol 7, controller speaks 8".into(),
    })
}

/// Runs `step` with effects that answer each request through `answer`.
fn with_fx(
    seen: &RefCell<Seen>,
    answer: &mut dyn FnMut(&Request) -> Answer,
    step: impl FnOnce(&mut Fx<'_>),
) {
    with_hooks(seen, answer, &|| {}, &|_| {}, step);
}

/// Like [`with_fx`], also running `after_post` once each post has landed and sending each
/// log line to `log`.
fn with_hooks(
    seen: &RefCell<Seen>,
    answer: &mut dyn FnMut(&Request) -> Answer,
    after_post: &dyn Fn(),
    log: &dyn Fn(&str),
    step: impl FnOnce(&mut Fx<'_>),
) {
    let mut call = |user: &str, req: Request| {
        let reply = answer(&req);
        seen.borrow_mut().users.push(user.to_owned());
        seen.borrow_mut().calls.push(req);
        reply
    };
    let mut reply = |c: &str, t: &str, text: &str| {
        seen.borrow_mut()
            .replies
            .push((c.to_owned(), t.to_owned(), text.to_owned()));
        after_post();
    };
    let mut check_token = |_: &TokenCheck| Ok(());
    let mut modal = |_: &str, _: &str| {};
    let mut respond = |_: &str, _: &str| {};
    let mut ephemeral = |_: &str, _: &str, _: &str, _: &str| {};
    let mut fx = Fx {
        call: &mut call,
        reply: &mut reply,
        ephemeral: &mut ephemeral,
        check_token: &mut check_token,
        modal: &mut modal,
        respond: &mut respond,
        log,
    };
    step(&mut fx);
}

/// Makes one `attempt` at `row`, answering each request with `answer`.
fn attempt_with(
    db: &Db,
    row: &SlackBox,
    attempt: Attempt,
    answer: &mut dyn FnMut(&Request) -> Answer,
) -> Seen {
    let seen = RefCell::default();
    with_fx(&seen, answer, |fx| {
        settle(db, fx, row, attempt);
    });
    seen.into_inner()
}

/// The command and Box name of each request, in order.
fn cmds(seen: &Seen) -> Vec<(&'static str, String)> {
    seen.calls
        .iter()
        .map(|r| match r {
            Request::Start(s) => ("start", s.name.as_str().to_owned()),
            Request::Show(s) => ("show", s.target.as_ref().unwrap().to_string()),
            _ => ("other", String::new()),
        })
        .collect()
}

fn state(db: &Db, channel: &str, thread: &str) -> Option<BoxState> {
    let boxes = db.thread_boxes(channel, thread).unwrap();
    boxes.last().map(|b| b.state.clone())
}

fn bound_to_7(name: &str) -> Option<BoxState> {
    Some(BoxState::Bound {
        box_id: BoxId::new(7),
        branch: Some(format!("moor/{name}")),
    })
}

#[test]
fn redelivered_event_does_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    recorded_start(&db);
    let again = record_one(&db, &mention("Ev1", "U1", "C1", "10.5", None));
    assert_eq!(again, None);
}

#[test]
fn failed_record_keeps_nothing_so_the_resent_mention_opens_the_thread() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let other = rusqlite::Connection::open(dir.path().join("slack.db")).unwrap();
    other
        .execute_batch(
            "CREATE TRIGGER no_items BEFORE INSERT ON slack_inbox \
             BEGIN SELECT RAISE(ABORT, 'disk full'); END;",
        )
        .unwrap();
    let m = mention("Ev1", "U1", "C1", "10.5", None);
    assert!(try_record(&db, &m).is_err());
    assert_eq!(db.thread("C1", "10.5").unwrap(), None);
    other.execute_batch("DROP TRIGGER no_items;").unwrap();
    assert!(matches!(try_record(&db, &m), Ok(None)));
    let opened = db.thread("C1", "10.5").unwrap().unwrap();
    assert_eq!(opened.owner, "U1");
    assert_eq!(
        db.inbox("C1", "10.5").unwrap()[0].text,
        "fix the flaky test"
    );
}

#[test]
fn any_user_in_any_channel_opens_a_thread() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let job = record_one(&db, &mention("Ev1", "U9", "C1", "10.5", None));
    assert_eq!(job, None);
    assert_eq!(db.thread("C1", "10.5").unwrap().unwrap().owner, "U9");
}

#[test]
fn a_mention_in_a_channel_nobody_configured_opens_a_thread() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    assert_eq!(
        record_one(&db, &mention("Ev1", "U1", "C9", "10.5", None)),
        None
    );
    assert_eq!(db.thread("C9", "10.5").unwrap().unwrap().owner, "U1");
}

#[test]
fn allowed_mention_starts_binds_and_replies() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let row = recorded_start(&db);
    let seen = attempt_with(&db, &row, Attempt::First, &mut |req| match req {
        Request::Start(s) => named("start", s.name.as_str()),
        _ => refused("show", "unexpected"),
    });
    let Some(Request::Start(req)) = seen.calls.first() else {
        panic!("no start: {:?}", cmds(&seen));
    };
    assert_eq!(seen.calls.len(), 1);
    assert_eq!(req.name.as_str(), "slack-10-5-1");
    assert_eq!(
        req.repo.as_ref().map(ToString::to_string).as_deref(),
        Some("acme/api")
    );
    assert_eq!(req.prompt.as_deref(), Some("fix the flaky test"));
    assert_eq!(
        req.parent,
        Some(SessionId::new(100)),
        "the thread's host Session"
    );
    assert_eq!(state(&db, "C1", "10.5"), bound_to_7("slack-10-5-1"));
    assert_eq!(
        seen.replies,
        [(
            "C1".into(),
            "10.5".into(),
            "Started Box `slack-10-5-1` on branch `moor/slack-10-5-1`.".into()
        )]
    );
    assert!(db.has_notice(&row, BoxEvent::Started, None).unwrap());
}

#[test]
fn failed_start_replies_and_binds_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let row = recorded_start(&db);
    let seen = attempt_with(&db, &row, Attempt::First, &mut |_| {
        refused("start", "unknown repo")
    });
    assert_eq!(seen.calls.len(), 1);
    assert_eq!(seen.replies.len(), 1);
    assert!(seen.replies[0].2.contains("unknown repo"));
    assert_eq!(state(&db, "C1", "10.5"), None);
    assert!(db.has_notice(&row, BoxEvent::NotStarted, None).unwrap());
}

#[test]
fn refused_hello_on_the_first_start_fails_it() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let row = recorded_start(&db);
    let seen = attempt_with(&db, &row, Attempt::First, &mut |_| refused_hello());
    assert_eq!(state(&db, "C1", "10.5"), None);
    assert!(seen.replies[0].2.contains("controller speaks 8"));
}

#[test]
fn lost_first_start_leaves_the_row_starting() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let row = recorded_start(&db);
    let seen = attempt_with(&db, &row, Attempt::First, &mut |_| hung_up());
    assert_eq!(cmds(&seen), [("start", "slack-10-5-1".into())]);
    assert_eq!(state(&db, "C1", "10.5"), Some(BoxState::Starting));
    assert!(seen.replies.is_empty());
}

#[test]
fn show_finds_the_box_of_a_lost_start() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let row = recorded_start(&db);
    let seen = attempt_with(&db, &row, Attempt::Again, &mut |_| {
        named("show", "slack-10-5-1")
    });
    assert_eq!(cmds(&seen), [("show", "slack-10-5-1".into())]);
    assert_eq!(state(&db, "C1", "10.5"), bound_to_7("slack-10-5-1"));
    assert!(seen.replies[0].2.starts_with("Started Box"));
}

#[test]
fn the_show_and_the_start_are_sent_as_the_user_who_asked() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let row = recorded_start(&db);
    let seen = attempt_with(&db, &row, Attempt::Again, &mut |req| match req {
        Request::Start(s) => named("start", s.name.as_str()),
        _ => refused("show", "no Box slack-10-5-1"),
    });
    assert_eq!(cmds(&seen).len(), 2);
    assert_eq!(seen.users, [row.requested_by.clone(), row.requested_by]);
}

#[test]
fn absent_box_is_started_again_with_the_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let row = recorded_start(&db);
    let seen = attempt_with(&db, &row, Attempt::Again, &mut |req| match req {
        Request::Start(s) => named("start", s.name.as_str()),
        _ => refused("show", "no Box slack-10-5-1"),
    });
    let Some(Request::Start(resent)) = seen.calls.get(1) else {
        panic!("no resend: {:?}", cmds(&seen));
    };
    assert_eq!(cmds(&seen)[0], ("show", "slack-10-5-1".into()));
    assert_eq!(resent.prompt.as_deref(), Some("fix the flaky test"));
    assert_eq!(state(&db, "C1", "10.5"), bound_to_7("slack-10-5-1"));
    assert_eq!(seen.replies.len(), 1);
}

#[test]
fn refused_hello_after_a_lost_start_leaves_the_row_starting() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let row = recorded_start(&db);
    let seen = attempt_with(&db, &row, Attempt::Again, &mut |_| refused_hello());
    assert_eq!(cmds(&seen), [("show", "slack-10-5-1".into())]);
    assert_eq!(state(&db, "C1", "10.5"), Some(BoxState::Starting));
    assert!(seen.replies.is_empty());
}

#[test]
fn show_failure_other_than_no_box_leaves_the_row_starting() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let row = recorded_start(&db);
    let seen = attempt_with(&db, &row, Attempt::Again, &mut |_| {
        refused("show", "store unavailable")
    });
    assert_eq!(cmds(&seen), [("show", "slack-10-5-1".into())]);
    assert_eq!(state(&db, "C1", "10.5"), Some(BoxState::Starting));
    assert!(seen.replies.is_empty());
}

#[test]
fn start_lost_again_leaves_the_row_starting() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let row = recorded_start(&db);
    let seen = attempt_with(&db, &row, Attempt::Again, &mut |req| match req {
        Request::Start(_) => hung_up(),
        _ => refused("show", "no Box slack-10-5-1"),
    });
    assert_eq!(seen.calls.len(), 2, "one show and one start per attempt");
    assert_eq!(state(&db, "C1", "10.5"), Some(BoxState::Starting));
    assert!(seen.replies.is_empty());
}

#[test]
fn rejected_box_found_by_show_is_a_failed_start() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    let row = recorded_start(&db);
    let seen = attempt_with(&db, &row, Attempt::Again, &mut |_| {
        let value =
            json!({"box": {"id": 7, "lifecycle": "rejected", "state_reason": "no credential"}});
        Ok(Response::ok("show", Some(value)))
    });
    assert_eq!(state(&db, "C1", "10.5"), None);
    assert_eq!(
        seen.replies[0].2,
        "Could not start Box `slack-10-5-1`: no credential"
    );
}

#[test]
fn row_left_starting_by_an_earlier_run_is_settled_first() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    recorded_start(&db);
    let left = db.starting().unwrap();
    let seen = RefCell::default();
    let (_, jobs) = mpsc::channel();
    with_fx(&seen, &mut |_| named("show", "slack-10-5-1"), |fx| {
        serve(&jobs, &db, left, fx);
    });
    let seen = seen.into_inner();
    assert_eq!(cmds(&seen), [("show", "slack-10-5-1".into())]);
    assert_eq!(state(&db, "C1", "10.5"), bound_to_7("slack-10-5-1"));
    assert_eq!(seen.replies[0].1, "10.5");
}

#[test]
fn a_lost_start_is_asked_about_on_a_timer_and_holds_up_no_other_thread() {
    let dir = tempfile::tempdir().unwrap();
    let db = db(dir.path());
    recorded_start(&db);
    started(&db, "Ev2", "20.5");
    let (tx, jobs) = mpsc::channel::<Job>();
    let feeder = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(1));
        drop(tx);
    });
    let seen = RefCell::default();
    let mut first_start = true;
    with_fx(
        &seen,
        &mut |req| match req {
            Request::Start(s) if s.name.as_str() == "slack-10-5-1" && first_start => {
                first_start = false;
                hung_up()
            }
            Request::Start(s) => named("start", s.name.as_str()),
            Request::Show(s) => named("show", &s.target.as_ref().unwrap().to_string()),
            _ => refused("other", "unexpected"),
        },
        |fx| serve(&jobs, &db, Vec::new(), fx),
    );
    feeder.join().unwrap();
    let seen = seen.into_inner();
    let order: Vec<_> = cmds(&seen);
    assert_eq!(
        order,
        [
            ("start", "slack-10-5-1".into()),
            ("start", "slack-20-5-1".into()),
            ("show", "slack-10-5-1".into()),
        ]
    );
    let threads: Vec<_> = seen.replies.iter().map(|r| r.1.as_str()).collect();
    assert_eq!(threads, ["20.5", "10.5"]);
    assert_eq!(state(&db, "C1", "10.5"), bound_to_7("slack-10-5-1"));
}

mod answer;
mod base;
mod close;
mod post_first;
mod refusal_kind;
mod start_notes;
mod stop;

/// Links the claude and github tokens in one job, where GitHub refuses its token.
#[test]
fn one_valid_and_one_refused_token_store_only_the_valid_one_and_name_the_failed() {
    use crate::account::Account;
    use crate::credentials::SlackUser;
    use moor_core::secret::Secret;

    let stored = RefCell::new(Vec::new());
    let mut call = |_: &str, req: Request| {
        if let Request::SecretSet(set) = &req {
            stored.borrow_mut().push(set.name.as_str().to_owned());
        }
        Ok(Response::ok(req.cmd(), Some(json!({}))))
    };
    let mut check_token = |check: &TokenCheck| match check.account {
        Account::Claude => Ok(()),
        Account::GitHub => Err("GitHub refused this token: 401. Nothing was stored.".to_owned()),
    };
    let shown = RefCell::new(Vec::new());
    let mut modal =
        |view: &str, text: &str| shown.borrow_mut().push((view.to_owned(), text.to_owned()));
    let mut reply = |_: &str, _: &str, _: &str| {};
    let mut respond = |_: &str, _: &str| {};
    let mut ephemeral = |_: &str, _: &str, _: &str, _: &str| {};
    let mut fx = Fx {
        call: &mut call,
        reply: &mut reply,
        ephemeral: &mut ephemeral,
        check_token: &mut check_token,
        modal: &mut modal,
        respond: &mut respond,
        log: &|_| {},
    };
    let check = |account, named| TokenCheck {
        user: SlackUser::parse("U1").unwrap(),
        account,
        view_id: "V1".into(),
        token: Secret::new(b"tok".to_vec()),
        named,
    };
    let both = [check(Account::GitHub, true), check(Account::Claude, true)];
    super::login::link(&mut fx, &both);
    super::login::link(&mut fx, &both[1..]);
    super::login::link(&mut fx, &[check(Account::Claude, false)]);
    assert_eq!(stored.into_inner(), ["claude", "claude", "claude"]);
    let want = "GitHub refused this token: 401. Nothing was stored.\nLinked Claude.";
    assert_eq!(
        shown.into_inner(),
        [
            ("V1".to_owned(), want.to_owned()),
            ("V1".to_owned(), "Linked Claude.".to_owned()),
            ("V1".to_owned(), "Linked.".to_owned()),
        ]
    );
}

#[test]
fn a_both_form_line_that_does_not_name_its_account_gets_the_account_first() {
    use crate::account::Account;
    use crate::credentials::SlackUser;
    use moor_core::secret::Secret;

    let mut call = |_: &str, req: Request| Ok(Response::ok(req.cmd(), Some(json!({}))));
    let mut check_token = |check: &TokenCheck| match check.account {
        Account::Claude => {
            Err("The coordinator is unavailable: no binary. Nothing was stored.".to_owned())
        }
        Account::GitHub => Ok(()),
    };
    let shown = RefCell::new(Vec::new());
    let mut modal = |_: &str, text: &str| shown.borrow_mut().push(text.to_owned());
    let mut reply = |_: &str, _: &str, _: &str| {};
    let mut respond = |_: &str, _: &str| {};
    let mut ephemeral = |_: &str, _: &str, _: &str, _: &str| {};
    let mut fx = Fx {
        call: &mut call,
        reply: &mut reply,
        ephemeral: &mut ephemeral,
        check_token: &mut check_token,
        modal: &mut modal,
        respond: &mut respond,
        log: &|_| {},
    };
    let check = |account| TokenCheck {
        user: SlackUser::parse("U1").unwrap(),
        account,
        view_id: "V1".into(),
        token: Secret::new(b"tok".to_vec()),
        named: true,
    };
    super::login::link(&mut fx, &[check(Account::Claude), check(Account::GitHub)]);
    let want =
        "Claude: The coordinator is unavailable: no binary. Nothing was stored.\nLinked GitHub.";
    assert_eq!(shown.into_inner(), [want]);
}
