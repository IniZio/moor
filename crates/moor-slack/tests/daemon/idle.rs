//! Quiet threads: the front ends a thread's host Session once the thread has been quiet for
//! `coordinator.idle_minutes` and none of its Boxes is busy, and the next message opens a new
//! one with the same key. A crash between a host Session request and its record in
//! `slack.db` leaves one live host Session for the thread, which a rerun adopts and a turn that
//! does not run again ends. A host Session ended outside the front is replaced before a turn;
//! a check that fails any other way keeps it. The sweep tests backdate the thread's last turn and restart the front, whose scheduler
//! sweeps once at start.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::path::Path;
use std::time::Duration;

use moor_core::request::Request;
use serde_json::json;

use super::inbox::reply;
use super::support::controller::{failed, refused, FakeController, Scripted};
use super::support::{Rig, BOX_NAME, CHANNEL, TS, USER};
use super::turn::{ask, GOOD};

/// The daemon's `slack.db`.
pub(super) fn db(rig: &Rig) -> rusqlite::Connection {
    let db = rusqlite::Connection::open(rig.path("state-slack/slack.db")).unwrap();
    db.busy_timeout(Duration::from_secs(5)).unwrap();
    db
}

/// The thread's recorded host Session.
pub(super) fn root_session(rig: &Rig) -> Option<i64> {
    thread_session(rig, TS)
}

/// The recorded host Session of thread `ts`.
pub(super) fn thread_session(rig: &Rig, ts: &str) -> Option<i64> {
    db(rig)
        .query_row(
            "SELECT root_session FROM slack_threads WHERE channel = ?1 AND thread_ts = ?2",
            [CHANNEL, ts],
            |r| r.get(0),
        )
        .unwrap()
}

/// Polls until the thread's recorded host Session is `want`; fails after ten seconds.
pub(super) fn wait_root_session(rig: &Rig, want: Option<i64>) {
    wait_thread_session(rig, TS, want);
}

/// Polls until thread `ts`'s recorded host Session is `want`; fails after ten seconds.
pub(super) fn wait_thread_session(rig: &Rig, ts: &str, want: Option<i64>) {
    for _ in 0..500 {
        if thread_session(rig, ts) == want {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!(
        "thread {ts}'s host Session stayed {:?}",
        thread_session(rig, ts)
    );
}

/// Polls until `n` turns have their line posted; fails after ten seconds.
pub(super) fn wait_posted(rig: &Rig, n: i64) {
    let posted = || {
        db(rig)
            .query_row(
                "SELECT count(*) FROM slack_turns WHERE posted_at IS NOT NULL",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
    };
    for _ in 0..500 {
        if posted() >= n {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("only {} turns posted, not {n}", posted());
}

/// Moves the thread's last turn a day back, past any idle limit.
pub(super) fn backdate(rig: &Rig) {
    db(rig)
        .execute(
            "UPDATE slack_threads SET last_turn_at = last_turn_at - 86400",
            [],
        )
        .unwrap();
}

/// Moves thread `ts`'s last turn `secs` seconds back.
pub(super) fn backdate_by(rig: &Rig, ts: &str, secs: i64) {
    db(rig)
        .execute(
            "UPDATE slack_threads SET last_turn_at = last_turn_at - ?1 WHERE thread_ts = ?2",
            rusqlite::params![secs, ts],
        )
        .unwrap();
}

/// Kills the daemon and starts it again; its scheduler sweeps for quiet threads at start.
pub(super) fn restart(rig: &mut Rig) {
    rig.kill_daemon();
    rig.spawn_daemon();
    rig.slack.wait_connected();
}

/// The key the front opens its thread's host Session with.
fn thread_key() -> String {
    format!("{CHANNEL}/{TS}")
}

/// The key of each `open-host-session` the controller was sent; none sends a workdir.
fn opens(controller: &FakeController) -> Vec<String> {
    controller.seen(|seen| {
        let open = |r: &Request| match r {
            Request::OpenHostSession(open) => {
                assert_eq!(open.workdir, None, "the front sends no workdir");
                open.key.clone()
            }
            _ => None,
        };
        seen.requests.iter().filter_map(open).collect()
    })
}

/// The Session of each `end-host-session` the controller was sent.
pub(super) fn ends(controller: &FakeController) -> Vec<i64> {
    controller.seen(|seen| {
        let end = |r: &Request| match r {
            Request::EndHostSession(end) => Some(end.session.get()),
            _ => None,
        };
        seen.requests.iter().filter_map(end).collect()
    })
}

/// The commands the controller was sent, in order.
fn commands(controller: &FakeController) -> Vec<&'static str> {
    controller.seen(|seen| seen.requests.iter().map(Request::cmd).collect())
}

/// A thread that ran one turn, made quiet, then the front restarted so it sweeps.
fn quiet_thread() -> (Rig, FakeController, std::path::PathBuf) {
    let (mut rig, controller, bin) = Rig::start_coordinated(Some(GOOD));
    ask(&rig, "hello there");
    wait_posted(&rig, 1);
    assert_eq!(root_session(&rig), Some(100));
    backdate(&rig);
    restart(&mut rig);
    (rig, controller, bin)
}

#[test]
fn a_quiet_thread_ends_its_host_session_and_the_next_message_reopens_it_with_the_same_key() {
    let (rig, controller, bin) = quiet_thread();
    controller.wait_until("end-host-session", |s| {
        s.requests
            .iter()
            .any(|r| matches!(r, Request::EndHostSession(_)))
    });
    wait_root_session(&rig, None);
    assert_eq!(ends(&controller), [100]);
    reply(&rig, 2, USER, "what did I ask?");
    rig.slack.wait_updates(2);
    let dir = thread_key();
    assert_eq!(opens(&controller), [dir.clone(), dir.clone()]);
    let hosts = controller.hosts();
    let want = [(100, dir.clone(), true), (101, dir, false)];
    assert_eq!(hosts, want, "a new host Session with the same key");
    assert_eq!(root_session(&rig), Some(101));
    let stdin = std::fs::read_to_string(bin.join("run-1").join("stdin")).unwrap();
    assert!(
        stdin.starts_with("## First request\nhello there\n"),
        "{stdin}"
    );
}

#[test]
fn a_thread_that_spoke_within_the_idle_minutes_keeps_its_host_session() {
    let (mut rig, controller, _bin) = Rig::start_coordinated(Some(GOOD));
    ask(&rig, "hello there");
    wait_posted(&rig, 1);
    restart(&mut rig);
    reply(&rig, 2, USER, "and then?");
    rig.slack.wait_updates(2);
    assert_eq!(
        ends(&controller),
        [] as [i64; 0],
        "the sweep at start ended nothing"
    );
    assert_eq!(opens(&controller).len(), 1);
    assert_eq!(root_session(&rig), Some(100));
}

/// A thread whose turn started Box [`BOX_NAME`], made quiet, with `show` answered by
/// `answer`, then the front restarted so it sweeps.
pub(super) fn quiet_thread_with_a_box(answer: Scripted) -> (Rig, FakeController) {
    let (mut rig, controller, _bin) = Rig::start_boxes();
    rig.open_thread_with_a_box();
    wait_posted(&rig, 1);
    backdate(&rig);
    controller.script("show", answer);
    restart(&mut rig);
    (rig, controller)
}

/// A `show` reply for [`BOX_NAME`] whose agent is in `state`.
pub(super) fn shown(state: &str) -> Scripted {
    let value = json!({"box": {"name": BOX_NAME, "lifecycle": "ready", "agent": {"state": state}}});
    Scripted::Reply(moor_core::response::Response::ok("show", Some(value)))
}

#[test]
fn a_box_whose_agent_is_idle_does_not_keep_the_thread_open_and_is_left_alone() {
    let (rig, controller) = quiet_thread_with_a_box(shown("idle"));
    wait_root_session(&rig, None);
    assert_eq!(ends(&controller), [100]);
    let after_start: Vec<_> = commands(&controller)
        .into_iter()
        .skip_while(|c| *c != "start")
        .skip(1)
        .collect();
    assert_eq!(
        after_start,
        ["show", "end-host-session"],
        "the Box is not touched"
    );
    rig.wait_box_state(BOX_NAME, "bound");
}

#[test]
fn a_failed_show_keeps_the_thread_open_and_changes_nothing() {
    let answer = Scripted::Reply(failed("show", "the store is busy"));
    let (rig, controller) = quiet_thread_with_a_box(answer);
    let thread = format!("{CHANNEL}/{TS}");
    let line = format!("idle: thread {thread} stays open: show {BOX_NAME}: the store is busy");
    rig.wait_stderr(&line);
    assert_eq!(ends(&controller), [] as [i64; 0]);
    assert_eq!(root_session(&rig), Some(100));
    rig.wait_box_state(BOX_NAME, "bound");
}

#[test]
fn a_box_the_controller_has_no_record_of_is_marked_gone_and_does_not_keep_the_thread_open() {
    let answer = Scripted::Reply(failed("show", &format!("no Box {BOX_NAME}")));
    let (rig, controller) = quiet_thread_with_a_box(answer);
    rig.wait_box_state(BOX_NAME, "gone");
    wait_root_session(&rig, None);
    assert_eq!(ends(&controller), [100]);
}

#[test]
fn a_box_waiting_on_a_question_keeps_the_thread_open() {
    let (rig, controller) = quiet_thread_with_a_box(shown("needs_input"));
    let thread = format!("{CHANNEL}/{TS}");
    rig.wait_stderr(&format!(
        "idle: thread {thread} stays open: Box {BOX_NAME} is needs_input"
    ));
    assert_eq!(ends(&controller), [] as [i64; 0]);
    assert_eq!(root_session(&rig), Some(100));
}

/// Kills the daemon once the controller has acted on its `cmd`, whose reply it never reads,
/// then starts it again.
fn crash_after(rig: &mut Rig, controller: &FakeController, cmd: &'static str, n: usize) {
    controller.wait_until(cmd, |s| {
        let sent = s.requests.iter().filter(|r| r.cmd() == cmd).count();
        sent >= n
    });
    restart(rig);
}

#[test]
fn a_host_session_opened_before_a_crash_is_adopted_by_its_key() {
    let (mut rig, controller, _bin) = Rig::start_coordinated(Some(GOOD));
    controller.script("open-host-session", Scripted::Lost);
    rig.mention(1, USER, "<@UBOT> hello there", TS, None);
    crash_after(&mut rig, &controller, "open-host-session", 1);
    assert_eq!(rig.slack.wait_updates(1)[0].text, "OK");
    let dir = thread_key();
    assert_eq!(controller.hosts(), [(100, dir.clone(), false)]);
    let again = "the rerun opens with the same key and gets the same Session";
    assert_eq!(opens(&controller), [dir.clone(), dir], "{again}");
    wait_root_session(&rig, Some(100));
    assert_eq!(ends(&controller), [] as [i64; 0], "the rerun keeps it");
}

#[test]
fn a_host_session_left_by_a_turn_that_does_not_run_again_is_ended_at_start() {
    let (mut rig, controller, bin) = Rig::start_coordinated(Some(GOOD));
    controller.script("open-host-session", Scripted::Lost);
    rig.mention(1, USER, "<@UBOT> hello there", TS, None);
    controller.wait_until("open-host-session", |s| {
        s.requests.iter().any(|r| r.cmd() == "open-host-session")
    });
    rig.kill_daemon();
    db(&rig)
        .execute("UPDATE slack_turns SET started_at = started_at - 3601", [])
        .unwrap();
    rig.spawn_daemon();
    rig.slack.wait_connected();
    let stopped = "Stopped: the front restarted. Send your message again.";
    assert_eq!(rig.slack.wait_updates(1)[0].text, stopped);
    controller.wait_until("end-host-session", |s| {
        s.requests.iter().any(|r| r.cmd() == "end-host-session")
    });
    let dir = thread_key();
    assert_eq!(controller.hosts(), [(100, dir, true)]);
    assert_eq!(ends(&controller), [100]);
    assert_eq!(root_session(&rig), None);
    assert!(!bin.join("run-0").exists(), "the old turn never ran");
}

/// The Session of each `update-host-session` the controller was sent.
fn touches(controller: &FakeController) -> Vec<i64> {
    controller.seen(|seen| {
        let touch = |r: &Request| match r {
            Request::UpdateHostSession(update) => Some(update.session.get()),
            _ => None,
        };
        seen.requests.iter().filter_map(touch).collect()
    })
}

#[test]
fn a_host_session_ended_outside_the_front_is_replaced_with_the_same_key_before_the_turn() {
    let (rig, controller, bin) = Rig::start_coordinated(Some(GOOD));
    ask(&rig, "hello there");
    wait_posted(&rig, 1);
    controller.end_host(100);
    reply(&rig, 2, USER, "and now?");
    assert_eq!(rig.slack.wait_updates(2)[1].text, "OK");
    let dir = thread_key();
    let want = [(100, dir.clone(), true), (101, dir, false)];
    assert_eq!(controller.hosts(), want);
    assert_eq!(touches(&controller), [100, 100, 101]);
    assert_eq!(root_session(&rig), Some(101));
    assert!(bin.join("run-1").exists(), "the turn ran");
}

#[test]
fn a_host_session_refused_again_after_the_reopen_ends_the_turn_with_its_refusal() {
    let (rig, controller, bin) = Rig::start_coordinated(Some(GOOD));
    ask(&rig, "hello there");
    wait_posted(&rig, 1);
    controller.end_host(100);
    for id in [100, 101] {
        let message = format!("host Session {id} has ended");
        let answer = Scripted::Reply(refused("update-host-session", &message));
        controller.script("update-host-session", answer);
    }
    reply(&rig, 2, USER, "and now?");
    let line = "The coordinator is unavailable: the controller did not open a host Session: \
                host Session 101 has ended.";
    assert_eq!(rig.slack.wait_updates(2)[1].text, line);
    let dir = thread_key();
    assert_eq!(
        opens(&controller),
        [dir.clone(), dir],
        "one reopen, no more"
    );
    assert_eq!(touches(&controller), [100, 100, 101]);
    assert_eq!(root_session(&rig), None);
    assert!(!bin.join("run-1").exists(), "the turn did not run");
}

/// A thread that ran one turn on host Session 100, whose check before the next turn the
/// controller answers twice with `answer`; the next message is then sent.
fn check_answered(answer: impl Fn() -> Scripted) -> (Rig, FakeController, std::path::PathBuf) {
    let (rig, controller, bin) = Rig::start_coordinated(Some(GOOD));
    ask(&rig, "hello there");
    wait_posted(&rig, 1);
    controller.script("update-host-session", answer());
    controller.script("update-host-session", answer());
    reply(&rig, 2, USER, "and now?");
    (rig, controller, bin)
}

/// The turn after [`check_answered`] ran on host Session 100, which stays recorded.
fn ran_on_the_recorded_session(rig: &Rig, controller: &FakeController, bin: &Path) {
    assert_eq!(rig.slack.wait_updates(2)[1].text, "OK");
    let dir = thread_key();
    assert_eq!(opens(controller), [dir], "no second host Session");
    assert_eq!(touches(controller), [100, 100]);
    assert_eq!(root_session(rig), Some(100));
    assert!(bin.join("run-1").exists(), "the turn ran");
}

#[test]
fn a_check_the_controller_fails_without_refusing_keeps_the_host_session() {
    let busy = || Scripted::Reply(failed("update-host-session", "the store is busy"));
    let (rig, controller, bin) = check_answered(busy);
    ran_on_the_recorded_session(&rig, &controller, &bin);
}

#[test]
fn a_check_cut_off_by_the_controller_keeps_the_host_session() {
    let (rig, controller, bin) = check_answered(|| Scripted::Hangup);
    ran_on_the_recorded_session(&rig, &controller, &bin);
}

#[test]
fn a_host_session_reopened_before_a_crash_is_adopted_and_the_ended_one_is_not() {
    let (mut rig, controller, _bin) = quiet_thread();
    wait_root_session(&rig, None);
    controller.script("open-host-session", Scripted::Lost);
    reply(&rig, 2, USER, "and now?");
    crash_after(&mut rig, &controller, "open-host-session", 2);
    rig.slack.wait_updates(2);
    let dir = thread_key();
    assert_eq!(opens(&controller), [dir.clone(), dir.clone(), dir.clone()]);
    let want = [(100, dir.clone(), true), (101, dir, false)];
    assert_eq!(controller.hosts(), want, "the third open adopts 101");
    wait_root_session(&rig, Some(101));
}

#[test]
fn an_end_cut_off_by_a_crash_is_done_once_the_controller_says_the_session_ended() {
    let (mut rig, controller, _bin) = Rig::start_coordinated(Some(GOOD));
    ask(&rig, "hello there");
    wait_posted(&rig, 1);
    backdate(&rig);
    controller.script("end-host-session", Scripted::Lost);
    restart(&mut rig);
    crash_after(&mut rig, &controller, "end-host-session", 1);
    wait_root_session(&rig, None);
    assert_eq!(ends(&controller), [100, 100]);
    reply(&rig, 2, USER, "and now?");
    rig.slack.wait_updates(2);
    let dir = thread_key();
    let want = [(100, dir.clone(), true), (101, dir, false)];
    assert_eq!(controller.hosts(), want, "the next message opens a new one");
    assert_eq!(root_session(&rig), Some(101));
}

#[test]
#[ignore = "runs real Claude Code for over two minutes; set MOOR_SLACK_LIVE_CLAUDE and \
            MOOR_SLACK_LIVE_TOKEN_FILE"]
fn a_real_claude_code_answers_from_the_rebuilt_context_after_an_idle_end() {
    let binary = std::env::var_os("MOOR_SLACK_LIVE_CLAUDE").expect("MOOR_SLACK_LIVE_CLAUDE");
    let token_file =
        std::env::var_os("MOOR_SLACK_LIVE_TOKEN_FILE").expect("MOOR_SLACK_LIVE_TOKEN_FILE");
    let token = std::fs::read_to_string(token_file).expect("read the token file");
    let mut rig = Rig::new();
    rig.set_coordinator(Path::new(&binary));
    rig.set_idle_minutes(1);
    rig.link_token(USER, token.trim());
    let controller = rig.serve(moor_core::controller::PROTOCOL);
    rig.spawn_daemon();
    rig.slack.wait_connected();
    rig.mention(1, USER, "<@UBOT> Remember the word heliotrope.", TS, None);
    wait_posted(&rig, 1);
    for _ in 0..1800 {
        if root_session(&rig).is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(root_session(&rig), None, "the host Session never ended");
    reply(&rig, 2, USER, "Which word did I ask you to remember?");
    for _ in 0..1800 {
        if db(&rig)
            .query_row(
                "SELECT count(*) FROM slack_turns WHERE posted_at IS NOT NULL",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
            >= 2
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let answer: String = db(&rig)
        .query_row(
            "SELECT reply_text FROM slack_turns WHERE end = 'replied' ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(answer.to_lowercase().contains("heliotrope"), "{answer}");
    let dir = thread_key();
    let hosts = controller.hosts();
    assert_eq!(hosts, [(100, dir.clone(), true), (101, dir, false)]);
    assert!(
        !rig.stderr().contains(token.trim()),
        "the token reached the log"
    );
}
