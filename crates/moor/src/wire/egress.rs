//! The shared egress Proxy a controller runs itself when `MOOR_EGRESS_URL` names none.
//!
//! Its key lives in the state dir, made on the first start. The `moor proxy serve-egress` process
//! listens on the state dir's egress socket in a process group of its own, and reads a pipe the
//! controller holds open for as long as it runs: when the controller exits, normally or by any
//! signal, the pipe closes and the Proxy drains and exits. A controller never keeps a Proxy it did
//! not start: whatever answers on the socket, even one still draining after the last controller
//! exited, is stopped first, and the controller starts its own. It never gets the database URL.
//! It reads the store through the ledger.

use std::fs::{self, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::config_load::load_user;
use crate::env::{
    Env, BLOBS_ACCESS_KEY_ID, BLOBS_SECRET_ACCESS_KEY, DATABASE_READERS, DATABASE_URL, EGRESS_KEY,
};
use crate::proxy::route::EgressRoute;
use crate::proxy::token::{base64url, key_id};

/// The key file, under the state dir.
const KEY_FILE: &str = "egress.key";
/// The Proxy's log, under the state dir.
const LOG_FILE: &str = "egress/serve-egress.log";
/// How long a started Proxy gets to answer, and a replaced one to stop listening.
const START_WAIT: Duration = Duration::from_secs(10);
/// How long a running Proxy gets to answer its readiness probe.
const PROBE_WAIT: Duration = Duration::from_secs(10);
/// Pause between looks at a starting Proxy.
const POLL: Duration = Duration::from_millis(100);

/// `env` with the egress key, and the controller's own shared Proxy running unless
/// `MOOR_EGRESS_URL` names another.
///
/// # Errors
/// The key cannot be read or made, or the Proxy did not start.
pub(crate) fn prepare_egress(env: Env) -> Result<Env, String> {
    let route = EgressRoute::from_env(&env)?;
    let Some(state) = env.state_dir() else {
        return Ok(env);
    };
    let env = match env.get(EGRESS_KEY).filter(|k| !k.is_empty()) {
        Some(_) => env,
        None if matches!(route, EgressRoute::Direct { .. }) => {
            return Err(format!(
                "MOOR_EGRESS_URL needs {EGRESS_KEY}, the key its Proxy was given"
            ));
        }
        None => {
            let key = key(&state)?;
            env.with_egress_key(&key)
        }
    };
    if let Some(socket) = route.socket() {
        replace(socket)?;
        start(&env, &state, socket)?;
    }
    Ok(env)
}

/// The key in the state dir, made first when there is none.
fn key(state: &Path) -> Result<String, String> {
    let path = state.join(KEY_FILE);
    let failed =
        |e: std::io::Error| format!("cannot keep the egress key in {}: {e}", path.display());
    match fs::read_to_string(&path) {
        Ok(key) if !key.trim().is_empty() => return Ok(key.trim().to_owned()),
        Ok(_) => fs::remove_file(&path).map_err(failed)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(failed(e)),
    }
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| format!("cannot make the egress key: {e}"))?;
    let key = base64url(&bytes);
    // Written whole, then linked into place: a crash mid-write leaves no empty key file, and a
    // link fails rather than replace the key another process linked first.
    let draft = state.join(format!("{KEY_FILE}.{}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&draft)
        .map_err(failed)?;
    file.write_all(key.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(failed)?;
    drop(file);
    let linked = fs::hard_link(&draft, &path);
    drop(fs::remove_file(&draft));
    match linked {
        Ok(()) => Ok(key),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let theirs = fs::read_to_string(&path).map_err(failed)?;
            Ok(theirs.trim().to_owned())
        }
        Err(e) => Err(failed(e)),
    }
}

/// The key id whatever listens on `socket` names when it answers ready; `None` when nothing
/// does.
fn probe(socket: &Path) -> Option<i64> {
    let mut conn = UnixStream::connect(socket).ok()?;
    let _ = conn.set_read_timeout(Some(PROBE_WAIT));
    let mut answer = String::new();
    let asked = conn
        .write_all(b"GET /ready HTTP/1.1\r\nHost: egress\r\n\r\n")
        .and_then(|()| conn.read_to_string(&mut answer).map(drop));
    asked
        .ok()
        .filter(|()| answer.starts_with("HTTP/1.1 200"))
        .and_then(|()| answer.split_once("\r\n\r\n"))
        .and_then(|(_, body)| body.trim().parse().ok())
}

/// Stops the process listening on `socket`, if any, found by the socket's peer credentials, and
/// waits for it to stop listening; it drains its open tunnels on its own.
fn replace(socket: &Path) -> Result<(), String> {
    let failed = |e: &dyn std::fmt::Display| {
        format!(
            "cannot replace the shared egress Proxy on {}: {e}",
            socket.display()
        )
    };
    let Ok(conn) = UnixStream::connect(socket) else {
        return Ok(());
    };
    let pid = peer_pid(&conn).map_err(|e| failed(&e))?;
    drop(conn);
    let pid = rustix::process::Pid::from_raw(pid)
        .ok_or_else(|| failed(&"its pid is not in this pid namespace"))?;
    match rustix::process::kill_process(pid, rustix::process::Signal::TERM) {
        // It exited on its own since it answered.
        Ok(()) | Err(rustix::io::Errno::SRCH) => {}
        Err(e) => return Err(failed(&e)),
    }
    for _ in 0..START_WAIT.as_millis() / POLL.as_millis() {
        if UnixStream::connect(socket).is_err() {
            return Ok(());
        }
        std::thread::sleep(POLL);
    }
    Err(failed(&"it still listens after SIGTERM"))
}

/// The pid of the process at the other end of `conn`.
#[cfg(target_os = "linux")]
fn peer_pid(conn: &UnixStream) -> std::io::Result<i32> {
    use nix::sys::socket::{getsockopt, sockopt::PeerCredentials};
    Ok(getsockopt(conn, PeerCredentials)?.pid())
}

/// The pid of the process at the other end of `conn`.
#[cfg(target_os = "macos")]
fn peer_pid(conn: &UnixStream) -> std::io::Result<i32> {
    use nix::sys::socket::{getsockopt, sockopt::LocalPeerPid};
    Ok(getsockopt(conn, LocalPeerPid)?)
}

/// Starts the Proxy on `socket` in a process group of its own, tied to this process by its
/// stdin, and waits for it to answer.
fn start(env: &Env, state: &Path, socket: &Path) -> Result<(), String> {
    let log_path = state.join(LOG_FILE);
    if let Some(dir) = log_path.parent() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    }
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&log_path)
        .map_err(|e| format!("cannot open {}: {e}", log_path.display()))?;
    let err = log
        .try_clone()
        .map_err(|e| format!("cannot open {}: {e}", log_path.display()))?;
    let exe = std::env::current_exe().map_err(|e| format!("cannot find moor: {e}"))?;
    let mut cmd = Command::new(exe);
    cmd.args(["proxy", "serve-egress", "--state-dir"])
        .arg(state)
        .arg("--listen")
        .arg(socket)
        .arg("--until-stdin-closes");
    let service = load_user(env).ok().and_then(|c| c.machines.service);
    if let Some(service) = service {
        cmd.arg("--machine-service")
            .arg(format!("{}={}", service.host, service.upstream));
    }
    let child = cmd
        .env_remove(DATABASE_URL)
        .env_remove(BLOBS_ACCESS_KEY_ID)
        .env_remove(BLOBS_SECRET_ACCESS_KEY)
        .env_remove(DATABASE_READERS)
        .env(EGRESS_KEY, env.get(EGRESS_KEY).unwrap_or_default())
        .stdin(Stdio::piped())
        .stdout(log)
        .stderr(err)
        .process_group(0)
        .spawn()
        .map_err(|e| format!("cannot start the shared egress Proxy: {e}"))?;
    let want = key_id(env.get(EGRESS_KEY).unwrap_or_default().as_bytes());
    wait_ready(child, socket, want, &log_path)
}

/// Waits for the started Proxy `child` to answer on `socket` under the key `want`.
fn wait_ready(
    mut child: std::process::Child,
    socket: &Path,
    want: i64,
    log_path: &Path,
) -> Result<(), String> {
    let tie = child.stdin.take();
    let reap = move |mut child: std::process::Child| {
        std::thread::spawn(move || {
            let _tie = tie;
            drop(child.wait());
        });
    };
    for _ in 0..START_WAIT.as_millis() / POLL.as_millis() {
        if probe(socket) == Some(want) {
            reap(child);
            return Ok(());
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!(
                "the shared egress Proxy exited ({status}); see {}",
                log_path.display()
            ));
        }
        std::thread::sleep(POLL);
    }
    reap(child);
    Err(format!(
        "the shared egress Proxy did not answer in {}s; see {}",
        START_WAIT.as_secs(),
        log_path.display()
    ))
}

#[cfg(test)]
mod tests;
