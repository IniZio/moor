//! Shared ssh connections, checked against a model of OpenSSH connection sharing.

use std::collections::HashMap;
use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moor_core::name::{BoxHandle, BoxPath};
use moor_core::seam::clock::{Clock, Ts};
use moor_core::seam::driver::{BoxStep, Driver, DriverError};
use moor_core::seam::proc::{Deadline, Tool};
use tempfile::TempDir;

use crate::adapters::crabbox::Crabbox;
use crate::testing::clock::FakeClock;
use crate::testing::proc::{Reply, ScriptedProc};
use crate::testing::rng::FakeRng;

/// What crabbox prints for an agent37 Box, with connection sharing turned off.
const SSH_LINE: &str = "ssh -o 'ControlMaster=no' -o 'ControlPath=none' -o 'ControlPersist=no' -o 'ProxyCommand=tunnel 7' 'u@box'\n";
const HOOK: &str = r#"{"state":"idle"}"#;
/// Longest `ControlPath` OpenSSH binds on macOS: 103 bytes less its 17-byte temporary suffix.
const SSH_MAX_PATH: usize = 103 - 17;

/// OpenSSH connection sharing, as far as moor relies on it. ssh keeps the first value given
/// for an option. A call whose `ControlPath` names a listening master rides on it, and hangs
/// when that master's connection stalled. Any other call connects, and with
/// `ControlMaster=auto` and a `ControlPersist` it leaves a master listening, in place of a dead
/// one. `-O exit` and `-O stop` take a listening master's socket away, and fail when no master
/// answers. A call whose master loses the network exits 255 and prints nothing.
#[derive(Default)]
struct OpenSsh {
    masters: HashMap<PathBuf, Master>,
    connects: usize,
    /// New connections are refused, as for a target that moved.
    refuse: bool,
    /// The next shared call loses its master mid-call.
    drop_next: bool,
    /// What the master logs to the `-E` file when it loses its master.
    master_log: String,
}

#[derive(Clone, Copy)]
enum Master {
    Up,
    Stalled,
}

impl OpenSsh {
    fn reply(&mut self, args: &[String]) -> Reply {
        let path = option(args, "ControlPath")
            .filter(|p| p != "none")
            .map(PathBuf::from);
        if path
            .as_ref()
            .is_some_and(|p| p.as_os_str().len() > SSH_MAX_PATH)
        {
            return fail("ControlPath too long");
        }
        if args.iter().any(|a| a == "-O") {
            let ended = path.filter(|p| self.masters.remove(p).is_some());
            return ended.map_or_else(
                || fail("Control socket connect: Connection refused"),
                |p| {
                    std::fs::remove_file(p).unwrap();
                    Reply::Ok(String::new())
                },
            );
        }
        match path.as_ref().and_then(|p| self.masters.get(p)) {
            Some(Master::Stalled) => return Reply::Timeout,
            Some(Master::Up) if self.drop_next => {
                self.drop_next = false;
                self.masters.clear();
                if let Some(log) = flag(args, "-E") {
                    std::fs::write(log, &self.master_log).unwrap();
                }
                return fail("");
            }
            Some(Master::Up) => return remote(args),
            None => {}
        }
        self.connects += 1;
        if self.refuse {
            return fail("Permission denied (publickey).");
        }
        let auto = option(args, "ControlMaster").as_deref() == Some("auto");
        let persist = option(args, "ControlPersist").is_some_and(|v| v != "no");
        if let (true, true, Some(p)) = (auto, persist, path) {
            std::fs::write(&p, "").unwrap();
            self.masters.insert(p, Master::Up);
        }
        remote(args)
    }
}

/// The remote command's result: a read of a file named `absent` exits 44, anything else
/// prints the hook record.
fn remote(args: &[String]) -> Reply {
    if args.last().is_some_and(|c| c.contains("absent")) {
        return Reply::Fail {
            code: 44,
            stdout: String::new(),
            stderr: String::new(),
        };
    }
    Reply::Ok(HOOK.to_owned())
}

/// The first value of `-o name=value` in `args`.
fn option(args: &[String], name: &str) -> Option<String> {
    args.windows(2).find_map(|w| match w {
        [o, v] if o == "-o" => v.strip_prefix(name)?.strip_prefix('=').map(str::to_owned),
        _ => None,
    })
}

/// The value after `name` in `args`.
fn flag(args: &[String], name: &str) -> Option<String> {
    args.windows(2).find_map(|w| match w {
        [f, v] if f == name => Some(v.clone()),
        _ => None,
    })
}

fn fail(stderr: &str) -> Reply {
    Reply::Fail {
        code: 255,
        stdout: String::new(),
        stderr: stderr.to_owned(),
    }
}

struct Lab {
    _dir: TempDir,
    state: PathBuf,
    sockets: PathBuf,
    proc: Arc<ScriptedProc>,
    clock: Arc<FakeClock>,
    ssh: Arc<Mutex<OpenSsh>>,
}

fn lab() -> Lab {
    let dir = tempfile::tempdir().unwrap();
    lab_in(dir.path().join("state"), dir)
}

fn lab_in(state: PathBuf, dir: TempDir) -> Lab {
    let proc = Arc::new(ScriptedProc::new());
    let ssh = Arc::new(Mutex::new(OpenSsh::default()));
    let model = Arc::clone(&ssh);
    proc.on(Tool::Crabbox, &["ssh"], Reply::Ok(SSH_LINE.to_owned()))
        .on(Tool::Crabbox, &["stop"], Reply::Ok(String::new()))
        .on(
            Tool::Ssh,
            &[],
            Reply::Computed(Arc::new(move |args| model.lock().unwrap().reply(args))),
        );
    Lab {
        sockets: dir.path().join("sockets"),
        _dir: dir,
        state,
        proc,
        clock: Arc::new(FakeClock::new(Ts::from_millis(1_000))),
        ssh,
    }
}

impl Lab {
    /// A driver wired the way the controller wires one on every settle tick.
    fn driver(&self) -> Crabbox {
        self.driver_in(self.state.clone())
    }

    /// A driver like [`Lab::driver`] whose state dir is `state`.
    fn driver_in(&self, state: PathBuf) -> Crabbox {
        let clock: Arc<dyn Clock> = self.clock.clone();
        let driver = Crabbox::new(self.proc.clone(), clock, Arc::new(FakeRng::default()))
            .with_state_dir(Some(state), self.sockets.clone());
        driver.version_ok.set(()).unwrap();
        driver
    }

    /// One probe tick: a freshly wired driver reads the agent's hook file.
    fn probe_tick(&self) -> Result<Option<String>, DriverError> {
        self.read("agent-state.json")
    }

    fn read(&self, name: &str) -> Result<Option<String>, DriverError> {
        let path = BoxPath::under("/work", &format!(".moor/{name}")).unwrap();
        self.driver()
            .peek_file(&handle(), &path, Path::new("/host/wt"))
    }

    fn stream(&self) -> Result<(), DriverError> {
        self.driver().stream_files(
            &handle(),
            &BoxPath::restore("/work").unwrap(),
            &["a".to_owned()],
            Path::new("/host/wt"),
            Deadline(Duration::from_secs(5)),
            &mut |_, _| {},
        )
    }

    fn batch(&self) -> Result<(), DriverError> {
        let steps = [BoxStep::Run("true".to_owned())];
        let deadline = Deadline(Duration::from_secs(5));
        let done = self
            .driver()
            .batch(&handle(), &[], &steps, Path::new("/host/wt"), deadline);
        done.into_iter().next().unwrap().map(drop)
    }

    fn connects(&self) -> usize {
        self.ssh.lock().unwrap().connects
    }

    fn lookups(&self) -> usize {
        let calls = self.proc.calls().into_iter();
        calls
            .filter(|c| c.tool == Tool::Crabbox && c.args.first().is_some_and(|a| a == "ssh"))
            .count()
    }

    /// The sockets dir entries that are not logs.
    fn sockets_only(&self) -> Vec<PathBuf> {
        let all = std::fs::read_dir(&self.sockets).unwrap();
        let paths = all.map(|e| e.unwrap().path());
        paths.filter(|p| p.extension().is_none()).collect()
    }

    /// The log file of the Box's master.
    fn log(&self) -> PathBuf {
        let all = std::fs::read_dir(&self.sockets).unwrap();
        let mut paths = all.map(|e| e.unwrap().path());
        paths.find(|p| p.extension().is_some()).unwrap()
    }

    fn link_files(&self) -> Vec<PathBuf> {
        [self.state.join("ssh"), self.sockets.clone()]
            .iter()
            .flat_map(|d| std::fs::read_dir(d).unwrap())
            .map(|e| e.unwrap().path())
            .collect()
    }
}

fn handle() -> BoxHandle {
    BoxHandle::new("external/moor-1-abc123")
}

#[test]
fn idle_box_probe_ticks_share_one_connection() {
    let lab = lab();
    for _ in 0..10 {
        assert_eq!(lab.probe_tick().unwrap().as_deref(), Some(HOOK));
    }
    assert_eq!(lab.connects(), 1, "ssh connections over 10 ticks");
    assert_eq!(lab.lookups(), 1, "crabbox target lookups over 10 ticks");
}

#[test]
fn every_short_call_shares_the_connection() {
    let lab = lab();
    lab.probe_tick().unwrap();
    let put = lab.driver().put_file(
        &handle(),
        &BoxPath::restore("/work/f").unwrap(),
        b"x",
        0o600,
        Path::new("/host/wt"),
    );
    put.unwrap();
    let _ = lab.batch();
    let _ = lab.stream();
    assert_eq!(
        lab.connects(),
        1,
        "connections for a read, write, batch and stream"
    );
}

#[test]
fn a_missing_file_keeps_the_connection() {
    let lab = lab();
    assert_eq!(lab.read("absent").unwrap(), None);
    lab.probe_tick().unwrap();
    assert_eq!(lab.connects(), 1);
}

#[test]
fn a_dead_master_is_replaced_on_the_next_tick() {
    let lab = lab();
    lab.probe_tick().unwrap();
    lab.ssh.lock().unwrap().masters.clear();
    for _ in 0..5 {
        assert_eq!(lab.probe_tick().unwrap().as_deref(), Some(HOOK));
    }
    assert_eq!(lab.connects(), 2, "one connect, one reconnect");
}

#[test]
fn a_call_whose_master_drops_mid_call_is_a_lost_connection() {
    let lab = lab();
    lab.probe_tick().unwrap();
    lab.ssh.lock().unwrap().drop_next = true;
    assert!(
        matches!(lab.batch(), Err(DriverError::Transport { .. })),
        "a silent exit 255 on a shared call"
    );
}

#[test]
fn a_stalled_master_is_dropped_and_the_next_tick_reconnects() {
    let lab = lab();
    lab.probe_tick().unwrap();
    for stalled in [Lab::probe_tick as fn(&Lab) -> _, |l: &Lab| {
        l.stream().map(|()| None)
    }] {
        lab.ssh
            .lock()
            .unwrap()
            .masters
            .values_mut()
            .for_each(|m| *m = Master::Stalled);
        assert!(matches!(stalled(&lab), Err(DriverError::Timeout(_))));
        for _ in 0..3 {
            assert_eq!(lab.probe_tick().unwrap().as_deref(), Some(HOOK));
        }
    }
    assert_eq!(lab.connects(), 3, "one connect, one reconnect per stall");
    assert_eq!(lab.lookups(), 1, "a stall keeps the saved target");
}

#[test]
fn an_ssh_failure_asks_crabbox_for_the_target_again() {
    let lab = lab();
    lab.probe_tick().unwrap();
    let mut ssh = lab.ssh.lock().unwrap();
    ssh.masters.clear();
    ssh.refuse = true;
    drop(ssh);
    assert!(lab.probe_tick().is_err());
    lab.ssh.lock().unwrap().refuse = false;
    lab.probe_tick().unwrap();
    assert_eq!(lab.lookups(), 2, "the failed target is looked up again");
}

#[test]
fn a_saved_target_is_looked_up_again_after_ten_minutes() {
    let lab = lab();
    lab.probe_tick().unwrap();
    lab.clock.sleep(Duration::from_mins(9));
    lab.probe_tick().unwrap();
    assert_eq!(lab.lookups(), 1, "within ten minutes");
    lab.clock.sleep(Duration::from_mins(2));
    lab.probe_tick().unwrap();
    assert_eq!(lab.lookups(), 2, "after ten minutes");
    assert_eq!(lab.connects(), 1, "a lookup keeps the connection");
}

#[test]
fn release_ends_the_master_and_removes_its_socket() {
    let lab = lab();
    lab.probe_tick().unwrap();
    assert!(!lab.link_files().is_empty());
    lab.driver().release(&handle(), None).unwrap();
    assert!(
        lab.ssh.lock().unwrap().masters.is_empty(),
        "master still up"
    );
    assert_eq!(lab.link_files(), Vec::<PathBuf>::new());
}

#[test]
fn release_removes_the_socket_of_a_dead_master() {
    let lab = lab();
    lab.probe_tick().unwrap();
    lab.ssh.lock().unwrap().masters.clear();
    lab.driver().release(&handle(), None).unwrap();
    assert_eq!(lab.link_files(), Vec::<PathBuf>::new());
}

#[test]
fn a_state_dir_too_deep_for_a_socket_still_shares_one_connection() {
    let dir = tempfile::tempdir().unwrap();
    let deep = dir.path().join("d".repeat(SSH_MAX_PATH));
    let lab = lab_in(deep, dir);
    for _ in 0..3 {
        assert_eq!(lab.probe_tick().unwrap().as_deref(), Some(HOOK));
    }
    assert_eq!(lab.connects(), 1, "ssh connections over 3 ticks");
    assert_eq!(lab.driver().link_warning(), None);
}

#[test]
fn two_state_dirs_never_share_a_master() {
    let lab = lab();
    lab.probe_tick().unwrap();
    let other = lab.driver_in(lab.state.with_file_name("other"));
    let path = BoxPath::under("/work", ".moor/agent-state.json").unwrap();
    other
        .peek_file(&handle(), &path, Path::new("/host/wt"))
        .unwrap();
    assert_eq!(lab.connects(), 2, "one master per state dir");
}

/// Asserts that `driver` warns about the lab's socket dir and that its calls connect one by one.
fn assert_unshared(lab: &Lab, driver: &Crabbox) {
    let warning = driver.link_warning().expect("a warning");
    assert_eq!(warning.code, "ssh_not_shared");
    assert!(
        warning.message.contains(&lab.sockets.display().to_string()),
        "{}",
        warning.message
    );
    let path = BoxPath::under("/work", ".moor/agent-state.json").unwrap();
    for _ in 0..2 {
        let read = driver.peek_file(&handle(), &path, Path::new("/host/wt"));
        assert_eq!(read.unwrap().as_deref(), Some(HOOK));
    }
    assert_eq!(lab.connects(), 2, "one connection per call");
}

#[test]
fn a_socket_dir_others_can_enter_is_a_warning_and_calls_connect_on_their_own() {
    let lab = lab();
    std::fs::create_dir(&lab.sockets).unwrap();
    std::fs::set_permissions(&lab.sockets, Permissions::from_mode(0o755)).unwrap();
    assert_unshared(&lab, &lab.driver());
}

#[test]
fn a_socket_dir_of_another_user_is_a_warning_and_calls_connect_on_their_own() {
    let lab = lab();
    let mut driver = lab.driver();
    let dirs = driver.links.as_mut().unwrap();
    dirs.owner = dirs.owner.checked_add(1).unwrap();
    assert_unshared(&lab, &driver);
}

#[test]
fn a_symlinked_socket_dir_is_a_warning_and_calls_connect_on_their_own() {
    let lab = lab();
    let real = lab.sockets.with_file_name("real");
    std::fs::create_dir(&real).unwrap();
    std::fs::set_permissions(&real, Permissions::from_mode(0o700)).unwrap();
    std::os::unix::fs::symlink(&real, &lab.sockets).unwrap();
    assert_unshared(&lab, &lab.driver());
}

#[test]
fn the_agent_pane_keeps_its_own_connection() {
    let lab = lab();
    lab.probe_tick().unwrap();
    let argv = lab
        .driver()
        .connect_argv(&handle(), Path::new("/host/wt"))
        .unwrap();
    assert_eq!(option(&argv, "ControlPath").as_deref(), Some("none"));
}

#[test]
fn a_shared_call_logs_the_masters_errors_to_a_private_file() {
    let lab = lab();
    lab.probe_tick().unwrap();
    let call = lab
        .proc
        .calls()
        .into_iter()
        .find(|c| c.tool == Tool::Ssh)
        .unwrap();
    assert_eq!(option(&call.args, "LogLevel").as_deref(), Some("ERROR"));
    let log = PathBuf::from(flag(&call.args, "-E").expect("an -E log"));
    assert_eq!(log.parent(), Some(lab.sockets.as_path()));
    let mode = std::fs::metadata(&log).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "log mode");
}

#[test]
fn a_lost_connection_carries_what_the_master_logged() {
    let lab = lab();
    lab.probe_tick().unwrap();
    {
        let mut ssh = lab.ssh.lock().unwrap();
        ssh.drop_next = true;
        ssh.master_log = "Connection closed by UNKNOWN port 65535\n".to_owned();
    }
    let err = lab.batch().unwrap_err();
    assert!(
        err.to_string()
            .ends_with("Connection closed by UNKNOWN port 65535"),
        "{err}"
    );
}

#[test]
fn the_logged_tail_is_capped() {
    let lab = lab();
    lab.probe_tick().unwrap();
    {
        let mut ssh = lab.ssh.lock().unwrap();
        ssh.drop_next = true;
        ssh.master_log = format!("{}\nlast line\n", "x".repeat(5_000));
    }
    let err = lab.batch().unwrap_err();
    let lost = "moor lost the ssh connection to the Box: ";
    let stderr_tail = err.to_string().strip_prefix(lost).unwrap().to_owned();
    assert!(stderr_tail.ends_with("last line"), "{stderr_tail}");
    assert!(stderr_tail.chars().count() <= 300, "{}", stderr_tail.len());
}

#[test]
fn a_new_master_starts_an_empty_log() {
    let lab = lab();
    lab.probe_tick().unwrap();
    let log = lab.log();
    std::fs::write(&log, "old master error\n").unwrap();
    for socket in lab.sockets_only() {
        std::fs::remove_file(socket).unwrap();
    }
    lab.ssh.lock().unwrap().masters.clear();
    lab.probe_tick().unwrap();
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "");
}

#[test]
fn an_oversized_log_is_emptied_before_the_next_call() {
    let lab = lab();
    lab.probe_tick().unwrap();
    let log = lab.log();
    std::fs::write(&log, "x".repeat(40_000)).unwrap();
    lab.probe_tick().unwrap();
    assert_eq!(std::fs::metadata(&log).unwrap().len(), 0);
}

fn silent_loss() -> moor_core::seam::proc::ProcError {
    moor_core::seam::proc::ProcError::NonZero {
        code: 255,
        stdout: String::new(),
        stderr_tail: String::new(),
    }
}

#[test]
fn a_lost_connection_in_an_unshared_dir_does_not_read_its_log() {
    let lab = lab();
    std::fs::create_dir(&lab.sockets).unwrap();
    std::fs::set_permissions(&lab.sockets, Permissions::from_mode(0o755)).unwrap();
    let driver = lab.driver();
    let log = driver.link(&handle()).unwrap().log;
    std::fs::write(&log, "PLANTED-MARKER").unwrap();
    let err = super::lost(&driver, &handle(), &silent_loss()).unwrap();
    assert!(!err.to_string().contains("PLANTED-MARKER"), "{err}");
}

#[test]
fn a_symlinked_log_is_not_read() {
    let lab = lab();
    lab.probe_tick().unwrap();
    let secret = lab.sockets.with_file_name("secret");
    std::fs::write(&secret, "SECRET-MARKER").unwrap();
    let log = lab.log();
    std::fs::remove_file(&log).unwrap();
    std::os::unix::fs::symlink(&secret, &log).unwrap();
    let err = super::lost(&lab.driver(), &handle(), &silent_loss()).unwrap();
    assert!(!err.to_string().contains("SECRET-MARKER"), "{err}");
}
