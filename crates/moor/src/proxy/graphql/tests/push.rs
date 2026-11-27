//! The push guard on the git host: a push moves only the Box's own branch.

use std::sync::Arc;

use nono_proxy::tls_intercept::EphemeralCa;

use super::{fixed, Client, Guard, Responder, Up, Upstream, Wire, RULE};
use crate::proxy::graphql::{FilterSpec, GitSpec, GraphqlFilter};
use crate::proxy::push::{Owned, PushRefusal};

/// An object id.
const A: &str = "1111111111111111111111111111111111111111";
/// Another object id.
const B: &str = "2222222222222222222222222222222222222222";
/// The id a deletion moves a ref to.
const ZERO: &str = "0000000000000000000000000000000000000000";
/// The Box's branch.
const MINE: &str = "refs/heads/moor/12-fix";
/// Where the Box pushes.
const PATH: &str = "/acme/widgets.git/git-receive-pack";
/// What the fake git host answers a push it took.
const TOOK: &[u8] = b"0030\x01000eunpack ok\n0019ok refs/heads/moor/12-fix\n00000000";

/// The filter with the push guard on `github.com`, for a Box that owns [`MINE`].
struct GitRig {
    /// The filter.
    filter: GraphqlFilter,
    /// The fake git host behind it.
    up: Up,
    /// PEM of the Box CA the client trusts.
    ca_pem: String,
}

impl GitRig {
    /// Starts the filter; the fake git host answers every request with `f`.
    fn start(f: Responder) -> Self {
        let ca = EphemeralCa::generate().unwrap();
        let ca_pem = ca.cert_pem().to_owned();
        let up = Up::start(f);
        let to = |host: &str| Upstream::new(host, 443, Some(up.addr), Some(&up.ca)).unwrap();
        let filter = GraphqlFilter::start(FilterSpec {
            ca,
            repos: vec![super::box_repo()],
            host: "api.github.com".to_owned(),
            port: 443,
            rule: RULE.to_owned(),
            credential_headers: Vec::new(),
            placeholder: None,
            upstream: to("api.github.com"),
            guard: Guard::default(),
            rest: Vec::new(),
            scope: moor_core::config::proxy::default_graphql(),
            git: Some(GitSpec {
                host: "github.com".to_owned(),
                upstream: to("github.com"),
                owned: Owned::branch(Some("moor/12-fix")),
            }),
        })
        .unwrap();
        Self { filter, up, ca_pem }
    }

    /// A tunnel to `github.com:443` as nono opens one.
    fn client(&self) -> Client {
        Client::open(self.filter.addr(), &self.ca_pem, "github.com:443").unwrap()
    }

    /// Requests the git host received.
    fn seen(&self) -> Vec<Wire> {
        self.up.seen.lock().unwrap().clone()
    }

    /// The pushes refused, waiting a little for the first.
    fn refused(&self) -> Vec<PushRefusal> {
        for _ in 0..50 {
            let got = self.filter.drain_pushes();
            if !got.is_empty() {
                return got;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        Vec::new()
    }
}

/// The git host's answer to a push it took.
fn took() -> Responder {
    let mut reply = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/x-git-receive-pack-result\r\n\
         content-length: {}\r\n\r\n",
        TOOK.len()
    )
    .into_bytes();
    reply.extend_from_slice(TOOK);
    fixed(reply)
}

/// One pkt-line holding `data`.
fn pkt(data: &str) -> String {
    format!("{:04x}{data}", data.len() + 4)
}

/// A push body: the command list for `updates`, then a pack.
fn body(updates: &[(&str, &str, &str)]) -> Vec<u8> {
    let mut out = String::new();
    for (i, (old, new, name)) in updates.iter().enumerate() {
        let caps = if i == 0 {
            "\0report-status side-band-64k"
        } else {
            ""
        };
        out.push_str(&pkt(&format!("{old} {new} {name}{caps}\n")));
    }
    out.push_str("0000PACK\x00\x00\x00\x02the-pack-bytes");
    out.into_bytes()
}

/// A push request with a length and the credential nono injected.
fn push(payload: &[u8], extra: &str) -> Vec<u8> {
    let mut out = format!(
        "POST {PATH} HTTP/1.1\r\nhost: github.com\r\nauthorization: Basic eC1hY2Nlc3M=\r\n\
         content-type: application/x-git-receive-pack-request\r\n{extra}content-length: {}\r\n\r\n",
        payload.len()
    )
    .into_bytes();
    out.extend_from_slice(payload);
    out
}

/// The same push sent chunked in pieces of `size` bytes.
fn chunked_push(payload: &[u8], size: usize) -> Vec<u8> {
    let mut out = format!(
        "POST {PATH} HTTP/1.1\r\nhost: github.com\r\n\
         content-type: application/x-git-receive-pack-request\r\ntransfer-encoding: chunked\r\n\r\n"
    )
    .into_bytes();
    for piece in payload.chunks(size) {
        out.extend_from_slice(format!("{:x}\r\n", piece.len()).as_bytes());
        out.extend_from_slice(piece);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"0\r\n\r\n");
    out
}

#[test]
fn a_push_to_the_box_branch_reaches_the_git_host_byte_for_byte() {
    let rig = GitRig::start(took());
    let request = push(&body(&[(A, B, MINE)]), "");
    let resp = rig.client().call(&request);
    assert_eq!(resp.status(), 200, "{}", resp.head);
    assert_eq!(resp.body, TOOK);
    let seen = rig.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen.first().unwrap().raw, request);
    assert!(rig.filter.drain_pushes().is_empty());
}

#[test]
fn a_chunked_push_to_the_box_branch_keeps_its_payload() {
    let rig = GitRig::start(took());
    let payload = body(&[(A, B, MINE)]);
    for size in [3, 7, 64, 4096] {
        let resp = rig.client().call(&chunked_push(&payload, size));
        assert_eq!(resp.status(), 200, "{size}: {}", resp.head);
        assert_eq!(rig.seen().last().unwrap().body, payload, "{size}");
    }
}

/// Sends a push of the Box's branch and `main` together.
fn push_with_main(rig: &GitRig) -> Wire {
    rig.client()
        .call(&push(&body(&[(A, B, MINE), (A, B, "refs/heads/main")]), ""))
}

#[test]
fn a_push_to_the_default_branch_is_refused_as_git_shows_a_hook_refusal() {
    let rig = GitRig::start(took());
    let resp = push_with_main(&rig);
    assert_eq!(resp.status(), 200, "{}", resp.head);
    assert_eq!(
        resp.header("content-type"),
        Some("application/x-git-receive-pack-result")
    );
    let text = String::from_utf8_lossy(&resp.body).into_owned();
    assert!(
        text.contains("ng refs/heads/main moor refuses a push to refs/heads/main"),
        "{text}"
    );
    assert!(text.contains("ng refs/heads/moor/12-fix "), "{text}");
    assert!(text.contains("(contract: branch)"), "{text}");
    assert!(rig.seen().is_empty(), "the git host got the push");
}

#[test]
fn a_refused_push_is_recorded_with_its_repo_and_ref() {
    let rig = GitRig::start(took());
    push_with_main(&rig);
    let refused = rig.refused();
    assert_eq!(refused.len(), 1);
    let first = refused.first().unwrap();
    assert_eq!(first.repo, "acme/widgets");
    assert_eq!(first.git_ref.as_deref(), Some("refs/heads/main"));
    assert_eq!(first.host, "github.com");
}

#[test]
fn deleting_a_ref_is_refused() {
    let rig = GitRig::start(took());
    let resp = rig
        .client()
        .call(&chunked_push(&body(&[(A, ZERO, MINE)]), 5));
    let text = String::from_utf8_lossy(&resp.body).into_owned();
    assert!(
        text.contains(&format!("ng {MINE} moor refuses to delete {MINE}")),
        "{text}"
    );
    assert!(rig.seen().is_empty());
    assert_eq!(
        rig.refused().first().unwrap().git_ref.as_deref(),
        Some(MINE)
    );
}

#[test]
fn a_push_moor_cannot_read_is_refused_not_passed() {
    let rig = GitRig::start(took());
    let gzip = push(&body(&[(A, B, MINE)]), "content-encoding: gzip\r\n");
    let garbage = push(b"not a pkt-line at all", "");
    for request in [gzip, garbage] {
        let resp = rig.client().call(&request);
        assert_eq!(resp.status(), 403, "{}", resp.head);
        assert!(resp.message().unwrap().contains("moor refuses this push"));
        let refused = rig.refused();
        assert_eq!(refused.first().unwrap().git_ref, None);
    }
    assert!(rig.seen().is_empty());
}

#[test]
fn every_spelling_of_the_push_path_meets_the_guard() {
    let rig = GitRig::start(took());
    for path in [
        "/Acme/Widgets.git/Git-Receive-Pack",
        "/acme/widgets/git-receive-pack/",
        "/acme/widgets/%67it-receive-pack",
    ] {
        let request = String::from_utf8(push(&body(&[(A, B, "refs/heads/main")]), ""))
            .unwrap()
            .replacen(PATH, path, 1);
        let resp = rig.client().call(request.as_bytes());
        assert!(
            String::from_utf8_lossy(&resp.body).contains("ng refs/heads/main"),
            "{path}"
        );
    }
    assert!(rig.seen().is_empty());
}

#[test]
fn other_git_requests_pass_untouched() {
    let rig = GitRig::start(Arc::new(|_: &Wire| {
        b"HTTP/1.1 200 OK\r\ncontent-length: 4\r\n\r\n0000".to_vec()
    }));
    let request = b"GET /acme/widgets.git/info/refs?service=git-receive-pack HTTP/1.1\r\n\
        host: github.com\r\n\r\n";
    let resp = rig.client().call(request);
    assert_eq!(resp.body, b"0000");
    assert_eq!(rig.seen().first().unwrap().raw, request);
}
