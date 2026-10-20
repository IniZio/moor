//! The default GitHub uploads rule through the embedded proxy, its host moved to a loopback fake.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use moor_core::config::proxy::{resolve_rules, ProxyConfig, ResolvedRule};
use moor_core::config::{CredentialSource, Identity};
use moor_core::seam::clock::Ts;
use moor_core::{BoxId, Secret};
use tempfile::TempDir;

use super::e2e::Recorder;
use super::upstream::{self, Seen};
use crate::proxy::nono::audit::{self, Withheld};
use crate::proxy::nono::capture::{MoorCapture, RuleSource};
use crate::proxy::nono::{build_config, ca, NonoProxy, NonoSpec};
use crate::testing::clock::FakeClock;

const SECRET: &str = "gho_r3al-forge-token-9c1e";
const TOKEN: &str = "token_AbCdEf-0123456789";
const PLACEHOLDER: &str = "token gho_moor-proxy";
const BOX_ID: BoxId = BoxId::new(7);

/// The resolved default `github-uploads` rule with its host set to `host`.
pub(super) fn uploads_rule(host: String) -> ResolvedRule {
    let creds = BTreeMap::from([(
        "CLAUDE_CODE_OAUTH_TOKEN".to_owned(),
        CredentialSource {
            env: Some("X".into()),
            ..Default::default()
        },
    )]);
    let rules = resolve_rules(
        &ProxyConfig::default(),
        &creds,
        Some(("o", "r")),
        Identity::You,
    )
    .unwrap();
    let mut rule = rules
        .into_iter()
        .find(|r| r.name == "github-uploads")
        .unwrap();
    rule.hosts = vec![host];
    rule
}

/// Sends `method path` with the Box's gh placeholder through the proxy.
///
/// Returns what the upstream saw and the proxy's audit calls.
fn send(method: &str, path: &str) -> (Vec<Seen>, Vec<String>) {
    let dir = TempDir::new().unwrap();
    let up = upstream::start(dir.path());
    let rules = vec![uploads_rule(format!("localhost:{}", up.port))];
    let source = RuleSource {
        config_key: "github".to_owned(),
        fetch: Arc::new(|| Ok(Secret::new(SECRET.as_bytes().to_vec()))),
        basic_user: None,
    };
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000_000)));
    let capture = MoorCapture::new(
        BTreeMap::from([("github-uploads".to_owned(), source)]),
        clock,
    );
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
    });
    cfg.0.upstream_extra_ca_pems = vec![up.ca_path.to_string_lossy().into_owned()];
    let capture = Arc::new(capture);
    let proxy = NonoProxy::start(cfg, Arc::clone(&capture)).unwrap();
    let agent = upstream::client(proxy.port(), TOKEN, ca.cert_pem());
    let url = format!("https://localhost:{}{path}", up.port);
    let status = match method {
        "GET" => agent.get(&url).header("Authorization", PLACEHOLDER).call(),
        "PUT" => agent
            .put(&url)
            .header("Authorization", PLACEHOLDER)
            .send_empty(),
        _ => agent
            .post(&url)
            .header("Authorization", PLACEHOLDER)
            .send_empty(),
    }
    .map(|r| r.status().as_u16());
    assert_eq!(status.ok(), Some(200), "{method} {path}");
    let (withheld, sink) = (Withheld::new(&rules, None), Recorder::default());
    for _ in 0..100 {
        audit::apply(&proxy.drain_audit(), &capture, &withheld, &sink);
        if sink
            .calls()
            .iter()
            .any(|c| !c.starts_with("allowed(CONNECT,"))
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    (up.seen(), sink.calls())
}

#[test]
fn an_asset_post_reaches_upstream_with_the_real_token() {
    let (seen, log) = send(
        "POST",
        "/user-attachments/assets?name=a.png&content_type=image%2Fpng&repository_id=1",
    );
    let [only] = seen.as_slice() else {
        unreachable!("{seen:?}")
    };
    assert_eq!(
        only.authorization.as_deref(),
        Some(format!("token {SECRET}").as_str())
    );
    assert!(!only.head.contains("gho_moor-proxy"), "{}", only.head);
    assert!(
        log.iter()
            .any(|l| l.starts_with("allowed(POST, github-uploads, 200")),
        "{log:?}"
    );
}

#[test]
fn any_other_method_or_path_is_withheld_without_the_placeholder() {
    for (method, path) in [
        ("GET", "/user-attachments/assets"),
        ("PUT", "/user-attachments/assets"),
        ("POST", "/user-attachments/assets/1"),
        ("POST", "/repos/o/r/releases/1/assets"),
    ] {
        let (seen, log) = send(method, path);
        let [only] = seen.as_slice() else {
            unreachable!("{seen:?}")
        };
        assert_eq!(only.authorization, None, "{method} {path}: {}", only.head);
        for leaked in [SECRET, "gho_moor-proxy", TOKEN] {
            assert!(
                !only.head.contains(leaked),
                "{method} {path}: {}",
                only.head
            );
        }
        let want = format!("withheld(localhost, github-uploads, {method})");
        assert!(log.contains(&want), "{method} {path}: {log:?}");
    }
}
