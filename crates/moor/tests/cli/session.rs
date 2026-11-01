//! `moor login`, `logout`, `status` and `token` share one sign-in per server, kept in the
//! person's state directory and refreshed at the issuer.
#![allow(clippy::indexing_slicing, reason = "test assertions index JSON values")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::Ordering;

use moor_testkit::oidc::Issuer;
use serde_json::json;

fn moor(state: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_moor"));
    cmd.env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", state.join("home"))
        .env("XDG_STATE_HOME", state);
    cmd
}

fn run(state: &Path, args: &[&str]) -> Output {
    moor(state).args(args).output().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Saves a sign-in to `issuer` as `moor login` would, its access token expiring at `expires_at`.
fn signed_in(state: &Path, issuer: &Issuer, expires_at: u64) {
    let creds = json!({"servers": {issuer.base.clone(): {
        "server": issuer.base, "issuer": issuer.base, "client_id": issuer.client_id,
        "token_endpoint": format!("{}/token", issuer.base), "refresh_token": "refresh-0",
        "access_token": "access-0", "expires_at": expires_at}}});
    std::fs::create_dir_all(state.join("moor")).unwrap();
    std::fs::write(state.join("moor/credentials.json"), creds.to_string()).unwrap();
}

#[test]
fn ten_token_calls_with_an_expired_token_refresh_once() {
    let issuer = Issuer::start("moor-cli");
    let dir = tempfile::tempdir().unwrap();
    signed_in(dir.path(), &issuer, 0);
    let server = format!("{}/mcp", issuer.base);
    let mut calls = Vec::new();
    for _ in 0..10 {
        let call = moor(dir.path())
            .args(["token", "--server", &server])
            .stdout(Stdio::piped())
            .spawn();
        calls.push(call.unwrap());
    }
    let tokens: Vec<String> = calls
        .into_iter()
        .map(|c| {
            let out = c.wait_with_output().unwrap();
            assert!(out.status.success(), "{}", text(&out.stderr));
            text(&out.stdout)
        })
        .collect();
    assert_eq!(issuer.refreshes.load(Ordering::SeqCst), 1);
    assert!(tokens
        .iter()
        .all(|t| t == &tokens[0] && t.trim() != "access-0"));
}

#[test]
fn token_without_a_sign_in_says_to_log_in() {
    let issuer = Issuer::start("moor-cli");
    let dir = tempfile::tempdir().unwrap();
    let out = moor(dir.path())
        .env("MOOR_SERVER", format!("{}/", issuer.base))
        .arg("token")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let base = &issuer.base;
    assert!(
        text(&out.stderr).contains(&format!(
            "not signed in to {base}; run `moor login --server {base}`"
        )),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn headers_prints_the_authorization_header_for_a_fresh_token() {
    let issuer = Issuer::start("moor-cli");
    let dir = tempfile::tempdir().unwrap();
    signed_in(dir.path(), &issuer, moor_testkit::oidc::now() + 3600);
    let out = run(
        dir.path(),
        &["token", "--server", &issuer.base, "--headers"],
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    let headers: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(headers, json!({"Authorization": "Bearer access-0"}));
    assert_eq!(issuer.refreshes.load(Ordering::SeqCst), 0);
}

/// The `Location` the issuer's authorize endpoint answers `url` with.
fn approve(url: &str) -> String {
    let rest = url.strip_prefix("http://").unwrap();
    let (host, path) = rest.split_once('/').unwrap();
    let mut stream = TcpStream::connect(host).unwrap();
    write!(stream, "GET /{path} HTTP/1.1\r\nHost: {host}\r\n\r\n").unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).unwrap();
    let line = answer
        .lines()
        .find(|l| l.starts_with("Location: "))
        .unwrap();
    line.trim_start_matches("Location: ").to_owned()
}

/// Runs `moor login --no-browser` against `issuer`, pasting the address it approves.
fn login(state: &Path, issuer: &Issuer) {
    let mut login = moor(state)
        .args(["login", "--server", &issuer.base, "--no-browser"])
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let prompt = BufReader::new(login.stderr.take().unwrap());
    let authorize = format!("{}/authorize", issuer.base);
    let url = prompt
        .lines()
        .map(Result::unwrap)
        .find(|l| l.starts_with(&authorize))
        .unwrap();
    let back = approve(&url);
    writeln!(login.stdin.take().unwrap(), "{back}").unwrap();
    assert!(login.wait().unwrap().success());
}

#[test]
fn login_asks_for_the_resource_status_shows_who_and_logout_revokes() {
    let issuer = Issuer::start("moor-cli");
    let dir = tempfile::tempdir().unwrap();
    login(dir.path(), &issuer);
    let asked = issuer.authorized.lock().unwrap()[0].clone();
    assert_eq!(asked["resource"], format!("{}/mcp", issuer.base));
    assert!(asked["scope"].contains("moor:read") && asked["scope"].contains("moor:write"));

    let status = run(dir.path(), &["status", "--server", &issuer.base]);
    let shown = text(&status.stdout);
    assert!(status.status.success(), "{}", text(&status.stderr));
    assert!(
        shown.contains("alice@example.com") && shown.contains(&issuer.base),
        "{shown}"
    );
    let token = text(&run(dir.path(), &["token", "--server", &issuer.base]).stdout);
    assert!(!shown.contains(token.trim()), "{shown}");

    let out = run(dir.path(), &["logout", "--server", &issuer.base]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(*issuer.revoked.lock().unwrap(), ["refresh-0"]);
    let after = run(dir.path(), &["token", "--server", &issuer.base]);
    assert_eq!(after.status.code(), Some(1));
    let status = run(dir.path(), &["status", "--server", &issuer.base]);
    assert!(
        text(&status.stdout).contains("not signed in"),
        "{}",
        text(&status.stdout)
    );
}
