use std::io::Cursor;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use moor_core::seam::proc::{pump, Piped, Pipes};

use super::{kill_script, WRAPPER};
use crate::adapters::crabbox::exec::env_file;

/// The wrapper under dash in a process group of its own, as sshd starts it, running `argv` in
/// `dir` with `env` before `input` on its stdin; `group` records the process group.
fn wrapper(env: &[(String, String)], dir: &str, group: &Path, argv: &[&str]) -> (Command, Vec<u8>) {
    let env = env_file(env).unwrap();
    let mut cmd = Command::new("dash");
    cmd.args(["-c", WRAPPER, "sh", &env.len().to_string(), dir])
        .arg(group)
        .args(argv)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    (cmd, env)
}

#[test]
fn the_env_loads_and_every_stdin_byte_after_it_reaches_the_command() {
    let tmp = tempfile::tempdir().unwrap();
    let group = tmp.path().join("group");
    let dir = tmp.path().join("work");
    let env = [("GREETING".to_owned(), "it's\nhere".to_owned())];
    let script = r#"printf '%s|%s' "$GREETING" "$(pwd)" >&2; cat"#;
    let (mut cmd, mut input) = wrapper(&env, dir.to_str().unwrap(), &group, &["sh", "-c", script]);
    let data: Vec<u8> = (0..300_000u32)
        .map(|i| (i % 251) as u8)
        .chain(*b"\n~.\n~C")
        .collect();
    input.extend_from_slice(&data);
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let ended = pump(
        cmd.spawn().unwrap(),
        Vec::new(),
        Pipes {
            stdin: Box::new(Cursor::new(input)),
            stdout: &mut out,
            stderr: &mut err,
            stop: &|| false,
        },
    )
    .unwrap();
    assert!(
        out == data,
        "stdout differs from the bytes sent after the env"
    );
    let err = String::from_utf8_lossy(&err).into_owned();
    assert_eq!(err, format!("it's\nhere|{}", dir.display()));
    assert_eq!(
        ended,
        Piped {
            code: Some(0),
            stopped: false
        }
    );
    assert!(!group.exists(), "the group record outlived the command");
}

#[test]
fn the_exit_code_of_the_command_is_the_wrapper_s() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut cmd, input) = wrapper(&[], "", &tmp.path().join("g"), &["sh", "-c", "exit 7"]);
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let ended = pump(
        cmd.spawn().unwrap(),
        Vec::new(),
        Pipes {
            stdin: Box::new(Cursor::new(input)),
            stdout: &mut out,
            stderr: &mut err,
            stop: &|| false,
        },
    )
    .unwrap();
    assert_eq!(ended.code, Some(7));
}

#[test]
fn the_kill_script_hangs_up_the_recorded_group() {
    let tmp = tempfile::tempdir().unwrap();
    let group = tmp.path().join("group");
    let (mut cmd, env) = wrapper(&[], "", &group, &["sh", "-c", "sleep 30; echo late"]);
    let mut child = moor_testkit::ChildGuard::new(cmd.spawn().unwrap());
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), &env).unwrap();
    for _ in 0..200 {
        if std::fs::read_to_string(&group).is_ok_and(|g| !g.trim().is_empty()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let killed = Command::new("sh")
        .args(["-c", &kill_script(group.to_str().unwrap())])
        .status()
        .unwrap();
    assert!(killed.success());
    let out = child.wait_with_output().unwrap();
    assert!(out.stdout.is_empty(), "the command outlived the hangup");
    assert!(out.status.code().is_none(), "the wrapper was not signalled");
    assert!(!group.exists());
}
