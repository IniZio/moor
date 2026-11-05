//! The controller's flock files: the state dir's lock and the one beside its socket.

use std::fs::{self, File, TryLockError};
use std::io::Write as _;
use std::path::Path;
use std::time::Duration;

use super::{mkdir, Stop};

/// How long a start waits for a held controller lock before it reports a running controller.
/// A store migration holds the lock briefly, and a child forked from any thread keeps a copy
/// of a lock's fd until it execs; only a running controller holds it longer.
pub(super) const LOCK_WAIT: Duration = Duration::from_secs(5);

/// How often a start tries a held lock again within [`LOCK_WAIT`].
const LOCK_POLL: Duration = Duration::from_millis(20);

/// Takes the flock beside `socket`, `<socket>.lock`.
pub(super) fn take_socket_lock(socket: &Path) -> Result<File, Stop> {
    let mut path = socket.as_os_str().to_owned();
    path.push(".lock");
    take_lock(Path::new(&path), socket, LOCK_WAIT)
}

/// Takes the flock on `path` within `wait` and records our pid and socket in it.
///
/// A lock still held after `wait` is a running controller, a refusal named by the pid and
/// socket its holder recorded.
pub(super) fn take_lock(path: &Path, socket: &Path, wait: Duration) -> Result<File, Stop> {
    let failed = |e: std::io::Error| Stop {
        code: 1,
        message: format!("cannot lock {}: {e}", path.display()),
    };
    if let Some(dir) = path.parent() {
        mkdir(dir).map_err(failed)?;
    }
    let mut file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .read(true)
        .open(path)
        .map_err(failed)?;
    let mut retries = wait.as_millis() / LOCK_POLL.as_millis();
    loop {
        match file.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) if retries > 0 => {
                retries -= 1;
                std::thread::sleep(LOCK_POLL);
            }
            Err(TryLockError::WouldBlock) => return Err(held_by(path, socket)),
            Err(TryLockError::Error(e)) => return Err(failed(e)),
        }
    }
    file.set_len(0)
        .and_then(|()| writeln!(file, "{}\n{}", std::process::id(), socket.display()))
        .map_err(failed)?;
    Ok(file)
}

/// The refusal for a controller lock at `path` that stayed held.
fn held_by(path: &Path, socket: &Path) -> Stop {
    let held = fs::read_to_string(path).unwrap_or_default();
    let mut lines = held.lines();
    let message = lines.next().map_or_else(
        || format!("already running: {} is held", path.display()),
        |pid| {
            let on = lines
                .next()
                .map_or_else(|| socket.display().to_string(), str::to_owned);
            format!("already running (pid {pid}) on {on}")
        },
    );
    Stop { code: 1, message }
}
