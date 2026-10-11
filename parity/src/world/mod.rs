pub mod config;
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
use std::time::{Duration, Instant};

pub const TOOLS: &[&str] = &["crabbox", "herdr", "claude", "ssh", "gh"];

const GITCONFIG: &str = "[user]\n\tname = parity\n\temail = parity@example.invalid\n\
[init]\n\tdefaultBranch = main\n[protocol \"file\"]\n\tallow = always\n";

pub struct World {
    dir: tempfile::TempDir,
    pids: Mutex<Vec<(u32, u64)>>,
    cred_snaps: BTreeMap<PathBuf, BTreeSet<String>>,
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
        let mut known: Vec<u32> = calls.iter().map(|c| c.pid).collect();
        known.extend(tracked.iter().map(|t| t.0));
        let mut groups: Vec<u32> = calls.iter().map(|c| c.pgid).collect();
        groups.extend(tracked.iter().map(|t| t.0));
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

struct Marked {
    pid: u32,
    pgrp: libc::pid_t,
    cmdline: String,
}

struct Stat {
    zombie: bool,
    pgrp: libc::pid_t,
    starttime: u64,
}

fn proc_stat(pid: u32) -> Option<Stat> {
    let raw = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, rest) = raw.rsplit_once(')')?;
    let f: Vec<&str> = rest.split_whitespace().collect();
    Some(Stat {
        zombie: f.first()? == &"Z",
        pgrp: f.get(2)?.parse().ok()?,
        starttime: f.get(19)?.parse().ok()?,
    })
}

fn find_marked(marker: &str) -> Vec<Marked> {
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
        let Ok(env) = fs::read(entry.path().join("environ")) else {
            continue;
        };
        if !env.split(|b| *b == 0).any(|e| e == marker.as_bytes()) {
            continue;
        }
        let Some(st) = proc_stat(pid) else { continue };
        if st.zombie {
            continue;
        }
        let raw = fs::read(entry.path().join("cmdline")).unwrap_or_default();
        let cmdline = String::from_utf8_lossy(&raw).replace('\0', " ");
        out.push(Marked {
            pid,
            pgrp: st.pgrp,
            cmdline: cmdline.trim_end().to_string(),
        });
    }
    out
}

fn wait_marked(marker: &str, limit: Duration) -> Vec<Marked> {
    let deadline = Instant::now() + limit;
    let mut left = find_marked(marker);
    while !left.is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        left = find_marked(marker);
    }
    left
}

fn creds_in(dir: &Path) -> BTreeSet<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return BTreeSet::new();
    };
    entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("moor-cred-"))
        .collect()
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
        assert!(find_marked(&format!("PARITY_FAKES={}", w.fakes_dir().display())).is_empty());
    }
}
