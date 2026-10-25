use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use super::{pane_line, start, NO_TMUX, NO_TMUX_EXIT, RECONNECTS};

/// Writes an executable `name` script into `bin`.
fn tool(bin: &Path, name: &str, body: &str) {
    let path = bin.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Runs the pane line with a fake `ssh` whose exit codes come from `codes` in call order, and a
/// `sleep` that returns at once; returns the pane output and each remote command ssh got.
fn run_pane(codes: &[i32]) -> (String, Vec<String>) {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let calls = dir.path().join("calls");
    let codes_file = dir.path().join("codes");
    let codes: Vec<String> = codes.iter().map(i32::to_string).collect();
    std::fs::write(&codes_file, codes.join("\n") + "\n").unwrap();
    tool(
        &bin,
        "ssh",
        &format!(
            "n=$(ls {calls} 2>/dev/null | wc -l); mkdir -p {calls}; printf '%s' \"$3\" > {calls}/$n\n\
             c=$(sed -n \"$((n + 1))p\" {codes}); exit \"${{c:-255}}\"",
            calls = calls.display(),
            codes = codes_file.display()
        ),
    );
    tool(&bin, "sleep", "exit 0");
    let line = pane_line(&["ssh".into(), "-t".into(), "box".into()], "/w", "agent").unwrap();
    let out = Command::new("sh")
        .arg("-c")
        .arg(line)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .output()
        .unwrap();
    let mut sent = Vec::new();
    for n in 0.. {
        match std::fs::read_to_string(calls.join(n.to_string())) {
            Ok(text) => sent.push(text),
            Err(_) => break,
        }
    }
    (String::from_utf8_lossy(&out.stdout).into_owned(), sent)
}

#[test]
fn a_lost_link_reattaches_and_the_pane_reports_the_agents_own_exit() {
    let (out, sent) = run_pane(&[255, 7]);
    assert!(out.contains("link to the Box lost, reconnecting"), "{out}");
    assert!(out.trim_end().ends_with("moor: agent exited 7"), "{out}");
    assert_eq!(sent.len(), 2, "{sent:?}");
    let (first, second) = (sent.first().unwrap(), sent.get(1).unwrap());
    assert!(
        first.contains("tmux new-session -d -s moor-agent"),
        "{first}"
    );
    assert!(
        second.contains("tmux attach-session -t moor-agent"),
        "{second}"
    );
    assert!(!second.contains("new-session"), "{second}");
}

#[test]
fn an_agent_exit_on_the_first_link_ends_without_reconnecting() {
    let (out, sent) = run_pane(&[3]);
    assert_eq!(sent.len(), 1);
    assert!(!out.contains("reconnecting"), "{out}");
    assert!(out.trim_end().ends_with("moor: agent exited 3"), "{out}");
}

#[test]
fn a_link_that_never_comes_back_gives_up_with_255() {
    let (out, sent) = run_pane(&[]);
    assert_eq!(sent.len(), 1 + RECONNECTS as usize);
    assert!(out.trim_end().ends_with("moor: agent exited 255"), "{out}");
}

#[test]
fn the_agent_session_writes_its_exit_status_where_the_attach_reads_it() {
    let (_, sent) = run_pane(&[255, 0]);
    assert_eq!(sent.len(), 2, "{sent:?}");
    let (first, second) = (sent.first().unwrap(), sent.get(1).unwrap());
    assert!(
        first.contains("echo $? > '\\''/w/.moor/agent.exit'\\''"),
        "{first}"
    );
    assert!(second.contains("cat '/w/.moor/agent.exit'"), "{second}");
}

#[test]
fn a_box_without_tmux_refuses_to_start_the_agent() {
    let empty = tempfile::tempdir().unwrap();
    let ran = empty.path().join("ran");
    let out = Command::new("/bin/sh")
        .arg("-c")
        .arg(start("/w/x", &format!("echo > {}", ran.display())))
        .env("PATH", empty.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(i32::from(NO_TMUX_EXIT)));
    assert!(String::from_utf8_lossy(&out.stderr).contains(NO_TMUX));
    assert!(!ran.exists(), "the agent ran on the bare link");
}
