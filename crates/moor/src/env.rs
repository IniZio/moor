//! Process environment and argv access; the only reader of either.

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// File name, inside the state directory, of the lock that a running controller holds.
pub(crate) const CONTROLLER_LOCK: &str = "controller.lock";

/// The variable naming the Postgres database the store lives in, in place of the state dir's
/// `moor.db`.
pub(crate) const DATABASE_URL: &str = "MOOR_DATABASE_URL";

/// The variable capping the read connections the store opens to its Postgres database.
pub(crate) const DATABASE_READERS: &str = "MOOR_DATABASE_READERS";

/// The variable holding the key a Proxy process presents on the ledger socket.
pub(crate) const PROXY_KEY: &str = "MOOR_PROXY_KEY";

/// The variable holding the key the shared egress Proxy presents on the ledger socket and signs
/// Box tokens with. The controller and the shared Proxy are given the same one; unset, a
/// controller makes one and keeps it in its state dir.
pub(crate) const EGRESS_KEY: &str = "MOOR_EGRESS_KEY";

/// The variable naming the shared egress Proxy's `/egress` address, `wss://`, that Boxes dial.
/// Unset, the controller runs the shared Proxy itself and each Box reaches it over its tunnel.
pub(crate) const EGRESS_URL: &str = "MOOR_EGRESS_URL";
/// The key the shared egress Proxy sends the machine service, which checks it before it trusts
/// the Box the request names.
pub(crate) const MACHINE_SERVICE_KEY: &str = "MOOR_MACHINE_SERVICE_KEY";

/// Whether two keys match; their digests are compared in constant time, so timing says nothing
/// of the key.
pub(crate) fn same_key(want: &str, got: &str) -> bool {
    use sha2::{Digest as _, Sha256};
    use subtle::ConstantTimeEq as _;
    Sha256::digest(want.as_bytes())
        .ct_eq(&Sha256::digest(got.as_bytes()))
        .into()
}

/// The variable naming the egress forwarder binary the controller delivers into Boxes; unset,
/// `moor-egress-forward` beside `moor`.
pub(crate) const EGRESS_FORWARD_BIN: &str = "MOOR_EGRESS_FORWARD_BIN";

/// Snapshot of environment variables.
#[derive(Clone, Default)]
pub struct Env {
    /// Variable name to value.
    vars: BTreeMap<String, String>,
    /// The controller's vault key file, when a controller runs this.
    vault_key: Option<PathBuf>,
    /// Set when whoever asked for the request is gone; its streamed children then stop.
    cancel: Cancel,
    /// The rules each front's Proxy serves, by namespace, when a controller runs this.
    front_proxies: Arc<BTreeMap<String, Vec<String>>>,
    /// The fronts that deliver the users' webhooks, by namespace.
    front_webhooks: Arc<BTreeSet<String>>,
    /// The fronts that run elsewhere and dial `ledger_tls`.
    remote_fronts: Arc<BTreeSet<String>>,
    /// The shortest gap a workflow's cron may have between firings.
    workflow_min_interval: Option<Duration>,
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

    /// Set the shared egress Proxy's key, unless the environment already names one.
    #[must_use]
    pub(crate) fn with_egress_key(mut self, key: &str) -> Self {
        if self.get(EGRESS_KEY).is_none_or(str::is_empty) {
            self.vars.insert(EGRESS_KEY.to_owned(), key.to_owned());
        }
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

    /// The key the shared egress Proxy checks tokens with; empty when it is off.
    pub(crate) fn egress_key(&self) -> &[u8] {
        self.get(EGRESS_KEY).unwrap_or_default().as_bytes()
    }

    /// Set the fronts that run elsewhere and dial `ledger_tls`, by namespace.
    #[must_use]
    pub fn with_remote_fronts(mut self, fronts: BTreeSet<String>) -> Self {
        self.remote_fronts = Arc::new(fronts);
        self
    }

    /// Whether front `ns` runs elsewhere, so its Proxy is the shared egress Proxy.
    pub(crate) fn front_is_remote(&self, ns: &str) -> bool {
        self.remote_fronts.contains(ns)
    }

    /// The fronts that run elsewhere.
    pub(crate) fn remote_fronts(&self) -> impl Iterator<Item = &String> {
        self.remote_fronts.iter()
    }

    /// Set the fronts that deliver the users' webhooks, by namespace.
    #[must_use]
    pub fn with_front_webhooks(mut self, fronts: BTreeSet<String>) -> Self {
        self.front_webhooks = Arc::new(fronts);
        self
    }

    /// Set the shortest gap a workflow's cron may have between firings.
    #[must_use]
    pub const fn with_workflow_min_interval(mut self, floor: Duration) -> Self {
        self.workflow_min_interval = Some(floor);
        self
    }

    /// The shortest gap a workflow's cron may have between firings.
    pub(crate) fn workflow_min_interval(&self) -> Duration {
        self.workflow_min_interval
            .unwrap_or(moor_core::workflow::MIN_FIRING_INTERVAL)
    }

    /// Whether front `ns` delivers the users' webhooks.
    pub(crate) fn front_webhooks(&self, ns: &str) -> bool {
        self.front_webhooks.contains(ns)
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

    /// The Postgres URL the store lives in, when set and non-empty.
    pub(crate) fn database_url(&self) -> Option<&str> {
        self.non_empty(DATABASE_URL)
    }

    /// The cap on the store's read connections to Postgres, when set and non-empty.
    pub(crate) fn database_readers(&self) -> Option<&str> {
        self.non_empty(DATABASE_READERS)
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
