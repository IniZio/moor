//! The Unix socket a front listens on: one per Box, in moor's private socket directory.
//!
//! Only moor's user can enter that directory, and the socket itself is 0600, so no other host
//! user can open it. The Box reaches it only through its reverse tunnel, which opens the path
//! once per connection, so a socket bound again at the same path serves the running tunnel.

use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use moor_core::BoxId;
use rustix::event::{poll, PollFd, PollFlags, Timespec};

use crate::adapters::sockets::{private_dir, short_hash};

/// Longest wait for a connection before the acceptor checks its stop flag again.
const POLL: Timespec = Timespec {
    tv_sec: 0,
    tv_nsec: 100_000_000,
};

/// Runs on its own thread for each accepted connection.
pub(super) type Handler = Arc<dyn Fn(UnixStream) + Send + Sync>;

/// Where the front of one Box listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrontSocket {
    /// moor's private socket directory.
    dir: PathBuf,
    /// The uid that must own `dir`: the uid moor runs as.
    owner: u32,
    /// `px-<hash>` inside `dir`.
    path: PathBuf,
}

/// The file a bind created, by device and inode number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Inode {
    /// Device number.
    dev: u64,
    /// Inode number.
    ino: u64,
}

/// A bound socket and the thread accepting on it.
pub(super) struct Listening {
    /// The file the bind created.
    inode: Inode,
    /// Set to ask the acceptor to exit.
    stop: Arc<AtomicBool>,
    /// The acceptor thread.
    acceptor: JoinHandle<()>,
}

impl FrontSocket {
    /// The socket of `box_id` in `dir`. The name also hashes `state_dir`, so two state dirs
    /// never share a socket, and it is the same for every supervisor of the Box.
    pub(crate) fn new(dir: &Path, owner: u32, state_dir: &Path, box_id: BoxId) -> Self {
        let id = box_id.get().to_string();
        let hash = short_hash(&[state_dir.as_os_str().as_bytes(), id.as_bytes()]);
        Self {
            dir: dir.to_path_buf(),
            owner,
            path: dir.join(format!("px-{hash}")),
        }
    }

    /// The socket path.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Checks the directory, binds the socket with mode 0600 and starts accepting into
    /// `handler`. With `replace`, a file left at the path is removed first.
    pub(super) fn listen(&self, replace: bool, handler: &Handler) -> Result<Listening, String> {
        private_dir(&self.dir, self.owner)?;
        let fail = |e: io::Error| format!("cannot listen on {}: {e}", self.path.display());
        if replace {
            match fs::remove_file(&self.path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(fail(e)),
                _ => {}
            }
        }
        let listener = UnixListener::bind(&self.path).map_err(fail)?;
        fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600)).map_err(fail)?;
        let inode = self.inode().map_err(fail)?;
        listener.set_nonblocking(true).map_err(fail)?;
        let stop = Arc::new(AtomicBool::new(false));
        let (flag, handler) = (Arc::clone(&stop), Arc::clone(handler));
        let acceptor = thread::Builder::new()
            .name("moor-front".to_owned())
            .spawn(move || accept_loop(&listener, &flag, &handler))
            .map_err(fail)?;
        Ok(Listening {
            inode,
            stop,
            acceptor,
        })
    }

    /// Whether no file is left at the path.
    pub(super) fn gone(&self) -> bool {
        matches!(fs::symlink_metadata(&self.path), Err(e) if e.kind() == io::ErrorKind::NotFound)
    }

    /// Stops `listening` and removes the socket file if it is still the one that bind
    /// created, so a supervisor that lost its lease never removes its successor's socket.
    /// The path is compared while the listener still holds its inode, so a freed inode number
    /// that a new file reuses cannot match.
    pub(super) fn close(&self, listening: Listening) {
        let ours = self.inode().is_ok_and(|now| now == listening.inode);
        listening.halt();
        if ours {
            let _ = fs::remove_file(&self.path);
        }
    }

    /// The device and inode number at the path.
    fn inode(&self) -> io::Result<Inode> {
        let meta = fs::symlink_metadata(&self.path)?;
        Ok(Inode {
            dev: meta.dev(),
            ino: meta.ino(),
        })
    }
}

impl Listening {
    /// Stops accepting and joins the acceptor. Open connections end on their socket timeouts
    /// or when a peer closes.
    pub(super) fn halt(self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.acceptor.join();
    }
}

/// Accepts connections until `stop` is set, one thread per connection.
fn accept_loop(listener: &UnixListener, stop: &AtomicBool, handler: &Handler) {
    while !stop.load(Ordering::SeqCst) {
        let mut fds = [PollFd::new(listener, PollFlags::IN)];
        match poll(&mut fds, Some(&POLL)) {
            Ok(0) => continue,
            Ok(_) => {}
            Err(_) => {
                thread::sleep(Duration::from_millis(10));
                continue;
            }
        }
        match listener.accept() {
            // An accepted socket inherits non-blocking mode on macOS.
            Ok((client, _)) if client.set_nonblocking(false).is_ok() => {
                let handler = Arc::clone(handler);
                let _ = thread::Builder::new()
                    .name("moor-front-conn".to_owned())
                    .spawn(move || handler(client));
            }
            Ok(_) | Err(_) => thread::sleep(Duration::from_millis(10)),
        }
    }
}
