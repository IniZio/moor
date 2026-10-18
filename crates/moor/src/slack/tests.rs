//! Tests for the mention worker: dedupe, allow-list, start, thread binding and prompt answers.

use std::cell::RefCell;
use std::sync::Arc;

use moor_core::domain::decision::{Decision, ExtraEvent, NewTask};
use moor_core::domain::task::{SetupOutcome, TaskInput};
use moor_core::request::{CloseReq, Request, StartReq, SteerReq};
use moor_core::seam::clock::Ts;
use moor_core::BoxName;

use super::answer::{handle_click, Fx, Ui};
use super::command::Allow;
use super::handle;
use super::socket::{Click, Mention};
use crate::store::{SlackPrompt, Store};
use crate::testing::clock::FakeClock;

fn store(dir: &std::path::Path) -> Store {
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000_000)));
    Store::open(&dir.join("moor.db"), clock).unwrap()
}

fn mention(event_id: &str, user: &str, channel: &str, ts: &str, thread: Option<&str>) -> Mention {
    Mention {
        event_id: event_id.into(),
        channel: channel.into(),
        user: user.into(),
        text: "<@UBOT> start api fix the flaky test".into(),
        ts: ts.into(),
        thread_ts: thread.map(str::to_owned),
        reply: false,
    }
}

/// What the effects saw.
#[derive(Default)]
struct Seen {
    calls: Vec<Request>,
    replies: Vec<(String, String, String)>,
    logs: Vec<String>,
    ephemerals: Vec<(String, String, String)>,
    updates: Vec<(String, String, String, serde_json::Value)>,
}

/// Records ephemeral messages and edits into `Seen`.
struct FakeUi<'a>(&'a RefCell<Seen>);

impl Ui for FakeUi<'_> {
    fn ephemeral(&mut self, _channel: &str, user: &str, thread: &str, text: &str) {
        let e = (user.to_owned(), thread.to_owned(), text.to_owned());
        self.0.borrow_mut().ephemerals.push(e);
    }

    fn update(&mut self, channel: &str, ts: &str, text: &str, blocks: &serde_json::Value) {
        let u = (
            channel.to_owned(),
            ts.to_owned(),
            text.to_owned(),
            blocks.clone(),
        );
        self.0.borrow_mut().updates.push(u);
    }
}

/// Runs `handle` for `m`; `fail` makes the start effect return that error.
fn run(store: &Store, m: &Mention, fail: Option<&str>, seen: &RefCell<Seen>) {
    let channels = vec!["C1".to_owned()];
    let users = vec!["U1".to_owned()];
    let allow = Allow {
        channels: &channels,
        users: &users,
    };
    let mut call = |req: Request| {
        seen.borrow_mut().calls.push(req);
        fail.map_or(Ok(()), |m| Err(m.to_owned()))
    };
    let mut reply = |c: &str, t: &str, text: &str| {
        seen.borrow_mut()
            .replies
            .push((c.to_owned(), t.to_owned(), text.to_owned()));
    };
    let log = |line: &str| seen.borrow_mut().logs.push(line.to_owned());
    let mut ui = FakeUi(seen);
    let mut fx = Fx {
        call: &mut call,
        reply: &mut reply,
        ui: &mut ui,
        log: &log,
    };
    handle(m, store, &allow, &mut fx);
}

/// The Start requests among the calls.
fn starts(seen: &Seen) -> Vec<&StartReq> {
    seen.calls
        .iter()
        .filter_map(|r| match r {
            Request::Start(s) => Some(s),
            _ => None,
        })
        .collect()
}

fn bound(store: &Store, channel: &str, thread: &str) -> Option<BoxName> {
    store.slack_thread(channel, thread).unwrap()
}

#[test]
fn allowed_mention_starts_binds_and_replies() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let seen = RefCell::default();
    run(
        &store,
        &mention("Ev1", "U1", "C1", "10.5", None),
        None,
        &seen,
    );
    let seen = seen.into_inner();
    assert_eq!(starts(&seen).len(), 1);
    let all = starts(&seen);
    let req = all.first().unwrap();
    assert_eq!(req.name.as_str(), "slack-10-5");
    assert_eq!(req.repo.as_deref(), Some("api"));
    assert_eq!(req.goal.as_deref(), Some("fix the flaky test"));
    assert_eq!(bound(&store, "C1", "10.5").unwrap().as_str(), "slack-10-5");
    assert_eq!(seen.replies.len(), 1);
    assert_eq!(seen.replies.first().unwrap().1, "10.5");
    assert!(seen.replies.first().unwrap().2.contains("slack-10-5"));
}

#[test]
fn redelivered_event_does_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let seen = RefCell::default();
    let m = mention("Ev1", "U1", "C1", "10.5", None);
    run(&store, &m, None, &seen);
    run(&store, &m, None, &seen);
    let seen = seen.into_inner();
    assert_eq!(starts(&seen).len(), 1);
    assert_eq!(seen.replies.len(), 1);
}

#[test]
fn second_mention_in_thread_names_the_bound_task() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let seen = RefCell::default();
    run(
        &store,
        &mention("Ev1", "U1", "C1", "10.5", None),
        None,
        &seen,
    );
    let again = mention("Ev2", "U1", "C1", "11.0", Some("10.5"));
    run(&store, &again, None, &seen);
    let seen = seen.into_inner();
    assert_eq!(starts(&seen).len(), 1);
    assert_eq!(seen.replies.len(), 2);
    assert_eq!(seen.replies.get(1).unwrap().1, "10.5");
    assert!(seen.replies.get(1).unwrap().2.contains("slack-10-5"));
}

#[test]
fn disallowed_user_is_refused_without_starting() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let seen = RefCell::default();
    run(
        &store,
        &mention("Ev1", "U9", "C1", "10.5", None),
        None,
        &seen,
    );
    let seen = seen.into_inner();
    assert!(starts(&seen).is_empty());
    assert_eq!(seen.replies.len(), 1);
    assert_eq!(seen.replies.first().unwrap().1, "10.5");
    assert!(seen.replies.first().unwrap().2.contains("not allowed"));
    assert_eq!(bound(&store, "C1", "10.5"), None);
}

#[test]
fn failed_start_replies_and_binds_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let seen = RefCell::default();
    run(
        &store,
        &mention("Ev1", "U1", "C1", "10.5", None),
        Some("unknown repo"),
        &seen,
    );
    let seen = seen.into_inner();
    assert_eq!(starts(&seen).len(), 1);
    assert_eq!(seen.replies.len(), 1);
    assert!(seen.replies.first().unwrap().2.contains("unknown repo"));
    assert_eq!(bound(&store, "C1", "10.5"), None);
}

#[test]
fn disallowed_channel_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let seen = RefCell::default();
    run(
        &store,
        &mention("Ev1", "U1", "C9", "10.5", None),
        None,
        &seen,
    );
    let seen = seen.into_inner();
    assert!(starts(&seen).is_empty());
    assert!(seen.replies.is_empty());
    assert_eq!(bound(&store, "C9", "10.5"), None);
}

fn reply_to(event_id: &str, user: &str, channel: &str, text: &str) -> Mention {
    Mention {
        text: text.into(),
        reply: true,
        ..mention(event_id, user, channel, "12.0", Some("10.5"))
    }
}

/// A store with a Task bound to thread `10.5` in `C1`, owned by `U1`.
fn owned(dir: &std::path::Path) -> Store {
    let store = store(dir);
    let name = BoxName::parse_allow_bare("slack-10-5").unwrap();
    assert!(store.bind_slack_thread("C1", "10.5", &name, "U1").unwrap());
    store
}

#[test]
fn owner_reply_steers_the_task() {
    let dir = tempfile::tempdir().unwrap();
    let store = owned(dir.path());
    let seen = RefCell::default();
    run(
        &store,
        &reply_to("Ev2", "U1", "C1", "try the other test"),
        None,
        &seen,
    );
    let seen = seen.into_inner();
    let Some(Request::Steer(SteerReq { task, text, menu })) = seen.calls.first() else {
        unreachable!("expected steer, got {:?}", seen.calls);
    };
    assert_eq!(task.as_str(), "slack-10-5");
    assert_eq!(text.as_deref(), Some("try the other test"));
    assert_eq!(*menu, None);
    assert!(seen.replies.is_empty());
}

#[test]
fn other_user_reply_is_refused_with_owner_mention() {
    let dir = tempfile::tempdir().unwrap();
    let store = owned(dir.path());
    let seen = RefCell::default();
    run(
        &store,
        &reply_to("Ev2", "U2", "C1", "do something else"),
        None,
        &seen,
    );
    let seen = seen.into_inner();
    assert!(seen.calls.is_empty());
    assert_eq!(seen.replies.len(), 1);
    assert_eq!(seen.replies.first().unwrap().1, "10.5");
    assert!(seen.replies.first().unwrap().2.contains("<@U1>"));
}

#[test]
fn reply_in_unbound_thread_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let seen = RefCell::default();
    run(&store, &reply_to("Ev2", "U1", "C1", "hello"), None, &seen);
    let seen = seen.into_inner();
    assert!(seen.calls.is_empty());
    assert!(seen.replies.is_empty());
}

#[test]
fn reply_in_disallowed_channel_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let name = BoxName::parse_allow_bare("slack-10-5").unwrap();
    store.bind_slack_thread("C9", "10.5", &name, "U1").unwrap();
    let seen = RefCell::default();
    run(&store, &reply_to("Ev2", "U1", "C9", "hello"), None, &seen);
    let seen = seen.into_inner();
    assert!(seen.calls.is_empty());
    assert!(seen.replies.is_empty());
}

#[test]
fn owner_close_closes_without_force() {
    let dir = tempfile::tempdir().unwrap();
    let store = owned(dir.path());
    let seen = RefCell::default();
    run(&store, &reply_to("Ev2", "U1", "C1", "close"), None, &seen);
    let seen = seen.into_inner();
    let Some(Request::Close(CloseReq {
        task,
        force,
        trust_moor_yaml,
    })) = seen.calls.first()
    else {
        unreachable!("expected close, got {:?}", seen.calls);
    };
    assert_eq!(task.as_str(), "slack-10-5");
    assert!(!force && !trust_moor_yaml);
}

#[test]
fn close_refusal_is_posted_verbatim() {
    let dir = tempfile::tempdir().unwrap();
    let store = owned(dir.path());
    let seen = RefCell::default();
    run(
        &store,
        &reply_to("Ev2", "U1", "C1", "close"),
        Some("unmerged work in slack-10-5"),
        &seen,
    );
    let seen = seen.into_inner();
    assert_eq!(seen.replies.len(), 1);
    assert_eq!(
        seen.replies.first().unwrap().2,
        "unmerged work in slack-10-5"
    );
}

#[test]
fn redelivered_reply_is_handled_once() {
    let dir = tempfile::tempdir().unwrap();
    let store = owned(dir.path());
    let seen = RefCell::default();
    let m = reply_to("Ev2", "U1", "C1", "keep going");
    run(&store, &m, None, &seen);
    run(&store, &m, None, &seen);
    assert_eq!(seen.into_inner().calls.len(), 1);
}

/// Opens task `slack-10-5` with a `needs_input` event and records its prompt; returns the event id.
fn prompted(store: &Store) -> i64 {
    let name = BoxName::parse_allow_bare("slack-10-5").unwrap();
    let id = store
        .apply(Decision::insert(NewTask {
            name: name.clone(),
            repo: "r".into(),
        }))
        .unwrap()
        .task;
    ask(store, id);
    let event_id = store.last_waiting_marker(id).unwrap().unwrap().id;
    store
        .add_slack_prompt(&SlackPrompt {
            event_id,
            channel: "C1".into(),
            thread_ts: "10.5".into(),
            message_ts: "20.1".into(),
            task: name,
            question: "Which one?".into(),
            options: vec!["alpha".into(), "beta".into()],
            answer: None,
        })
        .unwrap();
    event_id
}

/// Appends a new `needs_input` event to the task.
fn ask(store: &Store, id: moor_core::BoxId) {
    store
        .apply(Decision::new(id).event(ExtraEvent::NeedsInput {
            question: Some("Which one?".into()),
            options: vec!["alpha".into(), "beta".into()],
        }))
        .unwrap();
}

fn click(envelope: &str, user: &str, choice: u32) -> Click {
    Click {
        envelope_id: envelope.into(),
        channel: "C1".into(),
        user: user.into(),
        message_ts: "20.1".into(),
        thread_ts: Some("10.5".into()),
        choice,
    }
}

/// Runs `handle_click` for `c`.
fn run_click(store: &Store, c: &Click, seen: &RefCell<Seen>) {
    let mut call = |req: Request| {
        seen.borrow_mut().calls.push(req);
        Ok(())
    };
    let mut reply = |c: &str, t: &str, text: &str| {
        seen.borrow_mut()
            .replies
            .push((c.to_owned(), t.to_owned(), text.to_owned()));
    };
    let mut ui = FakeUi(seen);
    let log = |line: &str| seen.borrow_mut().logs.push(line.to_owned());
    let mut fx = Fx {
        call: &mut call,
        reply: &mut reply,
        ui: &mut ui,
        log: &log,
    };
    handle_click(c, store, &mut fx);
}

/// The menu choices steered.
fn menus(seen: &Seen) -> Vec<Option<u32>> {
    seen.calls
        .iter()
        .filter_map(|r| match r {
            Request::Steer(s) => Some(s.menu),
            _ => None,
        })
        .collect()
}

#[test]
fn owner_click_steers_menu_and_updates_message() {
    let dir = tempfile::tempdir().unwrap();
    let store = owned(dir.path());
    let event_id = prompted(&store);
    let seen = RefCell::default();
    run_click(&store, &click("E1", "U1", 2), &seen);
    let seen = seen.into_inner();
    assert_eq!(menus(&seen), vec![Some(2)]);
    let Some(Request::Steer(s)) = seen.calls.first() else {
        unreachable!();
    };
    assert_eq!(s.text, None);
    let (channel, ts, text, blocks) = seen.updates.first().unwrap();
    assert_eq!((channel.as_str(), ts.as_str()), ("C1", "20.1"));
    assert!(text.contains("Which one?"));
    assert!(blocks.to_string().contains("2. beta"));
    let p = store
        .slack_prompt_by_message("C1", "20.1")
        .unwrap()
        .unwrap();
    assert_eq!(p.event_id, event_id);
    assert_eq!(p.answer.as_deref(), Some("2. beta"));
}

#[test]
fn other_user_click_gets_ephemeral_and_no_steer() {
    let dir = tempfile::tempdir().unwrap();
    let store = owned(dir.path());
    prompted(&store);
    let seen = RefCell::default();
    run_click(&store, &click("E1", "U2", 1), &seen);
    let seen = seen.into_inner();
    assert!(seen.calls.is_empty() && seen.updates.is_empty());
    let (user, thread, text) = seen.ephemerals.first().unwrap();
    assert_eq!((user.as_str(), thread.as_str()), ("U2", "10.5"));
    assert_eq!(text, "Only <@U1> can answer this prompt.");
}

#[test]
fn duplicate_envelope_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let store = owned(dir.path());
    prompted(&store);
    let seen = RefCell::default();
    run_click(&store, &click("E1", "U1", 1), &seen);
    run_click(&store, &click("E1", "U1", 1), &seen);
    let seen = seen.into_inner();
    assert_eq!(menus(&seen).len(), 1);
    assert!(seen.ephemerals.is_empty());
}

#[test]
fn already_answered_prompt_gets_ephemeral() {
    let dir = tempfile::tempdir().unwrap();
    let store = owned(dir.path());
    prompted(&store);
    let seen = RefCell::default();
    run_click(&store, &click("E1", "U1", 1), &seen);
    run_click(&store, &click("E2", "U1", 2), &seen);
    let seen = seen.into_inner();
    assert_eq!(menus(&seen).len(), 1);
    let (_, _, text) = seen.ephemerals.first().unwrap();
    assert_eq!(text, "This prompt was already answered: 1. alpha.");
}

#[test]
fn click_on_prompt_after_newer_question_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = owned(dir.path());
    prompted(&store);
    let row = store
        .open_task_by_name(&BoxName::parse_allow_bare("slack-10-5").unwrap())
        .unwrap()
        .unwrap();
    ask(&store, row.id);
    let seen = RefCell::default();
    run_click(&store, &click("E1", "U1", 1), &seen);
    let seen = seen.into_inner();
    assert!(seen.calls.is_empty() && seen.updates.is_empty());
    let (_, thread, text) = seen.replies.first().unwrap();
    assert_eq!(thread, "10.5");
    assert_eq!(
        text,
        "This prompt is no longer open: slack-10-5 is now waiting on a newer question."
    );
}

#[test]
fn click_after_agent_resumed_reports_working() {
    let dir = tempfile::tempdir().unwrap();
    let store = owned(dir.path());
    prompted(&store);
    let row = store
        .open_task_by_name(&BoxName::parse_allow_bare("slack-10-5").unwrap())
        .unwrap()
        .unwrap();
    for input in [
        TaskInput::Admitted,
        TaskInput::BoxReady {
            setup: SetupOutcome::Ok,
        },
    ] {
        store
            .apply(Decision::new(row.id).task_input(input))
            .unwrap();
    }
    let seen = RefCell::default();
    run_click(&store, &click("E1", "U1", 1), &seen);
    let seen = seen.into_inner();
    assert!(seen.calls.is_empty());
    assert!(seen.replies.first().unwrap().2.ends_with("is now working."));
}

#[test]
fn owner_digit_reply_with_open_prompt_steers_menu() {
    let dir = tempfile::tempdir().unwrap();
    let store = owned(dir.path());
    prompted(&store);
    let seen = RefCell::default();
    run(&store, &reply_to("Ev2", "U1", "C1", " 1 "), None, &seen);
    let seen = seen.into_inner();
    assert_eq!(menus(&seen), vec![Some(1)]);
    assert_eq!(seen.updates.len(), 1);
}

#[test]
fn owner_free_text_with_open_prompt_steers_text() {
    let dir = tempfile::tempdir().unwrap();
    let store = owned(dir.path());
    prompted(&store);
    let seen = RefCell::default();
    run(
        &store,
        &reply_to("Ev2", "U1", "C1", "go with alpha"),
        None,
        &seen,
    );
    let seen = seen.into_inner();
    let Some(Request::Steer(s)) = seen.calls.first() else {
        unreachable!();
    };
    assert_eq!(s.text.as_deref(), Some("go with alpha"));
    assert_eq!(s.menu, None);
    assert!(seen.updates.is_empty());
}

#[test]
fn digit_reply_without_open_prompt_is_text() {
    let dir = tempfile::tempdir().unwrap();
    let store = owned(dir.path());
    let seen = RefCell::default();
    run(&store, &reply_to("Ev2", "U1", "C1", "2"), None, &seen);
    let seen = seen.into_inner();
    let Some(Request::Steer(s)) = seen.calls.first() else {
        unreachable!();
    };
    assert_eq!(s.text.as_deref(), Some("2"));
}
