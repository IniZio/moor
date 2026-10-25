//! Tests for the Box control tools: `show_box` is open to the thread, the other four hold the
//! requester gate, write their journal row, and send the request the controller takes.

use moor_client::ClientError;
use moor_core::name::BoxName;
use moor_core::request::Request;
use moor_core::response::{ErrorKind, ErrorView, Response};
use moor_core::seam::forge::ForgeRepo;
use serde_json::{json, Value};

use super::{turn_of, two_authors, Controller, ONE, TWO};
use crate::coordinator::tools::{Call, Reply};

impl Controller {
    /// The command name of each request sent, in order.
    fn cmds(&self) -> Vec<String> {
        let sent = self.sent.borrow();
        sent.iter().map(|r| r.cmd().to_owned()).collect()
    }
}

fn on(tool: &str, name: &str) -> Call {
    Call::parse(tool, &json!({"box": name})).unwrap()
}

fn refusal(request: &Request, message: &str) -> Response {
    let error = ErrorView {
        kind: ErrorKind::Refused,
        message: message.to_owned(),
        hints: Vec::new(),
        context: Vec::new(),
    };
    Response::err(request.cmd(), error)
}

/// What the controller says of a Box whose agent waits on a first-run screen.
fn first_run_view() -> Value {
    json!({
        "box": {
            "lifecycle": "running",
            "branch": "moor/b",
            "agent": {
                "state": "needs_input",
                "source": "first_run",
                "question": "Do you trust this folder?",
                "options": ["Yes", "No"]
            }
        },
        "pane": {"tail": "Do you trust this folder?\n> 1. Yes\n  2. No"}
    })
}

#[test]
fn show_box_is_open_to_anyone_in_the_thread_and_asks_for_the_agent_and_the_pane() {
    let dir = tempfile::tempdir().unwrap();
    let db = two_authors(dir.path());
    turn_of(&db, "U1", "Ev1");
    let controller = Controller::new();
    let reply = controller.run_with(&db, &on("show_box", TWO), &mut |r| {
        Ok(Response::ok(r.cmd(), Some(first_run_view())))
    });
    assert!(!reply.is_error, "{reply:?}");
    for want in [
        "agent needs_input",
        "first-run screen",
        "Do you trust this folder?",
        "1. Yes",
        "> 2. No",
    ] {
        assert!(reply.text.contains(want), "no `{want}` in {}", reply.text);
    }
    let sent = controller.sent.borrow();
    let [Request::Show(show)] = sent.as_slice() else {
        panic!("one show: {sent:?}");
    };
    assert_eq!(show.target.as_ref().unwrap().to_string(), TWO);
    assert!(show.agent && show.pane, "{show:?}");
    assert!(db.thread_actions("C1", "1.0").unwrap().is_empty());
}

#[test]
fn show_box_says_what_a_working_agent_does_not_wait_on() {
    let dir = tempfile::tempdir().unwrap();
    let db = two_authors(dir.path());
    turn_of(&db, "U1", "Ev1");
    let view = json!({
        "box": {"lifecycle": "running", "branch": "moor/b",
                "agent": {"state": "working", "source": "hooks", "options": []}},
        "pane": {"tail": "compiling"}
    });
    let reply = Controller::new().run_with(&db, &on("show_box", ONE), &mut |r| {
        Ok(Response::ok(r.cmd(), Some(view.clone())))
    });
    assert!(reply.text.contains("agent working"), "{reply:?}");
    assert!(!reply.text.contains("first-run"), "{reply:?}");
    assert!(reply.text.contains("> compiling"), "{reply:?}");
}

#[test]
fn show_box_of_a_box_that_does_not_run_or_is_not_the_threads_asks_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = two_authors(dir.path());
    turn_of(&db, "U1", "Ev1");
    db.mark_closing(&BoxName::parse_allow_bare(TWO).unwrap(), false)
        .unwrap();
    let controller = Controller::new();
    let closing = controller.run(&db, &on("show_box", TWO));
    assert_eq!(closing, Reply::refused(format!("Box `{TWO}` is closing.")));
    assert!(controller.run(&db, &on("show_box", "slack-9-9-1")).is_error);
    assert!(controller.sent.borrow().is_empty());
}

#[test]
fn interrupt_box_sends_the_interrupt_in_the_requesters_turn_and_journals_it() {
    let dir = tempfile::tempdir().unwrap();
    let db = two_authors(dir.path());
    let turn = turn_of(&db, "U1", "Ev1");
    let controller = Controller::new();
    let reply = controller.run(&db, &on("interrupt_box", ONE));
    assert_eq!(reply, Reply::done(format!("Interrupted Box `{ONE}`.")));
    let sent = controller.sent.borrow();
    let [Request::Steer(steer)] = sent.as_slice() else {
        panic!("one steer: {sent:?}");
    };
    assert_eq!(steer.target.to_string(), ONE);
    assert!(steer.interrupt && steer.text.is_none() && steer.menu.is_none());
    let journal = db.turn_actions(turn).unwrap();
    assert_eq!(journal[0].tool, "interrupt_box");
    assert_eq!(
        journal[0].outcome.as_deref(),
        Some(&*format!("interrupted Box `{ONE}`"))
    );
}

#[test]
fn interrupt_sleep_wake_and_restart_refuse_a_turn_without_the_requesters_message() {
    let dir = tempfile::tempdir().unwrap();
    let db = two_authors(dir.path());
    turn_of(&db, "U1", "Ev1");
    let controller = Controller::new();
    for tool in ["interrupt_box", "sleep_box", "wake_box", "restart_agent"] {
        let reply = controller.run(&db, &on(tool, TWO));
        assert_eq!(
            reply,
            Reply::refused(format!(
                "Only <@U2> can change Box `{TWO}`, and <@U2> wrote nothing in this turn."
            )),
            "{tool}"
        );
    }
    assert!(controller.sent.borrow().is_empty());
    assert!(db.thread_actions("C1", "1.0").unwrap().is_empty());
}

#[test]
fn sleep_box_and_wake_box_send_sleep_and_wake_for_the_box() {
    let dir = tempfile::tempdir().unwrap();
    let db = two_authors(dir.path());
    turn_of(&db, "U1", "Ev1");
    let controller = Controller::new();
    let first = controller.run(&db, &on("sleep_box", ONE));
    assert_eq!(first, Reply::done(format!("Put Box `{ONE}` to sleep.")));
    let second = controller.run(&db, &on("wake_box", ONE));
    assert_eq!(second, Reply::done(format!("Woke Box `{ONE}`.")));
    let sent = controller.sent.borrow();
    let [Request::Sleep(asleep), Request::Wake(awake)] = sent.as_slice() else {
        panic!("a sleep then a wake: {sent:?}");
    };
    assert_eq!(
        (asleep.box_name.as_str(), awake.box_name.as_str()),
        (ONE, ONE)
    );
    let tools: Vec<_> = db
        .thread_actions("C1", "1.0")
        .unwrap()
        .into_iter()
        .map(|a| a.tool)
        .collect();
    assert_eq!(tools, ["sleep_box", "wake_box"]);
}

#[test]
fn a_refused_or_lost_control_request_is_said_so_and_journaled() {
    let dir = tempfile::tempdir().unwrap();
    let db = two_authors(dir.path());
    let turn = turn_of(&db, "U1", "Ev1");
    let controller = Controller::new();
    let refused = controller.run_with(&db, &on("sleep_box", ONE), &mut |r| {
        Ok(refusal(r, "a run is in flight"))
    });
    assert_eq!(
        refused,
        Reply::refused("Not put to sleep: a run is in flight")
    );
    let lost = controller.run_with(&db, &on("wake_box", ONE), &mut |_| {
        Err(ClientError::Io(std::io::Error::from(
            std::io::ErrorKind::TimedOut,
        )))
    });
    assert!(lost.is_error);
    assert!(
        lost.text
            .starts_with("Maybe not woken: the controller did not answer"),
        "{lost:?}"
    );
    let journal = db.turn_actions(turn).unwrap();
    assert_eq!(
        journal[0].outcome.as_deref(),
        Some("the controller refused: a run is in flight")
    );
    assert!(journal[1]
        .outcome
        .as_deref()
        .unwrap()
        .starts_with("the controller did not answer"));
}

#[test]
fn a_box_that_does_not_run_is_not_asked_to_sleep_wake_interrupt_or_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = two_authors(dir.path());
    turn_of(&db, "U1", "Ev1");
    db.mark_closing(&BoxName::parse_allow_bare(ONE).unwrap(), false)
        .unwrap();
    let controller = Controller::new();
    for tool in ["interrupt_box", "sleep_box", "wake_box", "restart_agent"] {
        let reply = controller.run(&db, &on(tool, ONE));
        assert_eq!(
            reply,
            Reply::refused(format!("Box `{ONE}` is closing.")),
            "{tool}"
        );
    }
    assert!(controller.sent.borrow().is_empty());
}

#[test]
fn restart_agent_ends_the_session_then_resumes_it() {
    let dir = tempfile::tempdir().unwrap();
    let db = two_authors(dir.path());
    let turn = turn_of(&db, "U1", "Ev1");
    let controller = Controller::new();
    let reply = controller.run(&db, &on("restart_agent", ONE));
    assert_eq!(
        reply,
        Reply::done(format!(
            "Restarted the agent of Box `{ONE}`, resuming its session."
        ))
    );
    assert_eq!(controller.cmds(), ["end-session", "start"]);
    let sent = controller.sent.borrow();
    let (Request::EndSession(end), Request::Start(start)) = (&sent[0], &sent[1]) else {
        panic!("end-session then start: {sent:?}");
    };
    assert_eq!(end.box_name, ONE);
    assert_eq!(start.name.as_str(), ONE);
    assert!(start.resume && start.goal.is_none(), "{start:?}");
    assert_eq!(start.repo, Some(ForgeRepo::parse("acme/api").unwrap()));
    assert_eq!(db.turn_actions(turn).unwrap()[0].tool, "restart_agent");
}

#[test]
fn restart_agent_starts_the_goal_over_when_there_is_no_session_to_resume() {
    let dir = tempfile::tempdir().unwrap();
    let db = two_authors(dir.path());
    turn_of(&db, "U1", "Ev1");
    let controller = Controller::new();
    let mut starts = 0;
    let reply = controller.run_with(&db, &on("restart_agent", ONE), &mut |r| {
        if let Request::Start(_) = r {
            starts += 1;
            if starts == 1 {
                return Ok(refusal(r, "no agent session to resume"));
            }
        }
        Ok(Response::ok(r.cmd(), Some(json!({}))))
    });
    assert_eq!(
        reply,
        Reply::done(format!(
            "Restarted the agent of Box `{ONE}` from its goal; it had no session to resume."
        ))
    );
    assert_eq!(controller.cmds(), ["end-session", "start", "start"]);
    let sent = controller.sent.borrow();
    let Request::Start(again) = &sent[2] else {
        panic!("{sent:?}");
    };
    assert!(!again.resume);
    assert_eq!(again.goal.as_deref(), Some(&*format!("goal of {ONE}")));
}

#[test]
fn restart_agent_stops_when_the_session_cannot_be_ended_but_goes_on_when_none_runs() {
    let dir = tempfile::tempdir().unwrap();
    let db = two_authors(dir.path());
    turn_of(&db, "U1", "Ev1");
    let controller = Controller::new();
    let refused = controller.run_with(&db, &on("restart_agent", ONE), &mut |r| {
        Ok(refusal(r, "Box is busy"))
    });
    assert_eq!(refused, Reply::refused("Not restarted: Box is busy"));
    assert_eq!(controller.cmds(), ["end-session"]);
    let none = Controller::new();
    let reply = none.run_with(&db, &on("restart_agent", ONE), &mut |r| match r {
        Request::EndSession(_) => Ok(refusal(r, &format!("no live Session in Box {ONE}"))),
        _ => Ok(Response::ok(r.cmd(), Some(json!({})))),
    });
    assert!(!reply.is_error, "{reply:?}");
    assert_eq!(none.cmds(), ["end-session", "start"]);
}

#[test]
fn the_control_tools_parse_a_box_and_refuse_a_call_without_one() {
    for tool in [
        "show_box",
        "interrupt_box",
        "sleep_box",
        "wake_box",
        "restart_agent",
    ] {
        assert!(Call::parse(tool, &json!({"box": "b"})).is_ok(), "{tool}");
        assert_eq!(
            Call::parse(tool, &json!({})),
            Err(format!("{tool} needs a non-empty `box`."))
        );
    }
}

#[test]
fn every_control_tool_is_in_the_tool_list() {
    let names = crate::coordinator::tools::names();
    for tool in [
        "show_box",
        "interrupt_box",
        "sleep_box",
        "wake_box",
        "restart_agent",
    ] {
        assert!(names.contains(&tool), "{tool}");
    }
    assert_eq!(names.len(), 10);
}
