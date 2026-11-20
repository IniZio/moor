//! `moor up` and `moor down` against a fake `systemctl` that runs the rendered service's
//! `ExecStart` as a real controller.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::process::Command;

use moor_testkit::write_executable;

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
pub(super) struct Host {
    pub(super) home: Dir,
    pub(super) state: tempfile::TempDir,
    pub(super) bin: Dir,
}

impl Host {
    pub(super) fn new() -> Self {
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

    pub(super) fn path(&self) -> String {
        format!("{}:/usr/bin:/bin", self.bin.display())
    }

    pub(super) fn moor(&self, args: &[&str], stdin: &str) -> Run {
        self.moor_with(args, stdin, &[])
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
const FIRST_CALLS: &str = "--user daemon-reload\n--user enable moor.socket moor.service moor-front@authgear.socket\n\
                           --user start moor.socket moor-front@authgear.socket\n--user restart moor.service\n\
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
        "--user enable moor.socket moor.service moor-front@authgear.socket\n--user start moor.socket moor-front@authgear.socket\n--user start moor.service\n\
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
    assert!(host.home.join(".config/moor/controller.yaml").exists());
    assert!(host.state.path().join("moor.db").exists());
    assert!(host
        .systemctl_log()
        .ends_with(
            "--user disable --now moor-gate.service moor-agent.service moor-runner.service moor.service moor-front@authgear.socket moor.socket\n--user daemon-reload\n"
        ));
    let left = fs::read_dir(host.units()).unwrap().count();
    assert_eq!(
        left,
        0,
        "{:?}",
        fs::read_dir(host.units()).unwrap().collect::<Vec<_>>()
    );
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
fn up_keeps_one_private_service_key_the_controller_binds_the_gate_front_with() {
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
    for (path, text) in host.written() {
        assert!(
            path == key_file || !text.contains(key),
            "the key is in {}",
            path.display()
        );
    }
    assert!(host.state.path().join("fronts/authgear/front").exists());
}

#[test]
fn the_gate_runs_without_a_public_listener_and_both_units_read_the_key_file() {
    let (host, _) = converged();
    let uses = format!(
        "EnvironmentFile={}\n",
        host.state.path().join("service.env").display()
    );
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

#[test]
fn a_rerun_after_an_up_that_failed_part_way_still_starts_the_gate() {
    let host = Host::new();
    let config = host.home.join(".config/moor/config.yaml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, "nodes:\n  local:\n    runtime: external\n").unwrap();
    let failed = host.moor(&["up"], "a\nb\n");
    assert_eq!(failed.code, 1, "{failed:?}");
    fs::remove_file(&config).unwrap();
    let r = host.moor(&["up"], "a\nb\n");
    assert_eq!(r.code, 0, "{r:?}");
    assert!(host.units().join("moor-gate.service").exists(), "{r:?}");
    let service = fs::read_to_string(host.units().join("moor.service")).unwrap();
    assert!(service.contains("EnvironmentFile="), "{service}");
}

/// A `docker` of rootless Docker whose rootlesskit `up` cannot read, because the daemon is not
/// at a Unix socket on this host.
const ROOTLESS_DOCKER: &str = r#"#!/bin/sh
case "$*" in
  "info --format {{.ServerVersion}}") echo 29.10.0 ;;
  "info --format {{json .SecurityOptions}}") echo '["name=seccomp,profile=builtin","name=rootless"]' ;;
  "context inspect --format {{.Endpoints.docker.Host}}") echo tcp://10.0.0.2:2375 ;;
  *) echo "fake docker: $*" >&2; exit 2 ;;
esac
"#;

#[test]
fn up_on_rootless_docker_that_is_not_ready_names_each_fix_and_writes_nothing() {
    let host = Host::new();
    write_executable(&host.bin.join("docker"), ROOTLESS_DOCKER).unwrap();
    let run = host.moor(&["up"], "");
    assert_eq!(run.code, 1, "{}", run.stdout);
    assert!(
        run.stdout
            .contains("rootless Docker is not ready for Boxes:"),
        "{}",
        run.stdout
    );
    assert!(run.stdout.contains("- loopback: "), "{}", run.stdout);
    assert!(run.stdout.contains("; fix: "), "{}", run.stdout);
    assert!(
        !run.stdout.contains("a user who can run Docker is root"),
        "{}",
        run.stdout
    );
    assert!(host.written().iter().all(|(_, text)| text.is_empty()));
    assert_eq!(
        host.systemctl_log(),
        "--user is-enabled docker\n",
        "only a read"
    );
}

#[test]
fn a_rootful_up_after_a_rootless_one_removes_the_stale_relay_once() {
    let host = Host::new();
    fs::create_dir_all(host.units()).unwrap();
    let relay = host.units().join("moor-relay.service");
    fs::write(&relay, "[Unit]\nDescription=moor egress relay\n").unwrap();
    let first = host.moor(&["up"], "sk-claude\ngh-token\n");
    assert_eq!(first.code, 0, "{first:?}");
    assert!(!relay.exists(), "the rootless relay outlived a rootful up");
    assert!(
        host.systemctl_log()
            .contains("--user disable --now moor-relay.service\n--user daemon-reload\n"),
        "{}",
        host.systemctl_log()
    );
    let calls = host.systemctl_log();
    let again = host.moor(&["up"], "");
    assert_eq!(again.code, 0, "{again:?}");
    assert!(
        !host
            .systemctl_log()
            .strip_prefix(&calls)
            .expect("the systemctl log only grows")
            .contains("moor-relay"),
        "a rerun touched the relay again: {}",
        host.systemctl_log()
    );
}

#[test]
fn up_enables_the_gate_fronts_socket_before_it_starts_the_controller() {
    let (host, first) = converged();
    assert_eq!(first.code, 0, "{first:?}");
    let template = fs::read_to_string(host.units().join("moor-front@.socket")).unwrap();
    let front = host.state.path().join("fronts/%i/front");
    assert!(
        template.contains(&format!("ListenStream={}\n", front.display())),
        "{template}"
    );
    let log = host.systemctl_log();
    assert!(
        log.starts_with(
            "--user daemon-reload\n--user enable moor.socket moor.service moor-front@authgear.socket\n\
             --user start moor.socket moor-front@authgear.socket\n--user restart moor.service\n"
        ),
        "{log}"
    );
}

#[test]
fn up_writes_no_front_socket_for_fronts_of_the_controller_configs_own() {
    let host = Host::new();
    let config = host.home.join(".config/moor/controller.yaml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    let state = host.state.path().display();
    let own =
        format!("socket: {state}/controller.sock\nstate_dir: {state}\nfronts:\n  authgear: {{}}\n");
    fs::write(&config, &own).unwrap();
    let r = host.moor(&["up"], "a\nb\n");
    assert_eq!(r.code, 0, "{r:?}");
    assert!(!host.units().join("moor-front@.socket").exists());
    assert!(
        !host.systemctl_log().contains("moor-front@"),
        "{}",
        host.systemctl_log()
    );
}

#[test]
fn down_stops_the_front_socket_and_removes_its_template() {
    let (host, _) = converged();
    let down = host.moor(&["down"], "");
    assert_eq!(down.code, 0, "{down:?}");
    assert!(
        host.systemctl_log().contains(
            "--user disable --now moor-gate.service moor-agent.service moor-runner.service \
             moor.service moor-front@authgear.socket moor.socket\n"
        ),
        "{}",
        host.systemctl_log()
    );
    assert!(!host.units().join("moor-front@.socket").exists());
}
