//! Reads the host, and runs commands for `prepare` through a seam tests fake.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use moor_client::{kvm, rootless};

use super::{Daemon, Facts, Packager, AGAIN};

/// Docker's setup tool, from `docker-ce-rootless-extras` or Docker's rootless installer.
const SETUPTOOL: &str = "dockerd-rootless-setuptool.sh";

/// How a command ended.
#[derive(Debug, Clone)]
pub(crate) struct Ran {
    /// Whether it exited 0.
    pub(crate) ok: bool,
    /// Its stdout, then its stderr.
    pub(crate) out: String,
}

/// Runs commands for `prepare`; tests fake it.
pub(crate) trait Shell {
    /// Runs `program` with `args`, setting each variable of `env` (removing it when none),
    /// with no stdin, and keeps its output.
    fn run(&mut self, program: &str, args: &[&str], env: &[(&str, Option<&str>)]) -> Ran;
    /// Runs `script` with `sudo sh -ec` on the person's terminal; whether it succeeded.
    fn sudo(&mut self, script: &str) -> bool;
}

/// The host's own processes.
pub(crate) struct Real;

impl Shell for Real {
    fn run(&mut self, program: &str, args: &[&str], env: &[(&str, Option<&str>)]) -> Ran {
        let mut cmd = Command::new(program);
        cmd.args(args).stdin(Stdio::null());
        for (name, value) in env {
            match value {
                Some(v) => cmd.env(name, v),
                None => cmd.env_remove(name),
            };
        }
        match cmd.output() {
            Ok(out) => Ran {
                ok: out.status.success(),
                out: format!(
                    "{}{}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                ),
            },
            Err(e) => Ran {
                ok: false,
                out: format!("cannot run {program}: {e}"),
            },
        }
    }

    fn sudo(&mut self, script: &str) -> bool {
        Command::new("sudo")
            .args(["sh", "-ec", script])
            .status()
            .is_ok_and(|s| s.success())
    }
}

impl Facts {
    /// Reads this host for the person running `up`.
    ///
    /// # Errors
    /// `XDG_RUNTIME_DIR` or `HOME` is not set.
    pub(crate) fn read(shell: &mut dyn Shell) -> Result<Self, String> {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        let runtime = var("XDG_RUNTIME_DIR").ok_or_else(|| {
            format!(
                "XDG_RUNTIME_DIR is not set, so rootless Docker has nowhere to run; log in as this \
                 user directly, such as over ssh, not with su or sudo, {AGAIN}"
            )
        })?;
        let home = PathBuf::from(var("HOME").ok_or("HOME is not set")?);
        let config = var("XDG_CONFIG_HOME").map_or_else(|| home.join(".config"), PathBuf::from);
        let root = PathBuf::from(var(rootless::ROOT_VAR).unwrap_or_else(|| "/".to_owned()));
        let socket = Path::new(&runtime).join("docker.sock");
        let path = var("PATH");
        let kvm_device = kvm::device(var(kvm::DEVICE_VAR).as_deref()).to_owned();
        let docker_repo = docker_repo(&root);
        let setuptool = [root.join("usr/bin"), home.join("bin")]
            .into_iter()
            .map(|dir| dir.join(SETUPTOOL))
            .find(|p| p.is_file());
        let packaged = setuptool
            .as_ref()
            .map_or(docker_repo, |p| !p.starts_with(&home));
        let rootlesskit = if packaged {
            PathBuf::from("/usr/bin/rootlesskit")
        } else {
            home.join("bin/rootlesskit")
        };
        Ok(Self {
            rootless: rootless::Facts::read(
                path.as_deref(),
                var("USER").as_deref(),
                var(rootless::ROOT_VAR).as_deref(),
            ),
            kvm_mode: kvm::mode_of(&kvm_device),
            kvm_device,
            packager: packager(path.as_deref()),
            docker_repo,
            setuptool,
            profiled: profiled(&root.join("etc/apparmor.d"), &rootlesskit),
            rootlesskit,
            daemon: daemon(shell, &socket, &config),
            docker_config: var("DOCKER_CONFIG").map_or_else(|| home.join(".docker"), PathBuf::from),
            home,
            config,
            socket,
        })
    }
}

/// Whether a profile in `dir` names `program` as what it confines, as Ubuntu's own
/// `rootlesskit` profile names /usr/bin/rootlesskit.
pub(super) fn profiled(dir: &Path, program: &Path) -> bool {
    let program = program.to_string_lossy();
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| fs::read_to_string(entry.path()).ok())
        .any(|text| {
            text.lines()
                .filter(|line| !line.trim_start().starts_with('#'))
                .flat_map(str::split_whitespace)
                .any(|word| word.trim_matches('"') == program)
        })
}

/// The package manager on `path`.
fn packager(path: Option<&str>) -> Option<Packager> {
    let on_path = |bin: &str| {
        path.into_iter()
            .flat_map(std::env::split_paths)
            .any(|d| d.join(bin).is_file())
    };
    if on_path("apt-get") {
        Some(Packager::Apt)
    } else if on_path("dnf") {
        Some(Packager::Dnf)
    } else {
        None
    }
}

/// Whether apt or dnf under `root` lists Docker's package repository.
fn docker_repo(root: &Path) -> bool {
    let files = ["etc/apt/sources.list.d", "etc/yum.repos.d"]
        .into_iter()
        .filter_map(|dir| fs::read_dir(root.join(dir)).ok())
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .chain(std::iter::once(root.join("etc/apt/sources.list")));
    files
        .filter_map(|file| fs::read_to_string(file).ok())
        .any(|text| text.contains("download.docker.com"))
}

/// Where the person's rootless daemon at `socket`, with its unit under `config`, stands.
fn daemon(shell: &mut dyn Shell, socket: &Path, config: &Path) -> Daemon {
    if answers(shell, socket) {
        Daemon::Answers
    } else if config.join("systemd/user/docker.service").is_file() {
        let state = shell.run("systemctl", &["--user", "is-active", "docker"], &[]);
        Daemon::Installed {
            active: state.out.trim() == "active",
        }
    } else {
        Daemon::Absent
    }
}

/// Whether a rootless daemon answers at `socket`.
pub(super) fn answers(shell: &mut dyn Shell, socket: &Path) -> bool {
    let host = format!("unix://{}", socket.display());
    let ran = shell.run(
        "docker",
        &rootless::SECURITY_ARGS,
        &[("DOCKER_HOST", Some(&host)), ("DOCKER_CONTEXT", None)],
    );
    ran.ok && rootless::is_rootless(ran.out.trim())
}
