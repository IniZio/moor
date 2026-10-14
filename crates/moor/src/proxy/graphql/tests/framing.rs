//! Tunnel target, path canonicality, body limits, passthrough and connection handling.

use std::sync::mpsc;

use super::*;
use crate::proxy::graphql::{EGRESS_RULE, GUARD_CATEGORY, SCOPE_CATEGORY};

/// Largest GraphQL body the filter accepts.
const CAP: usize = 64 * 1024;

#[test]
fn query_string_and_non_canonical_paths_are_refused() {
    let rig = rig(fixed(json_reply("200 OK", OK_BODY)));
    let body = repo_query("IniZio", "moor", "Q");
    let paths = [
        "/graphql?query=%7Bviewer%7Blogin%7D%7D",
        "/graphql?",
        "/graphql/",
        "//graphql",
        "/GraphQL",
        "/%67raphql",
        "/graphql/../graphql",
        "/graphql/.",
        "/./graphql",
    ];
    for path in paths {
        let resp = rig.client().call(&post(path, &body));
        assert_refused(&resp, 403);
    }
    assert!(rig.requests().is_empty(), "no variant may reach upstream");
}

#[test]
fn oversize_body_by_content_length_is_refused() {
    let rig = rig(fixed(json_reply("200 OK", OK_BODY)));
    let mut client = rig.client();
    let _ = client.send(&request("POST", "/graphql", &vec![b' '; CAP + 1]));
    let resp = client.recv().expect("413 response");
    assert_refused(&resp, 413);
    assert!(rig.requests().is_empty());
}

#[test]
fn oversize_body_by_chunks_is_refused() {
    let rig = rig(fixed(json_reply("200 OK", OK_BODY)));
    let mut client = rig.client();
    let chunk = vec![b' '; CAP / 4];
    let chunks: Vec<&[u8]> = vec![&chunk, &chunk, &chunk, &chunk, b" "];
    let _ = client.send(&chunked_post("/graphql", &chunks));
    let resp = client.recv().expect("413 response");
    assert_refused(&resp, 413);
    assert!(rig.requests().is_empty());
}

#[test]
fn chunked_body_within_cap_is_judged_and_forwarded_verbatim() {
    let rig = rig(fixed(json_reply("200 OK", OK_BODY)));
    let body = repo_query("IniZio", "moor", "Q");
    let parts = halves(&body);
    let sent = chunked_post("/graphql", &parts);
    let resp = rig.client().call(&sent);
    assert_eq!(resp.status(), 200);
    let seen = rig.requests();
    assert!(
        seen.first().is_some_and(|w| w.raw == sent),
        "chunked bytes changed"
    );
}

#[test]
fn chunked_other_repo_query_is_refused() {
    let rig = rig(fixed(json_reply("200 OK", OK_BODY)));
    let body = repo_query("victim", "secrets", "Q");
    let parts = halves(&body);
    let resp = rig.client().call(&chunked_post("/graphql", &parts));
    assert_refused(&resp, 403);
    assert!(rig.requests().is_empty());
}

#[test]
fn other_paths_pass_through_unchanged() {
    let reply =
        b"HTTP/1.1 200 OK\r\nX-GitHub-Request-Id: AB12\r\ncontent-type: application/json\r\n\
        content-length: 8\r\n\r\n[{\"a\":1}"
            .to_vec();
    let rig = rig(fixed(reply.clone()));
    let mut client = rig.client();
    let get = format!(
        "GET /repos/o/r/pulls?state=open HTTP/1.1\r\nHost: api.github.com\r\n\
         Authorization: token {TOKEN}\r\nAccept: application/vnd.github+json\r\n\r\n"
    )
    .into_bytes();
    let resp = client.call(&get);
    assert!(resp.raw == reply, "reply bytes changed");
    let post = request("POST", "/repos/o/r/issues", br#"{"title":"hello"}"#);
    let resp = client.call(&post);
    assert!(resp.raw == reply, "reply bytes changed");
    let seen = rig.requests();
    assert_eq!(seen.len(), 2);
    assert!(
        seen.first().is_some_and(|w| w.raw == get),
        "GET bytes changed"
    );
    assert!(
        seen.get(1).is_some_and(|w| w.raw == post),
        "POST bytes changed"
    );
    assert!(rig.refusals(0).is_empty());
}

#[test]
fn connect_to_another_target_is_refused_and_recorded() {
    let rig = rig(fixed(json_reply("200 OK", OK_BODY)));
    let cases = [
        ("api.github.com:80", SCOPE_CATEGORY, RULE),
        ("api.github.com:not-a-port", SCOPE_CATEGORY, RULE),
        ("api.github.com", SCOPE_CATEGORY, RULE),
        ("127.0.0.1:443", GUARD_CATEGORY, EGRESS_RULE),
        ("[::1]:443", GUARD_CATEGORY, EGRESS_RULE),
        ("localhost:8080", GUARD_CATEGORY, EGRESS_RULE),
        ("10.0.0.1:443", GUARD_CATEGORY, EGRESS_RULE),
        ("169.254.169.254:80", GUARD_CATEGORY, EGRESS_RULE),
        ("nonexistent.invalid:443", GUARD_CATEGORY, EGRESS_RULE),
    ];
    for (target, category, rule) in cases {
        let (_tcp, status) = raw_connect(rig.filter.addr(), target).unwrap();
        assert_eq!(status, 403, "{target}");
        let got = rig.refusals(1);
        assert_eq!(got.len(), 1, "{target}: {got:?}");
        let r = got.first().unwrap();
        assert_eq!(r.category, category, "{target}");
        assert_eq!(r.rule, rule, "{target}");
        assert_eq!(r.method, "CONNECT", "{target}");
    }
    assert!(rig.requests().is_empty());
}

#[test]
fn keep_alive_serves_allowed_requests_then_closes_on_a_denied_one() {
    let rig = rig(fixed(json_reply("200 OK", OK_BODY)));
    let mut client = rig.client();
    let allowed = post("/graphql", &repo_query("IniZio", "moor", "Q"));
    for _ in 0..2 {
        assert_eq!(client.call(&allowed).status(), 200);
    }
    let denied = client.call(&post("/graphql", &repo_query("victim", "secrets", "Q")));
    assert_refused(&denied, 403);
    assert!(client.closed(), "connection must close after a refusal");
    assert_eq!(
        rig.requests().len(),
        2,
        "denied request must not go upstream"
    );
}

#[test]
fn keep_alive_mixes_graphql_and_other_paths() {
    let rig = rig(fixed(json_reply("200 OK", OK_BODY)));
    let mut client = rig.client();
    let gql = post("/graphql", &repo_query("IniZio", "moor", "Q"));
    let other = request("POST", "/repos/o/r/issues", b"{}");
    for req in [&gql, &other, &gql] {
        assert_eq!(client.call(req).status(), 200);
    }
    assert_eq!(rig.requests().len(), 3);
}

/// Splits `body` in two chunks.
fn halves(body: &str) -> Vec<&[u8]> {
    let (a, b) = body.as_bytes().split_at(10);
    vec![a, b]
}

/// Stops `filter` on another thread and fails if it does not return in time.
fn stop_promptly(filter: GraphqlFilter) {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        filter.stop();
        let _ = tx.send(());
    });
    rx.recv_timeout(Duration::from_secs(5))
        .expect("stop must return promptly");
}

#[test]
fn stop_returns_promptly_and_port_stops_accepting() {
    let Rig { filter, up, .. } = rig(fixed(json_reply("200 OK", OK_BODY)));
    let addr = filter.addr();
    stop_promptly(filter);
    assert!(
        TcpStream::connect_timeout(&addr, Duration::from_secs(1)).is_err(),
        "port still accepting"
    );
    drop(up);
}

#[test]
fn stop_returns_promptly_with_an_idle_tunnel_open() {
    let rig = rig(fixed(json_reply("200 OK", OK_BODY)));
    let mut client = rig.client();
    assert_eq!(
        client
            .call(&post("/graphql", &repo_query("IniZio", "moor", "Q")))
            .status(),
        200
    );
    let Rig { filter, up, .. } = rig;
    let addr = filter.addr();
    stop_promptly(filter);
    assert!(TcpStream::connect_timeout(&addr, Duration::from_secs(1)).is_err());
    drop(up);
}
