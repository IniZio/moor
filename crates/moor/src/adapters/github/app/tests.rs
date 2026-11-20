//! The App client against a fake GitHub on loopback, with an RSA key made for each test run.
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use moor_core::config::GitHubAppConfig;
use moor_core::seam::clock::Ts;
use moor_core::seam::forge::ForgeRepo;
use moor_core::Secret;
use serde_json::{json, Value};

use super::pull::Permission;
use super::{AppClient, Installation};
use crate::adapters::github::app::fake::{key, FakeGitHub, Seen};

mod live;

/// Unix seconds the tests sign at.
const NOW: i64 = 1_800_000_000;

fn now() -> Ts {
    Ts::from_millis(NOW * 1000)
}

/// The claims of an App JWT `auth` carries, checked against the test key's public half.
fn claims(auth: &str) -> Value {
    let jwt = auth.strip_prefix("Bearer ").unwrap();
    let key = DecodingKey::from_rsa_raw_components(&key().n, &key().e);
    let mut validation = Validation::new(Algorithm::RS256);
    validation.validate_exp = false;
    validation.set_required_spec_claims(&["iss", "iat", "exp"]);
    decode::<Value>(jwt, &key, &validation).unwrap().claims
}

#[test]
fn the_app_jwt_is_rs256_from_the_app_key_naming_its_id_for_under_ten_minutes() {
    let api = FakeGitHub::start(|_, _| {
        (
            200,
            json!({"html_url": "https://github.com/apps/moor-test"}),
        )
    });
    api.client().install_url(now()).unwrap();
    let seen = api.seen();
    let c = claims(&seen[0].auth);
    assert_eq!(c["iss"], "42");
    assert_eq!(c["iat"], NOW - 60);
    let life = c["exp"].as_i64().unwrap() - NOW;
    assert!(life > 0 && life <= 600, "{life}");
}

#[test]
fn the_install_url_is_the_app_page_and_read_once() {
    let api = FakeGitHub::start(|_, _| {
        (
            200,
            json!({"html_url": "https://github.com/apps/moor-test"}),
        )
    });
    let app = api.client();
    for _ in 0..2 {
        assert_eq!(
            app.install_url(now()).unwrap(),
            "https://github.com/apps/moor-test/installations/new"
        );
    }
    assert_eq!(api.seen().len(), 1);
    assert_eq!(api.seen()[0].path, "/app");
}

#[test]
fn installed_lists_every_installation_repos_with_read_only_tokens() {
    let api = FakeGitHub::start(|method, path| match (method, path) {
        ("GET", "/app") => (
            200,
            json!({"html_url": "https://github.com/apps/moor-test"}),
        ),
        ("GET", "/app/installations?per_page=100") => (200, json!([{"id": 7}, {"id": 8}])),
        ("POST", "/app/installations/7/access_tokens") => (201, json!({"token": "ghs_seven"})),
        ("POST", "/app/installations/8/access_tokens") => (201, json!({"token": "ghs_eight"})),
        ("GET", "/installation/repositories?per_page=100") => (
            200,
            json!({"repositories": [{"full_name": "acme/web"}, {"full_name": "acme/api"}]}),
        ),
        _ => (404, json!({})),
    });
    let installed = api.client().installed(now()).unwrap();
    assert_eq!(
        installed.install_url,
        "https://github.com/apps/moor-test/installations/new"
    );
    assert_eq!(
        installed.repos,
        vec![ForgeRepo::new("acme", "api"), ForgeRepo::new("acme", "web")]
    );
    let seen = api.seen();
    let mints: Vec<&Seen> = seen.iter().filter(|s| s.method == "POST").collect();
    assert_eq!(mints.len(), 2);
    for mint in mints {
        assert_eq!(claims(&mint.auth)["iss"], "42");
        let asked: Value = serde_json::from_str(&mint.body).unwrap();
        assert_eq!(
            asked,
            json!({"permissions": {"metadata": "read", "pull_requests": "read"}})
        );
    }
    let lists: Vec<&str> = seen
        .iter()
        .filter(|s| s.path.starts_with("/installation/"))
        .map(|s| s.auth.as_str())
        .collect();
    assert_eq!(lists, ["Bearer ghs_seven", "Bearer ghs_eight"]);
}

#[test]
fn a_repo_without_the_app_has_no_installation() {
    let api = FakeGitHub::start(|_, path| match path {
        "/repos/acme/web/installation" => (200, json!({"id": 7, "account": {"login": "acme"}})),
        _ => (404, json!({"message": "Not Found"})),
    });
    let app = api.client();
    assert_eq!(
        app.installation_of(&ForgeRepo::new("acme", "web"), now())
            .unwrap(),
        Some(Installation { id: 7 })
    );
    assert_eq!(
        app.installation_of(&ForgeRepo::new("acme", "none"), now())
            .unwrap(),
        None
    );
}

/// A fake with two pull requests and two collaborators on `acme/web`.
fn pulls_api() -> FakeGitHub {
    FakeGitHub::start(|_, path| match path {
        "/repos/acme/web/pulls/5" => (
            200,
            json!({
                "state": "open", "draft": true,
                "head": {"sha": "a".repeat(40), "repo": null},
                "user": {"login": "dependabot[bot]", "type": "Bot"},
            }),
        ),
        "/repos/acme/web/pulls/6" => (
            200,
            json!({
                "state": "closed", "draft": false,
                "head": {"sha": "b".repeat(40), "repo": {"full_name": "alice/web"}},
                "user": {"login": "alice", "type": "User"},
            }),
        ),
        "/repos/acme/web/pulls/8" => (
            200,
            json!({
                "state": "open", "draft": false,
                "head": {"sha": "c".repeat(40), "repo": {"full_name": "acme/web"}},
                "user": {"login": "renovate[bot]", "type": "User"},
            }),
        ),
        "/repos/acme/web/collaborators/alice/permission" => {
            (200, json!({"permission": "write", "role_name": "maintain"}))
        }
        "/repos/acme/web/collaborators/bob/permission" => (200, json!({"permission": "read"})),
        _ => (404, json!({})),
    })
}

#[test]
fn a_pull_reads_with_the_installation_token() {
    let api = pulls_api();
    let app = api.client();
    let repo = ForgeRepo::new("acme", "web");
    let token = Secret::new(b"ghs_token".to_vec());
    let bot = app.pull(&repo, 5, &token).unwrap();
    assert!(bot.open && bot.draft && bot.author_is_bot);
    assert_eq!(bot.head_repo, None);
    let fork = app.pull(&repo, 6, &token).unwrap();
    assert!(!fork.open && !fork.draft && !fork.author_is_bot);
    assert_eq!(fork.head_sha.as_str(), "b".repeat(40));
    assert_eq!(fork.head_repo, Some(ForgeRepo::new("alice", "web")));
    assert_eq!(fork.author, "alice");
    assert!(app.pull(&repo, 7, &token).unwrap_err().contains("HTTP 404"));
    assert!(
        app.pull(&repo, 8, &token).unwrap().author_is_bot,
        "a [bot] login is a bot whatever its type"
    );
    assert!(api
        .seen()
        .iter()
        .all(|s| s.auth == "Bearer ghs_token" && s.method == "GET"));
}

#[test]
fn a_head_reads_as_the_installation_that_covers_the_repo_or_names_a_repo_without_the_app() {
    let api = FakeGitHub::start(|method, path| match (method, path) {
        ("GET", "/repos/acme/web/installation") => (200, json!({"id": 7})),
        ("POST", "/app/installations/7/access_tokens") => (201, json!({"token": "ghs_seven"})),
        ("GET", "/repos/acme/web/pulls/6") => (
            200,
            json!({
                "state": "open", "draft": false,
                "head": {"sha": "b".repeat(40), "repo": {"full_name": "acme/web"}},
                "user": {"login": "alice", "type": "User"},
            }),
        ),
        _ => (404, json!({})),
    });
    let app = api.client();
    let head = app.head_of(&ForgeRepo::new("acme", "web"), 6, now());
    assert_eq!(head.unwrap().as_str(), "b".repeat(40));
    let pull_read = api
        .seen()
        .into_iter()
        .find(|s| s.path.ends_with("/pulls/6"));
    assert_eq!(pull_read.unwrap().auth, "Bearer ghs_seven");
    let none = app.head_of(&ForgeRepo::new("acme", "none"), 6, now());
    assert_eq!(
        none.unwrap_err(),
        "the GitHub App is not installed on acme/none"
    );
}

#[test]
fn an_author_permission_reads_with_the_installation_token() {
    let api = pulls_api();
    let app = api.client();
    let repo = ForgeRepo::new("acme", "web");
    let token = Secret::new(b"ghs_token".to_vec());
    let alice = app.permission(&repo, "alice", &token).unwrap();
    assert_eq!(alice, Permission::Write);
    assert!(alice.can_write());
    assert!(!app.permission(&repo, "bob", &token).unwrap().can_write());
    assert!(api
        .seen()
        .iter()
        .all(|s| s.auth == "Bearer ghs_token" && s.method == "GET"));
}

#[test]
fn only_a_pkcs1_rsa_key_loads_and_a_refusal_never_shows_the_key() {
    assert!(AppClient::from_pem(1, key().pkcs1.as_bytes(), "http://x").is_ok());
    let broken = pem::encode(&pem::Pem::new("RSA PRIVATE KEY", b"secret".to_vec()));
    for bad in [key().pkcs8.as_str(), broken.as_str(), "c2VjcmV0"] {
        let err = AppClient::from_pem(1, bad.as_bytes(), "http://x")
            .err()
            .unwrap();
        assert!(
            !err.contains("c2VjcmV0") && !err.contains("secret"),
            "{err}"
        );
    }
}

#[test]
fn load_reads_the_key_file() {
    let dir = tempfile::tempdir().unwrap();
    let key_file = dir.path().join("key.pem");
    std::fs::write(&key_file, &key().pkcs1).unwrap();
    let mut config = GitHubAppConfig {
        id: 42,
        private_key_file: key_file,
        webhook_secret_file: None,
    };
    let same = |p: &std::path::Path| p.to_path_buf();
    assert!(AppClient::load(&config, same).is_ok());
    config.private_key_file = dir.path().join("missing.pem");
    let err = AppClient::load(&config, same).err().unwrap();
    assert!(err.contains("missing.pem"), "{err}");
}
