//! Forwards the user's interrupt of a `run` to the controller: the first SIGINT shuts the
//! sending side of the connection, which the controller reads as a cancel, and the reply
//! still arrives; a second SIGINT exits 130 at once.

use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use signal_hook::consts::SIGINT;

/// Exit code of a second interrupt, as for a run that was interrupted.
const INTERRUPTED: i32 = 130;

/// How often the forwarder looks for an interrupt.
const POLL: Duration = Duration::from_millis(20);

/// Forwards interrupts until dropped.
pub(super) struct Forward {
    /// Set when the exchange is over.
    done: Arc<AtomicBool>,
}

impl Drop for Forward {
    fn drop(&mut self) {
        self.done.store(true, Ordering::SeqCst);
    }
}

/// Starts forwarding interrupts to the controller at the other end of `stream`; none when the
/// handlers cannot be installed, and then an interrupt ends the client as before.
pub(super) fn forward(stream: &UnixStream) -> Option<Forward> {
    let interrupted = Arc::new(AtomicBool::new(false));
    // The shutdown handler goes first, so it fires only on an interrupt after the first.
    signal_hook::flag::register_conditional_shutdown(SIGINT, INTERRUPTED, Arc::clone(&interrupted))
        .ok()?;
    signal_hook::flag::register(SIGINT, Arc::clone(&interrupted)).ok()?;
    let up = stream.try_clone().ok()?;
    let done = Arc::new(AtomicBool::new(false));
    let over = Arc::clone(&done);
    thread::spawn(move || {
        while !over.load(Ordering::SeqCst) {
            if interrupted.load(Ordering::SeqCst) {
                let _ = up.shutdown(Shutdown::Write);
                return;
            }
            thread::sleep(POLL);
        }
    });
    Some(Forward { done })
}
