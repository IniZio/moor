use std::io::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use moor_core::seam::engine::{AgentFate, EngineError};

use super::super::holder::{PROGRAM, SESSION};
use super::{attach, fate_by, pane_line, relay_argv, start, stop_cmd, NO_HOLDER_EXIT};

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
    let line = pane_line(&["ssh".into(), "-t".into(), "box".into()], "agent");
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

/// What the Box tells of the agent whose herdr-core is the script `fake`.
fn fate_with(fake: &str) -> AgentFate {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    let work = dir.path().join("work");
    std::fs::create_dir_all(work.join(".moor")).unwrap();
    std::fs::create_dir(&bin).unwrap();
    tool(&bin, PROGRAM, fake);
    fate_by(|cmd| {
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

/// A fake herdr-core whose `get` prints a terminal in `state` that ended with `exit`, or none when
/// `state` is empty, and which logs every other call to `$HOME/calls`.
fn fake_hc(state: &str, exit: Option<i32>) -> String {
    let exit = exit.map_or_else(|| "null".to_owned(), |c| c.to_string());
    format!(
        "if [ \"$1 $2\" = 'get {SESSION}' ]; then [ -n '{state}' ] || exit 3; \
         echo '{{\"exit_code\":{exit},\"name\":\"{SESSION}\",\"state\":\"{state}\"}}'; exit 0; fi; \
         echo \"$*\" >> \"$HOME/calls\""
    )
}

#[test]
fn the_box_tells_an_agent_whose_hc_terminal_is_live_is_running() {
    assert_eq!(fate_with(&fake_hc("live", None)), AgentFate::Running);
}

#[test]
fn the_box_tells_the_exit_hc_holds_of_an_agent_whose_terminal_finished() {
    assert_eq!(
        fate_with(&fake_hc("finished", Some(3))),
        AgentFate::Ended { exit: Some(3) }
    );
}

#[test]
fn an_agent_that_left_no_exit_code_or_whose_terminal_is_gone_ended_with_one() {
    for fake in [fake_hc("finished", None), fake_hc("", None)] {
        assert_eq!(
            fate_with(&fake),
            AgentFate::Ended { exit: Some(1) },
            "{fake}"
        );
    }
}

#[test]
fn a_finished_hc_terminal_is_not_attached_and_the_pane_gets_its_exit() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    tool(&bin, "herdr-core", &fake_hc("finished", Some(5)));
    let out = Command::new("sh")
        .arg("-c")
        .arg(attach())
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
    tool(
        &bin,
        "herdr-core",
        &format!("{}; exit 3", fake_hc("", None)),
    );
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
    let line = pane_line(&ssh, script.to_str().unwrap());
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
    let fate = fate_by(|cmd| {
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
