//! A Box whose config asked for extra `github-api` entries may use them; a Box without them may not.

use moor_core::config::proxy::{resolve_rules, ProxyConfig, ResolvedRule, ToolRule};
use moor_core::seam::forge::ForgeRepo;
use ureq::Agent;

use super::graphql_e2e::{rig_spec, Rig, SECRET};

const COMMENT: &str = "/repos/o/app/issues/comments/7";
const NEW_COMMENT: &str = "/repos/o/app/issues/7/comments";
const STATUS: &str = "/repos/o/app/statuses/abc123";

fn app() -> ForgeRepo {
    ForgeRepo {
        owner: "o".to_owned(),
        name: "app".to_owned(),
    }
}

/// The `github-api` rule of a Box on `o/app`, with `extra` entries added the way a plan adds them.
fn rules(extra: &[&str]) -> Vec<ResolvedRule> {
    let mut config = ProxyConfig::default();
    config.rules.insert(
        "github-api".to_owned(),
        ToolRule {
            allow: extra.iter().map(|e| (*e).to_owned()).collect(),
            ..ToolRule::default()
        },
    );
    resolve_rules(&config, &[app()], false)
        .unwrap()
        .into_iter()
        .filter(|r| r.name == "github-api")
        .collect()
}

fn rig(extra: &[&str]) -> Rig {
    rig_spec(app(), Vec::new(), rules(extra), None)
}

/// Sends `method path` to `api.github.com` with the Box placeholder; the status.
fn send(client: &Agent, method: &str, path: &str) -> u16 {
    let url = format!("https://api.github.com{path}");
    let req = ureq::http::Request::builder()
        .method(method)
        .uri(url)
        .header("Authorization", "token placeholder-not-real")
        .body("{}")
        .unwrap();
    client.run(req).unwrap().status().as_u16()
}

fn refused_recorded(rig: &Rig) -> bool {
    for _ in 0..100 {
        rig.run.drain();
        if rig
            .run
            .events()
            .iter()
            .any(|(k, _)| k == "credential_withheld" || k == "egress_refused")
        {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    false
}

#[test]
fn box_a_with_the_entries_may_patch_a_comment_and_set_a_status() {
    let a = rig(&[
        "PATCH /repos/{repo}/issues/comments/*",
        "POST /repos/{repo}/statuses/*",
    ]);
    let client = a.client();
    assert_eq!(send(&client, "PATCH", COMMENT), 200);
    assert_eq!(send(&client, "POST", STATUS), 200);
    let seen = a.seen();
    assert_eq!(seen.len(), 2, "{seen:?}");
    for s in &seen {
        assert_eq!(
            s.authorization,
            Some(format!("token {SECRET}")),
            "{}",
            s.line
        );
    }
    // Only this repo, only these methods.
    assert_eq!(
        send(&client, "PATCH", "/repos/o/other/issues/comments/7"),
        401
    );
    assert_eq!(send(&client, "DELETE", COMMENT), 401);
    assert_eq!(
        send(&client, "PATCH", "/repos/o/app/pulls/1/comments/7"),
        401
    );
    a.stop();
}

#[test]
fn box_b_without_them_is_refused_and_an_event_fires() {
    let b = rig(&[]);
    let client = b.client();
    assert_eq!(send(&client, "PATCH", COMMENT), 401);
    assert_eq!(send(&client, "POST", STATUS), 401);
    assert!(
        b.seen().iter().all(|s| s.authorization.is_none()),
        "{:?}",
        b.seen()
    );
    assert!(refused_recorded(&b), "no refusal event");
    b.stop();
}

fn withheld_recorded(rig: &Rig) -> bool {
    for _ in 0..100 {
        rig.run.drain();
        if rig
            .run
            .events()
            .iter()
            .any(|(k, _)| k == "credential_withheld")
        {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    false
}

#[test]
fn a_box_without_the_entry_may_not_post_an_issue_or_pr_comment() {
    let b = rig(&[]);
    let client = b.client();
    assert_eq!(send(&client, "POST", NEW_COMMENT), 401);
    assert_eq!(send(&client, "POST", "/repos/o/app/pulls/7/comments"), 401);
    // Reading comments stays allowed.
    assert_eq!(send(&client, "GET", NEW_COMMENT), 200);
    let with_token: Vec<_> = b
        .seen()
        .into_iter()
        .filter(|s| s.authorization.is_some())
        .collect();
    assert_eq!(with_token.len(), 1, "{with_token:?}");
    assert!(with_token[0].line.starts_with("GET "), "{with_token:?}");
    assert!(withheld_recorded(&b), "no credential_withheld event");
    b.stop();
}

#[test]
fn a_box_with_the_entry_may_post_a_comment_on_its_own_repo_only() {
    let a = rig(&["POST /repos/{repo}/issues/*/comments"]);
    let client = a.client();
    assert_eq!(send(&client, "POST", NEW_COMMENT), 200);
    assert_eq!(
        send(&client, "POST", "/repos/o/other/issues/7/comments"),
        401
    );
    assert_eq!(send(&client, "PATCH", COMMENT), 401);
    let with_token: Vec<_> = a
        .seen()
        .into_iter()
        .filter(|s| s.authorization.is_some())
        .collect();
    assert_eq!(with_token.len(), 1, "{with_token:?}");
    assert_eq!(with_token[0].authorization, Some(format!("token {SECRET}")));
    a.stop();
}
