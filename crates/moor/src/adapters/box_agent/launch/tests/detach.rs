use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use super::super::holder::SESSION;
use super::{relay_argv, start, stop_cmd, NO_HOLDER_EXIT};

/// Writes an executable `name` script into `bin`.
fn tool(bin: &Path, name: &str, body: &str) {
    let path = bin.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A fake herdr-core that logs every call to `$HOME/calls` and fails `get` as a missing terminal.
fn fake_hc() -> String {
    "echo \"$*\" >> \"$HOME/calls\"; [ \"$1\" != get ] || exit 3".to_owned()
}

#[test]
fn a_box_without_herdr_core_refuses_to_start_the_agent() {
    let empty = tempfile::tempdir().unwrap();
    let ran = empty.path().join("ran");
    let script = empty.path().join("agent.sh");
    std::fs::write(&script, format!("echo > {}\n", ran.display())).unwrap();
    let out = Command::new("/bin/sh")
        .arg("-c")
        .arg(start(script.to_str().unwrap()))
        .env("HOME", empty.path())
        .env("PATH", empty.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(i32::from(NO_HOLDER_EXIT)));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("moor: herdr-core is not installed on the Box"),
        "{err}"
    );
    assert!(!ran.exists(), "the agent ran on the bare link");
}

#[test]
fn stopping_kills_the_hc_terminal_and_a_box_without_herdr_core_is_already_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let run = |path: String| {
        Command::new("/bin/sh")
            .arg("-c")
            .arg(stop_cmd())
            .env("HOME", dir.path())
            .env("PATH", path)
            .status()
            .unwrap()
            .code()
    };
    assert_eq!(run(bin.display().to_string()), Some(0));
    tool(&bin, "herdr-core", &format!("{}; exit 3", fake_hc()));
    assert_eq!(run(format!("{}:/usr/bin:/bin", bin.display())), Some(0));
    let calls = std::fs::read_to_string(dir.path().join("calls")).unwrap();
    assert_eq!(calls, format!("kill {SESSION}\n"));
}

#[test]
fn a_relayed_client_attaches_to_the_hc_terminal() {
    let argv = relay_argv(&["ssh".into(), "box".into()]);
    assert_eq!(
        argv.get(..2),
        Some(&["ssh".to_owned(), "box".to_owned()][..])
    );
    assert!(
        argv.get(2)
            .unwrap()
            .ends_with(&format!("exec herdr-core attach {SESSION}")),
        "{argv:?}"
    );
}

#[test]
fn the_start_runs_the_script_file_in_a_detached_terminal_whose_exit_hc_keeps() {
    let Some(core) = crate::testing::hc::core() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let seen = dir.path().join("seen");
    let script = dir.path().join("agent.sh");
    std::fs::write(
        &script,
        format!(
            "printf '%s|%s\\n' \"it's\" '$HOME `x`' > {}\nexit 7\n",
            seen.display()
        ),
    )
    .unwrap();
    let out = Command::new("sh")
        .args(["-c", &start(script.to_str().unwrap())])
        .env("PATH", core.path())
        .env("HOME", dir.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    core.wait_get(SESSION, "\"state\":\"finished\"");
    assert!(core
        .hc(&["get", SESSION, "--json"])
        .contains("\"exit_code\":7"));
    assert_eq!(
        std::fs::read_to_string(&seen).unwrap_or_default(),
        "it's|$HOME `x`\n"
    );
}

#[test]
fn a_second_start_replaces_the_first_terminal() {
    let Some(core) = crate::testing::hc::core() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("agent.sh");
    std::fs::write(&script, "sleep 30\n").unwrap();
    let go = || {
        Command::new("sh")
            .args(["-c", &start(script.to_str().unwrap())])
            .env("PATH", core.path())
            .env("HOME", dir.path())
            .status()
            .unwrap()
            .code()
    };
    assert_eq!(go(), Some(0));
    core.wait_get(SESSION, "\"state\":\"live\"");
    assert_eq!(go(), Some(0));
    core.wait_get(SESSION, "\"state\":\"live\"");
    assert_eq!(core.hc(&["ls"]).matches(SESSION).count(), 1);
}
