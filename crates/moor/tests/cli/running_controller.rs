//! An in-process command never migrates a state dir whose controller lock another process holds.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};

use moor_testkit::ChildGuard;
use rusqlite::Connection;

use super::{fresh_dir, Run};

/// The store's file name inside the state dir.
const DB: &str = "moor.db";

/// A separate process holding an exclusive flock on a file; killed and reaped on drop.
struct Holder(ChildGuard);

impl Holder {
    fn lock(path: &Path) -> Self {
        let child = Command::new("perl")
            .args([
                "-e",
                "use Fcntl ':flock'; open(my $f, '>>', $ARGV[0]) or die $!; \
                 flock($f, LOCK_EX) or die $!; $| = 1; print \"locked\\n\"; sleep 300;",
            ])
            .arg(path)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut child = ChildGuard::new(child);
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let holder = Self(child);
        assert_eq!(line, "locked\n", "the helper did not take the lock");
        holder
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Writes a store no migration has run on yet, so the first open migrates it.
fn store_behind(state: &Path) {
    Connection::open(state.join(DB))
        .unwrap()
        .execute_batch("PRAGMA user_version = 0")
        .unwrap();
}

/// The highest migration recorded in the state dir's store, none before the first.
fn applied(state: &Path) -> Option<i64> {
    let conn = Connection::open(state.join(DB)).unwrap();
    conn.query_row(
        "SELECT max(version) FROM refinery_schema_history",
        [],
        |r| r.get(0),
    )
    .ok()
}

/// `moor show` run in-process on `state`, with no controller named.
fn show(state: &Path) -> Run {
    let home = fresh_dir("home");
    let out = Command::new(env!("CARGO_BIN_EXE_moor"))
        .args(["--json", "show"])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", &home)
        .env("MOOR_STATE_DIR", state)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    Run {
        code: out.status.code().unwrap(),
        stdout: String::from_utf8(out.stdout).unwrap(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

#[test]
fn a_command_does_not_migrate_a_store_a_running_controller_owns() {
    let state = fresh_dir("state");
    store_behind(&state);
    let _controller = Holder::lock(&state.join("controller.lock"));

    let r = show(&state);

    assert_ne!(r.code, 0, "stdout={:?} stderr={:?}", r.stdout, r.stderr);
    let said = format!("{}{}", r.stdout, r.stderr);
    assert!(said.contains("MOOR_CONTROLLER"), "{said}");
    assert_eq!(applied(&state), None, "left unmigrated");
}

#[test]
fn a_command_migrates_a_store_no_controller_owns() {
    let state = fresh_dir("state");
    store_behind(&state);

    let r = show(&state);

    assert_eq!(r.code, 0, "stdout={:?} stderr={:?}", r.stdout, r.stderr);
    assert!(applied(&state).is_some(), "migrated");
}
