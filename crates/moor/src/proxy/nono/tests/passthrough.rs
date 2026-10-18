//! A request no endpoint rule matches is forwarded without the credential.

use std::collections::BTreeMap;
use std::sync::Arc;

use moor_core::config::proxy::{EndpointRule, ResolvedInject, ResolvedRule};
use moor_core::seam::clock::Ts;
use moor_core::{BoxId, Secret};
use tempfile::TempDir;

use super::upstream;
use crate::proxy::nono::capture::{MoorCapture, RuleSource};
use crate::proxy::nono::{build_config, ca, NonoProxy, NonoSpec};
use crate::testing::clock::FakeClock;

const SECRET: &str = "s3cr3t-credential-value-7f3a";
const NONCE: &str = "nonce_AbCdEf-0123456789";
const TASK: BoxId = BoxId::new(7);

/// Sends `GET <path>` with the Box placeholder; returns the status and what the upstream saw.
fn fetch(path: &str) -> (Result<u16, String>, Vec<upstream::Seen>) {
    let dir = TempDir::new().unwrap();
    let up = upstream::start(dir.path());
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000_000)));
    let rules = vec![ResolvedRule {
        name: "hdr".to_owned(),
        hosts: vec![format!("localhost:{}", up.port)],
        credential: "TOKEN".to_owned(),
        inject: ResolvedInject::Header {
            header: "Authorization".to_owned(),
            form: "Bearer {secret}".to_owned(),
        },
        endpoint_rules: vec![EndpointRule {
            method: "GET".to_owned(),
            path: "/ok".to_owned(),
        }],
        box_env: BTreeMap::new(),
    }];
    let source = RuleSource {
        config_key: "credentials.TOKEN".to_owned(),
        fetch: Arc::new(|| Ok(Secret::new(SECRET.as_bytes().to_vec()))),
        basic_user: None,
    };
    let capture = Arc::new(MoorCapture::new(
        BTreeMap::from([("hdr".to_owned(), source)]),
        clock,
    ));
    let state = dir.path().join("state");
    let ca = ca::load_or_create(&state, TASK).unwrap();
    let nonce = Secret::new(NONCE.as_bytes().to_vec());
    let mut cfg = build_config(&NonoSpec {
        rules: &rules,
        nonce: &nonce,
        ca: &ca,
        ca_dir: &ca::ca_dir(&state, TASK),
        upstream_ca: Some(&up.ca_path),
        chain: None,
    });
    // Stands in for the chained filter's Task CA, trusted for every upstream.
    cfg.0.upstream_extra_ca_pems = vec![up.ca_path.to_string_lossy().into_owned()];
    let proxy = NonoProxy::start(cfg, capture).unwrap();
    let agent = upstream::client(proxy.port(), NONCE, ca.cert_pem());
    let status = agent
        .get(&format!("https://localhost:{}{path}", up.port))
        .header("Authorization", "Bearer moor-proxy")
        .call()
        .map(|r| r.status().as_u16())
        .map_err(|e| e.to_string());
    (status, up.seen())
}

#[test]
fn request_matching_no_endpoint_rule_is_forwarded_not_refused() {
    let (status, seen) = fetch("/release/asset");
    assert_eq!(status, Ok(200));
    let [only] = seen.as_slice() else {
        unreachable!("{seen:?}")
    };
    assert_eq!(only.line, "GET /release/asset HTTP/1.1");
}

#[test]
fn unmatched_request_reaches_upstream_without_any_credential() {
    let (status, seen) = fetch("/release/asset");
    assert_eq!(status, Ok(200));
    let [only] = seen.as_slice() else {
        unreachable!("{seen:?}")
    };
    assert_eq!(only.authorization, None, "{}", only.head);
    for leaked in [SECRET, NONCE, "moor-proxy"] {
        assert!(!only.head.contains(leaked), "{leaked} in {}", only.head);
    }
}

#[test]
fn request_matching_an_endpoint_rule_gets_the_credential() {
    let (status, seen) = fetch("/ok");
    assert_eq!(status, Ok(200));
    let last = seen.last().unwrap();
    assert_eq!(last.line, "GET /ok HTTP/1.1");
    assert_eq!(
        last.authorization.as_deref(),
        Some(format!("Bearer {SECRET}").as_str())
    );
}
