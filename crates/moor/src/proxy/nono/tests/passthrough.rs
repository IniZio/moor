//! Which requests get the credential, which go on without it, and which a front Proxy refuses.

use std::collections::BTreeMap;
use std::sync::Arc;

use moor_core::config::parse_user;
use moor_core::config::proxy::{resolve_rules, ResolvedRule};
use moor_core::{BoxId, Secret};
use tempfile::TempDir;
use ureq::Agent;

use super::upstream::{self, Upstream};
use crate::proxy::nono::capture::{MoorCapture, RuleSource};
use crate::proxy::nono::{build_config, ca, NonoProxy, NonoSpec};

const SECRET: &str = "s3cr3t-credential-value-7f3a";
const TOKEN: &str = "token_AbCdEf-0123456789";
const BOX_ID: BoxId = BoxId::new(7);
/// How npm asks a registry for a scoped package's metadata.
const SCOPED: &str = "/@fortawesome%2ffontawesome-pro";

/// The `hdr` rule as a user config with this `allow` list resolves it, served on `localhost:<port>`.
fn hdr_rule(port: u16, allow: &str) -> ResolvedRule {
    let yaml = format!(
        "proxy:\n  rules:\n    hdr:\n      hosts: [npm.example.com]\n      credential: token\n      \
         inject: {{form: 'Bearer {{secret}}'}}\n      allow: [{allow}]\n"
    );
    let user = parse_user("u.yaml", &yaml).unwrap();
    let mut rule = resolve_rules(&user.proxy, None)
        .unwrap()
        .into_iter()
        .find(|r| r.name == "hdr")
        .unwrap();
    rule.hosts = vec![format!("localhost:{port}")];
    rule
}

/// A running proxy that serves one `hdr` rule in front of a fake upstream.
struct Rig {
    /// Owns every file the rig writes.
    _dir: TempDir,
    /// The fake upstream behind the rule.
    up: Upstream,
    /// Keeps the proxy running.
    proxy: NonoProxy,
    /// A client through the proxy that trusts the Box CA.
    agent: Agent,
}

impl Rig {
    /// Starts the proxy for a rule with `allow`; `only_rules` makes it a front Proxy.
    fn start(allow: &str, only_rules: bool) -> Self {
        let dir = TempDir::new().unwrap();
        let up = upstream::start(dir.path());
        let rules = vec![hdr_rule(up.port, allow)];
        let source = RuleSource {
            config_key: "credentials.TOKEN".to_owned(),
            fetch: Arc::new(|| Ok(Secret::new(SECRET.as_bytes().to_vec()))),
            basic_user: None,
        };
        let capture = Arc::new(MoorCapture::new(BTreeMap::from([(
            "hdr".to_owned(),
            source,
        )])));
        let state = dir.path().join("state");
        let ca = ca::load_or_create(&state, BOX_ID).unwrap();
        let token = Secret::new(TOKEN.as_bytes().to_vec());
        let mut cfg = build_config(&NonoSpec {
            rules: &rules,
            token: &token,
            ca: &ca,
            ca_dir: &ca::ca_dir(&state, BOX_ID),
            upstream_ca: Some(&up.ca_path),
            chain: None,
            only_rules,
        });
        // Stands in for the chained filter's Box CA, trusted for every upstream.
        cfg.0.upstream_extra_ca_pems = vec![up.ca_path.to_string_lossy().into_owned()];
        let proxy = NonoProxy::start(cfg, capture).unwrap();
        let agent = upstream::client(proxy.port(), TOKEN, ca.cert_pem());
        Self {
            _dir: dir,
            up,
            proxy,
            agent,
        }
    }

    /// Sends `GET <url>` with the Box placeholder; status or error text.
    fn get(&self, url: &str) -> Result<u16, String> {
        self.agent
            .get(url)
            .header("Authorization", "Bearer moor-proxy")
            .call()
            .map(|r| r.status().as_u16())
            .map_err(|e| e.to_string())
    }

    /// Sends `GET <path>` to the upstream; returns the status and the one request the upstream saw.
    fn fetch(&self, path: &str) -> (Result<u16, String>, upstream::Seen) {
        let status = self.get(&format!("https://localhost:{}{path}", self.up.port));
        let seen = self.up.seen();
        let [only] = seen.as_slice() else {
            unreachable!("{seen:?}")
        };
        (status, only.clone())
    }
}

/// Sends `GET <path>` through a Box proxy whose rule allows `allow`.
fn fetch(allow: &str, path: &str) -> (Result<u16, String>, upstream::Seen) {
    Rig::start(allow, false).fetch(path)
}

#[test]
fn request_matching_no_endpoint_rule_is_forwarded_not_refused() {
    let (status, seen) = fetch("'GET /ok'", "/release/asset");
    assert_eq!(status, Ok(200));
    assert_eq!(seen.line, "GET /release/asset HTTP/1.1");
}

#[test]
fn unmatched_request_reaches_upstream_without_any_credential() {
    let (status, seen) = fetch("'GET /ok'", "/release/asset");
    assert_eq!(status, Ok(200));
    assert_eq!(seen.authorization, None, "{}", seen.head);
    for leaked in [SECRET, TOKEN, "moor-proxy"] {
        assert!(!seen.head.contains(leaked), "{leaked} in {}", seen.head);
    }
}

#[test]
fn request_matching_an_endpoint_rule_gets_the_credential() {
    let (status, seen) = fetch("'GET /ok'", "/ok");
    assert_eq!(status, Ok(200));
    assert_eq!(seen.line, "GET /ok HTTP/1.1");
    assert_eq!(
        seen.authorization.as_deref(),
        Some(format!("Bearer {SECRET}").as_str())
    );
}

#[test]
fn any_request_rule_injects_the_credential_on_an_encoded_slash() {
    let (status, seen) = fetch("'*'", SCOPED);
    assert_eq!(status, Ok(200));
    assert_eq!(seen.line, format!("GET {SCOPED} HTTP/1.1"));
    assert_eq!(
        seen.authorization.as_deref(),
        Some(format!("Bearer {SECRET}").as_str())
    );
}

#[test]
fn path_scoped_rule_withholds_the_credential_on_an_encoded_slash() {
    let (status, seen) = fetch("'GET /**'", SCOPED);
    assert_eq!(status, Ok(200));
    assert_eq!(seen.line, format!("GET {SCOPED} HTTP/1.1"));
    assert_eq!(seen.authorization, None, "{}", seen.head);
}

#[test]
fn path_scoped_rule_injects_on_the_same_path_without_the_encoding() {
    let (status, seen) = fetch("'GET /**'", "/@fortawesome/fontawesome-pro");
    assert_eq!(status, Ok(200));
    assert_eq!(
        seen.authorization.as_deref(),
        Some(format!("Bearer {SECRET}").as_str())
    );
}

#[test]
fn front_proxy_path_scoped_rule_refuses_an_encoded_slash() {
    let rig = Rig::start("'GET /**'", true);
    let status = rig.get(&format!("https://localhost:{}{SCOPED}", rig.up.port));
    assert!(
        matches!(&status, Ok(403)) || status.as_ref().is_err_and(|e| e.contains("403")),
        "got {status:?}"
    );
    assert!(rig.up.seen().is_empty(), "{:?}", rig.up.seen());
    let (status, seen) = rig.fetch("/@fortawesome/fontawesome-pro");
    assert_eq!(status, Ok(200));
    assert_eq!(
        seen.authorization.as_deref(),
        Some(format!("Bearer {SECRET}").as_str())
    );
}

#[test]
fn front_proxy_any_request_rule_still_refuses_an_unlisted_host() {
    let rig = Rig::start("'*'", true);
    let other = rig.up.twin();
    // Trusts the unlisted host's own CA, so only a blind tunnel to it could succeed.
    let direct = upstream::client(rig.proxy.port(), TOKEN, &other.ca_pem);
    let status = direct
        .get(&format!("https://localhost:{}/anything", other.port))
        .call()
        .map(|r| r.status().as_u16());
    assert!(
        status.is_err() || matches!(status, Ok(403)),
        "got {status:?}"
    );
    assert!(other.seen().is_empty(), "{:?}", other.seen());
    let (status, seen) = rig.fetch(SCOPED);
    assert_eq!(status, Ok(200));
    assert_eq!(
        seen.authorization.as_deref(),
        Some(format!("Bearer {SECRET}").as_str())
    );
}
