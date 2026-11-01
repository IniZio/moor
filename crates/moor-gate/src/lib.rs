//! moor-gate: the remote front. It checks a person's OIDC access token once per WebSocket
//! connection, then relays the controller's socket protocol to the controller's front socket,
//! acting as that person's identity on every request.

use std::io::{self, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;

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

/// Writes one `moor-gate: <msg>` line to stderr; a failed write has nowhere to go.
pub(crate) fn log(msg: &str) {
    let _ = writeln!(io::stderr(), "moor-gate: {msg}");
}
