//! The front listener passes `CONNECT` and absolute-form `http://` requests, the latter as a `CONNECT`.
//! Origin-form requests never reach nono or any upstream, and a plain request never carries a credential.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::time::Duration;

use moor_core::config::proxy::{EndpointRule, ResolvedInject, ResolvedRule};

use super::graphql_e2e::{rig_with, Rig, SECRET};
use crate::proxy::nono::tests::upstream::{self, Upstream};

/// The body the front refuses an origin-form or `https://` request with.
pub(super) const REFUSAL: &str =
    r#"{"message":"moor: only CONNECT or http:// requests are allowed"}"#;
/// A GraphQL query for a repository that is not the Box's.
const OTHER_REPO: &str = r#"{"query":"query{repository(owner:\"victim\",name:\"secrets\"){id}}"}"#;

/// Rig plus fake upstreams for the two hosts nono dials itself.
struct Front {
    /// The supervisor listener and the fake `api.github.com`.
    rig: Rig,
    /// Fake host of the `claude` rule.
    claude: Upstream,
    /// Fake host of the `github-git` rule.
    git: Upstream,
}

/// A header-injecting rule for `localhost:<port>`.
fn rule(name: &str, port: u16, form: &str) -> ResolvedRule {
    ResolvedRule {
        name: name.to_owned(),
        hosts: vec![format!("localhost:{port}")],
        credential: "TOOL_TOKEN".to_owned(),
        inject: ResolvedInject::Header {
            header: "authorization".to_owned(),
            form: form.to_owned(),
        },
        endpoint_rules: vec![EndpointRule {
            method: "POST".to_owned(),
            path: "/v1/messages".to_owned(),
        }],
        box_env: BTreeMap::new(),
    }
}

/// Starts the supervisor with GitHub, `claude` and `github-git` rules, each host a recording fake.
fn front() -> Front {
    let dir = tempfile::tempdir().unwrap();
    let claude = upstream::start(dir.path());
    let git = claude.twin();
    let rules = vec![
        rule("claude", claude.port, "Bearer {secret}"),
        rule("github-git", git.port, "Basic {secret}"),
    ];
    Front {
        rig: rig_with(rules),
        claude,
        git,
    }
}

/// What the front answered.
struct Reply {
    /// Status code.
    status: u16,
    /// Response body.
    body: String,
}

/// Sends `raw` to `port` and reads the answer until the peer closes or goes quiet.
fn send(port: u16, raw: &str) -> Reply {
    let mut tcp = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    tcp.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
    tcp.write_all(raw.as_bytes()).unwrap();
    let mut got = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        match tcp.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => got.extend_from_slice(buf.get(..n).unwrap_or_default()),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => break,
            Err(e) if e.kind() == ErrorKind::ConnectionReset => break,
            Err(e) => unreachable!("read failed: {e}"),
        }
    }
    let text = String::from_utf8_lossy(&got).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    Reply {
        status,
        body: body.to_owned(),
    }
}

/// A request with the given target and headers, closing after the answer.
fn request(method: &str, target: &str, host: &str, extra: &[String], body: &str) -> String {
    let mut out = format!("{method} {target} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    for h in extra {
        out.push_str(h);
        out.push_str("\r\n");
    }
    let _ = write!(
        out,
        "Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    out
}

/// The credential header variants a sandbox client can send.
fn credentials() -> Vec<Vec<String>> {
    let (proxy, auth) = (
        "Proxy-Authorization: Basic bW9vcjp4",
        "Authorization: Bearer x",
    );
    vec![
        Vec::new(),
        vec![proxy.to_owned()],
        vec![auth.to_owned()],
        vec![proxy.to_owned(), auth.to_owned()],
    ]
}

impl Front {
    /// Sends `raw` into the Box port and expects the refusal.
    fn refused(&self, raw: &str) {
        let port = self.rig.box_port;
        let reply = send(port, raw);
        assert_eq!(reply.status, 403, "unexpected answer: {}", reply.body);
        assert_eq!(reply.body, REFUSAL);
        assert!(!reply.body.contains(SECRET));
    }

    /// Nothing was dialled upstream and nothing reached nono.
    fn assert_untouched(&self) {
        std::thread::sleep(Duration::from_millis(200));
        assert!(self.rig.seen().is_empty(), "api.github.com was reached");
        assert!(self.claude.seen().is_empty(), "claude host was reached");
        assert!(self.git.seen().is_empty(), "github-git host was reached");
        let rig = &self.rig;
        rig.live.as_ref().unwrap().drain(&rig.deps(), rig.box_id);
        let events = rig.store.events(rig.box_id, 100).unwrap();
        let noisy: Vec<_> = events
            .iter()
            .filter(|e| e.kind.contains("egress") || e.kind.contains("credential"))
            .map(|e| e.kind.as_str())
            .collect();
        assert!(noisy.is_empty(), "nono audited {noisy:?}");
    }

    /// Stops the supervisor listeners.
    fn stop(mut self) {
        self.rig.live.take().unwrap().stop();
    }
}

#[test]
fn origin_form_graphql_route_is_refused_before_nono() {
    let f = front();
    for extra in credentials() {
        f.refused(&request(
            "POST",
            "/github-api__0/graphql",
            "api.github.com",
            &extra,
            OTHER_REPO,
        ));
    }
    f.assert_untouched();
    f.stop();
}

#[test]
fn origin_form_claude_route_is_refused_before_nono() {
    let f = front();
    for extra in credentials() {
        f.refused(&request(
            "POST",
            "/claude__0/v1/messages",
            "api.anthropic.com",
            &extra,
            r#"{"model":"m"}"#,
        ));
    }
    f.assert_untouched();
    f.stop();
}

#[test]
fn origin_form_git_route_is_refused_before_nono() {
    let f = front();
    for extra in credentials() {
        f.refused(&request(
            "GET",
            "/github-git__0/info/refs?service=git-upload-pack",
            "github.com",
            &extra,
            "",
        ));
    }
    f.assert_untouched();
    f.stop();
}

#[test]
fn plain_request_to_the_nono_port_is_refused_by_the_guard() {
    let f = front();
    let nono = f.rig.live.as_ref().unwrap().nono_port();
    let port = f.rig.box_port;
    let target = format!("http://127.0.0.1:{nono}/github-api__0/graphql");
    let reply = send(
        port,
        &request("GET", &target, &format!("127.0.0.1:{nono}"), &[], ""),
    );
    assert_eq!(reply.status, 502, "{}", reply.body);
    assert!(f.rig.seen().is_empty(), "api.github.com was reached");
    f.stop();
}

#[test]
fn plain_request_to_a_brokered_host_carries_no_credential() {
    let f = front();
    let port = f.rig.box_port;
    let target = format!("http://localhost:{}/v1/messages", f.claude.port);
    let reply = send(port, &request("POST", &target, "localhost", &[], "{}"));
    assert!(!(200..300).contains(&reply.status), "{}", reply.body);
    for seen in f.claude.seen().iter().chain(f.git.seen().iter()) {
        assert!(seen.authorization.is_none(), "credential leaked: {seen:?}");
    }
    assert!(!reply.body.contains(SECRET));
    f.stop();
}
