//! `up` and `down` as a person runs them: against an in-process gate with browser sign-in in
//! front of a controller that enrolls every machine as id 7, with stand-ins for crabbox,
//! `systemctl` and `loginctl` on the PATH. The `systemctl` stand-in marks the machine linked
//! when it starts the service, as the service does once it links.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::missing_docs_in_private_items,
    reason = "tests"
)]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use moor_core::response::Response;
use moor_gate::{Gate, Login, Verifier};
use moor_testkit::oidc::Issuer;
use moor_testkit::ChildGuard;
use serde_json::{json, Value};

const CLIENT_ID: &str = "moor-test-client";

const CRABBOX: &str = r#"#!/bin/sh
echo "$*" >> "$HOME/crabbox.log"
if [ "$*" = "doctor --json" ]; then
  provider=cloudhypervisor; [ -e "$HOME/no-provider" ] && provider=""
  echo "{\"ok\":true,\"provider\":\"$provider\",\"checks\":[]}"
  exit 0
fi
if [ -e "$HOME/doctor-fail" ]; then
  echo "ok      git      /usr/bin/git"
  echo "failed  kvm      /dev/kvm is missing; fix: turn on virtualization in the firmware"
  exit 1
fi
"#;

const SYSTEMCTL: &str = r#"#!/bin/sh
echo "$*" >> "$HOME/systemctl.log"
mark() {
  printf '{"server":"s","pid":%s,"start":"%s %s"}' "$1" "$(cat /proc/sys/kernel/random/boot_id)" \
    "$(awk '{print $22}' "/proc/$1/stat")" > "$XDG_STATE_HOME/moor-machine-agent/linked"
}
case "$2" in
  start|restart)
    [ -f "$HOME/service.pid" ] && kill "$(cat "$HOME/service.pid")" 2>/dev/null
    sleep 30 >/dev/null 2>&1 &
    pid=$!; echo "$pid" > "$HOME/service.pid"
    if [ -e "$HOME/slow-link" ]; then (sleep 2; mark "$pid") >/dev/null 2>&1 & else mark "$pid"; fi ;;
  disable) [ -f "$HOME/service.pid" ] && kill "$(cat "$HOME/service.pid")" 2>/dev/null ;;
esac
"#;

const LOGINCTL: &str = "#!/bin/sh\necho Linger=yes\n";

struct World {
    gate: String,
    enrolled: Arc<Mutex<Vec<Value>>>,
    home: tempfile::TempDir,
    _issuer: Issuer,
    _dir: tempfile::TempDir,
}

/// A gate with sign-in in front of a controller that enrolls every machine as id 7.
fn world() -> World {
    let issuer = Issuer::start(CLIENT_ID);
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("authgear.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let enrolled: Arc<Mutex<Vec<Value>>> = Arc::default();
    let kept = Arc::clone(&enrolled);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let kept = Arc::clone(&kept);
            std::thread::spawn(move || answer(stream, &kept));
        }
    });
    let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
    let gate = format!("http://{}", tcp.local_addr().unwrap());
    let login = Arc::new(Login::new(&gate).unwrap());
    let config = Arc::new(Gate {
        controller: moor_client::Client::new(socket),
        mcp: None,
        verifier: Verifier::new(issuer.base.clone(), CLIENT_ID.to_owned(), None),
    });
    std::thread::spawn(move || moor_gate::serve_with(&tcp, &config, Some(&login)));
    World {
        gate,
        enrolled,
        home: home(),
        _issuer: issuer,
        _dir: dir,
    }
}

fn answer(stream: std::os::unix::net::UnixStream, kept: &Mutex<Vec<Value>>) {
    let mut reply = stream.try_clone().unwrap();
    for line in BufReader::new(stream).lines().map_while(Result::ok) {
        let frame: Value = serde_json::from_str(&line).unwrap();
        if frame["frame"] == "hello" {
            writeln!(reply, "{line}").unwrap();
            continue;
        }
        let inner = &frame["request"]["request"];
        let value = if inner["cmd"] == "machine-enroll" {
            kept.lock().unwrap().push(inner.clone());
            json!({"machine_id": 7, "name": inner["name"], "user": 1,
                "owner": "alice@example.com", "capacity": inner["capacity"]})
        } else {
            json!({"identity": frame["request"]["identity"]})
        };
        let envelope = Response::ok("x", Some(value));
        let out = json!({"frame": "response", "id": frame["id"], "envelope": envelope});
        writeln!(reply, "{out}").unwrap();
    }
}

/// A home with the stand-ins in its `bin`.
fn home() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(home.path().join("state/moor-machine-agent")).unwrap();
    for (name, body) in [
        ("crabbox", CRABBOX),
        ("systemctl", SYSTEMCTL),
        ("loginctl", LOGINCTL),
    ] {
        let path = bin.join(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    home
}

/// The ureq client a browser stands in for: every status comes back, no redirect is followed.
fn browser() -> ureq::Agent {
    ureq::Agent::config_builder()
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .into()
}

/// The `name=value` of cookie `name` that `res` sets.
fn cookie(res: &ureq::http::Response<ureq::Body>, name: &str) -> String {
    res.headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap())
        .find(|v| v.starts_with(&format!("{name}=")))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

fn location(res: &ureq::http::Response<ureq::Body>) -> String {
    res.headers()["location"].to_str().unwrap().to_owned()
}

/// Alice opens `url` on her laptop, signs in and adds the machine.
fn confirm(gate: &str, url: &str) {
    let b = browser();
    let start = b
        .get(format!("{gate}/auth/login?return=/auth/me"))
        .call()
        .unwrap();
    let pending = cookie(&start, "moor_login");
    let back = location(&b.get(location(&start)).call().unwrap());
    let landed = b.get(&back).header("Cookie", &pending).call().unwrap();
    let session = cookie(&landed, "moor_session");
    let mut page = b.get(url).header("Cookie", &session).call().unwrap();
    assert_eq!(page.status(), 200);
    let body = page.body_mut().read_to_string().unwrap();
    let csrf = body
        .split_once("name=\"csrf\" value=\"")
        .unwrap()
        .1
        .split('"')
        .next()
        .unwrap()
        .to_owned();
    let code = url.split_once("code=").unwrap().1;
    let done = b
        .post(format!("{gate}/pair"))
        .header("Cookie", &session)
        .content_type("application/x-www-form-urlencoded")
        .send(format!("csrf={csrf}&code={code}"))
        .unwrap();
    assert_eq!(done.status(), 200);
}

/// What one run of the agent did.
struct Ran {
    ok: bool,
    said: String,
}

impl World {
    fn units(&self) -> PathBuf {
        self.home.path().join("units")
    }

    fn unit(&self) -> PathBuf {
        self.units().join("moor-machine-agent.service")
    }

    fn log(&self, name: &str) -> String {
        std::fs::read_to_string(self.home.path().join(name)).unwrap_or_default()
    }

    /// Runs the agent with `args`, confirming in the browser any pairing URL it prints.
    fn agent(&self, args: &[&str]) -> Ran {
        let path = format!(
            "{}:{}",
            self.home.path().join("bin").display(),
            std::env::var("PATH").unwrap()
        );
        let mut child = ChildGuard::new(
            Command::new(env!("CARGO_BIN_EXE_moor-machine-agent"))
                .args(args)
                .env("HOME", self.home.path())
                .env("XDG_STATE_HOME", self.home.path().join("state"))
                .env("XDG_CONFIG_HOME", self.home.path().join("config"))
                .env("MOOR_AGENT_SERVICE_DIR", self.units())
                .env("PATH", path)
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let mut said = String::new();
        for line in BufReader::new(child.stderr.take().unwrap()).lines() {
            let line = line.unwrap();
            if let Some(url) = line.split_whitespace().find(|w| w.contains("/pair?code=")) {
                confirm(&self.gate, url);
            }
            said.push_str(&line);
            said.push('\n');
        }
        let ok = child.wait().unwrap().success();
        Ran { ok, said }
    }

    fn up(&self) -> Ran {
        self.agent(&["up", "--server", &self.gate, "--name", "hv"])
    }
}

fn last_line(said: &str) -> &str {
    said.lines().last().unwrap_or_default()
}

#[test]
fn up_pairs_installs_the_service_and_says_the_machine_is_linked() {
    let w = world();
    let ran = w.up();
    assert!(ran.ok, "{}", ran.said);
    assert!(ran.said.contains("/pair?code="), "{}", ran.said);
    assert_eq!(
        last_line(&ran.said),
        "moor-machine-agent: linked: hv, 4 Box slots"
    );
    assert_paired_as_hv(&w);
    assert_service_serves_the_pool(&w);
}

/// Checks the controller enrolled `hv` once and the agent kept its answer.
fn assert_paired_as_hv(w: &World) {
    let enrolled = w.enrolled.lock().unwrap().clone();
    assert_eq!(enrolled.len(), 1);
    assert_eq!(enrolled[0]["name"], "hv");
    assert_eq!(enrolled[0]["capacity"], 4);
    assert_eq!(enrolled[0]["pool"], "cloudhypervisor");
    let state: Value = serde_json::from_str(
        &std::fs::read_to_string(w.home.path().join("state/moor-machine-agent/state.json"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(state["machine_id"], 7);
    assert_eq!(state["server"], w.gate);
}

/// Checks crabbox's doctor ran and the service runs and holds a pool of 4.
fn assert_service_serves_the_pool(w: &World) {
    assert!(w
        .log("crabbox.log")
        .contains("doctor --provider cloudhypervisor"));
    let unit = std::fs::read_to_string(w.unit()).unwrap();
    assert!(
        unit.contains("\"run\" \"--runtime\" \"cloudhypervisor\" \"--max-boxes\" \"4\""),
        "{unit}"
    );
    let calls = w.log("systemctl.log");
    assert!(
        calls.contains("--user enable moor-machine-agent.service"),
        "{calls}"
    );
    assert!(
        calls.contains("--user restart moor-machine-agent.service"),
        "{calls}"
    );
}

#[test]
fn a_second_up_keeps_the_pairing_and_the_running_service() {
    let w = world();
    assert!(w.up().ok);
    let unit = std::fs::read_to_string(w.unit()).unwrap();
    let calls = w.log("systemctl.log");

    let again = w.up();
    assert!(again.ok, "{}", again.said);
    assert!(!again.said.contains("/pair?code="), "{}", again.said);
    assert_eq!(
        last_line(&again.said),
        "moor-machine-agent: linked: hv, 4 Box slots"
    );
    assert_eq!(w.enrolled.lock().unwrap().len(), 1, "paired once");
    assert_eq!(std::fs::read_to_string(w.unit()).unwrap(), unit);
    let rerun = w.log("systemctl.log").replacen(&calls, "", 1);
    assert!(!rerun.contains("restart"), "{rerun}");
}

#[test]
fn up_stops_at_the_providers_own_fix_before_pairing() {
    let w = world();
    std::fs::write(w.home.path().join("doctor-fail"), "").unwrap();
    let ran = w.up();
    assert!(!ran.ok);
    assert!(
        ran.said
            .contains("failed  kvm      /dev/kvm is missing; fix: turn on virtualization"),
        "{}",
        ran.said
    );
    assert!(!ran.said.contains("ok      git"), "{}", ran.said);
    assert!(w.enrolled.lock().unwrap().is_empty());
    assert!(!w.unit().exists());
}

#[test]
fn up_against_a_server_that_takes_no_machines_says_so_before_anything_else() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let server = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut head = [0u8; 4096];
            let _ = std::io::Read::read(&mut stream, &mut head);
            let _ = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    let w = world();
    let ran = w.agent(&["up", "--server", &server, "--name", "hv"]);
    assert!(!ran.ok);
    assert!(ran.said.contains("takes no machines"), "{}", ran.said);
    assert!(
        w.log("crabbox.log").is_empty(),
        "no doctor before the server check"
    );
}

#[test]
#[allow(clippy::disallowed_methods, reason = "the test times the wait")]
fn a_mark_left_by_a_service_that_died_is_ignored_and_up_waits_for_a_fresh_link() {
    let w = world();
    assert!(w.up().ok);
    let pid = std::fs::read_to_string(w.home.path().join("service.pid")).unwrap();
    let pid = rustix::process::Pid::from_raw(pid.trim().parse().unwrap()).unwrap();
    rustix::process::kill_process(pid, rustix::process::Signal::TERM).unwrap();
    std::fs::write(w.home.path().join("slow-link"), "").unwrap();
    let began = std::time::Instant::now();
    let again = w.up();
    assert!(again.ok, "{}", again.said);
    assert!(
        began.elapsed() >= std::time::Duration::from_secs(2),
        "up waited for the fresh link"
    );
    assert_eq!(
        last_line(&again.said),
        "moor-machine-agent: linked: hv, 4 Box slots"
    );
}

#[test]
fn up_without_a_runtime_takes_the_provider_crabbox_is_configured_with() {
    let w = world();
    assert!(w.up().ok);
    assert!(w.log("crabbox.log").contains("doctor --json\n"));
    let unit = std::fs::read_to_string(w.unit()).unwrap();
    assert!(unit.contains("\"--runtime\" \"cloudhypervisor\""), "{unit}");
}

#[test]
fn up_without_a_runtime_when_crabbox_has_none_says_to_pass_one() {
    let w = world();
    std::fs::write(w.home.path().join("no-provider"), "").unwrap();
    let ran = w.up();
    assert!(!ran.ok);
    assert!(ran.said.contains("--runtime"), "{}", ran.said);
    assert!(ran.said.contains("crabbox providers"), "{}", ran.said);
    assert!(w.enrolled.lock().unwrap().is_empty());
}

#[test]
fn down_stops_and_removes_the_service_but_keeps_the_pairing() {
    let w = world();
    assert!(w.up().ok);
    let ran = w.agent(&["down"]);
    assert!(ran.ok, "{}", ran.said);
    assert!(!w.unit().exists());
    assert!(w
        .log("systemctl.log")
        .contains("--user disable --now moor-machine-agent.service"));
    assert!(Path::new(&w.home.path().join("state/moor-machine-agent/state.json")).exists());
    assert!(
        !w.home
            .path()
            .join("state/moor-machine-agent/linked")
            .exists(),
        "a stopped service is not linked"
    );
}

#[test]
fn the_old_enroll_command_and_slots_flag_are_gone() {
    let w = world();
    let ran = w.agent(&["enroll", "--server", &w.gate, "--name", "hv"]);
    assert!(!ran.ok);
    assert!(ran.said.contains("unrecognized subcommand"), "{}", ran.said);
    let ran = w.agent(&["up", "--server", &w.gate, "--slots", "2"]);
    assert!(!ran.ok);
    assert!(ran.said.contains("unexpected argument"), "{}", ran.said);
}

#[test]
fn up_with_allow_exec_pairs_a_machine_that_runs_scripts_and_hosts_no_vms() {
    let w = world();
    let ran = w.agent(&[
        "up",
        "--server",
        &w.gate,
        "--name",
        "mac",
        "--labels",
        "macos,ios-sim",
        "--allow-exec",
        "--max-boxes",
        "2",
    ]);
    assert!(ran.ok, "{}", ran.said);
    assert_eq!(
        last_line(&ran.said),
        "moor-machine-agent: linked: mac, 2 Box slots"
    );
    let enrolled = w.enrolled.lock().unwrap().clone();
    assert_eq!(enrolled[0]["labels"], json!(["macos", "ios-sim"]));
    assert_eq!(enrolled[0]["capacity"], 2);
    assert!(enrolled[0].get("pool").is_none(), "{}", enrolled[0]);
    assert!(w.log("crabbox.log").is_empty(), "no provider to check");
    let unit = std::fs::read_to_string(w.unit()).unwrap();
    assert!(unit.contains("\"run\" \"--allow-exec\""), "{unit}");
    assert!(!unit.contains("--runtime"), "{unit}");
}
