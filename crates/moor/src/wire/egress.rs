//! The shared egress Proxy a controller runs itself when `MOOR_EGRESS_URL` names none.
//!
//! Its key lives in the state dir, made on the first start. The `moor proxy serve-egress` process
//! listens on the state dir's egress socket in a process group of its own, so it outlives the
//! controller: a controller restart leaves the Proxy alone. A controller that finds it answering
//! keeps it. It never gets the database URL. It reads the store through the ledger.

use std::fs::{self, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::config_load::load_user;
use crate::env::{Env, DATABASE_READERS, DATABASE_URL, EGRESS_KEY};
use crate::proxy::route::EgressRoute;
use crate::proxy::token::base64url;

/// The key file, under the state dir.
const KEY_FILE: &str = "egress.key";
/// The Proxy's log, under the state dir.
const LOG_FILE: &str = "egress/serve-egress.log";
/// How long a started Proxy gets to answer.
const START_WAIT: Duration = Duration::from_secs(10);
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
        if !ready(socket) {
            start(&env, &state, socket)?;
        }
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
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(failed(e)),
    }
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| format!("cannot make the egress key: {e}"))?;
    let key = base64url(&bytes);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(failed)?;
    file.write_all(key.as_bytes()).map_err(failed)?;
    Ok(key)
}

/// Whether a Proxy answers its readiness probe on `socket`.
fn ready(socket: &Path) -> bool {
    let Ok(mut conn) = UnixStream::connect(socket) else {
        return false;
    };
    let _ = conn.set_read_timeout(Some(Duration::from_secs(2)));
    let asked = conn.write_all(b"GET /ready HTTP/1.1\r\nHost: egress\r\n\r\n");
    let mut answer = [0_u8; 12];
    asked.is_ok() && conn.read_exact(&mut answer).is_ok() && answer.starts_with(b"HTTP/1.1 200")
}

/// Starts the Proxy on `socket` in a process group of its own and waits for it to answer.
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
        .arg(socket);
    let service = load_user(env).ok().and_then(|c| c.machines.service);
    if let Some(service) = service {
        cmd.arg("--machine-service")
            .arg(format!("{}={}", service.host, service.upstream));
    }
    let mut child = cmd
        .env_remove(DATABASE_URL)
        .env_remove(DATABASE_READERS)
        .env(EGRESS_KEY, env.get(EGRESS_KEY).unwrap_or_default())
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(err)
        .process_group(0)
        .spawn()
        .map_err(|e| format!("cannot start the shared egress Proxy: {e}"))?;
    for _ in 0..START_WAIT.as_millis() / POLL.as_millis() {
        if ready(socket) {
            std::thread::spawn(move || drop(child.wait()));
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
    std::thread::spawn(move || drop(child.wait()));
    Err(format!(
        "the shared egress Proxy did not answer in {}s; see {}",
        START_WAIT.as_secs(),
        log_path.display()
    ))
}

#[cfg(test)]
mod tests;
