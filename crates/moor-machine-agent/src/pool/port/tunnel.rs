//! One running `crabbox tunnel`: starting it, signalling it and asking through it.

use std::net::SocketAddr;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::sync::{Arc, Mutex as Lock, PoisonError};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};

use super::super::{box_dir, Pool, CACHE_CAP, TAIL};
use super::{PROBE, READY, TERM_GRACE};

/// A running `crabbox tunnel`.
pub(super) struct Tunnel {
    /// The crabbox process.
    pub(super) child: Child,
    /// Where it listens.
    pub(super) addr: SocketAddr,
    /// Connections in flight.
    pub(super) conns: Arc<AtomicUsize>,
    /// Counts each connection start and end, so an idle timer knows nothing happened since.
    pub(super) epoch: Arc<AtomicU64>,
}

impl Tunnel {
    /// Whether crabbox has exited.
    pub(super) fn exited(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_some()
    }
}

/// Starts `crabbox tunnel` to `port` of lease `slug`, once its listener accepts.
pub(super) async fn start(
    home: &std::path::Path,
    pool: &Pool,
    slug: &str,
    port: u16,
) -> Result<Tunnel, String> {
    let ceiling = box_dir(home, slug);
    let dir = ceiling.join("lease");
    if !dir.is_dir() {
        return Err(format!("this host holds no lease {slug}"));
    }
    let mut cmd = Command::new(&pool.crabbox);
    cmd.args(["tunnel", "--provider", &pool.provider, "--id", slug])
        .args(["--local-port", "0"])
        .arg(port.to_string())
        .current_dir(&dir)
        .env("HOME", home)
        .env("GIT_CEILING_DIRECTORIES", &ceiling)
        .envs(pool.cache_gb.map(|gb| (CACHE_CAP, gb.to_string())))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cannot run crabbox: {e}"))?;
    let stderr = Arc::new(Lock::new(String::new()));
    if let Some(err) = child.stderr.take() {
        tokio::spawn(tail_of(err, Arc::clone(&stderr)));
    }
    let Some(out) = child.stdout.take() else {
        return Err("crabbox gave no output".to_owned());
    };
    let mut lines = BufReader::new(out);
    let Some(addr) = listener(&mut lines).await else {
        let code = match tokio::time::timeout(Duration::from_millis(500), child.wait()).await {
            Ok(Ok(status)) => status.code(),
            _ => None,
        };
        end_child(&mut child).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        let said = stderr.lock().unwrap_or_else(PoisonError::into_inner);
        return Err(failure(&said, code));
    };
    tokio::spawn(async move {
        let mut sink = Vec::new();
        let _ = lines.read_to_end(&mut sink).await;
    });
    Ok(Tunnel {
        child,
        addr,
        conns: Arc::new(AtomicUsize::new(0)),
        epoch: Arc::new(AtomicU64::new(0)),
    })
}

/// The address on the `http://` line crabbox prints once its listener accepts.
async fn listener(lines: &mut BufReader<tokio::process::ChildStdout>) -> Option<SocketAddr> {
    let mut line = String::new();
    match tokio::time::timeout(READY, lines.read_line(&mut line)).await {
        Ok(Ok(n)) if n > 0 => line
            .trim()
            .strip_prefix("http://")
            .and_then(|a| a.parse().ok()),
        _ => None,
    }
}

/// What a tunnel that ended before it listened says: crabbox's stderr, or its exit code.
fn failure(said: &str, code: Option<i32>) -> String {
    let said = said.trim();
    if said.is_empty() {
        format!("crabbox tunnel ended ({code:?}) before it listened")
    } else {
        said.to_owned()
    }
}

/// Keeps the last [`TAIL`] bytes `from` writes in `into`.
async fn tail_of(mut from: impl AsyncRead + Unpin, into: Arc<Lock<String>>) {
    let mut buf = [0u8; 1024];
    while let Ok(n @ 1..) = from.read(&mut buf).await {
        let chunk = String::from_utf8_lossy(buf.get(..n).unwrap_or_default()).into_owned();
        let mut said = into.lock().unwrap_or_else(PoisonError::into_inner);
        said.push_str(&chunk);
        let mut cut = said.len().saturating_sub(TAIL);
        while !said.is_char_boundary(cut) {
            cut += 1;
        }
        said.drain(..cut);
    }
}

/// Stops `tunnel`.
pub(super) async fn end(mut tunnel: Tunnel) {
    end_child(&mut tunnel.child).await;
}

/// Asks crabbox to stop with SIGTERM, which ends its ssh too, and kills it past the grace.
async fn end_child(child: &mut Child) {
    term(child);
    if tokio::time::timeout(TERM_GRACE, child.wait())
        .await
        .is_err()
    {
        let _ = child.kill().await;
    }
}

/// Sends SIGTERM to `child`'s crabbox, which ends its ssh.
pub(super) fn term(child: &Child) {
    if let Some(pid) = child
        .id()
        .and_then(|p| rustix::process::Pid::from_raw(p.cast_signed()))
    {
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
    }
}

/// Whether anything listens in the VM behind `addr`: ssh closes a connection at once when the
/// VM refuses it.
pub(super) async fn listens(addr: SocketAddr) -> bool {
    let Ok(mut conn) = TcpStream::connect(addr).await else {
        return false;
    };
    let mut byte = [0u8; 1];
    !matches!(
        tokio::time::timeout(PROBE, conn.read(&mut byte)).await,
        Ok(Ok(0) | Err(_))
    )
}
