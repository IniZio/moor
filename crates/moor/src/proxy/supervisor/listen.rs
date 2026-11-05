//! The nono proxy of one supervised Box, with its capture and request log.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use moor_core::config::proxy::{ResolvedInject, ResolvedRule};
use moor_core::graphql_scope::RepoRef;
use moor_core::seam::proxy_host::ProxySpec;
use moor_core::{BoxId, Secret};

use super::run::say;
use super::Deps;
use crate::proxy::chain::Proxy;
use crate::proxy::detached::base64url;
use crate::proxy::front::{Front, FrontSocket, Hold, MachineRoute};
use crate::proxy::graphql::upstream::Upstream;
use crate::proxy::graphql::{FilterSpec, GraphqlFilter};
use crate::proxy::guard::Guard;
use crate::proxy::nono::audit::{apply, BoxSink, EgressSink, Refusal, Withheld};
use crate::proxy::nono::capture::{sources_for, MoorCapture};
use crate::proxy::nono::{build_config, ca, Chain, NonoProxy, NonoSpec, GITHUB_API_HOST};
use crate::proxy::ProxyLog;

/// A started proxy and what it reports into.
pub(super) struct Live {
    /// The nono proxy. nono also serves origin-form requests directly to upstream, so its port is never exposed.
    proxy: NonoProxy,
    /// The CONNECT-only listener in front of nono; its socket is the only one the Box may reach.
    front: Front,
    /// The loopback filter every nono CONNECT chains through; it refuses non-public destinations.
    graphql: Option<GraphqlFilter>,
    /// Credential hosts watched for requests forwarded without a credential.
    withheld: Withheld,
    /// The Box's request log.
    pub(super) log: ProxyLog,
}

/// Loads the Box CA, opens the log and starts the filter and the proxy; the caller still holds the lease on failure.
pub(super) fn listen(deps: &Deps<'_>, box_id: BoxId, spec: &ProxySpec) -> Result<Live, String> {
    let ca = box_ca(deps, box_id)?;
    let withheld = Withheld::new(&spec.rules, spec.box_repo.clone());
    let capture = new_capture(deps, spec);
    let internal = internal_token()?;
    let graphql = start_filter(deps, spec, &ca)?;
    let ca_pem = ca::cert_path(deps.state_dir, box_id);
    let cfg = build_config(&NonoSpec {
        rules: &spec.rules,
        token: &internal,
        ca: &ca,
        ca_dir: &ca::ca_dir(deps.state_dir, box_id),
        upstream_ca: deps.upstream_ca.as_deref(),
        chain: graphql.as_ref().map(|f| Chain {
            proxy: f.addr(),
            host: GITHUB_API_HOST,
            ca_pem: &ca_pem,
        }),
        only_rules: false,
    });
    let log = match ProxyLog::open(deps.state_dir, box_id, Arc::clone(&deps.clock)) {
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
    let nono_addr = SocketAddr::from(([127, 0, 0, 1], proxy.port()));
    let socket = FrontSocket::new(deps.sockets, deps.owner, deps.state_dir, box_id);
    let front = match Front::start(
        nono_addr,
        socket,
        internal,
        machine_route(deps),
        Arc::clone(&deps.stderr),
    ) {
        Ok(front) => front,
        Err(err) => {
            proxy.stop();
            stop_filter(graphql);
            return Err(format!("cannot start the proxy front: {err}"));
        }
    };
    Ok(Live {
        proxy,
        front,
        graphql,
        withheld,
        log,
    })
}

/// The Box CA the controller keeps, installed in the Box's CA directory for nono to read.
fn box_ca(deps: &Deps<'_>, box_id: BoxId) -> Result<ca::BoxCa, String> {
    let pair = deps
        .ledger
        .ca()
        .map_err(|err| format!("cannot read the proxy CA: {err}"))?;
    ca::install(deps.state_dir, box_id, (&pair.key_pem, &pair.cert_pem))
        .map_err(|err| format!("cannot load the proxy CA: {err}"))
}

/// The machine service, reading what the Box holds through the ledger per request.
fn machine_route(deps: &Deps<'_>) -> Option<MachineRoute> {
    let service = deps.machine_service.clone()?;
    let ledger = Arc::clone(&deps.ledger);
    let held = Arc::new(move || {
        ledger.hold().unwrap_or_else(|e| Hold::Missing {
            box_id: ledger.box_id(),
            reason: format!("the Box's machine could not be read: {e}; try again later"),
        })
    });
    Some(MachineRoute { service, held })
}

/// The capture that reads the Box user's secrets on every request.
fn new_capture(deps: &Deps<'_>, spec: &ProxySpec) -> Arc<MoorCapture> {
    Arc::new(MoorCapture::new(sources_for(&spec.rules, &deps.secrets)))
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
pub(super) fn github_placeholder(rules: &[ResolvedRule]) -> Option<String> {
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
    deps: &Deps<'_>,
    spec: &ProxySpec,
    ca: &ca::BoxCa,
) -> Result<Option<GraphqlFilter>, String> {
    let rule = spec
        .rules
        .iter()
        .find(|r| lists_host(r, GITHUB_API_HOST))
        .map_or_else(|| "egress".to_owned(), |r| r.name.clone());
    let fail = |err: &dyn std::fmt::Display| format!("cannot start the GraphQL filter: {err}");
    let guard = Guard::new(deps.guard_exempt.clone()).with_proxy(Proxy::try_from_env());
    let upstream = Upstream::new(
        GITHUB_API_HOST,
        443,
        deps.upstream_dial,
        deps.upstream_ca.as_deref(),
    )
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

impl Live {
    /// The socket of the CONNECT-only front, the only listener the Box may reach.
    pub(super) fn socket(&self) -> &Path {
        self.front.path()
    }

    /// Binds the front socket again when its file is gone.
    pub(super) fn rebind(&self) -> Result<(), String> {
        self.front.rebind()
    }

    /// The port nono itself listens on, for tests that probe it directly.
    #[cfg(test)]
    pub(super) const fn nono_port(&self) -> u16 {
        self.proxy.port()
    }

    /// Stops the front, then the proxy, then the filter it chains through.
    pub(super) fn stop(self) {
        self.front.stop();
        self.proxy.stop();
        stop_filter(self.graphql);
    }

    /// Address of the running GraphQL filter, when one was started.
    #[cfg(test)]
    pub(super) fn graphql_addr(&self) -> Option<std::net::SocketAddr> {
        self.graphql.as_ref().map(GraphqlFilter::addr)
    }

    /// Moves the audit events recorded since the last call into the log and the store.
    pub(super) fn drain(&self, deps: &Deps<'_>) {
        let sink = BoxSink {
            log: &self.log,
            ledger: &deps.ledger,
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

    /// Removes the Box CA directory.
    pub(super) fn forget(deps: &Deps<'_>, box_id: BoxId) {
        if let Err(err) = ca::remove(deps.state_dir, box_id) {
            say(
                &deps.stderr,
                format_args!("cannot remove the proxy CA: {err}"),
            );
        }
    }
}
