//! moor-gate: the remote front. It checks a person's OIDC access token once per WebSocket
//! connection, then relays the controller's socket protocol to the controller's front socket,
//! acting as that person's identity on every request.

use std::io::{self, Write};
use std::net::TcpListener;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use rustix::event::{poll, PollFd, PollFlags};
use rustix::io::Errno;
use signal_hook::consts::{SIGINT, SIGTERM};

mod enroll;
mod events_http;
mod http;
mod login;
mod mcp;
mod relay;
mod rewrite;
mod token;
mod tools;

pub use login::Login;
pub use mcp::Mcp;
pub use token::{Person, Verifier};

/// The identity namespace the gate asserts; the controller's `fronts` key for this socket.
pub const NAMESPACE: &str = "authgear";

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
    loop {
        let (stream, _) = listener.accept()?;
        let gate = Arc::clone(gate);
        let login = login.cloned();
        std::thread::spawn(move || http::handle(stream, &gate, login.as_deref()));
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
        std::thread::spawn(move || {
            http::handle(stream, &gate, login.as_deref());
            drop(held);
        });
    }
    drop(listener);
    let (count, ended) = &*live;
    let (open, timed_out) = ended
        .wait_timeout_while(
            count.lock().unwrap_or_else(PoisonError::into_inner),
            drain_timeout,
            |n| *n > 0,
        )
        .map_or_else(
            |e| (*e.get_ref().0, e.get_ref().1.timed_out()),
            |(held, wait)| (*held, wait.timed_out()),
        );
    if timed_out {
        log(&format!("drain timed out with {open} connections open"));
    }
    Ok(())
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
