//! Opt-in check that a repo backend started by the setup script on a real Mac is healthy and
//! reachable at `localhost` there, and that the teardown script removes it again.
//!
//! Runs only when `MOOR_REAL_MAC_HOST` names the Mac (`user@host`); otherwise it returns at
//! once. Everything it starts lives in compose projects it names itself, and only those are
//! ever stopped.
#![expect(
    clippy::print_stderr,
    reason = "an opt-in run reports its evidence on stderr"
)]

use std::collections::hash_map::RandomState;
use std::collections::BTreeSet;
use std::hash::{BuildHasher, Hasher};
use std::process::Command;
use std::sync::Arc;

use moor_core::boxscript::setup::setup_script;
use moor_core::boxscript::teardown::teardown_script;
use moor_core::domain::run::SyncMode;
use moor_core::seam::driver::{Acquired, Driver, ExecOutcome, ExecSpec};
use moor_core::seam::proc::{Deadline, Output};
use std::time::Duration;

use super::real_mac::{acquire, crabbox_env, fixture_repo, Leases, HOST_VAR};
use super::Crabbox;
use crate::adapters::proc::{SystemProc, ToolPaths};

/// Puts docker on `PATH` for the plain-ssh observer commands only; a non-interactive ssh shell
/// there does not have it.
const DOCKER_PATH: &str = r#"export PATH="$HOME/.orbstack/bin:/usr/local/bin:$PATH""#;

/// One compose project the check owns.
struct Stack {
    /// Compose project name; every container the check makes carries it as a label.
    project: String,
    /// Compose file name in the Box workdir.
    file: String,
    /// Port published on the Mac.
    port: u16,
    /// Fixed container name, when set.
    container: Option<String>,
}

impl Stack {
    /// The compose file text: one nginx service with a one-second healthcheck.
    fn compose(&self) -> String {
        let name = self
            .container
            .as_ref()
            .map_or_else(String::new, |c| format!("    container_name: {c}\n"));
        format!(
            "services:\n  web:\n    image: nginx:alpine\n    network_mode: bridge\n{name}    ports:\n      - \"{}:80\"\n    healthcheck:\n      test: [\"CMD\", \"wget\", \"-q\", \"-O\", \"/dev/null\", \"http://127.0.0.1/\"]\n      interval: 1s\n      timeout: 3s\n      retries: 30\n",
            self.port
        )
    }

    /// Setup lines: write the compose file, start it and wait for health, then reach it from the Mac.
    /// These lines rely on moor's `PATH`, not their own.
    fn setup(&self) -> Vec<String> {
        vec![
            format!("printf '%s' '{}' > {}", self.compose(), self.file),
            format!(
                "docker compose -p {} -f {} up -d --wait",
                self.project, self.file
            ),
            format!("curl -fsS http://localhost:{}/ >/dev/null", self.port),
        ]
    }

    /// Teardown line: remove only this project.
    fn teardown(&self) -> Vec<String> {
        vec![format!(
            "docker compose -p {} -f {} down --remove-orphans",
            self.project, self.file
        )]
    }
}

/// Runs `cmd` on the Mac over plain ssh with docker on `PATH` and returns its stdout.
fn mac(target: &str, cmd: &str) -> String {
    let out = Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            target,
            &format!("{DOCKER_PATH}; {cmd}"),
        ])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// Every container on the Mac, as `id name` lines.
fn containers(target: &str) -> BTreeSet<String> {
    mac(target, "docker ps -a --format '{{.ID}} {{.Names}}'")
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Ids of the containers labelled with compose project `project`.
fn project_containers(target: &str, project: &str) -> Vec<String> {
    let cmd = format!("docker ps -a -q --filter label=com.docker.compose.project={project}");
    mac(target, &cmd).lines().map(str::to_owned).collect()
}

/// Removes the check's projects when the check ends, also after a failed assertion.
struct Projects<'a> {
    /// Mac as `user@host`.
    target: &'a str,
    /// Project names to remove.
    names: Vec<String>,
}

impl Drop for Projects<'_> {
    fn drop(&mut self) {
        for p in &self.names {
            mac(
                self.target,
                &format!("docker compose -p {p} down --remove-orphans"),
            );
        }
    }
}

/// Runs `script` in the Box workdir through the driver and returns its exit code and output.
fn exec(driver: &Crabbox, acq: &Acquired, repo: &std::path::Path, script: String) -> (i32, String) {
    let log = tempfile::NamedTempFile::new().unwrap();
    let spec = ExecSpec {
        argv: vec!["sh".to_owned(), "-c".to_owned(), script],
        env: Vec::new(),
        cwd: Some(acq.workdir.clone()),
        sync: SyncMode::None,
        output: Some(Output::Log(log.path().to_path_buf())),
        host_cwd: Some(repo.to_path_buf()),
        class: None,
    };
    let out = driver
        .exec(&acq.handle, &spec, Deadline(Duration::from_mins(10)))
        .unwrap();
    let ExecOutcome::Exited { code } = out else {
        return (-1, "interrupted".to_owned());
    };
    (
        code,
        std::fs::read_to_string(log.path()).unwrap_or_default(),
    )
}

/// A number that differs between runs.
fn unique() -> u64 {
    RandomState::new().build_hasher().finish()
}

/// A port in 18000..19000 that nothing on the Mac listens on.
fn free_port(target: &str) -> u16 {
    let mut n = unique();
    loop {
        let port = u16::try_from(18_000 + n % 1000).unwrap();
        let busy = mac(target, &format!("nc -z localhost {port} && echo busy"));
        if busy.is_empty() {
            return port;
        }
        n += 1;
    }
}

/// A Box on the Mac, and the way to run scripts in it.
struct Run<'a> {
    /// Driver that runs the scripts.
    driver: &'a Crabbox,
    /// The Box.
    acq: Acquired,
    /// Host directory the driver runs from.
    repo: &'a std::path::Path,
}

impl Run<'_> {
    /// Runs `script` in the Box workdir.
    fn script(&self, script: String) -> (i32, String) {
        exec(self.driver, &self.acq, self.repo, script)
    }
}

/// Starts `main` through its setup script and checks its container is healthy.
fn started_healthy(target: &str, main: &Stack, run: &Run<'_>) {
    let (code, log) = run.script(setup_script(&main.setup()));
    eprintln!("setup exit {code}\n{log}");
    assert_eq!(code, 0, "setup must succeed");
    let ids = project_containers(target, &main.project);
    assert_eq!(ids.len(), 1, "one container for the project: {ids:?}");
    let id = ids.first().unwrap();
    let cmd = format!("docker inspect -f '{{{{.State.Health.Status}}}}' {id}");
    let health = mac(target, &cmd);
    eprintln!("health after setup: {health}");
    assert_eq!(health, "healthy");
}

/// Starts `clash` on a port that is taken, checks the failure names the port, then tears it down.
fn clash_fails_and_cleans_up(target: &str, clash: &Stack, run: &Run<'_>) {
    let (code, log) = run.script(setup_script(&clash.setup()));
    eprintln!("clash setup exit {code}\n{log}");
    assert_ne!(code, 0, "setup on a taken port must fail");
    let port = clash.port;
    assert!(
        log.lines()
            .any(|l| l.contains(&format!(":{port} failed")) && l.contains("already allocated")),
        "failure names port {port}"
    );
    let (code, _) = run.script(teardown_script(&clash.teardown()));
    assert_eq!(code, 0);
    let left = project_containers(target, &clash.project);
    eprintln!("clash containers left after its teardown: {left:?}");
    assert!(left.is_empty());
}

#[test]
fn backend_started_by_setup_is_healthy_on_the_mac_and_teardown_removes_it() {
    let Ok(target) = std::env::var(HOST_VAR) else {
        eprintln!("skip: {HOST_VAR} is not set");
        return;
    };
    let env = crabbox_env(&target);
    let driver = Crabbox::fake_unchecked(Arc::new(SystemProc::new(&env, &ToolPaths::default())));
    let repo = tempfile::tempdir().unwrap();
    fixture_repo(repo.path());
    let mut leases = Leases {
        driver: &driver,
        target: &target,
        handles: Vec::new(),
        dirs: Vec::new(),
    };
    let acq = acquire(
        &driver,
        &mut leases,
        987_654_331,
        "realmac-backend",
        repo.path(),
    );
    let run = Run {
        driver: &driver,
        acq,
        repo: repo.path(),
    };

    let id = unique();
    let port = free_port(&target);
    let main = Stack {
        project: format!("moor-mx-{id}"),
        file: "compose.yml".to_owned(),
        port,
        container: None,
    };
    let clash = Stack {
        project: format!("moor-mx-{id}-clash"),
        file: "compose-clash.yml".to_owned(),
        port,
        container: Some(format!("moor-mx-{id}-clash-web")),
    };
    let _projects = Projects {
        target: &target,
        names: vec![main.project.clone(), clash.project.clone()],
    };
    let before = containers(&target);
    eprintln!("docker ps -a before:\n{}", join(&before));

    started_healthy(&target, &main, &run);
    clash_fails_and_cleans_up(&target, &clash, &run);

    let (code, log) = run.script(teardown_script(&main.teardown()));
    eprintln!("teardown exit {code}\n{log}");
    assert_eq!(code, 0, "teardown must succeed");
    let left = project_containers(&target, &main.project);
    eprintln!("project containers left after teardown: {left:?}");
    assert!(left.is_empty(), "teardown left {left:?}");
    let after = containers(&target);
    eprintln!("docker ps -a after:\n{}", join(&after));
    assert_eq!(before, after, "docker ps -a must equal the before set");
}

/// The lines of `set`, one per row.
fn join(set: &BTreeSet<String>) -> String {
    set.iter().cloned().collect::<Vec<_>>().join("\n")
}
