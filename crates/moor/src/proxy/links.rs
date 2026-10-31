//! The controller reaches its users' machines through the machine runner's links broker,
//! `<state>/runner/links.sock`, and dials a pool lease's VM with `moor link-dial`.

use std::future::Future;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use moor_client::link::broker;
use moor_client::link::lease::{Answer, Verb};
use moor_client::link::status::Status;
use moor_core::boxscript::quote;

use crate::runner_dir;

/// The runner's links broker under one state dir.
pub(crate) struct RunnerLinks {
    /// The state dir `moor link-dial` reads.
    state_dir: PathBuf,
    /// `<state>/runner/links.sock`.
    socket: PathBuf,
    /// This moor binary, which `link-dial` runs as.
    moor: PathBuf,
}

impl RunnerLinks {
    /// The broker of the runner under `state_dir`.
    pub(crate) fn under(state_dir: &Path) -> Self {
        Self {
            state_dir: state_dir.to_owned(),
            socket: runner_dir::of(state_dir).join(broker::SOCKET),
            moor: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("moor")),
        }
    }

    /// Asks machine `machine_id`'s agent to run `verb` on lease `slug`, installing `key` for
    /// an authorize.
    ///
    /// # Errors
    /// The broker or the agent's failure.
    pub(crate) fn lease(
        &self,
        machine_id: i64,
        verb: Verb,
        slug: Option<&str>,
        key: Option<&str>,
    ) -> Result<Answer, String> {
        match (verb, slug, key) {
            (Verb::Authorize, Some(slug), Some(key)) => {
                blocking(broker::authorize(&self.socket, machine_id, slug, key))
            }
            (Verb::Authorize, ..) => Err("an authorize needs a lease and a key".to_owned()),
            _ => blocking(broker::lease(&self.socket, machine_id, verb, slug)),
        }
    }

    /// Machine `machine_id`'s agent status.
    ///
    /// # Errors
    /// The broker or the agent's failure.
    pub(crate) fn status(&self, machine_id: i64) -> Result<Status, String> {
        blocking(broker::status(&self.socket, machine_id))
    }

    /// A blocking stream to the sshd of lease `slug` on machine `machine_id`.
    ///
    /// # Errors
    /// The broker or the agent's failure.
    pub(crate) fn dial(&self, machine_id: i64, slug: &str) -> Result<UnixStream, String> {
        let conn = blocking(async {
            let conn = broker::dial(&self.socket, machine_id, slug).await?;
            conn.into_std().map_err(|e| e.to_string())
        })?;
        conn.set_nonblocking(false).map_err(|e| e.to_string())?;
        Ok(conn)
    }

    /// The ssh `ProxyCommand` that reaches lease `slug`'s VM on machine `machine_id`.
    pub(crate) fn dial_command(&self, machine_id: i64, slug: &str) -> String {
        format!(
            "env MOOR_STATE_DIR={} {} link-dial {machine_id} {slug}",
            word(&self.state_dir),
            word(&self.moor)
        )
    }
}

/// Runs `call` to completion on a runtime of its own, off any runtime the caller is on.
fn blocking<T: Send>(call: impl Future<Output = Result<T, String>> + Send) -> Result<T, String> {
    std::thread::scope(|s| {
        s.spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?
                .block_on(call)
        })
        .join()
        .unwrap_or_else(|_| Err("the links broker call panicked".to_owned()))
    })
}

/// `s` as one shell word that ssh's `ProxyCommand` token expansion leaves alone.
fn word(s: &Path) -> String {
    quote(&s.to_string_lossy()).replace('%', "%%")
}

#[cfg(test)]
mod tests;
