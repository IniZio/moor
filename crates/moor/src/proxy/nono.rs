//! The embedded nono-proxy: config built from resolved tool rules, and the
//! runtime that serves it.
//!
//! nono needs three things for a rule host such as `127.0.0.1:4433`: the
//! `host:port` entry in `allowed_hosts` (its filter matches `host:port`
//! entries by the CONNECT port), an `https://host:port` route upstream, and
//! the route credential key. Neither the session token nor any credential is
//! ever printed.

pub(crate) mod audit;
pub(crate) mod ca;
pub(crate) mod capture;

use std::collections::BTreeSet;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_core::config::proxy::{ResolvedInject, ResolvedRule};
use moor_core::Secret;
use nono_proxy::config::{EndpointRule, ExternalProxyConfig, InjectMode, RouteConfig};
use nono_proxy::ProxyConfig;
use zeroize::Zeroizing;

/// Inputs for one Task's proxy.
pub(crate) struct NonoSpec<'a> {
    /// Resolved tool rules; one route per (rule, host).
    pub(crate) rules: &'a [ResolvedRule],
    /// Extra passthrough hosts (no credential).
    pub(crate) allow: &'a [String],
    /// The internal session token nono checks, base64url text. The Box never
    /// sees it: the front listener checks the Box's own nonce and swaps it
    /// for this one.
    pub(crate) nonce: &'a Secret,
    /// The Task's interception CA.
    pub(crate) ca: &'a ca::TaskCa,
    /// Directory nono writes its CA bundle into (0700, exists).
    pub(crate) ca_dir: &'a Path,
    /// Extra upstream root PEM for every route; `None` in production.
    pub(crate) upstream_ca: Option<&'a Path>,
    /// Sends every CONNECT through an external proxy with no bypass; `None` keeps all direct.
    pub(crate) chain: Option<Chain<'a>>,
}

/// The host whose TLS the filter terminates and whose GraphQL it judges.
pub(crate) const GITHUB_API_HOST: &str = "api.github.com";

/// The external proxy that receives all of nono's egress.
pub(crate) struct Chain<'a> {
    /// The external proxy (the loopback filter). Every CONNECT goes here.
    pub(crate) proxy: SocketAddr,
    /// Hostname (no port) whose TLS the filter terminates.
    pub(crate) host: &'a str,
    /// Task CA certificate PEM that signs the filter's leaf for `host`.
    pub(crate) ca_pem: &'a Path,
}

/// `host` without a trailing `:port`; route hosts may carry a port; the chained host is a bare hostname.
fn bare_host(host: &str) -> &str {
    match host.rsplit_once(':') {
        Some((h, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => h,
        _ => host,
    }
}

/// Route prefix for (rule, host index).
pub(crate) fn route_prefix(rule: &str, index: usize) -> String {
    format!("{rule}__{index}")
}

/// Rule name of a nono route id or prefix.
pub(crate) fn rule_of_route(route_id: &str) -> Option<&str> {
    route_id.rsplit_once("__").map(|(rule, _)| rule)
}

/// nono's proxy config with a Debug that hides the session token.
pub(crate) struct NonoConfig(ProxyConfig);

impl NonoConfig {
    /// The wrapped nono config.
    #[cfg(test)]
    pub(crate) const fn inner(&self) -> &ProxyConfig {
        &self.0
    }
}

impl fmt::Debug for NonoConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NonoConfig")
            .field("allowed_hosts", &self.0.allowed_hosts)
            .field("routes", &self.0.routes.len())
            .finish_non_exhaustive()
    }
}

/// One nono route for `host` of `rule`.
fn route_for(spec: &NonoSpec<'_>, rule: &ResolvedRule, index: usize, host: &str) -> RouteConfig {
    let mut route = RouteConfig {
        prefix: route_prefix(&rule.name, index),
        upstream: format!("https://{host}"),
        credential_key: Some(format!("cmd://{}", rule.name)),
        endpoint_rules: rule
            .endpoint_rules
            .iter()
            .map(|r| EndpointRule {
                method: r.method.clone(),
                path: r.path.clone(),
            })
            .collect(),
        tls_ca: spec
            .chain
            .as_ref()
            .filter(|c| bare_host(host).eq_ignore_ascii_case(c.host))
            .map(|c| c.ca_pem)
            .or(spec.upstream_ca)
            .map(|p| p.to_string_lossy().into_owned()),
        ..RouteConfig::default()
    };
    match &rule.inject {
        ResolvedInject::Header { header, form } => {
            route.inject_mode = InjectMode::Header;
            route.inject_header.clone_from(header);
            route.credential_format = Some(form.replace("{secret}", "{}"));
        }
        ResolvedInject::BasicAuth { .. } => {
            route.inject_mode = InjectMode::BasicAuth;
            "Authorization".clone_into(&mut route.inject_header);
        }
    }
    route
}

/// Builds the nono config for one Task.
///
/// A nonce that is not UTF-8 leaves the session token unset; `start` refuses
/// such a config.
pub(crate) fn build_config(spec: &NonoSpec<'_>) -> NonoConfig {
    let mut hosts: BTreeSet<String> = spec.allow.iter().cloned().collect();
    let mut routes = Vec::new();
    for rule in spec.rules {
        for (i, host) in rule.hosts.iter().enumerate() {
            hosts.insert(host.clone());
            routes.push(route_for(spec, rule, i, host));
        }
    }
    // An empty bypass list sends every CONNECT to the external proxy, so the
    // filter vets every destination.
    let external_proxy = spec.chain.as_ref().map(|c| ExternalProxyConfig {
        address: c.proxy.to_string(),
        auth: None,
        bypass_hosts: Vec::new(),
    });
    // Unmatched requests are passthrough, but the chained filter still
    // re-signs them with its CA.
    let upstream_extra_ca_pems = spec
        .chain
        .as_ref()
        .map(|c| c.ca_pem.to_string_lossy().into_owned())
        .into_iter()
        .collect();
    NonoConfig(ProxyConfig {
        external_proxy,
        upstream_extra_ca_pems,
        bind_addr: IpAddr::V4(Ipv4Addr::LOCALHOST),
        bind_port: 0,
        allowed_hosts: hosts.into_iter().collect(),
        strict_filter: true,
        require_auth: true,
        strict_connect_auth: true,
        session_token: std::str::from_utf8(spec.nonce.expose())
            .ok()
            .map(|t| Zeroizing::new(t.to_owned())),
        routes,
        intercept_ca_dir: Some(spec.ca_dir.to_path_buf()),
        preloaded_ca: Some(spec.ca.preloaded()),
        enable_h2: false,
        enable_network_audit: true,
        ..ProxyConfig::default()
    })
}

/// Why the proxy did not start.
#[derive(Debug, thiserror::Error)]
pub(crate) enum NonoError {
    /// The async runtime could not be built.
    #[error("proxy runtime: {0}")]
    Runtime(String),
    /// nono refused to start.
    #[error("proxy start: {0}")]
    Start(String),
}

/// How long runtime shutdown waits for in-flight connections.
const SHUTDOWN_GRACE: Duration = Duration::from_millis(500);

/// Running proxy: owns a 2-worker runtime and the nono handle.
pub(crate) struct NonoProxy {
    /// Runtime driving the proxy's tasks.
    runtime: tokio::runtime::Runtime,
    /// nono's handle (port, shutdown, audit buffer).
    handle: nono_proxy::ProxyHandle,
}

impl fmt::Debug for NonoProxy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NonoProxy")
            .field("port", &self.handle.port)
            .finish_non_exhaustive()
    }
}

impl NonoProxy {
    /// Starts the proxy on a loopback ephemeral port.
    ///
    /// # Errors
    /// [`NonoError::Runtime`] when the runtime cannot be built,
    /// [`NonoError::Start`] when nono rejects the config or cannot bind.
    pub(crate) fn start(
        cfg: NonoConfig,
        capture: Arc<capture::MoorCapture>,
    ) -> Result<Self, NonoError> {
        if cfg.0.session_token.is_none() {
            return Err(NonoError::Start("session token is not utf-8".to_owned()));
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| NonoError::Runtime(e.to_string()))?;
        let handle = runtime
            .block_on(
                nono_proxy::server::start_with_approval_and_capture_registry(
                    cfg.0,
                    None,
                    Some(capture),
                ),
            )
            .map_err(|e| NonoError::Start(e.to_string()))?;
        Ok(Self { runtime, handle })
    }

    /// The loopback port the proxy listens on.
    pub(crate) const fn port(&self) -> u16 {
        self.handle.port
    }

    /// Takes the audit events recorded since the last call.
    pub(crate) fn drain_audit(&self) -> Vec<nono::undo::NetworkAuditEvent> {
        self.handle.drain_audit_events()
    }

    /// Stops the proxy and its runtime.
    pub(crate) fn stop(self) {
        self.handle.shutdown();
        self.runtime.shutdown_timeout(SHUTDOWN_GRACE);
    }
}

#[cfg(test)]
pub(crate) mod tests;
