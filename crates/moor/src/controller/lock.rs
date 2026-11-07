//! What lets one controller run: the state dir's flock, the one beside its socket, and the
//! store's claim, which only a store shared between hosts needs.

use std::fs::{self, File, TryLockError};
use std::io::Write as _;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rustix::event::{poll, PollFd, PollFlags, Timespec};

use super::{check, log, mkdir, Sink, Stop};
use crate::env::Env;
use crate::Moor;

/// How long a start waits for a held controller lock before it reports a running controller.
/// A store migration holds the lock briefly, and a child forked from any thread keeps a copy
/// of a lock's fd until it execs; only a running controller holds it longer.
pub(super) const LOCK_WAIT: Duration = Duration::from_secs(5);

/// How often a start tries a held lock again within [`LOCK_WAIT`].
const LOCK_POLL: Duration = Duration::from_millis(20);

/// Waits for the right to run on `env`'s store, which lasts until the process exits. `wake`
/// turning readable ends the wait. Losing the right later ends the process at once, as another
/// controller may already be running.
pub(super) fn claim(env: &Env, wake: &UnixStream, sink: &Sink) -> Result<(), Stop> {
    let stopped = |wait: Duration| turned_readable(wake, wait);
    let note = |line: &str| log(sink, line);
    let leaving = Arc::clone(sink);
    #[expect(
        clippy::exit,
        reason = "work on other threads must end now, and no drain can wait for it"
    )]
    let lost = Arc::new(move |why: &str| {
        log(&leaving, &format!("exiting: {why}"));
        std::process::exit(1);
    });
    let claimed = Moor::claim_controller(env, &stopped, &note, lost);
    if claimed.map_err(|e| check::store_refused(&e))? {
        return Ok(());
    }
    Err(Stop {
        code: 0,
        message: "told to stop while waiting for the controller lock".to_owned(),
    })
}

/// Whether `wake` turns readable within `wait`.
fn turned_readable(wake: &UnixStream, wait: Duration) -> bool {
    let mut fds = [PollFd::new(wake, PollFlags::IN)];
    let wait = Timespec::try_from(wait).ok();
    matches!(poll(&mut fds, wait.as_ref()), Ok(n) if n > 0)
}

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
