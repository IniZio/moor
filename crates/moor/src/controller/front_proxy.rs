//! The front Proxy as a child of the controller: `moor proxy serve-fronts` for every front
//! whose config lists `proxy_rules`, restarted with backoff whenever it exits. A front with a
//! Proxy gets its socket, through the shim's loopback bridge, and the CA that Proxy signs with.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Instant;

use super::fronts::{relay_lines, watch, Outcome, Stop, FIRST_BACKOFF, MAX_BACKOFF, STEADY};
use moor_core::config::FrontEntry;

use super::{log, Sink};
use crate::env::Env;
use crate::Moor;

/// The variable that holds the PEM certificate of the CA a front's Proxy signs with. The
/// front writes it where it likes: no path it can read is the controller's.
pub(super) const CA_PEM_VAR: &str = "MOOR_FRONT_PROXY_CA_PEM";

/// What a front with a Proxy is given.
pub(super) struct Access {
    /// The Proxy's socket, which the outer shim bridges.
    pub(super) socket: PathBuf,
    /// The CA certificate, in PEM form.
    pub(super) ca_pem: String,
}

/// The front Proxy this controller runs.
pub(super) struct Plan {
    /// The controller's state dir.
    state_dir: PathBuf,
    /// The controller's vault key.
    vault_key: PathBuf,
    /// Each front with a Proxy and the names of its rules.
    fronts: BTreeMap<String, Vec<String>>,
    /// The CA certificate, in PEM form.
    ca_pem: String,
}

/// `env` with the rules each front's Proxy serves, and the Proxy for every entry of `fronts`
/// that lists `proxy_rules`; none when no entry does.
pub(super) fn plan(
    env: Env,
    fronts: &BTreeMap<String, FrontEntry>,
) -> Result<(Env, Option<Plan>), String> {
    let proxied: BTreeMap<String, Vec<String>> = fronts
        .iter()
        .filter(|(_, front)| !front.proxy_rules.is_empty())
        .map(|(ns, front)| (ns.clone(), front.proxy_rules.clone()))
        .collect();
    let env = env.with_front_proxies(proxied.clone());
    let plan = Plan::new(&env, proxied)?;
    Ok((env, plan))
}

impl Plan {
    /// The Proxy for the fronts in `fronts`, with the CA made when missing; none when no front
    /// lists rules.
    fn new(env: &Env, fronts: BTreeMap<String, Vec<String>>) -> Result<Option<Self>, String> {
        if fronts.is_empty() {
            return Ok(None);
        }
        let state_dir = env.state_dir().ok_or("a front Proxy needs a state dir")?;
        let vault_key = env
            .vault_key()
            .ok_or("a front Proxy needs the vault key")?
            .to_owned();
        let ca_pem = Moor::front_proxy_ca(&state_dir)
            .and_then(std::fs::read_to_string)
            .map_err(|e| format!("cannot make the front Proxy CA: {e}"))?;
        Ok(Some(Self {
            state_dir,
            vault_key,
            fronts,
            ca_pem,
        }))
    }

    /// What front `ns` is given; none when it has no Proxy.
    pub(super) fn access(&self, ns: &str) -> Option<Access> {
        self.fronts.contains_key(ns).then(|| Access {
            socket: Moor::front_proxy_socket(&self.state_dir, ns),
            ca_pem: self.ca_pem.clone(),
        })
    }

    /// The command that runs this Proxy with `program`, the moor binary.
    fn command(&self, program: &Path) -> Command {
        let mut command = Command::new(program);
        command
            .args([
                "proxy",
                "serve-fronts",
                "--parent",
                &process::id().to_string(),
            ])
            .arg("--state-dir")
            .arg(&self.state_dir)
            .arg("--vault-key")
            .arg(&self.vault_key);
        for (ns, rules) in &self.fronts {
            command
                .arg("--front")
                .arg(format!("{ns}={}", rules.join(",")));
        }
        command
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }
}

/// Runs `plan` with `program` until told to stop, restarting it with backoff whenever it exits.
#[expect(
    clippy::disallowed_methods,
    reason = "times how long a child ran to pick its restart backoff, not domain time"
)]
pub(super) fn keep_running(plan: &Plan, program: &OsString, stop: &Receiver<Stop>, sink: &Sink) {
    let mut backoff = FIRST_BACKOFF;
    loop {
        let started = Instant::now();
        let exited = match plan.command(Path::new(program)).spawn() {
            Ok(mut child) => {
                if let Some(out) = child.stdout.take() {
                    relay_lines(out, "proxy", sink);
                }
                if let Some(err) = child.stderr.take() {
                    relay_lines(err, "proxy", sink);
                }
                log(sink, &format!("front proxy: started pid {}", child.id()));
                let Outcome::Exited(status) = watch(&mut child, stop) else {
                    return;
                };
                format!("exited with {status}")
            }
            Err(e) => format!("cannot start: {e}"),
        };
        if started.elapsed() >= STEADY {
            backoff = FIRST_BACKOFF;
        }
        log(
            sink,
            &format!("front proxy: {exited}; restarting in {backoff:?}"),
        );
        if !matches!(stop.recv_timeout(backoff), Err(RecvTimeoutError::Timeout)) {
            return;
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}
