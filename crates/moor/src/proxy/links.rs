//! The controller reaches its users' machines through the machine runner that holds each
//! one's link, as its link row names it, and dials a pool lease's VM with `moor link-dial`.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_client::link::broker;
use moor_client::link::lease::{Answer, Boot, Verb};
use moor_client::link::reach::{self, Runner, Service, StdConn};
use moor_client::link::status::Status;
use moor_core::boxscript::quote;
use moor_core::seam::clock::Clock;

use crate::runner_dir;
use crate::store::Store;

/// The machine runners this controller reaches, found by each machine's link row.
pub(crate) struct Runners {
    /// The runner on this host, `<state>/runner/runner.sock`, which holds every link when there
    /// is no store.
    local: PathBuf,
    /// The recorded links; none outside a controller.
    store: Option<Store>,
    /// The time a link's last report is measured against.
    clock: Arc<dyn Clock>,
    /// The machine service key a runner across the network checks.
    key: Option<String>,
    /// This moor binary, which `link-dial` runs as.
    moor: PathBuf,
}

impl Runners {
    /// The runners of the controller under `state_dir`, routed by the links `store` records and
    /// reached across the network with `key`.
    pub(crate) fn new(
        state_dir: &Path,
        store: Option<Store>,
        clock: Arc<dyn Clock>,
        key: Option<&str>,
    ) -> Self {
        Self {
            local: runner_dir::of(state_dir).join("runner.sock"),
            store,
            clock,
            key: key.map(str::to_owned),
            moor: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("moor")),
        }
    }

    /// The address of the runner that holds machine `machine_id`'s link.
    fn addr(&self, machine_id: i64) -> Result<String, String> {
        let Some(store) = &self.store else {
            return Ok(self.local.to_string_lossy().into_owned());
        };
        match store.machine_runner(machine_id, self.clock.now()) {
            Ok(Some(addr)) => Ok(addr.as_str().to_owned()),
            Ok(None) => Err(format!("machine {machine_id} is not linked")),
            Err(e) => Err(format!("cannot read machine {machine_id}'s link: {e}")),
        }
    }

    /// The runner that holds machine `machine_id`'s link.
    ///
    /// # Errors
    /// No runner holds it, or one across the network and no key.
    pub(crate) fn runner(&self, machine_id: i64) -> Result<Runner, String> {
        Runner::at(&self.addr(machine_id)?, self.key.as_deref())
    }

    /// Asks machine `machine_id`'s agent to run `verb` on lease `slug`, installing `key` for
    /// an authorize and booting with `boot` on a warmup.
    ///
    /// # Errors
    /// The broker or the agent's failure.
    pub(crate) fn lease(
        &self,
        machine_id: i64,
        verb: Verb,
        slug: Option<&str>,
        key: Option<&str>,
        boot: &Boot,
    ) -> Result<Answer, String> {
        let runner = self.runner(machine_id)?;
        match (verb, slug, key) {
            (Verb::Authorize, Some(slug), Some(key)) => {
                blocking(broker::authorize(&runner, machine_id, slug, key))
            }
            (Verb::Authorize, ..) => Err("an authorize needs a lease and a key".to_owned()),
            _ => blocking(broker::lease(&runner, machine_id, verb, slug, boot.clone())),
        }
    }

    /// Machine `machine_id`'s agent status.
    ///
    /// # Errors
    /// The broker or the agent's failure.
    pub(crate) fn status(&self, machine_id: i64) -> Result<Status, String> {
        let runner = self.runner(machine_id)?;
        blocking(broker::status(&runner, machine_id))
    }

    /// The ssh `ProxyCommand` that reaches lease `slug`'s VM on machine `machine_id` through
    /// the runner that holds its link now; ssh hands it the key from its environment.
    ///
    /// # Errors
    /// No runner holds the machine's link.
    pub(crate) fn dial_command(&self, machine_id: i64, slug: &str) -> Result<String, String> {
        let addr = self.addr(machine_id)?;
        Ok(format!(
            "{} link-dial --runner {} {machine_id} {slug}",
            word(&self.moor.to_string_lossy()),
            word(&addr)
        ))
    }
}

/// A blocking stream to the sshd of lease `slug` on machine `machine_id` through `runner`,
/// found within `timeout_secs` when given.
///
/// # Errors
/// The broker or the agent's failure.
pub(crate) fn dial(
    runner: &Runner,
    machine_id: i64,
    slug: &str,
    timeout_secs: Option<u64>,
) -> Result<StdConn, String> {
    blocking(async {
        let conn = broker::dial(runner, machine_id, slug, timeout_secs).await?;
        reach::into_blocking(conn).map_err(|e| e.to_string())
    })
}

/// A blocking connection to `runner` for `service`, its hello sent.
///
/// # Errors
/// The runner cannot be reached.
pub(crate) fn connect(runner: &Runner, service: Service) -> Result<StdConn, String> {
    blocking(async {
        let conn = runner.connect(service).await?;
        reach::into_blocking(conn).map_err(|e| e.to_string())
    })
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
fn word(s: &str) -> String {
    quote(s).replace('%', "%%")
}

#[cfg(test)]
mod tests;
