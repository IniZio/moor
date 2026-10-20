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
use crate::proxy::detached::base64url;
use crate::proxy::front::{Front, FrontSocket};
use crate::proxy::graphql::http::Head;
use crate::proxy::graphql::upstream::Upstream;
use crate::proxy::graphql::{FilterSpec, GraphqlFilter};
use crate::proxy::guard::Guard;
use crate::proxy::nono::audit::{apply, BoxSink, EgressSink, Withheld};
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
    /// Credential cache the proxy and the audit drain share.
    pub(super) capture: Arc<MoorCapture>,
    /// Credential hosts watched for requests forwarded without a credential.
    withheld: Withheld,
    /// The Box's request log.
    pub(super) log: ProxyLog,
}

/// Loads the Box CA, opens the log and starts the filter and the proxy; the caller still holds the lease on failure.
pub(super) fn listen(deps: &Deps<'_>, box_id: BoxId, spec: &ProxySpec) -> Result<Live, String> {
    let ca = ca::load_or_create(deps.state_dir, box_id)
        .map_err(|err| format!("cannot load the proxy CA: {err}"))?;
    let withheld = Withheld::new(&spec.rules, spec.box_repo.clone());
    let capture = new_capture(deps, spec);
    let internal = internal_token()?;
    let graphql = start_filter(deps, spec, &ca, &capture)?;
    let ca_pem = ca::cert_path(deps.state_dir, box_id);
    let cfg = build_config(&NonoSpec {
        rules: &spec.rules,
        nonce: &internal,
        ca: &ca,
        ca_dir: &ca::ca_dir(deps.state_dir, box_id),
        upstream_ca: deps.upstream_ca.as_deref(),
        chain: graphql.as_ref().map(|f| Chain {
            proxy: f.addr(),
            host: GITHUB_API_HOST,
            ca_pem: &ca_pem,
        }),
    });
    let log = match ProxyLog::open(deps.state_dir, box_id, Arc::clone(&deps.clock)) {
        Ok(log) => log,
        Err(err) => {
            stop_filter(graphql);
            return Err(format!("cannot open the proxy log: {err}"));
        }
    };
    let proxy = match NonoProxy::start(cfg, Arc::clone(&capture)) {
        Ok(proxy) => proxy,
        Err(err) => {
            stop_filter(graphql);
            return Err(format!("cannot start the proxy: {err}"));
        }
    };
    let nono_addr = SocketAddr::from(([127, 0, 0, 1], proxy.port()));
    let socket = FrontSocket::new(deps.sockets, deps.owner, deps.state_dir, box_id);
    let front = match Front::start(nono_addr, socket, internal) {
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
        capture,
        withheld,
        log,
    })
}

/// The credential cache for `spec`'s rules; the GitHub rules' evictions are left to the filter.
fn new_capture(deps: &Deps<'_>, spec: &ProxySpec) -> Arc<MoorCapture> {
    let sources = sources_for(
        &spec.rules,
        Arc::clone(&deps.creds),
        Arc::clone(&deps.forge),
        spec.box_repo.clone(),
    );
    let mut capture = MoorCapture::new(sources, Arc::clone(&deps.clock));
    capture.defer_evictions(
        github_rules(spec)
            .into_iter()
            .map(|(name, _, inject)| (name, inject)),
    );
    Arc::new(capture)
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

/// For each rule that injects a credential for the GitHub API host: its
/// name, the header carrying the credential, and how it is injected.
pub(super) fn github_rules(spec: &ProxySpec) -> Vec<(String, String, ResolvedInject)> {
    spec.rules
        .iter()
        .filter(|r| lists_host(r, GITHUB_API_HOST))
        .map(|r| {
            let header = match &r.inject {
                ResolvedInject::Header { header, .. } => header.clone(),
                ResolvedInject::BasicAuth { .. } => "authorization".to_owned(),
            };
            (r.name.clone(), header, r.inject.clone())
        })
        .collect()
}

/// Drops a rule's cached credential when `head`, which just drew a 401,
/// carries exactly what nono injects for it in the rule's header.
pub(super) fn evict_credentialed(
    capture: &MoorCapture,
    rules: &[(String, String, ResolvedInject)],
    head: &Head,
) {
    for (rule, header, _) in rules {
        for (_, value) in head
            .headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case(header))
        {
            capture.evict_if_current(rule, value);
        }
    }
}

/// Starts the filter nono chains every CONNECT through; it resolves each destination once and refuses non-public ones.
fn start_filter(
    deps: &Deps<'_>,
    spec: &ProxySpec,
    ca: &ca::BoxCa,
    capture: &Arc<MoorCapture>,
) -> Result<Option<GraphqlFilter>, String> {
    let rule = spec
        .rules
        .iter()
        .find(|r| lists_host(r, GITHUB_API_HOST))
        .map_or_else(|| "egress".to_owned(), |r| r.name.clone());
    let fail = |err: &dyn std::fmt::Display| format!("cannot start the GraphQL filter: {err}");
    let guard = Guard::new(deps.guard_exempt.clone());
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
        repo: spec
            .box_repo
            .as_ref()
            .map(|r| RepoRef::new(&r.owner, &r.name)),
        host: GITHUB_API_HOST.to_owned(),
        port: 443,
        rule,
        on_credentialed_401: {
            let (capture, rules) = (Arc::clone(capture), github_rules(spec));
            Box::new(move |head| evict_credentialed(&capture, &rules, head))
        },
        credential_headers: spec
            .rules
            .iter()
            .filter(|r| lists_host(r, GITHUB_API_HOST))
            .filter_map(|r| match &r.inject {
                ResolvedInject::Header { header, .. } => Some(header.clone()),
                ResolvedInject::BasicAuth { .. } => None,
            })
            .collect(),
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
    pub(super) fn drain(&self, deps: &Deps<'_>, box_id: BoxId) {
        let sink = BoxSink {
            log: &self.log,
            store: deps.store,
            box_id,
        };
        // The filter notes an excused answer before nono sees it, so every one
        // in this audit batch is noted once the filter is drained after it.
        let audit = self.proxy.drain_audit();
        for status in self.graphql.iter().flat_map(GraphqlFilter::drain_excused) {
            self.withheld.excuse(GITHUB_API_HOST, status);
        }
        apply(&audit, &self.capture, &self.withheld, &sink);
        let sink: &dyn EgressSink = &sink;
        for method in self.graphql.iter().flat_map(GraphqlFilter::drain_withheld) {
            self.withheld
                .note(GITHUB_API_HOST, &method, "/graphql", sink);
        }
        for r in self.graphql.iter().flat_map(GraphqlFilter::drain_refusals) {
            sink.refused(&r.host, Some(&r.rule), Some(&r.method), Some(r.category));
        }
    }

    /// Removes the Box CA directory, which also holds the spec.
    pub(super) fn forget(deps: &Deps<'_>, box_id: BoxId) {
        if let Err(err) = ca::remove(deps.state_dir, box_id) {
            say(
                &deps.stderr,
                format_args!("cannot remove the proxy CA: {err}"),
            );
        }
    }
}
