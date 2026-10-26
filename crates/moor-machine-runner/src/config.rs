//! What the runner is started with.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use crate::relay::Relay;

/// What the runner needs to serve.
#[derive(Debug, Clone)]
pub struct Config {
    /// The controller's socket, which `sync` asks for the Box's work tree.
    pub socket: PathBuf,
    /// Where the runner keeps one host mirror per Box.
    pub state_dir: PathBuf,
    /// The `Host` the Box's Proxy passes through, accepted besides the loopback names.
    pub service_host: String,
    /// Units of each capped resource one machine has, such as `build` = 1.
    pub resources: BTreeMap<String, usize>,
    /// Command run on the machine before a Box's first `exec` or `machine_info`.
    pub prepare: Option<String>,
    /// Command run on the machine once a prepared Box is no longer open.
    pub release: Option<String>,
    /// Command run on the machine when a prepared Box falls asleep.
    pub suspend: Option<String>,
    /// Free GB a machine must have before a Box is prepared or synced there; 0 disables.
    pub min_free_gb: u64,
    /// How long `prepare`, `suspend` or `release` may run.
    pub hook_timeout: Duration,
    /// How often to look for prepared Boxes that are no longer open, and idle relay sessions.
    pub sweep: Duration,
    /// How often to list the files on each machine for Boxes the runner holds no record of.
    pub reconcile: Duration,
    /// The machine's stdio MCP servers whose tools the runner relays.
    pub relays: Vec<Relay>,
    /// How long a relay session may go without a call before it ends.
    pub relay_idle: Duration,
    /// How often an open call's stream gets a ping, so no client or proxy on the way sees it idle.
    pub keep_alive: Duration,
}
