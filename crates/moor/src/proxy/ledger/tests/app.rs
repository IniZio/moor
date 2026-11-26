//! The App credential: the controller mints a token for the Box's repo, keeps it for its
//! hour, and refuses with the reason when it cannot.

use std::collections::BTreeMap;
use std::sync::Arc;

use moor_core::config::proxy::{ResolvedInject, ResolvedRule, APP_CREDENTIAL};
use moor_core::domain::decision::{Decision, NewBox, SpecFields};
use moor_core::seam::forge::ForgeRepo;
use moor_core::seam::proxy_host::ProxySpec;
use moor_core::user::SecretName;
use moor_core::{BoxId, BoxName};
use serde_json::{json, Value};

use super::super::{serve, AppTokens, BoxLedger, Endpoint, Keys, LedgerError};
use super::{world, World, EGRESS_KEY};
use crate::adapters::github::app::fake::FakeGitHub;
use crate::adapters::github::app::AppClient;
use crate::testing::rng::FakeRng;

/// A Box of `w`'s user on `repo` whose spec has a rule injecting the App's token.
fn app_box(w: &World, repo: Option<ForgeRepo>) -> BoxId {
    let nt = NewBox {
        name: BoxName::parse_allow_bare("a").unwrap(),
        repo: None,
        spec: SpecFields::default(),
    };
    let id = w
        .store
        .apply(Decision::insert(nt).requester(w.ann))
        .unwrap()
        .box_id;
    drop(w.store.reserve(id, &FakeRng::default()));
    let rule = ResolvedRule {
        graphql: None,
        name: "github-api-app".to_owned(),
        hosts: vec!["api.github.com".to_owned()],
        credential: APP_CREDENTIAL.to_owned(),
        inject: ResolvedInject::Header {
            header: "Authorization".to_owned(),
            form: "token {secret}".to_owned(),
        },
        endpoint_rules: Vec::new(),
        box_env: BTreeMap::new(),
    };
    let spec = ProxySpec {
        box_id: id,
        box_handle: None,
        box_repo: repo,
        other_repos: Vec::new(),
        rules: vec![rule],
        user: w.ann,
        hands_inc: None,
    };
    w.store.set_proxy_spec(id, &spec).unwrap();
    id
}

/// Serves `w`'s ledger with `app` as the controller's App, on a thread per connection.
fn serve_with(w: &World, app: Option<AppClient>) {
    let listener = std::os::unix::net::UnixListener::bind(&w.socket).unwrap();
    let (store, vault, app) = (w.store.clone(), Arc::clone(&w.vault), app.map(Arc::new));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            let (store, vault, app) = (store.clone(), Arc::clone(&vault), app.clone());
            std::thread::spawn(move || {
                let keys = Keys {
                    egress: Some(EGRESS_KEY),
                };
                let app = app.as_deref().map(|a| a as &dyn AppTokens);
                serve((&store, &vault, app), &keys, stream);
            });
        }
    });
}

/// A fake GitHub with the App installed on `acme/*` alone, minting `ghs_minted`.
fn github() -> FakeGitHub {
    FakeGitHub::start(|method, path| {
        let parts: Vec<&str> = path.split('/').collect();
        match (method, parts.as_slice()) {
            ("GET", ["", "repos", "acme", _, "installation"]) => (200, json!({"id": 9})),
            ("POST", ["", "app", "installations", "9", "access_tokens"]) => (
                201,
                json!({"token": "ghs_minted", "expires_at": "2026-01-01T01:00:00Z"}),
            ),
            _ => (404, json!({"message": "Not Found"})),
        }
    })
}

fn app_secret(w: &World, id: BoxId) -> Result<Option<moor_core::Secret>, LedgerError> {
    let ledger = BoxLedger::egress(Endpoint::Unix(w.socket.clone()), id, EGRESS_KEY.to_owned());
    ledger.secret(&SecretName::parse(APP_CREDENTIAL).unwrap())
}

#[test]
fn the_app_secret_is_a_token_minted_for_the_box_repo_alone_and_kept() {
    let w = world();
    let api = github();
    serve_with(&w, Some(api.client()));
    let id = app_box(&w, Some(ForgeRepo::new("acme", "widgets")));
    let first = app_secret(&w, id).unwrap().unwrap();
    assert_eq!(first.expose(), b"ghs_minted");
    let again = app_secret(&w, id).unwrap().unwrap();
    assert_eq!(again.expose(), b"ghs_minted");
    let mints: Vec<Value> = api
        .seen()
        .into_iter()
        .filter(|s| s.path.ends_with("/access_tokens"))
        .map(|s| serde_json::from_str(&s.body).unwrap())
        .collect();
    assert_eq!(mints.len(), 1, "a second ask reuses the token: {mints:?}");
    assert_eq!(
        mints.first(),
        Some(&json!({
            "repositories": ["widgets"],
            "permissions": {
                "pull_requests": "write",
                "statuses": "write",
                "metadata": "read",
            },
        }))
    );
}

#[test]
fn a_repo_without_the_app_is_refused_with_the_reason() {
    let w = world();
    let api = github();
    serve_with(&w, Some(api.client()));
    let id = app_box(&w, Some(ForgeRepo::new("other-org", "widgets")));
    let refused = app_secret(&w, id).err();
    assert!(
        matches!(&refused, Some(LedgerError::Refused(why)) if why.contains("not installed on other-org/widgets")),
        "{refused:?}"
    );
}

#[test]
fn a_controller_without_an_app_or_a_box_without_a_repo_is_refused() {
    let w = world();
    serve_with(&w, None);
    let id = app_box(&w, Some(ForgeRepo::new("acme", "widgets")));
    assert!(
        matches!(app_secret(&w, id), Err(LedgerError::Refused(why)) if why.contains("no GitHub App"))
    );
    let w = world();
    serve_with(&w, Some(github().client()));
    let id = app_box(&w, None);
    assert!(
        matches!(app_secret(&w, id), Err(LedgerError::Refused(why)) if why.contains("has no repo"))
    );
}
