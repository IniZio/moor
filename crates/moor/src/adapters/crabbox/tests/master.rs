//! The Box master over real OpenSSH: ssh dials an `sshd -i` started by its `ProxyCommand`,
//! so each new connection is one `ProxyCommand` run and no port is opened.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::Duration;

use moor_core::name::BoxPath;
use moor_core::seam::driver::Driver;

use super::fake_ssh::{driver, handle, lab, Lab};
use crate::adapters::crabbox::link;
use crate::adapters::crabbox::Crabbox;
use crate::adapters::proc::ToolPaths;

const SSHD: &str = "/usr/sbin/sshd";
const SSH: &str = "/usr/bin/ssh";
/// The master's idle window here, in place of the ten minutes moor uses. ssh counts it in
/// whole seconds, so a window of a few seconds can end up to one second early.
const PERSIST: u32 = 4;

fn keygen(path: &Path) {
    let made = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", ""])
        .arg("-f")
        .arg(path)
        .status()
        .unwrap();
    assert!(made.success());
}

/// A driver whose lookups print words that reach this host's `sshd -i` with its host key
/// pinned, as crabbox prints them, its masters idling [`PERSIST`] seconds.
fn real_box(lab: &Lab) -> Crabbox {
    let d = lab.dir.path();
    keygen(&d.join("host"));
    keygen(&d.join("id"));
    fs::copy(d.join("id.pub"), d.join("authorized_keys")).unwrap();
    let config = format!(
        "HostKey {d}/host\nAuthorizedKeysFile {d}/authorized_keys\nStrictModes no\nUsePAM no\n\
         PasswordAuthentication no\nKbdInteractiveAuthentication no\nPidFile none\n",
        d = d.display()
    );
    fs::write(d.join("sshd_config"), config).unwrap();
    let proxy = d.join("proxy");
    let script = format!(
        "#!/bin/sh\necho >> '{d}/connects'\nexec {SSHD} -i -f '{d}/sshd_config' -E '{d}/sshd.log'\n",
        d = d.display()
    );
    moor_testkit::write_executable(&proxy, script).unwrap();
    let host_key = fs::read_to_string(d.join("host.pub")).unwrap();
    let mut key = host_key.split_whitespace();
    let (kind, blob) = (key.next().unwrap(), key.next().unwrap());
    fs::write(
        d.join("known_hosts"),
        format!("crabbox-lease-t {kind} {blob}\n"),
    )
    .unwrap();
    let user = Command::new("id").arg("-un").output().unwrap().stdout;
    let user = String::from_utf8(user).unwrap();
    let words = format!(
        "ssh -F none -o 'ControlMaster=no' -o 'ControlPath=none' -o 'StrictHostKeyChecking=yes' \
         -o 'UserKnownHostsFile={d}/known_hosts' -o 'GlobalKnownHostsFile=none' \
         -o 'HostKeyAlias=crabbox-lease-t' -o 'ProxyCommand={proxy}' -o 'BatchMode=yes' \
         -o 'IdentitiesOnly=yes' -i '{d}/id' '{user}@box'\n",
        d = d.display(),
        proxy = proxy.display(),
        user = user.trim(),
    );
    fs::write(lab.bin().join("words"), words).unwrap();
    let tools = ToolPaths {
        ssh: Some(SSH.into()),
        crabbox: Some(lab.bin().join("crabbox")),
        ..ToolPaths::default()
    };
    driver(d, &tools).with_persist(PERSIST)
}

fn connects(lab: &Lab) -> usize {
    fs::read_to_string(lab.dir.path().join("connects")).map_or(0, |s| s.lines().count())
}

#[test]
#[expect(
    clippy::print_stderr,
    reason = "the skip notice says why the test did not run"
)]
fn a_call_inside_the_idle_window_reuses_the_master_and_one_after_it_reconnects() {
    if !Path::new(SSHD).exists() {
        eprintln!("skipped: {SSHD} is missing, so no OpenSSH server can stand in for the Box");
        return;
    }
    let lab = lab();
    let cb = real_box(&lab);
    let file = lab.dir.path().join("hook");
    fs::write(&file, "idle").unwrap();
    let path = BoxPath::restore(&file.display().to_string()).unwrap();
    let read = || cb.peek_file(&handle(), &path);

    assert_eq!(read(), Ok(Some("idle".to_owned())));
    assert_eq!(connects(&lab), 1);
    thread::sleep(Duration::from_millis(1500));
    assert_eq!(read(), Ok(Some("idle".to_owned())));
    assert_eq!(
        connects(&lab),
        1,
        "the second call rode the first call's master"
    );
    thread::sleep(Duration::from_secs(u64::from(PERSIST) + 2));
    assert_eq!(read(), Ok(Some("idle".to_owned())));
    assert_eq!(connects(&lab), 2, "the master ended after its idle window");
    link::close(&cb, &handle());
}
