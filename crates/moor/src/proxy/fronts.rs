//! The front Proxy that `moor proxy serve-fronts` runs for the controller: one CONNECT
//! listener per front, reachable only through that front's shim. Each request names a lease
//! by its proxy password; the listener sends it to a nono that injects the secrets of the
//! lease's user, so a front never holds them. A lease of another front, an expired one or none
//! is refused.

pub(crate) mod lease;
pub(crate) mod run;
mod users;

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use moor_core::config::proxy::ResolvedRule;

use super::front::{Front, FrontSocket, Route};
use super::ledger::FrontsLedger;
use super::nono::ca;
use super::Sink;
use users::Users;

/// How often the Proxy checks its stop flag and its sockets.
const TICK: Duration = Duration::from_millis(200);

/// Common name of the front Proxy's CA.
pub(crate) const CA_NAME: &str = "moor front proxy";

/// What the front Proxy runs with.
pub(crate) struct Deps {
    /// Leases and the secrets of their users, read through the controller on every request.
    pub(crate) ledger: Arc<FrontsLedger>,
    /// The controller's state dir; the CA and each nono's files live under it.
    pub(crate) state_dir: PathBuf,
    /// The uid that must own each front's directory and socket.
    pub(crate) owner: u32,
    /// Extra upstream root PEM; `None` in production.
    pub(crate) upstream_ca: Option<PathBuf>,
    /// Set when a signal asks the Proxy to stop.
    pub(crate) stop: Arc<AtomicBool>,
    /// Where status lines go.
    pub(crate) stderr: Sink,
}

/// `<state>/proxy/fronts`, where the front Proxy's CA lives.
pub(crate) fn ca_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("proxy").join("fronts")
}

/// The CA certificate every front trusts.
///
/// # Errors
/// Why the CA could not be read.
pub(crate) fn ca_cert(state_dir: &Path) -> std::io::Result<PathBuf> {
    let dir = ca_dir(state_dir);
    ca::load_in(&dir)?;
    Ok(ca::cert_in(&dir))
}

/// The socket of front `ns`'s Proxy.
pub(crate) fn socket_path(state_dir: &Path, ns: &str) -> PathBuf {
    FrontSocket::of_front(0, state_dir, ns).path().to_owned()
}

/// Whether front `ns`'s Proxy accepts connections.
pub(crate) fn running(state_dir: &Path, ns: &str) -> bool {
    UnixStream::connect(socket_path(state_dir, ns)).is_ok()
}

/// Serves each of `fronts`, a namespace and the rules its Proxy serves, until the stop flag
/// is set; returns the exit code.
pub(crate) fn serve(deps: &Deps, fronts: &[(String, Vec<ResolvedRule>)]) -> u8 {
    let say = |line: &str| {
        let _ = writeln!(
            deps.stderr
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            "moor front proxy: {line}"
        );
    };
    let ca = match ca::load_in(&ca_dir(&deps.state_dir)) {
        Ok(ca) => Arc::new(ca),
        Err(e) => {
            say(&format!("cannot load the CA: {e}"));
            return 1;
        }
    };
    let users = Arc::new(Users::new(deps, ca));
    let mut listening = Vec::new();
    for (ns, rules) in fronts {
        let socket = FrontSocket::of_front(deps.owner, &deps.state_dir, ns);
        match Front::routed(socket, route(deps, &users, ns, rules), deps.stderr.clone()) {
            Ok(front) => listening.push(front),
            Err(e) => {
                say(&format!("front {ns}: {e}"));
                listening.into_iter().for_each(Front::stop);
                users.stop();
                return 1;
            }
        }
    }
    say(&format!("serving {} fronts", listening.len()));
    while !deps.stop.load(Ordering::Acquire) {
        for front in &listening {
            if let Err(e) = front.rebind() {
                say(&e);
            }
        }
        std::thread::sleep(TICK);
    }
    listening.into_iter().for_each(Front::stop);
    users.stop();
    0
}

/// Routes a request on front `ns`'s listener to the nono of its lease's user.
fn route(deps: &Deps, users: &Arc<Users>, ns: &str, rules: &[ResolvedRule]) -> Route {
    let (ledger, users, ns, rules) = (
        Arc::clone(&deps.ledger),
        Arc::clone(users),
        ns.to_owned(),
        rules.to_vec(),
    );
    Arc::new(move |head| {
        let token = lease::token(head)?;
        let user = ledger.lease_user(&token, &ns).ok()??;
        users.nono(&ns, user, &rules)
    })
}

#[cfg(test)]
mod tests;
