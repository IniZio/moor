//! Turns across a restart of the front: a stored line is posted once, a cut-off turn runs
//! once more in its placeholder, and a second loss or an old turn stops with a line.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::support::{Rig, QUIET, TS, USER};

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

/// The pid of the fake's `n`th run, once it has started.
fn run_pid(bin: &Path, n: u32) -> u32 {
    let file = bin.join(format!("run-{n}")).join("pid");
    for _ in 0..500 {
        if let Some(pid) = std::fs::read_to_string(&file)
            .ok()
            .and_then(|p| p.trim().parse().ok())
        {
            return pid;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("the fake's run {n} never started");
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
    rig.slack.wait_posts(1);
    let pid = run_pid(&bin, 0);
    (rig, bin, pid)
}

/// Kills the daemon and starts it again.
fn restart(rig: &mut Rig) {
    rig.kill_daemon();
    rig.spawn_daemon();
    rig.slack.wait_connected();
}

#[test]
fn a_turn_cut_off_by_a_restart_runs_once_more_in_its_placeholder() {
    let (mut rig, bin, pid) = hung_turn();
    rig.kill_daemon();
    rig.link_token(USER, GOOD);
    rig.spawn_daemon();
    rig.slack.wait_connected();
    let updates = rig.slack.wait_updates(1);
    let posts = rig.slack.posts();
    assert_eq!(posts.len(), 1, "the rerun posts no second placeholder");
    assert_eq!(
        (updates[0].ts.as_str(), updates[0].text.as_str()),
        (posts[0].ts.as_str(), "OK")
    );
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

#[test]
fn a_rerun_cut_off_again_stops_with_a_line() {
    let (mut rig, bin, _) = hung_turn();
    restart(&mut rig);
    let pid = run_pid(&bin, 1);
    restart(&mut rig);
    let updates = rig.slack.wait_updates(1);
    assert_eq!(updates[0].text, STOPPED);
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
    assert_eq!(rig.slack.wait_updates(1)[0].text, STOPPED);
    std::thread::sleep(QUIET);
    assert!(!bin.join("run-1").exists(), "an old turn ran again");
    assert_eq!(ends(&rig), [(Some("lost".to_owned()), None)]);
}

#[test]
fn a_line_stored_before_a_restart_is_posted_once_after_it() {
    let (mut rig, _, bin) = Rig::start_coordinated(Some(GOOD));
    rig.mention(1, USER, "<@UBOT> hello", TS, None);
    rig.slack.wait_updates(1);
    rig.slack.wait_until("the turn marked posted", |_, _| {
        db(&rig)
            .query_row("SELECT posted_at IS NOT NULL FROM slack_turns", [], |r| {
                r.get(0)
            })
            .unwrap_or(false)
    });
    rig.kill_daemon();
    db(&rig)
        .execute(
            "UPDATE slack_turns SET reply_text = 'stored answer', posted_at = NULL, ended_at = NULL",
            [],
        )
        .unwrap();
    rig.spawn_daemon();
    rig.slack.wait_connected();
    let updates = rig.slack.wait_updates(2);
    assert_eq!(
        (updates[1].ts.as_str(), updates[1].text.as_str()),
        (updates[0].ts.as_str(), "stored answer")
    );
    std::thread::sleep(QUIET);
    assert_eq!(
        rig.slack.updates().len(),
        2,
        "the stored line was posted twice"
    );
    assert!(!bin.join("run-1").exists(), "a posted turn ran again");
    assert_eq!(ends(&rig), [(Some("replied".to_owned()), None)]);
}
