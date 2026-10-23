//! `moor proxy serve-fronts`: the front Proxy process the controller starts and restarts.
//!
//! It takes `--parent PID --state-dir DIR --vault-key PATH` and one `--front NS=RULE[,RULE]`
//! per front. It dies with the controller `PID`, and exits at once when that is not its parent.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use moor_core::config::proxy::{front_rules, ResolvedRule};
use rustix::process::{getppid, set_parent_process_death_signal, Pid, Signal};
use signal_hook::consts::{SIGINT, SIGTERM};

use super::{serve, Deps};
use crate::adapters::clock::SystemClock;
use crate::adapters::sockets::socket_dir;
use crate::proxy::nono::capture::SecretsFn;
use crate::proxy::Sink;
use crate::store::{Store, STORE_FILE};
use crate::vault::Vault;

/// What the command line asks for.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Args {
    /// The controller's pid.
    pub(crate) parent: Option<i32>,
    /// The controller's state dir.
    pub(crate) state_dir: Option<PathBuf>,
    /// The controller's vault key.
    pub(crate) vault_key: Option<PathBuf>,
    /// Each front and the names of the rules its Proxy serves.
    pub(crate) fronts: Vec<(String, Vec<String>)>,
}

impl Args {
    /// Parses the words after `serve-fronts`.
    pub(crate) fn parse(words: &[OsString]) -> Result<Self, String> {
        let mut args = Self::default();
        let mut words = words.iter().map(|w| w.to_string_lossy().into_owned());
        while let Some(flag) = words.next() {
            let mut value = || words.next().ok_or_else(|| format!("{flag} needs a value"));
            match flag.as_str() {
                "--parent" => {
                    let pid = value()?;
                    args.parent = Some(pid.parse().map_err(|_| format!("bad pid {pid}"))?);
                }
                "--state-dir" => args.state_dir = Some(value()?.into()),
                "--vault-key" => args.vault_key = Some(value()?.into()),
                "--front" => {
                    let front = value()?;
                    let (ns, rules) = front
                        .split_once('=')
                        .ok_or_else(|| format!("--front {front}: expected NS=RULE[,RULE]"))?;
                    let rules = rules.split(',').map(str::to_owned).collect();
                    args.fronts.push((ns.to_owned(), rules));
                }
                other => return Err(format!("unknown argument {other}")),
            }
        }
        Ok(args)
    }
}

/// Runs the front Proxy for `words`; returns the exit code.
///
/// # Errors
/// Why the Proxy could not start.
pub(crate) fn main(words: &[OsString]) -> Result<u8, String> {
    let args = Args::parse(words)?;
    let parent = args.parent.ok_or("--parent is required")?;
    set_parent_process_death_signal(Some(Signal::TERM))
        .map_err(|e| format!("cannot set the parent death signal: {e}"))?;
    if getppid().map(Pid::as_raw_nonzero).map(i32::from) != Some(parent) {
        return Err(format!("pid {parent} is not this process's parent"));
    }
    let state_dir = args.state_dir.ok_or("--state-dir is required")?;
    let key = args.vault_key.ok_or("--vault-key is required")?;
    let vault = Vault::load(&key).map_err(|e| format!("cannot open the vault: {e}"))?;
    let clock = Arc::new(SystemClock::real());
    let store =
        Arc::new(Store::open(&state_dir.join(STORE_FILE), clock).map_err(|e| e.to_string())?);
    let fronts = resolve(&args.fronts)?;
    let stop = Arc::new(AtomicBool::new(false));
    for sig in [SIGTERM, SIGINT] {
        signal_hook::flag::register(sig, Arc::clone(&stop)).map_err(|e| e.to_string())?;
    }
    let shared = Arc::clone(&store);
    let secrets: SecretsFn = Arc::new(move |vault, user, name| {
        shared
            .user_secret(vault, user, name)
            .map_err(|e| e.to_string())
    });
    let stderr: Sink = Arc::new(Mutex::new(std::io::stderr()));
    let deps = Deps {
        store,
        secrets,
        vault: Arc::new(vault),
        state_dir,
        sockets: socket_dir(),
        owner: rustix::process::getuid().as_raw(),
        upstream_ca: None,
        stop,
        stderr,
    };
    Ok(serve(&deps, &fronts))
}

/// Each front with the rules its names pick.
fn resolve(fronts: &[(String, Vec<String>)]) -> Result<Vec<(String, Vec<ResolvedRule>)>, String> {
    let known = front_rules();
    fronts
        .iter()
        .map(|(ns, names)| {
            let rules = names
                .iter()
                .map(|name| {
                    known
                        .iter()
                        .find(|r| r.name == *name)
                        .cloned()
                        .ok_or_else(|| format!("front {ns}: no proxy rule {name}"))
                })
                .collect::<Result<_, _>>()?;
            Ok((ns.clone(), rules))
        })
        .collect()
}
