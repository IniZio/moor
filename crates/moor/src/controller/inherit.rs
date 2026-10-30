//! Listening sockets a service manager passed in by the `sd_listen_fds` protocol: `LISTEN_FDS`
//! fds from 3 up, named in order by `LISTEN_FDNAMES`, meant for the process `LISTEN_PID`.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::unix::fs::{FileTypeExt as _, PermissionsExt as _};
use std::os::unix::net::UnixListener;
use std::path::Path;

use crate::env::Env;

/// The first passed fd.
const FIRST_FD: i32 = 3;

/// Where the controller's listening sockets come from.
pub(super) enum Sockets {
    /// Nothing was passed to this process: it binds each socket itself.
    Bind,
    /// The sockets passed in, by name, not yet claimed.
    Passed(BTreeMap<String, UnixListener>),
}

impl Sockets {
    /// Takes the sockets passed to this process, each made close-on-exec.
    ///
    /// As `sd_listen_fds` does, an absent `LISTEN_PID` or `LISTEN_FDS`, or a `LISTEN_PID`
    /// naming another process, means nothing was passed here, and the fds are left alone.
    pub(super) fn from_env(env: &Env) -> Result<Self, String> {
        let (Some(pid), Some(count)) = (env.get("LISTEN_PID"), env.get("LISTEN_FDS")) else {
            return Ok(Self::Bind);
        };
        let pid: u32 = pid
            .parse()
            .map_err(|_| format!("LISTEN_PID is not a pid: {pid:?}"))?;
        if pid != std::process::id() {
            return Ok(Self::Bind);
        }
        let count: u16 = count
            .parse()
            .map_err(|_| format!("LISTEN_FDS is not a count: {count:?}"))?;
        if count == 0 {
            return Ok(Self::Bind);
        }
        let names: Vec<&str> = env
            .get("LISTEN_FDNAMES")
            .map_or_else(Vec::new, |n| n.split(':').collect());
        if names.len() != usize::from(count) {
            return Err(format!(
                "LISTEN_FDNAMES names {} sockets but LISTEN_FDS passes {count}",
                names.len()
            ));
        }
        let mut passed = BTreeMap::new();
        for (fd, name) in (FIRST_FD..).zip(names) {
            let listener = adopt(fd).map_err(|e| format!("cannot take passed fd {fd}: {e}"))?;
            if passed.insert(name.to_owned(), listener).is_some() {
                return Err(format!("socket {name} is passed twice"));
            }
        }
        Ok(Self::Passed(passed))
    }

    /// The listener for `path`: the passed socket `name`, or a fresh bind when nothing was
    /// passed. A passed socket is refused unless it is a listening stream socket bound at
    /// `path` that only its owner can connect to.
    pub(super) fn listen(&mut self, name: &str, path: &Path) -> Result<UnixListener, String> {
        let passed = match self {
            Self::Bind => {
                return super::bind(path)
                    .map_err(|e| format!("cannot bind {}: {e}", path.display()))
            }
            Self::Passed(passed) => passed,
        };
        let listener = passed.remove(name).ok_or_else(|| {
            format!(
                "no socket named {name} was passed in for {}",
                path.display()
            )
        })?;
        check(&listener, path).map_err(|e| format!("passed socket {name}: {e}"))?;
        Ok(listener)
    }

    /// [`Sockets::listen`] for a socket a service manager may not know of: a fresh bind when
    /// `name` was not passed.
    pub(super) fn listen_if_passed(
        &mut self,
        name: &str,
        path: &Path,
    ) -> Result<UnixListener, String> {
        match self {
            Self::Passed(passed) if passed.contains_key(name) => self.listen(name, path),
            _ => super::bind(path).map_err(|e| format!("cannot bind {}: {e}", path.display())),
        }
    }

    /// Refuses any passed socket no [`Sockets::listen`] claimed.
    pub(super) fn finish(self) -> Result<(), String> {
        match self {
            Self::Passed(left) if !left.is_empty() => Err(format!(
                "passed sockets this config does not use: {}",
                left.into_keys().collect::<Vec<_>>().join(", ")
            )),
            _ => Ok(()),
        }
    }
}

/// Checks `listener` is a listening stream socket bound at `path`, owner-only.
fn check(listener: &UnixListener, path: &Path) -> Result<(), String> {
    if !listening_stream(listener)? {
        return Err("not a listening stream socket".into());
    }
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let bound = addr.as_pathname().ok_or("not bound to a path")?;
    let same = |p: &Path| fs::canonicalize(p).map_err(|e| format!("{}: {e}", p.display()));
    if same(bound)? != same(path)? {
        return Err(format!(
            "bound at {}, but this config expects {}",
            bound.display(),
            path.display()
        ));
    }
    let meta = fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.file_type().is_socket() {
        return Err(format!("{} is not a socket", path.display()));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        return Err(format!(
            "{} is open to other users; set SocketMode=0600",
            path.display()
        ));
    }
    Ok(())
}

/// Whether `listener` is a stream socket that accepts connections.
#[cfg(target_os = "linux")]
fn listening_stream(listener: &UnixListener) -> Result<bool, String> {
    use rustix::net::sockopt::{socket_acceptconn, socket_type};
    let stream =
        socket_type(listener).map_err(|e| e.to_string())? == rustix::net::SocketType::STREAM;
    Ok(stream && socket_acceptconn(listener).map_err(|e| e.to_string())?)
}

/// Passed sockets are taken only on Linux, so none is checked elsewhere.
#[cfg(not(target_os = "linux"))]
fn listening_stream(_listener: &UnixListener) -> Result<bool, String> {
    Err("passed sockets are taken only on Linux".into())
}

/// Takes ownership of the passed fd number `fd`: a close-on-exec duplicate replaces it, so
/// Box processes and fronts never inherit it. `pidfd_getfd` on this process turns the bare
/// number into an owned fd without unsafe code.
#[cfg(target_os = "linux")]
fn adopt(fd: i32) -> io::Result<UnixListener> {
    use rustix::process::{getpid, pidfd_getfd, pidfd_open, PidfdFlags, PidfdGetfdFlags};
    let me = pidfd_open(getpid(), PidfdFlags::empty())?;
    let owned = pidfd_getfd(&me, fd, PidfdGetfdFlags::empty())?;
    nix::unistd::close(fd)?;
    Ok(UnixListener::from(owned))
}

/// Passed sockets are taken only on Linux.
#[cfg(not(target_os = "linux"))]
fn adopt(_fd: i32) -> io::Result<UnixListener> {
    Err(io::ErrorKind::Unsupported.into())
}
