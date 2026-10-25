use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use moor_core::seam::engine::{AgentFate, EngineError};

use super::{attach, fate_by, pane_line, start, NO_TMUX, NO_TMUX_EXIT, SESSION};

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
fn the_pane_reports_the_exit_of_its_one_link_and_never_reconnects() {
    for code in [3, 137, 255] {
        let (out, sent) = run_pane(&[code]);
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert!(
            out.trim_end()
                .ends_with(&format!("moor: agent exited {code}")),
            "{out}"
        );
    }
}

#[test]
fn the_agent_session_writes_its_exit_status_where_the_attach_reads_it() {
    let (_, sent) = run_pane(&[0]);
    let first = sent.first().unwrap();
    assert!(
        first.contains("echo $? > '\\''/w/.moor/agent.exit'\\''"),
        "{first}"
    );
    assert!(attach("'/w/.moor/agent.exit'").contains("cat '/w/.moor/agent.exit'"));
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

/// A tmux server the test started on its own socket; dropping it kills the server by its pid.
struct TmuxServer {
    /// The real tmux.
    tmux: PathBuf,
    /// The socket name, under the test's `TMUX_TMPDIR`.
    socket: String,
    /// The test's `TMUX_TMPDIR`.
    dir: PathBuf,
}

impl Drop for TmuxServer {
    fn drop(&mut self) {
        let out = Command::new(&self.tmux)
            .args([
                "-L",
                &self.socket,
                "display-message",
                "-p",
                concat!("#{", "pid}"),
            ])
            .env("TMUX_TMPDIR", &self.dir)
            .env_remove("TMUX")
            .output();
        let pid = out.ok().and_then(|o| String::from_utf8(o.stdout).ok());
        let pid = pid.and_then(|p| p.trim().parse::<i32>().ok());
        if let Some(pid) = pid.and_then(rustix::process::Pid::from_raw) {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
        }
    }
}

/// The real tmux on this host's `PATH`.
fn real_tmux() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join("tmux"))
        .find(|p| p.is_file())
}

/// Writes into `bin` an `ssh` that hands the remote command to a login shell on this host, as
/// sshd does, and a `tmux` on the test's own server under `dir`; the attach waits for the session
/// to end, since the test has no terminal to attach.
fn fake_box(bin: &Path, dir: &Path) -> TmuxServer {
    moor_testkit::write_host_guards(bin).unwrap();
    let tmux_dir = dir.join("tmux");
    std::fs::create_dir(&tmux_dir).unwrap();
    let server = TmuxServer {
        tmux: real_tmux().expect("tmux on PATH"),
        socket: format!("moor-test-{}", std::process::id()),
        dir: tmux_dir,
    };
    let real = format!(
        "{} -L {} -f /dev/null",
        server.tmux.display(),
        server.socket
    );
    tool(bin, "ssh", "shift 2; exec bash -l -c \"$*\"");
    tool(
        bin,
        "tmux",
        &format!(
            "if [ \"$1\" = attach-session ]; then \
             while {real} has-session -t {SESSION} 2>/dev/null; do sleep 0.05; done; exit 0; fi\n\
             exec {real} \"$@\""
        ),
    );
    server
}

#[test]
fn the_pane_line_starts_the_agent_through_real_shells_and_tmux() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    let server = fake_box(&bin, dir.path());
    let work = dir.path().join("w");
    std::fs::create_dir_all(work.join(".moor")).unwrap();
    let seen = dir.path().join("seen");
    let quoting = ": 'it'\\''s \"$HOME\" `id` \\\\ %s'\n".repeat(400);
    let script = dir.path().join("agent.sh");
    std::fs::write(
        &script,
        format!(
            "{quoting}printf '%s|%s\\n' \"it's\" '$HOME `x`' > {}\nexit 7\n",
            seen.display()
        ),
    )
    .unwrap();
    let ssh = ["ssh".into(), "-t".into(), "box".into()];
    let line = pane_line(&ssh, work.to_str().unwrap(), script.to_str().unwrap()).unwrap();
    let out = Command::new("bash")
        .arg("-c")
        .arg(&line)
        .current_dir(&work)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("HOME", dir.path())
        .env("TMUX_TMPDIR", &server.dir)
        .env_remove("TMUX")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.trim_end().ends_with("moor: agent exited 7"),
        "{stdout}{stderr}"
    );
    let seen = std::fs::read_to_string(&seen).unwrap_or_default();
    assert_eq!(seen, "it's|$HOME `x`\n", "{stdout}{stderr}");
}

/// What the Box tells of the agent when `tmux has-session` exits `has_session` and the workdir
/// holds `exit_file`, each Box command run by a real `sh`.
fn fate_on_box(has_session: i32, exit_file: Option<&str>) -> AgentFate {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    let work = dir.path().join("work");
    std::fs::create_dir_all(work.join(".moor")).unwrap();
    std::fs::create_dir(&bin).unwrap();
    tool(
        &bin,
        "tmux",
        &format!("[ \"$1 $2 $3\" = 'has-session -t {SESSION}' ] || exit 99; exit {has_session}"),
    );
    if let Some(content) = exit_file {
        std::fs::write(work.join(".moor/agent.exit"), content).unwrap();
    }
    fate_by(|cmd| {
        let out = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(&work)
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .output()
            .map_err(|e| EngineError::Failed {
                step: "fate",
                message: e.to_string(),
            })?;
        Ok(out.status.code().unwrap_or(-1))
    })
    .unwrap()
}

#[test]
fn the_box_tells_an_agent_whose_session_runs_is_alive() {
    assert_eq!(fate_on_box(0, Some("3\n")), AgentFate::Running);
}

#[test]
fn the_box_tells_the_recorded_exit_of_an_agent_whose_session_is_gone() {
    assert_eq!(
        fate_on_box(1, Some("3\n")),
        AgentFate::Ended { exit: Some(3) }
    );
}

#[test]
fn the_box_tells_exit_1_for_a_gone_agent_that_left_no_status() {
    assert_eq!(fate_on_box(1, None), AgentFate::Ended { exit: Some(1) });
}
