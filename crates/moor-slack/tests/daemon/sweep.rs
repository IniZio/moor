//! The idle sweep's busy rules and schedule: which Box states keep a quiet thread's host
//! Session open, how `idle_minutes` reads, and that the sweep runs again each minute.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use serde_json::json;

use super::idle::{
    backdate, backdate_by, db, ends, quiet_thread_with_a_box, root_session, shown, thread_session,
    wait_posted, wait_root_session, wait_thread_session,
};
use super::support::controller::{failed, FakeController, Scripted};
use super::support::slack_fake::TEAM;
use super::support::{Rig, BOX_NAME, CHANNEL, TS, USER};
use super::turn::{ask, GOOD};

/// Asserts every request the controller got, the sweep's `show` and `end-host-session` among
/// them, asserted the thread's owner.
fn all_as_the_owner(controller: &FakeController) {
    let acting = controller.seen(|seen| seen.acting.clone());
    let owner = Some(format!("slack:{TEAM}/{USER}"));
    assert!(!acting.is_empty());
    assert!(acting.iter().all(|who| *who == owner), "{acting:?}");
}

#[test]
fn a_box_whose_agent_is_working_keeps_the_thread_open() {
    let (rig, controller) = quiet_thread_with_a_box(shown("working"));
    let thread = format!("{CHANNEL}/{TS}");
    rig.wait_stderr(&format!(
        "idle: thread {thread} stays open: Box {BOX_NAME} is working"
    ));
    assert_eq!(ends(&controller), [] as [i64; 0]);
    assert_eq!(root_session(&rig), Some(100));
    all_as_the_owner(&controller);
}

#[test]
fn an_asleep_box_with_no_agent_does_not_keep_the_thread_open() {
    let value = json!({"box": {"name": BOX_NAME, "lifecycle": "asleep", "agent": null}});
    let asleep = Scripted::Reply(moor_core::response::Response::ok("show", Some(value)));
    let (rig, controller) = quiet_thread_with_a_box(asleep);
    wait_root_session(&rig, None);
    assert_eq!(ends(&controller), [100]);
    rig.wait_box_state(BOX_NAME, "bound");
}

#[test]
fn a_show_cut_off_by_the_controller_keeps_the_thread_open() {
    let (rig, controller) = quiet_thread_with_a_box(Scripted::Hangup);
    let thread = format!("{CHANNEL}/{TS}");
    rig.wait_stderr(&format!(
        "idle: thread {thread} stays open: show {BOX_NAME}: "
    ));
    assert_eq!(ends(&controller), [] as [i64; 0]);
    assert_eq!(root_session(&rig), Some(100));
    rig.wait_box_state(BOX_NAME, "bound");
}

/// A second thread, opened after [`TS`] and so swept after it.
const TS2: &str = "1730000005.000200";
/// The first Box `start_box` starts in thread [`TS2`].
const BOX2: &str = "slack-1730000005-000200-1";

/// Opens thread [`TS2`] with a mention and waits until `n` turns have their line posted.
fn open_second_thread(rig: &Rig, n: i64) {
    rig.mention(2, USER, "<@UBOT> and another thing", TS2, None);
    wait_posted(rig, n);
}

#[test]
fn a_box_closing_in_slack_db_keeps_the_thread_open() {
    let (mut rig, controller, _bin) = Rig::start_boxes();
    rig.open_thread_with_a_box();
    wait_posted(&rig, 1);
    open_second_thread(&rig, 2);
    rig.kill_daemon();
    backdate(&rig);
    db(&rig)
        .execute(
            "UPDATE slack_boxes SET state = 'closing' WHERE box_name = ?1",
            [BOX_NAME],
        )
        .unwrap();
    controller.script("close", Scripted::Hold);
    rig.spawn_daemon();
    rig.slack.wait_connected();
    wait_thread_session(&rig, TS2, None);
    assert_eq!(ends(&controller), [101], "only the thread with no Box ends");
    assert_eq!(root_session(&rig), Some(100));
}

#[test]
fn a_box_with_no_record_marks_only_its_own_row_gone() {
    let (mut rig, controller, bin) = Rig::start_boxes();
    rig.open_thread_with_a_box();
    wait_posted(&rig, 1);
    let prompt = json!({"prompt": "fix the other thing"});
    Rig::tool_calls(&bin, Some(1), &[("start_box", prompt)]);
    open_second_thread(&rig, 2);
    rig.wait_box_state(BOX2, "bound");
    rig.kill_daemon();
    backdate_by(&rig, TS, 86400);
    let answer = failed("show", &format!("no Box {BOX_NAME}"));
    controller.script("show", Scripted::Reply(answer));
    rig.spawn_daemon();
    rig.slack.wait_connected();
    rig.wait_box_state(BOX_NAME, "gone");
    wait_root_session(&rig, None);
    rig.wait_box_state(BOX2, "bound");
    assert_eq!(ends(&controller), [100]);
    assert_eq!(thread_session(&rig, TS2), Some(101));
}

#[test]
fn idle_minutes_counts_minutes_not_seconds() {
    let mut rig = Rig::new();
    rig.use_fake_claude(None);
    rig.set_idle_minutes(2);
    rig.link_token(USER, GOOD);
    let controller = rig.serve(moor_core::controller::PROTOCOL);
    rig.spawn_daemon();
    rig.slack.wait_connected();
    ask(&rig, "hello there");
    wait_posted(&rig, 1);
    open_second_thread(&rig, 2);
    rig.kill_daemon();
    backdate_by(&rig, TS, 60);
    backdate_by(&rig, TS2, 180);
    rig.spawn_daemon();
    rig.slack.wait_connected();
    wait_thread_session(&rig, TS2, None);
    assert_eq!(
        ends(&controller),
        [101],
        "quiet 180 s ends, quiet 60 s does not"
    );
    assert_eq!(root_session(&rig), Some(100));
    all_as_the_owner(&controller);
}

/// The clock the in-process daemon of [`the_sweep_runs_again_a_minute_after_the_first`] reads.
static NOW: AtomicI64 = AtomicI64::new(1_800_000_000);

/// The value of [`NOW`].
fn now() -> i64 {
    NOW.load(Ordering::SeqCst)
}

#[test]
fn the_sweep_runs_again_a_minute_after_the_first() {
    let rig = Rig::new();
    rig.use_fake_claude(None);
    rig.link_token(USER, GOOD);
    let controller = rig.serve(moor_core::controller::PROTOCOL);
    let settings = moor_slack::Settings {
        socket: rig.socket(),
        config: rig.path("slack.yaml"),
        state_dir: rig.path("state-slack"),
        home: Some(rig.path("home")),
        user: None,
        path: Some("/usr/bin:/bin".into()),
        proxy_port: Some(1),
        proxy_ca_pem: Some("fake pem".to_owned()),
        egress_proxy: Vec::new(),
        clock: now,
        stop: Arc::default(),
    };
    // `run` never returns; its threads outlive the test and only touch this rig.
    let log: moor_slack::Log = Arc::new(|_: &str| {});
    std::thread::spawn(move || moor_slack::run(&settings, &log));
    rig.slack.wait_connected();
    ask(&rig, "hello there");
    wait_posted(&rig, 1);
    assert_eq!(
        ends(&controller),
        [] as [i64; 0],
        "the first sweep found nothing quiet"
    );
    NOW.fetch_add(30 * 60 + 60, Ordering::SeqCst);
    wait_root_session(&rig, None);
    assert_eq!(ends(&controller), [100]);
}
