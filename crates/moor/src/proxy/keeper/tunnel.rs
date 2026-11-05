//! A kept Box's tunnel: opened into the Box's machine on its Box port, reopened with backoff
//! when it dies, after any session an earlier tunnel left on the port is ended.

use std::time::Duration;

use moor_core::domain::event::Event;
use moor_core::seam::driver::{DriverError, ExecSpec};
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

/// Shell run in the Box with the port as `$1`: ends each `sshd-session` that holds the loopback
/// listener on it, which an earlier tunnel's ssh can leave behind after it was killed. It reads
/// `/proc`, since a Box image need not have `ss`.
const CLEAR_SCRIPT: &str = r#"p=$(printf ':%04X' "$1"); ino=$(awk -v p="$p" 'substr($2, length($2) - 4) == p && $4 == "0A" {print $10}' /proc/net/tcp 2>/dev/null); [ -n "$ino" ] || exit 0; for f in /proc/[0-9]*/fd/*; do [ "$(readlink "$f" 2>/dev/null)" = "socket:[$ino]" ] || continue; pid=${f#/proc/}; pid=${pid%%/*}; [ "$(cat "/proc/$pid/comm" 2>/dev/null)" = sshd-session ] && kill "$pid" 2>/dev/null; done; exit 0"#;

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
        let mut inner = self.lock();
        if !self.current(&inner, &handle) {
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
