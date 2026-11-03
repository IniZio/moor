//! An in-process command never migrates a state dir whose controller lock another process holds.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};

use moor_migrate::Legacy;
use rusqlite::Connection;

use super::{fresh_dir, Run};

/// The store's file name inside the state dir.
const DB: &str = "moor.db";

/// A separate process holding an exclusive flock on a file; killed and reaped on drop.
struct Holder(Child);

impl Holder {
    fn lock(path: &Path) -> Self {
        let mut child = Command::new("perl")
            .args([
                "-e",
                "use Fcntl ':flock'; open(my $f, '>>', $ARGV[0]) or die $!; \
                 flock($f, LOCK_EX) or die $!; $| = 1; print \"locked\\n\"; sleep 300;",
            ])
            .arg(path)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
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

/// Every migration this build carries, oldest first, as `(file name, sql)`.
fn migration_files() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let mut files: Vec<(u32, String, String)> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let path = e.unwrap().path();
            let name = path.file_name().unwrap().to_str().unwrap().to_owned();
            let version = name[1..name.find("__").unwrap()].parse().unwrap();
            (version, name, std::fs::read_to_string(&path).unwrap())
        })
        .collect();
    files.sort_by_key(|(v, _, _)| *v);
    files.into_iter().map(|(_, n, s)| (n, s)).collect()
}

/// Writes a store one migration behind this build; returns that migration and the latest.
fn store_one_behind(state: &Path) -> (i64, i64) {
    let files = migration_files();
    let older: Vec<(&str, &str)> = files[..files.len() - 1]
        .iter()
        .map(|(n, s)| (n.as_str(), s.as_str()))
        .collect();
    let set = moor_migrate::migrations(&older).unwrap();
    let mut conn = Connection::open(state.join(DB)).unwrap();
    moor_migrate::migrate(&mut conn, &set, |_| Ok(Legacy::Empty)).unwrap();
    let behind = applied(state);
    (behind, behind + 1)
}

/// The highest migration recorded in the state dir's store.
fn applied(state: &Path) -> i64 {
    Connection::open(state.join(DB))
        .unwrap()
        .query_row(
            "SELECT max(version) FROM refinery_schema_history",
            [],
            |r| r.get(0),
        )
        .unwrap()
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
    let (behind, _) = store_one_behind(&state);
    let _controller = Holder::lock(&state.join("controller.lock"));

    let r = show(&state);

    assert_ne!(r.code, 0, "stdout={:?} stderr={:?}", r.stdout, r.stderr);
    let said = format!("{}{}", r.stdout, r.stderr);
    assert!(said.contains("MOOR_CONTROLLER"), "{said}");
    assert_eq!(applied(&state), behind);
}

#[test]
fn a_command_migrates_a_store_no_controller_owns() {
    let state = fresh_dir("state");
    let (_, latest) = store_one_behind(&state);

    let r = show(&state);

    assert_eq!(r.code, 0, "stdout={:?} stderr={:?}", r.stdout, r.stderr);
    assert_eq!(applied(&state), latest);
}
