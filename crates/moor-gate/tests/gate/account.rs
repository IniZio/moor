//! The account page at the gate root and the JSON it reads and writes under `/account/api`,
//! against the testkit issuer and the fake controller of the secrets tests.

use serde_json::{json, Value};

use super::login::{get, send, sign_in, Reply};
use super::secrets::{world, World};

const TOKEN: &str = "sk-ant-oat01-s3cret_value";
const API: &str = "/account/api";

fn put(w: &World, cookie: Option<&str>, csrf: Option<&str>, name: &str, body: &str) -> Reply {
    let extra: String = [
        ("Content-Type", Some("application/json")),
        ("Cookie", cookie),
        ("X-Moor-Csrf", csrf),
    ]
    .into_iter()
    .filter_map(|(h, v)| v.map(|v| format!("{h}: {v}\r\n")))
    .collect();
    send(
        &w.addr,
        "PUT",
        &format!("{API}/secrets/{name}"),
        &extra,
        body,
    )
}

fn view(w: &World, cookie: &str) -> Value {
    let reply = get(&w.addr, API, Some(cookie));
    assert_eq!(reply.status, 200, "{}", reply.body);
    serde_json::from_str(&reply.body).unwrap()
}

fn as_user(w: &World, sub: &str) -> String {
    let claims = json!({"sub": sub, "email": format!("{sub}@example.com"), "email_verified": true});
    *w.issuer.id_claims.lock().unwrap() = claims.clone();
    w.issuer.userinfo.lock().unwrap().1 = claims;
    sign_in(&w.addr)
}

fn save(w: &World, cookie: &str, name: &str, value: &str) -> Reply {
    let csrf = view(w, cookie)["csrf"].as_str().unwrap().to_owned();
    put(
        w,
        Some(cookie),
        Some(&csrf),
        name,
        &json!({"value": value}).to_string(),
    )
}

#[test]
fn the_root_sends_a_signed_out_browser_to_sign_in_and_serves_the_app_to_a_signed_in_one() {
    let w = world(true, &[]);
    let out = get(&w.addr, "/", None);
    assert_eq!(out.status, 302, "{}", out.body);
    assert_eq!(out.location(), "/auth/login?return=/");
    let cookie = sign_in(&w.addr);
    let page = get(&w.addr, "/", Some(&cookie));
    assert_eq!(page.status, 200, "{}", page.body);
    assert!(
        page.body.contains("/account/assets/app.js"),
        "{}",
        page.body
    );
    let policy = page.header("content-security-policy").unwrap();
    assert!(policy.contains("script-src 'self'"), "{policy}");
    assert!(policy.contains("connect-src 'self'"), "{policy}");
    assert!(!policy.contains("unsafe-inline'; form"), "{policy}");
    assert_eq!(page.header("cache-control"), Some("no-store"));
    assert!(w.cmds().is_empty(), "{:?}", w.cmds());
}

#[test]
fn the_built_assets_are_served_without_a_session() {
    let w = world(true, &[]);
    for (path, kind) in [
        ("/account/assets/app.js", "text/javascript"),
        ("/account/assets/app.css", "text/css"),
    ] {
        let asset = get(&w.addr, path, None);
        assert_eq!(asset.status, 200, "{path}");
        assert!(asset.header("content-type").unwrap().starts_with(kind));
        assert!(!asset.body.is_empty(), "{path}");
    }
    assert_eq!(get(&w.addr, "/account/assets/missing.js", None).status, 404);
}

#[test]
fn the_api_needs_a_session_and_the_csrf_header_to_write() {
    let w = world(true, &[]);
    let cookie = sign_in(&w.addr);
    let csrf = view(&w, &cookie)["csrf"].as_str().unwrap().to_owned();
    let body = json!({"value": TOKEN}).to_string();
    assert_eq!(get(&w.addr, API, None).status, 401);
    assert_eq!(put(&w, None, Some(&csrf), "claude", &body).status, 401);
    for csrf in [None, Some("forged")] {
        let reply = put(&w, Some(&cookie), csrf, "claude", &body);
        assert_eq!(reply.status, 403, "{}", reply.body);
    }
    assert!(
        !w.cmds().contains(&"secret-set".to_owned()),
        "{:?}",
        w.cmds()
    );
}

#[test]
fn the_view_lists_secrets_machines_and_the_connect_line_and_never_a_value() {
    let w = world(true, &[("github", "ghp_alreadyStored")]);
    let cookie = sign_in(&w.addr);
    let v = view(&w, &cookie);
    assert_eq!(v["email"], "alice@example.com");
    assert_eq!(
        v["secrets"],
        json!([{"name": "claude", "linked": false}, {"name": "github", "linked": true}])
    );
    assert_eq!(v["machines"]["nodes"][0]["name"], "hv");
    assert_eq!(v["machines"]["nodes"][0]["max_boxes"], 4);
    assert_eq!(v["connect"]["mcp_url"], format!("http://{}/mcp", w.addr));
    assert!(!v.to_string().contains("alreadyStored"), "{v}");
}

#[test]
fn saving_stores_the_value_for_the_signed_in_person_only() {
    let w = world(true, &[]);
    let alice = as_user(&w, "alice-sub");
    let bob = as_user(&w, "bob-sub");
    let reply = save(&w, &alice, "claude", TOKEN);
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert_eq!(
        serde_json::from_str::<Value>(&reply.body).unwrap(),
        json!({"name": "claude", "linked": true})
    );
    assert!(!reply.body.contains("s3cret"), "{}", reply.body);
    assert_eq!(view(&w, &alice)["secrets"][0]["linked"], true);
    assert_eq!(view(&w, &bob)["secrets"][0]["linked"], false);
    let vault = w.vault.lock().unwrap().clone();
    let theirs = |who: &str| vault.iter().find(|(k, _)| k.contains(who)).map(|(_, v)| v);
    assert_eq!(theirs("alice-sub").unwrap()["claude"], TOKEN);
    assert!(!theirs("bob-sub").unwrap().contains_key("claude"));
    let logged = format!("{:?}", w.seen.lock().unwrap());
    assert!(!logged.contains("s3cret"), "{logged}");
}

#[test]
fn a_session_cannot_write_with_another_sessions_csrf_token() {
    let w = world(true, &[]);
    let alice = as_user(&w, "alice-sub");
    let bob = as_user(&w, "bob-sub");
    let bobs_csrf = view(&w, &bob)["csrf"].as_str().unwrap().to_owned();
    let body = json!({"value": TOKEN}).to_string();
    let reply = put(&w, Some(&alice), Some(&bobs_csrf), "claude", &body);
    assert_eq!(reply.status, 403, "{}", reply.body);
    assert!(
        !w.cmds().contains(&"secret-set".to_owned()),
        "{:?}",
        w.cmds()
    );
}

#[test]
fn a_value_that_cannot_be_the_secret_is_refused_without_storing() {
    let w = world(true, &[]);
    let cookie = sign_in(&w.addr);
    for (name, value) in [
        ("claude", "not-a-claude-token"),
        ("claude", "sk-ant-has space"),
        ("github", ""),
        ("github", "two words"),
        ("Not-A-Name", TOKEN),
    ] {
        let reply = save(&w, &cookie, name, value);
        assert_eq!(reply.status, 400, "{name} {value}: {}", reply.body);
        assert!(
            !reply.body.contains(value) || value.is_empty(),
            "{}",
            reply.body
        );
    }
    let bad = put(
        &w,
        Some(&cookie),
        view(&w, &cookie)["csrf"].as_str(),
        "claude",
        "{",
    );
    assert_eq!(bad.status, 400, "{}", bad.body);
    assert!(
        !w.cmds().contains(&"secret-set".to_owned()),
        "{:?}",
        w.cmds()
    );
}

#[test]
fn the_account_page_is_not_found_without_browser_sign_in() {
    let w = world(false, &[]);
    assert_eq!(get(&w.addr, "/", None).status, 404);
    assert_eq!(get(&w.addr, API, None).status, 404);
}
