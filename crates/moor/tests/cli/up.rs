//! `moor up` and `moor down` against a fake crabbox and a fake `systemctl` that runs the
//! rendered service's `ExecStart` as a real controller.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use moor_testkit::{write_executable, ChildGuard};

use super::{fresh_dir, Dir, Run};

/// A crabbox whose `doctor` selects `external` and passes.
const READY: &str = r#"case "$*" in
  "--version") echo "crabbox 0.79.1-moor.9" ;;
  "doctor --json") echo '{"ok":true,"provider":"external","checks":[]}' ;;
  "doctor --provider external --json") echo '{"ok":true,"provider":"external","checks":[{"status":"ok","check":"kvm"}]}' ;;
  "providers --json --configured") echo '[]' ;;
  *) echo "fake crabbox: $*" >&2; exit 2 ;;
esac
"#;

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
    printf 'exec '
    sed -n 's/^ExecStart=//p' "$units/moor.service"; } > "$dir/run.sh"
  setsid sh "$dir/run.sh" >> "$dir/controller.log" 2>&1 < /dev/null &
  echo $! > "$pid"
}
case "$verb" in
  start) case "$*" in *moor.service*) alive || launch ;; esac ;;
  restart) stop; launch ;;
  disable) case "$*" in *--now*) stop ;; esac ;;
esac
exit 0
"#;

/// A temp HOME with fake tools on `PATH`; stops the controller it ran on drop.
struct Host {
    home: Dir,
    state: tempfile::TempDir,
    bin: Dir,
}

impl Host {
    fn new(crabbox: Option<&str>) -> Self {
        let bin = fresh_dir("up-bin");
        if let Some(body) = crabbox {
            write_executable(&bin.join("crabbox"), format!("#!/bin/sh\n{body}")).unwrap();
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
    let host = Host::new(Some(READY));
    let first = host.moor(&["up", "--max-boxes", "2"], "sk-claude\ngh-token\n");
    (host, first)
}

/// The `systemctl` calls of a first `up`.
const FIRST_CALLS: &str = "--user daemon-reload\n--user enable moor.socket moor.service\n\
                           --user start moor.socket\n--user restart moor.service\n";

#[test]
fn up_converges_a_fresh_host() {
    let (host, first) = converged();
    let config = host.home.join(".config/moor/config.yaml");
    let log = fs::read_to_string(host.bin.join("controller.log"));
    assert!(
        first
            .stdout
            .contains("ok    provider    external, ready per crabbox doctor\n"),
        "{first:?}\ncontroller log: {log:?}"
    );
    let node = format!("{}: node local (external, max 2 Boxes)", config.display());
    assert!(first.stdout.contains(&node), "{first:?}");
    assert!(
        first
            .stdout
            .contains("ok    user        enrolled as user 1\n"),
        "{first:?}"
    );
    assert!(
        first.stdout.contains("ok    secret      claude: set\n"),
        "{first:?}"
    );
    assert!(
        first.stdout.contains("ok    secret      github: set\n"),
        "{first:?}"
    );
    assert!(first
        .stderr
        .contains("paste what `claude setup-token` prints"));
    assert_eq!(host.systemctl_log(), FIRST_CALLS);
    let service = fs::read_to_string(host.units().join("moor.service")).unwrap();
    assert!(service.contains(&format!("Environment=\"MOOR_CONFIG={}\"", config.display())));
    let secrets = host.moor(&["user", "secret", "ls"], "");
    assert!(
        secrets.stdout.contains("claude") && secrets.stdout.contains("github"),
        "{secrets:?}"
    );
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
        "--user enable moor.socket moor.service\n--user start moor.socket\n--user start moor.service\n"
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
        .ends_with("--user disable --now moor.service moor.socket\n--user daemon-reload\n"));
    assert_eq!(
        host.moor(&["down"], "").stdout,
        "ok    service     not installed\n"
    );
}

#[test]
fn up_keeps_a_user_config_and_adds_what_it_lacks() {
    let host = Host::new(Some(READY));
    let config = host.home.join(".config/moor/config.yaml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, "# mine\nbox:\n  sleep_after: 30m\n").unwrap();
    let r = host.moor(&["up"], "a\nb\n");
    let text = fs::read_to_string(&config).unwrap();
    assert!(
        text.starts_with("# mine\nbox:\n  sleep_after: 30m\ncontroller:\n"),
        "{text}"
    );
    assert!(
        text.contains("nodes:\n  local:\n    runtime: external\n"),
        "{text}\n{r:?}"
    );
}

#[test]
fn up_without_secret_input_names_where_each_comes_from() {
    let host = Host::new(Some(READY));
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

/// `up` that stops before writing anything, with what it printed.
fn refused(crabbox: Option<&str>) -> (Run, Host) {
    let host = Host::new(crabbox);
    let r = host.moor(&["up"], "");
    assert_eq!(r.code, 1, "{r:?}");
    assert!(
        host.written().iter().all(|(_, t)| t.is_empty()),
        "wrote before the provider was ready"
    );
    assert_eq!(host.systemctl_log(), "");
    (r, host)
}

#[test]
fn up_without_crabbox_says_to_install_it() {
    let (r, _host) = refused(None);
    assert_eq!(
        r.stdout,
        "fail  provider    crabbox is not on PATH; fix: install a -moor build of IniZio/crabbox on PATH (docs/providers.md)\n"
    );
}

#[test]
fn up_with_no_selected_provider_prints_the_commands_that_install_one() {
    let (r, _host) = refused(Some(r#"echo '{"ok":true,"provider":"","checks":[]}'"#));
    assert!(
        r.stdout
            .starts_with("fail  provider    crabbox selects no provider; fix: "),
        "{r:?}"
    );
    assert!(
        r.stdout.ends_with(
            "  curl -fsSL https://github.com/IniZio/crabbox-cloudhypervisor/releases/download/v0.2.1/install.sh | sh\n  crabbox-cloudhypervisor install\n"
        ),
        "{r:?}"
    );
}

#[test]
fn up_with_a_provider_that_is_not_ready_names_crabbox_doctor() {
    let (r, _host) = refused(Some(
        r#"case "$*" in
  "doctor --json") echo '{"ok":true,"provider":"external","checks":[]}' ;;
  *) echo '{"ok":false,"provider":"external","checks":[{"status":"failed","check":"kvm","details":{"hint":"join_kvm_group"}}]}'; exit 1 ;;
esac"#,
    ));
    assert_eq!(
        r.stdout,
        "fail  provider    external: kvm (join_kvm_group); fix: run `crabbox doctor --provider external` and fix what it reports\n"
    );
}

#[test]
fn down_without_a_service_changes_nothing() {
    let host = Host::new(Some(READY));
    let r = host.moor(&["down"], "");
    assert_eq!(
        (r.code, r.stdout.as_str()),
        (0, "ok    service     not installed\n")
    );
    assert_eq!(host.systemctl_log(), "");
}
