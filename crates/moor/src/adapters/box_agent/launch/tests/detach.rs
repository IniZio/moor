use std::io::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use moor_core::boxscript::quote;
use moor_core::seam::engine::{AgentFate, EngineError};

use super::super::holder::{Holder, HC, SESSION, TMUX};
use super::{attach, fate_by, pane_line, refusal, relay_argv, start, stop_cmd, NO_HOLDER_EXIT};

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
    let line = pane_line(
        &TMUX,
        &["ssh".into(), "-t".into(), "box".into()],
        "/w",
        "agent",
    )
    .unwrap();
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
    assert!(attach(&TMUX, "'/w/.moor/agent.exit'").contains("cat '/w/.moor/agent.exit'"));
}

#[test]
fn a_box_without_tmux_refuses_to_start_the_agent() {
    let empty = tempfile::tempdir().unwrap();
    let ran = empty.path().join("ran");
    let out = Command::new("/bin/sh")
        .arg("-c")
        .arg(start(&TMUX, "/w/x", &format!("echo > {}", ran.display())))
        .env("PATH", empty.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(i32::from(NO_HOLDER_EXIT)));
    assert!(String::from_utf8_lossy(&out.stderr).contains(&refusal(&TMUX)));
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
    let line = pane_line(
        &TMUX,
        &ssh,
        work.to_str().unwrap(),
        script.to_str().unwrap(),
    )
    .unwrap();
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

#[test]
fn a_client_that_attaches_is_sent_the_status_the_agent_last_printed() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    let server = fake_box(&bin, dir.path());
    let real = format!(
        "exec {} -L {} -f /dev/null \"$@\"",
        server.tmux.display(),
        server.socket
    );
    tool(&bin, "tmux", &real);
    let work = dir.path().join("w");
    std::fs::create_dir_all(work.join(".moor")).unwrap();
    std::fs::write(
        work.join(".moor/status-osc"),
        "state=blocked:kind=permission:app=claude",
    )
    .unwrap();
    let script = dir.path().join("agent.sh");
    std::fs::write(&script, "sleep 1\n").unwrap();
    let exit_file = work.join(".moor/agent.exit");
    let cmd = start(
        &TMUX,
        &quote(exit_file.to_str().unwrap()),
        script.to_str().unwrap(),
    );
    let out = Command::new("script")
        .args([
            "-qefc",
            &format!("stty cols 80 rows 24; {cmd}"),
            "/dev/null",
        ])
        .current_dir(&work)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("TERM", "xterm")
        .env("TMUX_TMPDIR", &server.dir)
        .env_remove("TMUX")
        .output()
        .unwrap();
    let shown = String::from_utf8_lossy(&out.stdout);
    assert!(
        shown.contains("\x1b]7501;state=blocked:kind=permission:app=claude\x1b\\"),
        "{shown:?}"
    );
}

/// What the Box tells of the agent when `tmux has-session` exits `has_session` and the workdir
/// holds `exit_file`, each Box command run by a real `sh`.
fn fate_on_box(has_session: i32, exit_file: Option<&str>) -> AgentFate {
    let fake =
        format!("[ \"$1 $2 $3\" = 'has-session -t {SESSION}' ] || exit 99; exit {has_session}");
    fate_with(&TMUX, &fake, exit_file)
}

/// What the Box tells of the agent held by `h`, whose program is the script `fake`, when the
/// workdir holds `exit_file`.
fn fate_with(h: &Holder, fake: &str, exit_file: Option<&str>) -> AgentFate {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    let work = dir.path().join("work");
    std::fs::create_dir_all(work.join(".moor")).unwrap();
    std::fs::create_dir(&bin).unwrap();
    tool(&bin, h.program, fake);
    if let Some(content) = exit_file {
        std::fs::write(work.join(".moor/agent.exit"), content).unwrap();
    }
    fate_by(h, |cmd| {
        let out = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(&work)
            .env("HOME", dir.path())
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

/// A fake herdr-core whose `get` prints a terminal in `state`, or none when `state` is empty,
/// and which logs every other call to `$HOME/calls`.
fn fake_hc(state: &str) -> String {
    format!(
        "if [ \"$1 $2\" = 'get {SESSION}' ]; then [ -n '{state}' ] || exit 3; \
         echo '{{\"name\":\"{SESSION}\",\"state\":\"{state}\"}}'; exit 0; fi; \
         echo \"$*\" >> \"$HOME/calls\""
    )
}

#[test]
fn the_box_tells_an_agent_whose_hc_terminal_is_live_is_running() {
    assert_eq!(
        fate_with(&HC, &fake_hc("live"), Some("3\n")),
        AgentFate::Running
    );
}

#[test]
fn the_box_tells_the_recorded_exit_of_an_agent_whose_hc_terminal_finished_or_is_gone() {
    for state in ["finished", ""] {
        assert_eq!(
            fate_with(&HC, &fake_hc(state), Some("3\n")),
            AgentFate::Ended { exit: Some(3) },
            "{state:?}"
        );
    }
    assert_eq!(
        fate_with(&HC, &fake_hc(""), None),
        AgentFate::Ended { exit: Some(1) }
    );
}

#[test]
fn a_finished_hc_terminal_is_not_attached_and_the_pane_gets_its_exit() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    tool(&bin, "herdr-core", &fake_hc("finished"));
    let exit_file = dir.path().join("agent.exit");
    std::fs::write(&exit_file, "5\n").unwrap();
    let out = Command::new("sh")
        .arg("-c")
        .arg(attach(&HC, &quote(exit_file.to_str().unwrap())))
        .env("HOME", dir.path())
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(5));
    assert!(
        !dir.path().join("calls").exists(),
        "attached a finished terminal"
    );
}

#[test]
fn a_box_without_herdr_core_refuses_to_start_the_agent() {
    let empty = tempfile::tempdir().unwrap();
    let ran = empty.path().join("ran");
    let out = Command::new("/bin/sh")
        .arg("-c")
        .arg(start(&HC, "/w/x", &format!("echo > {}", ran.display())))
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
            .arg(stop_cmd(&HC))
            .env("HOME", dir.path())
            .env("PATH", path)
            .status()
            .unwrap()
            .code()
    };
    assert_eq!(run(bin.display().to_string()), Some(0));
    tool(&bin, "herdr-core", &format!("{}; exit 3", fake_hc("")));
    assert_eq!(run(format!("{}:/usr/bin:/bin", bin.display())), Some(0));
    let calls = std::fs::read_to_string(dir.path().join("calls")).unwrap();
    assert_eq!(calls, format!("kill {SESSION}\n"));
}

#[test]
fn a_relayed_client_attaches_to_the_hc_terminal() {
    let argv = relay_argv(&HC, &["ssh".into(), "box".into()]);
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

/// The real herdr-core on this host's `PATH` or in `~/.local/bin`.
fn real_hc() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(|h| Path::new(&h).join(".local/bin"));
    std::env::split_paths(&std::env::var_os("PATH")?)
        .chain(home)
        .map(|d| d.join("herdr-core"))
        .find(|p| p.is_file())
}

/// A herdr-core core of the test's own, under `dir`; dropping it stops the core.
struct HcCore {
    /// The wrapper that reaches only this core.
    bin: PathBuf,
}

impl Drop for HcCore {
    fn drop(&mut self) {
        let _ = Command::new(&self.bin).args(["server", "stop"]).output();
    }
}

/// Writes into `bin` a `herdr-core` that speaks only to a core under `dir`, whose `attach` waits
/// for the terminal to end since the test has no terminal to attach.
fn private_hc(real: &Path, bin: &Path, dir: &Path) -> HcCore {
    let env = format!(
        "unset HC_SOCKET HC_SESSION HC_TERMINAL_ID HC_SCOPE; \
         export HOME={d} XDG_RUNTIME_DIR={d}/run XDG_STATE_HOME={d}/state \
         XDG_CONFIG_HOME={d}/config XDG_DATA_HOME={d}/data XDG_CACHE_HOME={d}/cache",
        d = dir.display()
    );
    std::fs::create_dir_all(dir.join("run")).unwrap();
    let real = real.display();
    tool(
        bin,
        "herdr-core",
        &format!(
            "{env}\nif [ \"$1\" = attach ]; then \
             while {real} get \"$2\" --json 2>/dev/null | grep -q '\"state\":\"live\"'; do sleep 0.05; done; exit 0; fi\n\
             exec {real} \"$@\""
        ),
    );
    HcCore {
        bin: bin.join("herdr-core"),
    }
}

#[test]
fn the_pane_line_starts_the_agent_through_real_shells_and_herdr_core() {
    let Some(real) = real_hc() else {
        let _ = writeln!(std::io::stderr(), "skipped: herdr-core is not installed");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    moor_testkit::write_host_guards(&bin).unwrap();
    tool(&bin, "ssh", "shift 2; exec bash -l -c \"$*\"");
    let _core = private_hc(&real, &bin, &dir.path().join("hc"));
    let work = dir.path().join("w");
    std::fs::create_dir_all(work.join(".moor")).unwrap();
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
    let ssh = ["ssh".into(), "-t".into(), "box".into()];
    let line = pane_line(&HC, &ssh, work.to_str().unwrap(), script.to_str().unwrap()).unwrap();
    let out = Command::new("bash")
        .arg("-c")
        .arg(&line)
        .current_dir(&work)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("HOME", dir.path())
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
    let fate = fate_by(&HC, |cmd| {
        let out = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(&work)
            .env("HOME", dir.path())
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .output()
            .unwrap();
        Ok(out.status.code().unwrap_or(-1))
    });
    assert_eq!(fate.unwrap(), AgentFate::Ended { exit: Some(7) });
}
