//! Turns across a restart of the front: a stored line is posted once, a cut-off turn runs
//! once more, and a second loss or an old turn stops with a line. A turn that will not finish
//! loses the reaction it left on its thread.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use rustix::process::{kill_process_group, Pid, Signal};
use serde_json::json;

use super::support::{Rig, CHANNEL, QUIET, TS, USER};

/// A token the fake answers "OK" for.
const GOOD: &str = "sk-ant-oat01-good-4c1d";
/// A token the fake never answers for.
const HANG: &str = "sk-ant-oat01-hang-4c1d";
/// What a cut-off turn that does not run again says.
const STOPPED: &str = "Stopped: the front restarted. Send your message again.";

/// The daemon's `slack.db`.
fn db(rig: &Rig) -> rusqlite::Connection {
    rusqlite::Connection::open(rig.path("state-slack/slack.db")).unwrap()
}

/// `(end, rerun_of)` of each turn, in order.
fn ends(rig: &Rig) -> Vec<(Option<String>, Option<i64>)> {
    let db = db(rig);
    let mut stmt = db
        .prepare("SELECT end, rerun_of FROM slack_turns ORDER BY id")
        .unwrap();
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    rows.map(Result::unwrap).collect()
}

/// The pid of the fake's `n`th run, once it has written its load report and waits. A run
/// cut off before that write would die of the closed pipe, not of the restart.
fn run_pid(bin: &Path, n: u32) -> u32 {
    let run = bin.join(format!("run-{n}"));
    for _ in 0..500 {
        let pid = std::fs::read_to_string(run.join("pid"))
            .ok()
            .and_then(|p| p.trim().parse().ok());
        if let Some(pid) = pid.filter(|_| run.join("waiting").exists()) {
            return pid;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("the fake's run {n} never started waiting");
}

/// True once `pid` is gone or a zombie, waiting up to 2 s.
fn gone(pid: u32) -> bool {
    let stat = PathBuf::from(format!("/proc/{pid}/stat"));
    (0..100).any(|_| {
        let alive = std::fs::read_to_string(&stat).is_ok_and(|s| !s.contains(") Z "));
        if alive {
            std::thread::sleep(Duration::from_millis(20));
        }
        !alive
    })
}

/// Opens a thread whose first turn hangs and waits until the fake runs it.
fn hung_turn() -> (Rig, PathBuf, u32) {
    let (rig, _, bin) = Rig::start_coordinated(Some(HANG));
    rig.mention(1, USER, "<@UBOT> please do the thing", TS, None);
    rig.slack.wait_reactions(1);
    let pid = run_pid(&bin, 0);
    assert_eq!(
        rig.slack.reactions_now().len(),
        1,
        "the running turn shows its reaction"
    );
    (rig, bin, pid)
}

/// Kills the daemon and starts it again.
fn restart(rig: &mut Rig) {
    rig.kill_daemon();
    rig.spawn_daemon();
    rig.slack.wait_connected();
}

#[test]
fn a_turn_cut_off_by_a_restart_runs_once_more_and_posts_its_reply_once() {
    let (mut rig, bin, pid) = hung_turn();
    rig.kill_daemon();
    rig.link_token(USER, GOOD);
    rig.spawn_daemon();
    rig.slack.wait_connected();
    let posts = rig.slack.wait_posts(1);
    assert_eq!(posts[0].text, "OK");
    rig.slack.wait_no_reactions();
    let stderr = rig.stderr();
    assert!(!stderr.contains("reaction add failed"), "{stderr}");
    assert!(gone(pid), "the cut-off turn's process outlived the restart");
    let stdin = std::fs::read_to_string(bin.join("run-1").join("stdin")).unwrap();
    let want = "## Your last turn\nYour last turn stopped when the front restarted. \
                It already did these things:\nNothing recorded.\n\
                Do not repeat an action listed above.\n\n## New\n";
    assert!(stdin.contains(want), "{stdin}");
    assert!(stdin.ends_with("please do the thing\n"), "{stdin}");
    rig.slack.wait_until("the rerun marked replied", |_, _| {
        ends(&rig)
            == [
                (Some("lost".to_owned()), None),
                (Some("replied".to_owned()), Some(1)),
            ]
    });
}

/// A process in its own group that runs in the thread's workdir and that the front did not
/// start, like an operator's shell.
fn bystander(rig: &Rig) -> std::process::Child {
    let workdir = rig
        .path("state-slack/threads")
        .join(format!("{CHANNEL}-{TS}"));
    let mut sleep = Command::new("sleep");
    sleep.arg("60").current_dir(workdir).process_group(0);
    sleep.spawn().unwrap()
}

#[test]
fn a_process_that_only_runs_in_a_thread_workdir_survives_a_restart() {
    let (mut rig, bin, pid) = hung_turn();
    let mut shell = bystander(&rig);
    restart(&mut rig);
    run_pid(&bin, 1);
    assert!(gone(pid), "the cut-off turn's process outlived the restart");
    std::thread::sleep(QUIET);
    let alive = shell.try_wait().unwrap().is_none();
    let _ = shell.kill();
    assert!(alive, "the restart killed a process it did not start");
}

#[test]
fn a_recorded_pid_with_another_start_time_is_left_alone() {
    let (mut rig, bin, pid) = hung_turn();
    rig.kill_daemon();
    let reused = "UPDATE slack_turns SET pid_started = pid_started + 1";
    assert_eq!(db(&rig).execute(reused, []).unwrap(), 1);
    rig.spawn_daemon();
    rig.slack.wait_connected();
    run_pid(&bin, 1);
    std::thread::sleep(QUIET);
    let alive =
        std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| !s.contains(") Z "));
    if let Some(group) = i32::try_from(pid).ok().and_then(Pid::from_raw) {
        let _ = kill_process_group(group, Signal::KILL);
    }
    assert!(alive, "the restart killed a process by its pid alone");
}

#[test]
fn a_rerun_cut_off_again_stops_with_a_line() {
    let (mut rig, bin, _) = hung_turn();
    restart(&mut rig);
    let pid = run_pid(&bin, 1);
    restart(&mut rig);
    assert_eq!(rig.slack.wait_posts(1)[0].text, STOPPED);
    rig.slack.wait_no_reactions();
    assert!(gone(pid), "the rerun's process outlived the restart");
    std::thread::sleep(QUIET);
    assert!(!bin.join("run-2").exists(), "a second loss ran again");
    let lost = (Some("lost".to_owned()), None);
    assert_eq!(ends(&rig), [lost, (Some("lost".to_owned()), Some(1))]);
}

#[test]
fn a_turn_cut_off_over_an_hour_ago_stops_with_a_line() {
    let (mut rig, bin, _) = hung_turn();
    rig.kill_daemon();
    db(&rig)
        .execute("UPDATE slack_turns SET started_at = started_at - 3601", [])
        .unwrap();
    rig.spawn_daemon();
    rig.slack.wait_connected();
    assert_eq!(rig.slack.wait_posts(1)[0].text, STOPPED);
    rig.slack.wait_no_reactions();
    std::thread::sleep(QUIET);
    assert!(!bin.join("run-1").exists(), "an old turn ran again");
    assert_eq!(ends(&rig), [(Some("lost".to_owned()), None)]);
}

#[test]
fn a_line_stored_before_a_restart_is_posted_once_after_it() {
    let (mut rig, _, bin) = Rig::start_coordinated(Some(GOOD));
    rig.mention(1, USER, "<@UBOT> hello", TS, None);
    rig.slack.wait_posts(1);
    rig.slack.wait_until("the turn marked posted", |_, _| {
        db(&rig)
            .query_row("SELECT posted_at IS NOT NULL FROM slack_turns", [], |r| {
                r.get(0)
            })
            .unwrap_or(false)
    });
    rig.slack.wait_no_reactions();
    rig.kill_daemon();
    db(&rig)
        .execute(
            "UPDATE slack_turns SET reply_text = 'stored answer', posted_at = NULL, ended_at = NULL",
            [],
        )
        .unwrap();
    db(&rig)
        .execute(
            "UPDATE slack_turns SET reacted_ts = '1730000100.000009'",
            [],
        )
        .unwrap();
    let emoji = "hourglass_flowing_sand";
    rig.slack
        .leave_reaction(CHANNEL, "1730000100.000009", emoji);
    rig.spawn_daemon();
    rig.slack.wait_connected();
    let posts = rig.slack.wait_posts(2);
    assert_eq!(posts[1].text, "stored answer");
    rig.slack.wait_no_reactions();
    std::thread::sleep(QUIET);
    assert_eq!(
        rig.slack.posts().len(),
        2,
        "the stored line was posted twice"
    );
    assert!(!bin.join("run-1").exists(), "a posted turn ran again");
    assert_eq!(ends(&rig), [(Some("replied".to_owned()), None)]);
}

/// The message the only turn still reacts on.
fn reacted(rig: &Rig) -> Option<String> {
    let sql = "SELECT reacted_ts FROM slack_turns";
    db(rig).query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
fn a_reaction_left_after_the_line_was_posted_is_removed_at_restart() {
    let (mut rig, _, _) = Rig::start_coordinated(Some(GOOD));
    rig.mention(1, USER, "<@UBOT> hello", TS, None);
    rig.slack.wait_posts(1);
    rig.slack.wait_no_reactions();
    rig.slack
        .wait_until("the reaction forgotten", |_, _| reacted(&rig).is_none());
    rig.kill_daemon();
    let crash = format!("UPDATE slack_turns SET reacted_ts = '{TS}'");
    db(&rig).execute(&crash, []).unwrap();
    rig.slack
        .leave_reaction(CHANNEL, TS, "hourglass_flowing_sand");
    restart(&mut rig);
    rig.slack.wait_no_reactions();
    rig.slack
        .wait_until("the reaction forgotten", |_, _| reacted(&rig).is_none());
    assert_eq!(
        rig.slack.posts().len(),
        1,
        "the posted line is not posted again"
    );
}

/// Whether the only turn's line is marked posted.
fn posted(rig: &Rig) -> bool {
    let sql = "SELECT posted_at IS NOT NULL FROM slack_turns";
    db(rig).query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
fn a_reply_slack_never_took_stays_unposted_and_lands_after_a_restart() {
    let (mut rig, _, bin) = Rig::start_coordinated(Some(GOOD));
    let failed = json!({"ok": false, "error": "internal_error"});
    rig.slack.answer_posts(&vec![(500, failed); 4]);
    rig.mention(1, USER, "<@UBOT> hello", TS, None);
    rig.slack.wait_posts(4);
    std::thread::sleep(QUIET);
    assert!(!posted(&rig), "a line Slack never took was marked posted");
    assert_eq!(
        rig.slack.reactions_now().len(),
        1,
        "no reply, so the reaction stays"
    );
    restart(&mut rig);
    let posts = rig.slack.wait_posts(5);
    assert_eq!(posts[4].text, "OK");
    rig.slack
        .wait_until("the line marked posted", |_, _| posted(&rig));
    rig.slack.wait_no_reactions();
    assert!(!bin.join("run-1").exists(), "a stored turn ran again");
    assert_eq!(ends(&rig), [(Some("replied".to_owned()), None)]);
}

/// Slack refusing the next four `chat.postMessage` calls: a recovery post's first try and its three
/// retries.
fn refuse_a_post(rig: &Rig) {
    let failed = json!({"ok": false, "error": "internal_error"});
    rig.slack.answer_posts(&vec![(500, failed); 4]);
}

#[test]
fn a_stored_line_slack_refuses_during_recovery_stays_unposted_and_lands_once_later() {
    let (mut rig, _, _) = Rig::start_coordinated(Some(GOOD));
    refuse_a_post(&rig);
    rig.mention(1, USER, "<@UBOT> hello", TS, None);
    rig.slack.wait_posts(4);
    std::thread::sleep(QUIET);
    assert!(!posted(&rig), "a line Slack never took was marked posted");
    refuse_a_post(&rig);
    restart(&mut rig);
    rig.slack.wait_posts(8);
    std::thread::sleep(QUIET);
    assert!(!posted(&rig), "recovery marked a refused line posted");
    restart(&mut rig);
    let posts = rig.slack.wait_posts(9);
    assert_eq!(posts[8].text, "OK");
    rig.slack
        .wait_until("the line marked posted", |_, _| posted(&rig));
    std::thread::sleep(QUIET);
    assert_eq!(rig.slack.posts().len(), 9, "the line landed twice");
}

#[test]
fn a_lost_turns_line_slack_refuses_during_recovery_stays_unposted_and_lands_once_later() {
    let (mut rig, _, _) = hung_turn();
    rig.kill_daemon();
    db(&rig)
        .execute("UPDATE slack_turns SET started_at = started_at - 3601", [])
        .unwrap();
    refuse_a_post(&rig);
    rig.spawn_daemon();
    rig.slack.wait_connected();
    rig.slack.wait_posts(4);
    std::thread::sleep(QUIET);
    assert!(!posted(&rig), "recovery marked a refused line posted");
    restart(&mut rig);
    let posts = rig.slack.wait_posts(5);
    assert_eq!(posts[4].text, STOPPED);
    rig.slack
        .wait_until("the line marked posted", |_, _| posted(&rig));
    std::thread::sleep(QUIET);
    assert_eq!(rig.slack.posts().len(), 5, "the line landed twice");
}
