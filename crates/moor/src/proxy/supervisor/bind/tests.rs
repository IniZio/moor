//! Tests for freeing the Box port before the tunnel binds it.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use super::super::start;
use super::super::tests::{World, FIRST_TUNNEL_PID};
use super::clear_argv;
use crate::store::TunnelWant;

/// Position of the first call starting with `prefix`.
fn position(calls: &[String], prefix: &str) -> usize {
    calls.iter().position(|c| c.starts_with(prefix)).unwrap()
}

fn sleeper() -> Child {
    Command::new("sleep")
        .arg("60")
        .stdout(Stdio::null())
        .spawn()
        .unwrap()
}

/// Puts an `ss` on `bin` that records its arguments and lists `users`.
fn fake_ss(bin: &Path, users: &str) {
    let script = format!(
        "#!/bin/sh\necho \"$@\" > '{log}'\n\
         echo 'LISTEN 0 128 127.0.0.1:41234 0.0.0.0:* users:({users})'\n",
        log = bin.join("ss.args").display(),
    );
    let path = bin.join("ss");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn run_clear(bin: &Path, port: u16) {
    let argv = clear_argv(port);
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let status = Command::new(argv.first().unwrap())
        .args(argv.iter().skip(1))
        .env("PATH", path)
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn the_command_asks_ss_for_exactly_the_tunnel_port() {
    let argv = clear_argv(41234);
    assert_eq!(argv.first().map(String::as_str), Some("sh"));
    assert_eq!(argv.get(1).map(String::as_str), Some("-c"));
    assert_eq!(argv.last().map(String::as_str), Some("41234"));
    let script = argv.get(2).unwrap();
    assert!(script.contains(r#"ss -Htlnp "sport = :$1""#), "{script}");
    assert!(script.contains(r#""sshd-session",pid="#), "{script}");
}

#[test]
fn only_sshd_session_holders_of_the_port_are_killed() {
    let bin = tempfile::tempdir().unwrap();
    let (mut ssh, mut other) = (sleeper(), sleeper());
    fake_ss(
        bin.path(),
        &format!(
            r#"("sshd-session",pid={},fd=9),("sshd",pid={},fd=3)"#,
            ssh.id(),
            other.id()
        ),
    );

    run_clear(bin.path(), 41234);

    assert!(ssh.wait().is_ok_and(|s| !s.success()));
    assert!(other.try_wait().unwrap().is_none());
    other.kill().unwrap();
    let args = std::fs::read_to_string(bin.path().join("ss.args")).unwrap();
    assert_eq!(args.trim(), "-Htlnp sport = :41234");
}

#[test]
fn nothing_is_killed_when_no_sshd_session_holds_the_port() {
    let bin = tempfile::tempdir().unwrap();
    let mut other = sleeper();
    fake_ss(bin.path(), &format!(r#"("sshd",pid={},fd=3)"#, other.id()));

    run_clear(bin.path(), 41234);

    assert!(other.try_wait().unwrap().is_none());
    other.kill().unwrap();
}

#[test]
fn the_port_is_freed_before_the_first_bind() {
    let w = World::new([]);
    let deps = w.deps();
    let _run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();

    let calls = w.driver.calls();
    assert!(position(&calls, "exec ") < position(&calls, "reverse_tunnel "));
    let execs = w.driver.execs();
    let port = w.driver.tunnels().first().unwrap().box_port;
    assert_eq!(execs.len(), 1);
    assert_eq!(execs.first().unwrap().1.argv, clear_argv(port));
}

#[test]
fn the_port_is_freed_before_a_restart_binds_it() {
    let w = World::new([]);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    w.driver.end_tunnel(FIRST_TUNNEL_PID);

    assert!(run.tick().is_none());

    let calls = w.driver.calls();
    assert_eq!(
        calls
            .iter()
            .map(String::as_str)
            .filter(|c| c.starts_with("exec "))
            .count(),
        2
    );
    assert!(calls.last().unwrap().starts_with("reverse_tunnel "));
    assert!(calls
        .iter()
        .rev()
        .nth(1)
        .expect("penultimate call")
        .starts_with("exec "));
}

#[test]
fn the_port_is_freed_before_a_wake_reopens_the_tunnel() {
    let w = World::new([]);
    let deps = w.deps();
    let run = start(&deps, w.id).map_err(|c| format!("exit {c}")).unwrap();
    w.store.request_tunnel(w.id, TunnelWant::Parked).unwrap();
    assert!(run.tick().is_none());
    w.store.request_tunnel(w.id, TunnelWant::Up).unwrap();

    assert!(run.tick().is_none());

    let calls = w.driver.calls();
    assert!(calls.last().unwrap().starts_with("reverse_tunnel "));
    assert!(calls
        .iter()
        .rev()
        .nth(1)
        .expect("penultimate call")
        .starts_with("exec "));
    assert_eq!(w.driver.execs().len(), 2);
}
