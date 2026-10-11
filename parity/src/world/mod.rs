pub mod config;
pub mod hooks;
mod procs;
pub mod repo;

use crate::fake::{self, Call, Rule};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use procs::*;

pub const TOOLS: &[&str] = &["crabbox", "herdr", "claude", "ssh", "gh"];

const GITCONFIG: &str = "[user]\n\tname = parity\n\temail = parity@example.invalid\n\
[init]\n\tdefaultBranch = main\n[protocol \"file\"]\n\tallow = always\n";

pub struct World {
    dir: tempfile::TempDir,
    pids: Mutex<Vec<(u32, u64)>>,
    cred_snaps: BTreeMap<PathBuf, BTreeSet<String>>,
    suffix: String,
    torn_down: bool,
    extra_env: Mutex<Vec<(String, String)>>,
    transcript: Mutex<Vec<String>>,
}

fn random_hex6() -> String {
    let mut buf = [0u8; 3];
    fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .expect("read /dev/urandom");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

impl World {
    pub fn new(fake_bin: impl AsRef<Path>) -> World {
        let base = std::env::var_os("PARITY_WORLD_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../target/parity-worlds"
                ))
            });
        fs::create_dir_all(&base).expect("create world base dir");
        let base = fs::canonicalize(&base).expect("canonicalize world base dir");
        let dir = tempfile::Builder::new()
            .prefix("parity-")
            .tempdir_in(&base)
            .expect("create world tempdir");
        assert!(
            !dir.path().starts_with("/tmp/"),
            "world root must not live under /tmp: {}",
            dir.path().display()
        );
        let fake_bin = fs::canonicalize(fake_bin.as_ref()).expect("canonicalize fake_bin");
        let root = dir.path();
        for sub in [
            "bin", "home", "box/work", "box/home", "box/tmp", "state", "fakes", "tmp", "trees",
            "repos",
        ] {
            fs::create_dir_all(root.join(sub)).expect("create world dir");
        }
        let dot_config = root.join("home/.config");
        fs::create_dir_all(dot_config.join("moor")).expect("create config dir");
        fs::set_permissions(&dot_config, fs::Permissions::from_mode(0o755))
            .expect("chmod home/.config");
        for tool in TOOLS {
            std::os::unix::fs::symlink(&fake_bin, root.join("bin").join(tool))
                .expect("symlink tool");
        }
        fs::write(root.join("home/.gitconfig"), GITCONFIG).expect("write .gitconfig");
        World {
            dir,
            pids: Mutex::new(Vec::new()),
            cred_snaps: BTreeMap::from([(PathBuf::from("/tmp"), creds_in(Path::new("/tmp")))]),
            suffix: random_hex6(),
            torn_down: false,
            extra_env: Mutex::new(Vec::new()),
            transcript: Mutex::new(Vec::new()),
        }
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }
    pub fn bin_dir(&self) -> PathBuf {
        self.root().join("bin")
    }
    pub fn home(&self) -> PathBuf {
        self.root().join("home")
    }
    pub fn box_root(&self) -> PathBuf {
        self.root().join("box")
    }
    pub fn state_dir(&self) -> PathBuf {
        self.root().join("state")
    }
    pub fn fakes_dir(&self) -> PathBuf {
        self.root().join("fakes")
    }
    pub fn tmp_dir(&self) -> PathBuf {
        self.root().join("tmp")
    }
    pub fn config_path(&self) -> PathBuf {
        self.home().join(".config/moor/config.yaml")
    }
    pub fn tool_path(&self, tool: &str) -> PathBuf {
        self.bin_dir().join(tool)
    }

    pub fn task_name(&self, base: &str) -> String {
        format!("{base}-{}", self.suffix)
    }

    pub fn register_secret(&self, secret: &str) {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        assert!(
            !secret.is_empty() && !secret.contains('\n'),
            "register_secret: secret must be non-empty and single-line"
        );
        let dir = self.fakes_dir();
        let _lock = fake::lock(&dir).expect("lock fakes dir");
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(fake::secrets_path(&dir))
            .expect("open secrets file");
        f.write_all(format!("{secret}\n").as_bytes())
            .expect("write secret");
    }

    pub fn secrets(&self) -> Vec<String> {
        fake::read_secrets(&fake::secrets_path(&self.fakes_dir()))
    }

    pub fn base_env(&self) -> Vec<(String, String)> {
        let s = |p: PathBuf| p.to_string_lossy().into_owned();
        let home = self.home();
        vec![
            (
                "PATH".into(),
                format!("{}:/usr/bin:/bin", self.bin_dir().display()),
            ),
            ("HOME".into(), s(home.clone())),
            ("XDG_CONFIG_HOME".into(), s(home.join(".config"))),
            ("XDG_STATE_HOME".into(), s(home.join(".local/state"))),
            ("XDG_CACHE_HOME".into(), s(home.join(".cache"))),
            ("MOOR_STATE_DIR".into(), s(self.state_dir())),
            ("MOOR_CONFIG".into(), s(self.config_path())),
            ("GIT_CONFIG_GLOBAL".into(), s(home.join(".gitconfig"))),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            (
                "GIT_CEILING_DIRECTORIES".into(),
                s(self
                    .root()
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_default()),
            ),
            ("TMPDIR".into(), s(self.tmp_dir())),
            ("PARITY_FAKES".into(), s(self.fakes_dir())),
            (
                "PARITY_SECRETS".into(),
                s(fake::secrets_path(&self.fakes_dir())),
            ),
            ("LANG".into(), "C.UTF-8".into()),
        ]
        .into_iter()
        .chain(self.extra_env.lock().unwrap().iter().cloned())
        .collect()
    }

    /// Adds a variable to every child command's environment (not recorded by fakes unless the name matches their prefixes).
    pub fn set_env(&self, key: &str, value: &str) {
        self.extra_env
            .lock()
            .unwrap()
            .push((key.to_string(), value.to_string()));
    }

    /// Keeps one moor call's output so the secret scanner can read it later.
    pub fn log_transcript(&self, entry: String) {
        self.transcript.lock().unwrap().push(entry);
    }

    pub fn transcript(&self) -> Vec<String> {
        self.transcript.lock().unwrap().clone()
    }

    pub fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut cmd = Command::new(program);
        cmd.env_clear()
            .envs(self.base_env())
            .stdin(Stdio::null())
            .current_dir(self.root())
            .process_group(0);
        // SAFETY: signal() is async-signal-safe; resets SIGINT/SIGQUIT ignored by a bg shell.
        unsafe {
            cmd.pre_exec(|| {
                libc::signal(libc::SIGINT, libc::SIG_DFL);
                libc::signal(libc::SIGQUIT, libc::SIG_DFL);
                Ok(())
            });
        }
        cmd
    }

    pub fn calls(&self, tool: &str) -> Vec<Call> {
        fake::read_calls(&self.fakes_dir(), tool)
    }
    pub fn all_calls(&self) -> Vec<Call> {
        fake::all_calls(&self.fakes_dir())
    }
    pub fn add_rules(&self, tool: &str, rules: impl IntoIterator<Item = Rule>) {
        fake::append_rules(&self.fakes_dir(), tool, rules.into_iter().collect())
    }
    pub fn prepend_rules(&self, tool: &str, rules: impl IntoIterator<Item = Rule>) {
        fake::prepend_rules(&self.fakes_dir(), tool, rules.into_iter().collect())
    }

    pub fn track_pid(&self, pid: u32) {
        let start = proc_stat(pid).map_or(0, |s| s.starttime);
        self.pids.lock().unwrap().push((pid, start));
    }

    pub fn watch_creds_in(&mut self, dir: &Path) {
        self.cred_snaps.insert(dir.to_path_buf(), creds_in(dir));
    }

    pub fn assert_no_new_creds_in(&self, dir: &Path) {
        let snap = self
            .cred_snaps
            .get(dir)
            .unwrap_or_else(|| panic!("{} was never watched", dir.display()));
        let fresh: Vec<String> = creds_in(dir).difference(snap).cloned().collect();
        assert!(
            fresh.is_empty(),
            "new {}/moor-cred-* entries: {fresh:?}",
            dir.display()
        );
    }

    pub fn assert_no_new_tmp_creds(&self) {
        self.assert_no_new_creds_in(Path::new("/tmp"));
    }

    pub fn teardown(&mut self) -> Result<(), String> {
        if self.torn_down {
            return Ok(());
        }
        self.torn_down = true;
        let marker = format!("PARITY_FAKES={}", self.fakes_dir().display());
        let calls = self.all_calls();
        let tracked: Vec<(u32, u64)> = self.pids.lock().unwrap().clone();
        let proxies = proxy_pids(&self.state_dir());
        let mut known: Vec<u32> = calls.iter().map(|c| c.pid).collect();
        known.extend(tracked.iter().map(|t| t.0));
        known.extend(proxies.iter().copied());
        let mut groups: Vec<u32> = calls.iter().map(|c| c.pgid).collect();
        groups.extend(tracked.iter().map(|t| t.0));
        groups.extend(proxies.iter().copied());
        groups.sort_unstable();
        groups.dedup();

        let initial = find_marked(&marker);
        let mut leaks: Vec<(u32, String)> = initial
            .iter()
            .filter(|m| !known.contains(&m.pid))
            .map(|m| (m.pid, m.cmdline.clone()))
            .collect();
        let own_pgrp = unsafe { libc::getpgrp() };
        for g in groups {
            let g = g as libc::pid_t;
            if g > 1 && g != own_pgrp && initial.iter().any(|m| m.pgrp == g) {
                // SAFETY: plain syscall; the group holds a process carrying our marker.
                unsafe { libc::kill(-g, libc::SIGTERM) };
            }
        }
        for (pid, start) in &tracked {
            let same = proc_stat(*pid).is_some_and(|s| !s.zombie && s.starttime == *start);
            if same && *pid > 1 && !initial.iter().any(|m| m.pid == *pid) {
                // SAFETY: plain syscall; starttime proves the pid is still the recorded process.
                unsafe { libc::kill(-(*pid as libc::pid_t), libc::SIGTERM) };
            }
        }
        for m in &initial {
            // SAFETY: plain syscall on a pid whose environ carried our marker.
            unsafe { libc::kill(m.pid as libc::pid_t, libc::SIGTERM) };
        }
        let mut left = wait_marked(&marker, Duration::from_secs(2));
        if !left.is_empty() {
            for m in &left {
                // SAFETY: as above.
                unsafe { libc::kill(m.pid as libc::pid_t, libc::SIGKILL) };
                if !leaks.iter().any(|l| l.0 == m.pid) {
                    leaks.push((m.pid, m.cmdline.clone()));
                }
            }
            left = wait_marked(&marker, Duration::from_millis(500));
        }
        let _ = left;
        if leaks.is_empty() {
            return Ok(());
        }
        let list: Vec<String> = leaks.iter().map(|(p, c)| format!("{p} {c}")).collect();
        Err(format!("leaked processes: {}", list.join("; ")))
    }
}

impl Drop for World {
    fn drop(&mut self) {
        if self.torn_down {
            return;
        }
        if let Err(msg) = self.teardown() {
            if std::thread::panicking() {
                eprintln!("{msg}");
            } else {
                panic!("{msg}");
            }
        }
    }
}

#[cfg(test)]
mod tests;
