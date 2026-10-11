pub mod config;
pub mod repo;

use crate::fake::{self, Call, Rule};
use std::ffi::OsStr;
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const TOOLS: &[&str] = &["crabbox", "herdr", "claude", "ssh", "gh"];

const GITCONFIG: &str = "[user]\n\tname = parity\n\temail = parity@example.invalid\n\
[init]\n\tdefaultBranch = main\n[protocol \"file\"]\n\tallow = always\n";

pub struct World {
    dir: tempfile::TempDir,
    pids: Mutex<Vec<u32>>,
    suffix: String,
    torn_down: bool,
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
        let dir = tempfile::Builder::new()
            .prefix("parity-")
            .tempdir()
            .expect("create world tempdir");
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
            suffix: random_hex6(),
            torn_down: false,
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
            ("TMPDIR".into(), s(self.tmp_dir())),
            ("PARITY_FAKES".into(), s(self.fakes_dir())),
            ("LANG".into(), "C.UTF-8".into()),
        ]
    }

    pub fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut cmd = Command::new(program);
        cmd.env_clear()
            .envs(self.base_env())
            .stdin(Stdio::null())
            .current_dir(self.root())
            .process_group(0);
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
        self.pids.lock().unwrap().push(pid);
    }

    pub fn teardown(&mut self) -> Result<(), String> {
        if self.torn_down {
            return Ok(());
        }
        self.torn_down = true;
        let mut pids: Vec<u32> = self.all_calls().iter().map(|c| c.pid).collect();
        pids.extend(self.pids.lock().unwrap().iter().copied());
        pids.sort_unstable();
        pids.dedup();
        for pid in pids {
            signal_tree(pid, libc::SIGTERM);
        }
        let root = self.root().to_string_lossy().into_owned();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut survivors = find_survivors(&root);
        while !survivors.is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
            survivors = find_survivors(&root);
        }
        if survivors.is_empty() {
            return Ok(());
        }
        for (pid, _) in &survivors {
            signal_tree(*pid, libc::SIGKILL);
        }
        let list: Vec<String> = survivors.iter().map(|(p, c)| format!("{p} {c}")).collect();
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

fn signal_tree(pid: u32, sig: i32) {
    if pid <= 1 || pid > i32::MAX as u32 {
        return;
    }
    let pid = pid as libc::pid_t;
    // SAFETY: plain syscalls on integer ids; failures (ESRCH, EPERM) are ignored.
    unsafe {
        let pgid = libc::getpgid(pid);
        if pgid > 1 && pgid != libc::getpgrp() {
            libc::kill(-pgid, sig);
        }
        libc::kill(pid, sig);
    }
}

fn find_survivors(root: &str) -> Vec<(u32, String)> {
    let me = std::process::id();
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return out;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == me {
            continue;
        }
        let Ok(raw) = fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        let cmdline = String::from_utf8_lossy(&raw).replace('\0', " ");
        if !cmdline.contains(root) || is_zombie(pid) {
            continue;
        }
        out.push((pid, cmdline.trim_end().to_string()));
    }
    out
}

fn is_zombie(pid: u32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|s| {
            s.rsplit_once(')')
                .map(|(_, rest)| rest.trim_start().starts_with('Z'))
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> World {
        World::new("/bin/true")
    }

    #[test]
    fn layout_and_env() {
        let w = world();
        assert!(w
            .root()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("parity-"));
        for t in TOOLS {
            assert!(fs::symlink_metadata(w.tool_path(t))
                .unwrap()
                .file_type()
                .is_symlink());
        }
        for d in [
            "box/work", "box/home", "box/tmp", "state", "fakes", "tmp", "trees", "repos",
        ] {
            assert!(w.root().join(d).is_dir(), "{d}");
        }
        let mode = fs::metadata(w.home().join(".config"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
        assert!(fs::read_to_string(w.home().join(".gitconfig"))
            .unwrap()
            .contains("parity@example.invalid"));
        let env = w.base_env();
        assert!(env.contains(&("GIT_CONFIG_NOSYSTEM".to_string(), "1".to_string())));
        let name = w.task_name("t");
        let hex = name.strip_prefix("t-").unwrap();
        assert!(
            hex.len() == 6
                && hex
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn clean_teardown_ok_and_idempotent() {
        let mut w = world();
        assert_eq!(w.teardown(), Ok(()));
        assert_eq!(w.teardown(), Ok(()));
    }

    #[test]
    fn leak_detected_and_killed() {
        let mut w = world();
        let script = r#"trap "" TERM; while :; do sleep 0.1; done"#;
        let mut child = w
            .command("sh")
            .arg("-c")
            .arg(script)
            .arg(w.root())
            .spawn()
            .unwrap();
        w.track_pid(child.id());
        std::thread::sleep(Duration::from_millis(200));
        let err = w.teardown().unwrap_err();
        assert!(err.starts_with("leaked processes: "), "{err}");
        assert!(err.contains(&child.id().to_string()), "{err}");
        let status = child.wait().unwrap();
        assert!(!status.success());
        assert!(find_survivors(&w.root().to_string_lossy()).is_empty());
    }
}
