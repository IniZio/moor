//! The controller reaches its pool hosts through the machine runner's links broker,
//! `<state>/runner/links.sock`, and dials a lease's VM with `moor link-dial`.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_client::link::broker;
use moor_client::link::lease::{Answer, Verb};
use moor_core::boxscript::quote;

use super::PoolHosts;
use crate::adapters::crabbox::Crabbox;
use crate::runner_dir;

/// The runner's links broker under one state dir.
struct RunnerLinks {
    /// The state dir `moor link-dial` reads.
    state_dir: PathBuf,
    /// `<state>/runner/links.sock`.
    socket: PathBuf,
    /// This moor binary, which `link-dial` runs as.
    moor: PathBuf,
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

impl PoolHosts for RunnerLinks {
    fn lease(
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

    fn dial_command(&self, machine_id: i64, slug: &str) -> String {
        format!(
            "env MOOR_STATE_DIR={} {} link-dial {machine_id} {slug}",
            word(&self.state_dir),
            word(&self.moor)
        )
    }
}

impl Crabbox {
    /// Reaches pool hosts through the runner under `state_dir`, when there is one.
    #[must_use]
    pub(crate) fn with_runner_links(self, state_dir: Option<&Path>) -> Self {
        let Some(dir) = state_dir else {
            return self;
        };
        let moor = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("moor"));
        self.with_pool_hosts(Arc::new(RunnerLinks {
            state_dir: dir.to_owned(),
            socket: runner_dir::of(dir).join(broker::SOCKET),
            moor,
        }))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

    use super::*;

    #[test]
    fn the_dial_command_quotes_paths_for_the_shell_and_ssh() {
        let links = RunnerLinks {
            state_dir: PathBuf::from("/s/it's 100%"),
            socket: PathBuf::from("/s/runner/links.sock"),
            moor: PathBuf::from("/bin/moor"),
        };
        assert_eq!(
            links.dial_command(42, "moor-7"),
            "env MOOR_STATE_DIR='/s/it'\\''s 100%%' '/bin/moor' link-dial 42 moor-7"
        );
    }

    #[test]
    fn a_missing_broker_is_a_refusal_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let links = RunnerLinks {
            state_dir: dir.path().to_owned(),
            socket: dir.path().join("links.sock"),
            moor: PathBuf::from("/bin/moor"),
        };
        assert!(links.lease(1, Verb::List, None, None).is_err());
    }
}
