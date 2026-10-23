//! The loopback port a front reaches its Proxy on. pid 1 opens the directory of the Proxy's Unix
//! socket and binds the port before it enters the front's root and sandbox, which show neither
//! the socket nor let it bind; it then splices each connection to the socket, looked up by name
//! in that open directory each time, so a restarted Proxy's new socket is the one it reaches. It
//! parses nothing: the Proxy checks every request.

use std::ffi::OsString;
use std::io;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::fd::{AsRawFd as _, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

use rustix::fs::{Mode, OFlags};

/// The variable that tells the front its Proxy's port.
pub(super) const PORT_VAR: &str = "MOOR_FRONT_PROXY_PORT";

/// A bound loopback port and the Proxy socket it leads to.
pub(super) struct Bridge {
    /// The port's listener.
    listener: TcpListener,
    /// The Proxy socket's path through its directory, opened with `O_PATH`, so it stays
    /// reachable inside the root.
    proxy: Arc<Socket>,
}

/// A socket's name in an open directory.
struct Socket {
    /// The directory, held open.
    dir: OwnedFd,
    /// The socket's name in it.
    name: OsString,
}

impl Socket {
    /// A path that reaches the socket through the open directory.
    fn path(&self) -> PathBuf {
        Path::new(&format!("/proc/self/fd/{}", self.dir.as_raw_fd())).join(&self.name)
    }
}

impl Bridge {
    /// Opens `proxy` and binds a free loopback port; nothing is served yet.
    pub(super) fn open(proxy: &Path) -> Result<Self, String> {
        let (Some(dir), Some(name)) = (proxy.parent(), proxy.file_name()) else {
            return Err(format!("{} is not a socket path", proxy.display()));
        };
        let dir = rustix::fs::open(
            dir,
            OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| format!("cannot open the front Proxy dir {}: {e}", dir.display()))?;
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|e| format!("cannot listen for the front Proxy: {e}"))?;
        Ok(Self {
            listener,
            proxy: Arc::new(Socket {
                dir,
                name: name.to_owned(),
            }),
        })
    }

    /// Serves the port on a thread; returns it.
    pub(super) fn serve(self) -> Result<u16, String> {
        let port = self
            .listener
            .local_addr()
            .map_err(|e| format!("cannot read the front Proxy port: {e}"))?
            .port();
        thread::Builder::new()
            .name("moor-front-bridge".to_owned())
            .spawn(move || accept(&self.listener, &self.proxy))
            .map_err(|e| format!("cannot start the front Proxy bridge: {e}"))?;
        Ok(port)
    }
}

/// Splices every connection to `proxy`, each on its own threads.
fn accept(listener: &TcpListener, proxy: &Arc<Socket>) {
    for client in listener.incoming().flatten() {
        let proxy = Arc::clone(proxy);
        let _ = thread::Builder::new()
            .name("moor-front-bridge-conn".to_owned())
            .spawn(move || splice(client, &proxy));
    }
}

/// Copies both ways between `client` and a new connection to `proxy`; a Proxy that is down
/// closes the client at once.
fn splice(client: TcpStream, proxy: &Socket) {
    let Ok(upstream) = UnixStream::connect(proxy.path()) else {
        let _ = client.shutdown(Shutdown::Both);
        return;
    };
    let (Ok(mut client_rx), Ok(mut upstream_tx)) = (client.try_clone(), upstream.try_clone())
    else {
        return;
    };
    let up = thread::spawn(move || {
        let _ = io::copy(&mut client_rx, &mut upstream_tx);
        let _ = upstream_tx.shutdown(Shutdown::Write);
    });
    let (mut client, mut upstream) = (client, upstream);
    let _ = io::copy(&mut upstream, &mut client);
    let _ = client.shutdown(Shutdown::Write);
    let _ = up.join();
}
