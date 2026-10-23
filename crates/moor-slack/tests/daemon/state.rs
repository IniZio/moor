//! Startup: where the daemon keeps `slack.db`, a `slack.db` it cannot use, and the
//! coordinator binary it finds.

use std::path::Path;
use std::time::Duration;

use moor_core::controller::PROTOCOL;

use super::support::{Rig, TS, USER};

/// Polls until `path` exists; fails the test after ten seconds.
fn wait_for_file(path: &Path) {
    for _ in 0..500 {
        if path.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("{} never appeared", path.display());
}

#[test]
fn slack_db_defaults_to_xdg_state_home_else_home() {
    let mut rig = Rig::new();
    let xdg = rig.path("xdg");
    rig.spawn_daemon_with(|daemon| {
        daemon.env("XDG_STATE_HOME", &xdg);
    });
    wait_for_file(&xdg.join("moor-slack").join("slack.db"));
    rig.kill_daemon();
    let home_state = rig.path("home/.local/state/moor-slack");
    assert!(!home_state.exists(), "XDG_STATE_HOME wins over HOME");
    rig.spawn_daemon_with(|_| {});
    wait_for_file(&home_state.join("slack.db"));
}

#[test]
fn slack_db_at_another_schema_stops_startup_and_is_kept() {
    for version in [1, 99] {
        let mut rig = Rig::new();
        let state = rig.path("state-slack");
        std::fs::create_dir(&state).unwrap();
        let db = rusqlite::Connection::open(state.join("slack.db")).unwrap();
        db.execute_batch("CREATE TABLE old_rows (id INTEGER);")
            .unwrap();
        db.pragma_update(None, "user_version", version).unwrap();
        drop(db);
        let before = std::fs::read(state.join("slack.db")).unwrap();
        rig.spawn_daemon();
        let status = rig.wait_exit(Duration::from_secs(5));
        assert!(status.is_some_and(|s| !s.success()), "{status:?}");
        let stderr = rig.stderr();
        let refusal = format!("slack.db is schema {version} and this build needs ");
        assert!(stderr.contains(&refusal), "{stderr}");
        assert!(
            stderr.contains(". Start with a fresh slack.db."),
            "{stderr}"
        );
        assert_eq!(std::fs::read(state.join("slack.db")).unwrap(), before);
    }
}

#[test]
fn the_coordinator_version_is_logged_at_startup() {
    let mut rig = Rig::new();
    rig.use_fake_claude(None);
    rig.spawn_daemon();
    rig.wait_stderr("coordinator: Claude Code 2.1.304");
}

#[test]
fn a_claude_code_older_than_2_1_304_leaves_the_coordinator_unavailable() {
    let mut rig = Rig::new();
    rig.use_fake_claude(Some("2.1.303 (Claude Code)"));
    rig.spawn_daemon();
    rig.wait_stderr("coordinator unavailable: Claude Code 2.1.303 is older than 2.1.304");
    rig.slack.wait_connected();
    assert_eq!(
        rig.wait_exit(Duration::from_millis(200)),
        None,
        "the daemon keeps running"
    );
}

/// Spawns the daemon, whose config names no `coordinator.binary`, with `PATH` set to
/// `search`, and opens thread [`TS`] with a mention; returns the placeholder's edit.
fn ask_with_path(rig: &mut Rig, search: &str) -> String {
    rig.link_token(USER, "sk-ant-oat01-good-4c1d");
    let _controller = rig.serve(PROTOCOL);
    let state = rig.path("state-slack");
    rig.spawn_daemon_with(|daemon| {
        daemon.arg("--state-dir").arg(state).env("PATH", search);
    });
    rig.slack.wait_connected();
    rig.mention(1, USER, "<@UBOT> hello", TS, None);
    rig.slack.wait_posts(1).remove(0).text
}

#[test]
fn with_no_coordinator_binary_the_claude_on_path_coordinates() {
    let mut rig = Rig::new();
    let bin = rig.link_fake_claude();
    let search = format!("{}:/usr/bin:/bin", bin.display());
    assert_eq!(ask_with_path(&mut rig, &search), "OK");
    rig.wait_stderr("coordinator: Claude Code 2.1.304");
    assert!(
        bin.join("run-0").is_dir(),
        "the turn ran the claude on PATH"
    );
}

#[test]
fn with_no_coordinator_binary_and_no_claude_on_path_each_thread_names_the_setting() {
    let mut rig = Rig::new();
    let empty = rig.path("empty-bin");
    std::fs::create_dir(&empty).unwrap();
    let line = ask_with_path(&mut rig, &empty.display().to_string());
    let why = "found no claude on PATH; name its absolute path in coordinator.binary";
    assert_eq!(line, format!("The coordinator is unavailable: {why}."));
    rig.wait_stderr(&format!("coordinator unavailable: {why}"));
}
