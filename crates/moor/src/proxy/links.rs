//! The controller reaches its users' machines through the machine runner that holds each
//! one's link, as its link row names it.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moor_client::link::broker;
use moor_client::link::reach::{self, Runner, Service, StdConn};
use moor_client::link::status::Status;
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

    /// Machine `machine_id`'s agent status.
    ///
    /// # Errors
    /// The broker or the agent's failure.
    pub(crate) fn status(&self, machine_id: i64) -> Result<Status, String> {
        let runner = self.runner(machine_id)?;
        blocking(broker::status(&runner, machine_id))
    }
}

/// A blocking connection to `runner` for `service`, its hello sent, given up after `within`.
///
/// # Errors
/// The runner cannot be reached.
pub(crate) fn connect(
    runner: &Runner,
    service: Service,
    within: Duration,
) -> Result<StdConn, String> {
    blocking(async {
        let conn = runner.connect_within(service, within).await?;
        reach::into_blocking(conn).map_err(|e| e.to_string())
    })
}

/// Runs `call` to completion on a runtime of its own, off any runtime the caller is on.
fn blocking<T: Send>(call: impl Future<Output = Result<T, String>> + Send) -> Result<T, String> {
    std::thread::scope(|s| {
        s.spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?;
            let done = runtime.block_on(call);
            // A name lookup still running on the blocking pool must not hold the caller.
            runtime.shutdown_background();
            done
        })
        .join()
        .unwrap_or_else(|_| Err("the links broker call panicked".to_owned()))
    })
}

#[cfg(test)]
mod tests;
