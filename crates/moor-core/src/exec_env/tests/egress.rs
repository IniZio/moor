//! The egress lines run under a real `sh` against a fake Box: fake `apt-get`, `sudo`, `id` and
//! `tinyproxy`, and a loopback port that opens when the fake tinyproxy starts.

use std::path::{Path, PathBuf};

use moor_testkit::{FakeEgress, FAKE_TINYPROXY};

use super::super::egress::{config, lines, EGRESS_FAILED};

/// Tools the egress lines call that the fake Box takes from the host.
const HOST_TOOLS: &[&str] = &[
    "sh", "cmp", "mkdir", "mv", "rm", "grep", "sed", "sleep", "cat", "tail", "env", "cp", "chmod",
    "flock", "setsid",
];

/// Whether the fake Box has apt and what an install does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Apt {
    /// No `apt-get` on the Box.
    Missing,
    /// `apt-get install` installs the fake tinyproxy.
    Works,
    /// `apt-get` fails with a last line naming the package.
    Fails,
}

/// A fake Box: a temp `HOME` whose `bin` is the whole `PATH`.
struct FakeBox {
    home: PathBuf,
    bin: PathBuf,
    port: FakeEgress,
    _dir: tempfile::TempDir,
}

const UPSTREAM: u16 = 23456;

fn hosts(list: &[&str]) -> Vec<String> {
    list.iter().map(|h| (*h).to_owned()).collect()
}

impl FakeBox {
    fn new(uid: u32, apt: Apt, installed: bool) -> Self {
        let dir = tempfile::Builder::new()
            .prefix("moor-egress-")
            .tempdir()
            .unwrap();
        let home = dir.path().to_path_buf();
        let bin = home.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        for tool in HOST_TOOLS {
            let found = ["/usr/bin", "/bin"]
                .iter()
                .map(|d| Path::new(d).join(tool))
                .find(|p| p.exists())
                .unwrap();
            std::os::unix::fs::symlink(found, bin.join(tool)).unwrap();
        }
        let write = |name: &str, body: &str| {
            moor_testkit::write_executable(&bin.join(name), body).unwrap();
        };
        write(
            "id",
            &format!("#!/bin/sh\n[ \"$1\" = -u ] && echo {uid} || echo uid={uid}\n"),
        );
        write(
            "sudo",
            "#!/bin/sh\n[ \"$1\" = -n ] && shift\necho \"sudo\" >>\"$HOME/fake.log\"\nexec \"$@\"\n",
        );
        let record = "echo \"apt-get $* proxy=${HTTPS_PROXY-}${HTTP_PROXY-}${https_proxy-}${http_proxy-}${ALL_PROXY-}\" >>\"$HOME/fake.log\"";
        match apt {
            Apt::Missing => {}
            Apt::Works => write(
                "apt-get",
                &format!(
                    "#!/bin/sh\n{record}\ncase \"$*\" in *install*) cp \"$HOME/bin/tinyproxy.deb\" \"$HOME/bin/tinyproxy\";; esac\n"
                ),
            ),
            Apt::Fails => write(
                "apt-get",
                &format!(
                    "#!/bin/sh\n{record}\necho 'E: Unable to locate package tinyproxy-bin'\nexit 100\n"
                ),
            ),
        }
        moor_testkit::write_executable(&bin.join("tinyproxy.deb"), FAKE_TINYPROXY).unwrap();
        let port = FakeEgress::start(&home, &bin).unwrap();
        if !installed {
            std::fs::remove_file(bin.join("tinyproxy")).unwrap();
        }
        Self {
            home,
            bin,
            port,
            _dir: dir,
        }
    }

    /// The egress lines for `hosts`, then `echo ran`, under `sh` with the proxy variables set as a
    /// Box image might set them.
    fn run_with(&self, hosts: &[String]) -> (bool, String, String) {
        Self::outcome(&self.command(hosts).output().unwrap())
    }

    /// The `sh` that runs the egress lines for `hosts`, then `echo ran`.
    fn command(&self, hosts: &[String]) -> std::process::Command {
        let mut script = lines(self.port.port(), UPSTREAM, hosts).join("\n");
        script.push_str("\necho ran\n");
        let mut cmd = std::process::Command::new(self.bin.join("sh"));
        cmd.args(["-c", &script])
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", &self.bin)
            .env("HTTPS_PROXY", "http://image-proxy:1")
            .env("http_proxy", "http://image-proxy:1");
        cmd
    }

    fn outcome(out: &std::process::Output) -> (bool, String, String) {
        (
            out.status.success() && String::from_utf8_lossy(&out.stdout).contains("ran"),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn run(&self) -> (bool, String, String) {
        self.run_with(&hosts(&["api.anthropic.com", "github.com"]))
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.home.join("fake.log")).unwrap_or_default()
    }

    fn starts(&self) -> usize {
        self.log()
            .lines()
            .filter(|l| l.starts_with("tinyproxy "))
            .count()
    }

    fn conf(&self) -> String {
        std::fs::read_to_string(self.home.join(".moor/egress.conf")).unwrap()
    }

    /// Kills the placeholder process and closes the port, as a Box that slept and lost tinyproxy.
    fn kill(&self) {
        let pid = std::fs::read_to_string(self.home.join(".moor/egress.pid")).unwrap();
        let _ = std::process::Command::new("kill").arg(pid.trim()).status();
        self.port.close();
    }
}

impl Drop for FakeBox {
    fn drop(&mut self) {
        if let Ok(pid) = std::fs::read_to_string(self.home.join(".moor/egress.pid")) {
            let pid = pid.trim();
            let _ = std::process::Command::new("kill")
                .args(["-KILL", pid])
                .status();
            for _ in 0..100 {
                if !running(pid) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
}

/// Whether process `pid` runs. An exited process that no one reaped yet does not: the test
/// namespace's init never reaps the orphans the fake tinyproxy leaves.
fn running(pid: &str) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        stat.rsplit(") ")
            .next()
            .is_some_and(|rest| !rest.starts_with('Z'))
    })
}

fn failure(reason: &str) -> String {
    format!("moor: {EGRESS_FAILED}: {reason}")
}

#[test]
fn the_config_sends_each_credential_host_to_the_tunnel_and_allows_only_loopback_and_docker() {
    let got = config(
        40123,
        20123,
        &hosts(&[
            "github.com",
            "api.anthropic.com",
            ".corp.example",
            "github.com",
            "git.internal.example",
        ]),
    );
    assert_eq!(
        got,
        "Port 40123\nListen 0.0.0.0\nDisableViaHeader Yes\nLogLevel Critical\nMaxClients 1024\n\
         Allow 127.0.0.1\nAllow 172.16.0.0/12\n\
         upstream http 127.0.0.1:20123 \".corp.example\"\n\
         upstream http 127.0.0.1:20123 \"api.anthropic.com\"\n\
         upstream http 127.0.0.1:20123 \"git.internal.example\"\n\
         upstream http 127.0.0.1:20123 \"github.com\"\n"
    );
    for absent in ["LogFile", "ConnectPort", "Syslog"] {
        assert!(!got.contains(absent), "{absent}: {got}");
    }
}

#[test]
fn tinyproxy_starts_with_its_descriptor_limit_raised_to_the_hard_limit() {
    let b = FakeBox::new(0, Apt::Works, true);
    moor_testkit::write_executable(
        &b.bin.join("tinyproxy"),
        "#!/bin/sh\necho \"nofile $(ulimit -Sn) of $(ulimit -Hn)\" >>\"$HOME/fake.log\"\nexec \"$HOME/bin/tinyproxy.deb\" \"$@\"\n",
    )
    .unwrap();
    let script = format!(
        "ulimit -Sn 256\n{}\necho ran\n",
        lines(b.port.port(), UPSTREAM, &hosts(&["github.com"])).join("\n")
    );
    let out = std::process::Command::new(b.bin.join("sh"))
        .args(["-c", &script])
        .env_clear()
        .env("HOME", &b.home)
        .env("PATH", &b.bin)
        .output()
        .unwrap();
    let (ok, stdout, stderr) = FakeBox::outcome(&out);
    assert!(ok, "{stdout}{stderr}");
    let log = b.log();
    let line = log
        .lines()
        .find_map(|l| l.strip_prefix("nofile "))
        .expect(&log);
    let (soft, hard) = line.split_once(" of ").unwrap();
    assert_eq!(soft, hard, "{log}");
    assert_ne!(soft, "256", "{log}");
}

#[test]
fn a_config_with_no_credential_host_sends_everything_direct() {
    assert!(!config(40000, 20000, &[]).contains("upstream"));
}

#[test]
fn a_root_box_installs_tinyproxy_with_apt_without_sudo_or_proxy_variables() {
    let b = FakeBox::new(0, Apt::Works, false);
    let (ok, out, err) = b.run();
    assert!(ok, "{out}{err}");
    let log = b.log();
    assert!(!log.contains("sudo"), "{log}");
    let apt: Vec<&str> = log.lines().filter(|l| l.starts_with("apt-get")).collect();
    assert_eq!(apt.len(), 1, "{log}");
    let install = apt.first().copied().unwrap_or_default();
    assert!(install.contains("install tinyproxy-bin"), "{log}");
    assert!(
        install.ends_with("proxy="),
        "apt saw a proxy variable: {log}"
    );
    assert_eq!(b.starts(), 1, "{log}");
}

#[test]
fn a_non_root_box_installs_tinyproxy_through_sudo() {
    let b = FakeBox::new(1000, Apt::Works, false);
    let (ok, out, err) = b.run();
    assert!(ok, "{out}{err}");
    let log = b.log();
    let first: Vec<&str> = log.lines().take(2).collect();
    assert_eq!(first.first(), Some(&"sudo"), "{log}");
    assert!(
        first
            .get(1)
            .is_some_and(|l| l.starts_with("apt-get") && l.ends_with("proxy=")),
        "{log}"
    );
}

#[test]
fn a_box_without_apt_stops_the_exec_before_the_command_naming_apt() {
    let b = FakeBox::new(0, Apt::Missing, false);
    let (ok, out, err) = b.run();
    assert!(!ok);
    assert!(!out.contains("ran"), "{out}");
    assert!(
        err.contains(&failure(
            "tinyproxy is not installed and the Box has no apt-get; moor installs it only with apt"
        )),
        "{err}"
    );
    assert_eq!(b.starts(), 0);
}

#[test]
fn a_failed_install_stops_the_exec_with_the_last_apt_line() {
    let b = FakeBox::new(0, Apt::Fails, false);
    let (ok, out, err) = b.run();
    assert!(!ok);
    assert!(!out.contains("ran"), "{out}");
    assert!(
        err.contains(&failure(
            "apt-get could not install tinyproxy-bin: E: Unable to locate package tinyproxy-bin"
        )),
        "{err}"
    );
    let updates = b.log().lines().filter(|l| l.contains(" update")).count();
    assert_eq!(
        updates, 1,
        "a failed install retries once after apt-get update"
    );
}

#[test]
fn a_tinyproxy_that_never_listens_stops_the_exec() {
    let b = FakeBox::new(0, Apt::Missing, true);
    moor_testkit::write_executable(&b.bin.join("tinyproxy"), "#!/bin/sh\nexit 0\n").unwrap();
    let (ok, out, err) = b.run();
    assert!(!ok);
    assert!(!out.contains("ran"), "{out}");
    let reason = format!("tinyproxy is not listening on 127.0.0.1:{}", b.port.port());
    assert!(err.contains(&failure(&reason)), "{err}");
}

#[test]
fn tinyproxy_starts_from_the_config_with_its_pid_file_under_dot_moor() {
    let b = FakeBox::new(0, Apt::Missing, true);
    let (ok, out, err) = b.run();
    assert!(ok, "{out}{err}");
    let conf_path = b.home.join(".moor/egress.conf");
    assert!(
        b.log()
            .contains(&format!("tinyproxy -c {}", conf_path.display())),
        "{}",
        b.log()
    );
    let pid_file = b.home.join(".moor/egress.pid");
    let expected = config(
        b.port.port(),
        UPSTREAM,
        &hosts(&["api.anthropic.com", "github.com"]),
    ) + &format!("PidFile \"{}\"\n", pid_file.display());
    assert_eq!(b.conf(), expected);
    assert!(pid_file.exists());
}

#[test]
fn a_running_egress_proxy_is_left_alone() {
    let b = FakeBox::new(0, Apt::Missing, true);
    assert!(b.run().0);
    let (ok, out, err) = b.run();
    assert!(ok, "{out}{err}");
    assert_eq!(b.starts(), 1, "{}", b.log());
}

#[test]
fn a_dead_egress_proxy_is_started_again_before_the_command() {
    let b = FakeBox::new(0, Apt::Missing, true);
    assert!(b.run().0);
    b.kill();
    let (ok, out, err) = b.run();
    assert!(ok, "{out}{err}");
    assert_eq!(b.starts(), 2, "{}", b.log());
}

#[test]
fn a_changed_credential_host_list_restarts_the_egress_proxy() {
    let b = FakeBox::new(0, Apt::Missing, true);
    assert!(b.run().0);
    let pid = std::fs::read_to_string(b.home.join(".moor/egress.pid")).unwrap();
    b.port.close();
    let (ok, out, err) = b.run_with(&hosts(&["api.anthropic.com", "github.com", "gitlab.com"]));
    assert!(ok, "{out}{err}");
    assert_eq!(b.starts(), 2, "{}", b.log());
    assert!(b.conf().contains("\"gitlab.com\""));
    assert!(!running(pid.trim()), "the old tinyproxy was not stopped");
}

/// The real tinyproxy removes its pid file as it exits, a moment after TERM. When the port it
/// held is already free, as after a change of egress port, the restart must still wait for it.
#[test]
fn a_restart_keeps_the_new_pid_file_when_the_old_tinyproxy_exits_late() {
    let b = FakeBox::new(0, Apt::Missing, true);
    std::fs::create_dir_all(b.home.join(".fake-tinyproxy")).unwrap();
    moor_testkit::write_executable(
        &b.home.join(".fake-tinyproxy/tinyproxy"),
        "#!/bin/sh\ntrap 'sleep 0.5; rm -f \"$HOME/.moor/egress.pid\"; exit 0' TERM\nsleep \"$1\" & wait $!\n",
    )
    .unwrap();
    assert!(b.run().0);
    let old = std::fs::read_to_string(b.home.join(".moor/egress.pid")).unwrap();
    b.port.close();
    let changed = hosts(&["api.anthropic.com", "github.com", "gitlab.com"]);
    let (ok, out, err) = b.run_with(&changed);
    assert!(ok, "{out}{err}");
    std::thread::sleep(std::time::Duration::from_secs(1));
    assert!(!running(old.trim()), "the old tinyproxy was not stopped");
    let (ok, out, err) = b.run_with(&changed);
    assert!(ok, "{out}{err}");
    assert_eq!(b.starts(), 2, "{}", b.log());
}

/// An old tinyproxy that ignores TERM keeps serving the old host list on the same port. The
/// config on disk must stay the old one, so each command retries the restart and none reports
/// success while the old list is served.
#[test]
fn a_restart_that_fails_leaves_the_old_config_so_the_next_command_retries() {
    let b = FakeBox::new(0, Apt::Missing, true);
    std::fs::create_dir_all(b.home.join(".fake-tinyproxy")).unwrap();
    moor_testkit::write_executable(
        &b.home.join(".fake-tinyproxy/tinyproxy"),
        "#!/bin/sh\ntrap : TERM\nwhile :; do sleep 0.1; done\n",
    )
    .unwrap();
    assert!(b.run().0);
    let old_conf = b.conf();
    let old = std::fs::read_to_string(b.home.join(".moor/egress.pid"))
        .unwrap()
        .trim()
        .to_owned();
    let changed = hosts(&["api.anthropic.com", "github.com", "gitlab.com"]);
    for attempt in 1..=2 {
        let (ok, out, err) = b.run_with(&changed);
        assert!(!ok, "attempt {attempt} reported success: {out}{err}");
        assert!(
            err.contains(&failure(&format!(
                "the old tinyproxy {old} did not exit after TERM"
            ))),
            "attempt {attempt}: {err}"
        );
        assert_eq!(b.conf(), old_conf, "attempt {attempt} replaced the config");
        assert_eq!(b.starts(), 1, "{}", b.log());
    }
    let _ = std::process::Command::new("kill")
        .args(["-KILL", &old])
        .status();
    b.port.close();
    let (ok, out, err) = b.run_with(&changed);
    assert!(ok, "{out}{err}");
    assert_eq!(b.starts(), 2, "{}", b.log());
    assert!(b.conf().contains("\"gitlab.com\""));
}

#[test]
fn a_port_held_by_another_process_stops_the_exec_and_starts_nothing() {
    let b = FakeBox::new(0, Apt::Missing, true);
    b.port.occupy();
    let (ok, out, err) = b.run();
    assert!(!ok);
    assert!(!out.contains("ran"), "{out}");
    let reason = format!("port {} is taken by another process", b.port.port());
    assert!(err.contains(&failure(&reason)), "{err}");
    assert_eq!(b.starts(), 0, "{}", b.log());
}

#[test]
fn a_wildcard_listener_on_the_port_counts_as_listening() {
    let b = FakeBox::new(0, Apt::Missing, true);
    b.port.occupy_wildcard();
    let (ok, out, err) = b.run();
    assert!(!ok);
    let reason = format!("port {} is taken by another process", b.port.port());
    assert!(err.contains(&failure(&reason)), "{out}{err}");
    assert_eq!(b.starts(), 0, "{}", b.log());
}

#[test]
fn a_pid_file_naming_a_process_that_is_not_tinyproxy_does_not_claim_the_port() {
    let b = FakeBox::new(0, Apt::Missing, true);
    let mut other = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    std::fs::create_dir_all(b.home.join(".moor")).unwrap();
    std::fs::write(b.home.join(".moor/egress.pid"), format!("{}\n", other.id())).unwrap();
    b.port.occupy();
    let (ok, _, err) = b.run();
    let alive = other.try_wait().unwrap().is_none();
    let _ = other.kill();
    let _ = other.wait();
    assert!(!ok);
    let reason = format!("port {} is taken by another process", b.port.port());
    assert!(err.contains(&failure(&reason)), "{err}");
    assert!(alive, "the process the pid file named was left running");
}

#[test]
fn two_commands_that_start_together_start_one_tinyproxy() {
    let b = FakeBox::new(0, Apt::Missing, true);
    let list = hosts(&["api.anthropic.com", "github.com"]);
    let first = b
        .command(&list)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let second = b
        .command(&list)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    for child in [first, second] {
        let (ok, out, err) = FakeBox::outcome(&child.wait_with_output().unwrap());
        assert!(ok, "{out}{err}");
    }
    assert_eq!(b.starts(), 1, "{}", b.log());
}
