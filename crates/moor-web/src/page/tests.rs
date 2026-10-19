//! Tests for the static page routes and guards on the shipped page files.
#![allow(
    clippy::string_slice,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test scans slice at ASCII tag boundaries and unwrap I/O"
)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use super::*;
use crate::http::{self, Server};

const TOKEN: &str = "page-test-token";

fn server() -> Server {
    http::start(0, TOKEN.into(), handler(http::placeholder_handler())).unwrap()
}

/// Fetches a whole response (the server closes after it).
fn get(web: &Server, path: &str, headers: &[&str]) -> String {
    let mut s = TcpStream::connect(web.addr()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(s, "GET {path} HTTP/1.1\r\nHost: {}\r\n", web.addr()).unwrap();
    for h in headers {
        write!(s, "{h}\r\n").unwrap();
    }
    write!(s, "\r\n").unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    out
}

fn bearer() -> String {
    format!("Authorization: Bearer {TOKEN}")
}

#[test]
fn root_serves_the_page() {
    let page = get(&server(), "/", &[&bearer()]);
    assert!(page.starts_with("HTTP/1.1 200"));
    assert!(page.contains("Content-Type: text/html; charset=utf-8"));
    assert!(page.contains("Cache-Control: no-store"));
    assert!(page.contains("Content-Security-Policy: default-src 'none'; script-src 'self'"));
    assert!(page.ends_with(INDEX_HTML));
}

#[test]
fn script_is_served_as_javascript() {
    let js = get(&server(), "/app.js", &[&bearer()]);
    assert!(js.starts_with("HTTP/1.1 200"));
    assert!(js.contains("Content-Type: text/javascript; charset=utf-8"));
    assert!(js.contains("Cache-Control: no-store"));
    assert!(js.contains("Content-Security-Policy: default-src 'none'; script-src 'self'"));
    assert!(js.ends_with(APP_JS));
}

#[test]
fn script_needs_credentials() {
    assert!(get(&server(), "/app.js", &[]).starts_with("HTTP/1.1 401"));
}

#[test]
fn query_token_on_root_sets_cookie() {
    let page = get(&server(), &format!("/?token={TOKEN}"), &[]);
    assert!(page.starts_with("HTTP/1.1 200"));
    assert!(page.contains("Set-Cookie: "));
}

#[test]
fn other_paths_fall_back() {
    let web = server();
    assert!(get(&web, "/other", &[&bearer()]).starts_with("HTTP/1.1 404"));
    assert!(get(&web, "/app.js/x", &[&bearer()]).starts_with("HTTP/1.1 404"));
}

/// Every occurrence of an HTML element start such as `<button`, case-insensitive.
fn tags<'a>(html: &'a str, name: &str) -> Vec<&'a str> {
    let lower = html.to_ascii_lowercase();
    let open = format!("<{name}");
    lower
        .match_indices(&open)
        .filter(|(i, _)| {
            html[i + open.len()..]
                .chars()
                .next()
                .is_some_and(|c| c.is_whitespace() || c == '>' || c == '/')
        })
        .map(|(i, _)| html[i..].split('>').next().unwrap_or_default())
        .collect()
}

#[test]
fn html_loads_the_script_from_self_with_no_inline_code() {
    assert!(INDEX_HTML.contains("<script src=\"/app.js\""));
    let lower = INDEX_HTML.to_ascii_lowercase();
    for open in lower.match_indices("<script") {
        let rest = &lower[open.0..];
        let end = rest.find("</script>").unwrap();
        let head_end = rest.find('>').unwrap();
        assert!(
            rest[head_end + 1..end].trim().is_empty(),
            "inline script body"
        );
        assert!(rest[..head_end].contains(" src="), "script without src");
    }
}

#[test]
fn html_has_no_handler_attributes_or_forms() {
    let lower = INDEX_HTML.to_ascii_lowercase();
    assert!(!lower.contains("<form"), "form element");
    for tag in lower.split('<').skip(1) {
        let head = tag.split('>').next().unwrap_or_default();
        for word in head.split_whitespace() {
            assert!(
                !(word.starts_with("on") && word.contains('=')),
                "handler attribute: {word}"
            );
        }
    }
}

#[test]
fn html_controls_are_labelled() {
    for name in ["button", "input", "select", "textarea"] {
        for tag in tags(INDEX_HTML, name) {
            let id = tag.split("id=\"").nth(1).and_then(|r| r.split('"').next());
            let by_label = id.is_some_and(|id| INDEX_HTML.contains(&format!("for=\"{id}\"")));
            assert!(
                tag.contains("aria-label=") || by_label,
                "unlabelled control: {tag}"
            );
        }
    }
}

#[test]
fn script_avoids_dynamic_code_and_markup_injection() {
    for banned in [
        "eval(",
        "new Function",
        "innerHTML",
        "outerHTML",
        "insertAdjacentHTML",
        "document.write",
    ] {
        assert!(!APP_JS.contains(banned), "script uses {banned}");
    }
}

#[test]
fn script_creates_controls_only_through_the_helper() {
    let start = APP_JS.find("function control(").expect("no control helper");
    let end = APP_JS[start + 1..]
        .find("\nfunction ")
        .map_or(APP_JS.len(), |n| start + 1 + n);
    let helper = &APP_JS[start..end];
    assert!(helper.contains("aria-label"), "helper must set aria-label");
    for tag in ["button", "input", "select", "textarea"] {
        for quote in ['\'', '"'] {
            let call = format!("createElement({quote}{tag}{quote})");
            assert_eq!(
                APP_JS.matches(&call).count(),
                helper.matches(&call).count(),
                "{call} outside control()"
            );
        }
    }
}

#[test]
fn script_shows_artifacts_per_box_and_reloads_on_publish() {
    assert!(APP_JS.contains("'Artifacts'"), "panel label");
    assert!(APP_JS.contains("No artifacts yet."), "empty text");
    assert!(APP_JS.contains("'artifact_published'"), "SSE kind");
    assert!(
        APP_JS.contains("'/boxes/' + encodeURIComponent(box) + '/artifacts'"),
        "list url"
    );
    assert!(
        APP_JS.contains("encodeURIComponent(f.name)"),
        "download url name"
    );
}
