//! moor-gate: the remote front. It checks a person's OIDC access token once per WebSocket
//! connection, then relays the controller's socket protocol to the controller's front socket,
//! acting as that person's identity on every request.

use std::io::{self, Write};
use std::net::TcpListener;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use streams::Streams;

use rustix::event::{poll, PollFd, PollFlags};
use rustix::io::Errno;
use signal_hook::consts::{SIGINT, SIGTERM};

mod artifact;
mod artifacts_http;
mod enroll;
mod events_http;
mod http;
mod login;
mod mcp;
mod relay;
mod rewrite;
mod streams;
mod token;
mod tools;

pub use login::Login;
pub use mcp::Mcp;
pub use token::{Person, Verifier};

/// The identity namespace the gate asserts; the controller's `fronts` key for this socket.
pub const NAMESPACE: &str = "authgear";

/// Seconds a drain lets long-lived streams (`/front`, `/events`, `/mcp` SSE) run before
/// closing them.
/// Clients reconnect, so a deploy never waits on one.
pub const STREAM_DRAIN_SECS: u64 = 5;

/// What one gate serves.
pub struct Gate {
    /// The controller's front socket for [`NAMESPACE`].
    pub front_socket: PathBuf,
    /// Checks access tokens.
    pub verifier: Verifier,
    /// The `/mcp` surface; `/mcp` is a 404 without it.
    pub mcp: Option<Mcp>,
}

/// Serves `listener` until it fails, one thread per connection.
///
/// # Errors
/// The listener's accept failure.
pub fn serve(listener: &TcpListener, gate: &Arc<Gate>) -> io::Result<()> {
    serve_with(listener, gate, None)
}

/// Serves `listener` like [`serve`], with browser sign-in at `/auth/*` when `login` is given.
///
/// # Errors
/// The listener's accept failure.
pub fn serve_with(
    listener: &TcpListener,
    gate: &Arc<Gate>,
    login: Option<&Arc<Login>>,
) -> io::Result<()> {
    let streams = Arc::new(Streams::default());
    loop {
        let (stream, _) = listener.accept()?;
        let gate = Arc::clone(gate);
        let login = login.cloned();
        let streams = Arc::clone(&streams);
        std::thread::spawn(move || http::handle(stream, &gate, login.as_deref(), &streams));
    }
}

/// Takes SIGTERM and SIGINT for the gate: the returned socket turns readable once either
/// arrives, which is what [`serve_until`] waits on.
///
/// # Errors
/// The socket pair or a handler could not be set up.
pub fn on_termination() -> io::Result<UnixStream> {
    let (wake, signalled) = UnixStream::pair()?;
    signal_hook::low_level::pipe::register(SIGTERM, signalled.try_clone()?)?;
    signal_hook::low_level::pipe::register(SIGINT, signalled)?;
    Ok(wake)
}

/// Serves `listener` like [`serve_with`] until `wake` turns readable, then drains: nothing
/// more is accepted, and connections in flight get up to `drain_timeout` to end.
///
/// # Errors
/// The listener's accept failure or a failed wait.
pub fn serve_until(
    listener: TcpListener,
    gate: &Arc<Gate>,
    login: Option<&Arc<Login>>,
    wake: &UnixStream,
    drain_timeout: Duration,
) -> io::Result<()> {
    let live = Arc::new((Mutex::new(0_usize), Condvar::new()));
    let streams = Arc::new(Streams::default());
    loop {
        let mut fds = [
            PollFd::new(&listener, PollFlags::IN),
            PollFd::new(wake, PollFlags::IN),
        ];
        match poll(&mut fds, None) {
            Ok(_) => {}
            Err(Errno::INTR) => continue,
            Err(e) => return Err(e.into()),
        }
        if fds[1].revents().intersects(PollFlags::IN | PollFlags::HUP) {
            break;
        }
        let (stream, _) = listener.accept()?;
        let (gate, login, held) = (Arc::clone(gate), login.cloned(), Held::new(&live));
        let streams = Arc::clone(&streams);
        std::thread::spawn(move || {
            http::handle(stream, &gate, login.as_deref(), &streams);
            drop(held);
        });
    }
    drop(listener);
    if let Some(mcp) = &gate.mcp {
        mcp.drain();
    }
    let (count, ended) = &*live;
    let streams_bound = Duration::from_secs(STREAM_DRAIN_SECS).min(drain_timeout);
    let (mut open, mut timed_out) = wait_for_none(count, ended, streams_bound);
    if open > 0 && timed_out {
        streams.close_all();
        if let Some(mcp) = &gate.mcp {
            mcp.end_streams();
        }
        (open, timed_out) =
            wait_for_none(count, ended, drain_timeout.saturating_sub(streams_bound));
    }
    if timed_out {
        log(&format!("drain timed out with {open} connections open"));
    }
    Ok(())
}

/// Waits up to `bound` for no connection to be left; returns how many are, and whether the
/// wait ran out.
fn wait_for_none(count: &Mutex<usize>, ended: &Condvar, bound: Duration) -> (usize, bool) {
    ended
        .wait_timeout_while(
            count.lock().unwrap_or_else(PoisonError::into_inner),
            bound,
            |n| *n > 0,
        )
        .map_or_else(
            |e| (*e.get_ref().0, e.get_ref().1.timed_out()),
            |(held, wait)| (*held, wait.timed_out()),
        )
}

/// One connection being served; the drain waits until none are left.
struct Held(Arc<(Mutex<usize>, Condvar)>);

impl Held {
    /// Counts one more connection.
    fn new(live: &Arc<(Mutex<usize>, Condvar)>) -> Self {
        *live.0.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        Self(Arc::clone(live))
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        *self.0 .0.lock().unwrap_or_else(PoisonError::into_inner) -= 1;
        self.0 .1.notify_all();
    }
}

/// Writes one `moor-gate: <msg>` line to stderr; a failed write has nowhere to go.
pub(crate) fn log(msg: &str) {
    let _ = writeln!(io::stderr(), "moor-gate: {msg}");
}
