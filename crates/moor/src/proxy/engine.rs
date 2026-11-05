//! The nono proxy of one Box, with its capture and request log. The shared egress Proxy runs
//! one engine per Box it serves and sends each of the Box's tunnels into it as a CONNECT.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use moor_core::config::proxy::{ResolvedInject, ResolvedRule};
use moor_core::graphql_scope::RepoRef;
use moor_core::seam::clock::Clock;
use moor_core::seam::proxy_host::ProxySpec;
use moor_core::{BoxId, Secret};

use crate::proxy::chain::Proxy;
use crate::proxy::graphql::upstream::Upstream;
use crate::proxy::graphql::{FilterSpec, GraphqlFilter};
use crate::proxy::guard::Guard;
use crate::proxy::ledger::BoxLedger;
use crate::proxy::nono::audit::{apply, BoxSink, EgressSink, Refusal, Withheld};
use crate::proxy::nono::capture::{sources_for, MoorCapture, UserSecretsFn};
use crate::proxy::nono::{build_config, ca, Chain, NonoProxy, NonoSpec, GITHUB_API_HOST};
use crate::proxy::token::base64url;
use crate::proxy::ProxyLog;

/// What a Box's engine needs from the process that runs it.
pub(crate) struct Parts<'a> {
    /// The Box's store calls.
    pub(crate) ledger: &'a BoxLedger,
    /// Reads the Box user's secret, on every request that needs one.
    pub(crate) secrets: &'a UserSecretsFn,
    /// Stamps the request log.
    pub(crate) clock: &'a Arc<dyn Clock>,
    /// Directory holding the proxy log and CA.
    pub(crate) state_dir: &'a Path,
    /// Extra upstream root PEM the proxy trusts; `None` in production.
    pub(crate) upstream_ca: Option<&'a Path>,
    /// Address the GraphQL filter dials instead of resolving the GitHub API host; `None` in production.
    pub(crate) upstream_dial: Option<SocketAddr>,
    /// Loopback addresses the egress guard lets through; empty in production.
    pub(crate) guard_exempt: &'a [SocketAddr],
}

/// A Box's nono proxy with the filter it chains through, ready for each CONNECT on loopback.
pub(crate) struct Engine {
    /// The nono proxy. nono also serves origin-form requests directly to upstream, so its port is never exposed.
    proxy: NonoProxy,
    /// The loopback filter every nono CONNECT chains through; it refuses non-public destinations.
    graphql: Option<GraphqlFilter>,
    /// Credential hosts watched for requests forwarded without a credential.
    withheld: Withheld,
    /// The Box's request log.
    log: ProxyLog,
    /// The token nono checks; the Box never sees it.
    internal: Secret,
}

/// Loads the Box CA, opens the log and starts the filter and the proxy.
pub(crate) fn engine(parts: &Parts<'_>, box_id: BoxId, spec: &ProxySpec) -> Result<Engine, String> {
    let ca = box_ca(parts, box_id)?;
    let withheld = Withheld::new(&spec.rules, spec.box_repo.clone());
    let capture = Arc::new(MoorCapture::new(sources_for(&spec.rules, parts.secrets)));
    let internal = internal_token()?;
    let graphql = start_filter(parts, spec, &ca)?;
    let ca_pem = ca::cert_path(parts.state_dir, box_id);
    let cfg = build_config(&NonoSpec {
        rules: &spec.rules,
        token: &internal,
        ca: &ca,
        ca_dir: &ca::ca_dir(parts.state_dir, box_id),
        upstream_ca: parts.upstream_ca,
        chain: graphql.as_ref().map(|f| Chain {
            proxy: f.addr(),
            host: GITHUB_API_HOST,
            ca_pem: &ca_pem,
        }),
        only_rules: false,
    });
    let log = match ProxyLog::open(parts.state_dir, box_id, Arc::clone(parts.clock)) {
        Ok(log) => log,
        Err(err) => {
            stop_filter(graphql);
            return Err(format!("cannot open the proxy log: {err}"));
        }
    };
    let proxy = match NonoProxy::start(cfg, capture) {
        Ok(proxy) => proxy,
        Err(err) => {
            stop_filter(graphql);
            return Err(format!("cannot start the proxy: {err}"));
        }
    };
    Ok(Engine {
        proxy,
        graphql,
        withheld,
        log,
        internal,
    })
}

impl Engine {
    /// The loopback address nono takes each CONNECT on.
    pub(crate) fn addr(&self) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], self.proxy.port()))
    }

    /// The token a CONNECT to [`Engine::addr`] must present.
    pub(crate) const fn internal(&self) -> &Secret {
        &self.internal
    }

    /// Stops the proxy, then the filter it chains through.
    pub(crate) fn stop(self) {
        self.proxy.stop();
        stop_filter(self.graphql);
    }

    /// Moves the audit events recorded since the last call into the log and the store.
    pub(crate) fn drain(&self, ledger: &BoxLedger) {
        let sink = BoxSink {
            log: &self.log,
            ledger,
        };
        // The filter notes an excused answer before nono sees it, so every one
        // in this audit batch is noted once the filter is drained after it.
        let audit = self.proxy.drain_audit();
        for status in self.graphql.iter().flat_map(GraphqlFilter::drain_excused) {
            self.withheld.excuse(GITHUB_API_HOST, status);
        }
        apply(&audit, &self.withheld, &sink);
        let sink: &dyn EgressSink = &sink;
        for method in self.graphql.iter().flat_map(GraphqlFilter::drain_withheld) {
            self.withheld
                .note(GITHUB_API_HOST, &method, "/graphql", sink);
        }
        for r in self.graphql.iter().flat_map(GraphqlFilter::drain_refusals) {
            sink.refused(&Refusal {
                host: &r.host,
                rule: Some(&r.rule),
                method: Some(&r.method),
                category: Some(r.category),
                reason: None,
            });
        }
    }
}

/// The Box CA the controller keeps, installed in the Box's CA directory for nono to read.
fn box_ca(parts: &Parts<'_>, box_id: BoxId) -> Result<ca::BoxCa, String> {
    let pair = parts
        .ledger
        .ca()
        .map_err(|err| format!("cannot read the proxy CA: {err}"))?;
    ca::install(parts.state_dir, box_id, (&pair.key_pem, &pair.cert_pem))
        .map_err(|err| format!("cannot load the proxy CA: {err}"))
}

/// A fresh token nono authenticates with: 32 random bytes as base64url text.
/// The Box never sees it, so a request looped back into nono's own port cannot authenticate.
fn internal_token() -> Result<Secret, String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|err| format!("cannot generate the proxy token: {err}"))?;
    Ok(Secret::new(base64url(&bytes).into_bytes()))
}

/// Whether some rule lists `host` (case-insensitive, port ignored).
fn lists_host(rule: &ResolvedRule, host: &str) -> bool {
    rule.hosts.iter().any(|h| {
        let bare = match h.rsplit_once(':') {
            Some((name, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => {
                name
            }
            _ => h.as_str(),
        };
        bare.eq_ignore_ascii_case(host)
    })
}

/// The `GH_TOKEN` a Box gets from the rules for the GitHub API; a later rule
/// replaces an earlier one, as in the Box env. A value that cannot be one
/// header token counts as none, so config can never add a header line.
pub(crate) fn github_placeholder(rules: &[ResolvedRule]) -> Option<String> {
    rules
        .iter()
        .rev()
        .filter(|r| lists_host(r, GITHUB_API_HOST))
        .find_map(|r| r.box_env.get("GH_TOKEN"))
        .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_graphic()))
        .cloned()
}

/// Starts the filter nono chains every CONNECT through; it resolves each destination once and refuses non-public ones.
fn start_filter(
    parts: &Parts<'_>,
    spec: &ProxySpec,
    ca: &ca::BoxCa,
) -> Result<Option<GraphqlFilter>, String> {
    let rule = spec
        .rules
        .iter()
        .find(|r| lists_host(r, GITHUB_API_HOST))
        .map_or_else(|| "egress".to_owned(), |r| r.name.clone());
    let fail = |err: &dyn std::fmt::Display| format!("cannot start the GraphQL filter: {err}");
    let guard = Guard::new(parts.guard_exempt.to_vec()).with_proxy(Proxy::try_from_env());
    let upstream = Upstream::new(GITHUB_API_HOST, 443, parts.upstream_dial, parts.upstream_ca)
        .map_err(|err| fail(&err))?
        .with_guard(guard.clone());
    let ephemeral = ca.ephemeral().map_err(|err| fail(&err))?;
    GraphqlFilter::start(FilterSpec {
        ca: ephemeral,
        repos: spec
            .box_repo
            .iter()
            .chain(&spec.other_repos)
            .map(|r| RepoRef::new(&r.owner, &r.name))
            .collect(),
        host: GITHUB_API_HOST.to_owned(),
        port: 443,
        rule,
        credential_headers: spec
            .rules
            .iter()
            .filter(|r| lists_host(r, GITHUB_API_HOST))
            .filter_map(|r| match &r.inject {
                ResolvedInject::Header { header, .. } => Some(header.clone()),
                ResolvedInject::BasicAuth { .. } => None,
            })
            .collect(),
        placeholder: github_placeholder(&spec.rules),
        upstream,
        guard,
    })
    .map(Some)
    .map_err(|err| fail(&err))
}

/// Stops the filter, if any.
fn stop_filter(filter: Option<GraphqlFilter>) {
    if let Some(f) = filter {
        f.stop();
    }
}
