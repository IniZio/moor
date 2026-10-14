//! Tests for the nono config builder.

mod e2e;
mod passthrough;
pub(crate) mod upstream;

use moor_core::config::proxy::{EndpointRule, ResolvedInject, ResolvedRule};
use moor_core::{Secret, TaskId};
use nono_proxy::config::InjectMode;

use super::*;

const NONCE: &str = "nonce-text-ABC_123-xyz";

fn rule(name: &str, hosts: &[&str], inject: ResolvedInject) -> ResolvedRule {
    ResolvedRule {
        name: name.to_owned(),
        hosts: hosts.iter().map(|h| (*h).to_owned()).collect(),
        credential: "key".to_owned(),
        inject,
        endpoint_rules: vec![EndpointRule {
            method: "GET".to_owned(),
            path: "/v1/*/items/**".to_owned(),
        }],
        box_env: std::collections::BTreeMap::new(),
    }
}

fn bearer() -> ResolvedInject {
    ResolvedInject::Header {
        header: "Authorization".to_owned(),
        form: "Bearer {secret}".to_owned(),
    }
}

fn cfg_for(rules: &[ResolvedRule], allow: &[String], upstream_ca: Option<&Path>) -> NonoConfig {
    let state = tempfile::tempdir().unwrap();
    let ca = ca::load_or_create(state.path(), TaskId::new(3)).unwrap();
    let dir = ca::ca_dir(state.path(), TaskId::new(3));
    let nonce = Secret::new(NONCE.as_bytes().to_vec());
    build_config(&NonoSpec {
        rules,
        allow,
        nonce: &nonce,
        ca: &ca,
        ca_dir: &dir,
        upstream_ca,
    })
}

#[test]
fn prefixes_round_trip() {
    assert_eq!(route_prefix("gh", 2), "gh__2");
    assert_eq!(rule_of_route("gh__2"), Some("gh"));
    assert_eq!(rule_of_route(&route_prefix("a__b", 0)), Some("a__b"));
    assert_eq!(rule_of_route("plain"), None);
}

#[test]
fn one_route_per_rule_and_host() {
    let rules = [
        rule("gh", &["api.github.com", "github.com"], bearer()),
        rule("ant", &["api.anthropic.com"], bearer()),
    ];
    let cfg = cfg_for(&rules, &[], None);
    let routes = &cfg.inner().routes;
    let got: Vec<_> = routes
        .iter()
        .map(|r| {
            (
                r.prefix.as_str(),
                r.upstream.as_str(),
                r.credential_key.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("gh__0", "https://api.github.com", Some("cmd://gh")),
            ("gh__1", "https://github.com", Some("cmd://gh")),
            ("ant__0", "https://api.anthropic.com", Some("cmd://ant")),
        ]
    );
    let first = routes.first().unwrap().endpoint_rules.first().unwrap();
    assert_eq!(
        (first.method.as_str(), first.path.as_str()),
        ("GET", "/v1/*/items/**")
    );
    assert!(routes.iter().all(|r| rule_of_route(&r.prefix).is_some()));
}

#[test]
fn flags_are_hardened() {
    let cfg = cfg_for(&[], &[], None);
    let c = cfg.inner();
    assert!(c.strict_filter && c.require_auth && c.strict_connect_auth);
    assert!(!c.enable_h2);
    assert!(c.enable_network_audit);
    assert_eq!(c.bind_addr.to_string(), "127.0.0.1");
    assert_eq!(c.bind_port, 0);
    assert_eq!(c.session_token.as_deref().map(String::as_str), Some(NONCE));
    assert!(c.intercept_ca_dir.is_some() && c.preloaded_ca.is_some());
}

#[test]
fn header_form_maps_to_nono_format() {
    let token = ResolvedInject::Header {
        header: "Private-Token".to_owned(),
        form: "token {secret}".to_owned(),
    };
    let rules = [
        rule("a", &["a.test"], bearer()),
        rule("b", &["b.test"], token),
    ];
    let cfg = cfg_for(&rules, &[], None);
    let got: Vec<_> = cfg
        .inner()
        .routes
        .iter()
        .map(|r| {
            (
                &r.inject_mode,
                r.inject_header.as_str(),
                r.credential_format.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            (&InjectMode::Header, "Authorization", Some("Bearer {}")),
            (&InjectMode::Header, "Private-Token", Some("token {}")),
        ]
    );
}

#[test]
fn basic_auth_maps_to_basic_mode() {
    let basic = ResolvedInject::BasicAuth {
        user: "x-access-token".to_owned(),
    };
    let cfg = cfg_for(&[rule("g", &["g.test"], basic)], &[], None);
    let r = cfg.inner().routes.first().unwrap();
    assert_eq!(r.inject_mode, InjectMode::BasicAuth);
    assert_eq!(r.inject_header, "Authorization");
}

#[test]
fn allowed_hosts_union_rule_hosts_and_extras() {
    let rules = [rule("a", &["127.0.0.1:4433", "a.test"], bearer())];
    let allow = vec!["pypi.org".to_owned(), "a.test".to_owned()];
    let cfg = cfg_for(&rules, &allow, None);
    assert_eq!(
        cfg.inner().allowed_hosts,
        ["127.0.0.1:4433", "a.test", "pypi.org"]
    );
    assert_eq!(
        cfg.inner().routes.first().unwrap().upstream,
        "https://127.0.0.1:4433"
    );
}

#[test]
fn upstream_ca_applies_to_every_route() {
    let rules = [rule("a", &["a.test", "b.test"], bearer())];
    let cfg = cfg_for(&rules, &[], Some(Path::new("/tmp/up.pem")));
    assert!(cfg
        .inner()
        .routes
        .iter()
        .all(|r| r.tls_ca.as_deref() == Some("/tmp/up.pem")));
}

#[test]
fn debug_hides_the_nonce() {
    let cfg = cfg_for(&[rule("a", &["a.test"], bearer())], &[], None);
    let text = format!("{cfg:?}");
    assert!(!text.contains(NONCE), "{text}");
    assert!(text.contains("a.test"));
}

#[test]
fn non_utf8_nonce_is_refused_without_echoing_it() {
    let state = tempfile::tempdir().unwrap();
    let ca = ca::load_or_create(state.path(), TaskId::new(3)).unwrap();
    let dir = ca::ca_dir(state.path(), TaskId::new(3));
    let nonce = Secret::new(vec![0xff, 0xfe, b'k']);
    let cfg = build_config(&NonoSpec {
        rules: &[],
        allow: &[],
        nonce: &nonce,
        ca: &ca,
        ca_dir: &dir,
        upstream_ca: None,
    });
    assert!(cfg.inner().session_token.is_none());
    let capture = Arc::new(capture::MoorCapture::new(
        std::collections::BTreeMap::new(),
        Arc::new(crate::testing::clock::FakeClock::new(
            moor_core::seam::clock::Ts::from_millis(0),
        )),
    ));
    let err = NonoProxy::start(cfg, capture).unwrap_err();
    assert_eq!(err.to_string(), "proxy start: session token is not utf-8");
}
