//! `moor up` and `moor down` against a fake `systemctl` that runs the rendered service's
//! `ExecStart` as a real controller.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use moor_testkit::{write_executable, ChildGuard};

use super::{fresh_dir, Dir, Run};

/// A `systemctl --user` that logs each call and runs `moor.service`'s `ExecStart` itself.
const SYSTEMCTL: &str = r#"dir=$(dirname "$0")
echo "$*" >> "$dir/systemctl.log"
shift
verb=$1
units="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
pid="$dir/controller.pid"
alive() { [ -f "$pid" ] && kill -0 "$(cat "$pid")" 2>/dev/null; }
stop() {
  if alive; then
    kill "$(cat "$pid")"
    while kill -0 "$(cat "$pid")" 2>/dev/null; do sleep 0.1; done
  fi
  rm -f "$pid"
}
launch() {
  { sed -n 's/^Environment=/export /p' "$units/moor.service"
    sed -n 's/^EnvironmentFile=\(.*\)$/set -a; . "\1"; set +a/p' "$units/moor.service"
    printf 'exec '
    sed -n 's/^ExecStart=//p' "$units/moor.service"; } > "$dir/run.sh"
  setsid sh "$dir/run.sh" >> "$dir/controller.log" 2>&1 < /dev/null &
  echo $! > "$pid"
}
case "$verb" in
  start) case "$*" in *moor.service*) alive || launch ;; esac ;;
  restart) case "$*" in *" moor.service"*) stop; launch ;; esac ;;
  disable) case "$*" in *--now*) stop ;; esac ;;
esac
exit 0
"#;

/// A `docker` that answers `info` and names a bridge on loopback, which the controller can bind.
const DOCKER: &str = r#"#!/bin/sh
case "$*" in
  "info --format {{.ServerVersion}}") echo 27.0.0 ;;
  "network inspect bridge --format {{(index .IPAM.Config 0).Gateway}}") echo 127.0.0.1 ;;
  *) echo "fake docker: $*" >&2; exit 2 ;;
esac
"#;

/// A temp HOME with fake tools on `PATH`; stops the controller it ran on drop.
struct Host {
    home: Dir,
    state: tempfile::TempDir,
    bin: Dir,
}

impl Host {
    fn new() -> Self {
        Self::with_docker(true)
    }

    /// A host with or without a working `docker`.
    fn with_docker(works: bool) -> Self {
        let bin = fresh_dir("up-bin");
        write_executable(
            &bin.join("docker"),
            if works {
                DOCKER
            } else {
                "#!/bin/sh\necho 'permission denied' >&2\nexit 1\n"
            },
        )
        .unwrap();
        for tool in ["moor-machine-runner", "moor-machine-agent", "moor-gate"] {
            write_executable(&bin.join(tool), "#!/bin/sh\nexit 0\n").unwrap();
        }
        write_executable(&bin.join("systemctl"), format!("#!/bin/sh\n{SYSTEMCTL}")).unwrap();
        write_executable(&bin.join("moor-egress-forward"), "#!/bin/sh\nexit 0\n").unwrap();
        Self {
            home: fresh_dir("up-home"),
            state: moor_testkit::short_tempdir("up").unwrap(),
            bin,
        }
    }

    fn path(&self) -> String {
        format!("{}:/usr/bin:/bin", self.bin.display())
    }

    fn moor(&self, args: &[&str], stdin: &str) -> Run {
        let child = Command::new(env!("CARGO_BIN_EXE_moor"))
            .args(args)
            .current_dir(&self.home)
            .env_clear()
            .env("PATH", self.path())
            .env("HOME", &self.home)
            .env("MOOR_UP_LINK_WAIT_SECS", "0")
            .env("MOOR_STATE_DIR", self.state.path())
            .env(
                "MOOR_EGRESS_FORWARD_BIN",
                self.bin.join("moor-egress-forward"),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut child = ChildGuard::new(child);
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        Run {
            code: out.status.code().unwrap(),
            stdout: String::from_utf8(out.stdout).unwrap(),
            stderr: String::from_utf8(out.stderr).unwrap(),
        }
    }

    fn units(&self) -> PathBuf {
        self.home.join(".config/systemd/user")
    }

    fn systemctl_log(&self) -> String {
        fs::read_to_string(self.bin.join("systemctl.log")).unwrap_or_default()
    }

    /// Every file `up` writes, with its text.
    fn written(&self) -> Vec<(PathBuf, String)> {
        [
            self.home.join(".config/moor/controller.yaml"),
            self.home.join(".config/moor/config.yaml"),
            self.units().join("moor.socket"),
            self.units().join("moor.service"),
            self.units().join("moor-gate.service"),
            self.units().join("moor-runner.service"),
            self.units().join("moor-agent.service"),
            self.state.path().join("service.env"),
            self.state.path().join("up-ports.json"),
            self.state
                .path()
                .join("agent-home/.local/state/moor-machine-agent/state.json"),
            self.state
                .path()
                .join("agent-home/.local/state/moor-machine-agent/id_ed25519.pub"),
        ]
        .into_iter()
        .map(|p| {
            let text = fs::read_to_string(&p).unwrap_or_default();
            (p, text)
        })
        .collect()
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let pid = fs::read_to_string(self.bin.join("controller.pid")).unwrap_or_default();
        if let Some(group) = pid
            .trim()
            .parse()
            .ok()
            .and_then(rustix::process::Pid::from_raw)
        {
            let _ = rustix::process::kill_process_group(group, rustix::process::Signal::TERM);
        }
        let _ = Command::new("pkill")
            .args(["-f", &self.state.path().display().to_string()])
            .status();
    }
}

/// A host after one `up` that read both secrets from stdin.
fn converged() -> (Host, Run) {
    let host = Host::new();
    let first = host.moor(&["up"], "sk-claude\ngh-token\n");
    (host, first)
}

/// The `systemctl` calls of a first `up`.
const FIRST_CALLS: &str = "--user daemon-reload\n--user enable moor.socket moor.service\n\
                           --user start moor.socket\n--user restart moor.service\n\
                           --user daemon-reload\n--user enable moor-gate.service\n\
                           --user restart moor-gate.service\n\
                           --user daemon-reload\n--user enable moor-runner.service\n\
                           --user restart moor-runner.service\n\
                           --user daemon-reload\n--user enable moor-agent.service\n\
                           --user restart moor-agent.service\n";

#[test]
fn up_converges_a_fresh_host() {
    let (host, first) = converged();
    let config = host.home.join(".config/moor/config.yaml");
    assert_eq!(first.code, 0, "{first:?}");
    for line in [
        "ok    user        enrolled as user 1\n",
        "ok    secret      claude: set\n",
        "ok    secret      github: set\n",
        "ok    docker      Docker 27.0.0 answers\n",
        "Docker Boxes are privileged containers",
        "ok    node        local: Docker Boxes by moor-agent.service",
        "ok    gate        moor-gate.service serves report_step to the Boxes at 127.0.0.1:8444\n",
    ] {
        assert!(first.stdout.contains(line), "{line}: {first:?}");
    }
    assert!(first
        .stderr
        .contains("paste what `claude setup-token` prints"));
    assert_eq!(host.systemctl_log(), FIRST_CALLS);
    let service = fs::read_to_string(host.units().join("moor.service")).unwrap();
    assert!(service.contains(&format!("Environment=\"MOOR_CONFIG={}\"", config.display())));
    assert!(service.contains("Environment=\"MOOR_EGRESS_LISTEN=127.0.0.1:"));
    let secrets = host.moor(&["user", "secret", "ls"], "");
    assert!(
        secrets.stdout.contains("claude") && secrets.stdout.contains("github"),
        "{secrets:?}"
    );
}

#[test]
fn up_adds_a_docker_node_whose_agent_links_to_the_runner_on_loopback() {
    let (host, _) = converged();
    let agent = fs::read_to_string(host.units().join("moor-agent.service")).unwrap();
    let home = host.state.path().join("agent-home");
    assert!(
        agent.contains(&format!("Environment=\"HOME={}\"", home.display())),
        "{agent}"
    );
    assert!(agent.contains("\" \"run\"\n"), "{agent}");
    let ports: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(host.state.path().join("up-ports.json")).unwrap())
            .unwrap();
    let agents = ports.get("agents").unwrap();
    let runner = fs::read_to_string(host.units().join("moor-runner.service")).unwrap();
    assert!(
        runner.contains(&format!("\"--agents\" \"127.0.0.1:{agents}\"")),
        "{runner}"
    );
    let state =
        fs::read_to_string(home.join(".local/state/moor-machine-agent/state.json")).unwrap();
    assert!(
        state.contains(&format!("http://127.0.0.1:{agents}")),
        "{state}"
    );
    let nodes = host.moor(&["node", "ls"], "");
    assert!(nodes.stdout.contains("local"), "{nodes:?}");
    for (path, text) in host.written() {
        assert!(
            !text.to_lowercase().contains("crabbox"),
            "{}",
            path.display()
        );
    }
}

#[test]
fn a_rerun_changes_nothing_and_prints_the_same_state() {
    let (host, first) = converged();
    let before = host.written();
    let again = host.moor(&["up"], "");
    assert_eq!(host.written(), before, "a rerun rewrote a file");
    assert_eq!(again.stdout, first.stdout, "a rerun printed another state");
    assert_eq!(again.code, first.code);
    assert!(
        again.stderr.is_empty(),
        "a rerun prompted: {:?}",
        again.stderr
    );
    assert_eq!(
        host.systemctl_log().strip_prefix(FIRST_CALLS).unwrap(),
        "--user enable moor.socket moor.service\n--user start moor.socket\n--user start moor.service\n\
         --user enable moor-gate.service\n--user start moor-gate.service\n\
         --user enable moor-runner.service\n--user start moor-runner.service\n\
         --user enable moor-agent.service\n--user start moor-agent.service\n"
    );
}

#[test]
fn down_removes_the_units_and_keeps_config_and_state() {
    let (host, _) = converged();
    let down = host.moor(&["down"], "");
    assert_eq!(down.code, 0, "{down:?}");
    assert!(!host.units().join("moor.service").exists());
    assert!(!host.units().join("moor.socket").exists());
    assert!(host.home.join(".config/moor/controller.yaml").exists());
    assert!(host.state.path().join("moor.db").exists());
    assert!(host
        .systemctl_log()
        .ends_with(
            "--user disable --now moor-gate.service moor-agent.service moor-runner.service moor.service moor.socket\n--user daemon-reload\n"
        ));
    assert!(!host.units().join("moor-agent.service").exists());
    assert!(!host.units().join("moor-runner.service").exists());
    assert!(!host.units().join("moor-gate.service").exists());
    assert_eq!(
        host.moor(&["down"], "").stdout,
        "ok    service     not installed\n"
    );
}

#[test]
fn up_keeps_a_user_config_and_adds_what_it_lacks() {
    let host = Host::new();
    let config = host.home.join(".config/moor/config.yaml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, "# mine\nbox:\n  sleep_after: 30m\n").unwrap();
    host.moor(&["up"], "a\nb\n");
    let text = fs::read_to_string(&config).unwrap();
    assert!(
        text.starts_with("# mine\nbox:\n  sleep_after: 30m\ncontroller:\n"),
        "{text}"
    );
}

#[test]
fn up_without_secret_input_names_where_each_comes_from() {
    let host = Host::new();
    let r = host.moor(&["up"], "");
    assert_eq!(r.code, 1, "{r:?}");
    assert!(
        r.stdout.ends_with(
            "fail  secret      claude: not set; fix: rerun moor up and paste what `claude setup-token` \
             prints, or set CLAUDE_CODE_OAUTH_TOKEN\n"
        ),
        "{r:?}"
    );
}

#[test]
fn down_without_a_service_changes_nothing() {
    let host = Host::new();
    let r = host.moor(&["down"], "");
    assert_eq!(
        (r.code, r.stdout.as_str()),
        (0, "ok    service     not installed\n")
    );
    assert_eq!(host.systemctl_log(), "");
}

#[test]
fn up_without_docker_says_what_to_fix_and_writes_nothing() {
    let host = Host::with_docker(false);
    let r = host.moor(&["up"], "");
    assert_eq!(r.code, 1, "{r:?}");
    assert!(
        r.stdout.starts_with(
            "fail  docker      Docker does not work for this user: permission denied; fix: "
        ),
        "{r:?}"
    );
    assert!(host.written().iter().all(|(_, t)| t.is_empty()));
    assert_eq!(host.systemctl_log(), "");
}

#[test]
fn up_over_the_crabbox_node_of_an_earlier_release_says_to_delete_it() {
    let host = Host::new();
    let config = host.home.join(".config/moor/config.yaml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, "nodes:\n  local:\n    runtime: external\n").unwrap();
    let r = host.moor(&["up"], "");
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.stdout.contains("delete nodes.local from"), "{r:?}");
    assert_eq!(host.systemctl_log(), "");
}

/// The front the controller config gets so the gate can report steps.
const FRONT: &str = "fronts:\n  authgear:\n    step_reports: true\n";

#[test]
fn up_runs_a_box_only_gate_that_shares_one_service_key_with_the_controller() {
    let (host, first) = converged();
    assert_eq!(first.code, 0, "{first:?}");
    let controller = fs::read_to_string(host.home.join(".config/moor/controller.yaml")).unwrap();
    assert!(controller.ends_with(FRONT), "{controller}");
    let key_file = host.state.path().join("service.env");
    let env = fs::read_to_string(&key_file).unwrap();
    let key = env.trim().strip_prefix("MOOR_SERVICE_KEY=").unwrap();
    assert!(key.len() >= 32, "{env:?}");
    assert_eq!(
        fs::metadata(&key_file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let uses = format!("EnvironmentFile={}\n", key_file.display());
    let gate = fs::read_to_string(host.units().join("moor-gate.service")).unwrap();
    let front = host.state.path().join("fronts/authgear/front");
    for want in [
        "\"--listen\" \"off\"".to_owned(),
        "\"--box-listen\" \"127.0.0.1:8444\"".to_owned(),
        format!("\"--front-socket\" \"{}\"", front.display()),
        "Wants=moor.service\n".to_owned(),
        uses.clone(),
    ] {
        assert!(gate.contains(&want), "{want} missing from {gate}");
    }
    let service = fs::read_to_string(host.units().join("moor.service")).unwrap();
    assert!(service.contains(&uses), "{service}");
    for (path, text) in host.written() {
        if path != key_file {
            assert!(!text.contains(key), "the key is in {}", path.display());
        }
    }
    assert!(front.exists(), "the controller bound no front socket");
}

#[test]
fn up_adds_the_front_to_a_controller_config_an_earlier_up_wrote() {
    let host = Host::new();
    let config = host.home.join(".config/moor/controller.yaml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    let state = host.state.path().display();
    let earlier = format!("socket: {state}/controller.sock\nstate_dir: {state}\n");
    fs::write(&config, &earlier).unwrap();
    let r = host.moor(&["up"], "a\nb\n");
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        format!("{earlier}{FRONT}")
    );
    assert!(host.units().join("moor-gate.service").exists());
}

#[test]
fn up_leaves_the_fronts_of_a_controller_config_alone_and_starts_no_gate() {
    let host = Host::new();
    let config = host.home.join(".config/moor/controller.yaml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    let state = host.state.path().display();
    let own =
        format!("socket: {state}/controller.sock\nstate_dir: {state}\nfronts:\n  authgear: {{}}\n");
    fs::write(&config, &own).unwrap();
    let r = host.moor(&["up"], "a\nb\n");
    assert_eq!(r.code, 0, "{r:?}");
    assert!(
        r.stdout
            .contains("ok    gate        not started: controller.yaml sets fronts of its own\n"),
        "{r:?}"
    );
    assert_eq!(fs::read_to_string(&config).unwrap(), own);
    assert!(!host.units().join("moor-gate.service").exists());
    assert!(!host.state.path().join("service.env").exists());
}
