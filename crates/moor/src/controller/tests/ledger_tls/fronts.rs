//! Remote fronts on the ledger's TLS listener: a front presenting its own key is served as on
//! that front's socket, and any other key, namespace or hello is refused.
#![allow(clippy::panic, reason = "test assertions")]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use moor_client::tls::TlsPki;
use moor_client::{Client, ClientError};
use moor_core::request::Request;
use serde_json::Value;

use super::{env_with, pki, runner_asks, serve, start, started_with, tls_config, KEY, SERVICE_KEY};

const FRONT_KEY: &str = "herdr-front-key";

/// A controller with `herdr` as a remote front and `notify` as a front on its socket.
fn serving_fronts(dir: &tempfile::TempDir) -> (SocketAddr, PathBuf) {
    let key_file = dir.path().join("herdr-front.key");
    std::fs::write(&key_file, format!("{FRONT_KEY}\n")).unwrap();
    let fronts = format!(
        "fronts:\n  herdr:\n    remote:\n      key_file: {}\n  notify:\n    webhooks: true\n",
        key_file.display()
    );
    let keys = [
        ("MOOR_EGRESS_KEY", KEY),
        ("MOOR_MACHINE_SERVICE_KEY", SERVICE_KEY),
    ];
    let (running, addr, ca) = started_with(dir, &fronts, &keys);
    serve(running);
    (addr, ca)
}

/// What front `ns` presenting `key` gets for `request`, as JSON.
fn front_asks(
    (addr, ca): (SocketAddr, &Path),
    (ns, key): (&str, &str),
    request: &str,
) -> Result<Value, ClientError> {
    let dir = ca.parent().expect("pki dir");
    let files = TlsPki {
        ca: ca.to_owned(),
        cert: dir.join("ctl-client.pem"),
        key: dir.join("ctl-client.key"),
    };
    let client = Client::front(&format!("tls://{addr}"), &files, ns, key).expect("client");
    let request: Request = serde_json::from_str(request).expect("request");
    client
        .request(request, |_| {})
        .map(|r| serde_json::to_value(r).expect("encode"))
}

/// Asserts `got` is the refusal naming `why`.
fn refused_for(got: &Value, why: &str) {
    let message = got["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains(why), "{got}");
}

#[test]
fn a_front_with_its_key_acts_for_its_own_identities_over_tls() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, ca) = serving_fronts(&dir);
    let got = front_asks(
        (addr, &ca),
        ("herdr", FRONT_KEY),
        r#"{"cmd":"as","identity":"herdr:T1:U1","request":{"cmd":"show"}}"#,
    )
    .expect("answered");
    assert_eq!(got["class"], "ok", "{got}");
}

#[test]
fn a_front_key_holder_over_tls_is_never_the_operator() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, ca) = serving_fronts(&dir);
    let front = ("herdr", FRONT_KEY);
    let bare = front_asks((addr, &ca), front, r#"{"cmd":"show"}"#).expect("answered");
    refused_for(&bare, "must assert an identity");
    let other = front_asks(
        (addr, &ca),
        front,
        r#"{"cmd":"as","identity":"authgear:someone","request":{"cmd":"show"}}"#,
    )
    .expect("answered");
    refused_for(&other, "may assert only herdr:");
}

#[test]
fn a_hello_is_refused_unless_its_key_is_that_fronts() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, ca) = serving_fronts(&dir);
    let show = r#"{"cmd":"show"}"#;
    for (front, why) in [
        (("herdr", "wrong"), "wrong key"),
        (("herdr", SERVICE_KEY), "the runner's key"),
        (("herdr", KEY), "the egress key"),
        (("notify", FRONT_KEY), "another front's namespace"),
        (("nobody", FRONT_KEY), "no such front"),
    ] {
        match front_asks((addr, &ca), front, show) {
            Err(ClientError::Unauthorized(said)) => {
                assert!(said.contains("not the key of front"), "{why}: {said}");
            }
            other => panic!("{why} was served: {other:?}"),
        }
    }
}

#[test]
fn a_front_is_ready_with_its_key_and_refused_at_once_without_it() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, ca) = serving_fronts(&dir);
    let files = TlsPki {
        ca,
        cert: dir.path().join("ctl-client.pem"),
        key: dir.path().join("ctl-client.key"),
    };
    let front = |key| Client::front(&format!("tls://{addr}"), &files, "herdr", key).unwrap();
    front(FRONT_KEY).ready().unwrap();
    assert!(matches!(
        front("not-the-key").ready(),
        Err(ClientError::Unauthorized(_))
    ));
}

#[test]
fn a_front_key_is_not_a_runner_key() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, ca) = serving_fronts(&dir);
    match runner_asks(addr, &ca, FRONT_KEY, r#"{"cmd":"show"}"#) {
        Err(ClientError::Unauthorized(said)) => assert!(said.contains("not the machine"), "{said}"),
        other => panic!("a front key served as a runner: {other:?}"),
    }
}

#[test]
fn a_missing_or_empty_front_key_file_stops_the_start() {
    let dir = tempfile::tempdir().unwrap();
    pki(dir.path(), "ctl", true);
    let key_file = dir.path().join("herdr-front.key");
    let fronts = format!(
        "fronts:\n  herdr:\n    remote:\n      key_file: {}\n",
        key_file.display()
    );
    let cfg = tls_config(dir.path(), &fronts);
    let keys = [("MOOR_EGRESS_KEY", KEY)];
    let Err(stop) = start(&env_with(dir.path(), &keys), &cfg) else {
        panic!("started without the front's key file");
    };
    assert!(
        stop.message.contains("fronts.herdr.remote.key_file"),
        "{}",
        stop.message
    );
    std::fs::write(&key_file, "\n").unwrap();
    let Err(stop) = start(&env_with(dir.path(), &keys), &cfg) else {
        panic!("started with an empty front key");
    };
    assert!(stop.message.contains("is empty"), "{}", stop.message);
}
