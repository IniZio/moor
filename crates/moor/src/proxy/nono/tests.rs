//! Tests for the nono config builder.

mod chain;
mod e2e;
mod passthrough;
mod streaming;
mod uploads;
pub(crate) mod upstream;

use moor_core::config::proxy::{EndpointRule, ResolvedInject, ResolvedRule};
use moor_core::{BoxId, Secret};
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

fn cfg_for(rules: &[ResolvedRule], upstream_ca: Option<&Path>) -> NonoConfig {
    cfg_chained(rules, upstream_ca, None)
}

fn cfg_chained(
    rules: &[ResolvedRule],
    upstream_ca: Option<&Path>,
    chain: Option<Chain<'_>>,
) -> NonoConfig {
    let state = tempfile::tempdir().unwrap();
    let ca = ca::load_or_create(state.path(), BoxId::new(3)).unwrap();
    let dir = ca::ca_dir(state.path(), BoxId::new(3));
    let nonce = Secret::new(NONCE.as_bytes().to_vec());
    build_config(&NonoSpec {
        rules,
        nonce: &nonce,
        ca: &ca,
        ca_dir: &dir,
        upstream_ca,
        chain,
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
    let cfg = cfg_for(&rules, None);
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
    assert!(routes.iter().all(|r| r.endpoint_policy.is_none()));
    let first = routes.first().unwrap().endpoint_rules.first().unwrap();
    assert_eq!(
        (first.method.as_str(), first.path.as_str()),
        ("GET", "/v1/*/items/**")
    );
    assert!(routes.iter().all(|r| rule_of_route(&r.prefix).is_some()));
}

#[test]
fn flags_are_hardened() {
    let cfg = cfg_for(&[], None);
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
    let cfg = cfg_for(&rules, None);
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
    let cfg = cfg_for(&[rule("g", &["g.test"], basic)], None);
    let r = cfg.inner().routes.first().unwrap();
    assert_eq!(r.inject_mode, InjectMode::BasicAuth);
    assert_eq!(r.inject_header, "Authorization");
}

#[test]
fn every_host_passes_nono_filter_and_only_rule_hosts_get_routes() {
    let rules = [rule("a", &["127.0.0.1:4433", "a.test"], bearer())];
    let cfg = cfg_for(&rules, None);
    assert_eq!(cfg.inner().allowed_hosts, ["*"]);
    let filter = nono::net_filter::HostFilter::new_strict(&cfg.inner().allowed_hosts);
    assert!(filter.check_host("unlisted.example", &[]).is_allowed());
    assert_eq!(
        cfg.inner().routes.first().unwrap().upstream,
        "https://127.0.0.1:4433"
    );
}

#[test]
fn upstream_ca_applies_to_every_route() {
    let rules = [rule("a", &["a.test", "b.test"], bearer())];
    let cfg = cfg_for(&rules, Some(Path::new("/tmp/up.pem")));
    assert!(cfg
        .inner()
        .routes
        .iter()
        .all(|r| r.tls_ca.as_deref() == Some("/tmp/up.pem")));
}

#[test]
fn debug_hides_the_nonce() {
    let cfg = cfg_for(&[rule("a", &["a.test"], bearer())], None);
    let text = format!("{cfg:?}");
    assert!(!text.contains(NONCE), "{text}");
    assert!(text.contains("allowed_hosts"), "{text}");
}

#[test]
fn non_utf8_nonce_is_refused_without_echoing_it() {
    let state = tempfile::tempdir().unwrap();
    let ca = ca::load_or_create(state.path(), BoxId::new(3)).unwrap();
    let dir = ca::ca_dir(state.path(), BoxId::new(3));
    let nonce = Secret::new(vec![0xff, 0xfe, b'k']);
    let cfg = build_config(&NonoSpec {
        rules: &[],
        nonce: &nonce,
        ca: &ca,
        ca_dir: &dir,
        upstream_ca: None,
        chain: None,
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

fn chain_cfg(rules: &[ResolvedRule]) -> NonoConfig {
    let chain = Chain {
        proxy: "127.0.0.1:9911".parse().unwrap(),
        host: GITHUB_API_HOST,
        ca_pem: Path::new("/tmp/box-ca.pem"),
    };
    cfg_chained(rules, Some(Path::new("/tmp/up.pem")), Some(chain))
}

#[test]
fn no_chain_means_no_external_proxy() {
    let rules = [rule("gh", &[GITHUB_API_HOST], bearer())];
    assert!(cfg_for(&rules, None).inner().external_proxy.is_none());
}

#[test]
fn chain_sends_every_host_through_the_proxy_with_no_bypass() {
    let rules = [
        rule(
            "gh",
            &["api.github.com", "uploads.github.com:443"],
            bearer(),
        ),
        rule("o", &["o.test:8443"], bearer()),
    ];
    let cfg = chain_cfg(&rules);
    let ext = cfg.inner().external_proxy.as_ref().unwrap();
    assert_eq!(ext.address, "127.0.0.1:9911");
    assert!(ext.auth.is_none());
    assert!(ext.bypass_hosts.is_empty());
}

#[test]
fn chained_host_routes_trust_the_box_ca_and_others_keep_upstream() {
    let rules = [
        rule(
            "gh",
            &["api.github.com", "uploads.github.com:443"],
            bearer(),
        ),
        rule("o", &["o.test:8443"], bearer()),
    ];
    let cfg = chain_cfg(&rules);
    let by_host = |h: &str| {
        cfg.inner()
            .routes
            .iter()
            .find(|r| r.upstream == format!("https://{h}"))
            .and_then(|r| r.tls_ca.clone())
    };
    assert_eq!(
        by_host("api.github.com").as_deref(),
        Some("/tmp/box-ca.pem")
    );
    assert_eq!(by_host("o.test:8443").as_deref(), Some("/tmp/up.pem"));
    assert_eq!(
        by_host("uploads.github.com:443").as_deref(),
        Some("/tmp/up.pem")
    );
}

#[test]
fn chained_config_trusts_the_filter_ca_for_every_upstream() {
    let rules = [rule("gh", &[GITHUB_API_HOST], bearer())];
    assert_eq!(
        chain_cfg(&rules).inner().upstream_extra_ca_pems,
        vec!["/tmp/box-ca.pem".to_owned()]
    );
}

#[test]
fn unchained_config_adds_no_extra_ca() {
    let rules = [rule("gh", &[GITHUB_API_HOST], bearer())];
    assert!(cfg_for(&rules, None)
        .inner()
        .upstream_extra_ca_pems
        .is_empty());
}

#[test]
fn github_uploads_route_admits_only_the_asset_post() {
    use moor_core::config::proxy::ProxyConfig;
    use nono_proxy::config::CompiledEndpointRules;

    let src = moor_core::config::CredentialSource {
        env: Some("X".into()),
        ..Default::default()
    };
    let creds = std::collections::BTreeMap::from([("CLAUDE_CODE_OAUTH_TOKEN".to_owned(), src)]);
    let rules = moor_core::config::proxy::resolve_rules(
        &ProxyConfig::default(),
        &creds,
        Some(("o", "w")),
        moor_core::config::Identity::You,
    )
    .unwrap();
    let cfg = cfg_for(&rules, None);
    let route = cfg
        .inner()
        .routes
        .iter()
        .find(|r| r.upstream == "https://uploads.github.com")
        .expect("uploads route");
    let policy = CompiledEndpointRules::compile(&route.endpoint_rules).unwrap();
    assert!(policy.is_allowed("POST", "/user-attachments/assets"));
    assert!(policy.is_allowed(
        "POST",
        "/user-attachments/assets?name=a.png&content_type=image%2Fpng&repository_id=1"
    ));
    for (m, p) in [
        ("GET", "/user-attachments/assets"),
        ("PUT", "/user-attachments/assets"),
        ("DELETE", "/user-attachments/assets"),
        ("POST", "/user-attachments/assets/1"),
        ("POST", "/user-attachments"),
        ("POST", "/repos/o/r/releases/1/assets"),
        ("POST", "/"),
    ] {
        assert!(!policy.is_allowed(m, p), "{m} {p}");
    }
}
