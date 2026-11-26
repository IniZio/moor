//! A Box whose rule lists `app` lines sends them with the App's token and everything else with
//! the user's.

use std::sync::Arc;

use moor_core::config::proxy::{resolve_rules, ProxyConfig, ResolvedRule, ToolRule};
use moor_core::seam::forge::ForgeRepo;
use moor_core::Secret;
use ureq::Agent;

use super::graphql_e2e::{rig_spec_with, Rig};
use crate::proxy::nono::capture::UserSecretsFn;

const USER_TOKEN: &str = "user-token-1";
const APP_TOKEN: &str = "ghs_app-token-2";

fn app() -> ForgeRepo {
    ForgeRepo::new("o", "app")
}

/// The GitHub API rules of a Box on `o/app` whose `github-api` rule lists `lines` as `app`.
fn rules(lines: &[&str]) -> Vec<ResolvedRule> {
    let mut config = ProxyConfig::default();
    config.rules.insert(
        "github-api".to_owned(),
        ToolRule {
            app: lines.iter().map(|e| (*e).to_owned()).collect(),
            ..ToolRule::default()
        },
    );
    resolve_rules(&config, &[app()], false)
        .unwrap()
        .into_iter()
        .filter(|r| r.name.starts_with("github-api"))
        .collect()
}

fn rig(lines: &[&str]) -> Rig {
    let secrets: UserSecretsFn = Arc::new(|name| {
        let token = if name.as_str() == "github-app" {
            APP_TOKEN
        } else {
            USER_TOKEN
        };
        Ok(Some(Secret::new(token.as_bytes().to_vec())))
    });
    rig_spec_with(app(), Vec::new(), rules(lines), None, secrets)
}

fn send(client: &Agent, method: &str, path: &str) -> u16 {
    let req = ureq::http::Request::builder()
        .method(method)
        .uri(format!("https://api.github.com{path}"))
        .header("Authorization", "token placeholder-not-real")
        .body("{}")
        .unwrap();
    client.run(req).unwrap().status().as_u16()
}

#[test]
fn app_lines_go_out_with_the_app_token_and_reads_with_the_users() {
    let a = rig(&[
        "POST /repos/{repo}/issues/*/comments",
        "POST /repos/{repo}/statuses/*",
    ]);
    let client = a.client();
    for (method, path) in [
        ("POST", "/repos/o/app/issues/7/comments"),
        ("POST", "/repos/o/app/statuses/abc123"),
        ("GET", "/repos/o/app/issues/7/comments"),
        ("POST", "/repos/o/other/issues/7/comments"),
    ] {
        send(&client, method, path);
    }
    let auth: Vec<(String, Option<String>)> = a
        .seen()
        .into_iter()
        .map(|s| (s.line, s.authorization))
        .collect();
    let app = Some(format!("token {APP_TOKEN}"));
    let user = Some(format!("token {USER_TOKEN}"));
    let want = [app.clone(), app, user, None];
    assert_eq!(auth.len(), want.len(), "{auth:?}");
    for ((line, got), want) in auth.iter().zip(want) {
        assert_eq!(*got, want, "{line}");
    }
    a.stop();
}
