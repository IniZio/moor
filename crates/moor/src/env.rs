//! Process environment and argv access; the only reader of either.

use std::any::Any;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// File name, inside the state directory, of the lock that a running controller holds.
pub(crate) const CONTROLLER_LOCK: &str = "controller.lock";

/// Snapshot of environment variables.
#[derive(Clone, Default)]
pub struct Env {
    /// Variable name to value.
    vars: BTreeMap<String, String>,
    /// The controller's vault key file, when a controller runs this.
    vault_key: Option<PathBuf>,
    /// The machine runner's state directory, when a controller runs this.
    runner_state: Option<PathBuf>,
    /// Set when whoever asked for the request is gone; its streamed children then stop.
    cancel: Cancel,
    /// The rules each front's Proxy serves, by namespace, when a controller runs this.
    front_proxies: Arc<BTreeMap<String, Vec<String>>>,
    /// What the controller opened once for every request and tick, such as the state store.
    shared: Option<Arc<dyn Any + Send + Sync>>,
}

/// A request's stop flag, shared by everyone who holds a clone.
#[derive(Clone, Debug, Default)]
pub(crate) struct Cancel(Arc<AtomicBool>);

impl Cancel {
    /// Asks the request to stop.
    pub(crate) fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Whether the request was asked to stop.
    pub(crate) fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

impl Env {
    /// Capture the process environment; non-UTF8 pairs are skipped.
    #[must_use]
    pub fn capture() -> Self {
        let vars = std::env::vars_os()
            .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
            .collect();
        Self {
            vars,
            ..Self::default()
        }
    }

    /// Build an environment from explicit pairs.
    pub fn from_pairs<I, K, V>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Self {
            vars: pairs
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
            ..Self::default()
        }
    }

    /// Raw process arguments, program name first.
    #[must_use]
    pub fn args() -> Vec<OsString> {
        std::env::args_os().collect()
    }

    /// Value of `key`, if set.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.vars.get(key).map(String::as_str)
    }

    /// Variables whose name starts with `prefix`.
    pub fn with_prefix<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = (&'a str, &'a str)> {
        self.vars
            .iter()
            .filter(move |(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Override the state directory (sets `MOOR_STATE_DIR`).
    #[must_use]
    pub fn with_state_dir(mut self, dir: &Path) -> Self {
        self.vars.insert(
            "MOOR_STATE_DIR".to_owned(),
            dir.to_string_lossy().into_owned(),
        );
        self
    }

    /// Set the controller's vault key file.
    #[must_use]
    pub fn with_vault_key(mut self, path: PathBuf) -> Self {
        self.vault_key = Some(path);
        self
    }

    /// The controller's vault key file; `None` outside a controller.
    #[must_use]
    pub fn vault_key(&self) -> Option<&Path> {
        self.vault_key.as_deref()
    }

    /// Set the machine runner's state directory.
    #[must_use]
    pub fn with_runner_state(mut self, dir: Option<PathBuf>) -> Self {
        self.runner_state = dir;
        self
    }

    /// The machine runner's state directory; `None` outside a controller.
    #[must_use]
    pub fn runner_state(&self) -> Option<&Path> {
        self.runner_state.as_deref()
    }

    /// Set the rules each front's Proxy serves, by namespace.
    #[must_use]
    pub fn with_front_proxies(mut self, proxies: BTreeMap<String, Vec<String>>) -> Self {
        self.front_proxies = Arc::new(proxies);
        self
    }

    /// The names of the rules front `ns`'s Proxy serves; none when it has no Proxy.
    pub(crate) fn front_proxy_rules(&self, ns: &str) -> &[String] {
        self.front_proxies.get(ns).map_or(&[], Vec::as_slice)
    }

    /// Share `value` with every request and tick that runs in this environment.
    #[must_use]
    pub(crate) fn sharing<T: Any + Send + Sync>(mut self, value: T) -> Self {
        self.shared = Some(Arc::new(value));
        self
    }

    /// The shared value when it is a `T`; `None` outside a controller.
    pub(crate) fn shared<T: Any>(&self) -> Option<&T> {
        self.shared.as_deref()?.downcast_ref()
    }

    /// This environment for one request that stops when `cancel` is set.
    #[must_use]
    pub(crate) fn with_cancel(mut self, cancel: Cancel) -> Self {
        self.cancel = cancel;
        self
    }

    /// The stop flag of the request this environment serves.
    pub(crate) const fn cancel(&self) -> &Cancel {
        &self.cancel
    }

    /// Resolved state directory, or `None` when no source is set.
    #[must_use]
    pub fn state_dir(&self) -> Option<PathBuf> {
        if let Some(dir) = self.non_empty("MOOR_STATE_DIR") {
            return Some(PathBuf::from(dir));
        }
        self.xdg("XDG_STATE_HOME", ".local/state", "moor")
    }

    /// `$var/sub`, else `$HOME/fallback/sub`; empty values count as unset.
    #[must_use]
    pub fn xdg(&self, var: &str, fallback: &str, sub: &str) -> Option<PathBuf> {
        let base = match self.non_empty(var) {
            Some(x) => PathBuf::from(x),
            None => Path::new(self.non_empty("HOME")?).join(fallback),
        };
        Some(base.join(sub))
    }

    /// Expand a leading `~` or `~/` using `HOME` from this environment.
    ///
    /// Any other path, or any path when `HOME` is unset, is returned unchanged.
    #[must_use]
    pub fn expand_home(&self, p: &Path) -> PathBuf {
        match (p.strip_prefix("~"), self.get("HOME")) {
            (Ok(rest), Some(home)) => Path::new(home).join(rest),
            _ => p.to_path_buf(),
        }
    }

    /// Value of `key` when set and non-empty.
    fn non_empty(&self, key: &str) -> Option<&str> {
        self.get(key).filter(|v| !v.is_empty())
    }
}

#[cfg(test)]
mod tests;
