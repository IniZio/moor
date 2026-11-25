//! The repo settings routes under `/account/api/repos`, against the testkit issuer and a fake
//! controller that keeps repo configs in memory and lets everyone push except `reader-sub`.

use std::collections::BTreeMap;

use moor_core::response::{ErrorKind, ErrorView, Response};
use serde_json::{json, Value};

use super::login::{get, send, sign_in, Reply};
use super::secrets::{world, World};

/// Each repo's stored config: its text, its revision and who wrote it.
pub(super) type Configs = BTreeMap<String, (String, u64, String)>;

/// The one account that can see a repo and not push to it.
const READER: &str = "reader-sub";

fn refusal(kind: ErrorKind, message: &str) -> Response {
    let view = ErrorView {
        kind,
        message: message.to_owned(),
        hints: vec![],
        context: vec![],
        detail: None,
    };
    Response::err("x", view)
}

/// What the fake controller answers `repo-config-get`, `repo-config-set` and `show` from `who`.
pub(super) fn answer(configs: &mut Configs, cmd: &str, who: &str, inner: &Value) -> Response {
    let repo = inner["repo"].as_str().unwrap_or_default().to_owned();
    let file = inner["file"].as_str().unwrap_or("config").to_owned();
    let key = if file == "config" {
        repo.clone()
    } else {
        format!("{repo}#{file}")
    };
    let can_edit = !who.contains(READER);
    let held = |configs: &Configs| match configs.get(&key) {
        Some((text, revision, by)) => {
            json!({"repo": repo, "file": file, "text": text, "revision": revision, "edited_by": by,
                "edited_at": "2026-11-17T10:00:00Z", "can_edit": can_edit})
        }
        None => {
            json!({"repo": repo, "file": file, "text": null, "revision": 0, "can_edit": can_edit})
        }
    };
    match cmd {
        "show" => Response::ok(
            "x",
            Some(json!({"boxes": [{"repo": "acme/web"},
                {"repo": "acme/api", "other_repos": ["acme/web", "acme/lib"]}]})),
        ),
        "github-app" => Response::ok(
            "x",
            Some(json!({"install_url": "https://github.com/apps/moor",
                "repos": ["acme/other-a", "acme/other-b", "acme/web"]})),
        ),
        "repo-config-get" => Response::ok("x", Some(held(configs))),
        "repo-config-set" if !can_edit => {
            let why = format!("your GitHub account cannot push to {repo}");
            refusal(ErrorKind::Refused, &why)
        }
        "repo-config-set" => {
            let text = inner["text"].as_str().unwrap_or_default();
            if text.contains("bogus") {
                return refusal(ErrorKind::Usage, "unknown key `bogus`");
            }
            let at = configs.get(&key).map_or(0, |c| c.1);
            if inner["expected_revision"].as_u64().is_some_and(|r| r != at) {
                return refusal(ErrorKind::Refused, "it changed after you read it");
            }
            configs.insert(key.clone(), (text.to_owned(), at + 1, who.to_owned()));
            Response::ok("x", Some(held(configs)))
        }
        _ => Response::ok("x", None),
    }
}

fn as_user(w: &World, sub: &str) -> String {
    let claims = json!({"sub": sub, "email": format!("{sub}@example.com"), "email_verified": true});
    *w.issuer.id_claims.lock().unwrap() = claims.clone();
    w.issuer.userinfo.lock().unwrap().1 = claims;
    sign_in(&w.addr)
}

fn csrf(w: &World, cookie: &str) -> String {
    let reply = get(&w.addr, "/account/api", Some(cookie));
    let view: Value = serde_json::from_str(&reply.body).unwrap();
    view["csrf"].as_str().unwrap().to_owned()
}

fn read(w: &World, cookie: &str, repo: &str) -> Reply {
    get(&w.addr, &format!("/account/api/repos/{repo}"), Some(cookie))
}

fn json_of(reply: &Reply) -> Value {
    serde_json::from_str(&reply.body).unwrap_or_else(|_| panic!("not JSON: {}", reply.body))
}

/// Writes `text` to `repo` as the page of `cookie` would, at `revision`.
fn write(w: &World, cookie: &str, repo: &str, text: &str, revision: u64) -> Reply {
    let extra = format!(
        "Content-Type: application/json\r\nCookie: {cookie}\r\nX-Moor-Csrf: {}\r\n",
        csrf(w, cookie)
    );
    let body = json!({"text": text, "revision": revision}).to_string();
    send(
        &w.addr,
        "PUT",
        &format!("/account/api/repos/{repo}"),
        &extra,
        &body,
    )
}

#[test]
fn the_list_holds_each_repo_of_the_persons_boxes_once_sorted() {
    let w = world(true, &[]);
    let cookie = sign_in(&w.addr);
    let reply = get(&w.addr, "/account/api/repos", Some(&cookie));
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(
        json_of(&reply),
        json!({"repos": ["acme/api", "acme/lib", "acme/web"]})
    );
    assert_eq!(get(&w.addr, "/account/api/repos", None).status, 401);
}

#[test]
fn the_app_lists_only_the_installed_repos_the_person_has_in_moor() {
    let w = world(true, &[]);
    let cookie = sign_in(&w.addr);
    let reply = get(&w.addr, "/account/api/github-app", Some(&cookie));
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(
        json_of(&reply),
        json!({"install_url": "https://github.com/apps/moor", "repos": ["acme/web"]})
    );
    assert!(!reply.body.contains("other-"), "{}", reply.body);
}

#[test]
fn a_repo_reads_as_none_then_as_the_text_saved_and_who_saved_it() {
    let w = world(true, &[]);
    let cookie = as_user(&w, "alice-sub");
    let none = read(&w, &cookie, "acme/web");
    assert_eq!(none.status, 200, "{}", none.body);
    assert_eq!(json_of(&none)["revision"], 0);
    assert_eq!(json_of(&none)["can_edit"], true);
    let saved = write(&w, &cookie, "acme/web", "setup: [make]\n", 0);
    assert_eq!(saved.status, 200, "{}", saved.body);
    let now = json_of(&read(&w, &cookie, "acme/web"));
    assert_eq!(now["text"], "setup: [make]\n");
    assert_eq!(now["revision"], 1);
    assert!(now["edited_by"].as_str().unwrap().contains("alice-sub"));
    assert_eq!(read(&w, "moor_session=x", "acme/web").status, 401);
}

#[test]
fn a_write_at_a_revision_that_has_moved_is_a_conflict_that_names_who_changed_it() {
    let w = world(true, &[]);
    let alice = as_user(&w, "alice-sub");
    let bob = as_user(&w, "bob-sub");
    assert_eq!(write(&w, &alice, "acme/web", "setup: [a]\n", 0).status, 200);
    let stale = write(&w, &bob, "acme/web", "setup: [b]\n", 0);
    assert_eq!(stale.status, 409, "{}", stale.body);
    let body = json_of(&stale);
    let error = body["error"].as_str().unwrap();
    assert!(
        error.contains("alice-sub") && error.contains("2026-11-17T10:00:00Z"),
        "{error}"
    );
    assert_eq!(body["current"]["revision"], 1);
    assert_eq!(json_of(&read(&w, &bob, "acme/web"))["text"], "setup: [a]\n");
    assert_eq!(write(&w, &bob, "acme/web", "setup: [b]\n", 1).status, 200);
}

#[test]
fn text_that_does_not_parse_answers_the_controllers_reason_and_stores_nothing() {
    let w = world(true, &[]);
    let cookie = as_user(&w, "alice-sub");
    let reply = write(&w, &cookie, "acme/web", "bogus: 1\n", 0);
    assert_eq!(reply.status, 400, "{}", reply.body);
    assert!(json_of(&reply)["error"].as_str().unwrap().contains("bogus"));
    assert_eq!(json_of(&read(&w, &cookie, "acme/web"))["revision"], 0);
}

#[test]
fn an_account_that_cannot_push_reads_without_edit_and_is_refused_a_write() {
    let w = world(true, &[]);
    let cookie = as_user(&w, READER);
    assert_eq!(json_of(&read(&w, &cookie, "acme/web"))["can_edit"], false);
    let reply = write(&w, &cookie, "acme/web", "setup: [a]\n", 0);
    assert_eq!(reply.status, 403, "{}", reply.body);
    assert!(json_of(&reply)["error"]
        .as_str()
        .unwrap()
        .contains("cannot push"));
    assert!(w.configs.lock().unwrap().is_empty());
}

#[test]
fn a_write_needs_the_csrf_header_a_revision_and_a_repo_name() {
    let w = world(true, &[]);
    let cookie = as_user(&w, "alice-sub");
    let body = json!({"text": "setup: [a]\n", "revision": 0}).to_string();
    for csrf in ["", "X-Moor-Csrf: forged\r\n"] {
        let extra = format!("Content-Type: application/json\r\nCookie: {cookie}\r\n{csrf}");
        let reply = send(&w.addr, "PUT", "/account/api/repos/acme/web", &extra, &body);
        assert_eq!(reply.status, 403, "{}", reply.body);
    }
    let extra = "Content-Type: application/json\r\n".to_owned();
    assert_eq!(
        send(&w.addr, "PUT", "/account/api/repos/acme/web", &extra, &body).status,
        401
    );
    let extra = format!(
        "Content-Type: application/json\r\nCookie: {cookie}\r\nX-Moor-Csrf: {}\r\n",
        csrf(&w, &cookie)
    );
    for (path, body) in [
        ("/account/api/repos/acme/web", r#"{"text": "setup: [a]\n"}"#),
        ("/account/api/repos/%20/x", "{}"),
    ] {
        let reply = send(&w.addr, "PUT", path, &extra, body);
        assert_eq!(reply.status, 400, "{path}: {}", reply.body);
    }
    assert!(w.configs.lock().unwrap().is_empty());
}

#[test]
fn a_repos_rules_read_and_write_under_rules_apart_from_its_config() {
    let w = world(true, &[]);
    let cookie = as_user(&w, "alice-sub");
    let rules = "rules:\n  - hosts: [api.github.com]\n";
    let saved = write(&w, &cookie, "acme/web/rules", rules, 0);
    assert_eq!(saved.status, 200, "{}", saved.body);
    let now = json_of(&read(&w, &cookie, "acme/web/rules"));
    assert_eq!(now["file"], "rules");
    assert_eq!(now["text"], rules);
    assert_eq!(now["revision"], 1);
    let config = json_of(&read(&w, &cookie, "acme/web"));
    assert_eq!(config["revision"], 0, "the config is untouched: {config}");
    let stale = write(&w, &cookie, "acme/web/rules", rules, 0);
    assert_eq!(stale.status, 409, "{}", stale.body);
    assert_ne!(read(&w, &cookie, "acme/web/other").status, 200);
    assert_eq!(json_of(&read(&w, &cookie, "acme/rules"))["file"], "config");
}
