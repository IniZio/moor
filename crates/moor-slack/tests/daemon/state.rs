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
fn slack_db_written_by_a_newer_build_stops_startup() {
    let mut rig = Rig::new();
    let state = rig.path("state-slack");
    std::fs::create_dir(&state).unwrap();
    let db = rusqlite::Connection::open(state.join("slack.db")).unwrap();
    db.pragma_update(None, "user_version", 99).unwrap();
    drop(db);
    rig.spawn_daemon();
    let status = rig.wait_exit(Duration::from_secs(5));
    assert!(status.is_some_and(|s| !s.success()), "{status:?}");
    let stderr = rig.stderr();
    assert!(stderr.contains("slack.db schema 99 is newer"), "{stderr}");
}
