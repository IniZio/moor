//! A kept Box's tunnel: opened into the Box's machine on its Box port, reopened with backoff
//! when it dies, after any session an earlier tunnel left on the port is ended.

use std::time::{Duration, Instant};

use moor_core::domain::event::Event;
use moor_core::seam::driver::{DriverError, ExecSpec, TunnelHandle};
use moor_core::seam::proc::Deadline;
use moor_core::BoxHandle;

use super::Kept;

/// Waits before each try to reopen a dead tunnel; the last repeats.
const BACKOFF: [Duration; 6] = [
    Duration::ZERO,
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(30),
];

/// Time allowed to free the Box port from a session left behind by an earlier tunnel.
const CLEAR_DEADLINE: Duration = Duration::from_secs(45);

/// Shell run in the Box with the port as `$1`: ends each ssh session process (`sshd-session`
/// from OpenSSH 9.8, `sshd` before) that holds the loopback listener on it, IPv4 or IPv6, which
/// an earlier tunnel's ssh can leave behind after it was killed. It reads `/proc`, since a Box
/// image need not have `ss`.
const CLEAR_SCRIPT: &str = r#"p=$(printf ':%04X' "$1"); ino=$(cat /proc/net/tcp /proc/net/tcp6 2>/dev/null | awk -v p="$p" 'substr($2, length($2) - 4) == p && $4 == "0A" {printf " %s", $10}'); [ -n "$ino" ] || exit 0; for f in /proc/[0-9]*/fd/*; do l=$(readlink "$f" 2>/dev/null) || continue; case "$l" in "socket:["*"]") ;; *) continue ;; esac; i=${l#socket:[}; i=${i%]}; case "$ino " in *" $i "*) ;; *) continue ;; esac; pid=${f#/proc/}; pid=${pid%%/*}; case "$(cat "/proc/$pid/comm" 2>/dev/null)" in sshd | sshd-session) kill "$pid" 2>/dev/null ;; esac; done; exit 0"#;

impl Kept {
    /// Opens the tunnel when it is down and its next try is due; why it is down after. The
    /// caller holds the keeper's busy lock.
    pub(super) fn tend_tunnel(&self) -> Result<(), String> {
        let Some(socket) = &self.deps.egress else {
            return Ok(());
        };
        let mut inner = self.lock();
        let Some(handle) = inner.handle.clone().filter(|h| self.current(&inner, h)) else {
            return Ok(());
        };
        if inner.tunnel.as_ref().is_some_and(|t| t.alive()) {
            return Ok(());
        }
        let dead = inner.tunnel.take();
        if dead.is_some() {
            inner.was_up = true;
        }
        let now = self.deps.clock.instant();
        let due = inner.next_open.is_none_or(|at| now >= at);
        drop(inner);
        drop(dead);
        if !due {
            return Err("the tunnel is down and waits to be opened again".to_owned());
        }
        self.clear(&handle);
        let opened = self
            .deps
            .driver
            .reverse_tunnel(&handle, self.box_port, socket);
        self.keep_open(&handle, opened, now)
    }

    /// Keeps the tunnel opened into `handle` at `now`, or schedules the next try when it failed;
    /// a tunnel for a Box that changed meanwhile is closed.
    fn keep_open(
        &self,
        handle: &BoxHandle,
        opened: Result<Box<dyn TunnelHandle>, DriverError>,
        now: Instant,
    ) -> Result<(), String> {
        let mut inner = self.lock();
        if !self.current(&inner, handle) {
            drop(inner);
            drop(opened);
            return Err("the Box changed while its tunnel opened".to_owned());
        }
        match opened {
            Ok(tunnel) => {
                self.say(format_args!("opened the tunnel into {}", handle.as_str()));
                inner.tunnel = Some(tunnel);
                inner.failures = 0;
                inner.next_open = None;
                let reopened = std::mem::take(&mut inner.was_up);
                drop(inner);
                if reopened {
                    self.record(&Event::TunnelRestarted {
                        box_port: self.box_port,
                        reason: "tunnel exited".to_owned(),
                    });
                }
                Ok(())
            }
            Err(e) => {
                let wait = BACKOFF
                    .get(inner.failures + 1)
                    .or(BACKOFF.last())
                    .copied()
                    .unwrap_or_default();
                inner.failures += 1;
                inner.next_open = Some(now + wait);
                drop(inner);
                let why = format!("cannot open the tunnel into the Box: {e}");
                self.say(format_args!("{why}"));
                Err(why)
            }
        }
    }

    /// Ends any session that holds the Box port, reporting a failure; the open goes ahead.
    fn clear(&self, handle: &BoxHandle) {
        let spec = ExecSpec {
            argv: ["sh", "-c", CLEAR_SCRIPT, "sh"]
                .into_iter()
                .map(str::to_owned)
                .chain([self.box_port.to_string()])
                .collect(),
            env: Vec::new(),
            cwd: None,
            output: None,
        };
        let cleared = self
            .deps
            .driver
            .exec(handle, &spec, Deadline(CLEAR_DEADLINE));
        if let Err(e) = cleared.as_ref().map_err(DriverError::to_string) {
            self.say(format_args!(
                "cannot free port {} in the Box: {e}",
                self.box_port
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;
    use std::net::{Ipv6Addr, TcpListener};
    use std::time::Duration;

    use super::CLEAR_SCRIPT;

    /// Names the port [`hold_ipv6_port`] listens on in a child process.
    const HOLD: &str = "MOOR_TEST_HOLD_IPV6";

    #[test]
    fn hold_ipv6_port() {
        let Some(port) = std::env::var_os(HOLD) else {
            return;
        };
        let port: u16 = port.to_str().unwrap().parse().unwrap();
        let listener = TcpListener::bind((Ipv6Addr::LOCALHOST, port)).unwrap();
        let mut out = std::io::stdout();
        std::io::Write::write_all(&mut out, b"listening").unwrap();
        std::io::Write::flush(&mut out).unwrap();
        drop(listener.accept());
    }

    #[test]
    fn a_session_process_of_an_older_openssh_holding_the_port_over_ipv6_is_ended() {
        if std::env::var_os(HOLD).is_some() {
            return;
        }
        if TcpListener::bind((Ipv6Addr::LOCALHOST, 0)).is_err() {
            return;
        }
        let reserved = moor_testkit::LockedPort::reserve();
        let port = reserved.port();
        let dir = tempfile::tempdir().unwrap();
        let named = dir.path().join("sshd");
        std::os::unix::fs::symlink(std::env::current_exe().unwrap(), &named).unwrap();
        let mut held = moor_testkit::ChildGuard::new(
            std::process::Command::new(&named)
                .args(["--exact", "proxy::keeper::tunnel::tests::hold_ipv6_port"])
                .arg("--nocapture")
                .env(HOLD, port.to_string())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let mut out = held.stdout.take().unwrap();
        let mut said = Vec::new();
        let mut byte = [0_u8; 1];
        while !said.ends_with(b"listening") && out.read(&mut byte).unwrap() == 1 {
            said.push(byte[0]);
        }
        assert!(
            said.ends_with(b"listening"),
            "{}",
            String::from_utf8_lossy(&said)
        );
        let cleared = std::process::Command::new("sh")
            .args(["-c", CLEAR_SCRIPT, "sh", &port.to_string()])
            .status()
            .unwrap();
        assert!(cleared.success());
        let ended = (0..100).any(|_| {
            std::thread::sleep(Duration::from_millis(20));
            held.try_wait().unwrap().is_some()
        });
        assert!(ended, "the process holding the port was ended");
    }
}
