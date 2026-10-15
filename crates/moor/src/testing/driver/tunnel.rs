//! Reverse-tunnel half of the Driver fake: records requests and plays
//! scripted outcomes.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use moor_core::seam::driver::{DriverCaps, DriverError, SleepKind, TunnelFailure, TunnelHandle};
use moor_core::BoxHandle;

use super::FakeDriver;

/// First fake pid handed out to a tunnel.
const FIRST_TUNNEL_PID: u32 = 50_000;

/// How long a `Hang` outcome reports waiting before giving up.
const HANG_TIMEOUT: Duration = Duration::from_secs(30);

/// What `reverse_tunnel` returns.
pub(super) type Opened = Result<Box<dyn TunnelHandle>, DriverError>;

/// Scripted outcome of one `reverse_tunnel` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TunnelReply {
    /// The tunnel comes up and stays alive.
    Ready,
    /// A handle is returned, but the tunnel is already dead.
    ExitAfterReady,
    /// The tunnel exits before it is ready.
    EarlyExit { code: i32, stderr: String },
    /// The tunnel never becomes ready within the timeout.
    Hang,
}

/// One `reverse_tunnel` request, recorded even when it fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TunnelCall {
    pub(crate) handle: BoxHandle,
    pub(crate) box_port: u16,
    pub(crate) host_port: u16,
    pub(crate) host_cwd: PathBuf,
}

#[derive(Default)]
pub(super) struct TunnelState {
    enabled: bool,
    lacks_cap: bool,
    replies: VecDeque<TunnelReply>,
    calls: Vec<TunnelCall>,
    issued: u32,
    live: Vec<(u32, Arc<AtomicBool>)>,
    dropped: Arc<Mutex<Vec<u32>>>,
}

/// Fake tunnel; marks itself dead and records its pid when dropped.
struct FakeTunnel {
    pid: u32,
    alive: Arc<AtomicBool>,
    dropped: Arc<Mutex<Vec<u32>>>,
}

impl TunnelHandle for FakeTunnel {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }
}

impl Drop for FakeTunnel {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::SeqCst);
        self.dropped
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(self.pid);
    }
}

impl FakeDriver {
    /// Reports the reverse-tunnel capability and accepts tunnel requests.
    pub(crate) fn reverse_tunnels(self) -> Self {
        self.st().tunnel.enabled = true;
        self
    }

    /// Reports the provider as lacking the reverse-tunnel capability.
    pub(crate) fn without_reverse_tunnel_cap(self) -> Self {
        self.st().tunnel.lacks_cap = true;
        self
    }

    /// Queues outcomes served in order; once empty, requests are `Ready`.
    pub(crate) fn tunnel_replies(self, replies: impl IntoIterator<Item = TunnelReply>) -> Self {
        self.st().tunnel.replies.extend(replies);
        self
    }

    /// Every `reverse_tunnel` request so far, in call order.
    pub(crate) fn tunnels(&self) -> Vec<TunnelCall> {
        self.st().tunnel.calls.clone()
    }

    /// Makes the live tunnel with `pid` report dead, as if it crashed.
    pub(crate) fn end_tunnel(&self, pid: u32) {
        let s = self.st();
        for (_, alive) in s.tunnel.live.iter().filter(|(p, _)| *p == pid) {
            alive.store(false, Ordering::SeqCst);
        }
    }

    /// Pids of tunnel handles that were dropped, in drop order.
    pub(crate) fn dropped_tunnels(&self) -> Vec<u32> {
        let s = self.st();
        let dropped = s
            .tunnel
            .dropped
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        dropped.clone()
    }

    /// Reports `kind` as the provider's sleep capability.
    pub(crate) fn sleep_kind(self, kind: SleepKind) -> Self {
        self.st().sleep_kind = Some(kind);
        self
    }

    pub(super) fn caps_reply(&self) -> DriverCaps {
        let st = self.st();
        DriverCaps {
            sleep: st.sleep_kind.unwrap_or(SleepKind::None),
            wake: false,
            reverse_tunnel: !st.tunnel.lacks_cap,
            has_workdir: true,
        }
    }

    pub(super) fn tunnel_request(
        &self,
        handle: &BoxHandle,
        box_port: u16,
        host_port: u16,
        host_cwd: &Path,
    ) -> Opened {
        let mut s = self.st();
        s.calls.push(format!("reverse_tunnel {}", handle.as_str()));
        s.tunnel.calls.push(TunnelCall {
            handle: handle.clone(),
            box_port,
            host_port,
            host_cwd: host_cwd.to_path_buf(),
        });
        if !s.live.iter().any(|h| h == handle.as_str()) {
            return Err(DriverError::NotFound);
        }
        if !s.tunnel.enabled {
            return Err(DriverError::Unsupported("reverse tunnel"));
        }
        let reply = s.tunnel.replies.pop_front().unwrap_or(TunnelReply::Ready);
        let up = match reply {
            TunnelReply::Ready => true,
            TunnelReply::ExitAfterReady => false,
            TunnelReply::EarlyExit { code, stderr } => {
                return Err(DriverError::Tunnel {
                    failure: TunnelFailure::Exited {
                        code: Some(code),
                        signal: None,
                    },
                    stderr_tail: stderr,
                });
            }
            TunnelReply::Hang => {
                return Err(DriverError::Tunnel {
                    failure: TunnelFailure::TimedOut(HANG_TIMEOUT),
                    stderr_tail: String::new(),
                });
            }
        };
        let pid = FIRST_TUNNEL_PID + s.tunnel.issued;
        s.tunnel.issued += 1;
        let alive = Arc::new(AtomicBool::new(up));
        s.tunnel.live.push((pid, Arc::clone(&alive)));
        let dropped = Arc::clone(&s.tunnel.dropped);
        Ok(Box::new(FakeTunnel {
            pid,
            alive,
            dropped,
        }))
    }
}

#[cfg(test)]
mod tests;
