//! Startup: where the daemon keeps `slack.db`, and a `slack.db` it cannot use.

use std::path::Path;
use std::time::Duration;

use super::support::Rig;

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

#[test]
fn no_coordinator_in_the_config_leaves_it_unavailable() {
    let mut rig = Rig::new();
    rig.spawn_daemon();
    rig.wait_stderr("coordinator unavailable: the config names no coordinator.binary");
}
