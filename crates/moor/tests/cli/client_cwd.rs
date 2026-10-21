//! The CLI acts on its caller's directory, never on the controller's.

#![allow(clippy::indexing_slicing, reason = "test code")]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;

use moor_core::controller::{ServerFrame, PROTOCOL};
use moor_core::Response;
use serde_json::Value;

use super::fresh_dir;
use super::via_controller::Daemon;

/// A crabbox that reports a supported version and one provider that carries the reverse tunnel.
const FAKE_CRABBOX: &str = "#!/bin/sh\n\
    case \"$1\" in\n\
    --version) echo 'crabbox 0.79.1-moor.1' ;;\n\
    providers) echo '[{\"provider\": \"local-container\", \"features\": [\"ssh\"]}]' ;;\n\
    *) exit 1 ;;\n\
    esac\n";

/// A git repo with one commit at `dir`, by its canonical path.
fn repo(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-C"])
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    git(&["init", "-q"]);
    git(&["commit", "-q", "--allow-empty", "-m", "first"]);
    dir.canonicalize().unwrap()
}

/// Runs `moor --json <args>` in `cwd` against `socket` with `input` on stdin.
fn moor_in(cwd: &Path, socket: &Path, args: &[&str], input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_moor"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", cwd.join("home"))
        .env("MOOR_CONTROLLER", socket)
        .current_dir(cwd)
        .arg("--json")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn a_cli_start_with_no_repo_starts_from_the_callers_checkout_not_the_controllers() {
    let work = fresh_dir("cwd");
    let (a, b) = (repo(&work.join("a")), repo(&work.join("b")));
    let crabbox = work.join("crabbox");
    moor_testkit::write_executable(&crabbox, FAKE_CRABBOX).unwrap();
    let asked = work.join("herdr.log");
    let herdr = work.join("herdr");
    let record = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 1\n",
        asked.display()
    );
    moor_testkit::write_executable(&herdr, record).unwrap();
    let daemon = Daemon::start_with(|cmd| {
        cmd.current_dir(&a)
            .env("MOOR_CRABBOX_BIN", &crabbox)
            .env("MOOR_HERDR_BIN", &herdr);
    });
    let client = |args: &[&str], input: &str| moor_in(&b, &daemon.socket, args, input);
    client(&["user", "enroll"], "");
    for name in ["claude", "github"] {
        let set = client(&["user", "secret", "set", name], "not-a-token");
        assert!(set.status.success(), "{set:?}");
    }
    let started = client(&["start", "demo"], "");
    let log = std::fs::read_to_string(&asked).unwrap_or_default();
    let from_b = format!("worktree create --cwd {}", b.display());
    assert!(log.contains(&from_b), "{log}\n{started:?}");
    assert!(!log.contains(&a.display().to_string()), "{log}");
    let _ = std::fs::remove_dir_all(&work);
}

/// Answers one request on a fresh socket with a failure, and returns the request it got.
fn capture_one(socket: &Path) -> thread::JoinHandle<Value> {
    let listener = UnixListener::bind(socket).unwrap();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let hello = ServerFrame::Hello { protocol: PROTOCOL };
        stream.write_all(hello.to_line().as_bytes()).unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        let frame: Value = serde_json::from_str(&line).unwrap();
        let id = frame["id"].as_str().unwrap().to_owned();
        let envelope = Response::ok("cp", None);
        let answer = ServerFrame::Response { id, envelope };
        stream.write_all(answer.to_line().as_bytes()).unwrap();
        frame["request"].clone()
    })
}

#[test]
fn a_cli_cp_sends_its_destination_absolute_against_the_callers_directory() {
    let work = fresh_dir("cp");
    let b = work.join("b");
    std::fs::create_dir_all(&b).unwrap();
    let socket = work.join("c.sock");
    let server = capture_one(&socket);
    moor_in(&b, &socket, &["cp", "demo", "a.txt", "out/a.txt"], "");
    let sent = server.join().unwrap();
    assert_eq!(sent["cmd"], "cp", "{sent}");
    assert_eq!(sent["dest"], b.join("out/a.txt").display().to_string());
    let _ = std::fs::remove_dir_all(&work);
}
