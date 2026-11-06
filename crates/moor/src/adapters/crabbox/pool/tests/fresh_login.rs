//! What setup does to the Box user's groups reaches the next login on a node Box. Real ssh dials
//! through the same `ProxyCommand` shape the links broker hands out, and each dial starts a real
//! `sshd -i` as root of its own user and mount namespace, so it sets a login's groups with
//! `initgroups` from a group file only that namespace sees. The Box user is not root, as sshd
//! sets a root login's groups again for each session.

use std::fs::{self, Permissions};
use std::io::{Read as _, Write as _};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use moor_client::link::lease::{Answer, Boot, Verb};
use moor_core::seam::driver::{BoxStep, Driver, ExecOutcome, StepDone};
use moor_core::seam::proc::Deadline;
use tempfile::TempDir;

use super::{handle, PoolHosts};
use crate::adapters::crabbox::link;
use crate::adapters::crabbox::tests::fake_ssh::driver;
use crate::adapters::crabbox::Crabbox;
use crate::adapters::proc::ToolPaths;

const SSHD: &str = "/usr/sbin/sshd";
const SSH: &str = "/usr/bin/ssh";
const UNSHARE: &str = "unshare --user --map-root-user --map-auto --setgroups=allow --mount";
/// The Box user.
const USER: &str = "box";
/// A group outside the user's own, inside the range the namespace maps.
const DOCKER: &str = "2000";

/// A node host that answers with the VM's sign-in and dials through `dial`.
struct Node {
    dial: String,
    host_key: String,
}

impl PoolHosts for Node {
    fn lease(
        &self,
        _machine: i64,
        verb: Verb,
        _slug: Option<&str>,
        _key: Option<&str>,
        _boot: &Boot,
    ) -> Result<Answer, String> {
        Ok(match verb {
            Verb::Info => Answer::Info {
                user: USER.to_owned(),
                host_keys: vec![self.host_key.clone()],
            },
            _ => Answer::Done,
        })
    }

    fn dial_command(&self, _machine: i64, _slug: &str) -> Result<String, String> {
        Ok(self.dial.clone())
    }
}

struct Rig {
    dir: TempDir,
    cb: Crabbox,
}

/// Whether this host lets a test map a range of groups in a user namespace.
fn namespaces_work() -> bool {
    let probe = format!("{UNSHARE} true");
    let ran = Command::new("sh")
        .args(["-c", &probe])
        .stderr(std::process::Stdio::null())
        .status();
    Path::new(SSHD).exists() && ran.is_ok_and(|s| s.success())
}

fn keygen(path: &Path) {
    let made = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", ""])
        .arg("-f")
        .arg(path)
        .status()
        .unwrap();
    assert!(made.success());
}

fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let open_to = |path: &Path, mode| fs::set_permissions(path, Permissions::from_mode(mode));
    open_to(d, 0o755).unwrap();
    keygen(&d.join("host"));
    fs::create_dir(d.join("box")).unwrap();
    open_to(&d.join("box"), 0o777).unwrap();
    let config = format!(
        "HostKey {d}/host\nAuthorizedKeysFile {d}/authorized_keys\nStrictModes no\nUsePAM no\n\
         PermitRootLogin yes\nPasswordAuthentication no\nKbdInteractiveAuthentication no\n\
         PidFile none\n",
        d = d.display()
    );
    fs::write(d.join("sshd_config"), config).unwrap();
    write_group(d, "");
    write_passwd(d);
    let boot = format!(
        "umask 022; mount -t tmpfs tmpfs /run && mkdir /run/sshd && \
         mount --bind '{d}/group' /etc/group && mount --bind '{d}/passwd' /etc/passwd && \
         exec {SSHD} -i -f '{d}/sshd_config' -E '{d}/sshd.log'",
        d = d.display()
    );
    let dial = d.join("dial");
    let script = format!(
        "#!/bin/sh\necho >> '{d}/connects'\nexec {UNSHARE} sh -c \"{boot}\"\n",
        d = d.display()
    );
    moor_testkit::write_executable(&dial, script).unwrap();
    let host = Node {
        dial: dial.display().to_string(),
        host_key: fs::read_to_string(d.join("host.pub")).unwrap(),
    };
    let tools = ToolPaths {
        ssh: Some(SSH.into()),
        ..ToolPaths::default()
    };
    let cb = driver(d, &tools).with_pool_hosts(Some(Arc::new(host) as Arc<dyn PoolHosts>));
    let key = cb.controller_key().unwrap();
    fs::copy(key.with_extension("pub"), d.join("authorized_keys")).unwrap();
    Rig { dir, cb }
}

/// The account file every login reads: the Box user and the host's sshd account, as sshd drops
/// to it for its unprivileged part.
fn write_passwd(dir: &Path) {
    let host = fs::read_to_string("/etc/passwd").unwrap();
    let sshd = host
        .lines()
        .find(|l| l.starts_with("sshd:"))
        .unwrap_or_default();
    let text = format!("{USER}:x:1000:1000::/tmp:/bin/sh\n{sshd}\n");
    fs::write(dir.join("passwd"), text).unwrap();
}

/// The group file the next login reads, with `members` in the docker group.
fn write_group(dir: &Path, members: &str) {
    let text = format!("{USER}:x:1000:\ndocker:x:{DOCKER}:{members}\n");
    fs::write(dir.join("group"), text).unwrap();
}

impl Rig {
    fn connects(&self) -> usize {
        fs::read_to_string(self.dir.path().join("connects")).map_or(0, |s| s.lines().count())
    }

    /// What `script` prints in the Box; it must exit 0.
    fn run(&self, script: &str) -> String {
        let steps = [BoxStep::Run(script.to_owned())];
        let deadline = Deadline(Duration::from_secs(20));
        let done = self.cb.batch(&handle(), &[], &steps, deadline);
        let done = done.into_iter().next();
        let exited_0 = matches!(
            &done,
            Some(Ok(StepDone::Ran {
                outcome: ExecOutcome::Exited { code: 0 },
                ..
            }))
        );
        assert!(exited_0, "{script}: {done:?}; {}", self.sshd_log());
        match done {
            Some(Ok(StepDone::Ran { stdout, .. })) => stdout,
            _ => String::new(),
        }
    }

    fn sshd_log(&self) -> String {
        fs::read_to_string(self.dir.path().join("sshd.log")).unwrap_or_default()
    }

    fn groups(&self) -> Vec<String> {
        let out = self.run("id -G");
        out.split_whitespace().map(str::to_owned).collect()
    }

    /// As `usermod -aG docker` does in setup.
    fn setup_joins_docker(&self) {
        write_group(self.dir.path(), USER);
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        link::close(&self.cb, &handle());
    }
}

#[expect(
    clippy::print_stderr,
    reason = "the skip notice says why the test did not run"
)]
fn skipped() -> bool {
    let skip = !namespaces_work();
    if skip {
        eprintln!("skipped: no sshd or no user namespace with a mapped group range here");
    }
    skip
}

#[test]
fn a_group_setup_adds_reaches_the_login_after_a_refresh() {
    if skipped() {
        return;
    }
    let rig = rig();
    assert!(!rig.groups().contains(&DOCKER.to_owned()));
    rig.setup_joins_docker();
    assert!(
        !rig.groups().contains(&DOCKER.to_owned()),
        "a login on the open master keeps the groups it connected with"
    );
    assert_eq!(rig.connects(), 1);

    rig.cb.refresh_login(&handle()).unwrap();
    assert_eq!(
        rig.groups().iter().filter(|g| *g == DOCKER).count(),
        1,
        "the agent's login has the group setup added: {}",
        rig.sshd_log()
    );
    assert_eq!(rig.connects(), 2);

    rig.cb.refresh_login(&handle()).unwrap();
    rig.groups();
    assert_eq!(rig.connects(), 2, "a login with every group is kept");
}

#[test]
fn a_refresh_leaves_a_call_in_flight_and_the_tunnel_running() {
    if skipped() {
        return;
    }
    let rig = rig();
    rig.run("true");

    let socket = rig.dir.path().join("proxy.sock");
    let proxy = UnixListener::bind(&socket).unwrap();
    let port = {
        let free = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        free.local_addr().unwrap().port()
    };
    let tunnel = rig.cb.reverse_tunnel(&handle(), port, &socket).unwrap();
    thread::spawn(move || {
        for stream in proxy.incoming() {
            let mut stream = stream.unwrap();
            stream.write_all(b"proxy").unwrap();
        }
    });
    let through_the_tunnel = || {
        let mut got = String::new();
        let mut box_side = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        box_side.read_to_string(&mut got).unwrap();
        got
    };
    assert_eq!(through_the_tunnel(), "proxy");
    let opened = rig.connects();
    assert_eq!(opened, 2, "the tunnel keeps its own connection");

    let started = rig.dir.path().join("box/started");
    let in_flight = format!("touch '{}'; sleep 2; echo finished", started.display());
    thread::scope(|scope| {
        let call = scope.spawn(|| rig.run(&in_flight));
        while !started.exists() {
            thread::sleep(Duration::from_millis(20));
        }
        rig.setup_joins_docker();
        rig.cb.refresh_login(&handle()).unwrap();
        assert!(rig.groups().contains(&DOCKER.to_owned()));
        assert_eq!(rig.connects(), opened + 1);
        assert_eq!(call.join().unwrap(), "finished\n", "it ran to its end");
    });

    assert!(tunnel.alive());
    assert_eq!(through_the_tunnel(), "proxy");
    assert_eq!(rig.connects(), opened + 1);
}
