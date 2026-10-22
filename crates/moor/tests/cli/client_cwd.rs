//! The CLI resolves its caller's checkout itself and sends only what it found.

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

/// A git repo at `dir` whose `origin` is `origin`, with one commit pushed nowhere.
fn repo(dir: &Path, origin: &str) -> PathBuf {
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
    git(&["remote", "add", "origin", origin]);
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

/// Answers one request on a fresh socket with an ok reply, and returns the request it got.
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
}

#[test]
fn a_cli_start_sends_the_owner_and_name_of_its_callers_origin() {
    let work = fresh_dir("start");
    let b = repo(&work.join("b"), "git@github.com:o/n.git");
    let socket = work.join("c.sock");
    let server = capture_one(&socket);
    moor_in(&b, &socket, &["start", "n/1"], "");
    let sent = server.join().unwrap();
    assert_eq!(sent["cmd"], "start", "{sent}");
    assert_eq!(sent["repo"], "o/n", "{sent}");
    assert!(
        !sent.to_string().contains(&b.display().to_string()),
        "{sent}"
    );
}

#[test]
fn a_cli_start_in_a_checkout_of_another_forge_is_refused_before_sending() {
    let work = fresh_dir("gitlab");
    let b = repo(&work.join("b"), "https://u:tok3n@gitlab.com/o/n.git");
    let out = moor_in(&b, &work.join("none.sock"), &["start", "n/1"], "");
    let reply = String::from_utf8_lossy(&out.stdout);
    assert!(reply.contains("not a github.com repo"), "{reply}");
    assert!(!reply.contains("tok3n"), "{reply}");
}
