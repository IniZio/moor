//! A client goes through the engine's nono proxy to a fake HTTPS upstream.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use moor_core::config::proxy::{EndpointRule, ResolvedInject, ResolvedRule};
use moor_core::seam::proxy_host::ProxySpec;
use moor_core::Secret;
use ureq::Agent;

use super::{Host, Running, Wiring, DEADLINE};
use crate::proxy::ledger::tests::box_ledger;
use crate::proxy::nono::tests::upstream::{self, Upstream};

/// What the client sends before the proxy swaps it out.
const PLACEHOLDER: &str = "Bearer placeholder-not-real";
/// The credential the fake credential source returns.
const SECRET: &str = "test-token-1";

/// One header-injecting rule for `localhost:<port>` that allows only `GET /ok`.
fn rule(port: u16) -> ResolvedRule {
    ResolvedRule {
        name: "tool".to_owned(),
        hosts: vec![format!("localhost:{port}")],
        credential: "tool-token".to_owned(),
        inject: ResolvedInject::Header {
            header: "authorization".to_owned(),
            form: "Bearer {secret}".to_owned(),
        },
        endpoint_rules: vec![EndpointRule {
            method: "GET".to_owned(),
            path: "/ok".to_owned(),
        }],
        box_env: BTreeMap::new(),
    }
}

/// GETs `url` with the placeholder credential; status or error text.
fn get(agent: &Agent, url: &str) -> Result<u16, String> {
    agent
        .get(url)
        .header("Authorization", PLACEHOLDER)
        .call()
        .map(|r| r.status().as_u16())
        .map_err(|e| e.to_string())
}

/// An allowed request reaches the upstream with the real credential in place of the placeholder.
fn assert_injected(client: &Agent, up: &Upstream) {
    assert_eq!(
        get(client, &format!("https://localhost:{}/ok", up.port)),
        Ok(200)
    );
    let seen = up.seen();
    let [only] = seen.as_slice() else {
        unreachable!("{seen:?}")
    };
    assert_eq!(only.line, "GET /ok HTTP/1.1");
    assert_eq!(only.authorization.as_deref(), Some("Bearer test-token-1"));
}

/// Requests outside the rules never reach the upstream.
fn assert_refused(client: &Agent, up: &Upstream) {
    let denied = get(client, &format!("https://localhost:{}/denied", up.port));
    assert!(
        denied.is_err() || matches!(denied, Ok(403 | 502)),
        "got {denied:?}"
    );
    assert_eq!(up.seen().len(), 1);
    let other = get(client, "https://example.invalid/anything");
    assert!(other.is_err() || other == Ok(403), "got {other:?}");
}

/// The refusal is recorded as an event and the allowed request is logged.
fn assert_audited(run: &Running, host: &Host) {
    run.events_until("a refused-egress event", |events| {
        events.iter().any(|(k, _)| k == "egress_refused")
    });
    let log = host
        .dir
        .path()
        .join("proxy")
        .join(format!("{}.log", run.box_id.get()));
    let step = Duration::from_millis(25);
    let found = (0..DEADLINE.as_millis() / step.as_millis()).any(|_| {
        run.drain();
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        let hit = text
            .lines()
            .any(|l| l.contains(" GET ") && l.ends_with(" 200"));
        if !hit {
            std::thread::sleep(step);
        }
        hit
    });
    assert!(found, "no request log line for the allowed GET");
}

#[test]
fn engine_injects_the_credential_audits_requests_and_stops() {
    let host = Host::new();
    let up = upstream::start(host.dir.path());
    let id = host.new_box("demo", None);
    let spec = ProxySpec {
        box_id: id,
        box_handle: None,
        other_repos: Vec::new(),
        box_repo: None,
        rules: vec![rule(up.port)],
        user: moor_core::user::UserId::new(1),
    };
    let wiring = Wiring {
        secrets: Arc::new(|name| {
            assert_eq!(name.as_str(), "tool-token");
            Ok(Some(Secret::new(SECRET.as_bytes().to_vec())))
        }),
        upstream_ca: Some(up.ca_path.clone()),
        upstream_dial: None,
        guard_exempt: vec![SocketAddr::from((Ipv4Addr::LOCALHOST, up.port))],
    };
    let run = Running::start(&host, id, box_ledger(&host.store, id), &spec, &wiring);
    let client = run.client();

    assert_injected(&client, &up);
    assert_refused(&client, &up);
    assert_audited(&run, &host);
    assert_eq!(up.seen().len(), 1);

    let addr = run.engine().addr();
    run.stop();
    assert!(TcpStream::connect_timeout(&addr, Duration::from_secs(2)).is_err());
}
